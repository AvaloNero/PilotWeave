use crate::decimal::ExactDecimal;
use crate::usage_db::InputSemantics;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DataStatus {
    Available,
    SuccessfulEmpty,
    Disabled,
    Missing,
    Unsupported,
    SchemaError,
    Unauthorized,
    Forbidden,
    NetworkError,
    Stale,
    Partial,
    Canceled,
    Interrupted,
    InProgress,
    #[default]
    Unavailable,
}

impl DataStatus {
    pub fn successful(self) -> bool {
        matches!(self, Self::Available | Self::SuccessfulEmpty)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    OfficialGithub,
    Byok,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    Explicit,
    ExactConnectionIdentity,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CounterKind {
    SessionCumulative,
    RequestDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub id: String,
    pub source_id: String,
    pub session_hash: Option<String>,
    pub request_hash: Option<String>,
    pub raw_model: String,
    pub canonical_model: Option<String>,
    pub route: Route,
    pub confidence: Confidence,
    pub connection_id: Option<String>,
    pub provider: Option<String>,
    pub surfaces: Vec<String>,
    pub counter_kind: CounterKind,
    #[serde(with = "counter")]
    pub request_count: Option<u64>,
    pub input_semantics: InputSemantics,
    #[serde(with = "counter")]
    pub input_reported: Option<u64>,
    #[serde(with = "counter")]
    pub normalized_input: Option<u64>,
    #[serde(with = "counter")]
    pub fresh_input: Option<u64>,
    #[serde(with = "counter")]
    pub cache_read: Option<u64>,
    #[serde(with = "counter")]
    pub cache_write: Option<u64>,
    #[serde(with = "counter")]
    pub output: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub imported_at: DateTime<Utc>,
    pub quality: Vec<String>,
    pub price_snapshot_id: Option<String>,
    pub estimate_usd: Option<ExactDecimal>,
    pub estimate_detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub status: DataStatus,
    pub detail: String,
    pub parser: String,
    pub path_hint: String,
    pub setup_path: Option<String>,
    pub last_scan_at: Option<String>,
    pub last_success_at: Option<String>,
    pub record_count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParserState {
    pub session_hash: Option<String>,
    pub schema_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCursor {
    pub offset: u64,
    pub prefix_hash: String,
    pub file_identity: String,
    pub file_size: u64,
    pub modified: String,
    pub parser_version: u32,
    pub state: ParserState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRun {
    pub id: String,
    pub source_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: DataStatus,
    pub files_seen: usize,
    pub records_seen: usize,
    pub bytes_read: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageQuery {
    pub start: String,
    pub end: String,
    pub surface: Option<String>,
    pub route: Option<Route>,
    pub connection_id: Option<String>,
    pub model: Option<String>,
    pub confidence: Option<Confidence>,
    pub source_id: Option<String>,
    pub source_status: Option<DataStatus>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub records: usize,
    #[serde(serialize_with = "counter::serialize")]
    pub requests: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub input_reported: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub normalized_input: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub fresh_input: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub cache_read: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub cache_write: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub output: Option<u64>,
    pub cache_hit_rate: Option<ExactDecimal>,
    pub cache_covered_records: usize,
    pub estimate_usd: Option<ExactDecimal>,
    pub priced_records: usize,
    pub unpriced_records: usize,
    #[serde(serialize_with = "counter::serialize")]
    pub priced_requests: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub unpriced_requests: Option<u64>,
    pub unresolved_models: usize,
    pub unknown_semantics: usize,
    #[serde(serialize_with = "counter::serialize")]
    pub priced_tokens: Option<u64>,
    #[serde(serialize_with = "counter::serialize")]
    pub unpriced_tokens: Option<u64>,
    pub price_snapshots: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    pub key: String,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatistics {
    pub raw_model: String,
    pub canonical_model: Option<String>,
    pub route: Route,
    pub confidence: Confidence,
    pub surfaces: Vec<String>,
    pub totals: Totals,
    pub quality: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOverview {
    pub status: DataStatus,
    pub detail: String,
    pub start: String,
    pub end: String,
    pub totals: Totals,
    pub models: Vec<ModelStatistics>,
    pub days: Vec<Breakdown>,
    pub routes: Vec<Breakdown>,
    pub surfaces: Vec<Breakdown>,
    pub providers: Vec<Breakdown>,
    pub records: Vec<Observation>,
    pub total_records: usize,
    pub coverage_start: Option<DateTime<Utc>>,
    pub coverage_end: Option<DateTime<Utc>>,
    pub last_imported: Option<DateTime<Utc>>,
}

// Keep integer counters exact across the JavaScript IPC boundary. Older
// persisted numeric counters remain readable; new metadata uses decimal strings.
mod counter {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        value.map(|v| v.to_string()).serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u64),
            Text(String),
        }
        match Option::<Raw>::deserialize(deserializer)? {
            None => Ok(None),
            Some(Raw::Number(v)) => Ok(Some(v)),
            Some(Raw::Text(v)) => v.parse().map(Some).map_err(serde::de::Error::custom),
        }
    }
}
