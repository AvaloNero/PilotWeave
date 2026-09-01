use crate::adapters;
use crate::deployment::{self, PlanStore, StoredPlan};
use crate::domain::{
    ApplyResult, Connection, ConnectionInput, DashboardSnapshot, DeploymentOperation,
    DeploymentPlan, DeploymentRecord, DeploymentStatus, STATE_VERSION,
};
use crate::error::{AppError, AppResult};
use crate::redact;
use crate::state::StateStore;
use chrono::Utc;
use std::sync::{Mutex, MutexGuard};
use tauri::State;
use uuid::Uuid;

pub struct ManagedState {
    store: Mutex<StateStore>,
    plans: Mutex<PlanStore>,
}

impl ManagedState {
    pub fn new(store: StateStore) -> Self {
        Self {
            store: Mutex::new(store),
            plans: Mutex::new(PlanStore::default()),
        }
    }

    fn store(&self) -> AppResult<MutexGuard<'_, StateStore>> {
        self.store.lock().map_err(|_| AppError::Lock)
    }

    fn plans(&self) -> AppResult<MutexGuard<'_, PlanStore>> {
        self.plans.lock().map_err(|_| AppError::Lock)
    }
}

fn command_error(error: AppError) -> String {
    redact::redact_text(&error.to_string())
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, ManagedState>) -> Result<DashboardSnapshot, String> {
    let (state_path, connections, deployments) = {
        let store = state.store().map_err(command_error)?;
        (
            store.path().to_string_lossy().to_string(),
            store.connections().to_vec(),
            store.deployments().to_vec(),
        )
    };
    Ok(DashboardSnapshot {
        version: STATE_VERSION,
        state_path,
        connections,
        clients: adapters::discover_all(),
        deployments,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn upsert_connection(
    state: State<'_, ManagedState>,
    input: ConnectionInput,
) -> Result<Connection, String> {
    let mut store = state.store().map_err(command_error)?;
    deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
    store.upsert_connection(input).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_connection(
    state: State<'_, ManagedState>,
    connection_id: String,
) -> Result<bool, String> {
    let mut store = state.store().map_err(command_error)?;
    deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
    store
        .delete_connection(&connection_id)
        .map(|()| true)
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_deployment(
    state: State<'_, ManagedState>,
    connection_id: String,
    target_ids: Vec<String>,
) -> Result<DeploymentPlan, String> {
    let connection = {
        let store = state.store().map_err(command_error)?;
        deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
        store.connection(&connection_id).map_err(command_error)?
    };
    let plan = adapters::preview(&connection, &target_ids).map_err(command_error)?;
    let targets = adapters::discover_all();
    state
        .plans()
        .and_then(|mut plans| plans.insert(&connection, plan, &targets))
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_deployment(
    state: State<'_, ManagedState>,
    connection_id: String,
    target_ids: Vec<String>,
) -> Result<ApplyResult, String> {
    let stored = state
        .plans()
        .and_then(|mut plans| plans.consume_matching(&connection_id, &target_ids))
        .map_err(command_error)?;
    execute_stored_plan(&state, stored).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_deployment_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
) -> Result<ApplyResult, String> {
    let stored = state
        .plans()
        .and_then(|mut plans| plans.consume(&plan_id))
        .map_err(command_error)?;
    execute_stored_plan(&state, stored).map_err(command_error)
}

fn execute_stored_plan(
    state: &State<'_, ManagedState>,
    stored: StoredPlan,
) -> AppResult<ApplyResult> {
    let (connection, secret, state_path) = {
        let store = state.store()?;
        deployment::ensure_no_pending_journal(store.path())?;
        let connection = store.connection(&stored.plan.connection_id)?;
        let secret = store.secret_for(&connection)?;
        (connection, secret, store.path().to_path_buf())
    };

    let available = adapters::discover_all();
    deployment::validate_plan(&stored, &connection, &available)?;
    let mut snapshots = deployment::capture_file_snapshots(&stored.plan, &available)?;
    let mut journal = deployment::begin_journal(&state_path, &stored, &available, &snapshots)?;

    let mut operations = stored.plan.operations.iter().collect::<Vec<_>>();
    operations.sort_by_key(|operation| deployment::operation_rank(operation.target_kind));
    let mut records = Vec::new();
    let mut rollback_failed = false;

    for operation in operations {
        let target = available
            .iter()
            .find(|target| target.id == operation.target_id)
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "Target disappeared after preflight: {}",
                    operation.target_id
                ))
            })?;

        if !operation.supported {
            records.push(record(
                &stored.plan,
                operation,
                DeploymentStatus::Skipped,
                operation.description.clone(),
            ));
            continue;
        }

        match adapters::apply_to_target(&connection, secret.as_deref(), target) {
            Ok(detail) => {
                let after = deployment::fingerprint_target(target)?;
                if operation.target_kind == crate::domain::ClientKind::VsCodeCopilot {
                    let _ = deployment::mark_snapshot_applied(&mut snapshots, &target.id)?;
                }
                journal.mark_applied(&target.id, after)?;
                records.push(record(
                    &stored.plan,
                    operation,
                    DeploymentStatus::Applied,
                    detail,
                ));
            }
            Err(error) => {
                let detail = redact::redact_with_secret(&error.to_string(), secret.as_deref());
                records.push(record(
                    &stored.plan,
                    operation,
                    DeploymentStatus::Failed,
                    detail,
                ));
                match deployment::rollback_applied_files(&snapshots) {
                    Ok(()) => mark_rolled_back(&mut records),
                    Err(rollback_error) => {
                        rollback_failed = true;
                        if let Some(last) = records.last_mut() {
                            last.detail
                                .push_str("; rollback requires recovery review: ");
                            last.detail.push_str(&redact::redact_with_secret(
                                &rollback_error.to_string(),
                                secret.as_deref(),
                            ));
                        }
                    }
                }
                break;
            }
        }
    }

    {
        let mut store = state.store()?;
        store.record_deployments(records.clone())?;
    }

    if !rollback_failed {
        journal.clear()?;
    }

    Ok(ApplyResult {
        plan_id: stored.plan.id,
        records,
    })
}

fn mark_rolled_back(records: &mut [DeploymentRecord]) {
    for record in records {
        if record.status == DeploymentStatus::Applied
            && record.target_kind == crate::domain::ClientKind::VsCodeCopilot
        {
            record.status = DeploymentStatus::Failed;
            record
                .detail
                .push_str("; change was restored after a later target failed");
        }
    }
}

fn record(
    plan: &DeploymentPlan,
    operation: &DeploymentOperation,
    status: DeploymentStatus,
    detail: String,
) -> DeploymentRecord {
    DeploymentRecord {
        id: Uuid::new_v4().to_string(),
        plan_id: plan.id.clone(),
        connection_id: plan.connection_id.clone(),
        target_id: operation.target_id.clone(),
        target_kind: operation.target_kind,
        status,
        detail: redact::redact_text(&detail),
        created_at: Utc::now(),
    }
}
