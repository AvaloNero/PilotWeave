use crate::domain::{Connection, DeploymentRecord, PersistentState};
use crate::error::{AppError, AppResult};
use std::collections::HashSet;
use url::{Host, Url};

pub const MAX_CONNECTIONS: usize = 128;
const MAX_MODELS_PER_CONNECTION: usize = 128;
const MAX_HEADERS_PER_CONNECTION: usize = 32;
const MAX_CONNECTION_NAME_BYTES: usize = 160;
const MAX_CONNECTION_ID_BYTES: usize = 160;
const MAX_SECRET_REF_BYTES: usize = 256;
const MAX_ENDPOINT_BYTES: usize = 2_048;
const MAX_MODEL_ID_BYTES: usize = 320;
const MAX_MODEL_NAME_BYTES: usize = 240;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_HEADER_VALUE_BYTES: usize = 8 * 1_024;
const MAX_API_KEY_BYTES: usize = 64 * 1_024;
const MAX_CONNECTION_JSON_BYTES: usize = 1_024 * 1_024;
const MAX_TOKEN_LIMIT: u64 = 32_000_000;
const MAX_DEPLOYMENT_RECORDS: usize = 200;
const MAX_RECORD_FIELD_BYTES: usize = 512;
const MAX_RECORD_DETAIL_BYTES: usize = 16 * 1_024;

const AUTHENTICATION_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "api-key",
    "x-api-key",
    "anthropic-api-key",
    "openai-api-key",
    "azure-openai-api-key",
];

pub fn validate_connection(connection: &Connection) -> AppResult<()> {
    connection.validate()?;

    validate_required_text("Connection id", &connection.id, MAX_CONNECTION_ID_BYTES)?;
    validate_required_text(
        "Connection name",
        &connection.name,
        MAX_CONNECTION_NAME_BYTES,
    )?;
    validate_required_text(
        "Credential reference",
        &connection.secret_ref,
        MAX_SECRET_REF_BYTES,
    )?;
    validate_endpoint(&connection.base_url)?;

    if connection.models.len() > MAX_MODELS_PER_CONNECTION {
        return Err(AppError::InvalidInput(format!(
            "A connection may define at most {MAX_MODELS_PER_CONNECTION} models"
        )));
    }
    if !connection.models.iter().any(|model| model.enabled) {
        return Err(AppError::InvalidInput(
            "A connection requires at least one enabled model".to_string(),
        ));
    }

    for model in &connection.models {
        validate_required_text("Internal model id", &model.id, MAX_MODEL_ID_BYTES)?;
        validate_required_text("Upstream model id", &model.model_id, MAX_MODEL_ID_BYTES)?;
        validate_required_text("Model display name", &model.name, MAX_MODEL_NAME_BYTES)?;
        for (name, value) in [
            ("Model context window", model.capabilities.context_window),
            (
                "Model maximum output tokens",
                model.capabilities.max_output_tokens,
            ),
        ] {
            if let Some(value) = value {
                if value == 0 || value > MAX_TOKEN_LIMIT {
                    return Err(AppError::InvalidInput(format!(
                        "{name} must be between 1 and {MAX_TOKEN_LIMIT}"
                    )));
                }
            }
        }
    }

    if connection.headers.len() > MAX_HEADERS_PER_CONNECTION {
        return Err(AppError::InvalidInput(format!(
            "A connection may define at most {MAX_HEADERS_PER_CONNECTION} headers"
        )));
    }
    let mut header_names = HashSet::new();
    for (name, value) in &connection.headers {
        validate_header_name(name)?;
        validate_bounded_text("Header value", value, MAX_HEADER_VALUE_BYTES)?;
        if value.contains(['\r', '\n', '\0']) {
            return Err(AppError::InvalidInput(
                "Header values must not contain control newlines or NUL".to_string(),
            ));
        }
        let canonical_name = name.to_ascii_lowercase();
        if !header_names.insert(canonical_name.clone()) {
            return Err(AppError::InvalidInput(format!(
                "Duplicate header name ignoring case: {name}"
            )));
        }
        if AUTHENTICATION_HEADERS.contains(&canonical_name.as_str()) && !value.contains("${apiKey}")
        {
            return Err(AppError::InvalidInput(format!(
                "Authentication header {name} must use the ${{apiKey}} placeholder instead of a persisted literal credential"
            )));
        }
    }

    let serialized = serde_json::to_vec(connection).map_err(|error| {
        AppError::Config(format!("Failed to size the connection record: {error}"))
    })?;
    if serialized.len() > MAX_CONNECTION_JSON_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Connection configuration exceeds {} KiB",
            MAX_CONNECTION_JSON_BYTES / 1_024
        )));
    }
    Ok(())
}

pub fn validate_api_key(value: Option<&str>) -> AppResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > MAX_API_KEY_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Credential exceeds {} KiB",
            MAX_API_KEY_BYTES / 1_024
        )));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(AppError::InvalidInput(
            "Credentials must not contain newlines or NUL".to_string(),
        ));
    }
    Ok(())
}

/// Persisted identity fields are checked before normalization so malformed
/// state cannot silently manufacture replacement identifiers during startup.
pub fn validate_persisted_identities(state: &PersistentState) -> AppResult<()> {
    if state.connections.len() > MAX_CONNECTIONS {
        return Err(AppError::InvalidInput(format!(
            "State contains more than {MAX_CONNECTIONS} connections"
        )));
    }
    if state.deployments.len() > MAX_DEPLOYMENT_RECORDS {
        return Err(AppError::InvalidInput(format!(
            "State contains more than {MAX_DEPLOYMENT_RECORDS} deployment records"
        )));
    }
    for connection in &state.connections {
        validate_required_text(
            "Persisted connection id",
            &connection.id,
            MAX_CONNECTION_ID_BYTES,
        )?;
        validate_required_text(
            "Persisted credential reference",
            &connection.secret_ref,
            MAX_SECRET_REF_BYTES,
        )?;
        for model in &connection.models {
            validate_required_text("Persisted internal model id", &model.id, MAX_MODEL_ID_BYTES)?;
        }
    }
    Ok(())
}

pub fn validate_persistent_state(state: &PersistentState) -> AppResult<()> {
    if state.connections.len() > MAX_CONNECTIONS {
        return Err(AppError::InvalidInput(format!(
            "State contains more than {MAX_CONNECTIONS} connections"
        )));
    }
    if state.deployments.len() > MAX_DEPLOYMENT_RECORDS {
        return Err(AppError::InvalidInput(format!(
            "State contains more than {MAX_DEPLOYMENT_RECORDS} deployment records"
        )));
    }

    let mut connection_ids = HashSet::new();
    let mut secret_refs = HashSet::new();
    for connection in &state.connections {
        validate_connection(connection)?;
        let normalized_id = connection.id.to_ascii_lowercase();
        if !connection_ids.insert(normalized_id) {
            return Err(AppError::InvalidInput(format!(
                "Duplicate persisted connection id: {}",
                connection.id
            )));
        }
        let normalized_secret_ref = connection.secret_ref.to_ascii_lowercase();
        if !secret_refs.insert(normalized_secret_ref) {
            return Err(AppError::InvalidInput(format!(
                "Duplicate persisted credential reference: {}",
                connection.secret_ref
            )));
        }
    }

    let mut record_ids = HashSet::new();
    for record in &state.deployments {
        validate_deployment_record(record)?;
        if !record_ids.insert(record.id.to_ascii_lowercase()) {
            return Err(AppError::InvalidInput(format!(
                "Duplicate deployment record id: {}",
                record.id
            )));
        }
        if !connection_ids.contains(&record.connection_id.to_ascii_lowercase()) {
            return Err(AppError::InvalidInput(format!(
                "Deployment record {} references an unknown connection",
                record.id
            )));
        }
    }
    Ok(())
}

fn validate_deployment_record(record: &DeploymentRecord) -> AppResult<()> {
    validate_required_text("Deployment record id", &record.id, MAX_RECORD_FIELD_BYTES)?;
    validate_required_text(
        "Deployment plan id",
        &record.plan_id,
        MAX_RECORD_FIELD_BYTES,
    )?;
    validate_required_text(
        "Deployment connection id",
        &record.connection_id,
        MAX_RECORD_FIELD_BYTES,
    )?;
    validate_required_text(
        "Deployment target id",
        &record.target_id,
        MAX_RECORD_FIELD_BYTES,
    )?;
    validate_bounded_text("Deployment detail", &record.detail, MAX_RECORD_DETAIL_BYTES)?;
    Ok(())
}

fn validate_endpoint(value: &str) -> AppResult<()> {
    validate_required_text("Endpoint URL", value, MAX_ENDPOINT_BYTES)?;
    let url = Url::parse(value)
        .map_err(|error| AppError::InvalidInput(format!("Invalid endpoint URL: {error}")))?;
    if url.host().is_none() {
        return Err(AppError::InvalidInput(
            "Endpoint URL must include a host".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::InvalidInput(
            "Endpoint URL must not embed a username or password".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(AppError::InvalidInput(
            "Endpoint URL must not contain a fragment".to_string(),
        ));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&url) => Ok(()),
        "http" => Err(AppError::InvalidInput(
            "Plain HTTP endpoints are allowed only for loopback hosts".to_string(),
        )),
        _ => Err(AppError::InvalidInput(
            "Endpoint must use HTTPS, or HTTP on a loopback host".to_string(),
        )),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost")
        }
        None => false,
    }
}

fn validate_header_name(name: &str) -> AppResult<()> {
    validate_required_text("Header name", name, MAX_HEADER_NAME_BYTES)?;
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    {
        return Err(AppError::InvalidInput(format!(
            "Invalid HTTP header name: {name}"
        )));
    }
    Ok(())
}

fn validate_required_text(label: &str, value: &str, max_bytes: usize) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::InvalidInput(format!("{label} is required")));
    }
    validate_bounded_text(label, value, max_bytes)
}

fn validate_bounded_text(label: &str, value: &str, max_bytes: usize) -> AppResult<()> {
    if value.len() > max_bytes {
        return Err(AppError::InvalidInput(format!(
            "{label} exceeds the {max_bytes}-byte limit"
        )));
    }
    if value.contains('\0')
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\t' | '\r' | '\n'))
    {
        return Err(AppError::InvalidInput(format!(
            "{label} contains unsupported control characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ApiProtocol, ClientKind, DeploymentStatus, ModelCapabilities, ModelSpec, ProviderKind,
        STATE_VERSION,
    };
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn connection(endpoint: &str) -> Connection {
        Connection {
            id: "connection-one".to_string(),
            name: "One".to_string(),
            base_url: endpoint.to_string(),
            provider_kind: ProviderKind::Openai,
            protocol: ApiProtocol::ChatCompletions,
            headers: BTreeMap::new(),
            models: vec![ModelSpec {
                id: "model-one".to_string(),
                model_id: "vendor/model".to_string(),
                name: "Model".to_string(),
                enabled: true,
                capabilities: ModelCapabilities::default(),
            }],
            secret_ref: "connection:connection-one".to_string(),
            has_secret: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn rejects_remote_plain_http_but_allows_loopback() {
        assert!(validate_connection(&connection("http://example.com/v1")).is_err());
        assert!(validate_connection(&connection("http://localhost:11434/v1")).is_ok());
        assert!(validate_connection(&connection("http://127.0.0.1:11434/v1")).is_ok());
        assert!(validate_connection(&connection("http://[::1]:11434/v1")).is_ok());
    }

    #[test]
    fn rejects_url_credentials_and_fragments() {
        assert!(validate_connection(&connection("https://user:pass@example.com/v1")).is_err());
        assert!(validate_connection(&connection("https://example.com/v1#secret")).is_err());
    }

    #[test]
    fn authentication_headers_require_the_secret_placeholder() {
        let mut value = connection("https://example.com/v1");
        value
            .headers
            .insert("Authorization".to_string(), "Bearer literal".to_string());
        assert!(validate_connection(&value).is_err());
        value
            .headers
            .insert("Authorization".to_string(), "Bearer ${apiKey}".to_string());
        assert!(validate_connection(&value).is_ok());
    }

    #[test]
    fn header_names_are_tokens_and_unique_ignoring_case() {
        let mut value = connection("https://example.com/v1");
        value
            .headers
            .insert("X Test".to_string(), "value".to_string());
        assert!(validate_connection(&value).is_err());

        value.headers.clear();
        value
            .headers
            .insert("X-Route".to_string(), "one".to_string());
        value
            .headers
            .insert("x-route".to_string(), "two".to_string());
        assert!(validate_connection(&value).is_err());
    }

    #[test]
    fn requires_an_enabled_model_and_bounds_token_limits() {
        let mut value = connection("https://example.com/v1");
        value.models[0].enabled = false;
        assert!(validate_connection(&value).is_err());
        value.models[0].enabled = true;
        value.models[0].capabilities.context_window = Some(MAX_TOKEN_LIMIT + 1);
        assert!(validate_connection(&value).is_err());
    }

    #[test]
    fn persisted_state_rejects_duplicate_ids_and_orphan_records() {
        let first = connection("https://example.com/v1");
        let mut duplicate = first.clone();
        duplicate.name = "Duplicate".to_string();
        let mut state = PersistentState {
            version: STATE_VERSION,
            connections: vec![first.clone(), duplicate],
            deployments: Vec::new(),
        };
        assert!(validate_persistent_state(&state).is_err());

        state.connections = vec![first];
        state.deployments.push(DeploymentRecord {
            id: "record".to_string(),
            plan_id: "plan".to_string(),
            connection_id: "missing".to_string(),
            target_id: "target".to_string(),
            target_kind: ClientKind::CopilotCli,
            status: DeploymentStatus::Applied,
            detail: "done".to_string(),
            created_at: Utc::now(),
        });
        assert!(validate_persistent_state(&state).is_err());
    }

    #[test]
    fn api_keys_are_bounded_and_reject_multiline_values() {
        assert!(validate_api_key(Some("line-one\nline-two")).is_err());
        assert!(validate_api_key(Some(&"x".repeat(MAX_API_KEY_BYTES + 1))).is_err());
        assert!(validate_api_key(Some("secret")).is_ok());
    }
}
