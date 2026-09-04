use crate::adapters;
use crate::domain::{ClientKind, ClientTarget};
use crate::error::{AppError, AppResult};
use crate::installer::{self, InstallComponentStatus};
use crate::native_process;
use crate::redact;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const LOGIN_PLAN_TTL_SECONDS: i64 = 15 * 60;
const LOGIN_HISTORY_VERSION: u32 = 1;
const MAX_LOGIN_HISTORY_BYTES: u64 = 2 * 1_024 * 1_024;
const MAX_LOGIN_RUNS: usize = 100;
const MAX_GH_OUTPUT_BYTES: usize = 64 * 1_024;
const MAX_ACCOUNT_TEXT_BYTES: usize = 1_024;
const GH_ACCOUNT_TIMEOUT_SECONDS: u64 = 15;

const GITHUB_AUTH_ENVIRONMENT: &[&str] = &[
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum LoginSurface {
    VsCodeCopilot,
    CopilotCli,
    GithubCopilotApp,
}

impl LoginSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VsCodeCopilot => "vscode-copilot",
            Self::CopilotCli => "copilot-cli",
            Self::GithubCopilotApp => "github-copilot-app",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::VsCodeCopilot => "VS Code Copilot",
            Self::CopilotCli => "GitHub Copilot CLI",
            Self::GithubCopilotApp => "GitHub Copilot app",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AccountObservationState {
    Verified,
    Inferred,
    ActionRequired,
    Unknown,
    Unsupported,
    NotInstalled,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct GithubIdentity {
    pub host: String,
    pub login: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnchorObservation {
    pub state: AccountObservationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<GithubIdentity>,
    pub evidence: String,
    pub detail: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceAccountObservation {
    pub surface: LoginSurface,
    pub state: AccountObservationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<GithubIdentity>,
    pub evidence: String,
    pub detail: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusSnapshot {
    pub anchor: AccountAnchorObservation,
    pub surfaces: Vec<SurfaceAccountObservation>,
    pub observed_at: DateTime<Utc>,
    #[serde(default)]
    pub login_runs: Vec<LoginRunRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_recovery: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOperation {
    pub surface: LoginSurface,
    pub title: String,
    pub description: String,
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPlan {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_identity: Option<GithubIdentity>,
    pub requested_surfaces: Vec<LoginSurface>,
    pub operations: Vec<LoginOperation>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoginStepStatus {
    Pending,
    Launched,
    ActionRequired,
    SkippedNotInstalled,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStepResult {
    pub surface: LoginSurface,
    pub status: LoginStepStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoginRunStatus {
    InProgress,
    ActionRequired,
    Partial,
    Failed,
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRunRecord {
    pub id: String,
    pub plan_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_identity: Option<GithubIdentity>,
    pub requested_surfaces: Vec<LoginSurface>,
    pub status: LoginRunStatus,
    pub steps: Vec<LoginStepResult>,
    pub summary: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginApplyResult {
    pub run: LoginRunRecord,
    pub account_status: AccountStatusSnapshot,
}

#[derive(Debug, Clone)]
struct ExecutableLoginOperation {
    path: PathBuf,
    fingerprint: String,
    arguments: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoredLoginPlan {
    pub plan: LoginPlan,
    account_fingerprint: String,
    executables: BTreeMap<LoginSurface, ExecutableLoginOperation>,
}

#[derive(Default)]
pub struct LoginPlanStore {
    plans: HashMap<String, StoredLoginPlan>,
}

impl LoginPlanStore {
    pub fn preview(&mut self, requested_surfaces: Vec<LoginSurface>) -> AppResult<LoginPlan> {
        self.purge_expired();
        let surfaces = canonical_surfaces(&requested_surfaces)?;
        let status = discover_status(Vec::new(), None);
        let mut operations = Vec::new();
        let mut executables = BTreeMap::new();

        for surface in &surfaces {
            let installed = status
                .surfaces
                .iter()
                .find(|item| item.surface == *surface)
                .map(|item| item.state != AccountObservationState::NotInstalled)
                .unwrap_or(false);
            if !installed {
                operations.push(LoginOperation {
                    surface: *surface,
                    title: format!("Sign in to {}", surface.display_name()),
                    description: "Install this surface before starting its official sign-in flow"
                        .to_string(),
                    supported: false,
                });
                continue;
            }

            match executable_operation(*surface) {
                Ok(executable) => {
                    operations.push(LoginOperation {
                        surface: *surface,
                        title: format!("Open {} sign-in", surface.display_name()),
                        description: operation_description(*surface).to_string(),
                        supported: true,
                    });
                    executables.insert(*surface, executable);
                }
                Err(error) => operations.push(LoginOperation {
                    surface: *surface,
                    title: format!("Sign in to {}", surface.display_name()),
                    description: redact::redact_text(&error.to_string()),
                    supported: false,
                }),
            }
        }

        let target_identity = (status.anchor.state == AccountObservationState::Verified)
            .then(|| status.anchor.identity.clone())
            .flatten();
        let created_at = Utc::now();
        let plan = LoginPlan {
            id: Uuid::new_v4().to_string(),
            target_identity,
            requested_surfaces: surfaces,
            operations,
            created_at,
            expires_at: created_at + ChronoDuration::seconds(LOGIN_PLAN_TTL_SECONDS),
        };
        self.plans.insert(
            plan.id.clone(),
            StoredLoginPlan {
                plan: plan.clone(),
                account_fingerprint: account_fingerprint(&status),
                executables,
            },
        );
        Ok(plan)
    }

    pub fn consume(&mut self, plan_id: &str) -> AppResult<StoredLoginPlan> {
        self.purge_expired();
        let stored = self.plans.remove(plan_id).ok_or_else(|| {
            AppError::InvalidInput(
                "Sign-in plan is missing, expired, or was already consumed; preview again"
                    .to_string(),
            )
        })?;
        if stored.plan.expires_at < Utc::now() {
            return Err(AppError::InvalidInput(
                "Sign-in plan expired; preview again".to_string(),
            ));
        }

        let status = discover_status(Vec::new(), None);
        if account_fingerprint(&status) != stored.account_fingerprint {
            return Err(AppError::InvalidInput(
                "Account or client state changed after sign-in preview; preview again".to_string(),
            ));
        }
        for operation in stored.executables.values() {
            let current = native_process::fingerprint_regular_file(&operation.path)?;
            if current != operation.fingerprint {
                return Err(AppError::InvalidInput(
                    "A sign-in executable changed after preview; preview again".to_string(),
                ));
            }
        }
        Ok(stored)
    }

    fn purge_expired(&mut self) {
        let now = Utc::now();
        self.plans.retain(|_, stored| stored.plan.expires_at >= now);
    }
}

pub struct LoginStore {
    path: Option<PathBuf>,
    state: LoginHistoryState,
    recovery: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginHistoryState {
    #[serde(default = "default_history_version")]
    version: u32,
    #[serde(default)]
    runs: Vec<LoginRunRecord>,
}

impl Default for LoginHistoryState {
    fn default() -> Self {
        Self {
            version: LOGIN_HISTORY_VERSION,
            runs: Vec::new(),
        }
    }
}

fn default_history_version() -> u32 {
    LOGIN_HISTORY_VERSION
}

impl LoginStore {
    pub fn open() -> Self {
        let Some(config_dir) = dirs::config_dir() else {
            return Self {
                path: None,
                state: LoginHistoryState::default(),
                recovery: Some("Cannot resolve the user config directory".to_string()),
            };
        };
        Self::open_at(config_dir.join("PilotWeave").join("login-runs.json"))
    }

    pub fn open_at(path: PathBuf) -> Self {
        match load_history(&path) {
            Ok(mut state) => {
                let now = Utc::now();
                let mut changed = false;
                for run in &mut state.runs {
                    if run.status == LoginRunStatus::InProgress {
                        run.status = LoginRunStatus::Interrupted;
                        run.summary =
                            "PilotWeave restarted before this sign-in run reached a final state"
                                .to_string();
                        run.finished_at = Some(now);
                        changed = true;
                    }
                }
                let mut store = Self {
                    path: Some(path),
                    state,
                    recovery: None,
                };
                if changed {
                    if let Err(error) = store.persist() {
                        store.recovery = Some(error.to_string());
                    }
                }
                store
            }
            Err(error) => Self {
                path: Some(path),
                state: LoginHistoryState::default(),
                recovery: Some(error.to_string()),
            },
        }
    }

    pub fn runs(&self) -> Vec<LoginRunRecord> {
        self.state.runs.clone()
    }

    pub fn recovery(&self) -> Option<String> {
        self.recovery.clone()
    }

    pub fn begin_run(&mut self, plan: &LoginPlan) -> AppResult<LoginRunRecord> {
        self.ensure_writable()?;
        let run = LoginRunRecord {
            id: Uuid::new_v4().to_string(),
            plan_id: plan.id.clone(),
            target_identity: plan.target_identity.clone(),
            requested_surfaces: plan.requested_surfaces.clone(),
            status: LoginRunStatus::InProgress,
            steps: plan
                .requested_surfaces
                .iter()
                .map(|surface| LoginStepResult {
                    surface: *surface,
                    status: LoginStepStatus::Pending,
                    detail: "Waiting for the native sign-in launcher".to_string(),
                })
                .collect(),
            summary: "Preparing official sign-in flows".to_string(),
            started_at: Utc::now(),
            finished_at: None,
        };
        self.state.runs.insert(0, run.clone());
        self.state.runs.truncate(MAX_LOGIN_RUNS);
        if let Err(error) = self.persist() {
            self.state.runs.retain(|item| item.id != run.id);
            return Err(error);
        }
        Ok(run)
    }

    pub fn finish_run(&mut self, run: LoginRunRecord) -> AppResult<()> {
        self.ensure_writable()?;
        validate_run(&run)?;
        let index = self
            .state
            .runs
            .iter()
            .position(|item| item.id == run.id)
            .ok_or_else(|| AppError::Config("Sign-in run history entry disappeared".to_string()))?;
        let previous = self.state.runs[index].clone();
        self.state.runs[index] = run;
        if let Err(error) = self.persist() {
            self.state.runs[index] = previous;
            return Err(error);
        }
        Ok(())
    }

    fn ensure_writable(&self) -> AppResult<()> {
        if let Some(reason) = &self.recovery {
            return Err(AppError::Unsupported(format!(
                "Sign-in history is in read-only recovery ({reason})"
            )));
        }
        if self.path.is_none() {
            return Err(AppError::Unsupported(
                "Sign-in history has no writable path".to_string(),
            ));
        }
        Ok(())
    }

    fn persist(&self) -> AppResult<()> {
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| AppError::Config("Sign-in history path is unavailable".to_string()))?;
        write_history(path, &self.state)
    }
}

pub fn discover_status(
    login_runs: Vec<LoginRunRecord>,
    history_recovery: Option<String>,
) -> AccountStatusSnapshot {
    let observed_at = Utc::now();
    let anchor = observe_github_cli_account(observed_at);
    let clients = adapters::discover_all();
    let components = installer::discover_components();
    let environment_overrides = present_auth_environment();
    let surfaces = [
        LoginSurface::VsCodeCopilot,
        LoginSurface::CopilotCli,
        LoginSurface::GithubCopilotApp,
    ]
    .into_iter()
    .map(|surface| {
        observe_surface(
            surface,
            observed_at,
            &anchor,
            &clients,
            &components,
            &environment_overrides,
        )
    })
    .collect();
    AccountStatusSnapshot {
        anchor,
        surfaces,
        observed_at,
        login_runs,
        history_recovery: history_recovery.map(|value| redact::redact_text(&value)),
    }
}

pub fn execute_plan(stored: &StoredLoginPlan, mut run: LoginRunRecord) -> LoginRunRecord {
    let mut steps = Vec::new();
    for operation in &stored.plan.operations {
        let step = match stored.executables.get(&operation.surface) {
            Some(executable) => {
                let arguments = executable
                    .arguments
                    .iter()
                    .map(OsStr::new)
                    .collect::<Vec<_>>();
                match native_process::spawn_detached(&executable.path, &arguments) {
                    Ok(()) => LoginStepResult {
                        surface: operation.surface,
                        status: LoginStepStatus::ActionRequired,
                        detail: launched_detail(operation.surface).to_string(),
                    },
                    Err(error) => LoginStepResult {
                        surface: operation.surface,
                        status: LoginStepStatus::Failed,
                        detail: redact::redact_text(&error.to_string()),
                    },
                }
            }
            None => {
                let status = if operation
                    .description
                    .to_ascii_lowercase()
                    .contains("install")
                {
                    LoginStepStatus::SkippedNotInstalled
                } else {
                    LoginStepStatus::Unsupported
                };
                LoginStepResult {
                    surface: operation.surface,
                    status,
                    detail: operation.description.clone(),
                }
            }
        };
        steps.push(step);
    }

    let launched = steps
        .iter()
        .filter(|step| step.status == LoginStepStatus::ActionRequired)
        .count();
    let failed = steps
        .iter()
        .filter(|step| step.status == LoginStepStatus::Failed)
        .count();
    run.status = match (launched, failed) {
        (0, 0) => LoginRunStatus::Failed,
        (0, _) => LoginRunStatus::Failed,
        (_, 0) => LoginRunStatus::ActionRequired,
        _ => LoginRunStatus::Partial,
    };
    run.summary = match run.status {
        LoginRunStatus::ActionRequired => {
            "Official sign-in flows were opened; complete them in each client, then refresh account status"
                .to_string()
        }
        LoginRunStatus::Partial => {
            "Some sign-in flows opened while other clients could not be launched".to_string()
        }
        LoginRunStatus::Failed => "No sign-in flow could be launched".to_string(),
        _ => "Sign-in run finished".to_string(),
    };
    run.steps = steps;
    run.finished_at = Some(Utc::now());
    run
}

fn observe_github_cli_account(observed_at: DateTime<Utc>) -> AccountAnchorObservation {
    let Some(gh) = find_gh_executable() else {
        return AccountAnchorObservation {
            state: AccountObservationState::NotInstalled,
            identity: None,
            evidence: "GitHub CLI was not found as a regular executable".to_string(),
            detail: "A verified github.com account anchor is unavailable".to_string(),
            observed_at,
        };
    };
    let arguments = [
        OsStr::new("api"),
        OsStr::new("user"),
        OsStr::new("--hostname"),
        OsStr::new("github.com"),
        OsStr::new("--jq"),
        OsStr::new("{login: .login, id: .id, avatarUrl: .avatar_url}"),
    ];
    let output = match native_process::run_capture_bounded(
        &gh,
        &arguments,
        Duration::from_secs(GH_ACCOUNT_TIMEOUT_SECONDS),
        MAX_GH_OUTPUT_BYTES,
    ) {
        Ok(output) => output,
        Err(error) => {
            return AccountAnchorObservation {
                state: AccountObservationState::Unknown,
                identity: None,
                evidence: "GitHub CLI API query failed safely".to_string(),
                detail: bounded_detail(&error.to_string()),
                observed_at,
            }
        }
    };
    if !output.status.success() {
        let detail = if output.stderr.trim().is_empty() {
            "GitHub CLI is not authenticated to github.com".to_string()
        } else {
            bounded_detail(&output.stderr)
        };
        return AccountAnchorObservation {
            state: AccountObservationState::ActionRequired,
            identity: None,
            evidence: "gh api user --hostname github.com".to_string(),
            detail,
            observed_at,
        };
    }
    if output.stdout_truncated || output.stderr_truncated {
        return AccountAnchorObservation {
            state: AccountObservationState::Unknown,
            identity: None,
            evidence: "GitHub CLI API response exceeded the safety limit".to_string(),
            detail: "The account response was not parsed".to_string(),
            observed_at,
        };
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GhUser {
        login: String,
        id: u64,
        avatar_url: Option<String>,
    }

    match serde_json::from_str::<GhUser>(&output.stdout) {
        Ok(user) if valid_login(&user.login) => AccountAnchorObservation {
            state: AccountObservationState::Verified,
            identity: Some(GithubIdentity {
                host: "github.com".to_string(),
                login: user.login,
                user_id: Some(user.id),
                avatar_url: user.avatar_url.and_then(validate_avatar_url),
            }),
            evidence: "Authenticated GitHub API /user response through GitHub CLI".to_string(),
            detail: "This account is a verified anchor only; client-specific identities remain separately qualified"
                .to_string(),
            observed_at,
        },
        Ok(_) => AccountAnchorObservation {
            state: AccountObservationState::Unknown,
            identity: None,
            evidence: "GitHub API returned an invalid login identifier".to_string(),
            detail: "The account response was rejected".to_string(),
            observed_at,
        },
        Err(error) => AccountAnchorObservation {
            state: AccountObservationState::Unknown,
            identity: None,
            evidence: "GitHub CLI API response schema was not recognized".to_string(),
            detail: bounded_detail(&error.to_string()),
            observed_at,
        },
    }
}

fn observe_surface(
    surface: LoginSurface,
    observed_at: DateTime<Utc>,
    anchor: &AccountAnchorObservation,
    clients: &[ClientTarget],
    components: &[installer::InstallComponentObservation],
    environment_overrides: &[String],
) -> SurfaceAccountObservation {
    if !surface_installed(surface, clients, components) {
        return SurfaceAccountObservation {
            surface,
            state: AccountObservationState::NotInstalled,
            identity: None,
            evidence: "Local component discovery".to_string(),
            detail: "The client must be installed before account verification".to_string(),
            observed_at,
        };
    }

    match surface {
        LoginSurface::CopilotCli if !environment_overrides.is_empty() => {
            SurfaceAccountObservation {
                surface,
                state: AccountObservationState::Unknown,
                identity: None,
                evidence: format!(
                    "Authentication override variable(s) are present: {}",
                    environment_overrides.join(", ")
                ),
                detail: "An environment credential can override the OAuth account stored by Copilot CLI; PilotWeave did not read any value"
                    .to_string(),
                observed_at,
            }
        }
        LoginSurface::CopilotCli if anchor.state == AccountObservationState::Verified => {
            SurfaceAccountObservation {
                surface,
                state: AccountObservationState::Inferred,
                identity: anchor.identity.clone(),
                evidence: "GitHub CLI is a documented lowest-priority Copilot CLI authentication fallback"
                    .to_string(),
                detail: "A stored Copilot CLI OAuth account may take precedence; verify inside Copilot CLI with /user"
                    .to_string(),
                observed_at,
            }
        }
        LoginSurface::CopilotCli => SurfaceAccountObservation {
            surface,
            state: AccountObservationState::ActionRequired,
            identity: None,
            evidence: "No stable token-free client identity interface was used".to_string(),
            detail: "Open the official sign-in flow, then verify the selected account with /user"
                .to_string(),
            observed_at,
        },
        LoginSurface::VsCodeCopilot => SurfaceAccountObservation {
            surface,
            state: AccountObservationState::ActionRequired,
            identity: None,
            evidence: "VS Code authentication secrets and SecretStorage were not inspected".to_string(),
            detail: "Use the official VS Code Accounts interface and confirm the github.com account"
                .to_string(),
            observed_at,
        },
        LoginSurface::GithubCopilotApp => SurfaceAccountObservation {
            surface,
            state: AccountObservationState::ActionRequired,
            identity: None,
            evidence: "GitHub Copilot app private authentication state was not inspected".to_string(),
            detail: "Use the app's official Sign in to GitHub flow and confirm the account"
                .to_string(),
            observed_at,
        },
    }
}

fn surface_installed(
    surface: LoginSurface,
    clients: &[ClientTarget],
    components: &[installer::InstallComponentObservation],
) -> bool {
    let client_kind = match surface {
        LoginSurface::VsCodeCopilot => ClientKind::VsCodeCopilot,
        LoginSurface::CopilotCli => ClientKind::CopilotCli,
        LoginSurface::GithubCopilotApp => ClientKind::GithubCopilotApp,
    };
    let client_detected = clients
        .iter()
        .any(|client| client.kind == client_kind && client.detected);
    let component_id = match surface {
        LoginSurface::VsCodeCopilot => installer::COMPONENT_VSCODE,
        LoginSurface::CopilotCli => installer::COMPONENT_COPILOT_CLI,
        LoginSurface::GithubCopilotApp => installer::COMPONENT_COPILOT_APP,
    };
    client_detected
        || components.iter().any(|component| {
            component.id == component_id && component.status == InstallComponentStatus::Ready
        })
}

fn executable_operation(surface: LoginSurface) -> AppResult<ExecutableLoginOperation> {
    let path = match surface {
        LoginSurface::VsCodeCopilot => find_vscode_executable(),
        LoginSurface::CopilotCli => find_copilot_executable(),
        LoginSurface::GithubCopilotApp => find_copilot_app_executable(),
    }
    .ok_or_else(|| {
        AppError::Unsupported(format!(
            "{} is installed but a safe regular launcher was not resolved",
            surface.display_name()
        ))
    })?;
    let fingerprint = native_process::fingerprint_regular_file(&path)?;
    let arguments = match surface {
        LoginSurface::VsCodeCopilot => vec!["--reuse-window".to_string()],
        LoginSurface::CopilotCli => vec![
            "login".to_string(),
            "--host".to_string(),
            "https://github.com".to_string(),
            "--web-flow".to_string(),
        ],
        LoginSurface::GithubCopilotApp => Vec::new(),
    };
    Ok(ExecutableLoginOperation {
        path,
        fingerprint,
        arguments,
    })
}

fn operation_description(surface: LoginSurface) -> &'static str {
    match surface {
        LoginSurface::VsCodeCopilot => {
            "Launch the verified VS Code application; complete sign-in through its official Accounts interface"
        }
        LoginSurface::CopilotCli => {
            "Launch the verified Copilot CLI with its official github.com browser sign-in flow"
        }
        LoginSurface::GithubCopilotApp => {
            "Launch the verified GitHub Copilot application; use its official Sign in to GitHub control"
        }
    }
}

fn launched_detail(surface: LoginSurface) -> &'static str {
    match surface {
        LoginSurface::VsCodeCopilot => {
            "VS Code was opened. Complete or verify GitHub sign-in in the Accounts interface; no credential was copied"
        }
        LoginSurface::CopilotCli => {
            "Copilot CLI browser sign-in was launched with github.com fixed by the backend; complete the browser flow and verify with /user"
        }
        LoginSurface::GithubCopilotApp => {
            "GitHub Copilot app was opened. Complete its Sign in to GitHub flow; no private app state was read or written"
        }
    }
}

fn canonical_surfaces(values: &[LoginSurface]) -> AppResult<Vec<LoginSurface>> {
    let mut values = if values.is_empty() {
        vec![
            LoginSurface::VsCodeCopilot,
            LoginSurface::CopilotCli,
            LoginSurface::GithubCopilotApp,
        ]
    } else {
        values.to_vec()
    };
    values.sort();
    values.dedup();
    if values.len() > 3 {
        return Err(AppError::InvalidInput(
            "Too many sign-in surfaces were requested".to_string(),
        ));
    }
    Ok(values)
}

fn present_auth_environment() -> Vec<String> {
    GITHUB_AUTH_ENVIRONMENT
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .map(|name| (*name).to_string())
        .collect()
}

fn account_fingerprint(status: &AccountStatusSnapshot) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    status.anchor.state.hash(&mut hasher);
    status.anchor.identity.hash(&mut hasher);
    for surface in &status.surfaces {
        surface.surface.hash(&mut hasher);
        surface.state.hash(&mut hasher);
        surface.identity.hash(&mut hasher);
        surface.evidence.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn find_gh_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        native_process::resolve_on_path(&["gh.exe"])
    }
    #[cfg(not(windows))]
    {
        native_process::resolve_on_path(&["gh"])
    }
}

fn find_copilot_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        native_process::resolve_on_path(&["copilot.exe"])
    }
    #[cfg(not(windows))]
    {
        native_process::resolve_on_path(&["copilot"])
    }
}

fn find_vscode_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            let root = PathBuf::from(root).join("Programs");
            candidates.push(root.join("Microsoft VS Code/Code.exe"));
            candidates.push(root.join("Microsoft VS Code Insiders/Code - Insiders.exe"));
        }
        if let Some(root) = std::env::var_os("PROGRAMFILES") {
            let root = PathBuf::from(root);
            candidates.push(root.join("Microsoft VS Code/Code.exe"));
            candidates.push(root.join("Microsoft VS Code Insiders/Code - Insiders.exe"));
        }
        native_process::resolve_on_path(&["code.exe", "Code.exe"])
            .or_else(|| native_process::resolve_candidates(candidates))
    }
    #[cfg(target_os = "macos")]
    {
        native_process::resolve_on_path(&["code", "code-insiders"]).or_else(|| {
            native_process::resolve_candidates([
                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
                "/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Electron",
            ])
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        native_process::resolve_on_path(&["code", "code-insiders"])
    }
}

fn find_copilot_app_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(root).join("Programs/GitHub Copilot/GitHub Copilot.exe"));
        }
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(root) = std::env::var_os(variable) {
                candidates.push(PathBuf::from(root).join("GitHub Copilot/GitHub Copilot.exe"));
            }
        }
        native_process::resolve_candidates(candidates)
    }
    #[cfg(target_os = "macos")]
    {
        native_process::resolve_candidates([
            "/Applications/GitHub Copilot.app/Contents/MacOS/GitHub Copilot",
        ])
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut candidates = vec![
            PathBuf::from("/opt/GitHub Copilot/github-copilot"),
            PathBuf::from("/usr/bin/github-copilot"),
        ];
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".local/bin/github-copilot"));
            candidates.push(home.join("Applications/GitHub-Copilot.AppImage"));
        }
        native_process::resolve_on_path(&["github-copilot"])
            .or_else(|| native_process::resolve_candidates(candidates))
    }
}

fn valid_login(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn validate_avatar_url(value: String) -> Option<String> {
    let url = Url::parse(&value).ok()?;
    (url.scheme() == "https" && url.host_str().is_some()).then_some(value)
}

fn bounded_detail(value: &str) -> String {
    let value = redact::redact_text(value.trim());
    if value.len() <= MAX_ACCOUNT_TEXT_BYTES {
        value
    } else {
        format!("{}…", &value[..MAX_ACCOUNT_TEXT_BYTES])
    }
}

fn validate_run(run: &LoginRunRecord) -> AppResult<()> {
    for value in [&run.id, &run.plan_id, &run.summary] {
        if value.is_empty() || value.len() > MAX_ACCOUNT_TEXT_BYTES {
            return Err(AppError::InvalidInput(
                "Sign-in history contains an invalid bounded text field".to_string(),
            ));
        }
    }
    if run.requested_surfaces.is_empty() || run.requested_surfaces.len() > 3 {
        return Err(AppError::InvalidInput(
            "Sign-in history contains an invalid surface set".to_string(),
        ));
    }
    if run.steps.len() > 3 {
        return Err(AppError::InvalidInput(
            "Sign-in history contains too many steps".to_string(),
        ));
    }
    for step in &run.steps {
        if step.detail.len() > MAX_ACCOUNT_TEXT_BYTES {
            return Err(AppError::InvalidInput(
                "Sign-in history detail exceeds the safety limit".to_string(),
            ));
        }
    }
    Ok(())
}

fn load_history(path: &Path) -> AppResult<LoginHistoryState> {
    if !path.exists() {
        return Ok(LoginHistoryState::default());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::InvalidInput(format!(
            "Sign-in history must be a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_LOGIN_HISTORY_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Sign-in history exceeds {} MiB",
            MAX_LOGIN_HISTORY_BYTES / 1_024 / 1_024
        )));
    }
    let bytes = fs::read(path).map_err(|error| AppError::io(path, error))?;
    let state: LoginHistoryState =
        serde_json::from_slice(&bytes).map_err(|error| AppError::json(path, error))?;
    if state.version > LOGIN_HISTORY_VERSION {
        return Err(AppError::Config(format!(
            "Sign-in history version {} is newer than this build supports ({LOGIN_HISTORY_VERSION})",
            state.version
        )));
    }
    if state.runs.len() > MAX_LOGIN_RUNS {
        return Err(AppError::InvalidInput(format!(
            "Sign-in history contains more than {MAX_LOGIN_RUNS} runs"
        )));
    }
    for run in &state.runs {
        validate_run(run)?;
    }
    Ok(state)
}

fn write_history(path: &Path, state: &LoginHistoryState) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Sign-in history path has no parent".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
        AppError::Config(format!("Failed to serialize sign-in history: {error}"))
    })?;
    if bytes.len() as u64 > MAX_LOGIN_HISTORY_BYTES {
        return Err(AppError::InvalidInput(
            "Sign-in history exceeds its storage limit".to_string(),
        ));
    }
    let temp = parent.join(format!(".login-runs-{}.tmp", Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut file = options
            .open(&temp)
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| AppError::io(&temp, error))?;
        }
        file.write_all(&bytes)
            .map_err(|error| AppError::io(&temp, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
        }
        fs::rename(&temp, path).map_err(|error| AppError::io(path, error))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_plans_are_one_shot() {
        let mut store = LoginPlanStore::default();
        let plan = store
            .preview(vec![LoginSurface::GithubCopilotApp])
            .expect("preview");
        let _ = store.consume(&plan.id).expect("consume");
        assert!(store.consume(&plan.id).is_err());
    }

    #[test]
    fn empty_surface_request_selects_all_clients() {
        let values = canonical_surfaces(&[]).expect("surfaces");
        assert_eq!(values.len(), 3);
        assert!(values.contains(&LoginSurface::VsCodeCopilot));
        assert!(values.contains(&LoginSurface::CopilotCli));
        assert!(values.contains(&LoginSurface::GithubCopilotApp));
    }

    #[test]
    fn invalid_github_logins_are_rejected() {
        assert!(valid_login("octocat"));
        assert!(!valid_login("-octocat"));
        assert!(!valid_login("octo_cat"));
        assert!(!valid_login(""));
    }

    #[test]
    fn interrupted_runs_are_recovered_on_open() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("login-runs.json");
        let run = LoginRunRecord {
            id: "run".to_string(),
            plan_id: "plan".to_string(),
            target_identity: None,
            requested_surfaces: vec![LoginSurface::CopilotCli],
            status: LoginRunStatus::InProgress,
            steps: vec![LoginStepResult {
                surface: LoginSurface::CopilotCli,
                status: LoginStepStatus::Pending,
                detail: "pending".to_string(),
            }],
            summary: "pending".to_string(),
            started_at: Utc::now(),
            finished_at: None,
        };
        write_history(
            &path,
            &LoginHistoryState {
                version: LOGIN_HISTORY_VERSION,
                runs: vec![run],
            },
        )
        .expect("write");

        let store = LoginStore::open_at(path);
        assert_eq!(store.runs()[0].status, LoginRunStatus::Interrupted);
    }

    #[test]
    fn corrupt_history_enters_read_only_recovery() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("login-runs.json");
        fs::write(&path, b"not json").expect("write");
        let mut store = LoginStore::open_at(path.clone());
        assert!(store.recovery().is_some());
        let plan = LoginPlan {
            id: "plan".to_string(),
            target_identity: None,
            requested_surfaces: vec![LoginSurface::CopilotCli],
            operations: Vec::new(),
            created_at: Utc::now(),
            expires_at: Utc::now() + ChronoDuration::minutes(15),
        };
        assert!(store.begin_run(&plan).is_err());
        assert_eq!(fs::read(&path).expect("history"), b"not json");
    }
}
