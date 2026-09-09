//! Versioned, allowlist-only parsers. See docs/usage-sources.md for provenance.
use super::{bounded_id, invalid, types::*};
use crate::{error::AppResult, fingerprint, usage_db::InputSemantics};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const PARSER_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_TOKENS: u64 = 1_000_000_000_000;

pub fn hash(domain: &str, value: &str) -> String {
    fingerprint::bytes(domain, Some(value.as_bytes()))
}

fn count(value: &Value, field: &str) -> AppResult<Option<u64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= MAX_TOKENS)
            .map(Some)
            .ok_or_else(|| invalid("Invalid or oversized usage counter")),
    }
}

fn timestamp(value: &Value) -> AppResult<DateTime<Utc>> {
    if let Some(s) = value.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|_| invalid("Invalid usage timestamp"));
    }
    // OpenTelemetry HrTime is [seconds, nanoseconds], not milliseconds.
    let parts = value
        .as_array()
        .filter(|p| p.len() == 2)
        .ok_or_else(|| invalid("Unsupported usage timestamp schema"))?;
    let secs = parts[0]
        .as_i64()
        .ok_or_else(|| invalid("Invalid usage time"))?;
    let nanos = parts[1]
        .as_u64()
        .filter(|n| *n < 1_000_000_000)
        .ok_or_else(|| invalid("Invalid usage time"))?;
    DateTime::from_timestamp(secs, nanos as u32).ok_or_else(|| invalid("Invalid usage time"))
}

fn base(
    source: &str,
    model: String,
    started: DateTime<Utc>,
    finished: DateTime<Utc>,
) -> AppResult<Observation> {
    if started > finished
        || started.year() < 2020
        || finished > Utc::now() + chrono::Duration::days(1)
    {
        return Err(invalid("Usage time is outside the supported interval"));
    }
    Ok(Observation {
        id: String::new(),
        source_id: source.into(),
        session_hash: None,
        request_hash: None,
        raw_model: model,
        canonical_model: None,
        route: Route::Unknown,
        confidence: Confidence::Unknown,
        connection_id: None,
        provider: None,
        surfaces: vec![],
        counter_kind: CounterKind::RequestDelta,
        request_count: Some(1),
        input_semantics: InputSemantics::Unknown,
        input_reported: None,
        normalized_input: None,
        fresh_input: None,
        cache_read: None,
        cache_write: None,
        output: None,
        started_at: started,
        finished_at: finished,
        imported_at: Utc::now(),
        quality: vec![],
        price_snapshot_id: None,
        estimate_usd: None,
        estimate_detail: "No compatible price snapshot".into(),
    })
}
use chrono::Datelike;

pub fn parse_line(
    source: &str,
    bytes: &[u8],
    state: &mut ParserState,
) -> AppResult<Vec<Observation>> {
    if bytes.len() > MAX_LINE_BYTES {
        return Err(invalid("Usage event exceeds the line limit"));
    }
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|_| invalid("Usage JSONL contains a malformed complete event"))?;
    match source {
        "copilot-session-events" => cli(&v, state),
        "vscode-otel" => vscode(&v),
        _ => Err(invalid("Unsupported physical usage source")),
    }
}

fn cli(v: &Value, state: &mut ParserState) -> AppResult<Vec<Observation>> {
    let kind = v
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Unsupported Copilot event envelope"))?;
    if kind == "session.start" {
        let data = &v["data"];
        // Explicitly supported event contract. Future versions need fixtures.
        if data["version"].as_u64() != Some(1) {
            return Err(invalid(
                "Unsupported Copilot session schema version (supported: 1)",
            ));
        }
        let id = bounded_id(
            data["sessionId"]
                .as_str()
                .ok_or_else(|| invalid("Copilot session identity is missing"))?,
        )?;
        state.session_hash = Some(hash("copilot-session-v1", &id));
        state.schema_version = Some(1);
        return Ok(vec![]);
    }
    // Never count ephemeral usage and cumulative shutdown metrics together.
    if kind != "session.shutdown" {
        return Ok(vec![]);
    }
    if state.schema_version != Some(1) {
        return Err(invalid(
            "Copilot usage has no supported session.start event",
        ));
    }
    let session = state
        .session_hash
        .as_ref()
        .ok_or_else(|| invalid("Missing session identity"))?;
    let data = &v["data"];
    let metrics = data["modelMetrics"]
        .as_object()
        .filter(|m| m.len() <= 128)
        .ok_or_else(|| invalid("Unsupported Copilot model metrics schema"))?;
    let finished = timestamp(&v["timestamp"])?;
    let started = data["sessionStartTime"]
        .as_i64()
        .and_then(DateTime::from_timestamp_millis)
        .ok_or_else(|| invalid("Copilot usage start time is missing"))?;
    let mut result = vec![];
    for (model, metric) in metrics {
        let mut record = base(
            "copilot-session-events",
            bounded_id(model)?,
            started,
            finished,
        )?;
        record.id = hash("copilot-session-model-v1", &format!("{session}:{model}"));
        record.session_hash = Some(session.clone());
        record.counter_kind = CounterKind::SessionCumulative;
        record.request_count = count(&metric["requests"], "count")?;
        let usage = metric
            .get("usage")
            .filter(|v| v.is_object())
            .ok_or_else(|| invalid("Missing Copilot token metrics"))?;
        record.input_reported = count(usage, "inputTokens")?;
        record.cache_read = count(usage, "cacheReadTokens")?;
        record.cache_write = count(usage, "cacheWriteTokens")?;
        record.output = count(usage, "outputTokens")?;
        record.surfaces = vec!["shared-copilot-runtime".into()];
        // Upstream shutdown types do not promise total-vs-fresh semantics or
        // a route/client identity. Do not infer either from the model name.
        record.quality = vec![
            "Cumulative session/model total; daily grouping uses shutdown date".into(),
            "Input token semantics and CLI/app attribution are not established by this schema"
                .into(),
        ];
        result.push(record);
    }
    Ok(result)
}

fn vscode(v: &Value) -> AppResult<Vec<Observation>> {
    let Some(attrs) = v.get("attributes").and_then(Value::as_object) else {
        // The file exporter also writes metrics and log records. These are
        // deliberately excluded to avoid counting the same API call again.
        if v.get("scopeMetrics").is_some() || v.get("body").is_some() {
            return Ok(vec![]);
        }
        return Err(invalid("Unsupported VS Code OTel file schema"));
    };
    if attrs.get("gen_ai.operation.name").and_then(Value::as_str) != Some("chat") {
        return Ok(vec![]);
    }
    // Ignore inference-detail log events and aggregate parent spans.
    if v.get("body").is_some() || attrs.contains_key("event.name") {
        return Ok(vec![]);
    }
    match v.get("kind").and_then(Value::as_u64) {
        Some(2) => {} // OTel CLIENT inference spans, excluding aggregate parents.
        Some(0..=5) => return Ok(vec![]),
        _ => return Err(invalid("Unsupported OTel span kind schema")),
    }
    let context = v
        .get("_spanContext")
        .or_else(|| v.get("spanContext"))
        .ok_or_else(|| invalid("OTel chat span has no stable identity"))?;
    let trace = bounded_id(
        context["traceId"]
            .as_str()
            .ok_or_else(|| invalid("Missing trace ID"))?,
    )?;
    let span = bounded_id(
        context["spanId"]
            .as_str()
            .ok_or_else(|| invalid("Missing span ID"))?,
    )?;
    let model = attrs
        .get("gen_ai.response.model")
        .or_else(|| attrs.get("gen_ai.request.model"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Missing OTel model"))?;
    let mut r = base(
        "vscode-otel",
        bounded_id(model)?,
        timestamp(&v["startTime"])?,
        timestamp(&v["endTime"])?,
    )?;
    r.id = hash("vscode-span-v1", &format!("{trace}:{span}"));
    r.request_hash = Some(r.id.clone());
    if let Some(id) = attrs.get("gen_ai.conversation.id").and_then(Value::as_str) {
        r.session_hash = Some(hash("vscode-session-v1", &bounded_id(id)?));
    }
    r.surfaces = vec!["vs-code-copilot".into()];
    let a = &v["attributes"];
    r.input_reported = count(a, "gen_ai.usage.input_tokens")?;
    r.cache_read = count(a, "gen_ai.usage.cache_read.input_tokens")?;
    r.cache_write = count(a, "gen_ai.usage.cache_creation.input_tokens")?;
    r.output = count(a, "gen_ai.usage.output_tokens")?;
    r.provider = attrs
        .get("gen_ai.provider.name")
        .and_then(Value::as_str)
        .map(bounded_id)
        .transpose()?;
    // chatMLFetcher also serves custom endpoints but labels their provider
    // "github". A concrete non-GitHub host therefore invalidates that label.
    let server = attrs.get("server.address").and_then(Value::as_str);
    if r.provider.as_deref() == Some("github") && server.is_none() {
        r.route = Route::OfficialGithub;
        r.confidence = Confidence::Explicit;
    }
    let agent = attrs.get("gen_ai.agent.name").and_then(Value::as_str);
    if matches!(agent, Some("AnthropicBYOK")) {
        r.route = Route::Byok;
        r.confidence = Confidence::Explicit;
    }
    if r.route == Route::Unknown {
        r.quality.push(
            "The source does not establish an unambiguous route or Connection identity".into(),
        );
    }
    r.input_semantics = InputSemantics::TotalIncludesCacheReadAndWrite;
    normalize(&mut r);
    Ok(vec![r])
}

pub fn normalize(r: &mut Observation) {
    if r.input_semantics == InputSemantics::TotalIncludesCacheReadAndWrite {
        r.normalized_input = r.input_reported;
        r.fresh_input = r
            .input_reported
            .zip(r.cache_read)
            .zip(r.cache_write)
            .and_then(|((total, read), write)| total.checked_sub(read)?.checked_sub(write));
        if let Some(total) = r.input_reported {
            if r.cache_read.is_some_and(|n| n > total)
                || r.cache_write.is_some_and(|n| n > total)
                || r.cache_read
                    .zip(r.cache_write)
                    .is_some_and(|(a, b)| a.checked_add(b).is_none_or(|n| n > total))
            {
                r.normalized_input = None;
                r.fresh_input = None;
                r.quality
                    .push("Inconsistent cache counters; derived input is unavailable".into());
            }
        }
    }
    if r.cache_write.is_none() {
        r.quality
            .push("Cache-write tokens were not reported".into());
    }
}
