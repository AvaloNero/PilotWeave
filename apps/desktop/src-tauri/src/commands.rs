use crate::account::{
    self, AccountStatusSnapshot, LoginApplyResult, LoginPlan, LoginPlanStore, LoginStore,
    LoginSurface,
};
use crate::adapters;
use crate::deployment::{self, PlanStore, StoredPlan};
use crate::domain::{
    ApplyResult, Connection, ConnectionInput, DashboardSnapshot, DeploymentOperation,
    DeploymentPlan, DeploymentRecord, DeploymentStatus, UsageDbStatus, STATE_VERSION,
};
use crate::error::{AppError, AppResult};
use crate::github_auth::{
    self, GithubAuthorizationStatus, GithubAuthorizationStore, GithubValidationOutcome,
};
use crate::installer::{
    self, InstallApplyResult, InstallComponentObservation, InstallPlan, InstallPlanStore,
};
use crate::redact;
use crate::state::StateStore;
use crate::usage_db::UsageDb;
use chrono::Utc;
use std::sync::{Mutex, MutexGuard};
use tauri::State;
use uuid::Uuid;

pub struct ManagedState {
    store: Mutex<StateStore>,
    plans: Mutex<PlanStore>,
    install_plans: Mutex<InstallPlanStore>,
    login_plans: Mutex<LoginPlanStore>,
    login_store: Mutex<LoginStore>,
    github_authorization: Mutex<GithubAuthorizationStore>,
    usage_db: Mutex<Option<UsageDb>>,
    usage_db_error: Option<String>,
}

impl ManagedState {
    pub fn new(
        store: StateStore,
        login_store: LoginStore,
        github_authorization: GithubAuthorizationStore,
        usage_db: Option<UsageDb>,
        usage_db_error: Option<String>,
    ) -> Self {
        Self {
            store: Mutex::new(store),
            plans: Mutex::new(PlanStore::default()),
            install_plans: Mutex::new(InstallPlanStore::default()),
            login_plans: Mutex::new(LoginPlanStore::default()),
            login_store: Mutex::new(login_store),
            github_authorization: Mutex::new(github_authorization),
            usage_db: Mutex::new(usage_db),
            usage_db_error,
        }
    }

    fn store(&self) -> AppResult<MutexGuard<'_, StateStore>> {
        self.store.lock().map_err(|_| AppError::Lock)
    }

    fn plans(&self) -> AppResult<MutexGuard<'_, PlanStore>> {
        self.plans.lock().map_err(|_| AppError::Lock)
    }

    fn install_plans(&self) -> AppResult<MutexGuard<'_, InstallPlanStore>> {
        self.install_plans.lock().map_err(|_| AppError::Lock)
    }

    fn login_plans(&self) -> AppResult<MutexGuard<'_, LoginPlanStore>> {
        self.login_plans.lock().map_err(|_| AppError::Lock)
    }

    fn login_store(&self) -> AppResult<MutexGuard<'_, LoginStore>> {
        self.login_store.lock().map_err(|_| AppError::Lock)
    }

    fn github_authorization(&self) -> AppResult<MutexGuard<'_, GithubAuthorizationStore>> {
        self.github_authorization
            .lock()
            .map_err(|_| AppError::Lock)
    }

    fn usage_db_status(&self) -> AppResult<UsageDbStatus> {
        let guard = self.usage_db.lock().map_err(|_| AppError::Lock)?;
        Ok(match guard.as_ref() {
            Some(db) => db.status(),
            None => UsageDbStatus::unavailable(
                self.usage_db_error
                    .clone()
                    .unwrap_or_else(|| "Usage database is not available".to_string()),
            ),
        })
    }
}

fn command_error(error: AppError) -> String {
    redact::redact_text(&error.to_string())
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, ManagedState>) -> Result<DashboardSnapshot, String> {
    let (state_path, connections, deployments, state_recovery) = {
        let store = state.store().map_err(command_error)?;
        (
            store.path().to_string_lossy().to_string(),
            store.connections().to_vec(),
            store.deployments().to_vec(),
            store.recovery().map(str::to_string),
        )
    };
    Ok(DashboardSnapshot {
        version: STATE_VERSION,
        state_path,
        connections,
        clients: adapters::discover_all(),
        deployments,
        state_recovery: state_recovery.map(|reason| redact::redact_text(&reason)),
        usage_db: state.usage_db_status().map_err(command_error)?,
    })
}

#[tauri::command]
pub fn get_installation_status() -> Vec<InstallComponentObservation> {
    installer::discover_components()
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_install(
    state: State<'_, ManagedState>,
    component_ids: Vec<String>,
) -> Result<InstallPlan, String> {
    state
        .install_plans()
        .and_then(|mut plans| plans.preview(component_ids))
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_install_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
) -> Result<InstallApplyResult, String> {
    state
        .install_plans()
        .and_then(|mut plans| installer::apply_plan(&mut plans, &plan_id))
        .map_err(command_error)
}

#[tauri::command]
pub fn get_account_status(state: State<'_, ManagedState>) -> Result<AccountStatusSnapshot, String> {
    let (runs, recovery) = {
        let store = state.login_store().map_err(command_error)?;
        (store.runs(), store.recovery())
    };
    Ok(account::discover_status(runs, recovery))
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_login(
    state: State<'_, ManagedState>,
    surfaces: Vec<LoginSurface>,
) -> Result<LoginPlan, String> {
    state
        .login_plans()
        .and_then(|mut plans| plans.preview(surfaces))
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_login_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
) -> Result<LoginApplyResult, String> {
    let stored = {
        let mut plans = state.login_plans().map_err(command_error)?;
        plans.consume(&plan_id).map_err(command_error)?
    };
    let run = {
        let mut history = state.login_store().map_err(command_error)?;
        history.begin_run(&stored.plan).map_err(command_error)?
    };
    let finished = account::execute_plan(&stored, run);
    {
        let mut history = state.login_store().map_err(command_error)?;
        if let Err(error) = history.finish_run(finished.clone()) {
            return Err(command_error(AppError::Config(format!(
                "Official sign-in clients may have been launched, but the final local run summary could not be persisted; the prepared history entry remains interrupted: {error}"
            ))));
        }
    }
    let (runs, recovery) = {
        let history = state.login_store().map_err(command_error)?;
        (history.runs(), history.recovery())
    };
    Ok(LoginApplyResult {
        run: finished,
        account_status: account::discover_status(runs, recovery),
    })
}

#[tauri::command]
pub fn get_github_authorization_status(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    Ok(state
        .github_authorization()
        .map_err(command_error)?
        .status())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn authorize_github(
    state: State<'_, ManagedState>,
    token: String,
) -> Result<GithubAuthorizationStatus, String> {
    let existing = state
        .github_authorization()
        .map_err(command_error)?
        .status();
    let (token, outcome) = validate_github_token(token).await?;
    match outcome {
        GithubValidationOutcome::Verified(validation) => state
            .github_authorization()
            .and_then(|mut store| store.save_verified(&token, validation))
            .map_err(command_error),
        GithubValidationOutcome::Rejected(status) => Ok(merge_rejected_attempt(status, existing)),
    }
}

#[tauri::command]
pub async fn refresh_github_authorization(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    let (token, existing) = {
        let store = state.github_authorization().map_err(command_error)?;
        let existing = store.status();
        let Some(token) = store.secret_for_refresh().map_err(command_error)? else {
            return Ok(existing);
        };
        (token, existing)
    };
    let (token, outcome) = validate_github_token(token).await?;
    match outcome {
        GithubValidationOutcome::Verified(validation) => state
            .github_authorization()
            .and_then(|mut store| store.save_verified(&token, validation))
            .map_err(command_error),
        GithubValidationOutcome::Rejected(status) => Ok(merge_rejected_attempt(status, existing)),
    }
}

#[tauri::command]
pub fn clear_github_authorization(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    state
        .github_authorization()
        .and_then(|mut store| store.clear())
        .map_err(command_error)
}

async fn validate_github_token(
    token: String,
) -> Result<(String, GithubValidationOutcome), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = github_auth::validate_token_native(&token)?;
        Ok::<_, AppError>((token, outcome))
    })
    .await
    .map_err(|error| {
        command_error(AppError::Config(format!(
            "GitHub authorization task failed: {error}"
        )))
    })?
    .map_err(command_error)
}

fn merge_rejected_attempt(
    mut rejected: GithubAuthorizationStatus,
    existing: GithubAuthorizationStatus,
) -> GithubAuthorizationStatus {
    if existing.has_secret {
        rejected.identity = existing.identity;
        rejected.has_secret = true;
        rejected.scopes = existing.scopes;
        rejected.billing_capability = existing.billing_capability;
        rejected.billing_detail = existing.billing_detail;
        rejected.validated_at = existing.validated_at;
        rejected
            .detail
            .push_str(" The previously stored authorization was left unchanged.");
    }
    rejected
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
