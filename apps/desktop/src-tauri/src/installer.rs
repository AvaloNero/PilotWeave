use crate::error::{AppError, AppResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Output};
use uuid::Uuid;

const INSTALL_PLAN_TTL_SECONDS: i64 = 15 * 60;
#[cfg(windows)]
const COPILOT_EXTENSION_ID: &str = "GitHub.copilot";

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub id: String,
    pub requested_component_ids: Vec<String>,
    pub operations: Vec<InstallOperation>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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
}

#[derive(Default)]
pub struct InstallPlanStore {
    plans: HashMap<String, StoredInstallPlan>,
}

impl InstallPlanStore {
    pub fn preview(&mut self, requested_component_ids: Vec<String>) -> AppResult<InstallPlan> {
        self.purge_expired();
        let observations = discover_components();
        let requested = canonical_component_ids(&requested_component_ids)?;
        let mut operations = Vec::new();
        for component_id in &requested {
            let observation = observation(&observations, component_id)?;
            if observation.status == InstallComponentStatus::Ready {
                continue;
            }
            operations.push(operation_for(component_id)?);
        }
        operations.sort_by_key(|operation| operation_rank(&operation.component_id));
        let created_at = Utc::now();
        let plan = InstallPlan {
            id: Uuid::new_v4().to_string(),
            requested_component_ids: requested.clone(),
            operations,
            created_at,
            expires_at: created_at + Duration::seconds(INSTALL_PLAN_TTL_SECONDS),
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
            },
        );
        Ok(plan)
    }

    pub fn consume(&mut self, plan_id: &str) -> AppResult<InstallPlan> {
        self.purge_expired();
        let stored = self.plans.remove(plan_id).ok_or_else(|| {
            AppError::InvalidInput(
                "Install plan is missing, expired, or was already consumed; preview again"
                    .to_string(),
            )
        })?;
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
        Ok(stored.plan)
    }

    fn purge_expired(&mut self) {
        let now = Utc::now();
        self.plans.retain(|_, stored| stored.plan.expires_at >= now);
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

pub fn apply_plan(store: &mut InstallPlanStore, plan_id: &str) -> AppResult<InstallApplyResult> {
    let plan = store.consume(plan_id)?;
    #[cfg(windows)]
    {
        let mut runner = NativeRunner;
        apply_windows_plan(&plan, &mut runner)
    }
    #[cfg(not(windows))]
    {
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
    let code = find_code_executable();
    let copilot = find_on_path("copilot.exe").or_else(|| find_on_path("copilot.cmd"));
    let app = github_copilot_app_path();
    let extension_ready = code.as_deref().map(extension_installed).unwrap_or(false);

    vec![
        observation_from_path(COMPONENT_VSCODE, "Visual Studio Code", code),
        InstallComponentObservation {
            id: COMPONENT_VSCODE_COPILOT.to_string(),
            name: "GitHub Copilot extension".to_string(),
            status: if extension_ready {
                InstallComponentStatus::Ready
            } else {
                InstallComponentStatus::Missing
            },
            detail: if extension_ready {
                format!("VS Code extension {COPILOT_EXTENSION_ID} is installed")
            } else {
                format!("VS Code extension {COPILOT_EXTENSION_ID} is not installed")
            },
            version: None,
        },
        observation_from_path(COMPONENT_COPILOT_CLI, "GitHub Copilot CLI", copilot),
        observation_from_path(COMPONENT_COPILOT_APP, "GitHub Copilot app", app),
    ]
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
            "Visual Studio Marketplace: GitHub.copilot",
            false,
            "Use the verified VS Code executable to install the exact GitHub.copilot extension",
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
trait ProcessRunner {
    fn run(&mut self, executable: &Path, args: &[&str]) -> std::io::Result<Output>;
}

#[cfg(windows)]
struct NativeRunner;

#[cfg(windows)]
impl ProcessRunner for NativeRunner {
    fn run(&mut self, executable: &Path, args: &[&str]) -> std::io::Result<Output> {
        Command::new(executable).args(args).output()
    }
}

#[cfg(windows)]
fn apply_windows_plan(
    plan: &InstallPlan,
    runner: &mut dyn ProcessRunner,
) -> AppResult<InstallApplyResult> {
    let mut results = Vec::new();
    let winget = find_on_path("winget.exe");

    for operation in &plan.operations {
        let before = discover_windows_components();
        if observation(&before, &operation.component_id)?.status == InstallComponentStatus::Ready {
            results.push(InstallOperationResult {
                component_id: operation.component_id.clone(),
                status: InstallResultStatus::SkippedAlreadyReady,
                detail: "Component became ready before this operation ran".to_string(),
            });
            continue;
        }

        let output = match operation.strategy {
            InstallStrategy::WingetPackage => {
                let Some(winget) = winget.as_deref() else {
                    results.push(InstallOperationResult {
                        component_id: operation.component_id.clone(),
                        status: InstallResultStatus::Failed,
                        detail: "winget.exe is unavailable; no fallback shell command was executed"
                            .to_string(),
                    });
                    continue;
                };
                let package_id = match operation.component_id.as_str() {
                    COMPONENT_VSCODE => "Microsoft.VisualStudioCode",
                    COMPONENT_COPILOT_CLI => "GitHub.Copilot",
                    COMPONENT_COPILOT_APP => "GitHub.CopilotApp",
                    _ => {
                        return Err(AppError::Config(
                            "Unexpected component for WinGet strategy".into(),
                        ))
                    }
                };
                runner.run(
                    winget,
                    &[
                        "install",
                        "--id",
                        package_id,
                        "--exact",
                        "--source",
                        "winget",
                        "--accept-package-agreements",
                        "--accept-source-agreements",
                        "--disable-interactivity",
                    ],
                )
            }
            InstallStrategy::VsCodeExtension => {
                let Some(code) = find_code_executable() else {
                    results.push(InstallOperationResult {
                        component_id: operation.component_id.clone(),
                        status: InstallResultStatus::SkippedDependencyFailed,
                        detail:
                            "VS Code is not available, so the Copilot extension was not installed"
                                .to_string(),
                    });
                    continue;
                };
                runner.run(&code, &["--install-extension", COPILOT_EXTENSION_ID])
            }
        }
        .map_err(|error| AppError::Config(format!("Failed to launch installer: {error}")))?;

        let after = discover_windows_components();
        let verified =
            observation(&after, &operation.component_id)?.status == InstallComponentStatus::Ready;
        let process_ok = output.status.success();
        results.push(InstallOperationResult {
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
        });
    }

    Ok(InstallApplyResult {
        plan_id: plan.id.clone(),
        results,
        observations: discover_windows_components(),
    })
}

#[cfg(windows)]
fn extension_installed(code: &Path) -> bool {
    Command::new(code)
        .arg("--list-extensions")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case(COPILOT_EXTENSION_ID))
        })
        .unwrap_or(false)
}

#[cfg(windows)]
fn find_code_executable() -> Option<PathBuf> {
    find_on_path("code.exe")
        .or_else(|| find_on_path("code.cmd"))
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .and_then(|root| {
                    [
                        root.join("Programs/Microsoft VS Code/bin/code.cmd"),
                        root.join("Programs/Microsoft VS Code/Code.exe"),
                    ]
                    .into_iter()
                    .find(|path| path.is_file())
                })
        })
}

#[cfg(windows)]
fn github_copilot_app_path() -> Option<PathBuf> {
    let mut values = Vec::new();
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        values.push(PathBuf::from(root).join("Programs/GitHub Copilot/GitHub Copilot.exe"));
    }
    if let Some(root) = std::env::var_os("PROGRAMFILES") {
        values.push(PathBuf::from(root).join("GitHub Copilot/GitHub Copilot.exe"));
    }
    values.into_iter().find(|path| path.is_file())
}

#[cfg(windows)]
fn find_on_path(name: &str) -> Option<PathBuf> {
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
    fn install_plan_is_one_shot() {
        let mut store = InstallPlanStore::default();
        let plan = store
            .preview(vec![COMPONENT_COPILOT_CLI.to_string()])
            .expect("preview");
        let _ = store.consume(&plan.id).expect("consume");
        assert!(store.consume(&plan.id).is_err());
    }
}
