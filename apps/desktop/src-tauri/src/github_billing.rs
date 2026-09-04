use crate::decimal::ExactDecimal;
use crate::error::{AppError, AppResult};
use crate::github_auth::{GithubAuthorizationIdentity, GithubAuthorizationStatus};
use crate::domain::UsageDbStatus;
use chrono::{DateTime, Datelike, Months, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

pub const GITHUB_BILLING_API_VERSION: &str = "2026-03-10";
pub const MAX_BILLING_ITEMS_PER_SNAPSHOT: usize = 2_048;
pub const MAX_BILLING_ITEMS_RETURNED: usize = 250;
const MAX_RESPONSE_BYTES: u64 = 2 * 1_024 * 1_024;
const MAX_TOKEN_BYTES: usize = 64 * 1_024;
const MAX_TEXT_BYTES: usize = 512;
const REQUEST_TIMEOUT_SECONDS: u64 = 20;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum GithubBillingEndpointFamily {
    AiCredit,
    PremiumRequest,
}

impl GithubBillingEndpointFamily {
    pub const ALL: [Self; 2] = [Self::AiCredit, Self::PremiumRequest];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AiCredit => "ai-credit",
            Self::PremiumRequest => "premium-request",
        }
    }

    pub fn path_segment(self) -> &'static str {
        match self {
            Self::AiCredit => "ai_credit",
            Self::PremiumRequest => "premium_request",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "ai-credit" => Some(Self::AiCredit),
            "premium-request" => Some(Self::PremiumRequest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GithubBillingSnapshotStatus {
    Available,
    SuccessfulEmpty,
    Unauthorized,
    InsufficientPermission,
    NotCovered,
    RateLimited,
    NetworkError,
    SchemaError,
    Unavailable,
}

impl GithubBillingSnapshotStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::SuccessfulEmpty => "successful-empty",
            Self::Unauthorized => "unauthorized",
            Self::InsufficientPermission => "insufficient-permission",
            Self::NotCovered => "not-covered",
            Self::RateLimited => "rate-limited",
            Self::NetworkError => "network-error",
            Self::SchemaError => "schema-error",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "successful-empty" => Some(Self::SuccessfulEmpty),
            "unauthorized" => Some(Self::Unauthorized),
            "insufficient-permission" => Some(Self::InsufficientPermission),
            "not-covered" => Some(Self::NotCovered),
            "rate-limited" => Some(Self::RateLimited),
            "network-error" => Some(Self::NetworkError),
            "schema-error" => Some(Self::SchemaError),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }

    pub fn is_success(self) -> bool {
        matches!(Self::Available | Self::SuccessfulEmpty, self)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GithubBillingCoverage {
    PersonalAccountOnly,
    NotCovered,
    Unknown,
}

impl GithubBillingCoverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PersonalAccountOnly => "personal-account-only",
            Self::NotCovered => "not-covered",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "personal-account-only" => Some(Self::PersonalAccountOnly),
            "not-covered" => Some(Self::NotCovered),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubBillingPeriod {
    pub year: i32,
    pub month: u32,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl GithubBillingPeriod {
    pub fn current() -> AppResult<Self> {
        let now = Utc::now();
        Self::new(now.year(), now.month())
    }

    pub fn new(year: i32, month: u32) -> AppResult<Self> {
        let first = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| {
            AppError::InvalidInput("Billing period must contain a valid year and month".to_string())
        })?;
        let current_now = Utc::now();
        let current = NaiveDate::from_ymd_opt(current_now.year(), current_now.month(), 1)
            .ok_or_else(|| AppError::Config("Cannot construct the current month".to_string()))?;
        let oldest = current
            .checked_sub_months(Months::new(23))
            .ok_or_else(|| AppError::Config("Cannot construct the Billing history window".to_string()))?;
        if first < oldest || first > current {
            return Err(AppError::InvalidInput(
                "Personal GitHub Billing is limited to the current month and the previous 23 months"
                    .to_string(),
            ));
        }
        let next = first
            .checked_add_months(Months::new(1))
            .ok_or_else(|| AppError::InvalidInput("Billing period is out of range".to_string()))?;
        let start = first
            .and_hms_opt(0, 0, 0)
            .map(|value| Utc.from_utc_datetime(&value))
            .ok_or_else(|| AppError::Config("Cannot construct Billing period start".to_string()))?;
        let end = next
            .and_hms_opt(0, 0, 0)
            .map(|value| Utc.from_utc_datetime(&value))
            .ok_or_else(|| AppError::Config("Cannot construct Billing period end".to_string()))?;
        Ok(Self {
            year,
            month,
            start,
            end,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubBillingItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub quantity: ExactDecimal,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_amount_usd: Option<ExactDecimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discount_amount_usd: Option<ExactDecimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_amount_usd: Option<ExactDecimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowance: Option<ExactDecimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<ExactDecimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubBillingSnapshot {
    pub id: String,
    pub account_hint: String,
    pub endpoint_family: GithubBillingEndpointFamily,
    pub api_version: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub coverage: GithubBillingCoverage,
    pub status: GithubBillingSnapshotStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub items: Vec<GithubBillingItem>,
    pub total_item_count: u64,
    pub items_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubBillingFamilyView {
    pub endpoint_family: GithubBillingEndpointFamily,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<GithubBillingSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful: Option<GithubBillingSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubBillingOverview {
    pub authorization: GithubAuthorizationStatus,
    pub storage: UsageDbStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<GithubAuthorizationIdentity>,
    pub families: Vec<GithubBillingFamilyView>,
    pub observed_at: DateTime<Utc>,
}

pub fn empty_family_views() -> Vec<GithubBillingFamilyView> {
    GithubBillingEndpointFamily::ALL
        .into_iter()
        .map(|endpoint_family| GithubBillingFamilyView {
            endpoint_family,
            latest: None,
            last_successful: None,
        })
        .collect()
}

pub fn fetch_personal_billing(
    token: &str,
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
) -> AppResult<Vec<GithubBillingSnapshot>> {
    validate_token(token)?;
    validate_identity(identity)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(REQUEST_TIMEOUT_SECONDS)))
        .http_status_as_error(false)
        .https_only(true)
        .max_redirects(0)
        .user_agent("PilotWeave/0.1")
        .build()
        .into();
    let authorization = format!("Bearer {token}");
    Ok(GithubBillingEndpointFamily::ALL
        .into_iter()
        .map(|family| fetch_family(&agent, &authorization, identity, period, family))
        .collect())
}

fn fetch_family(
    agent: &ureq::Agent,
    authorization: &str,
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
    family: GithubBillingEndpointFamily,
) -> GithubBillingSnapshot {
    let fetched_at = Utc::now();
    let url = match endpoint_url(identity, period, family) {
        Ok(url) => url,
        Err(_) => {
            return error_snapshot(
                identity,
                period,
                family,
                fetched_at,
                GithubBillingSnapshotStatus::SchemaError,
                GithubBillingCoverage::Unknown,
                "PilotWeave could not construct the fixed GitHub Billing endpoint",
            )
        }
    };
    let mut response = match agent
        .get(url.as_str())
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", authorization)
        .header("X-GitHub-Api-Version", GITHUB_BILLING_API_VERSION)
        .call()
    {
        Ok(response) => response,
        Err(_) => {
            return error_snapshot(
                identity,
                period,
                family,
                fetched_at,
                GithubBillingSnapshotStatus::NetworkError,
                GithubBillingCoverage::Unknown,
                "GitHub Billing could not be reached before the request timeout",
            )
        }
    };

    let status = response.status().as_u16();
    if status != 200 {
        return status_snapshot(identity, period, family, fetched_at, status, response.headers());
    }

    let bytes = match response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
    {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_snapshot(
                identity,
                period,
                family,
                fetched_at,
                GithubBillingSnapshotStatus::SchemaError,
                GithubBillingCoverage::Unknown,
                "GitHub Billing response exceeded the supported size or could not be read",
            )
        }
    };
    parse_report(&bytes, identity, period, family, fetched_at).unwrap_or_else(|_| {
        error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::SchemaError,
            GithubBillingCoverage::Unknown,
            "GitHub Billing response did not match the supported versioned schema",
        )
    })
}

fn status_snapshot(
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
    family: GithubBillingEndpointFamily,
    fetched_at: DateTime<Utc>,
    status: u16,
    headers: &ureq::http::HeaderMap,
) -> GithubBillingSnapshot {
    match status {
        401 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::Unauthorized,
            GithubBillingCoverage::Unknown,
            "GitHub rejected the stored PilotWeave authorization",
        ),
        403 if is_rate_limited(headers) => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::RateLimited,
            GithubBillingCoverage::Unknown,
            &rate_limit_detail(headers),
        ),
        403 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::InsufficientPermission,
            GithubBillingCoverage::Unknown,
            "The authorization identity is valid, but GitHub did not permit this personal Billing report; verify Plan user permission (read)",
        ),
        404 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::NotCovered,
            GithubBillingCoverage::NotCovered,
            "This personal endpoint does not expose usage for the account/plan; organization-paid usage is outside this view and is not zero",
        ),
        429 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::RateLimited,
            GithubBillingCoverage::Unknown,
            &rate_limit_detail(headers),
        ),
        400 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::SchemaError,
            GithubBillingCoverage::Unknown,
            "GitHub rejected the fixed Billing request parameters",
        ),
        500 | 502 | 503 | 504 => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::Unavailable,
            GithubBillingCoverage::Unknown,
            "GitHub Billing is temporarily unavailable",
        ),
        _ => error_snapshot(
            identity,
            period,
            family,
            fetched_at,
            GithubBillingSnapshotStatus::Unavailable,
            GithubBillingCoverage::Unknown,
            "GitHub Billing returned an unsupported HTTP status",
        ),
    }
}

fn parse_report(
    bytes: &[u8],
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
    family: GithubBillingEndpointFamily,
    fetched_at: DateTime<Utc>,
) -> AppResult<GithubBillingSnapshot> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ApiTimePeriod {
        year: i32,
        #[serde(default)]
        month: Option<u32>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ApiItem {
        product: String,
        sku: String,
        #[serde(default)]
        model: Option<String>,
        unit_type: String,
        price_per_unit: ApiDecimal,
        gross_quantity: ApiDecimal,
        gross_amount: ApiDecimal,
        discount_quantity: ApiDecimal,
        discount_amount: ApiDecimal,
        net_quantity: ApiDecimal,
        net_amount: ApiDecimal,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ApiReport {
        time_period: ApiTimePeriod,
        user: String,
        usage_items: Vec<ApiItem>,
    }

    let report: ApiReport = serde_json::from_slice(bytes).map_err(|_| {
        AppError::InvalidInput("Unsupported GitHub Billing response schema".to_string())
    })?;
    if !report.user.eq_ignore_ascii_case(&identity.login) {
        return Err(AppError::InvalidInput(
            "GitHub Billing response identity does not match the authorization".to_string(),
        ));
    }
    if report.time_period.year != period.year
        || report
            .time_period
            .month
            .is_some_and(|month| month != period.month)
    {
        return Err(AppError::InvalidInput(
            "GitHub Billing response period does not match the request".to_string(),
        ));
    }
    if report.usage_items.len() > MAX_BILLING_ITEMS_PER_SNAPSHOT {
        return Err(AppError::InvalidInput(format!(
            "GitHub Billing response contains more than {MAX_BILLING_ITEMS_PER_SNAPSHOT} items"
        )));
    }

    let mut items = Vec::with_capacity(report.usage_items.len());
    for item in report.usage_items {
        validate_api_text("Billing product", &item.product)?;
        validate_api_text("Billing SKU", &item.sku)?;
        validate_api_text("Billing unit", &item.unit_type)?;
        if let Some(model) = &item.model {
            validate_api_text("Billing model", model)?;
        }
        let _price_per_unit = item.price_per_unit.into_non_negative("price per unit")?;
        let quantity = item.gross_quantity.into_non_negative("gross quantity")?;
        let gross_amount = item.gross_amount.into_non_negative("gross amount")?;
        let _discount_quantity = item
            .discount_quantity
            .into_non_negative("discount quantity")?;
        let discount_amount = item
            .discount_amount
            .into_non_negative("discount amount")?;
        let _net_quantity = item.net_quantity.into_non_negative("net quantity")?;
        let net_amount = item.net_amount.into_non_negative("net amount")?;
        items.push(GithubBillingItem {
            id: Uuid::new_v4().to_string(),
            product: Some(item.product),
            sku: Some(item.sku),
            model: item.model,
            quantity,
            unit: item.unit_type,
            gross_amount_usd: Some(gross_amount),
            discount_amount_usd: Some(discount_amount),
            net_amount_usd: Some(net_amount),
            allowance: None,
            remaining: None,
            reset_at: None,
        });
    }

    let status = if items.is_empty() {
        GithubBillingSnapshotStatus::SuccessfulEmpty
    } else {
        GithubBillingSnapshotStatus::Available
    };
    Ok(GithubBillingSnapshot {
        id: Uuid::new_v4().to_string(),
        account_hint: identity.login.clone(),
        endpoint_family: family,
        api_version: GITHUB_BILLING_API_VERSION.to_string(),
        period_start: period.start,
        period_end: period.end,
        fetched_at,
        coverage: GithubBillingCoverage::PersonalAccountOnly,
        status,
        error: None,
        total_item_count: items.len() as u64,
        items,
        items_truncated: false,
    })
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct ApiDecimal(serde_json::Number);

impl ApiDecimal {
    fn into_non_negative(self, label: &str) -> AppResult<ExactDecimal> {
        let text = self.0.to_string();
        let value = ExactDecimal::parse(&text).ok_or_else(|| {
            AppError::InvalidInput(format!(
                "GitHub Billing {label} is not a supported exact decimal"
            ))
        })?;
        if value < ExactDecimal::ZERO {
            return Err(AppError::InvalidInput(format!(
                "GitHub Billing {label} must not be negative"
            )));
        }
        Ok(value)
    }
}

fn endpoint_url(
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
    family: GithubBillingEndpointFamily,
) -> AppResult<Url> {
    let mut url = Url::parse("https://api.github.com/")
        .map_err(|error| AppError::Config(format!("Invalid GitHub API base URL: {error}")))?;
    url.path_segments_mut()
        .map_err(|_| AppError::Config("GitHub API base URL cannot hold path segments".to_string()))?
        .extend([
            "users",
            identity.login.as_str(),
            "settings",
            "billing",
            family.path_segment(),
            "usage",
        ]);
    url.query_pairs_mut()
        .append_pair("year", &period.year.to_string())
        .append_pair("month", &period.month.to_string());
    Ok(url)
}

fn error_snapshot(
    identity: &GithubAuthorizationIdentity,
    period: GithubBillingPeriod,
    family: GithubBillingEndpointFamily,
    fetched_at: DateTime<Utc>,
    status: GithubBillingSnapshotStatus,
    coverage: GithubBillingCoverage,
    detail: &str,
) -> GithubBillingSnapshot {
    GithubBillingSnapshot {
        id: Uuid::new_v4().to_string(),
        account_hint: identity.login.clone(),
        endpoint_family: family,
        api_version: GITHUB_BILLING_API_VERSION.to_string(),
        period_start: period.start,
        period_end: period.end,
        fetched_at,
        coverage,
        status,
        error: Some(detail.to_string()),
        items: Vec::new(),
        total_item_count: 0,
        items_truncated: false,
    }
}

fn validate_identity(identity: &GithubAuthorizationIdentity) -> AppResult<()> {
    if identity.host != "github.com"
        || identity.login.is_empty()
        || identity.login.len() > 128
        || identity.login.starts_with('-')
        || identity.login.ends_with('-')
        || identity
            .login
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
    {
        return Err(AppError::InvalidInput(
            "Personal Billing requires a validated github.com identity".to_string(),
        ));
    }
    Ok(())
}

fn validate_token(token: &str) -> AppResult<()> {
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || !token.is_ascii()
        || token.trim() != token
        || token.chars().any(char::is_control)
    {
        return Err(AppError::InvalidInput(
            "Stored GitHub authorization is not a valid bounded token".to_string(),
        ));
    }
    Ok(())
}

fn validate_api_text(label: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(|character| character.is_control())
    {
        return Err(AppError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn is_rate_limited(headers: &ureq::http::HeaderMap) -> bool {
    retry_after_seconds(headers).is_some()
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == "0")
}

fn retry_after_seconds(headers: &ureq::http::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value <= 86_400)
}

fn rate_limit_detail(headers: &ureq::http::HeaderMap) -> String {
    match retry_after_seconds(headers) {
        Some(seconds) => format!(
            "GitHub rate-limited the Billing request; retry after approximately {seconds} seconds"
        ),
        None => "GitHub rate-limited the Billing request; retry after the reported limit resets"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> GithubAuthorizationIdentity {
        GithubAuthorizationIdentity {
            host: "github.com".to_string(),
            login: "octocat".to_string(),
            user_id: 42,
            avatar_url: None,
        }
    }

    fn current_period() -> GithubBillingPeriod {
        GithubBillingPeriod::current().expect("current period")
    }

    fn report_json(period: GithubBillingPeriod, user: &str, items: &str) -> Vec<u8> {
        format!(
            r#"{{"timePeriod":{{"year":{},"month":{}}},"user":"{}","usageItems":{}}}"#,
            period.year, period.month, user, items
        )
        .into_bytes()
    }

    #[test]
    fn exact_billing_decimals_are_preserved_without_binary_float_rounding() {
        let period = current_period();
        let bytes = report_json(
            period,
            "octocat",
            r#"[{"product":"Copilot","sku":"Copilot Premium Request","model":"GPT-5","unitType":"requests","pricePerUnit":0.040000001,"grossQuantity":100.125,"grossAmount":4.005000100125,"discountQuantity":0.125,"discountAmount":0.005,"netQuantity":100,"netAmount":4.000000100125}]"#,
        );
        let snapshot = parse_report(
            &bytes,
            &identity(),
            period,
            GithubBillingEndpointFamily::PremiumRequest,
            Utc::now(),
        )
        .expect("parse");
        assert_eq!(snapshot.status, GithubBillingSnapshotStatus::Available);
        assert_eq!(snapshot.items[0].quantity.to_string(), "100.125");
        assert_eq!(
            snapshot.items[0]
                .net_amount_usd
                .expect("net amount")
                .to_string(),
            "4.000000100125"
        );
    }

    #[test]
    fn empty_report_is_successful_empty_not_missing_or_zero_usage() {
        let period = current_period();
        let snapshot = parse_report(
            &report_json(period, "octocat", "[]"),
            &identity(),
            period,
            GithubBillingEndpointFamily::AiCredit,
            Utc::now(),
        )
        .expect("parse");
        assert_eq!(
            snapshot.status,
            GithubBillingSnapshotStatus::SuccessfulEmpty
        );
        assert_eq!(snapshot.coverage, GithubBillingCoverage::PersonalAccountOnly);
        assert!(snapshot.items.is_empty());
    }

    #[test]
    fn report_identity_and_period_must_match_the_authorization_and_request() {
        let period = current_period();
        assert!(parse_report(
            &report_json(period, "someone-else", "[]"),
            &identity(),
            period,
            GithubBillingEndpointFamily::AiCredit,
            Utc::now(),
        )
        .is_err());

        let wrong_year = format!(
            r#"{{"timePeriod":{{"year":{},"month":{}}},"user":"octocat","usageItems":[]}}"#,
            period.year - 1,
            period.month
        );
        assert!(parse_report(
            wrong_year.as_bytes(),
            &identity(),
            period,
            GithubBillingEndpointFamily::AiCredit,
            Utc::now(),
        )
        .is_err());
    }

    #[test]
    fn endpoint_families_are_fixed_and_never_combined() {
        let period = current_period();
        let credit = endpoint_url(&identity(), period, GithubBillingEndpointFamily::AiCredit)
            .expect("credit URL");
        let premium = endpoint_url(
            &identity(),
            period,
            GithubBillingEndpointFamily::PremiumRequest,
        )
        .expect("premium URL");
        assert!(credit.path().contains("/ai_credit/usage"));
        assert!(premium.path().contains("/premium_request/usage"));
        assert_ne!(credit.path(), premium.path());
    }

    #[test]
    fn periods_are_limited_to_the_documented_twenty_four_month_window() {
        let now = Utc::now();
        assert!(GithubBillingPeriod::new(now.year(), now.month()).is_ok());
        assert!(GithubBillingPeriod::new(now.year() + 1, now.month()).is_err());
        assert!(GithubBillingPeriod::new(now.year() - 3, now.month()).is_err());
        assert!(GithubBillingPeriod::new(now.year(), 13).is_err());
    }

    #[test]
    fn tokens_and_api_text_are_bounded() {
        assert!(validate_token("github_pat_test").is_ok());
        assert!(validate_token(" github_pat_test").is_err());
        assert!(validate_token("github_pat_令牌").is_err());
        assert!(validate_api_text("unit", "requests").is_ok());
        assert!(validate_api_text("unit", "requests\nsecret").is_err());
    }
}
