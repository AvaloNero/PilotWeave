//! Bounded, redacted installation history and cancellation. No process output is persisted.
use super::{InstallApplyResult, InstallOperationResult, InstallPlan, InstallResultStatus};
use crate::error::{AppError, AppResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use uuid::Uuid;

const MAX_HISTORY_BYTES: u64 = 512 * 1024;
const MAX_RUNS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallRunStatus {
    Running,
    Complete,
    Partial,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRun {
    pub id: String,
    pub plan_id: String,
    pub status: InstallRunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub component_ids: Vec<String>,
    pub current_component_id: Option<String>,
    pub results: Vec<InstallOperationResult>,
    pub detail: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    version: u32,
    runs: Vec<InstallRun>,
}

struct History {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
    document: Document,
    recovery: Option<String>,
}

impl History {
    fn open(path: PathBuf) -> Self {
        let read = crate::safe_file::read_optional(&path, MAX_HISTORY_BYTES);
        let mut value = Self {
            path,
            bytes: read.as_ref().ok().cloned().flatten(),
            document: Document {
                version: 1,
                runs: Vec::new(),
            },
            recovery: None,
        };
        let loaded = read.and_then(|bytes| {
            bytes
                .map(|bytes| {
                    let document: Document = serde_json::from_slice(&bytes)
                        .map_err(|error| AppError::json(&value.path, error))?;
                    validate(&document)?;
                    Ok(document)
                })
                .transpose()
        });
        match loaded {
            Ok(Some(mut document)) => {
                for run in &mut document.runs {
                    if run.status == InstallRunStatus::Running {
                        run.status = InstallRunStatus::Interrupted;
                        run.detail = "Installation was interrupted. Rediscover components before retrying; no uninstall or downgrade was attempted".into();
                        run.current_component_id = None;
                    }
                }
                value.document = document;
            }
            Ok(None) => {}
            Err(error) => value.recovery = Some(crate::redact::redact_text(&error.to_string())),
        }
        value
    }
    fn save(&mut self, next: Document) -> AppResult<()> {
        if self.recovery.is_some() {
            return Err(AppError::Config(
                "Installation history is in read-only recovery; no installer was launched".into(),
            ));
        }
        validate(&next)?;
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|_| AppError::Config("Cannot serialize installation history".into()))?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err(AppError::Config(
                "Installation history exceeds its bound".into(),
            ));
        }
        crate::safe_file::atomic_write_private_if(&self.path, &bytes, self.bytes.as_deref())?;
        self.document = next;
        self.bytes = Some(bytes);
        Ok(())
    }
}

fn validate(document: &Document) -> AppResult<()> {
    if document.version != 1 || document.runs.len() > MAX_RUNS {
        return Err(AppError::Config(
            "Unsupported or oversized installation history".into(),
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for run in &document.runs {
        if Uuid::parse_str(&run.id).is_err()
            || Uuid::parse_str(&run.plan_id).is_err()
            || !ids.insert(&run.id)
            || run.component_ids.len() > 4
            || run.results.len() > 4
            || run.detail.len() > 1024
            || run.results.iter().any(|result| result.detail.len() > 1024)
        {
            return Err(AppError::Config("Invalid installation history".into()));
        }
        super::canonical_component_ids(&run.component_ids)?;
        for result in &run.results {
            super::operation_for(&result.component_id)?;
        }
        if let Some(id) = &run.current_component_id {
            super::operation_for(id)?;
        }
    }
    Ok(())
}

struct Active {
    id: String,
    cancel: Arc<AtomicBool>,
}
pub struct InstallJobs {
    history: Mutex<History>,
    active: Mutex<Option<Active>>,
}
pub struct InstallTicket<'a> {
    pub id: String,
    pub cancel: Arc<AtomicBool>,
    jobs: &'a InstallJobs,
}
impl Drop for InstallTicket<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.jobs.active.lock() {
            if active.as_ref().is_some_and(|value| value.id == self.id) {
                *active = None;
            }
        }
    }
}

impl InstallJobs {
    pub fn open(path: PathBuf) -> Self {
        Self {
            history: Mutex::new(History::open(path)),
            active: Mutex::new(None),
        }
    }
    pub fn runs(&self) -> AppResult<Vec<InstallRun>> {
        let active = self
            .active
            .lock()
            .map_err(|_| AppError::Lock)?
            .as_ref()
            .map(|value| value.id.clone());
        let history = self.history.lock().map_err(|_| AppError::Lock)?;
        if history.recovery.is_some() {
            return Err(AppError::Config(
                "Installation history is unavailable in read-only recovery".into(),
            ));
        }
        let mut runs = history.document.runs.clone();
        for run in &mut runs {
            if run.status == InstallRunStatus::Running && active.as_deref() != Some(&run.id) {
                run.status = InstallRunStatus::Interrupted;
                run.detail = "The run is no longer active and its final summary is unavailable; rediscover before retrying".into();
                run.current_component_id = None;
            }
        }
        Ok(runs)
    }
    pub fn begin(&self, plan: &InstallPlan) -> AppResult<(InstallTicket<'_>, InstallRun)> {
        let mut active = self.active.lock().map_err(|_| AppError::Lock)?;
        if active.is_some() {
            return Err(AppError::Busy);
        }
        let run = InstallRun {
            id: Uuid::new_v4().to_string(),
            plan_id: plan.id.clone(),
            status: InstallRunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            component_ids: plan.requested_component_ids.clone(),
            current_component_id: None,
            results: Vec::new(),
            detail: "Installation started; completed operations are not automatically rolled back"
                .into(),
        };
        let mut history = self.history.lock().map_err(|_| AppError::Lock)?;
        let mut next = history.document.clone();
        next.runs.insert(0, run.clone());
        next.runs.truncate(MAX_RUNS);
        history.save(next)?;
        let cancel = Arc::new(AtomicBool::new(false));
        *active = Some(Active {
            id: run.id.clone(),
            cancel: cancel.clone(),
        });
        Ok((
            InstallTicket {
                id: run.id.clone(),
                cancel,
                jobs: self,
            },
            run,
        ))
    }
    pub fn cancel(&self, run_id: &str) -> AppResult<bool> {
        let active = self.active.lock().map_err(|_| AppError::Lock)?;
        let Some(active) = active.as_ref().filter(|value| value.id == run_id) else {
            return Ok(false);
        };
        active.cancel.store(true, Ordering::Release);
        Ok(true)
    }
    pub fn progress(
        &self,
        run_id: &str,
        component: &str,
        result: Option<InstallOperationResult>,
    ) -> AppResult<InstallRun> {
        let mut history = self.history.lock().map_err(|_| AppError::Lock)?;
        let mut next = history.document.clone();
        let run = next
            .runs
            .iter_mut()
            .find(|run| run.id == run_id && run.status == InstallRunStatus::Running)
            .ok_or(AppError::PlanUnavailable)?;
        run.current_component_id = Some(component.into());
        if let Some(result) = result {
            run.results.retain(|value| value.component_id != component);
            run.results.push(result);
        }
        let value = run.clone();
        history.save(next)?;
        Ok(value)
    }
    pub fn finish(
        &self,
        run_id: &str,
        result: &AppResult<InstallApplyResult>,
        cancelled: bool,
    ) -> AppResult<InstallRun> {
        let mut history = self.history.lock().map_err(|_| AppError::Lock)?;
        let mut next = history.document.clone();
        let run = next
            .runs
            .iter_mut()
            .find(|run| run.id == run_id && run.status == InstallRunStatus::Running)
            .ok_or(AppError::PlanUnavailable)?;
        if let Ok(value) = result {
            run.results = value.results.clone();
        }
        let success = run
            .results
            .iter()
            .filter(|value| {
                matches!(
                    value.status,
                    InstallResultStatus::CompletedAndVerified
                        | InstallResultStatus::SkippedAlreadyReady
                )
            })
            .count();
        run.status = if cancelled {
            InstallRunStatus::Cancelled
        } else if success > 0 && (success < run.component_ids.len() || result.is_err()) {
            InstallRunStatus::Partial
        } else if result.is_err() || success == 0 && !run.component_ids.is_empty() {
            InstallRunStatus::Failed
        } else if success == run.component_ids.len() {
            InstallRunStatus::Complete
        } else {
            InstallRunStatus::Partial
        };
        run.detail = match run.status {
            InstallRunStatus::Complete => "Every requested component was verified ready",
            InstallRunStatus::Cancelled => {
                "Cancellation requested. Completed installations remain; rediscover before retrying"
            }
            InstallRunStatus::Partial => {
                "Some components need attention; completed installations remain"
            }
            _ => "Installation did not complete. Rediscover before creating a new plan",
        }
        .into();
        run.current_component_id = None;
        run.finished_at = Some(Utc::now());
        let value = run.clone();
        history.save(next)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> InstallPlan {
        InstallPlan {
            id: Uuid::new_v4().to_string(),
            requested_component_ids: vec![super::super::COMPONENT_COPILOT_CLI.into()],
            operations: Vec::new(),
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(15),
            executable_bindings: std::collections::BTreeMap::new(),
        }
    }
    #[test]
    fn history_is_persisted_before_work_cancellation_is_scoped_and_restart_is_interrupted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.json");
        let jobs = InstallJobs::open(path.clone());
        let plan = plan();
        let (ticket, run) = jobs.begin(&plan).unwrap();
        assert!(path.is_file());
        assert!(!jobs.cancel("wrong-run").unwrap());
        assert!(jobs.begin(&plan).is_err());
        assert!(jobs.cancel(&run.id).unwrap());
        assert!(ticket.cancel.load(Ordering::Acquire));
        let reopened = InstallJobs::open(path);
        assert_eq!(
            reopened.runs().unwrap()[0].status,
            InstallRunStatus::Interrupted
        );
        let finished = jobs
            .finish(&run.id, &Err(AppError::Cancelled), true)
            .unwrap();
        assert_eq!(finished.status, InstallRunStatus::Cancelled);
        drop(ticket);
        assert!(!jobs.cancel(&run.id).unwrap());
        assert!(jobs.begin(&plan).is_ok());
    }
    #[test]
    fn invalid_or_externally_changed_history_blocks_new_work() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.json");
        std::fs::write(&path, b"invalid-secret-fixture").unwrap();
        let jobs = InstallJobs::open(path.clone());
        assert!(jobs.begin(&plan()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid-secret-fixture");
        std::fs::remove_file(&path).unwrap();
        let jobs = InstallJobs::open(path.clone());
        std::fs::write(&path, b"external").unwrap();
        assert!(jobs.begin(&plan()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"external");
    }

    #[test]
    fn a_later_failure_does_not_erase_already_verified_installation_progress() {
        let root = tempfile::tempdir().unwrap();
        let jobs = InstallJobs::open(root.path().join("history.json"));
        let mut reviewed = plan();
        reviewed
            .requested_component_ids
            .push(super::super::COMPONENT_COPILOT_APP.into());
        let (_ticket, run) = jobs.begin(&reviewed).unwrap();
        jobs.progress(
            &run.id,
            super::super::COMPONENT_COPILOT_CLI,
            Some(InstallOperationResult {
                component_id: super::super::COMPONENT_COPILOT_CLI.into(),
                status: InstallResultStatus::CompletedAndVerified,
                detail: "Verified fixture".into(),
            }),
        )
        .unwrap();
        let finished = jobs.finish(&run.id, &Err(AppError::Busy), false).unwrap();
        assert_eq!(finished.status, InstallRunStatus::Partial);
        assert_eq!(
            finished.results[0].status,
            InstallResultStatus::CompletedAndVerified
        );
    }
}
