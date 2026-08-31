pub mod copilot_cli;
pub mod github_app;
pub mod vscode;

use crate::domain::{ClientKind, ClientStatus, ClientTarget, Connection, DeploymentPlan};
use crate::error::{AppError, AppResult};
use chrono::Utc;
use std::collections::HashSet;
use uuid::Uuid;

pub fn discover_all() -> Vec<ClientTarget> {
    let mut targets = Vec::new();
    match vscode::discover_targets() {
        Ok(mut discovered) => targets.append(&mut discovered),
        Err(error) => targets.push(ClientTarget {
            id: "vscode:error".to_string(),
            kind: ClientKind::VsCodeCopilot,
            name: "VS Code Copilot".to_string(),
            detail: "Profile discovery failed".to_string(),
            path: None,
            detected: false,
            supports_write: false,
            status: ClientStatus::Error,
            diagnostic: Some(error.to_string()),
        }),
    }
    targets.push(copilot_cli::discover_target());
    targets.push(github_app::discover_target());
    targets.sort_by(|left, right| {
        left.kind
            .as_str()
            .cmp(right.kind.as_str())
            .then_with(|| left.name.cmp(&right.name))
    });
    targets
}

pub fn preview(
    connection: &Connection,
    requested_target_ids: &[String],
) -> AppResult<DeploymentPlan> {
    if requested_target_ids.is_empty() {
        return Err(AppError::InvalidInput(
            "Select at least one deployment target".to_string(),
        ));
    }
    let available = discover_all();
    let mut seen = HashSet::new();
    let mut operations = Vec::new();
    let mut canonical_ids = Vec::new();

    for target_id in requested_target_ids {
        if !seen.insert(target_id.clone()) {
            continue;
        }
        let target = available
            .iter()
            .find(|target| target.id == *target_id)
            .ok_or_else(|| AppError::InvalidInput(format!("Unknown client target: {target_id}")))?;
        let operation = match target.kind {
            ClientKind::VsCodeCopilot => vscode::preview(connection, target),
            ClientKind::CopilotCli => copilot_cli::preview(connection, target),
            ClientKind::GithubCopilotApp => github_app::preview(connection, target),
        };
        canonical_ids.push(target.id.clone());
        operations.push(operation);
    }

    Ok(DeploymentPlan {
        id: Uuid::new_v4().to_string(),
        connection_id: connection.id.clone(),
        connection_name: connection.name.clone(),
        target_ids: canonical_ids,
        operations,
        created_at: Utc::now(),
    })
}

pub fn apply_to_target(
    connection: &Connection,
    secret: Option<&str>,
    target: &ClientTarget,
) -> AppResult<String> {
    if !target.detected {
        return Err(AppError::Unsupported(format!(
            "{} is not installed or could not be detected",
            target.name
        )));
    }
    match target.kind {
        ClientKind::VsCodeCopilot => vscode::apply(connection, secret, target),
        ClientKind::CopilotCli => copilot_cli::apply(connection, secret, target),
        ClientKind::GithubCopilotApp => github_app::apply(connection, secret, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ApiProtocol, ModelCapabilities, ModelSpec, ProviderKind};
    use std::collections::BTreeMap;

    fn connection() -> Connection {
        Connection {
            id: "one".to_string(),
            name: "One".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: ProviderKind::Openai,
            protocol: ApiProtocol::ChatCompletions,
            headers: BTreeMap::new(),
            models: vec![ModelSpec {
                id: "model".to_string(),
                model_id: "gpt-example".to_string(),
                name: "GPT Example".to_string(),
                enabled: true,
                capabilities: ModelCapabilities::default(),
            }],
            secret_ref: "connection:one".to_string(),
            has_secret: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn discover_all_always_includes_cli_and_app_targets() {
        let targets = discover_all();
        assert!(targets
            .iter()
            .any(|target| target.id == "copilot-cli:user-environment"));
        assert!(targets
            .iter()
            .any(|target| target.id == "github-copilot-app:local"));
    }

    #[test]
    fn preview_requires_at_least_one_target() {
        let error = preview(&connection(), &[]).expect_err("empty selection must fail");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn preview_rejects_unknown_target_ids() {
        let error = preview(&connection(), &["no-such-target".to_string()])
            .expect_err("unknown target must fail");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn preview_deduplicates_repeated_targets() {
        let plan = preview(
            &connection(),
            &[
                "github-copilot-app:local".to_string(),
                "github-copilot-app:local".to_string(),
            ],
        )
        .expect("plan");
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.target_ids, vec!["github-copilot-app:local"]);
    }

    #[test]
    fn preview_describes_the_default_model_for_cli_and_app() {
        let plan = preview(
            &connection(),
            &[
                "copilot-cli:user-environment".to_string(),
                "github-copilot-app:local".to_string(),
            ],
        )
        .expect("plan");
        assert_eq!(plan.operations.len(), 2);
        for operation in &plan.operations {
            let rendered = format!("{} {}", operation.description, operation.changes.join(" "));
            assert!(
                rendered.contains("gpt-example"),
                "operation should name the active model: {rendered}"
            );
        }
    }

    #[test]
    fn apply_to_target_rejects_undetected_targets() {
        let target = ClientTarget {
            id: "copilot-cli:user-environment".to_string(),
            kind: ClientKind::CopilotCli,
            name: "GitHub Copilot CLI".to_string(),
            detail: "not detected".to_string(),
            path: None,
            detected: false,
            supports_write: false,
            status: ClientStatus::NotInstalled,
            diagnostic: None,
        };
        let error = apply_to_target(&connection(), Some("secret"), &target)
            .expect_err("undetected target must fail");
        assert!(matches!(error, AppError::Unsupported(_)));
    }
}
