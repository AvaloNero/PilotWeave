use super::{bounded_id, db_error, invalid, store, types::*};
use crate::{
    decimal::ExactDecimal,
    error::AppResult,
    usage_db::{InputSemantics, UsageDb},
};
use chrono::{Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};

pub fn default_query() -> UsageQuery {
    let today = Utc::now().date_naive();
    UsageQuery {
        start: (today - Duration::days(29)).to_string(),
        end: today.to_string(),
        surface: None,
        route: None,
        connection_id: None,
        model: None,
        confidence: None,
        source_id: None,
        source_status: None,
        page: Some(0),
        page_size: Some(50),
    }
}

pub fn validate(q: &UsageQuery) -> AppResult<(String, String)> {
    let start = NaiveDate::parse_from_str(&q.start, "%Y-%m-%d")
        .map_err(|_| invalid("Usage start must be YYYY-MM-DD"))?;
    let end = NaiveDate::parse_from_str(&q.end, "%Y-%m-%d")
        .map_err(|_| invalid("Usage end must be YYYY-MM-DD"))?;
    if end < start
        || (end - start).num_days() > 365
        || end > Utc::now().date_naive() + Duration::days(1)
        || q.page.unwrap_or(0) > 10_000
        || !(1..=100).contains(&q.page_size.unwrap_or(50))
    {
        return Err(invalid(
            "Usage queries allow at most 366 days and 100 rows per page",
        ));
    }
    if let Some(model) = &q.model {
        super::bounded_model_id(model)?;
    }
    for value in [&q.surface, &q.connection_id, &q.source_id]
        .into_iter()
        .flatten()
    {
        bounded_id(value)?;
    }
    Ok((
        Utc.from_utc_datetime(&start.and_hms_opt(0, 0, 0).unwrap())
            .to_rfc3339(),
        Utc.from_utc_datetime(&(end + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap())
            .to_rfc3339(),
    ))
}

pub fn overview(db: &UsageDb, q: &UsageQuery) -> AppResult<UsageOverview> {
    let (start, end) = validate(q)?;
    let mut stmt=db.conn.prepare("SELECT d.payload FROM usage_observation_details d JOIN usage_records r ON r.id=d.record_id JOIN usage_sources s ON s.id=r.source_id WHERE r.finished_at>=?1 AND r.finished_at<?2 AND (?3 IS NULL OR (CASE WHEN s.enabled=0 THEN '\"disabled\"' ELSE s.status END)=?3) ORDER BY r.finished_at DESC,r.id LIMIT 100001").map_err(db_error)?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                start,
                end,
                q.source_status.map(|s| store::encode(&s)).transpose()?
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(db_error)?;
    let mut records = vec![];
    for (i, row) in rows.enumerate() {
        if i == 100_000 {
            return Err(invalid(
                "Usage range exceeds 100,000 records; choose a smaller range",
            ));
        }
        let r: Observation = store::decode(row.map_err(db_error)?)?;
        if q.surface.as_ref().is_some_and(|v| !r.surfaces.contains(v))
            || q.route.is_some_and(|v| r.route != v)
            || q.connection_id
                .as_ref()
                .is_some_and(|v| r.connection_id.as_ref() != Some(v))
            || q.model
                .as_ref()
                .is_some_and(|v| r.raw_model != *v && r.canonical_model.as_ref() != Some(v))
            || q.confidence.is_some_and(|v| r.confidence != v)
            || q.source_id.as_ref().is_some_and(|v| r.source_id != *v)
        {
            continue;
        }
        records.push(r);
    }
    summarize(records, q)
}

fn sum_optional<'a>(
    records: impl Iterator<Item = &'a Observation>,
    field: impl Fn(&Observation) -> Option<u64>,
) -> Option<u64> {
    let mut any = false;
    let mut total = 0u64;
    for r in records {
        any = true;
        total = total.checked_add(field(r)?)?;
    }
    any.then_some(total)
}

pub fn totals(records: &[&Observation]) -> AppResult<Totals> {
    let mut result = Totals {
        records: records.len(),
        requests: sum_optional(records.iter().copied(), |r| r.request_count),
        input_reported: sum_optional(records.iter().copied(), |r| r.input_reported),
        normalized_input: sum_optional(records.iter().copied(), |r| r.normalized_input),
        fresh_input: sum_optional(records.iter().copied(), |r| r.fresh_input),
        cache_read: sum_optional(records.iter().copied(), |r| r.cache_read),
        cache_write: sum_optional(records.iter().copied(), |r| r.cache_write),
        output: sum_optional(records.iter().copied(), |r| r.output),
        ..Totals::default()
    };
    let covered: Vec<_> = records
        .iter()
        .copied()
        .filter(|r| r.normalized_input.is_some() && r.cache_read.is_some())
        .collect();
    result.cache_covered_records = covered.len();
    let numerator = sum_optional(covered.iter().copied(), |r| r.cache_read);
    let denominator = sum_optional(covered.iter().copied(), |r| r.normalized_input);
    result.cache_hit_rate = numerator
        .zip(denominator)
        .filter(|(_, d)| *d > 0)
        .and_then(|(n, d)| Decimal::from(n).checked_div(Decimal::from(d)))
        .map(ExactDecimal::new);
    let mut sum = ExactDecimal::ZERO;
    let mut snapshots = BTreeSet::new();
    let mut models = BTreeSet::new();
    for r in records {
        if let Some(value) = r.estimate_usd {
            sum = sum
                .checked_add(value)
                .ok_or_else(|| invalid("Aggregate estimate exceeds decimal range"))?;
            result.priced_records += 1;
        }
        if let Some(id) = &r.price_snapshot_id {
            snapshots.insert(id.clone());
        }
        if r.canonical_model.is_none() {
            models.insert(r.raw_model.clone());
        }
        if r.input_semantics == InputSemantics::Unknown {
            result.unknown_semantics += 1;
        }
    }
    result.unresolved_models = models.len();
    result.price_snapshots = snapshots.into_iter().collect();
    result.unpriced_records = records.len() - result.priced_records;
    result.priced_requests = sum_optional(
        records.iter().copied().filter(|r| r.estimate_usd.is_some()),
        |r| r.request_count,
    );
    result.unpriced_requests = sum_optional(
        records.iter().copied().filter(|r| r.estimate_usd.is_none()),
        |r| r.request_count,
    );
    result.estimate_usd = (result.priced_records > 0).then_some(sum);
    result.priced_tokens = sum_optional(
        records.iter().copied().filter(|r| r.estimate_usd.is_some()),
        |r| r.normalized_input?.checked_add(r.output?),
    );
    result.unpriced_tokens = sum_optional(
        records.iter().copied().filter(|r| r.estimate_usd.is_none()),
        |r| r.normalized_input?.checked_add(r.output?),
    );
    Ok(result)
}

fn group(
    records: &[Observation],
    key: impl Fn(&Observation) -> String,
) -> AppResult<Vec<Breakdown>> {
    let mut groups: BTreeMap<String, Vec<&Observation>> = BTreeMap::new();
    for r in records {
        groups.entry(key(r)).or_default().push(r);
    }
    if groups.len() > 4096 {
        return Err(invalid("Too many usage groups; narrow the date range"));
    }
    groups
        .into_iter()
        .map(|(key, rows)| {
            Ok(Breakdown {
                key,
                totals: totals(&rows)?,
            })
        })
        .collect()
}

pub fn summarize(records: Vec<Observation>, q: &UsageQuery) -> AppResult<UsageOverview> {
    let all = totals(&records.iter().collect::<Vec<_>>())?;
    let status = if records.is_empty() {
        DataStatus::SuccessfulEmpty
    } else if all.unpriced_records > 0 || all.cache_covered_records < records.len() {
        DataStatus::Partial
    } else {
        DataStatus::Available
    };
    Ok(UsageOverview {status,detail:"Only imported observations in the selected UTC date range. Missing buckets remain unknown; cache rate uses summed tokens over records with known total input and cache read. Session totals are assigned to their shutdown date. Estimates cover priced records only; shared-runtime rows are counted once.".into(),start:q.start.clone(),end:q.end.clone(),totals:all,
        models:model_statistics(&records)?,days:group(&records,|r|r.finished_at.date_naive().to_string())?,
        routes:group(&records,|r|format!("{:?}",r.route))?,surfaces:group(&records,|r|r.surfaces.join(", "))?,
        providers:group(&records,|r|r.provider.clone().unwrap_or_else(||"Unknown".into()))?,
        coverage_start:records.iter().map(|r|r.started_at).min(),coverage_end:records.iter().map(|r|r.finished_at).max(),last_imported:records.iter().map(|r|r.imported_at).max(),
        total_records:records.len(),records:records.into_iter().skip(q.page.unwrap_or(0) as usize*q.page_size.unwrap_or(50) as usize).take(q.page_size.unwrap_or(50) as usize).collect()})
}

fn model_statistics(records: &[Observation]) -> AppResult<Vec<ModelStatistics>> {
    let mut groups: BTreeMap<String, Vec<&Observation>> = BTreeMap::new();
    for r in records {
        groups
            .entry(format!("{}:{:?}:{:?}", r.raw_model, r.route, r.confidence))
            .or_default()
            .push(r);
    }
    if groups.len() > 4096 {
        return Err(invalid("Too many model groups; narrow the range"));
    }
    groups
        .into_values()
        .map(|rows| {
            let first = rows[0];
            let canonical = rows
                .iter()
                .all(|r| r.canonical_model == first.canonical_model)
                .then(|| first.canonical_model.clone())
                .flatten();
            Ok(ModelStatistics {
                raw_model: first.raw_model.clone(),
                canonical_model: canonical,
                route: first.route,
                confidence: first.confidence,
                surfaces: rows
                    .iter()
                    .flat_map(|r| r.surfaces.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                quality: rows
                    .iter()
                    .flat_map(|r| r.quality.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                totals: totals(&rows)?,
            })
        })
        .collect()
}
