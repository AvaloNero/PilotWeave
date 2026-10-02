//! Reviewed WinGet manifest schema branch (2026-10-02), Windows x64 only.
//! No frontend strings participate in package selection or command construction.
use super::*;

pub(super) struct Package {
    pub id: &'static str,
    pub version: &'static str,
    pub dependencies: &'static [&'static str],
}

pub(super) fn package(component: &str) -> AppResult<Package> {
    match component {
        COMPONENT_VSCODE => Ok(Package { id: "Microsoft.VisualStudioCode", version: "1.140.0", dependencies: &[] }),
        COMPONENT_COPILOT_CLI => Ok(Package { id: "GitHub.Copilot", version: "v1.0.90", dependencies: &["PowerShell 7 or newer must already be installed; automatic dependency installation is disabled"] }),
        COMPONENT_COPILOT_APP => Ok(Package { id: "GitHub.CopilotApp", version: "1.1.25", dependencies: &[] }),
        _ => Err(AppError::InvalidInput("Component has no reviewed WinGet package".into())),
    }
}

#[cfg(any(windows, test))]
pub(super) fn arguments(component: &str) -> AppResult<Vec<&'static str>> {
    let package = package(component)?;
    let mut args = vec![
        "install",
        "--id",
        package.id,
        "--exact",
        "--version",
        package.version,
        "--source",
        "winget",
        "--architecture",
        "x64",
        "--skip-dependencies",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--disable-interactivity",
    ];
    // The portable CLI manifest has no Scope selector. Desktop packages do.
    if component != COMPONENT_COPILOT_CLI {
        args.extend(["--scope", "user"]);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviewed_versions_architecture_and_dependencies_are_not_remote_arguments() {
        for id in [
            COMPONENT_VSCODE,
            COMPONENT_COPILOT_CLI,
            COMPONENT_COPILOT_APP,
        ] {
            let args = arguments(id).unwrap();
            assert!(args
                .windows(2)
                .any(|a| a == ["--version", package(id).unwrap().version]));
            assert!(args.windows(2).any(|a| a == ["--architecture", "x64"]));
            assert!(args.contains(&"--skip-dependencies"));
            assert!(!args.contains(&"--ignore-security-hash"));
        }
        assert!(!package(COMPONENT_COPILOT_CLI)
            .unwrap()
            .dependencies
            .is_empty());
        assert!(arguments("arbitrary;command").is_err());
    }
}
