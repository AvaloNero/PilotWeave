use crate::decimal::ExactDecimal;
use crate::error::{AppError, AppResult};
use crate::github_billing::{
    GithubBillingCoverage, GithubBillingEndpointFamily, GithubBillingFamilyView,
    GithubBillingItem, GithubBillingSnapshot, GithubBillingSnapshotStatus,
    MAX_BILLING_ITEMS_PER_SNAPSHOT, MAX_BILLING_ITEMS_RETURNED,
};
use crate::usage_db::UsageDb;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection as SqliteConnection, OptionalExtension};
use std::collections::HashSet;

const MAX_BILLING_SNAPSHOTS_PER_FAMILY: usize = 120;
const MAX_TEXT_FIELD: usize = 512;

pub fn insert_snapshots(
    db: &mut UsageDb,
    snapshots: &[GithubBillingSnapshot],
) -> AppResult<()> {
    if snapshots.is_empty() || snapshots.len() > GithubBillingEndpointFamily::ALL.len() {
        return Err(AppError::InvalidInput(
            "A personal Billing refresh must contain one or two endpoint snapshots".to_string(),
        ));
    }

    let first = &snapshots[0];
    let mut ids = HashSet::new();
    let mut families = HashSet::new();
    for snapshot in snapshots {
        validate_snapshot(snapshot)?;
        if !ids.insert(snapshot.id.as_str()) {
            return Err(AppError::InvalidInput(
                "A personal Billing refresh contains duplicate snapshot IDs".to_string(),
            ));
        }
        if !families.insert(snapshot.endpoint_family) {
            return Err(AppError::InvalidInput(
                "A personal Billing refresh contains duplicate endpoint families".to_string(),
            ));
        }
        if snapshot.account_hint != first.account_hint
            || snapshot.period_start != first.period_start
            || snapshot.period_end != first.period_end
        {
            return Err(AppError::InvalidInput(
                "A personal Billing refresh must use one account and period".to_string(),
            ));
        }
    }

    let tx = db.conn.transaction().map_err(sqlite_error)?;
    for snapshot in snapshots {
        tx.execute(
            "INSERT INTO github_billing_snapshots
                (id, account_hint, endpoint_family, api_version, period_start, period_end,
                 fetched_at, coverage, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                snapshot.id,
                snapshot.account_hint,
                snapshot.endpoint_family.as_str(),
                snapshot.api_version,
                snapshot.period_start.to_rfc3339(),
                snapshot.period_end.to_rfc3339(),
                snapshot.fetched_at.to_rfc3339(),
                snapshot.coverage.as_str(),
                snapshot.status.as_str(),
                snapshot.error,
            ],
        )
        .map_err(sqlite_error)?;

        for item in &snapshot.items {
            tx.execute(
                "INSERT INTO github_billing_items
                    (id, snapshot_id, product, sku, model, quantity, unit,
                     gross_amount_usd, discount_amount_usd, net_amount_usd,
                     allowance, remaining, reset_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    item.id,
                    snapshot.id,
                    item.product,
                    item.sku,
                    item.model,
                    item.quantity.to_string(),
                    item.unit,
                    item.gross_amount_usd.map(|value| value.to_string()),
                    item.discount_amount_usd.map(|value| value.to_string()),
                    item.net_amount_usd.map(|value| value.to_string()),
                    item.allowance.map(|value| value.to_string()),
                    item.remaining.map(|value| value.to_string()),
                    item.reset_at.map(|value| value.to_rfc3339()),
                ],
            )
            .map_err(sqlite_error)?;
        }

        tx.execute(
            "DELETE FROM github_billing_snapshots
             WHERE id IN (
                SELECT id FROM github_billing_snapshots
                WHERE account_hint = ?1 AND endpoint_family = ?2
                ORDER BY fetched_at DESC, rowid DESC
                LIMIT -1 OFFSET ?3
             )",
            params![
                snapshot.account_hint,
                snapshot.endpoint_family.as_str(),
                MAX_BILLING_SNAPSHOTS_PER_FAMILY as i64,
            ],
        )
        .map_err(sqlite_error)?;
    }
    tx.commit().map_err(sqlite_error)
}

pub fn family_views(
    db: &UsageDb,
    account_hint: &str,
) -> AppResult<Vec<GithubBillingFamilyView>> {
    check_text("Billing account", account_hint)?;
    GithubBillingEndpointFamily::ALL
        .into_iter()
        .map(|endpoint_family| {
            let latest = load_snapshot(&db.conn, account_hint, endpoint_family, false)?;
            let last_successful = if latest
                .as_ref()
                .is_some_and(|snapshot| !snapshot.status.is_success())
            {
                load_snapshot(&db.conn, account_hint, endpoint_family, true)?
            } else {
                None
            };
            Ok(GithubBillingFamilyView {
                endpoint_family,
                latest,
                last_successful,
            })
        })
        .collect()
}

fn validate_snapshot(snapshot: &GithubBillingSnapshot) -> AppResult<()> {
    check_text("Billing snapshot id", &snapshot.id)?;
    check_text("Billing account", &snapshot.account_hint)?;
    check_text("Billing API version", &snapshot.api_version)?;
    if snapshot.period_end <= snapshot.period_start {
        return Err(AppError::InvalidInput(
            "Billing snapshot period end must be after its start".to_string(),
        ));
    }
    if snapshot.items.len() > MAX_BILLING_ITEMS_PER_SNAPSHOT {
        return Err(AppError::InvalidInput(format!(
            "Billing snapshot contains more than {MAX_BILLING_ITEMS_PER_SNAPSHOT} items"
        )));
    }
    if snapshot.items_truncated || snapshot.total_item_count != snapshot.items.len() as u64 {
        return Err(AppError::InvalidInput(
            "Only complete native Billing snapshots may be persisted".to_string(),
        ));
    }
    match snapshot.status {
        GithubBillingSnapshotStatus::Available if snapshot.items.is_empty() => {
            return Err(AppError::InvalidInput(
                "An available Billing snapshot must contain at least one item".to_string(),
            ));
        }
        GithubBillingSnapshotStatus::SuccessfulEmpty if !snapshot.items.is_empty() => {
            return Err(AppError::InvalidInput(
                "A successful-empty Billing snapshot must not contain items".to_string(),
            ));
        }
        status if !status.is_success() && !snapshot.items.is_empty() => {
            return Err(AppError::InvalidInput(
                "A failed Billing observation must not contain usage items".to_string(),
            ));
        }
        _ => {}
    }
    if snapshot.status.is_success() != snapshot.error.is_none() {
        return Err(AppError::InvalidInput(
            "Billing snapshot status and safe error detail disagree".to_string(),
        ));
    }
    if let Some(error) = &snapshot.error {
        check_text("Billing error", error)?;
    }

    let mut item_ids = HashSet::new();
    for item in &snapshot.items {
        check_text("Billing item id", &item.id)?;
        if !item_ids.insert(item.id.as_str()) {
            return Err(AppError::InvalidInput(
                "Billing snapshot contains duplicate item IDs".to_string(),
            ));
        }
        check_optional_text("Billing product", item.product.as_deref())?;
        check_optional_text("Billing SKU", item.sku.as_deref())?;
        check_optional_text("Billing model", item.model.as_deref())?;
        check_text("Billing unit", &item.unit)?;
        validate_non_negative("Billing quantity", item.quantity)?;
        for (label, value) in [
            ("Billing gross amount", item.gross_amount_usd),
            ("Billing discount amount", item.discount_amount_usd),
            ("Billing net amount", item.net_amount_usd),
            ("Billing allowance", item.allowance),
            ("Billing remaining", item.remaining),
        ] {
            if let Some(value) = value {
                validate_non_negative(label, value)?;
            }
        }
    }
    Ok(())
}

fn load_snapshot(
    conn: &SqliteConnection,
    account_hint: &str,
    endpoint_family: GithubBillingEndpointFamily,
    successful_only: bool,
) -> AppResult<Option<GithubBillingSnapshot>> {
    let sql = if successful_only {
        "SELECT id, account_hint, endpoint_family, api_version, period_start, period_end,
                fetched_at, coverage, status, error
         FROM github_billing_snapshots
         WHERE account_hint = ?1 AND endpoint_family = ?2
           AND status IN ('available', 'successful-empty')
         ORDER BY fetched_at DESC, rowid DESC
         LIMIT 1"
    } else {
        "SELECT id, account_hint, endpoint_family, api_version, period_start, period_end,
                fetched_at, coverage, status, error
         FROM github_billing_snapshots
         WHERE account_hint = ?1 AND endpoint_family = ?2
         ORDER BY fetched_at DESC, rowid DESC
         LIMIT 1"
    };

    let row = conn
        .query_row(sql, params![account_hint, endpoint_family.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .optional()
        .map_err(sqlite_error)?;

    let Some((
        id,
        account_hint,
        stored_family,
        api_version,
        period_start,
        period_end,
        fetched_at,
        coverage,
        status,
        error,
    )) = row
    else {
        return Ok(None);
    };

    let endpoint_family = GithubBillingEndpointFamily::from_str(&stored_family)
        .ok_or_else(|| AppError::Config("Stored Billing endpoint family is invalid".to_string()))?;
    let coverage = GithubBillingCoverage::from_str(&coverage)
        .ok_or_else(|| AppError::Config("Stored Billing coverage is invalid".to_string()))?;
    let status = GithubBillingSnapshotStatus::from_str(&status)
        .ok_or_else(|| AppError::Config("Stored Billing status is invalid".to_string()))?;
    let period_start = parse_datetime("Billing period start", &period_start)?;
    let period_end = parse_datetime("Billing period end", &period_end)?;
    let fetched_at = parse_datetime("Billing fetched time", &fetched_at)?;
    let total_item_count = conn
        .query_row(
            "SELECT count(*) FROM github_billing_items WHERE snapshot_id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?
        .max(0) as u64;
    let items = load_items(conn, &id)?;
    Ok(Some(GithubBillingSnapshot {
        id,
        account_hint,
        endpoint_family,
        api_version,
        period_start,
        period_end,
        fetched_at,
        coverage,
        status,
        error,
        items_truncated: total_item_count > items.len() as u64,
        total_item_count,
        items,
    }))
}

fn load_items(conn: &SqliteConnection, snapshot_id: &str) -> AppResult<Vec<GithubBillingItem>> {
    type StoredItem = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    let mut statement = conn
        .prepare(
            "SELECT id, product, sku, model, quantity, unit,
                    gross_amount_usd, discount_amount_usd, net_amount_usd,
                    allowance, remaining, reset_at
             FROM github_billing_items
             WHERE snapshot_id = ?1
             ORDER BY rowid
             LIMIT ?2",
        )
        .map_err(sqlite_error)?;
    let stored = statement
        .query_map(
            params![snapshot_id, MAX_BILLING_ITEMS_RETURNED as i64],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .map_err(sqlite_error)?
        .collect::<Result<Vec<StoredItem>, _>>()
        .map_err(sqlite_error)?;

    stored
        .into_iter()
        .map(
            |(
                id,
                product,
                sku,
                model,
                quantity,
                unit,
                gross,
                discount,
                net,
                allowance,
                remaining,
                reset_at,
            )| {
                Ok(GithubBillingItem {
                    id,
                    product,
                    sku,
                    model,
                    quantity: parse_decimal("Billing quantity", &quantity)?,
                    unit,
                    gross_amount_usd: gross
                        .map(|value| parse_decimal("Billing gross amount", &value))
                        .transpose()?,
                    discount_amount_usd: discount
                        .map(|value| parse_decimal("Billing discount amount", &value))
                        .transpose()?,
                    net_amount_usd: net
                        .map(|value| parse_decimal("Billing net amount", &value))
                        .transpose()?,
                    allowance: allowance
                        .map(|value| parse_decimal("Billing allowance", &value))
                        .transpose()?,
                    remaining: remaining
                        .map(|value| parse_decimal("Billing remaining", &value))
                        .transpose()?,
                    reset_at: reset_at
                        .map(|value| parse_datetime("Billing reset time", &value))
                        .transpose()?,
                })
            },
        )
        .collect()
}

fn check_text(label: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_TEXT_FIELD
        || value.chars().any(char::is_control)
    {
        return Err(AppError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn check_optional_text(label: &str, value: Option<&str>) -> AppResult<()> {
    match value {
        Some(value) => check_text(label, value),
        None => Ok(()),
    }
}

fn validate_non_negative(label: &str, value: ExactDecimal) -> AppResult<()> {
    if value < ExactDecimal::ZERO {
        return Err(AppError::InvalidInput(format!(
            "{label} must not be negative"
        )));
    }
    Ok(())
}

fn parse_decimal(label: &str, value: &str) -> AppResult<ExactDecimal> {
    ExactDecimal::parse(value)
        .ok_or_else(|| AppError::Config(format!("Stored {label} is not an exact decimal")))
}

fn parse_datetime(label: &str, value: &str) -> AppResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Config(format!("Stored {label} is invalid")))
}

fn sqlite_error(error: rusqlite::Error) -> AppError {
    AppError::Config(format!("Usage database error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use uuid::Uuid;

    fn item(id: &str) -> GithubBillingItem {
        GithubBillingItem {
            id: id.to_string(),
            product: Some("Copilot".to_string()),
            sku: Some("Copilot Premium Request".to_string()),
            model: Some("GPT-5".to_string()),
            quantity: ExactDecimal::parse("100.125").expect("quantity"),
            unit: "requests".to_string(),
            gross_amount_usd: Some(ExactDecimal::parse("4.005").expect("gross")),
            discount_amount_usd: Some(ExactDecimal::parse("0.005").expect("discount")),
            net_amount_usd: Some(ExactDecimal::parse("4.000").expect("net")),
            allowance: None,
            remaining: None,
            reset_at: None,
        }
    }

    fn snapshot(
        id: &str,
        family: GithubBillingEndpointFamily,
        status: GithubBillingSnapshotStatus,
        fetched_at: DateTime<Utc>,
    ) -> GithubBillingSnapshot {
        let successful_items = match status {
            GithubBillingSnapshotStatus::Available => vec![item(&format!("{id}-item"))],
            _ => Vec::new(),
        };
        let successful = status.is_success();
        GithubBillingSnapshot {
            id: id.to_string(),
            account_hint: "octocat".to_string(),
            endpoint_family: family,
            api_version: "2026-03-10".to_string(),
            period_start: fetched_at - Duration::days(1),
            period_end: fetched_at + Duration::days(30),
            fetched_at,
            coverage: if status == GithubBillingSnapshotStatus::NotCovered {
                GithubBillingCoverage::NotCovered
            } else if successful {
                GithubBillingCoverage::PersonalAccountOnly
            } else {
                GithubBillingCoverage::Unknown
            },
            status,
            error: (!successful).then(|| "safe failure detail".to_string()),
            total_item_count: successful_items.len() as u64,
            items: successful_items,
            items_truncated: false,
        }
    }

    #[test]
    fn failed_refresh_keeps_the_last_successful_snapshot_visible() {
        let directory = tempfile::tempdir().expect("directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let now = Utc::now();
        insert_snapshots(
            &mut db,
            &[snapshot(
                "success",
                GithubBillingEndpointFamily::PremiumRequest,
                GithubBillingSnapshotStatus::Available,
                now,
            )],
        )
        .expect("insert success");
        insert_snapshots(
            &mut db,
            &[snapshot(
                "failure",
                GithubBillingEndpointFamily::PremiumRequest,
                GithubBillingSnapshotStatus::NetworkError,
                now + Duration::seconds(1),
            )],
        )
        .expect("insert failure");

        let views = family_views(&db, "octocat").expect("views");
        let premium = views
            .iter()
            .find(|view| {
                view.endpoint_family == GithubBillingEndpointFamily::PremiumRequest
            })
            .expect("premium");
        assert_eq!(
            premium.latest.as_ref().expect("latest").status,
            GithubBillingSnapshotStatus::NetworkError
        );
        let successful = premium.last_successful.as_ref().expect("last success");
        assert_eq!(successful.id, "success");
        assert_eq!(
            successful.items[0]
                .net_amount_usd
                .expect("net")
                .to_string(),
            "4.000"
        );
    }

    #[test]
    fn endpoint_families_are_persisted_and_queried_separately() {
        let directory = tempfile::tempdir().expect("directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let now = Utc::now();
        insert_snapshots(
            &mut db,
            &[
                snapshot(
                    "credit",
                    GithubBillingEndpointFamily::AiCredit,
                    GithubBillingSnapshotStatus::SuccessfulEmpty,
                    now,
                ),
                snapshot(
                    "premium",
                    GithubBillingEndpointFamily::PremiumRequest,
                    GithubBillingSnapshotStatus::Available,
                    now,
                ),
            ],
        )
        .expect("insert");

        let views = family_views(&db, "octocat").expect("views");
        assert_eq!(views.len(), 2);
        assert_eq!(
            views[0].latest.as_ref().expect("credit").endpoint_family,
            GithubBillingEndpointFamily::AiCredit
        );
        assert_eq!(
            views[1]
                .latest
                .as_ref()
                .expect("premium")
                .endpoint_family,
            GithubBillingEndpointFamily::PremiumRequest
        );
    }

    #[test]
    fn returned_items_are_bounded_without_losing_the_authoritative_count() {
        let directory = tempfile::tempdir().expect("directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let now = Utc::now();
        let mut value = snapshot(
            "many",
            GithubBillingEndpointFamily::PremiumRequest,
            GithubBillingSnapshotStatus::Available,
            now,
        );
        value.items = (0..=MAX_BILLING_ITEMS_RETURNED)
            .map(|index| item(&format!("item-{index}")))
            .collect();
        value.total_item_count = value.items.len() as u64;
        insert_snapshots(&mut db, &[value]).expect("insert");

        let latest = family_views(&db, "octocat").expect("views")[1]
            .latest
            .clone()
            .expect("latest");
        assert_eq!(latest.items.len(), MAX_BILLING_ITEMS_RETURNED);
        assert_eq!(
            latest.total_item_count,
            (MAX_BILLING_ITEMS_RETURNED + 1) as u64
        );
        assert!(latest.items_truncated);
    }

    #[test]
    fn invalid_batch_is_rejected_before_any_snapshot_is_written() {
        let directory = tempfile::tempdir().expect("directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let now = Utc::now();
        let valid = snapshot(
            "valid",
            GithubBillingEndpointFamily::AiCredit,
            GithubBillingSnapshotStatus::SuccessfulEmpty,
            now,
        );
        let mut invalid = snapshot(
            "invalid",
            GithubBillingEndpointFamily::PremiumRequest,
            GithubBillingSnapshotStatus::Available,
            now,
        );
        invalid.items[0].quantity = ExactDecimal::parse("-1").expect("negative");
        assert!(insert_snapshots(&mut db, &[valid, invalid]).is_err());
        let count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM github_billing_snapshots", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn snapshot_ids_are_immutable() {
        let directory = tempfile::tempdir().expect("directory");
        let mut db = UsageDb::open_at(&directory.path().join("usage.sqlite3")).expect("open");
        let value = snapshot(
            "immutable",
            GithubBillingEndpointFamily::AiCredit,
            GithubBillingSnapshotStatus::SuccessfulEmpty,
            Utc::now(),
        );
        insert_snapshots(&mut db, &[value.clone()]).expect("first insert");
        assert!(insert_snapshots(&mut db, &[value]).is_err());
    }

    #[test]
    fn generated_item_ids_are_accepted() {
        let mut value = item(&Uuid::new_v4().to_string());
        value.quantity = ExactDecimal::ZERO;
        validate_non_negative("quantity", value.quantity).expect("zero is valid");
    }
}
