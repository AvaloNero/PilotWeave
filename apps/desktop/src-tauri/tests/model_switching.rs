//! End-to-end model-switching coverage for the three Copilot surfaces:
//!
//! - VS Code Copilot consumes `chatLanguageModels.json` (verified in a
//!   temporary directory),
//! - Copilot CLI consumes the `COPILOT_PROVIDER_*` user environment
//!   (verified through the pure projection, never the real environment),
//! - the GitHub Copilot app stays behind its intentional read-only/manual
//!   boundary.
//!
//! These tests touch no real profile, registry, environment, or shell file.

use chrono::Utc;
use pilotweave_lib::adapters::{self, copilot_cli};
use pilotweave_lib::domain::{
    ApiProtocol, ClientKind, ClientStatus, ClientTarget, Connection, ModelCapabilities, ModelSpec,
    ProviderKind,
};
use pilotweave_lib::error::AppError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn model(model_id: &str, enabled: bool) -> ModelSpec {
    ModelSpec {
        id: format!("id-{model_id}"),
        model_id: model_id.to_string(),
        name: format!("Model {model_id}"),
        enabled,
        capabilities: ModelCapabilities::default(),
    }
}

fn connection(model_id: &str) -> Connection {
    Connection {
        id: "switch-test".to_string(),
        name: "Switch Test".to_string(),
        base_url: "https://example.invalid/v1".to_string(),
        provider_kind: ProviderKind::Openai,
        protocol: ApiProtocol::ChatCompletions,
        headers: BTreeMap::new(),
        models: vec![model(model_id, true)],
        secret_ref: "connection:switch-test".to_string(),
        has_secret: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn vscode_target(path: &Path) -> ClientTarget {
    ClientTarget {
        id: "vscode:stable:default".to_string(),
        kind: ClientKind::VsCodeCopilot,
        name: "Visual Studio Code · Default".to_string(),
        detail: "integration test target".to_string(),
        path: Some(path.to_string_lossy().to_string()),
        detected: true,
        supports_write: true,
        status: ClientStatus::Available,
        diagnostic: None,
    }
}

fn read_config(path: &Path) -> Value {
    let text = fs::read_to_string(path).expect("VS Code config");
    serde_json::from_str(&text).expect("config must stay valid JSON")
}

#[test]
fn switching_models_projects_every_supported_client() {
    let directory = tempfile::tempdir().expect("temp directory");
    let config = directory.path().join("chatLanguageModels.json");
    fs::write(
        &config,
        r#"[{"name":"Personal","vendor":"customendpoint","models":[{"id":"user/model"}]}]"#,
    )
    .expect("seed config");
    let target = vscode_target(&config);

    // Activate the connection with model-a on every surface.
    let mut connection = connection("upstream/model-a");
    adapters::apply_to_target(&connection, Some("pw-key"), &target).expect("apply model-a");
    let groups = read_config(&config);
    let rendered = serde_json::to_string(&groups).expect("serialize");
    assert!(rendered.contains("upstream/model-a"));
    assert!(rendered.contains("Personal"));

    let environment =
        copilot_cli::desired_environment(&connection, Some("pw-key")).expect("cli environment");
    assert_eq!(
        environment["COPILOT_MODEL"].as_deref(),
        Some("upstream/model-a")
    );

    // Switch the connection to model-b: every surface must follow.
    connection.models = vec![model("upstream/model-b", true)];

    adapters::apply_to_target(&connection, Some("pw-key"), &target).expect("apply model-b");
    let groups = read_config(&config);
    let rendered = serde_json::to_string(&groups).expect("serialize");
    assert!(rendered.contains("upstream/model-b"));
    assert!(!rendered.contains("upstream/model-a"));
    // The user's own group survives every switch.
    assert!(rendered.contains("Personal"));

    // The rollback backup preserved the pre-PilotWeave configuration.
    let backup =
        fs::read_to_string(config.with_extension("json.pilotweave.bak")).expect("rollback backup");
    assert!(backup.contains("Personal"));
    assert!(!backup.contains("upstream/model-b"));

    let environment =
        copilot_cli::desired_environment(&connection, Some("pw-key")).expect("cli environment");
    for name in [
        "COPILOT_MODEL",
        "COPILOT_PROVIDER_MODEL_ID",
        "COPILOT_PROVIDER_WIRE_MODEL",
    ] {
        assert_eq!(
            environment[name].as_deref(),
            Some("upstream/model-b"),
            "{name}"
        );
    }

    // The GitHub Copilot app refuses automated writes by design.
    let app_target = pilotweave_lib::adapters::github_app::discover_target();
    let error = adapters::apply_to_target(&connection, Some("pw-key"), &app_target)
        .expect_err("GitHub Copilot app must stay read-only");
    assert!(matches!(error, AppError::Unsupported(_)));
}

#[test]
fn a_plan_covers_every_client_kind_with_the_active_model() {
    let connection = connection("upstream/model-b");
    let plan = adapters::preview(
        &connection,
        &[
            "copilot-cli:user-environment".to_string(),
            "github-copilot-app:local".to_string(),
        ],
    )
    .expect("deployment plan");

    assert_eq!(plan.connection_id, "switch-test");
    assert_eq!(plan.operations.len(), 2);
    assert!(plan
        .operations
        .iter()
        .any(|operation| operation.target_kind == ClientKind::CopilotCli));
    assert!(plan
        .operations
        .iter()
        .any(|operation| operation.target_kind == ClientKind::GithubCopilotApp));
    for operation in &plan.operations {
        let rendered = format!("{} {}", operation.description, operation.changes.join(" "));
        assert!(
            rendered.contains("upstream/model-b"),
            "operation must name the active model: {rendered}"
        );
    }
}
