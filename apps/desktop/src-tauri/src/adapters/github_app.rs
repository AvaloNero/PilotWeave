use crate::domain::{ClientKind, ClientStatus, ClientTarget, Connection, DeploymentOperation};
use crate::error::{AppError, AppResult};
#[cfg(windows)]
use std::env;
use std::path::PathBuf;
use uuid::Uuid;

pub fn discover_target() -> ClientTarget {
    let path = installation_candidates()
        .into_iter()
        .find(|candidate| candidate.exists());
    let detected = path.is_some();
    ClientTarget {
        id: "github-copilot-app:local".to_string(),
        kind: ClientKind::GithubCopilotApp,
        name: "GitHub Copilot app".to_string(),
        detail: if detected {
            "Installation detected; provider management is manual in this MVP".to_string()
        } else {
            "No supported installation path was detected".to_string()
        },
        path: path.map(|path| path.to_string_lossy().to_string()),
        detected,
        supports_write: false,
        status: if detected {
            ClientStatus::ReadOnly
        } else {
            ClientStatus::NotInstalled
        },
        diagnostic: detected.then(|| {
            "PilotWeave will not write private app state or credential storage without a stable external interface"
                .to_string()
        }),
    }
}

pub fn preview(connection: &Connection, target: &ClientTarget) -> DeploymentOperation {
    DeploymentOperation {
        id: Uuid::new_v4().to_string(),
        target_id: target.id.clone(),
        target_kind: ClientKind::GithubCopilotApp,
        title: format!("Configure {} in GitHub Copilot app", connection.name),
        description: "Manual action required in the app's Model providers settings".to_string(),
        changes: vec![
            format!("Provider name: {}", connection.name),
            format!("Endpoint: {}", connection.base_url),
            format!(
                "Default model: {}",
                connection
                    .default_model()
                    .map(|model| model.model_id.as_str())
                    .unwrap_or("<none>")
            ),
            "Credential remains in the OS credential store until a supported deployment path exists"
                .to_string(),
        ],
        supported: false,
        requires_restart: false,
    }
}

pub fn apply(
    _connection: &Connection,
    _secret: Option<&str>,
    _target: &ClientTarget,
) -> AppResult<String> {
    Err(AppError::Unsupported(
        "GitHub Copilot app provider deployment is intentionally read-only in this MVP; use the app's Model providers settings"
            .to_string(),
    ))
}

fn installation_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(windows)]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            candidates.push(
                local
                    .join("Programs")
                    .join("GitHub Copilot")
                    .join("GitHub Copilot.exe"),
            );
            candidates.push(local.join("GitHubCopilot").join("GitHub Copilot.exe"));
        }
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(root) = env::var_os(variable) {
                candidates.push(
                    PathBuf::from(root)
                        .join("GitHub Copilot")
                        .join("GitHub Copilot.exe"),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/Applications/GitHub Copilot.app"));
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("Applications").join("GitHub Copilot.app"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/opt/GitHub Copilot/github-copilot"));
        candidates.push(PathBuf::from("/usr/bin/github-copilot"));
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".local").join("bin").join("github-copilot"));
            candidates.push(home.join("Applications").join("GitHub-Copilot.AppImage"));
        }
    }

    candidates
}
