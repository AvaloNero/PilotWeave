use super::{ResourceInput, ResourceOverview, ResourcePlan, SharedResource};
use crate::{
    adapters,
    commands::ManagedState,
    deployment,
    error::{AppError, AppResult},
    redact, transaction,
};
use tauri::State;

fn error(value: AppError) -> String {
    redact::redact_text(&value.to_string())
}
async fn job<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| {
            "Resource worker could not complete; review recovery before retrying".to_string()
        })?
        .map_err(error)
}

#[tauri::command]
pub async fn get_resources(state: State<'_, ManagedState>) -> Result<ResourceOverview, String> {
    let store = state.store().map_err(error)?;
    Ok(ResourceOverview { resources: store.resources().iter().filter(|record| !record.archived).cloned().collect(),
        bindings: store.resource_bindings().to_vec(),
        detail: "Publication metadata is not proof of client consumption. Preview rechecks ownership, target availability and drift".into() })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn upsert_resource(
    state: State<'_, ManagedState>,
    input: serde_json::Value,
) -> Result<SharedResource, String> {
    let state = state.inner().clone();
    job(move || {
        let input: ResourceInput = serde_json::from_value(input).map_err(|_| {
            AppError::InvalidInput("Resource input has unsupported fields or types".into())
        })?;
        let _write = state.writes.try_lock().map_err(|_| AppError::Busy)?;
        let mut store = state.store()?;
        let _lease = crate::write_lock::WriteLock::acquire(store.path())?;
        deployment::ensure_no_pending_journal(store.path())?;
        store.upsert_resource(input)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn preview_resource_sync(
    state: State<'_, ManagedState>,
    resource_id: String,
    revoke_and_delete: bool,
) -> Result<ResourcePlan, String> {
    let state = state.inner().clone();
    job(move || {
        let store = state.store()?;
        store.ensure_writable()?;
        deployment::ensure_no_pending_journal(store.path())?;
        let record = store.resource(&resource_id)?;
        state
            .resource_plans
            .lock()
            .map_err(|_| AppError::Lock)?
            .preview(
                &record,
                store.installation_owner_id(),
                store.revision()?,
                &adapters::discover_all(),
                store.resource_bindings(),
                revoke_and_delete,
            )
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn apply_resource_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
    confirmed: bool,
) -> Result<ResourcePlan, String> {
    let state = state.inner().clone();
    job(move || {
        let stored = state.resource_plans.lock().map_err(|_| AppError::Lock)?.consume(&plan_id)?;
        if !confirmed { return Err(AppError::InvalidInput("Explicit resource synchronization confirmation is required".into())); }
        let _write = state.writes.try_lock().map_err(|_| AppError::Busy)?;
        let mut store = state.store()?;
        let _lease = crate::write_lock::WriteLock::acquire(store.path())?;
        store.ensure_writable()?; deployment::ensure_no_pending_journal(store.path())?;
        let record = store.resource(&stored.plan.resource_id)?;
        stored.validate(&store.revision()?, &record, &adapters::discover_all())?;
        let mut tx = transaction::Transaction::begin(deployment::journal_path(store.path()), &stored.plan.id, stored.writes.clone())?;
        let applied = tx.apply().and_then(|()| {
            for write in &stored.writes { if write.resource.read()? != write.after { return Err(AppError::PlanChanged); } }
            Ok(())
        });
        if applied.is_err() {
            let restored = tx.rollback().is_ok();
            if restored { tx.clear()?; }
            return Err(AppError::Config(if restored { "Resource apply failed; attempted changes were restored. Generate a new preview" }
                else { "Resource apply failed and recovery is incomplete; no catalog deletion was committed. Review the journal" }.into()));
        }
        if store.commit_resource(&record.id, stored.bindings_after, stored.plan.revoke_and_delete).is_err() {
            let restored = tx.rollback().is_ok();
            return Err(AppError::Config(format!("Resource audit could not be persisted; recovery journal retained. Compensation: {}", if restored { "restored" } else { "incomplete" })));
        }
        tx.complete()?;
        Ok(stored.plan)
    }).await
}
