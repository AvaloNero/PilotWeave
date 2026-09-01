//! Integration coverage for the `usage.sqlite3` foundation (spec 15.2/20.2):
//! migration and reopen behavior, foreign keys, unique source records,
//! transactional cursor advancement, exact decimal round trips, and error
//! modes that never recreate data. All files live in temporary directories.

use chrono::Utc;
use pilotweave_lib::decimal::ExactDecimal;
use pilotweave_lib::usage_db::{
    InputSemantics, NewPriceRow, NewPriceSnapshot, NewUsageSource, SourceCursor, UsageDb,
    UsageRecordUpsert, UsageSourceKind, MAX_BATCH_RECORDS, USAGE_SCHEMA_VERSION,
};

fn source(id: &str) -> NewUsageSource {
    NewUsageSource {
        id: id.to_string(),
        kind: UsageSourceKind::CopilotCli,
        parser_kind: "copilot-cli-jsonl".to_string(),
        parser_version: 1,
        enabled: true,
        logical_surfaces: vec!["copilot-cli".to_string(), "github-copilot-app".to_string()],
        path_hint: None,
    }
}

fn record(key: &str, output: u64) -> UsageRecordUpsert {
    UsageRecordUpsert {
        source_record_key: key.to_string(),
        raw_model: "claude-example".to_string(),
        session_hash: Some("session:shared".to_string()),
        request_hash: None,
        input_semantics: InputSemantics::TotalIncludesCacheReadAndWrite,
        input_reported: Some(1_000),
        fresh_input: Some(600),
        cache_read: Some(300),
        cache_write: Some(100),
        output: Some(output),
        started_at: Some(Utc::now()),
        finished_at: None,
        source_created_at: Some(Utc::now()),
    }
}

fn cursor(offset: u64) -> SourceCursor {
    SourceCursor {
        file_identity: Some("inode-or-hash".to_string()),
        size_bytes: Some(4_096),
        mtime_utc: Some(Utc::now()),
        offset,
        parser_version: 1,
    }
}

#[test]
fn full_sync_cycle_survives_reopen_and_stays_idempotent() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("usage.sqlite3");

    {
        let mut db = UsageDb::open_at(&path).expect("open fresh");
        assert_eq!(db.status().schema_version, Some(USAGE_SCHEMA_VERSION));
        db.register_source(&source("cli-runtime")).expect("source");

        let batch = vec![record("req-1", 42), record("req-2", 7)];
        assert_eq!(
            db.upsert_records_with_cursor("cli-runtime", &cursor(128), &batch)
                .expect("first sync"),
            2
        );
        // Repeating the same scan neither duplicates records nor loses the cursor.
        db.upsert_records_with_cursor("cli-runtime", &cursor(128), &batch)
            .expect("repeat sync");
        assert_eq!(db.record_count("cli-runtime").expect("count"), 2);
    }

    {
        let mut db = UsageDb::open_at(&path).expect("reopen");
        assert_eq!(db.record_count("cli-runtime").expect("count"), 2);
        let stored = db
            .cursor("cli-runtime")
            .expect("cursor query")
            .expect("cursor present");
        assert_eq!(stored.offset, 128);

        // A cumulative session snapshot replaces the prior values by key.
        db.upsert_records_with_cursor("cli-runtime", &cursor(256), &[record("req-1", 50)])
            .expect("cumulative upsert");
        assert_eq!(db.record_count("cli-runtime").expect("count"), 2);
        assert_eq!(
            db.cursor("cli-runtime")
                .expect("cursor query")
                .expect("cursor present")
                .offset,
            256
        );
    }
}

#[test]
fn batch_bounds_and_foreign_keys_are_enforced_together() {
    let directory = tempfile::tempdir().expect("temp directory");
    let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");

    // Foreign key: records need a registered source.
    assert!(db
        .upsert_records_with_cursor("ghost", &cursor(0), &[record("r", 1)])
        .is_err());

    db.register_source(&source("cli")).expect("source");
    let oversized = vec![record("r", 1); MAX_BATCH_RECORDS + 1];
    assert!(db
        .upsert_records_with_cursor("cli", &cursor(0), &oversized)
        .is_err());
    assert_eq!(db.record_count("cli").expect("count"), 0);
}

#[test]
fn price_snapshots_are_immutable_and_rates_stay_exact() {
    let directory = tempfile::tempdir().expect("temp directory");
    let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");

    let snapshot = NewPriceSnapshot {
        id: "snap-1".to_string(),
        source: "provider-docs".to_string(),
        source_version: "2026-09".to_string(),
        fetched_at: Utc::now(),
        effective_at: None,
        currency: "USD".to_string(),
        provenance: "integration fixture".to_string(),
        parser_version: 1,
    };
    db.insert_price_snapshot(&snapshot).expect("snapshot");
    // Immutable: the same snapshot ID cannot be inserted twice.
    assert!(db.insert_price_snapshot(&snapshot).is_err());

    db.insert_price_row(&NewPriceRow {
        id: "row-1".to_string(),
        snapshot_id: "snap-1".to_string(),
        provider: "anthropic".to_string(),
        canonical_model_id: "claude-example".to_string(),
        tier: "standard".to_string(),
        context_threshold: Some(200_000),
        input_rate_per_million: ExactDecimal::parse("3.00").expect("rate"),
        cache_read_rate_per_million: Some(ExactDecimal::parse("0.30").expect("rate")),
        cache_write_rate_per_million: Some(ExactDecimal::parse("3.75").expect("rate")),
        output_rate_per_million: ExactDecimal::parse("15.00").expect("rate"),
    })
    .expect("row");

    // Rates survive a close/reopen with string-exact precision.
    let db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("reopen");
    let row = db
        .price_row("snap-1", "anthropic", "claude-example", "standard")
        .expect("query")
        .expect("row present");
    assert_eq!(row.input_rate_per_million.to_string(), "3.00");
    assert_eq!(
        row.cache_read_rate_per_million.map(|rate| rate.to_string()),
        Some("0.30".to_string())
    );
    assert_eq!(
        row.cache_write_rate_per_million
            .map(|rate| rate.to_string()),
        Some("3.75".to_string())
    );
    assert_eq!(row.output_rate_per_million.to_string(), "15.00");
}

#[test]
fn a_corrupt_database_is_never_recreated() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("usage.sqlite3");
    std::fs::write(&path, b"definitely not sqlite").expect("write garbage");
    let before = std::fs::read(&path).expect("read garbage");

    assert!(UsageDb::open_at(&path).is_err());
    assert_eq!(
        std::fs::read(&path).expect("read after"),
        before,
        "the corrupt file must be left untouched for inspection"
    );
}
