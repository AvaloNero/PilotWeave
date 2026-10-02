#[cfg(windows)]
use crate::adapters::github_app;
#[cfg(windows)]
use crate::adapters::vscode_install::{self, InstalledCopilot, VsCodeCli};
use crate::error::{AppError, AppResult};
#[cfg(windows)]
use crate::native_process::{CaptureMode, CapturedOutput};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicBool, Arc};
use uuid::Uuid;
mod catalog;
pub(crate) mod history;
#[cfg(windows)]
mod identity;
#[cfg(windows)]
mod source;

const INSTALL_PLAN_TTL_SECONDS: i64 = 15 * 60;

pub const COMPONENT_VSCODE: &str = "vscode";
pub const COMPONENT_VSCODE_COPILOT: &str = "vscode-copilot-extension";
pub const COMPONENT_COPILOT_CLI: &str = "copilot-cli";
pub const COMPONENT_COPILOT_APP: &str = "github-copilot-app";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallComponentStatus {
    Ready,
    Missing,
    Unsupported,
    Broken,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallComponentObservation {
    pub id: String,
    pub name: String,
    pub status: InstallComponentStatus,
    pub detail: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStrategy {
    WingetPackage,
    VsCodeExtension,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOperation {
    pub id: String,
    pub component_id: String,
    pub component_name: String,
    pub strategy: InstallStrategy,
    pub source: String,
    pub requires_elevation: bool,
    pub description: String,
    pub version: Option<String>,
    pub architecture: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub id: String,
    pub requested_component_ids: Vec<String>,
    pub operations: Vec<InstallOperation>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(skip)]
    executable_bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallResultStatus {
    CompletedAndVerified,
    ProcessSucceededVerificationFailed,
    SkippedAlreadyReady,
    SkippedDependencyFailed,
    Failed,
    Unsupported,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOperationResult {
    pub component_id: String,
    pub status: InstallResultStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApplyResult {
    pub plan_id: String,
    pub results: Vec<InstallOperationResult>,
    pub observations: Vec<InstallComponentObservation>,
}

#[derive(Debug, Clone)]
struct StoredInstallPlan {
    plan: InstallPlan,
    observation_fingerprints: BTreeMap<String, String>,
    deadline: std::time::Instant,
}

#[derive(Default)]
pub struct InstallPlanStore {
    plans: HashMap<String, StoredInstallPlan>,
}

impl InstallPlanStore {
    pub fn preview(&mut self, requested_component_ids: Vec<String>) -> AppResult<InstallPlan> {
        self.purge_expired();
        if self.plans.len() >= 16 {
            return Err(AppError::InvalidInput(
                "Too many pending plans; wait for expiry or apply an existing plan".into(),
            ));
        }
        let observations = discover_components();
        let requested = canonical_component_ids(&requested_component_ids)?;
        let mut operations = Vec::new();
        for component_id in &requested {
            let observation = observation(&observations, component_id)?;
            if !requires_install(observation.status)? {
                continue;
            }
            operations.push(operation_for(component_id)?);
        }
        operations.sort_by_key(|operation| operation_rank(&operation.component_id));
        let executable_bindings = executable_bindings(&requested, &operations)?;
        let created_at = Utc::now();
        let plan = InstallPlan {
            id: Uuid::new_v4().to_string(),
            requested_component_ids: requested.clone(),
            operations,
            created_at,
            expires_at: created_at + Duration::seconds(INSTALL_PLAN_TTL_SECONDS),
            executable_bindings,
        };
        let observation_fingerprints = requested
            .iter()
            .map(|id| {
                let value = observation(&observations, id)?;
                Ok((id.clone(), observation_fingerprint(value)))
            })
            .collect::<AppResult<BTreeMap<_, _>>>()?;
        self.plans.insert(
            plan.id.clone(),
            StoredInstallPlan {
                plan: plan.clone(),
                observation_fingerprints,
                deadline: std::time::Instant::now()
                    + std::time::Duration::from_secs(INSTALL_PLAN_TTL_SECONDS as u64),
            },
        );
        Ok(plan)
    }

    pub fn consume(&mut self, plan_id: &str) -> AppResult<InstallPlan> {
        let stored = self.take(plan_id)?;
        let current = discover_components();
        for component_id in &stored.plan.requested_component_ids {
            let before = stored
                .observation_fingerprints
                .get(component_id)
                .ok_or_else(|| AppError::Config("Install plan is missing an observation".into()))?;
            let now = observation_fingerprint(observation(&current, component_id)?);
            if &now != before {
                return Err(AppError::InvalidInput(format!(
                    "Component state changed after preview: {component_id}; preview again"
                )));
            }
        }
        if executable_bindings(
            &stored.plan.requested_component_ids,
            &stored.plan.operations,
        )? != stored.plan.executable_bindings
        {
            return Err(AppError::InvalidInput(
                "An installation executable changed after preview; preview again".into(),
            ));
        }
        Ok(stored.plan)
    }

    fn take(&mut self, plan_id: &str) -> AppResult<StoredInstallPlan> {
        self.purge_expired();
        self.plans.remove(plan_id).ok_or_else(|| {
            AppError::InvalidInput("Install plan is missing, expired or consumed".into())
        })
    }

    fn purge_expired(&mut self) {
        let now = Utc::now();
        let monotonic_now = std::time::Instant::now();
        self.plans
            .retain(|_, stored| stored.plan.expires_at >= now && stored.deadline > monotonic_now);
    }
}

pub fn discover_components() -> Vec<InstallComponentObservation> {
    #[cfg(windows)]
    {
        discover_windows_components()
    }
    #[cfg(not(windows))]
    {
        vec![
            unsupported(COMPONENT_VSCODE, "Visual Studio Code"),
            unsupported(COMPONENT_VSCODE_COPILOT, "GitHub Copilot extension"),
            unsupported(COMPONENT_COPILOT_CLI, "GitHub Copilot CLI"),
            unsupported(COMPONENT_COPILOT_APP, "GitHub Copilot app"),
        ]
    }
}

pub fn execute_plan(
    plan: InstallPlan,
    cancel: Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str, Option<InstallOperationResult>) -> AppResult<()>,
) -> AppResult<InstallApplyResult> {
    #[cfg(windows)]
    {
        let mut runner = NativeRunner {
            cancel: cancel.clone(),
        };
        apply_windows_plan(&plan, &mut runner, &cancel, progress)
    }
    #[cfg(not(windows))]
    {
        let _ = (cancel, progress);
        Ok(InstallApplyResult {
            plan_id: plan.id,
            results: plan
                .requested_component_ids
                .iter()
                .map(|component_id| InstallOperationResult {
                    component_id: component_id.clone(),
                    status: InstallResultStatus::Unsupported,
                    detail: "One-click installation is currently supported only on Windows"
                        .to_string(),
                })
                .collect(),
            observations: discover_components(),
        })
    }
}

#[cfg(windows)]
fn discover_windows_components() -> Vec<InstallComponentObservation> {
    let code = vscode_install::find_executable();
    let copilot = find_on_path("copilot.exe").or_else(|| find_on_path("copilot.cmd"));
    let app = github_app::installation_path();
    let cli = code.as_deref().map(VsCodeCli::resolve);
    let extension = match cli.as_ref() {
        None => extension_observation(Ok(None)),
        Some(Ok(cli)) => extension_observation(cli.copilot()),
        Some(Err(_)) => extension_observation(Err(AppError::Unsupported(
            "VS Code CLI layout could not be verified; rescan after its update finishes".into(),
        ))),
    };
    let mut code_observation = observation_from_path(COMPONENT_VSCODE, "Visual Studio Code", code);
    match cli {
        Some(Ok(cli)) => code_observation.version = Some(cli.version),
        Some(Err(_)) => {
            code_observation.status = InstallComponentStatus::Unknown;
            code_observation.detail =
                "VS Code exists, but its supported CLI identity and version could not be verified"
                    .into();
        }
        None => {}
    }

    vec![
        code_observation,
        extension,
        verified_observation(
            COMPONENT_COPILOT_CLI,
            "GitHub Copilot CLI",
            copilot,
            cli_version,
        ),
        verified_observation(
            COMPONENT_COPILOT_APP,
            "GitHub Copilot app",
            app,
            app_version,
        ),
    ]
}

fn requires_install(status: InstallComponentStatus) -> AppResult<bool> {
    match status {
        InstallComponentStatus::Ready => Ok(false),
        InstallComponentStatus::Missing | InstallComponentStatus::Broken => Ok(true),
        InstallComponentStatus::Unknown | InstallComponentStatus::Unsupported => Err(AppError::Unsupported(
            "Component status is unknown or unsupported; refresh and resolve detection before creating an installation plan".into(),
        )),
    }
}

#[cfg(windows)]
fn extension_observation(
    result: AppResult<Option<InstalledCopilot>>,
) -> InstallComponentObservation {
    let (status, detail, version) = match result {
        Ok(Some(extension)) => (InstallComponentStatus::Ready, extension.detail, Some(extension.version)),
        Ok(None) => (InstallComponentStatus::Missing, "Copilot was not found in the selected VS Code installation or its registered profiles".into(), None),
        Err(_) => (InstallComponentStatus::Unknown, "Copilot installation could not be verified. Refresh after VS Code finishes updating; detection failure does not mean it is missing.".into(), None),
    };
    InstallComponentObservation {
        id: COMPONENT_VSCODE_COPILOT.into(),
        name: "GitHub Copilot capability".into(),
        status,
        detail,
        version,
    }
}

fn operation_for(component_id: &str) -> AppResult<InstallOperation> {
    let (name, strategy, source, requires_elevation, description) = match component_id {
        COMPONENT_VSCODE => (
            "Visual Studio Code",
            InstallStrategy::WingetPackage,
            "WinGet: Microsoft.VisualStudioCode",
            false,
            "Install the exact Microsoft.VisualStudioCode package from the WinGet source",
        ),
        COMPONENT_VSCODE_COPILOT => (
            "GitHub Copilot extension",
            InstallStrategy::VsCodeExtension,
            "Visual Studio Marketplace: GitHub.copilot-chat",
            false,
            "Use the verified VS Code CLI to install GitHub.copilot-chat in the default profile",
        ),
        COMPONENT_COPILOT_CLI => (
            "GitHub Copilot CLI",
            InstallStrategy::WingetPackage,
            "WinGet: GitHub.Copilot",
            false,
            "Install the exact GitHub.Copilot package from the WinGet source",
        ),
        COMPONENT_COPILOT_APP => (
            "GitHub Copilot app",
            InstallStrategy::WingetPackage,
            "WinGet: GitHub.CopilotApp",
            false,
            "Install the exact GitHub.CopilotApp package whose manifest points to github/app releases",
        ),
        _ => {
            return Err(AppError::InvalidInput(format!(
                "Unknown install component: {component_id}"
            )))
        }
    };
    Ok(InstallOperation {
        id: Uuid::new_v4().to_string(),
        component_id: component_id.to_string(),
        component_name: name.to_string(),
        strategy,
        source: source.to_string(),
        requires_elevation,
        description: description.to_string(),
        version: (strategy == InstallStrategy::WingetPackage)
            .then(|| catalog::package(component_id).map(|p| p.version.to_string()))
            .transpose()?,
        architecture: "x64".into(),
        dependencies: if strategy == InstallStrategy::WingetPackage {
            catalog::package(component_id)?
                .dependencies
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            vec!["A verified VS Code installation is required".into()]
        },
    })
}

fn operation_rank(component_id: &str) -> u8 {
    match component_id {
        COMPONENT_VSCODE => 0,
        COMPONENT_VSCODE_COPILOT => 1,
        COMPONENT_COPILOT_CLI => 2,
        COMPONENT_COPILOT_APP => 3,
        _ => 10,
    }
}

fn canonical_component_ids(values: &[String]) -> AppResult<Vec<String>> {
    if values.len() > 16 || values.iter().any(|v| v.len() > 64) {
        return Err(AppError::InvalidInput(
            "Install component request exceeds limits".into(),
        ));
    }
    let mut values = if values.is_empty() {
        vec![
            COMPONENT_VSCODE.to_string(),
            COMPONENT_VSCODE_COPILOT.to_string(),
            COMPONENT_COPILOT_CLI.to_string(),
            COMPONENT_COPILOT_APP.to_string(),
        ]
    } else {
        values.to_vec()
    };
    values.sort();
    values.dedup();
    if values.len() > 4 {
        return Err(AppError::InvalidInput("Too many install components".into()));
    }
    for value in &values {
        operation_for(value)?;
    }
    Ok(values)
}

fn observation<'a>(
    observations: &'a [InstallComponentObservation],
    component_id: &str,
) -> AppResult<&'a InstallComponentObservation> {
    observations
        .iter()
        .find(|item| item.id == component_id)
        .ok_or_else(|| AppError::Config(format!("Missing component observation: {component_id}")))
}

fn observation_fingerprint(value: &InstallComponentObservation) -> String {
    format!(
        "{}|{:?}|{}|{}",
        value.id,
        value.status,
        value.detail,
        value.version.as_deref().unwrap_or_default()
    )
}

#[cfg(not(windows))]
fn unsupported(id: &str, name: &str) -> InstallComponentObservation {
    InstallComponentObservation {
        id: id.to_string(),
        name: name.to_string(),
        status: InstallComponentStatus::Unsupported,
        detail: "One-click installation is currently supported only on Windows".to_string(),
        version: None,
    }
}

#[cfg(windows)]
fn observation_from_path(
    id: &str,
    name: &str,
    path: Option<PathBuf>,
) -> InstallComponentObservation {
    InstallComponentObservation {
        id: id.to_string(),
        name: name.to_string(),
        status: if path.is_some() {
            InstallComponentStatus::Ready
        } else {
            InstallComponentStatus::Missing
        },
        detail: path
            .as_deref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "Not detected".to_string()),
        version: None,
    }
}

#[cfg(windows)]
fn verified_observation(
    id: &str,
    name: &str,
    path: Option<PathBuf>,
    probe: impl FnOnce(&Path) -> AppResult<String>,
) -> InstallComponentObservation {
    let mut value = observation_from_path(id, name, path.clone());
    if let Some(path) = path {
        match probe(&path) {
            Ok(version) => {
                value.version = Some(version);
                value.detail = "Installed program identity/version was re-detected; account sign-in is separate".into();
            }
            Err(_) => {
                value.status = InstallComponentStatus::Unknown;
                value.detail = "A candidate exists, but its identity/version could not be verified. No replacement or downgrade is authorized".into();
            }
        }
    }
    value
}

#[cfg(windows)]
fn cli_version(path: &Path) -> AppResult<String> {
    if !path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    {
        return Err(AppError::Unsupported(
            "Script-wrapped CLI requires manual identity verification".into(),
        ));
    }
    let output = crate::native_process::run_capture_bounded(
        path,
        &[std::ffi::OsStr::new("--version")],
        std::time::Duration::from_secs(10),
        8192,
    )?;
    if !output.status.success() || output.stdout_truncated || output.stderr_truncated {
        return Err(AppError::Unsupported("CLI version probe failed".into()));
    }
    parse_cli_version(&output.stdout)
}

#[cfg(any(windows, test))]
fn parse_cli_version(text: &str) -> AppResult<String> {
    let first = text.lines().next().unwrap_or_default().trim();
    let version = first
        .strip_prefix("GitHub Copilot CLI ")
        .or_else(|| first.strip_prefix("GitHub Copilot CLI version "))
        .and_then(|s| s.strip_prefix("version ").or(Some(s)))
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or_default()
        .trim_start_matches('v');
    if !valid_version(version) || text.len() > 8192 {
        return Err(AppError::Unsupported(
            "Unsupported Copilot CLI version-output schema".into(),
        ));
    }
    Ok(version.into())
}

#[cfg(any(windows, test))]
fn valid_version(text: &str) -> bool {
    let fields = text.split('.').collect::<Vec<_>>();
    fields.len() == 3
        && text.len() <= 32
        && fields
            .iter()
            .all(|s| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(windows)]
fn app_version(path: &Path) -> AppResult<String> {
    let product = identity::product(path)?;
    if product.product != "GitHub Copilot" || !valid_version(&product.version) {
        return Err(AppError::Unsupported(
            "Unsupported GitHub Copilot app product metadata".into(),
        ));
    }
    Ok(product.version)
}

fn executable_bindings(
    requested: &[String],
    operations: &[InstallOperation],
) -> AppResult<BTreeMap<String, String>> {
    #[cfg(windows)]
    {
        if !cfg!(target_arch = "x86_64") && !operations.is_empty() {
            return Err(AppError::Unsupported(
                "Reviewed one-click installation currently supports Windows x64 only".into(),
            ));
        }
        let mut bindings = BTreeMap::new();
        for id in requested {
            let path = match id.as_str() {
                COMPONENT_VSCODE | COMPONENT_VSCODE_COPILOT => vscode_install::find_executable(),
                COMPONENT_COPILOT_CLI => {
                    find_on_path("copilot.exe").or_else(|| find_on_path("copilot.cmd"))
                }
                COMPONENT_COPILOT_APP => github_app::installation_path(),
                _ => None,
            };
            if let Some(path) = path {
                bindings.insert(
                    id.clone(),
                    crate::native_process::fingerprint_regular_file(&path)?,
                );
                if matches!(id.as_str(), COMPONENT_VSCODE | COMPONENT_VSCODE_COPILOT) {
                    let cli = VsCodeCli::resolve(&path)?;
                    if let Some(entry) = cli.args(&[]).first() {
                        bindings.insert(
                            format!("{id}:cli-entry"),
                            crate::native_process::fingerprint_regular_file(Path::new(entry))?,
                        );
                    }
                }
            }
        }
        if operations
            .iter()
            .any(|o| o.strategy == InstallStrategy::WingetPackage)
        {
            let path = identity::winget_path()?;
            bindings.insert(
                "winget".into(),
                crate::native_process::fingerprint_regular_file(&path)?,
            );
            bindings.insert("winget-source".into(), source::observe(&path, None)?);
        }
        Ok(bindings)
    }
    #[cfg(not(windows))]
    {
        let _ = (requested, operations);
        Ok(BTreeMap::new())
    }
}

#[cfg(windows)]
trait ProcessRunner {
    fn run(
        &mut self,
        executable: &Path,
        args: &[&std::ffi::OsStr],
        mode: CaptureMode,
    ) -> AppResult<CapturedOutput>;
}

#[cfg(windows)]
struct NativeRunner {
    cancel: Arc<AtomicBool>,
}

#[cfg(windows)]
impl ProcessRunner for NativeRunner {
    fn run(
        &mut self,
        executable: &Path,
        args: &[&std::ffi::OsStr],
        mode: CaptureMode,
    ) -> AppResult<CapturedOutput> {
        crate::native_process::run_capture_cancelable(
            executable,
            args,
            std::time::Duration::from_secs(20 * 60),
            128 * 1024,
            mode,
            Some(&self.cancel),
        )
    }
}

#[cfg(windows)]
fn apply_windows_plan(
    plan: &InstallPlan,
    runner: &mut dyn ProcessRunner,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, Option<InstallOperationResult>) -> AppResult<()>,
) -> AppResult<InstallApplyResult> {
    let mut results = Vec::new();
    let winget = if plan
        .operations
        .iter()
        .any(|o| o.strategy == InstallStrategy::WingetPackage)
    {
        Some(identity::winget_path()?)
    } else {
        None
    };

    for id in &plan.requested_component_ids {
        if !plan
            .operations
            .iter()
            .any(|operation| &operation.component_id == id)
        {
            record_result(
                &mut results,
                InstallOperationResult {
                    component_id: id.clone(),
                    status: InstallResultStatus::SkippedAlreadyReady,
                    detail:
                        "Component was verified ready in the reviewed plan; no installer was needed"
                            .into(),
                },
                progress,
            )?;
        }
    }

    for operation in &plan.operations {
        if cancel.load(Ordering::Acquire) {
            record_result(
                &mut results,
                InstallOperationResult {
                    component_id: operation.component_id.clone(),
                    status: InstallResultStatus::Cancelled,
                    detail: "Not started because cancellation was requested".into(),
                },
                progress,
            )?;
            continue;
        }
        progress(&operation.component_id, None)?;
        let before = discover_windows_components();
        if observation(&before, &operation.component_id)?.status == InstallComponentStatus::Ready {
            record_result(
                &mut results,
                InstallOperationResult {
                    component_id: operation.component_id.clone(),
                    status: InstallResultStatus::SkippedAlreadyReady,
                    detail: "Component became ready before this operation ran".to_string(),
                },
                progress,
            )?;
            continue;
        }
        if requires_install(observation(&before, &operation.component_id)?.status).is_err() {
            record_result(
                &mut results,
                InstallOperationResult {
                    component_id: operation.component_id.clone(),
                    status: InstallResultStatus::Failed,
                    detail: "Component detection became unknown; no installer was launched".into(),
                },
                progress,
            )?;
            continue;
        }

        let output = match operation.strategy {
            InstallStrategy::WingetPackage => {
                let Some(winget) = winget.as_deref() else {
                    record_result(
                        &mut results,
                        InstallOperationResult {
                            component_id: operation.component_id.clone(),
                            status: InstallResultStatus::Failed,
                            detail:
                                "winget.exe is unavailable; no fallback shell command was executed"
                                    .to_string(),
                        },
                        progress,
                    )?;
                    continue;
                };
                identity::verify_winget(winget)?;
                if plan.executable_bindings.get("winget")
                    != Some(&crate::native_process::fingerprint_regular_file(winget)?)
                {
                    return Err(AppError::InvalidInput("WinGet changed after review; installation stopped without launching this step".into()));
                }
                if plan.executable_bindings.get("winget-source")
                    != Some(&source::observe(winget, Some(cancel))?)
                {
                    return Err(AppError::InvalidInput("WinGet repository changed after review; installation stopped before this step".into()));
                }
                if operation.component_id == COMPONENT_COPILOT_CLI && !powershell_ready() {
                    record_result(&mut results, InstallOperationResult { component_id: operation.component_id.clone(), status: InstallResultStatus::SkippedDependencyFailed, detail: "The reviewed CLI package requires PowerShell 7 or newer. Install this prerequisite manually and preview again; no automatic dependency command was executed".into() }, progress)?;
                    continue;
                }
                let arguments = catalog::arguments(&operation.component_id)?;
                runner.run(
                    winget,
                    &arguments
                        .iter()
                        .map(std::ffi::OsStr::new)
                        .collect::<Vec<_>>(),
                    CaptureMode::Standard,
                )
            }
            InstallStrategy::VsCodeExtension => {
                let Some(code) = vscode_install::find_executable() else {
                    record_result(&mut results, InstallOperationResult {
                        component_id: operation.component_id.clone(),
                        status: InstallResultStatus::SkippedDependencyFailed,
                        detail:
                            "VS Code is not available, so the Copilot extension was not installed"
                                .to_string(),
                    }, progress)?;
                    continue;
                };
                match VsCodeCli::resolve(&code) {
                    Ok(cli) => {
                        let args = cli.args(&[
                            std::ffi::OsStr::new("--install-extension"),
                            std::ffi::OsStr::new(vscode_install::INSTALL_EXTENSION_ID),
                        ]);
                        runner.run(
                            &cli.executable,
                            &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>(),
                            CaptureMode::VsCodeCli,
                        )
                    }
                    Err(error) => Err(error),
                }
            }
        };
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                record_result(&mut results, InstallOperationResult {
                    component_id: operation.component_id.clone(), status: match error {
                        AppError::Cancelled => InstallResultStatus::Cancelled,
                        AppError::TimedOut => InstallResultStatus::TimedOut,
                        _ => InstallResultStatus::Failed,
                    },
                    detail: "Installer stopped or failed within its safety limits. Completed effects are not uninstalled; rediscover before retrying".into(),
                }, progress)?;
                continue;
            }
        };

        let after = discover_windows_components();
        let verified =
            observation(&after, &operation.component_id)?.status == InstallComponentStatus::Ready;
        let process_ok = output.status.success();
        record_result(
            &mut results,
            InstallOperationResult {
                component_id: operation.component_id.clone(),
                status: match (process_ok, verified) {
                    (_, true) => InstallResultStatus::CompletedAndVerified,
                    (true, false) => InstallResultStatus::ProcessSucceededVerificationFailed,
                    (false, false) => InstallResultStatus::Failed,
                },
                detail: if verified {
                    "Installation completed and the component was re-detected".to_string()
                } else {
                    format!(
                        "Installer exited with code {:?}; component verification did not pass",
                        output.status.code()
                    )
                },
            },
            progress,
        )?;
    }

    Ok(InstallApplyResult {
        plan_id: plan.id.clone(),
        results,
        observations: discover_windows_components(),
    })
}

#[cfg(windows)]
fn record_result(
    results: &mut Vec<InstallOperationResult>,
    result: InstallOperationResult,
    progress: &mut dyn FnMut(&str, Option<InstallOperationResult>) -> AppResult<()>,
) -> AppResult<()> {
    progress(&result.component_id, Some(result.clone()))?;
    results.push(result);
    Ok(())
}

#[cfg(windows)]
fn powershell_ready() -> bool {
    let Some(path) = find_on_path("pwsh.exe") else {
        return false;
    };
    // No profile or commands are evaluated by this fixed version-only probe.
    crate::native_process::run_capture_bounded(
        &path,
        &[std::ffi::OsStr::new("--version")],
        std::time::Duration::from_secs(5),
        1024,
    )
    .is_ok_and(|output| {
        if !output.status.success() || output.stdout_truncated || output.stderr_truncated {
            return false;
        }
        output
            .stdout
            .trim()
            .strip_prefix("PowerShell ")
            .is_some_and(|v| {
                valid_version(v)
                    && v.split('.')
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .is_some_and(|v| v >= 7)
            })
    })
}

#[cfg(windows)]
fn find_on_path(name: &str) -> Option<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::executable(name);
    }

    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&path)
            .map(|root| root.join(name))
            .find(|candidate| candidate.is_file())
        {
            return Some(found);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let links = PathBuf::from(local)
            .join("Microsoft/WinGet/Links")
            .join(name);
        if links.is_file() {
            return Some(links);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_means_all_four_components() {
        let values = canonical_component_ids(&[]).expect("components");
        assert_eq!(values.len(), 4);
        assert!(values.contains(&COMPONENT_COPILOT_APP.to_string()));
        assert!(values.contains(&COMPONENT_COPILOT_CLI.to_string()));
        assert!(values.contains(&COMPONENT_VSCODE.to_string()));
        assert!(values.contains(&COMPONENT_VSCODE_COPILOT.to_string()));
    }

    #[test]
    fn failed_discovery_never_authorizes_installation() {
        for status in [
            InstallComponentStatus::Unknown,
            InstallComponentStatus::Unsupported,
        ] {
            assert!(requires_install(status).is_err());
        }
        assert!(!requires_install(InstallComponentStatus::Ready).unwrap());
        assert!(requires_install(InstallComponentStatus::Missing).unwrap());
        #[cfg(windows)]
        assert_eq!(
            extension_observation(Err(AppError::Config("probe failed".into()))).status,
            InstallComponentStatus::Unknown
        );
    }

    #[test]
    fn rejects_unknown_component_ids() {
        assert!(canonical_component_ids(&["evil-command".to_string()]).is_err());
    }

    #[test]
    fn operation_sources_are_backend_owned() {
        assert_eq!(
            operation_for(COMPONENT_COPILOT_CLI).expect("cli").source,
            "WinGet: GitHub.Copilot"
        );
        assert_eq!(
            operation_for(COMPONENT_COPILOT_APP).expect("app").source,
            "WinGet: GitHub.CopilotApp"
        );
        assert_eq!(
            operation_for(COMPONENT_VSCODE).expect("vscode").source,
            "WinGet: Microsoft.VisualStudioCode"
        );
    }

    #[test]
    fn install_plan_is_one_shot_without_live_machine_discovery() {
        let mut store = InstallPlanStore::default();
        let now = Utc::now();
        let plan = InstallPlan {
            id: "fixture".into(),
            requested_component_ids: vec![COMPONENT_COPILOT_CLI.into()],
            operations: Vec::new(),
            created_at: now,
            expires_at: now + Duration::minutes(15),
            executable_bindings: BTreeMap::new(),
        };
        store.plans.insert(
            plan.id.clone(),
            StoredInstallPlan {
                plan: plan.clone(),
                observation_fingerprints: BTreeMap::new(),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(900),
            },
        );
        assert!(store.take(&plan.id).is_ok());
        assert!(store.take(&plan.id).is_err());
    }

    #[test]
    fn version_output_is_bounded_typed_and_never_exposes_raw_failed_output() {
        assert_eq!(
            parse_cli_version("GitHub Copilot CLI 1.0.90\nCommit: fixture").unwrap(),
            "1.0.90"
        );
        assert_eq!(
            parse_cli_version("GitHub Copilot CLI version 1.0.90").unwrap(),
            "1.0.90"
        );
        for output in [
            "1.0.90",
            "private-fixture",
            "GitHub Copilot CLI invalid-private-fixture",
            "GitHub Copilot CLI 1.2.3.4",
        ] {
            let error = parse_cli_version(output).unwrap_err().to_string();
            assert!(!error.contains("private-fixture"));
        }
        assert!(!valid_version("1.0.90\n"));
    }
}
