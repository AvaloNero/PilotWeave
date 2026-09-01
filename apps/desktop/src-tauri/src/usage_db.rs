//! Migration-safe `usage.sqlite3` foundation (spec section 15.2).
//!
//! The database stores allowlisted usage metadata only: never raw log lines,
//! prompts, responses, headers, cookies, or credentials. Migrations are
//! transactional, foreign keys are enforced, source records upsert by stable
//! unique keys in the same transaction as cursor advancement, and a failed or
//! newer-than-supported schema opens nothing instead of recreating data.

use crate::decimal::ExactDecimal;
use crate::domain::{UsageDbState, UsageDbStatus};
use crate::error::{AppError, AppResult};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection as SqliteConnection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const USAGE_SCHEMA_VERSION: u32 = 1;
/// Upper bound for one sync transaction (spec section 12.3).
pub const MAX_BATCH_RECORDS: usize = 500;
/// Bound for externally derived short text fields.
const MAX_TEXT_FIELD: usize = 512;

const MIGRATION_V1: &str = "
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE usage_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    parser_kind TEXT NOT NULL,
    parser_version INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    logical_surfaces TEXT NOT NULL DEFAULT '[]',
    path_hint TEXT,
    status TEXT NOT NULL DEFAULT 'available',
    last_scan_at TEXT,
    last_success_at TEXT,
    coverage_start TEXT,
    coverage_end TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE usage_sync_runs (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES usage_sources(id),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    status TEXT NOT NULL,
    detail TEXT,
    records_seen INTEGER NOT NULL DEFAULT 0,
    bytes_read INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE usage_source_cursors (
    source_id TEXT PRIMARY KEY REFERENCES usage_sources(id),
    file_identity TEXT,
    size_bytes INTEGER,
    mtime_utc TEXT,
    cursor_offset INTEGER NOT NULL DEFAULT 0,
    parser_version INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE price_snapshots (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    source_version TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    effective_at TEXT,
    currency TEXT NOT NULL DEFAULT 'USD',
    status TEXT NOT NULL DEFAULT 'active',
    provenance TEXT NOT NULL,
    parser_version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE price_rows (
    id TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL REFERENCES price_snapshots(id),
    provider TEXT NOT NULL,
    canonical_model_id TEXT NOT NULL,
    tier TEXT NOT NULL DEFAULT 'standard',
    context_threshold INTEGER,
    input_rate_per_million TEXT NOT NULL,
    cache_read_rate_per_million TEXT,
    cache_write_rate_per_million TEXT,
    output_rate_per_million TEXT NOT NULL,
    UNIQUE (snapshot_id, provider, canonical_model_id, tier)
);

CREATE TABLE usage_records (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES usage_sources(id),
    source_record_key TEXT NOT NULL,
    session_hash TEXT,
    request_hash TEXT,
    raw_model TEXT NOT NULL,
    canonical_model_id TEXT,
    route TEXT NOT NULL DEFAULT 'unknown',
    attribution_confidence TEXT NOT NULL DEFAULT 'unknown',
    input_semantics TEXT NOT NULL DEFAULT 'unknown',
    input_reported INTEGER,
    fresh_input INTEGER,
    cache_read INTEGER,
    cache_write INTEGER,
    output INTEGER,
    started_at TEXT,
    finished_at TEXT,
    source_created_at TEXT,
    imported_at TEXT NOT NULL,
    price_snapshot_id TEXT REFERENCES price_snapshots(id),
    estimate_usd TEXT,
    estimate_status TEXT NOT NULL DEFAULT 'unpriced',
    quality_flags TEXT NOT NULL DEFAULT '[]',
    UNIQUE (source_id, source_record_key)
);

CREATE TABLE usage_record_surfaces (
    record_id TEXT NOT NULL REFERENCES usage_records(id) ON DELETE CASCADE,
    surface TEXT NOT NULL,
    PRIMARY KEY (record_id, surface)
);

CREATE TABLE official_quota_snapshots (
    id TEXT PRIMARY KEY,
    account_hint TEXT,
    runtime_version TEXT,
    parser_version INTEGER NOT NULL,
    fetched_at TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT
);

CREATE TABLE official_quota_items (
    snapshot_id TEXT NOT NULL REFERENCES official_quota_snapshots(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    item_value TEXT,
    unit TEXT,
    PRIMARY KEY (snapshot_id, item_key)
);

CREATE TABLE github_billing_snapshots (
    id TEXT PRIMARY KEY,
    account_hint TEXT,
    endpoint_family TEXT NOT NULL,
    api_version TEXT NOT NULL,
    period_start TEXT,
    period_end TEXT,
    fetched_at TEXT NOT NULL,
    coverage TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT
);

CREATE TABLE github_billing_items (
    id TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL REFERENCES github_billing_snapshots(id) ON DELETE CASCADE,
    product TEXT,
    sku TEXT,
    model TEXT,
    quantity TEXT NOT NULL,
    unit TEXT NOT NULL,
    gross_amount_usd TEXT,
    discount_amount_usd TEXT,
    net_amount_usd TEXT,
    allowance TEXT,
    remaining TEXT,
    reset_at TEXT
);

CREATE TABLE model_aliases (
    id TEXT PRIMARY KEY,
    source_scope TEXT NOT NULL,
    raw_value TEXT NOT NULL,
    canonical_provider TEXT,
    canonical_model_id TEXT,
    status TEXT NOT NULL,
    confidence TEXT NOT NULL DEFAULT 'unknown',
    effective_from TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    UNIQUE (source_scope, raw_value, version)
);
";

/// Token-count semantics reported by a source (spec section 12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum InputSemantics {
    FreshOnly,
    TotalIncludesCacheRead,
    TotalIncludesCacheReadAndWrite,
    SeparateBucketsWithNoTotal,
    #[default]
    Unknown,
}

impl InputSemantics {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreshOnly => "fresh-only",
            Self::TotalIncludesCacheRead => "total-includes-cache-read",
            Self::TotalIncludesCacheReadAndWrite => "total-includes-cache-read-and-write",
            Self::SeparateBucketsWithNoTotal => "separate-buckets-with-no-total",
            Self::Unknown => "unknown",
        }
    }
}

/// Physical origin of imported usage metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageSourceKind {
    CopilotCli,
    VsCode,
    GithubCopilotApp,
}

impl UsageSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CopilotCli => "copilot-cli",
            Self::VsCode => "vscode",
            Self::GithubCopilotApp => "github-copilot-app",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewUsageSource {
    pub id: String,
    pub kind: UsageSourceKind,
    pub parser_kind: String,
    pub parser_version: u32,
    pub enabled: bool,
    pub logical_surfaces: Vec<String>,
    pub path_hint: Option<String>,
}

/// Incremental parse position plus file identity evidence (spec 12.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCursor {
    pub file_identity: Option<String>,
    pub size_bytes: Option<u64>,
    pub mtime_utc: Option<DateTime<Utc>>,
    pub offset: u64,
    pub parser_version: u32,
}

/// Allowlisted metadata for one upserted usage record. Raw content is never
/// accepted here; counts stay `Option` so unknown is never stored as zero.
#[derive(Debug, Clone)]
pub struct UsageRecordUpsert {
    pub source_record_key: String,
    pub raw_model: String,
    pub session_hash: Option<String>,
    pub request_hash: Option<String>,
    pub input_semantics: InputSemantics,
    pub input_reported: Option<u64>,
    pub fresh_input: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub output: Option<u64>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub source_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewPriceSnapshot {
    pub id: String,
    pub source: String,
    pub source_version: String,
    pub fetched_at: DateTime<Utc>,
    pub effective_at: Option<DateTime<Utc>>,
    pub currency: String,
    pub provenance: String,
    pub parser_version: u32,
}

#[derive(Debug, Clone)]
pub struct NewPriceRow {
    pub id: String,
    pub snapshot_id: String,
    pub provider: String,
    pub canonical_model_id: String,
    pub tier: String,
    pub context_threshold: Option<u64>,
    pub input_rate_per_million: ExactDecimal,
    /// `None` means an explicit NotApplicable rate, never "unknown zero".
    pub cache_read_rate_per_million: Option<ExactDecimal>,
    pub cache_write_rate_per_million: Option<ExactDecimal>,
    pub output_rate_per_million: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceRow {
    pub input_rate_per_million: ExactDecimal,
    pub cache_read_rate_per_million: Option<ExactDecimal>,
    pub cache_write_rate_per_million: Option<ExactDecimal>,
    pub output_rate_per_million: ExactDecimal,
}

#[derive(Debug)]
pub struct UsageDb {
    conn: SqliteConnection,
    path: PathBuf,
}

impl UsageDb {
    pub fn open() -> AppResult<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| AppError::Config("Cannot resolve the user config directory".into()))?;
        Self::open_at(&config_dir.join("PilotWeave").join("usage.sqlite3"))
    }

    pub fn open_at(path: &Path) -> AppResult<Self> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(AppError::InvalidInput(format!(
                    "Usage database must be a regular file: {}",
                    path.display()
                )));
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
        }
        let conn = SqliteConnection::open(path).map_err(|error| {
            AppError::Config(format!(
                "Failed to open usage database {}: {error}",
                path.display()
            ))
        })?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sqlite_error)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(sqlite_error)?;
        // Force a schema read so a corrupt file fails here instead of on first use.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|error| {
                AppError::Config(format!(
                    "Usage database {} is not readable: {error}",
                    path.display()
                ))
            })?;

        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> AppResult<()> {
        let version: u32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if version > USAGE_SCHEMA_VERSION {
            return Err(AppError::Config(format!(
                "Usage database schema v{version} is newer than this build supports (v{USAGE_SCHEMA_VERSION}); leaving it untouched"
            )));
        }
        if version == USAGE_SCHEMA_VERSION {
            return Ok(());
        }

        let tx = self.conn.transaction().map_err(sqlite_error)?;
        tx.execute_batch(MIGRATION_V1).map_err(|error| {
            AppError::Config(format!("Usage database migration to v1 failed: {error}"))
        })?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![USAGE_SCHEMA_VERSION, Utc::now().to_rfc3339()],
        )
        .map_err(sqlite_error)?;
        tx.pragma_update(None, "user_version", USAGE_SCHEMA_VERSION)
            .map_err(sqlite_error)?;
        tx.commit().map_err(sqlite_error)?;
        Ok(())
    }

    pub fn status(&self) -> UsageDbStatus {
        UsageDbStatus {
            state: UsageDbState::Available,
            detail: format!("Usage database ready (schema v{USAGE_SCHEMA_VERSION})"),
            path: Some(self.path.to_string_lossy().to_string()),
            schema_version: Some(USAGE_SCHEMA_VERSION),
        }
    }

    pub fn register_source(&mut self, source: &NewUsageSource) -> AppResult<()> {
        check_text_field("source id", &source.id)?;
        check_text_field("parser kind", &source.parser_kind)?;
        let now = Utc::now().to_rfc3339();
        let surfaces = serde_json::to_string(&source.logical_surfaces)
            .map_err(|error| AppError::Config(format!("Failed to encode surfaces: {error}")))?;
        self.conn
            .execute(
                "INSERT INTO usage_sources
                    (id, kind, parser_kind, parser_version, enabled, logical_surfaces, path_hint, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (id) DO UPDATE SET
                    parser_kind = excluded.parser_kind,
                    parser_version = excluded.parser_version,
                    enabled = excluded.enabled,
                    logical_surfaces = excluded.logical_surfaces,
                    path_hint = excluded.path_hint,
                    updated_at = excluded.updated_at",
                params![
                    source.id,
                    source.kind.as_str(),
                    source.parser_kind,
                    source.parser_version,
                    source.enabled,
                    surfaces,
                    source.path_hint,
                    now,
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    /// Upsert a bounded batch of records and advance the source cursor in one
    /// transaction, so repeated syncs never duplicate totals (spec 12.4).
    pub fn upsert_records_with_cursor(
        &mut self,
        source_id: &str,
        cursor: &SourceCursor,
        records: &[UsageRecordUpsert],
    ) -> AppResult<usize> {
        if records.len() > MAX_BATCH_RECORDS {
            return Err(AppError::InvalidInput(format!(
                "A sync batch may contain at most {MAX_BATCH_RECORDS} records"
            )));
        }
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.transaction().map_err(sqlite_error)?;
        let mut upserted = 0usize;
        for record in records {
            check_text_field("source record key", &record.source_record_key)?;
            check_text_field("raw model", &record.raw_model)?;
            upserted += tx
                .execute(
                    "INSERT INTO usage_records
                        (id, source_id, source_record_key, session_hash, request_hash,
                         raw_model, input_semantics, input_reported, fresh_input,
                         cache_read, cache_write, output,
                         started_at, finished_at, source_created_at, imported_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                     ON CONFLICT (source_id, source_record_key) DO UPDATE SET
                        session_hash = excluded.session_hash,
                        request_hash = excluded.request_hash,
                        raw_model = excluded.raw_model,
                        input_semantics = excluded.input_semantics,
                        input_reported = excluded.input_reported,
                        fresh_input = excluded.fresh_input,
                        cache_read = excluded.cache_read,
                        cache_write = excluded.cache_write,
                        output = excluded.output,
                        started_at = excluded.started_at,
                        finished_at = excluded.finished_at,
                        source_created_at = excluded.source_created_at,
                        imported_at = excluded.imported_at",
                    params![
                        Uuid::new_v4().to_string(),
                        source_id,
                        record.source_record_key,
                        record.session_hash,
                        record.request_hash,
                        record.raw_model,
                        record.input_semantics.as_str(),
                        record
                            .input_reported
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                AppError::InvalidInput("Token count exceeds storage range".into())
                            })?,
                        record
                            .fresh_input
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                AppError::InvalidInput("Token count exceeds storage range".into())
                            })?,
                        record
                            .cache_read
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                AppError::InvalidInput("Token count exceeds storage range".into())
                            })?,
                        record
                            .cache_write
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                AppError::InvalidInput("Token count exceeds storage range".into())
                            })?,
                        record.output.map(i64::try_from).transpose().map_err(|_| {
                            AppError::InvalidInput("Token count exceeds storage range".into())
                        })?,
                        record.started_at.map(|value| value.to_rfc3339()),
                        record.finished_at.map(|value| value.to_rfc3339()),
                        record.source_created_at.map(|value| value.to_rfc3339()),
                        now,
                    ],
                )
                .map_err(sqlite_error)?;
        }
        tx.execute(
            "INSERT INTO usage_source_cursors
                (source_id, file_identity, size_bytes, mtime_utc, cursor_offset, parser_version, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (source_id) DO UPDATE SET
                file_identity = excluded.file_identity,
                size_bytes = excluded.size_bytes,
                mtime_utc = excluded.mtime_utc,
                cursor_offset = excluded.cursor_offset,
                parser_version = excluded.parser_version,
                updated_at = excluded.updated_at",
            params![
                source_id,
                cursor.file_identity,
                cursor
                    .size_bytes
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| AppError::InvalidInput("File size exceeds storage range".into()))?,
                cursor.mtime_utc.map(|value| value.to_rfc3339()),
                i64::try_from(cursor.offset).map_err(|_| {
                    AppError::InvalidInput("Cursor offset exceeds storage range".into())
                })?,
                cursor.parser_version,
                now,
            ],
        )
        .map_err(sqlite_error)?;
        tx.commit().map_err(sqlite_error)?;
        Ok(upserted)
    }

    pub fn record_count(&self, source_id: &str) -> AppResult<u64> {
        self.conn
            .query_row(
                "SELECT count(*) FROM usage_records WHERE source_id = ?1",
                params![source_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as u64)
            .map_err(sqlite_error)
    }

    pub fn cursor(&self, source_id: &str) -> AppResult<Option<SourceCursor>> {
        let row = match self.conn.query_row(
            "SELECT file_identity, size_bytes, mtime_utc, cursor_offset, parser_version
                 FROM usage_source_cursors WHERE source_id = ?1",
            params![source_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, u32>(4)?,
                ))
            },
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(sqlite_error(error)),
        };
        let mtime_utc = row
            .2
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&Utc));
        Ok(Some(SourceCursor {
            file_identity: row.0,
            size_bytes: row.1.map(|value| value.max(0) as u64),
            mtime_utc,
            offset: row.3.max(0) as u64,
            parser_version: row.4,
        }))
    }

    /// Insert an immutable price snapshot; existing IDs are rejected because
    /// snapshots must never be mutated (spec 14.3).
    pub fn insert_price_snapshot(&mut self, snapshot: &NewPriceSnapshot) -> AppResult<()> {
        check_text_field("price snapshot id", &snapshot.id)?;
        self.conn
            .execute(
                "INSERT INTO price_snapshots
                    (id, source, source_version, fetched_at, effective_at, currency, provenance, parser_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    snapshot.id,
                    snapshot.source,
                    snapshot.source_version,
                    snapshot.fetched_at.to_rfc3339(),
                    snapshot.effective_at.map(|value| value.to_rfc3339()),
                    snapshot.currency,
                    snapshot.provenance,
                    snapshot.parser_version,
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    pub fn insert_price_row(&mut self, row: &NewPriceRow) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO price_rows
                    (id, snapshot_id, provider, canonical_model_id, tier, context_threshold,
                     input_rate_per_million, cache_read_rate_per_million,
                     cache_write_rate_per_million, output_rate_per_million)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    row.id,
                    row.snapshot_id,
                    row.provider,
                    row.canonical_model_id,
                    row.tier,
                    row.context_threshold
                        .map(i64::try_from)
                        .transpose()
                        .map_err(|_| AppError::InvalidInput(
                            "Threshold exceeds storage range".into()
                        ))?,
                    row.input_rate_per_million.to_string(),
                    row.cache_read_rate_per_million.map(|rate| rate.to_string()),
                    row.cache_write_rate_per_million
                        .map(|rate| rate.to_string()),
                    row.output_rate_per_million.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    pub fn price_row(
        &self,
        snapshot_id: &str,
        provider: &str,
        canonical_model_id: &str,
        tier: &str,
    ) -> AppResult<Option<PriceRow>> {
        let row = match self.conn.query_row(
            "SELECT input_rate_per_million, cache_read_rate_per_million,
                        cache_write_rate_per_million, output_rate_per_million
                 FROM price_rows
                 WHERE snapshot_id = ?1 AND provider = ?2 AND canonical_model_id = ?3 AND tier = ?4",
            params![snapshot_id, provider, canonical_model_id, tier],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(sqlite_error(error)),
        };
        let parse = |text: &str| {
            ExactDecimal::parse(text)
                .ok_or_else(|| AppError::Config(format!("Invalid stored rate: {text}")))
        };
        Ok(Some(PriceRow {
            input_rate_per_million: parse(&row.0)?,
            cache_read_rate_per_million: row.1.map(|text| parse(&text)).transpose()?,
            cache_write_rate_per_million: row.2.map(|text| parse(&text)).transpose()?,
            output_rate_per_million: parse(&row.3)?,
        }))
    }
}

fn check_text_field(name: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::InvalidInput(format!("{name} must not be empty")));
    }
    if value.len() > MAX_TEXT_FIELD {
        return Err(AppError::InvalidInput(format!(
            "{name} exceeds {MAX_TEXT_FIELD} characters"
        )));
    }
    Ok(())
}

fn sqlite_error(error: rusqlite::Error) -> AppError {
    AppError::Config(format!("Usage database error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> NewUsageSource {
        NewUsageSource {
            id: id.to_string(),
            kind: UsageSourceKind::CopilotCli,
            parser_kind: "copilot-cli-jsonl".to_string(),
            parser_version: 1,
            enabled: true,
            logical_surfaces: vec!["copilot-cli".to_string()],
            path_hint: None,
        }
    }

    fn record(key: &str, output: u64) -> UsageRecordUpsert {
        UsageRecordUpsert {
            source_record_key: key.to_string(),
            raw_model: "gpt-example".to_string(),
            session_hash: Some("session:abc".to_string()),
            request_hash: None,
            input_semantics: InputSemantics::FreshOnly,
            input_reported: Some(10),
            fresh_input: Some(10),
            cache_read: None,
            cache_write: None,
            output: Some(output),
            started_at: None,
            finished_at: None,
            source_created_at: None,
        }
    }

    fn cursor(offset: u64) -> SourceCursor {
        SourceCursor {
            file_identity: Some("file-identity".to_string()),
            size_bytes: Some(1_024),
            mtime_utc: Some(Utc::now()),
            offset,
            parser_version: 1,
        }
    }

    #[test]
    fn fresh_database_migrates_and_reopens() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite3");
        {
            let db = UsageDb::open_at(&path).expect("open fresh");
            assert_eq!(db.status().schema_version, Some(USAGE_SCHEMA_VERSION));
        }
        // Reopening an up-to-date database is a no-op, not a re-migration.
        let db = UsageDb::open_at(&path).expect("reopen");
        assert_eq!(db.status().state, UsageDbState::Available);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let error = db
            .upsert_records_with_cursor("missing-source", &cursor(0), &[record("r1", 1)])
            .expect_err("orphan records must fail");
        assert!(error.to_string().contains("FOREIGN KEY"));
    }

    #[test]
    fn repeated_sync_is_idempotent_and_advances_one_cursor() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        db.register_source(&source("cli")).expect("register source");

        let batch = vec![record("r1", 100), record("r2", 200)];
        db.upsert_records_with_cursor("cli", &cursor(40), &batch)
            .expect("first sync");
        assert_eq!(db.record_count("cli").expect("count"), 2);

        // A repeated scan of the same range changes nothing.
        db.upsert_records_with_cursor("cli", &cursor(40), &batch)
            .expect("second sync");
        assert_eq!(db.record_count("cli").expect("count"), 2);

        // A later cumulative snapshot replaces values for the same key.
        db.upsert_records_with_cursor("cli", &cursor(80), &[record("r1", 150)])
            .expect("third sync");
        assert_eq!(db.record_count("cli").expect("count"), 2);
        let stored = db.cursor("cli").expect("cursor").expect("cursor present");
        assert_eq!(stored.offset, 80);
        assert_eq!(stored.size_bytes, Some(1_024));
    }

    #[test]
    fn batches_are_bounded() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        db.register_source(&source("cli")).expect("register source");
        let batch = vec![record("r", 1); MAX_BATCH_RECORDS + 1];
        assert!(db
            .upsert_records_with_cursor("cli", &cursor(0), &batch)
            .is_err());
    }

    #[test]
    fn corrupt_database_is_reported_not_recreated() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite3");
        fs::write(&path, b"this is not a sqlite database").expect("write garbage");
        let before = fs::read(&path).expect("read garbage");

        assert!(UsageDb::open_at(&path).is_err());
        let after = fs::read(&path).expect("read after failed open");
        assert_eq!(before, after, "a corrupt database must never be recreated");
    }

    #[test]
    fn newer_schema_is_left_untouched() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("usage.sqlite3");
        {
            let db = UsageDb::open_at(&path).expect("open fresh");
            db.conn
                .pragma_update(None, "user_version", USAGE_SCHEMA_VERSION + 1)
                .expect("bump version");
        }
        let error = UsageDb::open_at(&path).expect_err("newer schema must fail");
        assert!(error.to_string().contains("newer"));
    }

    #[test]
    fn symlinked_database_is_refused() {
        let directory = tempfile::tempdir().expect("temp directory");
        let real = directory.path().join("real.sqlite3");
        fs::write(&real, b"").expect("real file");
        let link = directory.path().join("usage.sqlite3");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(windows)]
        {
            // Windows symlink creation needs privileges; emulate with a directory.
            fs::create_dir(&link).expect("directory placeholder");
        }
        assert!(UsageDb::open_at(&link).is_err());
    }

    #[test]
    fn price_rates_round_trip_exactly() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        db.insert_price_snapshot(&NewPriceSnapshot {
            id: "prices-2026-09".to_string(),
            source: "official-docs".to_string(),
            source_version: "2026.09".to_string(),
            fetched_at: Utc::now(),
            effective_at: None,
            currency: "USD".to_string(),
            provenance: "unit test fixture".to_string(),
            parser_version: 1,
        })
        .expect("snapshot");
        db.insert_price_row(&NewPriceRow {
            id: "row-1".to_string(),
            snapshot_id: "prices-2026-09".to_string(),
            provider: "openai".to_string(),
            canonical_model_id: "gpt-example".to_string(),
            tier: "standard".to_string(),
            context_threshold: None,
            input_rate_per_million: ExactDecimal::parse("1.25").expect("rate"),
            cache_read_rate_per_million: Some(ExactDecimal::parse("0.125").expect("rate")),
            cache_write_rate_per_million: None,
            output_rate_per_million: ExactDecimal::parse("10.00").expect("rate"),
        })
        .expect("row");

        let row = db
            .price_row("prices-2026-09", "openai", "gpt-example", "standard")
            .expect("query")
            .expect("row present");
        assert_eq!(row.input_rate_per_million.to_string(), "1.25");
        assert_eq!(
            row.cache_read_rate_per_million.map(|rate| rate.to_string()),
            Some("0.125".to_string())
        );
        assert_eq!(row.cache_write_rate_per_million, None);
        assert_eq!(row.output_rate_per_million.to_string(), "10.00");

        // Price snapshots are immutable.
        assert!(db
            .insert_price_snapshot(&NewPriceSnapshot {
                id: "prices-2026-09".to_string(),
                source: "official-docs".to_string(),
                source_version: "2026.09".to_string(),
                fetched_at: Utc::now(),
                effective_at: None,
                currency: "USD".to_string(),
                provenance: "duplicate".to_string(),
                parser_version: 1,
            })
            .is_err());
    }

    #[test]
    fn price_rows_require_an_existing_snapshot() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        assert!(db
            .insert_price_row(&NewPriceRow {
                id: "row-1".to_string(),
                snapshot_id: "missing".to_string(),
                provider: "openai".to_string(),
                canonical_model_id: "gpt-example".to_string(),
                tier: "standard".to_string(),
                context_threshold: None,
                input_rate_per_million: ExactDecimal::ZERO,
                cache_read_rate_per_million: None,
                cache_write_rate_per_million: None,
                output_rate_per_million: ExactDecimal::ZERO,
            })
            .is_err());
    }
}
