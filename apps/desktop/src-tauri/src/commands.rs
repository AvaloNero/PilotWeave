use crate::account::{
    self, AccountStatusSnapshot, LoginApplyResult, LoginPlan, LoginPlanStore, LoginStore,
    LoginSurface,
};
use crate::adapters;
use crate::deployment::{self, PlanContext, PlanStore, StoredPlan};
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
use crate::state::{DeleteConnectionResult, StateStore};
use crate::usage_db::UsageDb;
use chrono::Utc;
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::State;
use uuid::Uuid;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPlan {
    id: String,
    action: RecoveryAction,
    view: crate::transaction::RecoveryView,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryAction {
    #[default]
    Restore,
    KeepCurrent,
}

#[derive(Clone)]
pub struct ManagedState {
    writes: Arc<Mutex<()>>,
    recovery_plan: Arc<Mutex<Option<RecoveryPlan>>>,
    installs: Arc<Mutex<()>>,
    logins: Arc<Mutex<()>>,
    store: Arc<Mutex<StateStore>>,
    plans: Arc<Mutex<PlanStore>>,
    install_plans: Arc<Mutex<InstallPlanStore>>,
    login_plans: Arc<Mutex<LoginPlanStore>>,
    login_store: Arc<Mutex<LoginStore>>,
    github_authorization: Arc<Mutex<GithubAuthorizationStore>>,
    pub(crate) usage_db: Arc<Mutex<Option<UsageDb>>>,
    pub(crate) usage_jobs: Arc<crate::usage::commands::UsageJobs>,
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
            writes: Arc::new(Mutex::new(())),
            recovery_plan: Arc::new(Mutex::new(None)),
            installs: Arc::new(Mutex::new(())),
            logins: Arc::new(Mutex::new(())),
            store: Arc::new(Mutex::new(store)),
            plans: Arc::new(Mutex::new(PlanStore::default())),
            install_plans: Arc::new(Mutex::new(InstallPlanStore::default())),
            login_plans: Arc::new(Mutex::new(LoginPlanStore::default())),
            login_store: Arc::new(Mutex::new(login_store)),
            github_authorization: Arc::new(Mutex::new(github_authorization)),
            usage_db: Arc::new(Mutex::new(usage_db)),
            usage_jobs: Arc::new(crate::usage::commands::UsageJobs::default()),
            usage_db_error,
        }
    }

    pub(crate) fn store(&self) -> AppResult<MutexGuard<'_, StateStore>> {
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

    pub(crate) fn github_authorization(
        &self,
    ) -> AppResult<MutexGuard<'_, GithubAuthorizationStore>> {
        self.github_authorization.lock().map_err(|_| AppError::Lock)
    }

    pub(crate) fn usage_db_status(&self) -> AppResult<UsageDbStatus> {
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
pub async fn get_dashboard(state: State<'_, ManagedState>) -> Result<DashboardSnapshot, String> {
    let state = state.inner().clone();
    native_job(move || {
        let (state_path, mut connections, deployments, state_recovery) = {
            let store = state.store().map_err(command_error)?;
            (
                store.path().to_string_lossy().to_string(),
                store.connections().to_vec(),
                store.deployments().to_vec(),
                store.recovery().map(str::to_string),
            )
        };
        let credential_statuses = connections
            .iter()
            .map(|connection| {
                (
                    connection.id.clone(),
                    if state_recovery.is_some() {
                        crate::domain::CredentialObservation {
                            state: crate::domain::CredentialState::Unavailable,
                        }
                    } else {
                        crate::secrets::observe(&connection.id)
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for connection in &mut connections {
            match credential_statuses[&connection.id].state {
                crate::domain::CredentialState::Stored => connection.has_secret = true,
                crate::domain::CredentialState::Missing => connection.has_secret = false,
                crate::domain::CredentialState::Unavailable => {}
            }
        }
        let deployment_recovery = deployment::recovery_required(std::path::Path::new(&state_path))
            .then(|| {
                "An interrupted deployment requires recovery review; managed writes are disabled"
                    .to_string()
            });
        Ok(DashboardSnapshot {
            credential_statuses,
            deployment_recovery,
            version: STATE_VERSION,
            state_path,
            connections,
            clients: adapters::discover_all(),
            deployments,
            state_recovery: state_recovery.map(|reason| redact::redact_text(&reason)),
            usage_db: state.usage_db_status().map_err(command_error)?,
        })
    })
    .await
}

#[tauri::command]
pub async fn get_installation_status() -> Result<Vec<InstallComponentObservation>, String> {
    native_job(|| Ok(installer::discover_components())).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn preview_install(
    state: State<'_, ManagedState>,
    component_ids: Vec<String>,
) -> Result<InstallPlan, String> {
    let state = state.inner().clone();
    native_job(move || {
        state
            .install_plans()
            .and_then(|mut plans| plans.preview(component_ids))
            .map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn apply_install_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
) -> Result<InstallApplyResult, String> {
    let state = state.inner().clone();
    native_job(move || {
        let _run = state
            .installs
            .try_lock()
            .map_err(|_| "Another installation is active".to_string())?;
        let plan = state
            .install_plans()
            .and_then(|mut plans| plans.consume(&plan_id))
            .map_err(command_error)?;
        installer::execute_plan(plan).map_err(command_error)
    })
    .await
}

#[tauri::command]
pub async fn get_account_status(
    state: State<'_, ManagedState>,
) -> Result<AccountStatusSnapshot, String> {
    let state = state.inner().clone();
    native_job(move || {
        let (runs, recovery) = {
            let store = state.login_store().map_err(command_error)?;
            (store.runs(), store.recovery())
        };
        Ok(account::discover_status(runs, recovery))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn preview_login(
    state: State<'_, ManagedState>,
    surfaces: Vec<LoginSurface>,
) -> Result<LoginPlan, String> {
    let state = state.inner().clone();
    native_job(move || {
        state
            .login_plans()
            .and_then(|mut plans| plans.preview(surfaces))
            .map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn apply_login_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
) -> Result<LoginApplyResult, String> {
    let state = state.inner().clone();
    native_job(move || {
    let _run = state.logins.try_lock().map_err(|_| "Another sign-in launch is active".to_string())?;
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

    }).await
}

#[tauri::command]
pub async fn get_github_authorization_status(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    let state = state.inner().clone();
    native_job(move || {
        Ok(state
            .github_authorization()
            .map_err(command_error)?
            .status())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn authorize_github(
    state: State<'_, ManagedState>,
    token: String,
) -> Result<GithubAuthorizationStatus, String> {
    let (revision, existing) = {
        let mut store = state.github_authorization().map_err(command_error)?;
        (
            store.begin_attempt().map_err(command_error)?,
            store.status(),
        )
    };
    let (token, outcome) = validate_github_token(token).await?;
    let mut store = state.github_authorization().map_err(command_error)?;
    store.ensure_current(revision).map_err(command_error)?;
    match outcome {
        GithubValidationOutcome::Verified(validation) => store
            .save_verified(&token, validation)
            .map_err(command_error),
        GithubValidationOutcome::Rejected(status) if existing.has_secret => Err(format!(
            "{} The previously stored authorization was left unchanged.",
            status.detail
        )),
        GithubValidationOutcome::Rejected(status) => Ok(status),
    }
}

#[tauri::command]
pub async fn refresh_github_authorization(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    let (token, revision) = {
        let mut store = state.github_authorization().map_err(command_error)?;
        let Some(token) = store.secret_for_refresh().map_err(command_error)? else {
            return Ok(store.status());
        };
        (token, store.begin_attempt().map_err(command_error)?)
    };
    let (token, outcome) = validate_github_token(token).await?;
    let mut store = state.github_authorization().map_err(command_error)?;
    store.ensure_current(revision).map_err(command_error)?;
    match outcome {
        GithubValidationOutcome::Verified(validation) => store
            .save_verified(&token, validation)
            .map_err(command_error),
        GithubValidationOutcome::Rejected(status) => {
            store.record_refresh_failure(status).map_err(command_error)
        }
    }
}

#[tauri::command]
pub async fn clear_github_authorization(
    state: State<'_, ManagedState>,
) -> Result<GithubAuthorizationStatus, String> {
    let state = state.inner().clone();
    native_job(move || {
        state
            .github_authorization()
            .and_then(|mut store| store.clear())
            .map_err(command_error)
    })
    .await
}

async fn validate_github_token(token: String) -> Result<(String, GithubValidationOutcome), String> {
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

#[tauri::command(rename_all = "camelCase")]
pub async fn upsert_connection(
    state: State<'_, ManagedState>,
    input: ConnectionInput,
) -> Result<Connection, String> {
    let state = state.inner().clone();
    native_job(move || {
        let _write = state
            .writes
            .try_lock()
            .map_err(|_| "Another managed write is active".to_string())?;
        let mut store = state.store().map_err(command_error)?;
        let _lease = crate::write_lock::WriteLock::acquire(store.path()).map_err(command_error)?;
        deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
        store.upsert_connection(input).map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_connection(
    state: State<'_, ManagedState>,
    connection_id: String,
) -> Result<DeleteConnectionResult, String> {
    let state = state.inner().clone();
    native_job(move || {
        let _write = state
            .writes
            .try_lock()
            .map_err(|_| "Another managed write is active".to_string())?;
        let mut store = state.store().map_err(command_error)?;
        let _lease = crate::write_lock::WriteLock::acquire(store.path()).map_err(command_error)?;
        deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
        store
            .delete_connection(&connection_id)
            .map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn preview_deployment(
    state: State<'_, ManagedState>,
    connection_id: String,
    target_ids: Vec<String>,
) -> Result<DeploymentPlan, String> {
    let state = state.inner().clone();
    native_job(move || {
        let (connection, secret, context) = {
            let store = state.store().map_err(command_error)?;
            store.ensure_writable().map_err(command_error)?;
            deployment::ensure_no_pending_journal(store.path()).map_err(command_error)?;
            let connection = store.connection(&connection_id).map_err(command_error)?;
            let secret = store.secret_for(&connection).map_err(command_error)?;
            let context = PlanContext::new(
                store.installation_owner_id(),
                store.revision().map_err(command_error)?,
                secret.as_deref(),
            );
            (connection, secret, context)
        };
        let targets = adapters::discover_all();
        let plan = adapters::preview_resolved(&connection, &target_ids, &targets)
            .map_err(command_error)?;
        state
            .plans()
            .and_then(|mut plans| {
                plans.insert(&connection, secret.as_deref(), plan, &targets, context)
            })
            .map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn apply_deployment_plan(
    state: State<'_, ManagedState>,
    plan_id: String,
    confirmed: bool,
) -> Result<ApplyResult, String> {
    let state = state.inner().clone();
    native_job(move || {
        let stored = state
            .plans()
            .and_then(|mut plans| plans.consume(&plan_id))
            .map_err(command_error)?;
        if !confirmed {
            return Err("Explicit deployment confirmation is required".into());
        }
        execute_stored_plan(&state, stored).map_err(command_error)
    })
    .await
}

fn execute_stored_plan(state: &ManagedState, stored: StoredPlan) -> AppResult<ApplyResult> {
    let _write = state
        .writes
        .try_lock()
        .map_err(|_| AppError::Config("Another managed write is active".into()))?;
    let state_path = state.store()?.path().to_path_buf();
    let _lease = crate::write_lock::WriteLock::acquire(&state_path)?;
    let (connection, secret, context) = {
        let store = state.store()?;
        store.ensure_writable()?;
        deployment::ensure_no_pending_journal(store.path())?;
        let connection = store.connection(&stored.plan.connection_id)?;
        let secret = store.secret_for(&connection)?;
        let context = PlanContext::new(
            store.installation_owner_id(),
            store.revision()?,
            secret.as_deref(),
        );
        (connection, secret, context)
    };
    let targets = adapters::discover_all();
    deployment::validate_plan(&stored, &connection, &targets, &context)?;
    let mut tx = crate::transaction::Transaction::begin(
        deployment::journal_path(&state_path),
        &stored.plan.id,
        stored.writes,
    )?;
    let applied = tx.apply();
    let failed = applied.is_err();
    let mut detail = "Prepared changes were applied and verified".to_string();
    let mut recovery_failed = false;
    if let Err(error) = applied {
        detail = redact::redact_with_secret(&error.to_string(), secret.as_deref());
        match tx.rollback() {
            Ok(()) => detail.push_str("; attempted changes were restored"),
            Err(error) => {
                recovery_failed = true;
                detail.push_str(&format!(
                    "; {}",
                    redact::redact_with_secret(&error.to_string(), secret.as_deref())
                ));
            }
        }
    }
    let records = stored
        .plan
        .operations
        .iter()
        .map(|operation| {
            let mut record = record(
                &stored.plan,
                operation,
                if !operation.supported {
                    DeploymentStatus::Skipped
                } else if failed {
                    DeploymentStatus::Failed
                } else {
                    DeploymentStatus::Applied
                },
                if operation.supported {
                    detail.clone()
                } else {
                    operation.description.clone()
                },
            );
            if record.status == DeploymentStatus::Applied {
                record.target_fingerprint =
                    stored.after_fingerprints.get(&record.target_id).cloned();
                record.connection_revision = Some(crate::fingerprint::json(
                    "setup-connection-v1",
                    &connection,
                )?);
            }
            Ok(record)
        })
        .collect::<AppResult<Vec<_>>>();
    let recorded = records.and_then(|records| {
        for record in records
            .iter()
            .filter(|record| record.status == DeploymentStatus::Applied)
        {
            let target = targets
                .iter()
                .find(|target| target.id == record.target_id)
                .ok_or(AppError::PlanChanged)?;
            if record.target_fingerprint.as_ref() != Some(&deployment::fingerprint_target(target)?)
            {
                return Err(AppError::PlanChanged);
            }
        }
        state.store()?.record_deployments(records.clone())?;
        Ok(records)
    });
    if let Err(error) = &recorded {
        let rollback = tx.rollback();
        return Err(AppError::Config(format!(
            "Final deployment audit could not be saved; recovery journal retained. Audit: {}; compensation: {}",
            redact::redact_with_secret(&error.to_string(), secret.as_deref()),
            if rollback.is_ok() { "restored" } else { "incomplete; inspect recovery" }
        )));
    }
    let records = recorded?;
    if !recovery_failed {
        if failed {
            tx.clear()?;
        } else {
            tx.complete()?;
        }
    }
    // Notification is not part of registry persistence; failure does not undo
    // a successfully committed environment or falsely report a failed write.
    if let Some(warning) = adapters::copilot_cli::notify_environment() {
        log::warn!("{warning}");
    }
    Ok(ApplyResult {
        plan_id: stored.plan.id,
        records,
    })
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
        target_fingerprint: None,
        connection_revision: None,
    }
}

#[tauri::command]
pub async fn preview_deployment_recovery(
    state: State<'_, ManagedState>,
    action: Option<RecoveryAction>,
) -> Result<RecoveryPlan, String> {
    let state = state.inner().clone();
    native_job(move || {
        let path = deployment::journal_path(state.store().map_err(command_error)?.path());
        let action = action.unwrap_or_default();
        let allowed = match action {
            RecoveryAction::Restore => deployment::recovery_resources().unwrap_or_default(),
            RecoveryAction::KeepCurrent => Vec::new(),
        };
        let plan = RecoveryPlan {
            id: Uuid::new_v4().to_string(),
            action,
            view: crate::transaction::recovery_view(&path, &allowed).map_err(command_error)?,
            expires_at: Utc::now() + chrono::Duration::minutes(15),
        };
        *state
            .recovery_plan
            .lock()
            .map_err(|_| "Recovery plan lock failed".to_string())? = Some(plan.clone());
        Ok(plan)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn apply_deployment_recovery(
    state: State<'_, ManagedState>,
    plan_id: String,
    confirmed: bool,
) -> Result<bool, String> {
    let state = state.inner().clone();
    native_job(move || {
        if !confirmed {
            return Err("Explicit recovery confirmation is required".into());
        }
        let plan = state
            .recovery_plan
            .lock()
            .map_err(|_| "Recovery plan lock failed".to_string())?
            .take()
            .ok_or_else(|| {
                "Preview recovery again; previous plan is missing or consumed".to_string()
            })?;
        if plan.id != plan_id || plan.expires_at <= Utc::now() {
            return Err("Recovery preview is stale".into());
        }
        let _write = state
            .writes
            .try_lock()
            .map_err(|_| "Another managed write is active".to_string())?;
        let store = state.store().map_err(command_error)?;
        let _lease = crate::write_lock::WriteLock::acquire(store.path()).map_err(command_error)?;
        let path = deployment::journal_path(store.path());
        match plan.action {
            RecoveryAction::Restore => {
                let allowed = deployment::recovery_resources().unwrap_or_default();
                crate::transaction::recover(&path, &plan.view.digest, &allowed)
            }
            RecoveryAction::KeepCurrent => {
                crate::transaction::keep_current(&path, &plan.view.digest)
            }
        }
        .map_err(command_error)?;
        Ok(true)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_github_billing_overview(
    state: State<'_, ManagedState>,
    year: i32,
    month: u32,
) -> Result<crate::github_billing::GithubBillingOverview, String> {
    crate::usage::commands::get_github_billing(state, Some(year), Some(month))
}
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_github_billing(
    state: State<'_, ManagedState>,
    year: i32,
    month: u32,
) -> Result<crate::github_billing::GithubBillingOverview, String> {
    crate::usage::commands::refresh_personal_github_usage(state, year, month).await
}

async fn native_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|_| "Native task failed; inspect recovery status before retrying".to_string())?
}
