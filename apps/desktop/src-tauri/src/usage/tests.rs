use super::{
    importer::{self, UsageRoots},
    parsers, pricing, queries, store,
    types::*,
};
use crate::{
    decimal::ExactDecimal,
    usage_db::{InputSemantics, UsageDb},
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

const CLI: &str = include_str!("../../tests/fixtures/usage/copilot-session-v1.jsonl");
const OTEL: &[u8] = include_bytes!("../../tests/fixtures/usage/vscode-otel-v1.json");
const COMPLETE: &[u8] =
    include_bytes!("../../tests/fixtures/usage/vscode-explicit-zero-cache-write.json");
const PRICES: &[u8] = include_bytes!("../../tests/fixtures/usage/openrouter-text-prices-v1.json");
fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap()
}
fn observation() -> Observation {
    parsers::parse_line("vscode-otel", COMPLETE, &mut ParserState::default())
        .unwrap()
        .remove(0)
}
fn catalog() -> pricing::PriceCatalog {
    pricing::parse(PRICES, fixture_time()).unwrap()
}
fn query() -> UsageQuery {
    let mut q = queries::default_query();
    q.start = "2026-09-01".into();
    q.end = "2026-09-09".into();
    q
}
struct Fixture {
    dir: tempfile::TempDir,
    db: UsageDb,
    roots: UsageRoots,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = UsageDb::open_at(&dir.path().join("usage.sqlite3")).unwrap();
        importer::initialize(&db).unwrap();
        let roots = UsageRoots {
            sessions: dir.path().join("sessions"),
            vscode_otel: dir.path().join("otel/events.jsonl"),
        };
        Self { dir, db, roots }
    }
    fn enable(&self, source: &str) {
        importer::set_enabled(&self.db, &self.roots, source, true).unwrap();
    }
    fn session(&self, text: &str) {
        fs::create_dir_all(self.roots.sessions.join("a")).unwrap();
        fs::write(self.roots.sessions.join("a/events.jsonl"), text).unwrap();
    }
    fn sync(&mut self) -> Vec<SyncRun> {
        importer::sync(
            &mut self.db,
            &self.roots,
            &[],
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap()
    }
}

#[test]
fn cli_preserves_unknown_semantics_and_discards_ephemeral_content() {
    let mut state = ParserState::default();
    let mut rows = vec![];
    for line in CLI.lines() {
        rows.extend(
            parsers::parse_line("copilot-session-events", line.as_bytes(), &mut state).unwrap(),
        );
    }
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.input_reported, Some(1000));
    assert_eq!(r.request_count, Some(2));
    assert_eq!(r.input_semantics, InputSemantics::Unknown);
    assert_eq!(r.normalized_input, None);
    assert_eq!(r.route, Route::Unknown);
    assert_eq!(r.surfaces, ["shared-copilot-runtime"]);
    let persisted = store::encode(r).unwrap();
    assert!(!persisted.contains("PRIVATE_"));
    assert!(!persisted.contains("sanitized-session"));
    let mut start: Value = serde_json::from_str(CLI.lines().next().unwrap()).unwrap();
    start["data"]["version"] = json!(2);
    assert!(parsers::parse_line(
        "copilot-session-events",
        &serde_json::to_vec(&start).unwrap(),
        &mut state
    )
    .is_err());
}

#[test]
fn otel_missing_is_not_zero_and_counter_underflow_is_rejected() {
    let r = parsers::parse_line("vscode-otel", OTEL, &mut ParserState::default())
        .unwrap()
        .remove(0);
    assert_eq!(r.cache_write, None);
    assert_eq!(r.fresh_input, None);
    assert_eq!(r.normalized_input, Some(1000));
    assert_eq!(r.route, Route::OfficialGithub);
    assert!(!store::encode(&r).unwrap().contains("PRIVATE_"));
    let mut complete = observation();
    assert_eq!(complete.fresh_input, Some(200));
    assert_eq!(complete.cache_write, Some(0));
    complete.cache_read = Some(1001);
    parsers::normalize(&mut complete);
    assert_eq!(complete.normalized_input, None);
    assert_eq!(complete.fresh_input, None);
    for value in [
        json!({"scopeMetrics":[]}),
        json!({"body":"PRIVATE_LINE"}),
        json!({"attributes":{"gen_ai.operation.name":"execute_tool"}}),
    ] {
        assert!(parsers::parse_line(
            "vscode-otel",
            &serde_json::to_vec(&value).unwrap(),
            &mut ParserState::default()
        )
        .unwrap()
        .is_empty());
    }
}

#[test]
fn custom_endpoint_and_unknown_agent_are_not_official_by_model_name() {
    let mut span: Value = serde_json::from_slice(OTEL).unwrap();
    span["attributes"]["server.address"] = json!("example-provider.invalid");
    let r = parsers::parse_line(
        "vscode-otel",
        &serde_json::to_vec(&span).unwrap(),
        &mut ParserState::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(r.route, Route::Unknown);
    assert_eq!(r.connection_id, None);
    span["attributes"]["gen_ai.agent.name"] = json!("AnthropicBYOK");
    let r = parsers::parse_line(
        "vscode-otel",
        &serde_json::to_vec(&span).unwrap(),
        &mut ParserState::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(r.route, Route::Byok);
    assert_eq!(r.confidence, Confidence::Explicit);
    assert_eq!(r.connection_id, None);
    span["kind"] = json!(0);
    assert!(parsers::parse_line(
        "vscode-otel",
        &serde_json::to_vec(&span).unwrap(),
        &mut ParserState::default()
    )
    .unwrap()
    .is_empty());
}

#[test]
fn exact_prices_convert_token_units_and_zero_is_a_real_price() {
    let c = catalog();
    let price = c.models.iter().find(|m| m.model == "openai/gpt-5").unwrap();
    assert_eq!(price.tiers[0].input, ExactDecimal::parse("1.25").unwrap());
    assert_eq!(price.tiers[0].cache_read, ExactDecimal::parse("0.125"));
    assert_eq!(
        pricing::estimate(&observation(), price).unwrap(),
        ExactDecimal::parse("0.00075").unwrap()
    );
    let free = c
        .models
        .iter()
        .find(|m| m.model == "sample/free-model")
        .unwrap();
    assert_eq!(
        pricing::estimate(&observation(), free).unwrap(),
        ExactDecimal::ZERO
    );
    assert!(
        !c.models
            .iter()
            .find(|m| m.model == "sample/dynamic-model")
            .unwrap()
            .supported
    );
    let missing = parsers::parse_line("vscode-otel", OTEL, &mut ParserState::default())
        .unwrap()
        .remove(0);
    assert!(pricing::estimate(&missing, price).is_err());
}

#[test]
fn ambiguous_aliases_unknown_rates_context_and_cache_retention_never_pick_cheaper_price() {
    let data = json!({"data":[{"id":"a/same","pricing":{"prompt":"0.00001","completion":"0.00002"}},{"id":"b/same","pricing":{"prompt":"0.00001","completion":"0.00002"}}]});
    let aliases = pricing::parse(&serde_json::to_vec(&data).unwrap(), fixture_time()).unwrap();
    assert!(!aliases.aliases.contains_key("same"));
    let c = catalog();
    let model = c
        .models
        .iter()
        .find(|m| m.model == "anthropic/claude-sonnet-4")
        .unwrap();
    let mut r = observation();
    r.counter_kind = CounterKind::SessionCumulative;
    assert!(pricing::estimate(&r, model)
        .unwrap_err()
        .contains("per-request"));
    r.counter_kind = CounterKind::RequestDelta;
    r.cache_write = Some(1);
    r.fresh_input = Some(199);
    assert!(pricing::estimate(&r, model)
        .unwrap_err()
        .contains("retention"));
    r.cache_write = Some(0);
    r.normalized_input = Some(250000);
    r.fresh_input = Some(249200);
    assert_eq!(
        pricing::estimate(&r, model).unwrap(),
        ExactDecimal::parse("1.49634").unwrap()
    );
}

#[test]
fn cache_rate_is_weighted_by_tokens_and_partial_estimates_expose_coverage() {
    let mut first = observation();
    first.estimate_usd = ExactDecimal::parse("0.00075");
    let mut second = observation();
    second.input_reported = Some(9000);
    second.cache_read = Some(900);
    parsers::normalize(&mut second);
    let totals = queries::totals(&[&first, &second]).unwrap();
    assert_eq!(totals.cache_hit_rate, ExactDecimal::parse("0.17"));
    assert_eq!(totals.priced_records, 1);
    assert_eq!(totals.unpriced_records, 1);
    assert_eq!(totals.estimate_usd, ExactDecimal::parse("0.00075"));
    assert_eq!(totals.priced_requests, Some(1));
    second.output = None;
    let totals = queries::totals(&[&first, &second]).unwrap();
    assert_eq!(totals.output, None);
    assert!(queries::totals(&[]).unwrap().estimate_usd.is_none());
}

#[test]
fn repeated_import_cumulative_replacement_rotation_and_shared_runtime_do_not_duplicate() {
    let mut f = Fixture::new();
    f.session(CLI);
    f.enable("copilot-session-events");
    assert_eq!(f.sync().len(), 1);
    assert_eq!(f.db.record_count("copilot-session-events").unwrap(), 1);
    f.sync();
    assert_eq!(
        queries::overview(&f.db, &query())
            .unwrap()
            .totals
            .input_reported,
        Some(1000)
    );
    let next = CLI
        .lines()
        .last()
        .unwrap()
        .replace("01:00:00Z", "02:00:00Z")
        .replace("\"inputTokens\":1000", "\"inputTokens\":2000");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(f.roots.sessions.join("a/events.jsonl"))
            .unwrap(),
        "{next}"
    )
    .unwrap();
    f.sync();
    assert_eq!(
        queries::overview(&f.db, &query())
            .unwrap()
            .totals
            .input_reported,
        Some(2000)
    );
    fs::rename(
        f.roots.sessions.join("a/events.jsonl"),
        f.roots.sessions.join("a/rotated.jsonl"),
    )
    .unwrap();
    f.session(CLI);
    f.sync(); // Older rotation cannot overwrite the newer cumulative value.
    let overview = queries::overview(&f.db, &query()).unwrap();
    assert_eq!(overview.total_records, 1);
    assert_eq!(overview.totals.input_reported, Some(2000));
    assert!(importer::set_enabled(&f.db, &f.roots, "github-copilot-app", true).is_err());
    assert_eq!(
        importer::sources(&f.db, &f.roots).unwrap()[2].status,
        DataStatus::Unsupported
    );
}

#[test]
fn active_final_line_and_atomic_cursor_survive_reopen_and_truncation() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    let complete = std::str::from_utf8(COMPLETE).unwrap().trim_end();
    fs::write(
        &f.roots.vscode_otel,
        &complete.as_bytes()[..complete.len() / 2],
    )
    .unwrap();
    f.sync();
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 0);
    fs::write(&f.roots.vscode_otel, format!("{complete}\n")).unwrap();
    f.sync();
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 1);
    let path = f.db.path().to_path_buf();
    drop(f.db);
    f.db = UsageDb::open_at(&path).unwrap();
    f.sync();
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 1);
    let mut different: Value = serde_json::from_slice(COMPLETE).unwrap();
    different["_spanContext"]["spanId"] = json!("new-span");
    fs::write(&f.roots.vscode_otel, format!("{different}\n")).unwrap();
    f.sync();
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 2);
    assert!(importer::sources(&f.db, &f.roots).unwrap()[1].enabled);
}

#[test]
fn malformed_or_oversized_complete_events_do_not_advance_cursors() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    let mut bytes = COMPLETE.to_vec();
    bytes.extend(b"{malformed}\n");
    fs::write(&f.roots.vscode_otel, bytes).unwrap();
    assert_eq!(f.sync()[0].status, DataStatus::Partial);
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 0);
    let count: i64 =
        f.db.conn
            .query_row("SELECT count(*) FROM usage_file_cursors", [], |r| r.get(0))
            .unwrap();
    assert_eq!(count, 0);
    let bytes = vec![b' '; parsers::MAX_LINE_BYTES + 1];
    assert!(parsers::parse_line("vscode-otel", &bytes, &mut ParserState::default()).is_err());
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 1);
}

#[test]
fn cancellation_after_last_file_is_not_reported_as_success_and_clear_preserves_source() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    let cancel = AtomicBool::new(false);
    let runs = importer::sync(&mut f.db, &f.roots, &[], &cancel, |r| {
        if r.files_seen > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(runs[0].status, DataStatus::Canceled);
    pricing::save(&mut f.db, &catalog()).unwrap();
    importer::clear(&mut f.db, "vscode-otel").unwrap();
    assert_eq!(fs::read(&f.roots.vscode_otel).unwrap(), COMPLETE);
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 0);
    assert!(pricing::latest(&f.db).unwrap().is_some());
    assert!(!importer::sources(&f.db, &f.roots).unwrap()[1].enabled);
}

#[test]
fn interrupted_jobs_are_recovered_and_nonregular_sources_are_rejected() {
    let mut f = Fixture::new();
    let run = SyncRun {
        id: "interrupted".into(),
        source_id: "vscode-otel".into(),
        started_at: Utc::now(),
        finished_at: None,
        status: DataStatus::InProgress,
        files_seen: 0,
        records_seen: 0,
        bytes_read: 0,
        detail: "In progress".into(),
    };
    store::save_run(&f.db, &run).unwrap();
    store::recover(&f.db).unwrap();
    assert_eq!(
        store::runs(&f.db).unwrap()[0].status,
        DataStatus::Interrupted
    );
    f.enable("vscode-otel");
    fs::create_dir(&f.roots.vscode_otel).unwrap();
    assert_eq!(f.sync()[0].status, DataStatus::Partial);
    assert!(importer::set_enabled(&f.db, &f.roots, "../arbitrary-source", true).is_err());
}

#[test]
fn historical_price_bindings_do_not_change_and_storage_contains_no_source_content() {
    let mut f = Fixture::new();
    let c = catalog();
    pricing::save(&mut f.db, &c).unwrap();
    let vendor: String =
        f.db.conn
            .query_row(
                "SELECT canonical_provider FROM model_aliases WHERE raw_value='gpt-5' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(vendor, "openai");
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    let first = queries::overview(&f.db, &query())
        .unwrap()
        .records
        .remove(0);
    assert_eq!(first.estimate_usd, ExactDecimal::parse("0.00075"));
    assert_eq!(first.price_snapshot_id, Some(c.id.clone()));
    let mut changed: Value = serde_json::from_slice(PRICES).unwrap();
    changed["data"][0]["pricing"]["prompt"] = json!("0.001");
    let next = pricing::parse(
        &serde_json::to_vec(&changed).unwrap(),
        fixture_time() + chrono::Duration::days(1),
    )
    .unwrap();
    pricing::save(&mut f.db, &next).unwrap();
    pricing::price_unbound(&mut f.db, &next).unwrap();
    f.sync();
    let after = queries::overview(&f.db, &query())
        .unwrap()
        .records
        .remove(0);
    assert_eq!(after.price_snapshot_id, first.price_snapshot_id);
    assert_eq!(after.estimate_usd, first.estimate_usd);
    // Persisted metadata never contains sentinel conversation/auth data, including WAL pages.
    for entry in fs::read_dir(f.dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert!(!String::from_utf8_lossy(&fs::read(path).unwrap()).contains("PRIVATE_"));
        }
    }
}

#[test]
fn returning_price_content_selects_current_rates_without_rebinding_history() {
    let mut f = Fixture::new();
    let first = catalog();
    pricing::save(&mut f.db, &first).unwrap();
    let mut changed: Value = serde_json::from_slice(PRICES).unwrap();
    changed["data"][0]["pricing"]["prompt"] = json!("0.001");
    let second = pricing::parse(
        &serde_json::to_vec(&changed).unwrap(),
        fixture_time() + chrono::Duration::days(1),
    )
    .unwrap();
    pricing::save(&mut f.db, &second).unwrap();
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    let historical = queries::overview(&f.db, &query())
        .unwrap()
        .records
        .remove(0);
    assert_eq!(
        historical.price_snapshot_id.as_deref(),
        Some(second.id.as_str())
    );

    let returned = pricing::parse(PRICES, fixture_time() + chrono::Duration::days(2)).unwrap();
    assert_eq!(returned.id, first.id);
    pricing::save(&mut f.db, &returned).unwrap();
    // A late older response cannot displace the most recent successful observation.
    pricing::save(&mut f.db, &second).unwrap();
    let current = pricing::latest(&f.db).unwrap().unwrap();
    assert_eq!(current.id, first.id);
    assert_eq!(current.fetched_at, first.fetched_at);
    assert_eq!(
        pricing::view(&f.db).unwrap().checked_at,
        Some(returned.fetched_at)
    );

    let mut next: Value = serde_json::from_slice(COMPLETE).unwrap();
    next["_spanContext"]["spanId"] = json!("1111111111111111");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&f.roots.vscode_otel)
            .unwrap(),
        "{next}"
    )
    .unwrap();
    f.sync();
    pricing::price_unbound(&mut f.db, &current).unwrap();
    let rows = queries::overview(&f.db, &query()).unwrap().records;
    assert_eq!(rows.len(), 2);
    let old = rows.iter().find(|r| r.id == historical.id).unwrap();
    assert_eq!(old.price_snapshot_id, historical.price_snapshot_id);
    assert_eq!(old.estimate_usd, historical.estimate_usd);
    let new = rows.iter().find(|r| r.id != historical.id).unwrap();
    assert_eq!(new.price_snapshot_id.as_deref(), Some(first.id.as_str()));
    assert_eq!(new.estimate_usd, ExactDecimal::parse("0.00075"));
    let count: u32 =
        f.db.conn
            .query_row("SELECT count(*) FROM price_snapshots", [], |r| r.get(0))
            .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn v2_price_history_migrates_to_a_current_pointer_and_reopens() {
    let mut f = Fixture::new();
    let first = catalog();
    pricing::save(&mut f.db, &first).unwrap();
    let mut changed: Value = serde_json::from_slice(PRICES).unwrap();
    changed["data"][0]["pricing"]["prompt"] = json!("0.001");
    let second = pricing::parse(
        &serde_json::to_vec(&changed).unwrap(),
        fixture_time() + chrono::Duration::days(1),
    )
    .unwrap();
    pricing::save(&mut f.db, &second).unwrap();
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    let original = queries::overview(&f.db, &query())
        .unwrap()
        .records
        .remove(0);
    // Reconstruct the previous schema while retaining real v2 catalog/usage rows.
    f.db.conn
        .execute_batch(
            "DROP TABLE current_price_catalog;
        DELETE FROM schema_migrations WHERE version=3; PRAGMA user_version=2;",
        )
        .unwrap();
    let path = f.db.path().to_path_buf();
    drop(f.db);
    f.db = UsageDb::open_at(&path).unwrap();
    assert_eq!(f.db.status().schema_version, Some(3));
    let view = pricing::view(&f.db).unwrap();
    assert_eq!(view.catalog.unwrap().id, second.id);
    assert_eq!(view.checked_at, Some(second.fetched_at));
    assert_eq!(
        store::encode(&queries::overview(&f.db, &query()).unwrap().records[0]).unwrap(),
        store::encode(&original).unwrap()
    );
    drop(f.db);
    let db = UsageDb::open_at(&path).unwrap();
    assert_eq!(pricing::latest(&db).unwrap().unwrap().id, second.id);
}

#[test]
fn filters_pagination_foreign_keys_and_batch_bounds_are_enforced() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    let mut q = query();
    q.route = Some(Route::Byok);
    assert_eq!(queries::overview(&f.db, &q).unwrap().total_records, 0);
    q.route = None;
    q.page_size = Some(101);
    assert!(queries::overview(&f.db, &q).is_err());
    q.page_size = Some(1);
    q.page = Some(1);
    assert!(queries::overview(&f.db, &q).unwrap().records.is_empty());
    q.start = "2020-01-01".into();
    assert!(queries::overview(&f.db, &q).is_err());
    let fk: i64 =
        f.db.conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
    assert_eq!(fk, 1);
    let cursor = FileCursor {
        offset: 1,
        prefix_hash: "a".into(),
        file_identity: "a".into(),
        file_size: 1,
        modified: Utc::now().to_rfc3339(),
        parser_version: 1,
        state: ParserState::default(),
    };
    assert!(store::commit_batch(
        &mut f.db,
        "vscode-otel",
        "test",
        &cursor,
        &vec![observation(); 501]
    )
    .is_err());
    assert!(store::commit_batch(&mut f.db, "unknown", "test", &cursor, &[]).is_err());
}

struct FakeRpc {
    calls: Vec<String>,
    protocol: u64,
    login: String,
    quota_failure: bool,
}
impl super::runtime::ReadOnlyRpc for FakeRpc {
    fn request(&mut self, method: &'static str, params: Value) -> super::runtime::RpcResult<Value> {
        self.calls.push(method.into());
        match method {
            "connect" => {
                assert_eq!(params["enableGitHubTelemetryForwarding"], false);
                assert_eq!(params["supportedTaskKinds"], json!([]));
                Ok(json!({"protocolVersion":self.protocol,"version":"0.0.fixture"}))
            }
            "auth.getStatus" => Ok(
                json!({"isAuthenticated":true,"login":self.login,"host":"github.com","statusMessage":"PRIVATE_TOKEN_DO_NOT_RETAIN"}),
            ),
            "account.getQuota" => {
                if self.quota_failure {
                    Err(super::runtime::RuntimeError(
                        DataStatus::NetworkError,
                        "Safe simulated failure",
                    ))
                } else {
                    Ok(serde_json::from_str(include_str!(
                        "../../tests/fixtures/usage/copilot-quota-v3.json"
                    ))
                    .unwrap())
                }
            }
            "models.list" => {
                Ok(json!({"models":[{"id":"gpt-5","name":"GPT-5","policy":{"state":"enabled"}}]}))
            }
            _ => panic!("Runtime must not create a session or invoke a prompt"),
        }
    }
}
#[test]
fn runtime_uses_readonly_methods_and_distinguishes_zero_unlimited_empty_and_schema_error() {
    use super::runtime;
    let mut rpc = FakeRpc {
        calls: vec![],
        protocol: 3,
        login: "fixture".into(),
        quota_failure: false,
    };
    let s = runtime::snapshot(&mut rpc);
    assert_eq!(s.status, DataStatus::Available);
    assert_eq!(
        rpc.calls,
        [
            "connect",
            "auth.getStatus",
            "account.getQuota",
            "models.list"
        ]
    );
    assert!(s
        .quotas
        .iter()
        .any(|q| q.unlimited && q.entitlement.is_none()));
    assert_eq!(
        s.quotas
            .iter()
            .find(|q| q.key == "premium_interactions")
            .unwrap()
            .used,
        Some(ExactDecimal::ZERO)
    );
    assert!(!store::encode(&s).unwrap().contains("PRIVATE_"));
    assert!(runtime::parse_quotas(&json!({"quotaSnapshots":{}}))
        .unwrap()
        .is_empty());
    assert!(runtime::parse_quotas(
        &json!({"quotaSnapshots":{"x":{"isUnlimitedEntitlement":false,"entitlementRequests":-1}}})
    )
    .is_err());
    assert!(runtime::parse_quotas(
        &json!({"quotaSnapshots":{"x":{"isUnlimitedEntitlement":false,"remainingPercentage":101}}})
    )
    .is_err());
    rpc.protocol = 4;
    let unsupported = runtime::snapshot(&mut rpc);
    assert_eq!(unsupported.status, DataStatus::Unsupported);
}

#[test]
fn runtime_failures_keep_same_account_success_and_models_are_independent() {
    use super::runtime;
    let mut f = Fixture::new();
    let mut rpc = FakeRpc {
        calls: vec![],
        protocol: 3,
        login: "first".into(),
        quota_failure: false,
    };
    let successful = runtime::snapshot(&mut rpc);
    runtime::save(&mut f.db, &successful).unwrap();
    rpc.quota_failure = true;
    let failure = runtime::snapshot(&mut rpc);
    assert_eq!(failure.model_status, DataStatus::Available);
    assert_eq!(failure.models.len(), 1);
    runtime::save(&mut f.db, &failure).unwrap();
    let view = runtime::view(&f.db).unwrap();
    assert!(view.stale);
    assert_eq!(view.last_successful.unwrap().id, successful.id);
    rpc.login = "different-account".into();
    runtime::save(&mut f.db, &runtime::snapshot(&mut rpc)).unwrap();
    assert!(runtime::view(&f.db).unwrap().last_successful.is_none());
}

#[test]
fn rpc_frames_are_bounded_and_require_unique_exact_content_length() {
    use super::runtime::read_frame;
    let body = br#"{"id":1,"result":{"ok":true}}"#;
    let framed = format!(
        "Content-Length: {}\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).unwrap()
    );
    assert_eq!(
        read_frame(&mut std::io::Cursor::new(framed)).unwrap()["result"]["ok"],
        true
    );
    for input in [
        "Content-Length: 3000000\r\n\r\n",
        "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        "Content-Length: 10\r\n\r\n{}",
        "Content-Length: 5\r\n\r\nabcde",
    ] {
        assert!(read_frame(&mut std::io::Cursor::new(input)).is_err());
    }
}

#[test]
fn token_counters_cross_ipc_without_javascript_rounding_and_read_legacy_numbers() {
    let mut r = observation();
    r.input_reported = Some(9_007_199_254_740_993);
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["inputReported"], "9007199254740993");
    assert_eq!(
        serde_json::from_value::<Observation>(json.clone())
            .unwrap()
            .input_reported,
        r.input_reported
    );
    let mut legacy = json;
    legacy["inputReported"] = serde_json::json!(1000);
    assert_eq!(
        serde_json::from_value::<Observation>(legacy)
            .unwrap()
            .input_reported,
        Some(1000)
    );
}

#[test]
fn stale_price_derivation_cannot_overwrite_a_newer_import() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    fs::write(&f.roots.vscode_otel, COMPLETE).unwrap();
    f.sync();
    let before = queries::overview(&f.db, &query())
        .unwrap()
        .records
        .remove(0);
    let original = store::encode(&before).unwrap();
    let c = catalog();
    pricing::save(&mut f.db, &c).unwrap();
    let mut priced = before.clone();
    pricing::bind(&f.db, &mut priced, None, Some(&c)).unwrap();
    let mut newer = before.clone();
    newer.input_reported = Some(2000);
    newer.imported_at = Utc::now();
    f.db.conn
        .execute(
            "UPDATE usage_observation_details SET payload=?1 WHERE record_id=?2",
            rusqlite::params![store::encode(&newer).unwrap(), newer.id],
        )
        .unwrap();
    pricing::persist_price_updates(&mut f.db, vec![(&original, priced)]).unwrap();
    let result = store::existing(&f.db, &newer.id).unwrap().unwrap();
    assert_eq!(result.input_reported, Some(2000));
    assert_eq!(result.price_snapshot_id, None);
}

#[test]
fn sensitive_usage_symlink_is_refused_without_reading_its_content() {
    let mut f = Fixture::new();
    f.enable("vscode-otel");
    let target = f.dir.path().join("private-source.jsonl");
    fs::write(&target, COMPLETE).unwrap();
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&target, &f.roots.vscode_otel);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(&target, &f.roots.vscode_otel);
    if let Err(error) = created {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!("Symlink creation is unavailable to this test account");
            return;
        }
        panic!("Could not create temporary test symlink: {error}");
    }
    let runs = f.sync();
    assert!(!runs[0].status.successful());
    assert!(!runs[0].detail.contains(f.dir.path().to_str().unwrap()));
    assert_eq!(f.db.record_count("vscode-otel").unwrap(), 0);
}
