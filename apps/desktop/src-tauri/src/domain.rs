use crate::error::{AppError, AppResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use url::Url;
use uuid::Uuid;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApiProtocol {
    #[default]
    ChatCompletions,
    Responses,
    Messages,
}

impl ApiProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat-completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    Openai,
    Azure,
    Anthropic,
    Local,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calling: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    pub id: String,
    pub model_id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}

fn default_true() -> bool {
    true
}

impl ModelSpec {
    pub fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        if self.id.is_empty() {
            self.id = Uuid::new_v4().to_string();
        }
        self.model_id = self.model_id.trim().to_string();
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = self.model_id.clone();
        }
        self.capabilities.context_window =
            self.capabilities.context_window.filter(|value| *value > 0);
        self.capabilities.max_output_tokens = self
            .capabilities
            .max_output_tokens
            .filter(|value| *value > 0);
    }

    pub fn validate(&self) -> AppResult<()> {
        if self.model_id.is_empty() {
            return Err(AppError::InvalidInput(
                "Every model requires an upstream model id".to_string(),
            ));
        }
        if self.name.is_empty() {
            return Err(AppError::InvalidInput(
                "Every model requires a display name".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub provider_kind: ProviderKind,
    #[serde(default)]
    pub protocol: ApiProtocol,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ModelSpec>,
    pub secret_ref: String,
    #[serde(default)]
    pub has_secret: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Connection {
    pub fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        if self.id.is_empty() {
            self.id = Uuid::new_v4().to_string();
        }
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.secret_ref = self.secret_ref.trim().to_string();
        if self.secret_ref.is_empty() {
            self.secret_ref = format!("connection:{}", self.id);
        }
        self.headers = std::mem::take(&mut self.headers)
            .into_iter()
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .filter(|(name, _)| !name.is_empty())
            .collect();
        for model in &mut self.models {
            model.normalize();
        }
    }

    pub fn validate(&self) -> AppResult<()> {
        if self.name.is_empty() {
            return Err(AppError::InvalidInput(
                "Connection name is required".to_string(),
            ));
        }

        let url = Url::parse(&self.base_url)
            .map_err(|error| AppError::InvalidInput(format!("Invalid endpoint URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(AppError::InvalidInput(
                "Endpoint must be an absolute HTTP(S) URL".to_string(),
            ));
        }

        if self.models.is_empty() {
            return Err(AppError::InvalidInput(
                "A connection must define at least one model".to_string(),
            ));
        }

        let mut ids = HashSet::new();
        let mut model_ids = HashSet::new();
        for model in &self.models {
            model.validate()?;
            if !ids.insert(model.id.to_ascii_lowercase()) {
                return Err(AppError::InvalidInput(format!(
                    "Duplicate internal model id: {}",
                    model.id
                )));
            }
            if !model_ids.insert(model.model_id.to_ascii_lowercase()) {
                return Err(AppError::InvalidInput(format!(
                    "Duplicate upstream model id: {}",
                    model.model_id
                )));
            }
        }

        for (name, value) in &self.headers {
            if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err(AppError::InvalidInput(
                    "Header names and values must not contain newlines".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn enabled_models(&self) -> impl Iterator<Item = &ModelSpec> {
        self.models.iter().filter(|model| model.enabled)
    }

    pub fn default_model(&self) -> Option<&ModelSpec> {
        self.enabled_models().next()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub provider_kind: ProviderKind,
    #[serde(default)]
    pub protocol: ApiProtocol,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ModelSpec>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_secret: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ClientKind {
    VsCodeCopilot,
    CopilotCli,
    GithubCopilotApp,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VsCodeCopilot => "vscode-copilot",
            Self::CopilotCli => "copilot-cli",
            Self::GithubCopilotApp => "github-copilot-app",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ClientStatus {
    Available,
    NotInstalled,
    ReadOnly,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientTarget {
    pub id: String,
    pub kind: ClientKind,
    pub name: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub detected: bool,
    pub supports_write: bool,
    pub status: ClientStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentOperation {
    pub id: String,
    pub target_id: String,
    pub target_kind: ClientKind,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub changes: Vec<String>,
    pub supported: bool,
    pub requires_restart: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlan {
    pub id: String,
    pub connection_id: String,
    pub connection_name: String,
    pub target_ids: Vec<String>,
    pub operations: Vec<DeploymentOperation>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentStatus {
    Applied,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentRecord {
    pub id: String,
    pub plan_id: String,
    pub connection_id: String,
    pub target_id: String,
    pub target_kind: ClientKind,
    pub status: DeploymentStatus,
    pub detail: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub plan_id: String,
    pub records: Vec<DeploymentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub version: u32,
    pub state_path: String,
    pub connections: Vec<Connection>,
    pub clients: Vec<ClientTarget>,
    pub deployments: Vec<DeploymentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistentState {
    #[serde(default = "default_state_version")]
    pub version: u32,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub deployments: Vec<DeploymentRecord>,
}

fn default_state_version() -> u32 {
    STATE_VERSION
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            connections: Vec::new(),
            deployments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, model_id: &str, enabled: bool) -> ModelSpec {
        ModelSpec {
            id: id.to_string(),
            model_id: model_id.to_string(),
            name: model_id.to_string(),
            enabled,
            capabilities: ModelCapabilities::default(),
        }
    }

    fn connection_with(models: Vec<ModelSpec>) -> Connection {
        Connection {
            id: "one".to_string(),
            name: "One".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: ProviderKind::Openai,
            protocol: ApiProtocol::ChatCompletions,
            headers: BTreeMap::new(),
            models,
            secret_ref: String::new(),
            has_secret: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn the_first_enabled_model_is_the_default() {
        let connection = connection_with(vec![
            model("a", "upstream/disabled", false),
            model("b", "upstream/active", true),
            model("c", "upstream/also-active", true),
        ]);
        assert_eq!(
            connection
                .default_model()
                .map(|model| model.model_id.as_str()),
            Some("upstream/active")
        );
        assert_eq!(connection.enabled_models().count(), 2);
    }

    #[test]
    fn disabling_every_model_leaves_no_default() {
        let connection = connection_with(vec![model("a", "upstream/model", false)]);
        assert!(connection.default_model().is_none());
        assert_eq!(connection.enabled_models().count(), 0);
    }

    #[test]
    fn validate_rejects_duplicate_upstream_model_ids_case_insensitively() {
        let connection = connection_with(vec![
            model("a", "Vendor/Model", true),
            model("b", "vendor/model", true),
        ]);
        let error = connection.validate().expect_err("duplicates must fail");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    fn validate_rejects_non_http_endpoints() {
        let mut connection = connection_with(vec![model("a", "upstream/model", true)]);
        connection.base_url = "file:///etc/passwd".to_string();
        assert!(connection.validate().is_err());
        connection.base_url = "not a url".to_string();
        assert!(connection.validate().is_err());
    }

    #[test]
    fn validate_requires_at_least_one_model() {
        let connection = connection_with(Vec::new());
        assert!(connection.validate().is_err());
    }

    #[test]
    fn normalize_trims_urls_and_defaults_the_secret_reference() {
        let mut connection = connection_with(vec![model("a", "upstream/model", true)]);
        connection.base_url = "  https://example.invalid/v1/  ".to_string();
        connection.normalize();
        assert_eq!(connection.base_url, "https://example.invalid/v1");
        assert_eq!(connection.secret_ref, "connection:one");
    }

    #[test]
    fn normalize_drops_zeroed_capability_limits() {
        let mut spec = model("a", "upstream/model", true);
        spec.capabilities.context_window = Some(0);
        spec.capabilities.max_output_tokens = Some(0);
        spec.normalize();
        assert_eq!(spec.capabilities.context_window, None);
        assert_eq!(spec.capabilities.max_output_tokens, None);
    }
}
