use crate::adapters;
use crate::domain::{
    ApplyResult, Connection, ConnectionInput, DashboardSnapshot, DeploymentPlan, DeploymentRecord,
    DeploymentStatus, STATE_VERSION,
};
use crate::error::{AppError, AppResult};
use crate::state::StateStore;
use chrono::Utc;
use std::sync::{Mutex, MutexGuard};
use tauri::State;
use uuid::Uuid;

pub struct ManagedState(pub Mutex<StateStore>);

impl ManagedState {
    pub fn new(store: StateStore) -> Self {
        Self(Mutex::new(store))
    }

    fn lock(&self) -> AppResult<MutexGuard<'_, StateStore>> {
        self.0.lock().map_err(|_| AppError::Lock)
    }
}

fn command_error(error: AppError) -> String {
    error.to_string()
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, ManagedState>) -> Result<DashboardSnapshot, String> {
    let store = state.lock().map_err(command_error)?;
    Ok(DashboardSnapshot {
        version: STATE_VERSION,
        state_path: store.path().to_string_lossy().to_string(),
        connections: store.connections().to_vec(),
        clients: adapters::discover_all(),
        deployments: store.deployments().to_vec(),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn upsert_connection(
    state: State<'_, ManagedState>,
    input: ConnectionInput,
) -> Result<Connection, String> {
    state
        .lock()
        .and_then(|mut store| store.upsert_connection(input))
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_connection(
    state: State<'_, ManagedState>,
    connection_id: String,
) -> Result<bool, String> {
    state
        .lock()
        .and_then(|mut store| store.delete_connection(&connection_id))
        .map(|()| true)
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_deployment(
    state: State<'_, ManagedState>,
    connection_id: String,
    target_ids: Vec<String>,
) -> Result<DeploymentPlan, String> {
    let connection = state
        .lock()
        .and_then(|store| store.connection(&connection_id))
        .map_err(command_error)?;
    adapters::preview(&connection, &target_ids).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_deployment(
    state: State<'_, ManagedState>,
    connection_id: String,
    target_ids: Vec<String>,
) -> Result<ApplyResult, String> {
    let (connection, secret) = {
        let store = state.lock().map_err(command_error)?;
        let connection = store.connection(&connection_id).map_err(command_error)?;
        let secret = store.secret_for(&connection).map_err(command_error)?;
        (connection, secret)
    };

    let plan = adapters::preview(&connection, &target_ids).map_err(command_error)?;
    let available = adapters::discover_all();
    let mut records = Vec::new();

    for operation in &plan.operations {
        let Some(target) = available
            .iter()
            .find(|target| target.id == operation.target_id)
        else {
            records.push(record(
                &plan,
                operation,
                DeploymentStatus::Failed,
                "Target disappeared between preview and apply".to_string(),
            ));
            continue;
        };

        if !operation.supported {
            records.push(record(
                &plan,
                operation,
                DeploymentStatus::Skipped,
                operation.description.clone(),
            ));
            continue;
        }

        match adapters::apply_to_target(&connection, secret.as_deref(), target) {
            Ok(detail) => records.push(record(&plan, operation, DeploymentStatus::Applied, detail)),
            Err(error) => records.push(record(
                &plan,
                operation,
                DeploymentStatus::Failed,
                error.to_string(),
            )),
        }
    }

    state
        .lock()
        .and_then(|mut store| store.record_deployments(records.clone()))
        .map_err(command_error)?;

    Ok(ApplyResult {
        plan_id: plan.id,
        records,
    })
}

fn record(
    plan: &DeploymentPlan,
    operation: &crate::domain::DeploymentOperation,
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
        detail,
        created_at: Utc::now(),
    }
}
