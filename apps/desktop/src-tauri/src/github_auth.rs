use crate::error::{AppError, AppResult};
use crate::redact;
use crate::secrets;
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const AUTH_STATE_VERSION: u32 = 1;
const AUTH_SECRET_REF: &str = "github:personal-usage";
const API_VERSION: &str = "2026-03-10";
const USER_ENDPOINT: &str = "https://api.github.com/user";
const MAX_AUTH_STATE_BYTES: u64 = 256 * 1_024;
const MAX_TOKEN_BYTES: usize = 64 * 1_024;
const MAX_RESPONSE_BYTES: u64 = 512 * 1_024;
const MAX_SCOPES: usize = 64;
const MAX_SCOPE_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 1_024;
const MAX_BILLING_ITEMS: usize = 2_048;
const REQUEST_TIMEOUT_SECONDS: u64 = 20;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GithubAuthorizationState {
    Missing,
    Verified,
    Unauthorized,
    Forbidden,
    NetworkError,
    CredentialUnavailable,
    SchemaError,
    Conflict,
    ReadOnlyRecovery,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GithubBillingCapability {
    Available,
    InsufficientPermission,
    NotCovered,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubAuthorizationIdentity {
    pub host: String,
    pub login: String,
    pub user_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubAuthorizationStatus {
    pub state: GithubAuthorizationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<GithubAuthorizationIdentity>,
    pub has_secret: bool,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub billing_capability: GithubBillingCapability,
    pub billing_detail: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GithubAuthorizationValidation {
    identity: GithubAuthorizationIdentity,
    scopes: Vec<String>,
    billing_capability: GithubBillingCapability,
    billing_detail: String,
    validated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum GithubValidationOutcome {
    Verified(GithubAuthorizationValidation),
    Rejected(GithubAuthorizationStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubAuthorizationRecord {
    host: String,
    login: String,
    user_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
    secret_ref: String,
    #[serde(default)]
    scopes: Vec<String>,
    billing_capability: GithubBillingCapability,
    billing_detail: String,
    validated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubAuthorizationFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validation_failure: Option<ValidationFailure>,
    #[serde(default = "default_auth_state_version")]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authorization: Option<GithubAuthorizationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidationFailure {
    state: GithubAuthorizationState,
    detail: String,
    checked_at: DateTime<Utc>,
}

impl Default for GithubAuthorizationFile {
    fn default() -> Self {
        Self {
            version: AUTH_STATE_VERSION,
            authorization: None,
            validation_failure: None,
        }
    }
}

fn default_auth_state_version() -> u32 {
    AUTH_STATE_VERSION
}

trait SecretBackend: Send {
    fn get(&self, secret_ref: &str) -> AppResult<Option<String>>;
    fn set(&self, secret_ref: &str, value: &str) -> AppResult<()>;
    fn delete(&self, secret_ref: &str) -> AppResult<()>;
}

struct NativeSecretBackend;

impl SecretBackend for NativeSecretBackend {
    fn get(&self, secret_ref: &str) -> AppResult<Option<String>> {
        secrets::get(secret_ref)
    }

    fn set(&self, secret_ref: &str, value: &str) -> AppResult<()> {
        secrets::set(secret_ref, value)
    }

    fn delete(&self, secret_ref: &str) -> AppResult<()> {
        secrets::delete(secret_ref)
    }
}

pub struct GithubAuthorizationStore {
    path: PathBuf,
    state: GithubAuthorizationFile,
    recovery: Option<String>,
    secrets: Box<dyn SecretBackend>,
    revision: u64,
    observed_bytes: Option<Vec<u8>>,
}

impl GithubAuthorizationStore {
    pub fn open() -> Self {
        let Some(config_dir) = dirs::config_dir() else {
            return Self {
                path: PathBuf::new(),
                state: GithubAuthorizationFile::default(),
                recovery: Some("Cannot resolve the user config directory".to_string()),
                secrets: Box::new(NativeSecretBackend),
                revision: 0,
                observed_bytes: None,
            };
        };
        Self::open_at_with_backend(
            config_dir
                .join("PilotWeave")
                .join("github-authorization.json"),
            Box::new(NativeSecretBackend),
        )
    }

    fn open_at_with_backend(path: PathBuf, secrets: Box<dyn SecretBackend>) -> Self {
        match load_state(&path) {
            Ok(state) => Self {
                observed_bytes: crate::safe_io::read_optional(&path, MAX_AUTH_STATE_BYTES)
                    .ok()
                    .flatten(),
                path,
                state,
                recovery: None,
                secrets,
                revision: 0,
            },
            Err(error) => Self {
                path,
                state: GithubAuthorizationFile::default(),
                recovery: Some(error.to_string()),
                secrets,
                revision: 0,
                observed_bytes: None,
            },
        }
    }

    pub fn status(&self) -> GithubAuthorizationStatus {
        if let Some(reason) = &self.recovery {
            return GithubAuthorizationStatus {
                state: GithubAuthorizationState::ReadOnlyRecovery,
                identity: self.state.authorization.as_ref().map(record_identity),
                has_secret: false,
                scopes: Vec::new(),
                billing_capability: GithubBillingCapability::Unknown,
                billing_detail: "Authorization capability is unavailable in recovery mode"
                    .to_string(),
                detail: "The GitHub authorization metadata file could not be loaded; it has not been overwritten"
                    .to_string(),
                validated_at: None,
                recovery: Some(redact::redact_text(reason)),
                cleanup_warning: None,
            };
        }

        let secret = match self.secrets.get(AUTH_SECRET_REF) {
            Ok(value) => value,
            Err(_) => {
                return GithubAuthorizationStatus {
                    state: GithubAuthorizationState::CredentialUnavailable,
                    identity: self.state.authorization.as_ref().map(record_identity),
                    has_secret: false,
                    scopes: self
                        .state
                        .authorization
                        .as_ref()
                        .map(|record| record.scopes.clone())
                        .unwrap_or_default(),
                    billing_capability: self
                        .state
                        .authorization
                        .as_ref()
                        .map(|record| record.billing_capability)
                        .unwrap_or(GithubBillingCapability::Unknown),
                    billing_detail: self
                        .state
                        .authorization
                        .as_ref()
                        .map(|record| record.billing_detail.clone())
                        .unwrap_or_else(|| "Billing capability is unknown".to_string()),
                    detail: "The operating-system credential store could not be read".to_string(),
                    validated_at: self
                        .state
                        .authorization
                        .as_ref()
                        .map(|record| record.validated_at),
                    recovery: None,
                    cleanup_warning: None,
                }
            }
        };

        let mut status = match (&self.state.authorization, secret.is_some()) {
            (Some(record), true) => GithubAuthorizationStatus {
                state: GithubAuthorizationState::Verified,
                identity: Some(record_identity(record)),
                has_secret: true,
                scopes: record.scopes.clone(),
                billing_capability: record.billing_capability,
                billing_detail: record.billing_detail.clone(),
                detail: "PilotWeave has a separate GitHub authorization validated through the official REST API"
                    .to_string(),
                validated_at: Some(record.validated_at),
                recovery: None,
                cleanup_warning: None,
            },
            (Some(record), false) => GithubAuthorizationStatus {
                state: GithubAuthorizationState::Missing,
                identity: Some(record_identity(record)),
                has_secret: false,
                scopes: record.scopes.clone(),
                billing_capability: GithubBillingCapability::Unknown,
                billing_detail: "The stored authorization secret is missing".to_string(),
                detail: "Authorization metadata exists, but the operating-system credential entry is missing"
                    .to_string(),
                validated_at: Some(record.validated_at),
                recovery: None,
                cleanup_warning: None,
            },
            (None, true) => GithubAuthorizationStatus {
                state: GithubAuthorizationState::Conflict,
                identity: None,
                has_secret: true,
                scopes: Vec::new(),
                billing_capability: GithubBillingCapability::Unknown,
                billing_detail: "Billing capability cannot be attributed without metadata"
                    .to_string(),
                detail: "An orphan PilotWeave GitHub credential exists without validated metadata; clear or re-authorize it"
                    .to_string(),
                validated_at: None,
                recovery: None,
                cleanup_warning: None,
            },
            (None, false) => missing_status(),
        };
        if status.state == GithubAuthorizationState::Verified {
            if let Some(failure) = &self.state.validation_failure {
                status.state = failure.state;
                status.detail =
                    format!("{} (last attempt: {})", failure.detail, failure.checked_at);
                status.billing_capability = GithubBillingCapability::Unknown;
                status.billing_detail =
                    "Previous capability is stale until authorization is revalidated".into();
            }
        }
        status
    }

    /// Latest-started request wins. Clearing also invalidates in-flight work.
    pub fn begin_attempt(&mut self) -> AppResult<u64> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::Config("Authorization revision exhausted; restart".into()))?;
        Ok(self.revision)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn ensure_current(&self, revision: u64) -> AppResult<()> {
        if revision != self.revision {
            return Err(AppError::Config(
                "Authorization changed while the request was running; stale result discarded"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn record_refresh_failure(
        &mut self,
        failure: GithubAuthorizationStatus,
    ) -> AppResult<GithubAuthorizationStatus> {
        self.ensure_writable()?;
        let _lease = self.lease_and_check_disk()?;
        validate_text("Validation failure", &failure.detail)?;
        let previous = self.state.clone();
        self.state.validation_failure = Some(ValidationFailure {
            state: failure.state,
            detail: failure.detail,
            checked_at: Utc::now(),
        });
        if let Err(error) = write_state(&self.path, &self.state) {
            self.state = previous;
            return Err(error);
        }
        self.capture_committed_bytes()?;
        Ok(self.status())
    }

    pub fn secret_for_refresh(&self) -> AppResult<Option<String>> {
        if let Some(reason) = &self.recovery {
            return Err(AppError::Unsupported(format!(
                "GitHub authorization is in read-only recovery ({reason})"
            )));
        }
        self.secrets.get(AUTH_SECRET_REF)
    }

    pub fn save_verified(
        &mut self,
        token: &str,
        validation: GithubAuthorizationValidation,
    ) -> AppResult<GithubAuthorizationStatus> {
        self.ensure_writable()?;
        let _lease = self.lease_and_check_disk()?;
        validate_token_input(token)?;
        validate_identity(&validation.identity)?;
        validate_scopes(&validation.scopes)?;
        validate_text("Billing capability detail", &validation.billing_detail)?;

        let previous_state = self.state.clone();
        let previous_secret = self.secrets.get(AUTH_SECRET_REF)?;
        self.secrets.set(AUTH_SECRET_REF, token)?;
        self.state = GithubAuthorizationFile {
            version: AUTH_STATE_VERSION,
            validation_failure: None,
            authorization: Some(GithubAuthorizationRecord {
                host: validation.identity.host,
                login: validation.identity.login,
                user_id: validation.identity.user_id,
                avatar_url: validation.identity.avatar_url,
                secret_ref: AUTH_SECRET_REF.to_string(),
                scopes: validation.scopes,
                billing_capability: validation.billing_capability,
                billing_detail: validation.billing_detail,
                validated_at: validation.validated_at,
            }),
        };
        if let Err(error) = write_state(&self.path, &self.state) {
            self.state = previous_state;
            let rollback = match previous_secret {
                Some(value) => self.secrets.set(AUTH_SECRET_REF, &value),
                None => self.secrets.delete(AUTH_SECRET_REF),
            };
            if let Err(rollback_error) = rollback {
                return Err(AppError::Config(format!(
                    "{error}; additionally failed to restore the previous GitHub credential: {rollback_error}"
                )));
            }
            return Err(error);
        }
        self.capture_committed_bytes()?;
        Ok(self.status())
    }

    pub fn clear(&mut self) -> AppResult<GithubAuthorizationStatus> {
        self.begin_attempt()?;
        self.ensure_writable()?;
        let _lease = self.lease_and_check_disk()?;
        let previous_state = self.state.clone();
        self.state = GithubAuthorizationFile::default();
        if let Err(error) = write_state(&self.path, &self.state) {
            self.state = previous_state;
            return Err(error);
        }

        self.capture_committed_bytes()?;
        match self.secrets.delete(AUTH_SECRET_REF) {
            Ok(()) => Ok(missing_status()),
            Err(_) => Ok(GithubAuthorizationStatus {
                cleanup_warning: Some(
                    "Authorization metadata was cleared, but the operating-system credential could not be deleted"
                        .to_string(),
                ),
                ..missing_status()
            }),
        }
    }

    fn lease_and_check_disk(&self) -> AppResult<crate::safe_io::Lease> {
        let lease =
            crate::safe_io::Lease::acquire(&crate::safe_io::sibling(&self.path, ".write-lock"))?;
        if crate::safe_io::read_optional(&self.path, MAX_AUTH_STATE_BYTES)? != self.observed_bytes {
            return Err(AppError::Config(
                "GitHub authorization changed in another process; restart before changing it"
                    .into(),
            ));
        }
        Ok(lease)
    }

    fn capture_committed_bytes(&mut self) -> AppResult<()> {
        self.observed_bytes = crate::safe_io::read_optional(&self.path, MAX_AUTH_STATE_BYTES)?;
        Ok(())
    }

    fn ensure_writable(&self) -> AppResult<()> {
        if let Some(reason) = &self.recovery {
            return Err(AppError::Unsupported(format!(
                "GitHub authorization is in read-only recovery ({reason}); fix or remove the metadata file and restart"
            )));
        }
        if self.path.as_os_str().is_empty() {
            return Err(AppError::Unsupported(
                "GitHub authorization metadata has no writable path".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn validate_token_native(token: &str) -> AppResult<GithubValidationOutcome> {
    validate_token_input(token)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(REQUEST_TIMEOUT_SECONDS)))
        .https_only(true)
        .max_redirects(0)
        .user_agent("PilotWeave/0.1")
        .build()
        .into();
    let authorization = format!("Bearer {token}");

    let mut response = match agent
        .get(USER_ENDPOINT)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", &authorization)
        .header("X-GitHub-Api-Version", API_VERSION)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401)) => {
            return Ok(GithubValidationOutcome::Rejected(transient_status(
                GithubAuthorizationState::Unauthorized,
                "GitHub rejected the authorization token",
            )))
        }
        Err(ureq::Error::StatusCode(403)) => {
            return Ok(GithubValidationOutcome::Rejected(transient_status(
                GithubAuthorizationState::Forbidden,
                "GitHub forbade the authorization request; rate limiting or policy may apply",
            )))
        }
        Err(ureq::Error::StatusCode(_)) | Err(ureq::Error::Protocol(_)) => {
            return Ok(GithubValidationOutcome::Rejected(transient_status(
                GithubAuthorizationState::NetworkError,
                "GitHub returned an unexpected response while validating authorization",
            )))
        }
        Err(_) => {
            return Ok(GithubValidationOutcome::Rejected(transient_status(
                GithubAuthorizationState::NetworkError,
                "GitHub could not be reached while validating authorization",
            )))
        }
    };

    let scopes = response
        .headers()
        .get("x-oauth-scopes")
        .and_then(|value| value.to_str().ok())
        .map(parse_scopes)
        .unwrap_or_default();
    validate_scopes(&scopes)?;

    #[derive(Deserialize)]
    struct ApiUser {
        login: String,
        id: u64,
        avatar_url: Option<String>,
    }

    let user = match response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_json::<ApiUser>()
    {
        Ok(user) => user,
        Err(_) => {
            return Ok(GithubValidationOutcome::Rejected(transient_status(
                GithubAuthorizationState::SchemaError,
                "GitHub's authenticated-user response did not match the supported schema",
            )))
        }
    };
    let identity = GithubAuthorizationIdentity {
        host: "github.com".to_string(),
        login: user.login,
        user_id: user.id,
        avatar_url: user.avatar_url.and_then(validate_avatar_url),
    };
    if validate_identity(&identity).is_err() {
        return Ok(GithubValidationOutcome::Rejected(transient_status(
            GithubAuthorizationState::SchemaError,
            "GitHub returned an invalid authenticated identity",
        )));
    }

    let (billing_capability, billing_detail) =
        probe_personal_billing(&agent, &authorization, &identity.login);
    Ok(GithubValidationOutcome::Verified(
        GithubAuthorizationValidation {
            identity,
            scopes,
            billing_capability,
            billing_detail,
            validated_at: Utc::now(),
        },
    ))
}

fn probe_personal_billing(
    agent: &ureq::Agent,
    authorization: &str,
    login: &str,
) -> (GithubBillingCapability, String) {
    let now = Utc::now();
    let endpoint = format!(
        "https://api.github.com/users/{login}/settings/billing/premium_request/usage?year={}&month={}",
        now.year(),
        now.month()
    );
    let mut response = match agent
        .get(&endpoint)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", authorization)
        .header("X-GitHub-Api-Version", API_VERSION)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(403)) => {
            return (
                GithubBillingCapability::InsufficientPermission,
                "The token identity is valid, but personal Copilot Billing requires Plan user permission (read)"
                    .to_string(),
            )
        }
        Err(ureq::Error::StatusCode(404)) => {
            return (
                GithubBillingCapability::NotCovered,
                "Personal Billing is not exposed for this account/token combination; organization-paid usage is outside this MVP"
                    .to_string(),
            )
        }
        Err(ureq::Error::StatusCode(_)) | Err(ureq::Error::Protocol(_)) => {
            return (
                GithubBillingCapability::Unavailable,
                "GitHub returned an unexpected Billing response".to_string(),
            )
        }
        Err(_) => {
            return (
                GithubBillingCapability::Unavailable,
                "GitHub Billing could not be reached during the capability probe".to_string(),
            )
        }
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BillingProbe {
        user: String,
        usage_items: Vec<serde_json::Value>,
    }

    match response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_json::<BillingProbe>()
    {
        Ok(report)
            if report.user.eq_ignore_ascii_case(login)
                && report.usage_items.len() <= MAX_BILLING_ITEMS =>
        {
            (
                GithubBillingCapability::Available,
                "Personal premium-request Billing is available for this authorization".to_string(),
            )
        }
        _ => (
            GithubBillingCapability::Unavailable,
            "GitHub Billing returned an unsupported or oversized schema".to_string(),
        ),
    }
}

fn parse_scopes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(MAX_SCOPES + 1)
        .map(str::to_string)
        .collect()
}

fn validate_token_input(token: &str) -> AppResult<()> {
    if token.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "GitHub authorization token is required".to_string(),
        ));
    }
    if token.len() > MAX_TOKEN_BYTES {
        return Err(AppError::InvalidInput(format!(
            "GitHub authorization token exceeds {} KiB",
            MAX_TOKEN_BYTES / 1_024
        )));
    }
    if token.trim() != token || !token.is_ascii() || token.chars().any(char::is_control) {
        return Err(AppError::InvalidInput(
            "GitHub authorization token must be ASCII and must not contain whitespace padding or control characters"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_identity(identity: &GithubAuthorizationIdentity) -> AppResult<()> {
    if identity.host != "github.com" {
        return Err(AppError::InvalidInput(
            "Only github.com authorization is supported".to_string(),
        ));
    }
    if !valid_login(&identity.login) || identity.user_id == 0 {
        return Err(AppError::InvalidInput(
            "GitHub authorization identity is invalid".to_string(),
        ));
    }
    if let Some(value) = &identity.avatar_url {
        if validate_avatar_url(value.clone()).is_none() {
            return Err(AppError::InvalidInput(
                "GitHub avatar URL is invalid".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_scopes(scopes: &[String]) -> AppResult<()> {
    if scopes.len() > MAX_SCOPES {
        return Err(AppError::InvalidInput(format!(
            "GitHub authorization reports more than {MAX_SCOPES} scopes"
        )));
    }
    for scope in scopes {
        if scope.is_empty()
            || scope.len() > MAX_SCOPE_BYTES
            || scope
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || b":_-".contains(&byte)))
        {
            return Err(AppError::InvalidInput(
                "GitHub authorization reports an invalid scope value".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_text(label: &str, value: &str) -> AppResult<()> {
    if value.len() > MAX_TEXT_BYTES || value.chars().any(|character| character == '\0') {
        return Err(AppError::InvalidInput(format!(
            "{label} exceeds its safety limit"
        )));
    }
    Ok(())
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
    if value.len() > MAX_TEXT_BYTES {
        return None;
    }
    let url = Url::parse(&value).ok()?;
    (url.scheme() == "https" && url.host_str().is_some()).then_some(value)
}

fn record_identity(record: &GithubAuthorizationRecord) -> GithubAuthorizationIdentity {
    GithubAuthorizationIdentity {
        host: record.host.clone(),
        login: record.login.clone(),
        user_id: record.user_id,
        avatar_url: record.avatar_url.clone(),
    }
}

fn missing_status() -> GithubAuthorizationStatus {
    GithubAuthorizationStatus {
        state: GithubAuthorizationState::Missing,
        identity: None,
        has_secret: false,
        scopes: Vec::new(),
        billing_capability: GithubBillingCapability::Unknown,
        billing_detail: "Authorize PilotWeave before requesting personal Billing data".to_string(),
        detail: "PilotWeave has no separate GitHub authorization".to_string(),
        validated_at: None,
        recovery: None,
        cleanup_warning: None,
    }
}

fn transient_status(
    state: GithubAuthorizationState,
    detail: impl Into<String>,
) -> GithubAuthorizationStatus {
    GithubAuthorizationStatus {
        state,
        identity: None,
        has_secret: false,
        scopes: Vec::new(),
        billing_capability: GithubBillingCapability::Unknown,
        billing_detail: "Billing capability was not established".to_string(),
        detail: detail.into(),
        validated_at: None,
        recovery: None,
        cleanup_warning: None,
    }
}

fn load_state(path: &Path) -> AppResult<GithubAuthorizationFile> {
    let Some(bytes) = crate::safe_file::read_optional(path, MAX_AUTH_STATE_BYTES)? else {
        return Ok(GithubAuthorizationFile::default());
    };
    let state: GithubAuthorizationFile =
        serde_json::from_slice(&bytes).map_err(|error| AppError::json(path, error))?;
    if state.version > AUTH_STATE_VERSION {
        return Err(AppError::Config(format!(
            "GitHub authorization metadata version {} is newer than this build supports ({AUTH_STATE_VERSION})",
            state.version
        )));
    }
    if let Some(record) = &state.authorization {
        if record.secret_ref != AUTH_SECRET_REF {
            return Err(AppError::InvalidInput(
                "GitHub authorization metadata contains an unexpected credential reference"
                    .to_string(),
            ));
        }
        validate_identity(&record_identity(record))?;
        validate_scopes(&record.scopes)?;
        validate_text("Billing capability detail", &record.billing_detail)?;
    }
    if let Some(failure) = &state.validation_failure {
        validate_text("Validation failure", &failure.detail)?;
        if !matches!(
            failure.state,
            GithubAuthorizationState::Unauthorized
                | GithubAuthorizationState::Forbidden
                | GithubAuthorizationState::NetworkError
                | GithubAuthorizationState::SchemaError
        ) {
            return Err(AppError::Config(
                "Invalid persisted authorization failure state".into(),
            ));
        }
    }
    Ok(GithubAuthorizationFile {
        version: AUTH_STATE_VERSION,
        validation_failure: state.validation_failure,
        authorization: state.authorization,
    })
}

fn write_state(path: &Path, state: &GithubAuthorizationFile) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
        AppError::Config(format!("Cannot serialize authorization metadata: {error}"))
    })?;
    if bytes.len() as u64 > MAX_AUTH_STATE_BYTES {
        return Err(AppError::InvalidInput(
            "authorization metadata exceeds its storage limit".into(),
        ));
    }
    crate::safe_io::write_private(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct MemorySecretBackend {
        values: Arc<Mutex<HashMap<String, String>>>,
    }

    impl SecretBackend for MemorySecretBackend {
        fn get(&self, secret_ref: &str) -> AppResult<Option<String>> {
            Ok(self
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(secret_ref)
                .cloned())
        }

        fn set(&self, secret_ref: &str, value: &str) -> AppResult<()> {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(secret_ref.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, secret_ref: &str) -> AppResult<()> {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(secret_ref);
            Ok(())
        }
    }

    fn validation(login: &str) -> GithubAuthorizationValidation {
        GithubAuthorizationValidation {
            identity: GithubAuthorizationIdentity {
                host: "github.com".to_string(),
                login: login.to_string(),
                user_id: 42,
                avatar_url: Some("https://avatars.githubusercontent.com/u/42".to_string()),
            },
            scopes: vec!["read:user".to_string()],
            billing_capability: GithubBillingCapability::InsufficientPermission,
            billing_detail: "Plan user permission (read) is required".to_string(),
            validated_at: Utc::now(),
        }
    }

    #[test]
    fn verified_metadata_and_secret_are_stored_separately() {
        let directory = tempfile::tempdir().expect("directory");
        let backend = MemorySecretBackend::default();
        let probe = backend.clone();
        let mut store = GithubAuthorizationStore::open_at_with_backend(
            directory.path().join("github-authorization.json"),
            Box::new(backend),
        );

        let status = store
            .save_verified("github_pat_test", validation("octocat"))
            .expect("save");
        assert_eq!(status.state, GithubAuthorizationState::Verified);
        assert!(status.has_secret);
        assert_eq!(status.identity.expect("identity").login, "octocat");
        assert_eq!(
            probe.get(AUTH_SECRET_REF).expect("secret"),
            Some("github_pat_test".to_string())
        );
        let metadata = fs::read_to_string(directory.path().join("github-authorization.json"))
            .expect("metadata");
        assert!(!metadata.contains("github_pat_test"));
        assert!(metadata.contains("octocat"));
    }

    #[test]
    fn corrupt_metadata_enters_read_only_recovery_without_overwriting() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("github-authorization.json");
        fs::write(&path, b"not json").expect("write");
        let mut store = GithubAuthorizationStore::open_at_with_backend(
            path.clone(),
            Box::new(MemorySecretBackend::default()),
        );
        assert_eq!(
            store.status().state,
            GithubAuthorizationState::ReadOnlyRecovery
        );
        assert!(store
            .save_verified("github_pat_test", validation("octocat"))
            .is_err());
        assert_eq!(fs::read(&path).expect("metadata"), b"not json");
    }

    #[test]
    fn failed_metadata_write_restores_the_previous_secret() {
        let directory = tempfile::tempdir().expect("directory");
        let parent_file = directory.path().join("not-a-directory");
        fs::write(&parent_file, b"file").expect("write parent file");
        let backend = MemorySecretBackend::default();
        backend
            .set(AUTH_SECRET_REF, "old-token")
            .expect("old secret");
        let probe = backend.clone();
        let mut store = GithubAuthorizationStore::open_at_with_backend(
            parent_file.join("github-authorization.json"),
            Box::new(backend),
        );

        assert!(store
            .save_verified("new-token", validation("octocat"))
            .is_err());
        assert_eq!(
            probe.get(AUTH_SECRET_REF).expect("secret"),
            Some("old-token".to_string())
        );
    }

    #[test]
    fn clear_removes_metadata_and_secret() {
        let directory = tempfile::tempdir().expect("directory");
        let backend = MemorySecretBackend::default();
        let probe = backend.clone();
        let mut store = GithubAuthorizationStore::open_at_with_backend(
            directory.path().join("github-authorization.json"),
            Box::new(backend),
        );
        store
            .save_verified("github_pat_test", validation("octocat"))
            .expect("save");
        let status = store.clear().expect("clear");
        assert_eq!(status.state, GithubAuthorizationState::Missing);
        assert_eq!(probe.get(AUTH_SECRET_REF).expect("secret"), None);
    }

    #[test]
    fn rejects_whitespace_and_multiline_tokens() {
        assert!(validate_token_input(" token").is_err());
        assert!(validate_token_input("token\nvalue").is_err());
        assert!(validate_token_input("github_pat_test").is_ok());
        assert!(validate_token_input("github_pat_令牌").is_err());
    }

    #[test]
    fn scopes_are_bounded_and_validated() {
        assert_eq!(parse_scopes("read:user, gist"), ["read:user", "gist"]);
        assert!(validate_scopes(&["invalid scope".to_string()]).is_err());
        assert!(validate_scopes(&vec!["scope".to_string(); MAX_SCOPES + 1]).is_err());
    }
    #[test]
    fn clear_and_new_requests_invalidate_old_results() {
        let dir = tempfile::tempdir().expect("temp");
        let mut store = GithubAuthorizationStore::open_at_with_backend(
            dir.path().join("auth.json"),
            Box::new(MemorySecretBackend::default()),
        );
        let first = store.begin_attempt().expect("first");
        let second = store.begin_attempt().expect("second");
        assert!(store.ensure_current(first).is_err());
        assert!(store.ensure_current(second).is_ok());
        store.clear().expect("clear");
        assert!(store.ensure_current(second).is_err());
    }

    #[test]
    fn revoked_authorization_does_not_reappear_verified_after_reopen() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("auth.json");
        let backend = MemorySecretBackend::default();
        let mut store =
            GithubAuthorizationStore::open_at_with_backend(path.clone(), Box::new(backend.clone()));
        store
            .save_verified("synthetic-token", validation("octocat"))
            .expect("save");
        store
            .record_refresh_failure(transient_status(
                GithubAuthorizationState::Unauthorized,
                "Revoked",
            ))
            .expect("record");
        let reopened = GithubAuthorizationStore::open_at_with_backend(path, Box::new(backend));
        assert_eq!(
            reopened.status().state,
            GithubAuthorizationState::Unauthorized
        );
    }
}
