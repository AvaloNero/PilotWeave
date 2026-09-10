//! Read-only provider catalogs. No persistence, inference calls or client credentials.
use crate::commands::ManagedState;
use crate::domain::{ApiProtocol, Connection, ProviderKind};
use crate::{redact, validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use url::Url;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MODELS: usize = 2048;
const MAX_PAGES: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(15);
static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryInput {
    connection_id: Option<String>,
    base_url: String,
    provider_kind: ProviderKind,
    protocol: ApiProtocol,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    api_key: Option<String>,
    #[serde(default)]
    clear_secret: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Available,
    Empty,
    Partial,
    Unsupported,
    Unauthorized,
    RateLimited,
    NetworkError,
    SchemaError,
    InvalidInput,
    CredentialRequired,
    CredentialUnavailable,
    Busy,
}

impl Status {
    fn detail(self) -> &'static str {
        match self {
            Self::Available => "Select models to add. This catalog does not verify chat compatibility or model capabilities.",
            Self::Empty => "The provider returned an empty model catalog. You can still enter model IDs manually.",
            Self::Partial => "Only part of the catalog was retrieved within the request limits. You can select these models or enter others manually.",
            Self::Unsupported => "This endpoint does not support the selected model-list API. Azure requires deployment names entered manually.",
            Self::Unauthorized => "The provider refused model-list access (401/403). Check the API key and its permissions, or enter model IDs manually.",
            Self::RateLimited => "The provider rate-limited this query. Retry later or enter model IDs manually.",
            Self::NetworkError => "Could not read the model catalog within 15 seconds. Check the endpoint and network, then retry.",
            Self::SchemaError => "The provider returned an unsupported, unsafe or oversized model-list response. Enter model IDs manually.",
            Self::InvalidInput => "Check the URL, API key and header templates. Use HTTPS (HTTP is allowed only on loopback), no URL credentials, query or fragment, and no transport or session headers.",
            Self::CredentialRequired => "The saved credential cannot be used with changed connection settings. Enter the key again for this endpoint, or fetch without a saved credential.",
            Self::CredentialUnavailable => "The saved credential is missing or unavailable. Enter an API key again or retry after unlocking the credential store.",
            Self::Busy => "A model query is already running. Retry when it finishes.",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    model_id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    status: Status,
    detail: String,
    models: Vec<DiscoveredModel>,
    parser_version: u32,
}

impl DiscoveryResult {
    fn new(status: Status, models: Vec<DiscoveredModel>) -> Self {
        Self {
            status,
            detail: redact::redact_text(status.detail()),
            models,
            parser_version: 1,
        }
    }
    fn error(status: Status) -> Self {
        Self::new(status, Vec::new())
    }
}

struct Flight;
impl Flight {
    fn claim() -> Option<Self> {
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn discover_connection_models(
    state: tauri::State<'_, ManagedState>,
    input: DiscoveryInput,
) -> Result<DiscoveryResult, String> {
    let Some(flight) = Flight::claim() else {
        return Ok(DiscoveryResult::error(Status::Busy));
    };
    let state = state.inner().clone();
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let _flight = flight;
        let result = (|| {
            let url = endpoint(&input)?;
            // Capture the configuration and its key under the same locks.
            // Release them before HTTP so editing remains responsive.
            let key = {
                let store = state.store().map_err(|_| Status::CredentialUnavailable)?;
                key_from_store(&store, &input, crate::secrets::get)?
            };
            let headers = request_headers(&input, key.as_deref())?;
            Ok(fetch(&input, url, headers, key.as_deref()))
        })();
        result.unwrap_or_else(DiscoveryResult::error)
    })
    .await
    .unwrap_or_else(|_| DiscoveryResult::error(Status::NetworkError)))
}

fn key_from_store(
    store: &crate::state::StateStore,
    input: &DiscoveryInput,
    read: impl FnOnce(&str) -> crate::error::AppResult<Option<String>>,
) -> Result<Option<String>, Status> {
    let saved = input
        .connection_id
        .as_ref()
        .map(|id| store.connection(id))
        .transpose()
        .map_err(|_| Status::InvalidInput)?;
    let reuse = saved.as_ref().is_some_and(|saved| saved.has_secret)
        && !input.clear_secret
        && input.api_key.as_ref().is_none_or(|key| key.is_empty());
    let _lease = if reuse {
        Some(crate::write_lock::WriteLock::acquire(store.path()).map_err(|_| Status::Busy)?)
    } else {
        None
    };
    // Recheck persisted bytes after taking the cross-process lease. Otherwise
    // another instance could rotate the reference before this key is read.
    store
        .ensure_writable()
        .map_err(|_| Status::CredentialUnavailable)?;
    credential(input, saved.as_ref(), read)
}

fn endpoint(input: &DiscoveryInput) -> Result<Url, Status> {
    if input
        .connection_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 160)
    {
        return Err(Status::InvalidInput);
    }
    validation::validate_endpoint(input.base_url.trim()).map_err(|_| Status::InvalidInput)?;
    validation::validate_api_key(input.api_key.as_deref()).map_err(|_| Status::InvalidInput)?;
    validation::validate_headers(&input.headers).map_err(|_| Status::InvalidInput)?;
    for name in input.headers.keys() {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "cookie"
                | "cookie2"
                | "proxy-authorization"
                | "proxy-connection"
                | "upgrade"
                | "te"
                | "trailer"
        ) {
            return Err(Status::InvalidInput);
        }
    }
    if input.provider_kind == ProviderKind::Azure {
        return Err(Status::Unsupported);
    }
    let mut url = Url::parse(input.base_url.trim()).map_err(|_| Status::InvalidInput)?;
    if url.query().is_some() {
        return Err(Status::InvalidInput);
    }
    let path = url.path().trim_end_matches('/');
    let terminal_prefix = ["/chat/completions", "/responses", "/messages"]
        .iter()
        .find_map(|suffix| path.strip_suffix(suffix));
    let prefix = terminal_prefix.unwrap_or(path);
    let path = if prefix.is_empty() && terminal_prefix.is_none() {
        "/v1/models".to_owned()
    } else if prefix.ends_with("/models") {
        prefix.to_owned()
    } else {
        format!("{prefix}/models")
    };
    url.set_path(&path);
    if input.protocol == ApiProtocol::Messages {
        url.query_pairs_mut().append_pair("limit", "1000");
    }
    Ok(url)
}

fn canonical_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect()
}

fn credential(
    input: &DiscoveryInput,
    saved: Option<&Connection>,
    read: impl FnOnce(&str) -> crate::error::AppResult<Option<String>>,
) -> Result<Option<String>, Status> {
    if let Some(key) = input.api_key.as_ref().filter(|key| !key.is_empty()) {
        if input.clear_secret {
            return Err(Status::InvalidInput);
        }
        return Ok(Some(key.clone()));
    }
    if input.clear_secret {
        return Ok(None);
    }
    let Some(saved) = saved else {
        return Ok(None);
    };
    if !saved.has_secret {
        return Ok(None);
    }
    // A persisted secret is authorized for exactly its saved configuration.
    // Even a same-origin path or custom-header change requires a newly entered key.
    if input.base_url.trim().trim_end_matches('/') != saved.base_url.trim_end_matches('/')
        || input.protocol != saved.protocol
        || input.provider_kind != saved.provider_kind
        || canonical_headers(&input.headers) != canonical_headers(&saved.headers)
    {
        return Err(Status::CredentialRequired);
    }
    validation::validate_connection(saved).map_err(|_| Status::CredentialUnavailable)?;
    let key = read(&saved.secret_ref).map_err(|_| Status::CredentialUnavailable)?;
    if key.as_ref().is_none_or(|key| key.is_empty()) {
        return Err(Status::CredentialUnavailable);
    }
    validation::validate_api_key(key.as_deref()).map_err(|_| Status::CredentialUnavailable)?;
    Ok(key)
}

fn request_headers(
    input: &DiscoveryInput,
    key: Option<&str>,
) -> Result<BTreeMap<String, String>, Status> {
    let mut headers = canonical_headers(&input.headers);
    for value in headers.values_mut() {
        if value.contains("${apiKey}") {
            *value = value.replace("${apiKey}", key.ok_or(Status::CredentialRequired)?);
        }
        if value.len() > 65536 || !value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b)) {
            return Err(Status::InvalidInput);
        }
    }
    if input.protocol == ApiProtocol::Messages {
        headers
            .entry("anthropic-version".into())
            .or_insert("2023-06-01".into());
        if let Some(key) = key {
            headers.entry("x-api-key".into()).or_insert(key.to_owned());
        }
    } else if let Some(key) = key {
        headers
            .entry("authorization".into())
            .or_insert(format!("Bearer {key}"));
    }
    headers
        .entry("accept".into())
        .or_insert("application/json".into());
    Ok(headers)
}

struct Page {
    models: Vec<DiscoveredModel>,
    next: Option<String>,
    partial: bool,
}

fn safe_text(value: &str, limit: usize, key: Option<&str>) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && !value.chars().any(|c| c.is_control() || c == '|')
        && !key.is_some_and(|key| !key.is_empty() && value.contains(key))
        && redact::redact_text(value) == value
}

/// v1: OpenAI data/id catalog; Anthropic data/id/display_name + cursor envelope.
fn parse(bytes: &[u8], protocol: ApiProtocol, key: Option<&str>) -> Result<Page, Status> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Status::SchemaError);
    }
    let body: Value = serde_json::from_slice(bytes).map_err(|_| Status::SchemaError)?;
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or(Status::SchemaError)?;
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for row in data {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or(Status::SchemaError)?;
        if !safe_text(id, 320, key) || id.chars().any(char::is_whitespace) {
            return Err(Status::SchemaError);
        }
        let name = match row.get("display_name").or_else(|| row.get("name")) {
            Some(Value::String(value)) if safe_text(value, 240, key) => value.to_owned(),
            None | Some(Value::Null) => id
                .chars()
                .scan(0, |size, c| {
                    *size += c.len_utf8();
                    (*size <= 240).then_some(c)
                })
                .collect(),
            _ => return Err(Status::SchemaError),
        };
        if seen.insert(id.to_ascii_lowercase()) && models.len() < MAX_MODELS {
            models.push(DiscoveredModel {
                model_id: id.to_owned(),
                name,
            });
        }
    }
    let mut partial = seen.len() > MAX_MODELS;
    let next = if protocol == ApiProtocol::Messages {
        match body.get("has_more").and_then(Value::as_bool) {
            Some(false) => None,
            Some(true) => {
                let cursor = body
                    .get("last_id")
                    .and_then(Value::as_str)
                    .ok_or(Status::SchemaError)?;
                if !safe_text(cursor, 320, key)
                    || data
                        .last()
                        .and_then(|r| r.get("id"))
                        .and_then(Value::as_str)
                        != Some(cursor)
                {
                    return Err(Status::SchemaError);
                }
                Some(cursor.to_owned())
            }
            None => return Err(Status::SchemaError),
        }
    } else {
        // OpenAI has no pagination. Unknown compatible pagination is never called complete.
        if body.get("has_more").is_some_and(|v| v != false)
            || body.get("next").is_some_and(|v| !v.is_null())
        {
            partial = true;
        }
        None
    };
    Ok(Page {
        models,
        next,
        partial,
    })
}

fn http_status(code: u16) -> Status {
    match code {
        401 | 403 => Status::Unauthorized,
        404 | 405 | 300..=399 => Status::Unsupported,
        429 => Status::RateLimited,
        _ => Status::NetworkError,
    }
}

fn fetch(
    input: &DiscoveryInput,
    mut url: Url,
    headers: BTreeMap<String, String>,
    key: Option<&str>,
) -> DiscoveryResult {
    let started = Instant::now();
    let mut remaining = MAX_BYTES;
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    let mut cursors = HashSet::new();
    for page_number in 0..MAX_PAGES {
        let result = (|| {
            let timeout = TIMEOUT
                .checked_sub(started.elapsed())
                .filter(|d| !d.is_zero())
                .ok_or(Status::NetworkError)?;
            let config = crate::platform::http_config()
                .https_only(false)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(timeout))
                .timeout_connect(Some(timeout.min(Duration::from_secs(5))));
            // Loopback requests must never send credentials through an ambient proxy.
            let config = if url.scheme() == "http" {
                config.proxy(None)
            } else {
                config
            };
            let agent: ureq::Agent = config.build().into();
            let mut request = agent.get(crate::platform::endpoint(url.as_str()));
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            let mut response = request.call().map_err(|_| Status::NetworkError)?;
            let code = response.status().as_u16();
            if code != 200 {
                return Err(http_status(code));
            }
            let bytes = response
                .body_mut()
                .with_config()
                .limit(remaining)
                .read_to_vec()
                .map_err(|_| Status::SchemaError)?;
            remaining = remaining.saturating_sub(bytes.len() as u64);
            parse(&bytes, input.protocol, key)
        })();
        let page = match result {
            Ok(page) => page,
            Err(status) => {
                return DiscoveryResult::new(
                    if models.is_empty() {
                        status
                    } else {
                        Status::Partial
                    },
                    models,
                )
            }
        };
        let mut partial = page.partial;
        for model in page.models {
            if seen.insert(model.model_id.to_ascii_lowercase()) {
                if models.len() == MAX_MODELS {
                    partial = true;
                    break;
                }
                models.push(model);
            }
        }
        let Some(next) = page.next else {
            let status = if partial {
                Status::Partial
            } else if models.is_empty() {
                Status::Empty
            } else {
                Status::Available
            };
            return DiscoveryResult::new(status, models);
        };
        if partial
            || models.len() == MAX_MODELS
            || remaining == 0
            || page_number + 1 == MAX_PAGES
            || !cursors.insert(next.clone())
        {
            return DiscoveryResult::new(Status::Partial, models);
        }
        // Never follow upstream URLs. Pagination changes only a bounded cursor on this URL.
        url.query_pairs_mut()
            .clear()
            .append_pair("limit", "1000")
            .append_pair("after_id", &next);
    }
    DiscoveryResult::new(Status::Partial, models)
}

#[cfg(test)]
mod tests;
