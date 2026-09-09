use super::{db_error, invalid, parsers, pricing, store, types::*};
use crate::{error::AppResult, safe_file, usage_db::UsageDb};
use chrono::Utc;
use rusqlite::params;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use uuid::Uuid;

pub const SOURCE_IDS: [&str; 3] = [
    "copilot-session-events",
    "vscode-otel",
    "github-copilot-app",
];
const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUN_EVENTS: usize = 100_000;

#[derive(Clone)]
pub struct UsageRoots {
    pub sessions: PathBuf,
    pub vscode_otel: PathBuf,
}
impl UsageRoots {
    pub fn native() -> AppResult<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| invalid("Cannot locate the user home directory"))?;
        let config =
            dirs::config_dir().ok_or_else(|| invalid("Cannot locate local application storage"))?;
        Ok(Self {
            sessions: home.join(".copilot/session-state"),
            vscode_otel: config.join("PilotWeave/usage-inbox/vscode-otel.jsonl"),
        })
    }
}

pub fn initialize(db: &UsageDb) -> AppResult<()> {
    for (id, kind, parser, hint) in [
        (
            SOURCE_IDS[0],
            "copilot-cli",
            "copilot-session-v1",
            "~/.copilot/session-state/*/events.jsonl (shared runtime)",
        ),
        (
            SOURCE_IDS[1],
            "vscode",
            "vscode-otel-span-v1",
            "PilotWeave/usage-inbox/vscode-otel.jsonl",
        ),
        (
            SOURCE_IDS[2],
            "github-copilot-app",
            "unsupported",
            "Shared runtime is counted once; private app stores are not inspected",
        ),
    ] {
        db.conn.execute("INSERT OR IGNORE INTO usage_sources(id,kind,parser_kind,parser_version,enabled,path_hint,status,created_at,updated_at) VALUES (?1,?2,?3,1,0,?4,'disabled',?5,?5)",params![id,kind,parser,hint,Utc::now().to_rfc3339()]).map_err(db_error)?;
    }
    Ok(())
}

fn validate_source(source: &str) -> AppResult<()> {
    if !SOURCE_IDS.contains(&source) {
        return Err(invalid("Unknown usage source ID"));
    }
    Ok(())
}

pub fn sources(db: &UsageDb, roots: &UsageRoots) -> AppResult<Vec<SourceView>> {
    let mut values = vec![];
    for id in SOURCE_IDS {
        let (enabled,parser,path_hint,status,detail,last_scan_at,last_success_at):(bool,String,String,String,Option<String>,Option<String>,Option<String>)=db.conn.query_row(
            "SELECT enabled,parser_kind,path_hint,status,error,last_scan_at,last_success_at FROM usage_sources WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(db_error)?;
        let mut state: DataStatus = serde_json::from_str(&status).unwrap_or(DataStatus::Disabled);
        let mut reason = detail.unwrap_or_else(|| "Enable local metadata import to begin".into());
        if !enabled {
            state = DataStatus::Disabled;
        }
        if id == SOURCE_IDS[2] {
            state = DataStatus::Unsupported;
            reason="No separate stable usage source is established for the app. Shared Copilot runtime records appear once, with client attribution unknown.".into();
        }
        if enabled && last_scan_at.is_none() {
            state = DataStatus::Unavailable;
            reason = "Enabled; import has not run yet".into();
        }
        let path = if id == SOURCE_IDS[0] {
            &roots.sessions
        } else {
            &roots.vscode_otel
        };
        if enabled && !path.exists() && id != SOURCE_IDS[2] {
            state = DataStatus::Missing;
            reason="The supported source has not been created yet. Follow the source setup instructions.".into();
        }
        values.push(SourceView {
            id: id.into(),
            name: match id {
                "vscode-otel" => "VS Code Copilot",
                "github-copilot-app" => "GitHub Copilot app",
                _ => "Copilot CLI / shared runtime",
            }
            .into(),
            enabled,
            status: state,
            detail: reason,
            parser,
            path_hint,
            setup_path: (id == SOURCE_IDS[1])
                .then(|| roots.vscode_otel.to_string_lossy().into_owned()),
            last_scan_at,
            last_success_at,
            record_count: db.record_count(id)?,
        });
    }
    Ok(values)
}

pub fn set_enabled(db: &UsageDb, roots: &UsageRoots, source: &str, enabled: bool) -> AppResult<()> {
    validate_source(source)?;
    if enabled && source == SOURCE_IDS[2] {
        return Err(invalid("The app has no separately supported usage source"));
    }
    if enabled && source == SOURCE_IDS[1] {
        source_safety(&roots.vscode_otel)?;
        fs::create_dir_all(
            roots
                .vscode_otel
                .parent()
                .ok_or_else(|| invalid("Missing import directory"))?,
        )
        .map_err(|_| invalid("Could not create the local usage inbox"))?;
    }
    db.conn.execute("UPDATE usage_sources SET enabled=?1,updated_at=?2,status=CASE WHEN ?1 THEN '\"unavailable\"' ELSE '\"disabled\"' END WHERE id=?3",params![enabled,Utc::now().to_rfc3339(),source]).map_err(db_error)?;
    Ok(())
}

pub fn clear(db: &mut UsageDb, source: &str) -> AppResult<()> {
    validate_source(source)?;
    let tx = db.conn.transaction().map_err(db_error)?;
    tx.execute("DELETE FROM usage_records WHERE source_id=?1", [source])
        .map_err(db_error)?;
    tx.execute(
        "DELETE FROM usage_file_cursors WHERE source_id=?1",
        [source],
    )
    .map_err(db_error)?;
    tx.execute(
        "DELETE FROM usage_source_cursors WHERE source_id=?1",
        [source],
    )
    .map_err(db_error)?;
    tx.execute("DELETE FROM usage_jobs WHERE source_id=?1", [source])
        .map_err(db_error)?;
    tx.execute("UPDATE usage_sources SET enabled=0,last_scan_at=NULL,last_success_at=NULL,status='disabled',error=NULL,coverage_start=NULL,coverage_end=NULL WHERE id=?1",[source]).map_err(db_error)?;
    tx.commit().map_err(db_error)
}

fn candidates(roots: &UsageRoots, source: &str) -> AppResult<(Vec<PathBuf>, bool)> {
    if source == SOURCE_IDS[1] {
        return Ok((vec![roots.vscode_otel.clone()], false));
    }
    // Check every ancestor before reading directory entries. Only one fixed
    // directory level and one fixed filename are accepted.
    source_safety(&roots.sessions.join(".pilotweave-probe"))?;
    let mut paths = vec![];
    let entries = fs::read_dir(&roots.sessions)
        .map_err(|_| invalid("Supported session directory is missing or inaccessible"))?;
    let mut truncated = false;
    for (n, item) in entries.enumerate() {
        if n == MAX_FILES {
            truncated = true;
            break;
        }
        let item = item.map_err(|_| invalid("Session directory is inaccessible"))?;
        let meta = fs::symlink_metadata(item.path())
            .map_err(|_| invalid("Session directory metadata is inaccessible"))?;
        if safe_file::is_link(&meta) {
            return Err(invalid("A session directory is a symlink or reparse point"));
        }
        if meta.is_dir() {
            paths.push(item.path().join("events.jsonl"));
        }
    }
    paths.sort();
    Ok((paths, truncated))
}

fn source_safety(path: &Path) -> AppResult<()> {
    safe_file::ensure_regular_or_missing(path).map_err(|_|invalid("Usage source failed file safety checks; linked, inaccessible or non-regular paths cannot be imported"))
}

fn open_source(path: &Path) -> AppResult<File> {
    source_safety(path)?;
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file = opts
        .open(path)
        .map_err(|_| invalid("Usage file is missing or inaccessible"))?;
    let meta = file
        .metadata()
        .map_err(|_| invalid("Usage file metadata is inaccessible"))?;
    if !meta.is_file() || safe_file::is_link(&meta) || meta.len() > MAX_FILE_BYTES {
        return Err(invalid("Usage source is not a bounded regular file"));
    }
    source_safety(path)?;
    Ok(file)
}

fn file_identity(file: &File) -> AppResult<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file
            .metadata()
            .map_err(|_| invalid("Usage file identity is unavailable"))?;
        Ok(format!("{}:{}", m.dev(), m.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: live file handle, correctly sized writable output buffer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(invalid("Usage file identity is unavailable"));
        }
        let info = unsafe { info.assume_init() };
        Ok(format!(
            "{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
}

pub fn sync(
    db: &mut UsageDb,
    roots: &UsageRoots,
    selected: &[String],
    cancel: &AtomicBool,
    mut progress: impl FnMut(&SyncRun),
) -> AppResult<Vec<SyncRun>> {
    if selected.len() > 3 {
        return Err(invalid("At most three sources may be selected"));
    }
    for id in selected {
        validate_source(id)?;
    }
    let mut result = vec![];
    for source in sources(db, roots)? {
        if !source.enabled || (!selected.is_empty() && !selected.contains(&source.id)) {
            continue;
        }
        let mut run = SyncRun {
            id: Uuid::new_v4().to_string(),
            source_id: source.id.clone(),
            started_at: Utc::now(),
            finished_at: None,
            status: DataStatus::InProgress,
            files_seen: 0,
            records_seen: 0,
            bytes_read: 0,
            detail: "Importing local metadata".into(),
        };
        store::save_run(db, &run)?;
        progress(&run);
        let operation = sync_source(db, roots, &mut run, cancel, &mut progress);
        if let Err(e) = operation {
            run.status = if source.status == DataStatus::Missing {
                DataStatus::Missing
            } else {
                DataStatus::SchemaError
            };
            run.detail = crate::redact::redact_text(&e.to_string());
        }
        run.finished_at = Some(Utc::now());
        store::save_run(db, &run)?;
        db.conn.execute("UPDATE usage_sources SET status=?1,error=?2,last_scan_at=?3,last_success_at=CASE WHEN ?4 THEN ?3 ELSE last_success_at END WHERE id=?5",
            params![store::encode(&run.status)?,run.detail,run.finished_at.map(|d|d.to_rfc3339()),run.status.successful(),source.id]).map_err(db_error)?;
        progress(&run);
        result.push(run);
    }
    Ok(result)
}

fn sync_source(
    db: &mut UsageDb,
    roots: &UsageRoots,
    run: &mut SyncRun,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(&SyncRun),
) -> AppResult<()> {
    let (paths, truncated) = candidates(roots, &run.source_id)?;
    let started = Instant::now();
    let mut errors = 0;
    let mut events = 0;
    let mut last_error = None;
    let catalog = pricing::latest(db)?;
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            run.status = DataStatus::Canceled;
            run.detail = "Canceled; committed metadata is retained".into();
            return Ok(());
        }
        if started.elapsed().as_secs() >= 30
            || run.bytes_read >= MAX_RUN_BYTES
            || events >= MAX_RUN_EVENTS
        {
            run.status = DataStatus::Partial;
            run.detail =
                "Import limit reached; sync again to continue from committed cursors".into();
            return Ok(());
        }
        run.files_seen += 1;
        if let Err(e) = sync_file(
            db,
            &path,
            run,
            cancel,
            &mut events,
            catalog.as_ref(),
            started,
        ) {
            errors += 1;
            last_error = Some(crate::redact::redact_text(&e.to_string()));
        }
        progress(run);
    }
    if cancel.load(Ordering::Relaxed) {
        run.status = DataStatus::Canceled;
        run.detail = "Canceled; committed metadata is retained".into();
        return Ok(());
    }
    if started.elapsed().as_secs() >= 30
        || run.bytes_read >= MAX_RUN_BYTES
        || events >= MAX_RUN_EVENTS
    {
        run.status = DataStatus::Partial;
        run.detail = "Import limit reached; sync again to continue from committed cursors".into();
        return Ok(());
    }
    run.status = if errors > 0 || truncated {
        DataStatus::Partial
    } else if db.record_count(&run.source_id)? == 0 {
        DataStatus::SuccessfulEmpty
    } else {
        DataStatus::Available
    };
    run.detail = if errors > 0 {
        format!(
            "{errors} files could not be imported. {}",
            last_error.unwrap_or_default()
        )
    } else if truncated {
        "Directory scan reached 512 entries; coverage is partial".into()
    } else {
        "Supported metadata imported. Active sessions and absent counters may limit coverage."
            .into()
    };
    Ok(())
}

fn sync_file(
    db: &mut UsageDb,
    path: &Path,
    run: &mut SyncRun,
    cancel: &AtomicBool,
    events: &mut usize,
    catalog: Option<&pricing::PriceCatalog>,
    started: Instant,
) -> AppResult<()> {
    let file_key = parsers::hash("usage-file-path-v1", &path.to_string_lossy());
    let mut file = open_source(path)?;
    let meta = file
        .metadata()
        .map_err(|_| invalid("Usage metadata unavailable"))?;
    let identity = file_identity(&file)?;
    let modified = meta
        .modified()
        .map(chrono::DateTime::<Utc>::from)
        .map(|v| v.to_rfc3339())
        .map_err(|_| invalid("Usage modification time unavailable"))?;
    let old = store::cursor(db, &run.source_id, &file_key)?;
    let prefix_len = old.as_ref().map_or(0, |c| c.offset.min(4096)) as usize;
    let mut prefix = vec![0; prefix_len.min(meta.len() as usize)];
    file.read_exact(&mut prefix)
        .map_err(|_| invalid("Usage file changed while inspecting cursor"))?;
    let mut cursor = old.unwrap_or(FileCursor {
        offset: 0,
        prefix_hash: String::new(),
        file_identity: identity.clone(),
        file_size: meta.len(),
        modified: modified.clone(),
        parser_version: parsers::PARSER_VERSION,
        state: ParserState::default(),
    });
    if cursor.offset > meta.len()
        || cursor.file_identity != identity
        || cursor.parser_version != parsers::PARSER_VERSION
        || (cursor.offset > 0
            && cursor.prefix_hash != crate::fingerprint::bytes("usage-prefix-v1", Some(&prefix)))
        || (cursor.file_size == meta.len() && cursor.modified != modified)
    {
        cursor.offset = 0;
        cursor.state = ParserState::default();
    }
    cursor.file_identity = identity;
    cursor.file_size = meta.len();
    cursor.modified = modified;
    cursor.parser_version = parsers::PARSER_VERSION;
    file.seek(SeekFrom::Start(cursor.offset))
        .map_err(|_| invalid("Usage cursor cannot be read"))?;
    let mut reader = BufReader::new(file.take(meta.len() - cursor.offset));
    let mut records = vec![];
    let mut batch_lines = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if run.bytes_read >= MAX_RUN_BYTES
            || *events >= MAX_RUN_EVENTS
            || started.elapsed().as_secs() >= 30
        {
            break;
        }
        let mut line = Vec::new();
        let n = (&mut reader)
            .take((parsers::MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| invalid("Usage event could not be read"))?;
        if n == 0 {
            break;
        }
        run.bytes_read += n as u64;
        if line.len() > parsers::MAX_LINE_BYTES {
            return Err(invalid("Usage event exceeds the 1 MiB line limit"));
        }
        if !line.ends_with(b"\n") {
            break;
        }
        let parsed = parsers::parse_line(&run.source_id, &line, &mut cursor.state)?;
        for mut record in parsed {
            let old = store::existing(db, &record.id)?;
            if old
                .as_ref()
                .is_some_and(|o| o.finished_at > record.finished_at)
            {
                continue;
            }
            pricing::bind(db, &mut record, old.as_ref(), catalog)?;
            records.push(record);
        }
        cursor.offset += n as u64;
        *events += 1;
        batch_lines += 1;
        if records.len() >= 350 || batch_lines >= 1000 {
            persist(db, path, &file_key, &mut cursor, &records, &run.source_id)?;
            run.records_seen += records.len();
            records.clear();
            batch_lines = 0;
        }
    }
    persist(db, path, &file_key, &mut cursor, &records, &run.source_id)?;
    run.records_seen += records.len();
    Ok(())
}

fn persist(
    db: &mut UsageDb,
    path: &Path,
    file_key: &str,
    cursor: &mut FileCursor,
    records: &[Observation],
    source: &str,
) -> AppResult<()> {
    let mut file = open_source(path)?;
    if file_identity(&file)? != cursor.file_identity {
        return Err(invalid("Usage file was replaced during import; retry"));
    }
    let mut prefix = vec![0; cursor.offset.min(4096) as usize];
    file.read_exact(&mut prefix)
        .map_err(|_| invalid("Usage file was truncated during import"))?;
    cursor.prefix_hash = crate::fingerprint::bytes("usage-prefix-v1", Some(&prefix));
    store::commit_batch(db, source, file_key, cursor, records)
}
