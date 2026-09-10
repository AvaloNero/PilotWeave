use super::{db_error, invalid, types::*};
use crate::error::AppResult;
use crate::usage_db::UsageDb;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

pub const MIGRATION_V2: &str = "
CREATE TABLE usage_file_cursors (
 source_id TEXT NOT NULL REFERENCES usage_sources(id), file_key TEXT NOT NULL,
 cursor_json TEXT NOT NULL, PRIMARY KEY(source_id,file_key));
CREATE TABLE usage_observation_details (
 record_id TEXT PRIMARY KEY REFERENCES usage_records(id) ON DELETE CASCADE,
 payload TEXT NOT NULL);
CREATE INDEX usage_records_time ON usage_records(finished_at, source_id);
CREATE TABLE usage_jobs (id TEXT PRIMARY KEY, source_id TEXT NOT NULL, status TEXT NOT NULL, payload TEXT NOT NULL);
CREATE TABLE runtime_usage_payloads (
 snapshot_id TEXT PRIMARY KEY REFERENCES official_quota_snapshots(id) ON DELETE CASCADE, payload TEXT NOT NULL);
CREATE TABLE price_catalog_payloads (
 snapshot_id TEXT PRIMARY KEY REFERENCES price_snapshots(id), payload TEXT NOT NULL);
CREATE TABLE setup_preferences (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
ALTER TABLE github_billing_snapshots ADD COLUMN retry_at TEXT;
";

// One backend-owned catalog source. The pointer tracks successful observations;
// content-addressed snapshots and their original provenance remain immutable.
pub const MIGRATION_V3: &str = "
CREATE TABLE current_price_catalog (
 id INTEGER PRIMARY KEY CHECK(id=1),
 snapshot_id TEXT NOT NULL REFERENCES price_catalog_payloads(snapshot_id),
 checked_at_ms INTEGER NOT NULL);
INSERT INTO current_price_catalog (id,snapshot_id,checked_at_ms)
 SELECT 1,s.id,CAST(strftime('%s',s.fetched_at) AS INTEGER)*1000
   + CAST(substr(strftime('%f',s.fetched_at),4,3) AS INTEGER)
 FROM price_snapshots s JOIN price_catalog_payloads p ON p.snapshot_id=s.id
 ORDER BY julianday(s.fetched_at) DESC,s.rowid DESC LIMIT 1;
";

pub fn encode(value: &impl serde::Serialize) -> AppResult<String> {
    serde_json::to_string(value).map_err(|_| invalid("Could not encode usage metadata"))
}
pub fn decode<T: serde::de::DeserializeOwned>(value: String) -> AppResult<T> {
    serde_json::from_str(&value)
        .map_err(|_| invalid("Stored usage metadata has an unsupported schema"))
}

fn sql_count(value: Option<u64>) -> AppResult<Option<i64>> {
    value
        .map(i64::try_from)
        .transpose()
        .map_err(|_| invalid("Usage counter exceeds SQLite range"))
}

pub fn cursor(db: &UsageDb, source: &str, file: &str) -> AppResult<Option<FileCursor>> {
    db.conn
        .query_row(
            "SELECT cursor_json FROM usage_file_cursors WHERE source_id=?1 AND file_key=?2",
            params![source, file],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .map(decode)
        .transpose()
}

/// A file batch, its derivations, surfaces and cursor commit together. Failure
/// cannot advance the cursor over observations that were not persisted.
pub fn commit_batch(
    db: &mut UsageDb,
    source: &str,
    file: &str,
    cursor: &FileCursor,
    records: &[Observation],
) -> AppResult<()> {
    if records.len() > 500 {
        return Err(invalid("Usage batch exceeds 500 records"));
    }
    let tx = db.conn.transaction().map_err(db_error)?;
    for record in records {
        if record.source_id != source {
            return Err(invalid("Usage source mismatch"));
        }
        let stored: Option<String> = tx
            .query_row(
                "SELECT payload FROM usage_observation_details WHERE record_id=?1",
                [&record.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(old) = stored.map(decode::<Observation>).transpose()? {
            if old.finished_at > record.finished_at {
                continue;
            }
            if old.price_snapshot_id.is_some() && old.price_snapshot_id != record.price_snapshot_id
            {
                return Err(invalid(
                    "Price binding changed during import; sync again from the retained cursor",
                ));
            }
        }
        // Repeated cumulative snapshots preserve the originally bound price.
        tx.execute("INSERT INTO usage_records
          (id,source_id,source_record_key,session_hash,request_hash,raw_model,canonical_model_id,route,attribution_confidence,input_semantics,
           input_reported,fresh_input,cache_read,cache_write,output,started_at,finished_at,source_created_at,imported_at,price_snapshot_id,estimate_usd,estimate_status,quality_flags)
          VALUES (?1,?2,?1,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?15,?17,?18,?19,?20,?21)
          ON CONFLICT(id) DO UPDATE SET input_reported=excluded.input_reported,fresh_input=excluded.fresh_input,cache_read=excluded.cache_read,
           cache_write=excluded.cache_write,output=excluded.output,finished_at=excluded.finished_at,imported_at=excluded.imported_at,
           canonical_model_id=excluded.canonical_model_id,price_snapshot_id=excluded.price_snapshot_id,estimate_usd=excluded.estimate_usd,
           estimate_status=excluded.estimate_status,quality_flags=excluded.quality_flags",
          params![record.id,source,record.session_hash,record.request_hash,record.raw_model,record.canonical_model,encode(&record.route)?,encode(&record.confidence)?,record.input_semantics.as_str(),
            sql_count(record.input_reported)?,sql_count(record.fresh_input)?,sql_count(record.cache_read)?,sql_count(record.cache_write)?,sql_count(record.output)?,record.started_at.to_rfc3339(),record.finished_at.to_rfc3339(),record.imported_at.to_rfc3339(),
            record.price_snapshot_id,record.estimate_usd.map(|v|v.to_string()),record.estimate_detail,encode(&record.quality)?]).map_err(db_error)?;
        tx.execute("INSERT INTO usage_observation_details VALUES (?1,?2) ON CONFLICT(record_id) DO UPDATE SET payload=excluded.payload",params![record.id,encode(record)?]).map_err(db_error)?;
        tx.execute(
            "DELETE FROM usage_record_surfaces WHERE record_id=?1",
            [&record.id],
        )
        .map_err(db_error)?;
        for surface in &record.surfaces {
            tx.execute(
                "INSERT INTO usage_record_surfaces VALUES (?1,?2)",
                params![record.id, surface],
            )
            .map_err(db_error)?;
        }
    }
    tx.execute("INSERT INTO usage_file_cursors VALUES (?1,?2,?3) ON CONFLICT(source_id,file_key) DO UPDATE SET cursor_json=excluded.cursor_json",params![source,file,encode(cursor)?]).map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    #[cfg(feature = "local-e2e")]
    crate::test_support::fault("import-commit")?;
    Ok(())
}

pub fn existing(db: &UsageDb, id: &str) -> AppResult<Option<Observation>> {
    db.conn
        .query_row(
            "SELECT payload FROM usage_observation_details WHERE record_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .map(decode)
        .transpose()
}

pub fn save_run(db: &UsageDb, run: &SyncRun) -> AppResult<()> {
    db.conn.execute("INSERT INTO usage_jobs VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET status=excluded.status,payload=excluded.payload",params![run.id,run.source_id,encode(&run.status)?,encode(run)?]).map_err(db_error)?;
    db.conn.execute("DELETE FROM usage_jobs WHERE id IN (SELECT id FROM usage_jobs ORDER BY rowid DESC LIMIT -1 OFFSET 100)",[]).map_err(db_error)?;
    Ok(())
}

pub fn runs(db: &UsageDb) -> AppResult<Vec<SyncRun>> {
    let mut query = db
        .conn
        .prepare("SELECT payload FROM usage_jobs ORDER BY rowid DESC LIMIT 100")
        .map_err(db_error)?;
    let rows = query
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(db_error)?
        .map(|r| decode(r.map_err(db_error)?))
        .collect();
    rows
}

/// Best-effort failure finalization for a live worker that exited early. The
/// conditional update cannot overwrite a terminal outcome from another worker.
pub fn finish_abandoned(db: &UsageDb, id: &str) -> AppResult<()> {
    let pending = encode(&DataStatus::InProgress)?;
    let payload: Option<String> = db
        .conn
        .query_row(
            "SELECT payload FROM usage_jobs WHERE id=?1 AND status=?2",
            params![id, pending],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if let Some(payload) = payload {
        let mut run: SyncRun = decode(payload)?;
        run.status = DataStatus::Unavailable;
        run.finished_at = Some(Utc::now());
        run.detail =
            "Refresh failed before completion; previous committed snapshots retained".into();
        db.conn
            .execute(
                "UPDATE usage_jobs SET status=?1,payload=?2 WHERE id=?3 AND status=?4",
                params![encode(&run.status)?, encode(&run)?, id, pending],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

pub fn recover(db: &UsageDb) -> AppResult<()> {
    for mut run in runs(db)? {
        if run.status == DataStatus::InProgress {
            run.status = DataStatus::Interrupted;
            run.finished_at = Some(Utc::now());
            run.detail =
                "Import interrupted. Committed file cursors are retained; sync again to continue."
                    .into();
            save_run(db, &run)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod job_finalization_tests {
    use super::*;
    #[test]
    fn abandoned_jobs_finish_without_rewriting_terminal_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let db = UsageDb::open_at(&dir.path().join("usage.sqlite3")).unwrap();
        let mut run = SyncRun {
            id: "job".into(),
            source_id: "price-catalog".into(),
            started_at: Utc::now(),
            finished_at: None,
            status: DataStatus::InProgress,
            files_seen: 0,
            records_seen: 0,
            bytes_read: 0,
            detail: "Started".into(),
        };
        save_run(&db, &run).unwrap();
        finish_abandoned(&db, "job").unwrap();
        assert_eq!(runs(&db).unwrap()[0].status, DataStatus::Unavailable);
        assert!(runs(&db).unwrap()[0].finished_at.is_some());
        for status in [
            DataStatus::Available,
            DataStatus::Canceled,
            DataStatus::NetworkError,
        ] {
            run.status = status;
            save_run(&db, &run).unwrap();
            finish_abandoned(&db, "job").unwrap();
            assert_eq!(runs(&db).unwrap()[0].status, status);
        }
    }
}
