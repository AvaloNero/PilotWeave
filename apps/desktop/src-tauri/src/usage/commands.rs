use super::{
    importer::{self, UsageRoots},
    pricing, queries, runtime, store,
    types::*,
};
use crate::{
    commands::ManagedState,
    error::{AppError, AppResult},
    github_billing::{self, GithubBillingOverview, GithubBillingPeriod},
    github_billing_store, redact,
    usage_db::UsageDb,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use tauri::{Emitter, State};

#[derive(Default)]
pub struct UsageJobs {
    pub local_busy: Arc<AtomicBool>,
    pub local_cancel: Arc<AtomicBool>,
    pub runtime_busy: Arc<AtomicBool>,
    pub runtime_cancel: Arc<AtomicBool>,
    pub price_busy: Arc<AtomicBool>,
    pub price_cancel: Arc<AtomicBool>,
    pub billing_busy: Arc<AtomicBool>,
    pub billing_cancel: Arc<AtomicBool>,
    pub refresh_times: Mutex<[Option<std::time::Instant>; 3]>,
}
struct JobGuard(Arc<AtomicBool>);
impl JobGuard {
    fn begin(flag: Arc<AtomicBool>) -> AppResult<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| AppError::Busy)?;
        Ok(Self(flag))
    }
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
pub(crate) fn error(e: AppError) -> String {
    redact::redact_text(&e.to_string())
}
fn lock(db: &Mutex<Option<UsageDb>>) -> AppResult<MutexGuard<'_, Option<UsageDb>>> {
    db.lock().map_err(|_| AppError::Busy)
}
pub(crate) fn with_db<T>(
    state: &ManagedState,
    f: impl FnOnce(&UsageDb) -> AppResult<T>,
) -> Result<T, String> {
    let guard = lock(&state.usage_db).map_err(error)?;
    f(guard.as_ref().ok_or_else(|| {
        error(AppError::Config(
            "Usage storage is unavailable; Connection management remains available".into(),
        ))
    })?)
    .map_err(error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_usage_overview(
    state: State<'_, ManagedState>,
    query: Option<UsageQuery>,
) -> Result<UsageOverview, String> {
    let path = with_db(&state, |db| Ok(db.path().to_path_buf()))?;
    tauri::async_runtime::spawn_blocking(move || {
        queries::overview(
            &UsageDb::open_at(&path)?,
            &query.unwrap_or_else(queries::default_query),
        )
    })
    .await
    .map_err(|_| "Usage query worker failed".to_string())?
    .map_err(error)
}
#[tauri::command]
pub fn get_usage_sources(state: State<'_, ManagedState>) -> Result<Vec<SourceView>, String> {
    with_db(&state, |db| importer::sources(db, &UsageRoots::native()?))
}
#[tauri::command]
pub fn get_usage_runs(state: State<'_, ManagedState>) -> Result<Vec<SyncRun>, String> {
    with_db(&state, store::runs)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_usage_source_enabled(
    state: State<'_, ManagedState>,
    source_id: String,
    enabled: bool,
) -> Result<(), String> {
    let _job = JobGuard::begin(state.usage_jobs.local_busy.clone()).map_err(error)?;
    with_db(&state, |db| {
        importer::set_enabled(db, &UsageRoots::native()?, &source_id, enabled)
    })
}
#[tauri::command(rename_all = "camelCase")]
pub fn clear_local_usage(
    state: State<'_, ManagedState>,
    source_id: String,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm clearing PilotWeave's imported metadata".into());
    }
    let _job = JobGuard::begin(state.usage_jobs.local_busy.clone()).map_err(error)?;
    let mut guard = lock(&state.usage_db).map_err(error)?;
    importer::clear(
        guard
            .as_mut()
            .ok_or_else(|| "Usage storage is unavailable".to_string())?,
        &source_id,
    )
    .map_err(error)
}
#[tauri::command(rename_all = "camelCase")]
pub async fn sync_local_usage(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    source_ids: Vec<String>,
) -> Result<Vec<SyncRun>, String> {
    let job = JobGuard::begin(state.usage_jobs.local_busy.clone()).map_err(error)?;
    let path = with_db(&state, |db| Ok(db.path().to_path_buf()))?;
    let cancel = state.usage_jobs.local_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let _job = job;
        let mut db = UsageDb::open_at(&path)?;
        importer::sync(
            &mut db,
            &UsageRoots::native()?,
            &source_ids,
            &cancel,
            |run| {
                let _ = app.emit("usage-progress", run);
            },
        )
    })
    .await
    .map_err(|_| "Usage worker failed; committed cursors are retained".to_string())?
    .map_err(error)
}
#[tauri::command]
pub fn cancel_usage_sync(state: State<'_, ManagedState>) {
    state.usage_jobs.local_cancel.store(true, Ordering::SeqCst);
    state
        .usage_jobs
        .runtime_cancel
        .store(true, Ordering::SeqCst);
    state.usage_jobs.price_cancel.store(true, Ordering::SeqCst);
    state
        .usage_jobs
        .billing_cancel
        .store(true, Ordering::SeqCst);
}
fn begin_remote(
    state: &ManagedState,
    index: usize,
    flag: Arc<AtomicBool>,
    source: &str,
) -> Result<(JobGuard, SyncRun, std::path::PathBuf), String> {
    let job = JobGuard::begin(flag).map_err(error)?;
    let mut times = state
        .usage_jobs
        .refresh_times
        .lock()
        .map_err(|_| "Refresh status unavailable".to_string())?;
    if times[index].is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(5)) {
        return Err("Wait five seconds between refresh attempts".into());
    }
    let run = SyncRun {
        id: uuid::Uuid::new_v4().to_string(),
        source_id: source.into(),
        started_at: chrono::Utc::now(),
        finished_at: None,
        status: DataStatus::InProgress,
        files_seen: 0,
        records_seen: 0,
        bytes_read: 0,
        detail: "Refresh started".into(),
    };
    let path = with_db(state, |db| {
        store::save_run(db, &run)?;
        Ok(db.path().to_path_buf())
    })?;
    times[index] = Some(std::time::Instant::now());
    Ok((job, run, path))
}
fn finish_remote(
    db: &UsageDb,
    run: &mut SyncRun,
    status: DataStatus,
    detail: &str,
) -> AppResult<()> {
    run.status = status;
    run.detail = redact::redact_text(detail);
    run.finished_at = Some(chrono::Utc::now());
    store::save_run(db, run)
}
#[tauri::command]
pub fn get_price_catalog(state: State<'_, ManagedState>) -> Result<pricing::PriceView, String> {
    with_db(&state, pricing::view)
}
#[tauri::command]
pub async fn refresh_price_catalog(
    state: State<'_, ManagedState>,
) -> Result<pricing::PriceView, String> {
    let (job, mut run, path) = begin_remote(
        &state,
        0,
        state.usage_jobs.price_busy.clone(),
        "price-catalog",
    )?;
    let cancel = state.usage_jobs.price_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let _job = job;
        let result = pricing::fetch();
        let mut db = UsageDb::open_at(&path)?;
        if cancel.load(Ordering::SeqCst) {
            finish_remote(
                &db,
                &mut run,
                DataStatus::Canceled,
                "Price refresh canceled; previous snapshot retained",
            )?;
            return pricing::view(&db);
        }
        match result {
            Ok(catalog) => {
                pricing::save(&mut db, &catalog)?;
                if let Some(current) = pricing::latest(&db)? {
                    pricing::price_unbound(&mut db, &current)?;
                }
                run.records_seen = catalog.models.len();
                finish_remote(
                    &db,
                    &mut run,
                    DataStatus::Available,
                    "Price catalog checked; existing historical bindings retained",
                )?;
            }
            Err((status, detail)) => finish_remote(&db, &mut run, status, &detail)?,
        }
        pricing::view(&db)
    })
    .await
    .map_err(|_| "Price refresh worker failed".to_string())?
    .map_err(error)
}
#[tauri::command]
pub fn get_official_runtime_usage(
    state: State<'_, ManagedState>,
) -> Result<runtime::RuntimeView, String> {
    with_db(&state, runtime::view)
}
#[tauri::command]
pub async fn refresh_official_runtime_usage(
    state: State<'_, ManagedState>,
) -> Result<runtime::RuntimeView, String> {
    let (job, mut run, path) = begin_remote(
        &state,
        1,
        state.usage_jobs.runtime_busy.clone(),
        "official-runtime",
    )?;
    let cancel = state.usage_jobs.runtime_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let _job = job;
        let snapshot = runtime::refresh(&cancel);
        let mut db = UsageDb::open_at(&path)?;
        runtime::save(&mut db, &snapshot)?;
        finish_remote(&db, &mut run, snapshot.status, &snapshot.detail)?;
        runtime::view(&db)
    })
    .await
    .map_err(|_| "Runtime quota worker failed".to_string())?
    .map_err(error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_github_billing(
    state: State<'_, ManagedState>,
    year: Option<i32>,
    month: Option<u32>,
) -> Result<GithubBillingOverview, String> {
    let period = match (year, month) {
        (None, None) => GithubBillingPeriod::current(),
        (Some(y), Some(m)) => GithubBillingPeriod::new(y, m),
        _ => return Err("Supply both Billing year and month".into()),
    }
    .map_err(error)?;
    billing_view(&state, period)
}
fn billing_view(
    state: &ManagedState,
    period: GithubBillingPeriod,
) -> Result<GithubBillingOverview, String> {
    let authorization = state.github_authorization().map_err(error)?.status();
    let storage = state.usage_db_status().map_err(error)?;
    let account = authorization.identity.clone();
    let families = if let Some(identity) = &account {
        with_db(state, |db| {
            github_billing_store::family_views(
                db,
                &github_billing::identity_key(identity),
                Some(period),
            )
        })?
    } else {
        github_billing::empty_family_views()
    };
    Ok(GithubBillingOverview {
        authorization,
        storage,
        account,
        families,
        observed_at: chrono::Utc::now(),
    })
}
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_personal_github_usage(
    state: State<'_, ManagedState>,
    year: i32,
    month: u32,
) -> Result<GithubBillingOverview, String> {
    let period = GithubBillingPeriod::new(year, month).map_err(error)?;
    let (token, identity, authorization_revision) = {
        let auth = state.github_authorization().map_err(error)?;
        if auth.status().state != crate::github_auth::GithubAuthorizationState::Verified {
            return Err("Validate PilotWeave's separate GitHub authorization before refreshing personal Billing".into());
        }
        let identity = auth
            .status()
            .identity
            .ok_or_else(|| "Authorize PilotWeave separately for personal Billing".to_string())?;
        let token = auth
            .secret_for_refresh()
            .map_err(error)?
            .ok_or_else(|| "PilotWeave authorization is missing".to_string())?;
        (token, identity, auth.revision())
    };
    let previous = with_db(&state, |db| {
        github_billing_store::family_views(db, &github_billing::identity_key(&identity), None)
    })?;
    if previous
        .iter()
        .filter_map(|f| f.latest.as_ref().and_then(|s| s.retry_at))
        .any(|d| d > chrono::Utc::now())
    {
        return Err(
            "GitHub requested a cooldown; retry after the time shown on the Billing snapshot"
                .into(),
        );
    }
    let (_job, mut run, _path) = begin_remote(
        &state,
        2,
        state.usage_jobs.billing_busy.clone(),
        "personal-billing",
    )?;
    let owner = identity.clone();
    let cancel = state.usage_jobs.billing_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let snapshots = tauri::async_runtime::spawn_blocking(move || {
        github_billing::fetch_personal_billing(&token, &identity, period, &cancel)
    })
    .await
    .map_err(|_| "Billing worker failed".to_string())?
    .map_err(error)?;
    if state.usage_jobs.billing_cancel.load(Ordering::SeqCst) {
        with_db(&state, |db| {
            finish_remote(
                db,
                &mut run,
                DataStatus::Canceled,
                "Billing refresh canceled; previous snapshots retained",
            )
        })?;
        return billing_view(&state, period);
    }
    let auth = state.github_authorization().map_err(error)?;
    let current = auth.status();
    if auth.ensure_current(authorization_revision).is_err()
        || current.identity.as_ref() != Some(&owner)
        || !current.has_secret
    {
        with_db(&state, |db| {
            finish_remote(
                db,
                &mut run,
                DataStatus::Canceled,
                "Authorization changed; Billing response discarded",
            )
        })?;
        return Err("Authorization changed during refresh; Billing response was discarded".into());
    }
    {
        let mut guard = lock(&state.usage_db).map_err(error)?;
        let db = guard
            .as_mut()
            .ok_or_else(|| "Usage storage is unavailable".to_string())?;
        github_billing_store::insert_snapshots(db, &snapshots).map_err(error)?;
        finish_remote(db,&mut run,if snapshots.iter().all(|s|s.status.is_success()){DataStatus::Available}else{DataStatus::Partial},"Personal Billing refresh finished; inspect each endpoint family for coverage and errors").map_err(error)?;
    }
    drop(auth);
    billing_view(&state, period)
}
