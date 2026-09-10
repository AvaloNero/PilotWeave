use super::{bounded_model_id, db_error, invalid, store, types::*};
use crate::{decimal::ExactDecimal, error::AppResult, fingerprint, usage_db::UsageDb};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub const CATALOG_URL: &str = "https://openrouter.ai/api/v1/models";
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTier {
    pub min_input: u64,
    pub input: ExactDecimal,
    pub output: ExactDecimal,
    pub cache_read: Option<ExactDecimal>,
    pub cache_write: Option<ExactDecimal>,
    pub cache_write_1h: Option<ExactDecimal>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub model: String,
    pub tiers: Vec<PriceTier>,
    pub supported: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceCatalog {
    pub id: String,
    pub source: String,
    pub source_version: String,
    pub fetched_at: DateTime<Utc>,
    pub parser_version: u32,
    pub currency: String,
    pub detail: String,
    pub models: Vec<ModelPrice>,
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceView {
    pub catalog: Option<PriceCatalog>,
    pub checked_at: Option<DateTime<Utc>>,
    pub latest: Option<SyncRun>,
    pub stale: bool,
}
pub fn view(db: &UsageDb) -> AppResult<PriceView> {
    let catalog = latest(db)?;
    let latest = store::runs(db)?
        .into_iter()
        .find(|r| r.source_id == "price-catalog");
    let checked = db
        .conn
        .query_row(
            "SELECT checked_at_ms FROM current_price_catalog WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .and_then(DateTime::from_timestamp_millis);
    let stale = latest.as_ref().is_some_and(|r| !r.status.successful())
        || checked.is_none_or(|d| Utc::now() - d > chrono::Duration::days(1));
    Ok(PriceView {
        catalog,
        checked_at: checked,
        latest,
        stale,
    })
}

fn rate(v: &Value, key: &str) -> AppResult<Option<ExactDecimal>> {
    let Some(v) = v.get(key) else {
        return Ok(None);
    };
    let text = v
        .as_str()
        .filter(|s| s.len() <= 64)
        .ok_or_else(|| invalid("Catalog rate must be an exact decimal string"))?;
    let number = ExactDecimal::parse(text)
        .filter(|n| n.get() >= Decimal::ZERO && n.get() <= Decimal::from(10))
        .ok_or_else(|| invalid("Catalog rate is outside the supported range"))?;
    number
        .checked_mul(ExactDecimal::new(Decimal::from(1_000_000)))
        .map(Some)
        .ok_or_else(|| invalid("Catalog rate overflows"))
}

fn tier(v: &Value, min: u64, base: Option<&PriceTier>) -> AppResult<PriceTier> {
    Ok(PriceTier {
        min_input: min,
        input: rate(v, "prompt")?
            .or_else(|| base.map(|b| b.input))
            .ok_or_else(|| invalid("Catalog input price is missing"))?,
        output: rate(v, "completion")?
            .or_else(|| base.map(|b| b.output))
            .ok_or_else(|| invalid("Catalog output price is missing"))?,
        cache_read: rate(v, "input_cache_read")?.or_else(|| base.and_then(|b| b.cache_read)),
        cache_write: rate(v, "input_cache_write")?.or_else(|| base.and_then(|b| b.cache_write)),
        cache_write_1h: rate(v, "input_cache_write_1h")?
            .or_else(|| base.and_then(|b| b.cache_write_1h)),
    })
}

fn parse_tiers(p: &Value) -> AppResult<(Vec<PriceTier>, bool)> {
    let base = tier(p, 0, None)?;
    let mut tiers = vec![base.clone()];
    let mut supported = true;
    if let Some(overrides) = p.get("overrides") {
        let overrides = overrides
            .as_array()
            .filter(|v| v.len() <= 16)
            .ok_or_else(|| invalid("Unsupported price tier schema"))?;
        for row in overrides {
            let min = row["min_prompt_tokens"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 10_000_000)
                .ok_or_else(|| invalid("Unsupported price threshold"))?;
            if tiers.iter().any(|t| t.min_input == min) {
                return Err(invalid("Duplicate price threshold"));
            }
            tiers.push(tier(row, min, Some(&base))?);
            if row.as_object().is_none_or(|m| {
                m.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "min_prompt_tokens"
                            | "prompt"
                            | "completion"
                            | "input_cache_read"
                            | "input_cache_write"
                            | "input_cache_write_1h"
                    )
                })
            }) {
                supported = false;
            }
        }
    }
    // Unknown tier/rate keys are retained as unsupported, never guessed.
    if p.as_object().is_none_or(|m| {
        m.keys().any(|k| {
            !matches!(
                k.as_str(),
                "prompt"
                    | "completion"
                    | "input_cache_read"
                    | "input_cache_write"
                    | "input_cache_write_1h"
                    | "overrides"
                    | "request"
                    | "image"
                    | "web_search"
                    | "internal_reasoning"
                    | "audio"
                    | "input_audio_cache"
            )
        })
    }) {
        supported = false;
    }
    tiers.sort_by_key(|t| t.min_input);
    Ok((tiers, supported))
}

pub fn parse(bytes: &[u8], fetched_at: DateTime<Utc>) -> AppResult<PriceCatalog> {
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(invalid("Price catalog exceeds the size limit"));
    }
    let root: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid("Price catalog schema is invalid"))?;
    let data = root["data"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 4096)
        .ok_or_else(|| invalid("Price catalog model list is missing or oversized"))?;
    let mut parser_version = 1;
    let mut models = vec![];
    let mut aliases: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for item in data {
        let model = bounded_model_id(
            item["id"]
                .as_str()
                .ok_or_else(|| invalid("Price model identity is missing"))?,
        )?;
        if model.starts_with('~') {
            parser_version = 2;
        }
        if !seen.insert(model.clone()) {
            return Err(invalid("Price catalog has duplicate model identities"));
        }
        let p = &item["pricing"];
        let (tiers, supported) = parse_tiers(p).unwrap_or_default();
        // Floating routing aliases must not create unqualified aliases.
        if let Some((_, raw)) = model.split_once('/').filter(|_| !model.starts_with('~')) {
            aliases
                .entry(raw.into())
                .and_modify(|entry| *entry = None)
                .or_insert(Some(model.clone()));
        }
        models.push(ModelPrice {
            model,
            tiers,
            supported,
        });
    }
    if !models.iter().any(|m| m.supported) {
        return Err(invalid(
            "No supported text prices in this catalog; previous snapshot retained",
        ));
    }
    models.sort_by(|a, b| a.model.cmp(&b.model));
    let version = fingerprint::json(
        if parser_version == 1 {
            "openrouter-text-price-v1"
        } else {
            "openrouter-text-price-v2"
        },
        &models,
    )?;
    Ok(PriceCatalog{id:version.clone(),source:CATALOG_URL.into(),source_version:version,parser_version,fetched_at,currency:"USD".into(),
        detail:"OpenRouter published minimum-provider catalog rates, USD per million text tokens. A catalog-price comparison, not an invoice or a direct-provider quote. Non-token fees are excluded; actual routing, cache retention and context tiers may change the price. Fetch time is not an asserted historical effective date.".into(),
        models,aliases:aliases.into_iter().filter_map(|(k,v)|v.map(|v|(k,v))).collect()})
}

pub fn fetch() -> Result<PriceCatalog, (DataStatus, String)> {
    let agent: ureq::Agent = crate::platform::http_config()
        .timeout_global(Some(Duration::from_secs(25)))
        .max_redirects(0)
        .build()
        .into();
    let mut response = agent
        .get(crate::platform::endpoint(CATALOG_URL))
        .call()
        .map_err(|_| {
            (
                DataStatus::NetworkError,
                "Official OpenRouter price catalog is unavailable; previous snapshot retained"
                    .into(),
            )
        })?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_CATALOG_BYTES)
        .read_to_vec()
        .map_err(|_| {
            (
                DataStatus::SchemaError,
                "Price catalog download was truncated or oversized".into(),
            )
        })?;
    parse(&bytes, Utc::now()).map_err(|e| (DataStatus::SchemaError, e.to_string()))
}

pub fn save(db: &mut UsageDb, catalog: &PriceCatalog) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        crate::test_support::fault("price-save")?;
    }

    let tx = db.conn.transaction().map_err(db_error)?;
    let inserted=tx.execute("INSERT OR IGNORE INTO price_snapshots(id,source,source_version,fetched_at,currency,provenance,parser_version) VALUES (?1,?2,?3,?4,'USD',?5,?6)",params![catalog.id,catalog.source,catalog.source_version,catalog.fetched_at.to_rfc3339(),catalog.detail,catalog.parser_version]).map_err(db_error)?;
    if inserted == 1 {
        tx.execute(
            "INSERT INTO price_catalog_payloads VALUES (?1,?2)",
            params![catalog.id, store::encode(catalog)?],
        )
        .map_err(db_error)?;
        for model in &catalog.models {
            for tier in &model.tiers {
                tx.execute("INSERT INTO price_rows(id,snapshot_id,provider,canonical_model_id,tier,context_threshold,input_rate_per_million,output_rate_per_million,cache_read_rate_per_million,cache_write_rate_per_million) VALUES (?1,?2,'openrouter',?3,?4,?5,?6,?7,?8,?9)",
                    params![format!("{}:{}:{}",catalog.id,model.model,tier.min_input),catalog.id,model.model,format!("minimum-catalog-{}",tier.min_input),i64::try_from(tier.min_input).map_err(|_|invalid("Price threshold exceeds storage range"))?,tier.input.to_string(),tier.output.to_string(),tier.cache_read.map(|n|n.to_string()),tier.cache_write.map(|n|n.to_string())]).map_err(db_error)?;
            }
        }
        for (raw, canonical) in &catalog.aliases {
            for scope in ["copilot-session-v1", "vscode-otel-span-v1"] {
                tx.execute("INSERT OR IGNORE INTO model_aliases(id,source_scope,raw_value,canonical_provider,canonical_model_id,status,confidence,effective_from,version) VALUES (?1,?2,?3,?6,?4,'resolved','exact',?5,1)",
                    params![format!("{}:{scope}:{raw}",catalog.id),format!("{scope}:{}",catalog.id),raw,canonical,catalog.fetched_at.to_rfc3339(),canonical.split('/').next()]).map_err(db_error)?;
            }
        }
    }
    // A -> B -> A must select A again without mutating A's historical payload.
    // Discard an older concurrent fetch when a newer observation is already saved.
    tx.execute("INSERT INTO current_price_catalog (id,snapshot_id,checked_at_ms) VALUES (1,?1,?2)
        ON CONFLICT(id) DO UPDATE SET snapshot_id=excluded.snapshot_id,checked_at_ms=excluded.checked_at_ms
        WHERE excluded.checked_at_ms >= current_price_catalog.checked_at_ms",
        params![catalog.id, catalog.fetched_at.timestamp_millis()]).map_err(db_error)?;
    tx.commit().map_err(db_error)
}

pub fn latest(db: &UsageDb) -> AppResult<Option<PriceCatalog>> {
    db.conn.query_row("SELECT p.payload FROM current_price_catalog c JOIN price_catalog_payloads p ON p.snapshot_id=c.snapshot_id WHERE c.id=1",[],|r|r.get(0)).optional().map_err(db_error)?.map(store::decode).transpose()
}

pub fn price_unbound(db: &mut UsageDb, catalog: &PriceCatalog) -> AppResult<()> {
    let rows: Vec<String> = {
        let mut query=db.conn.prepare("SELECT d.payload FROM usage_observation_details d JOIN usage_records r ON r.id=d.record_id WHERE r.price_snapshot_id IS NULL ORDER BY r.finished_at DESC LIMIT 10000").map_err(db_error)?;
        let values = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        values
    };
    for batch in rows.chunks(500) {
        let mut updates = vec![];
        for payload in batch {
            let mut r: Observation = store::decode(payload.clone())?;
            bind(db, &mut r, None, Some(catalog))?;
            updates.push((payload, r));
        }
        persist_price_updates(db, updates)?;
    }
    Ok(())
}

pub(super) fn persist_price_updates(
    db: &mut UsageDb,
    updates: Vec<(&String, Observation)>,
) -> AppResult<()> {
    if updates.len() > 500 {
        return Err(invalid("Price binding batch exceeds 500 records"));
    }
    let tx = db.conn.transaction().map_err(db_error)?;
    for (original, r) in updates {
        let changed=tx.execute("UPDATE usage_records SET canonical_model_id=?1,price_snapshot_id=?2,estimate_usd=?3,estimate_status=?4 WHERE id=?5 AND price_snapshot_id IS NULL AND EXISTS(SELECT 1 FROM usage_observation_details WHERE record_id=?5 AND payload=?6)",params![r.canonical_model,r.price_snapshot_id,r.estimate_usd.map(|v|v.to_string()),r.estimate_detail,r.id,original]).map_err(db_error)?;
        if changed == 0 {
            continue;
        }
        tx.execute(
            "UPDATE usage_observation_details SET payload=?1 WHERE record_id=?2",
            params![store::encode(&r)?, r.id],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)
}

pub fn bind(
    db: &UsageDb,
    r: &mut Observation,
    old: Option<&Observation>,
    current: Option<&PriceCatalog>,
) -> AppResult<()> {
    let historical: Option<PriceCatalog> =
        if let Some(id) = old.and_then(|r| r.price_snapshot_id.as_ref()) {
            db.conn
                .query_row(
                    "SELECT payload FROM price_catalog_payloads WHERE snapshot_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_error)?
                .map(store::decode)
                .transpose()?
        } else {
            None
        };
    let Some(catalog) = historical.as_ref().or(current) else {
        return Ok(());
    };
    let canonical = if catalog.models.iter().any(|m| m.model == r.raw_model) {
        Some(r.raw_model.clone())
    } else {
        catalog.aliases.get(&r.raw_model).cloned()
    };
    r.canonical_model = canonical.clone();
    let Some(model) = catalog
        .models
        .iter()
        .find(|m| Some(&m.model) == canonical.as_ref())
    else {
        r.estimate_detail =
            "Unresolved model: no exact catalog identity or unique versioned alias".into();
        return Ok(());
    };
    r.price_snapshot_id = Some(catalog.id.clone());
    match estimate(r, model) {
        Ok(amount) => {
            r.estimate_usd = Some(amount);
            r.estimate_detail = "Catalog-price comparison (text tokens only)".into();
        }
        Err(detail) => {
            r.estimate_usd = None;
            r.estimate_detail = detail.into();
        }
    }
    if catalog.fetched_at > r.finished_at {
        r.quality.push("Price was fetched after this usage; this is an API-equivalent comparison, not a historical charge".into());
    }
    Ok(())
}

pub fn estimate(r: &Observation, model: &ModelPrice) -> Result<ExactDecimal, &'static str> {
    if !model.supported {
        return Err("Unsupported pricing schema");
    }
    if model.tiers.len() > 1 && r.counter_kind != CounterKind::RequestDelta {
        return Err("Context pricing requires per-request input; session aggregates are ambiguous");
    }
    let total = r
        .normalized_input
        .ok_or("Input semantics are unknown or inconsistent")?;
    let tier = model
        .tiers
        .iter()
        .rev()
        .find(|t| total >= t.min_input)
        .ok_or("No matching price tier")?;
    if r.cache_write.is_some_and(|n| n > 0)
        && tier.cache_write_1h.is_some()
        && tier.cache_write_1h != tier.cache_write
    {
        return Err("Cache retention is unknown; 5-minute and 1-hour write prices differ");
    }
    let buckets = [
        (r.fresh_input, Some(tier.input)),
        (r.cache_read, tier.cache_read),
        (r.cache_write, tier.cache_write),
        (r.output, Some(tier.output)),
    ];
    let mut sum = Decimal::ZERO;
    for (count, rate) in buckets {
        let count = count.ok_or("Token bucket is unknown; estimate coverage is incomplete")?;
        if count == 0 {
            continue;
        }
        let rate = rate.ok_or("Token rate is unknown; estimate coverage is incomplete")?;
        let cost = rate
            .get()
            .checked_mul(Decimal::from(count))
            .and_then(|v| v.checked_div(Decimal::from(1_000_000)))
            .ok_or("Estimate exceeds decimal range")?;
        sum = sum
            .checked_add(cost)
            .ok_or("Estimate exceeds decimal range")?;
    }
    Ok(ExactDecimal::new(sum))
}
