use crate::adapters;
use crate::domain::{ClientKind, ClientTarget, Connection, DeploymentPlan};
use crate::error::{AppError, AppResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const PLAN_TTL_SECONDS: i64 = 10 * 60;
const MAX_FINGERPRINT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct StoredPlan {
    pub plan: DeploymentPlan,
    connection_updated_at: DateTime<Utc>,
    target_fingerprints: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct PlanStore {
    plans: HashMap<String, StoredPlan>,
}

impl PlanStore {
    pub fn insert(
        &mut self,
        connection: &Connection,
        plan: DeploymentPlan,
        targets: &[ClientTarget],
    ) -> AppResult<DeploymentPlan> {
        self.purge_expired();
        let mut fingerprints = BTreeMap::new();
        for target_id in &plan.target_ids {
            let target = targets
                .iter()
                .find(|target| target.id == *target_id)
                .ok_or_else(|| {
                    AppError::InvalidInput(format!("Unknown client target: {target_id}"))
                })?;
            fingerprints.insert(target_id.clone(), fingerprint_target(target)?);
        }
        self.plans.insert(
            plan.id.clone(),
            StoredPlan {
                plan: plan.clone(),
                connection_updated_at: connection.updated_at,
                target_fingerprints: fingerprints,
            },
        );
        Ok(plan)
    }

    pub fn consume(&mut self, plan_id: &str) -> AppResult<StoredPlan> {
        self.purge_expired();
        self.plans.remove(plan_id).ok_or_else(|| {
            AppError::InvalidInput(
                "Deployment plan is missing, expired, or was already consumed; preview again"
                    .to_string(),
            )
        })
    }

    pub fn consume_matching(
        &mut self,
        connection_id: &str,
        target_ids: &[String],
    ) -> AppResult<StoredPlan> {
        self.purge_expired();
        let requested = canonical_target_ids(target_ids);
        let candidate = self
            .plans
            .values()
            .filter(|stored| {
                stored.plan.connection_id == connection_id
                    && canonical_target_ids(&stored.plan.target_ids) == requested
            })
            .max_by_key(|stored| stored.plan.created_at.timestamp_millis())
            .map(|stored| stored.plan.id.clone())
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "No live deployment preview matches this request; preview again before apply"
                        .to_string(),
                )
            })?;
        self.consume(&candidate)
    }

    fn purge_expired(&mut self) {
        let cutoff = Utc::now() - Duration::seconds(PLAN_TTL_SECONDS);
        self.plans
            .retain(|_, stored| stored.plan.created_at >= cutoff);
    }
}

pub fn validate_plan(
    stored: &StoredPlan,
    connection: &Connection,
    targets: &[ClientTarget],
) -> AppResult<()> {
    if connection.updated_at != stored.connection_updated_at {
        return Err(AppError::InvalidInput(
            "Connection changed after preview; preview the deployment again".to_string(),
        ));
    }

    if Utc::now() - stored.plan.created_at > Duration::seconds(PLAN_TTL_SECONDS) {
        return Err(AppError::InvalidInput(
            "Deployment preview expired; preview the deployment again".to_string(),
        ));
    }

    for operation in &stored.plan.operations {
        let target = targets
            .iter()
            .find(|target| target.id == operation.target_id)
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "Target disappeared after preview: {}",
                    operation.target_id
                ))
            })?;
        let before = stored
            .target_fingerprints
            .get(&operation.target_id)
            .ok_or_else(|| {
                AppError::Config("Deployment plan is missing a target fingerprint".into())
            })?;
        let current = fingerprint_target(target)?;
        if &current != before {
            return Err(AppError::InvalidInput(format!(
                "Target changed after preview: {}; preview again before apply",
                target.name
            )));
        }
        if operation.supported {
            preflight_target(connection, target)?;
        }
    }
    Ok(())
}

fn preflight_target(connection: &Connection, target: &ClientTarget) -> AppResult<()> {
    if !target.detected || !target.supports_write {
        return Err(AppError::Unsupported(format!(
            "{} is not available for managed writes",
            target.name
        )));
    }
    match target.kind {
        ClientKind::VsCodeCopilot => {
            let path = target.path.as_deref().ok_or_else(|| {
                AppError::Config("VS Code target has no configuration path".into())
            })?;
            ensure_regular_or_missing(Path::new(path))?;
        }
        ClientKind::CopilotCli => {
            let _ = adapters::copilot_cli::desired_environment(connection, Some("preflight"))?;
        }
        ClientKind::GithubCopilotApp => {
            return Err(AppError::Unsupported(
                "GitHub Copilot app remains a read-only deployment target".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn operation_rank(kind: ClientKind) -> u8 {
    match kind {
        ClientKind::VsCodeCopilot => 0,
        ClientKind::CopilotCli => 1,
        ClientKind::GithubCopilotApp => 2,
    }
}

#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub target_id: String,
    path: PathBuf,
    existed: bool,
    bytes: Vec<u8>,
    before_fingerprint: String,
    after_fingerprint: Option<String>,
}

pub fn capture_file_snapshots(
    plan: &DeploymentPlan,
    targets: &[ClientTarget],
) -> AppResult<Vec<FileSnapshot>> {
    let mut snapshots = Vec::new();
    for operation in &plan.operations {
        if !operation.supported || operation.target_kind != ClientKind::VsCodeCopilot {
            continue;
        }
        let target = targets
            .iter()
            .find(|target| target.id == operation.target_id)
            .ok_or_else(|| AppError::Config("Prepared target disappeared".into()))?;
        let path =
            PathBuf::from(target.path.as_deref().ok_or_else(|| {
                AppError::Config("VS Code target has no configuration path".into())
            })?);
        ensure_regular_or_missing(&path)?;
        let existed = path.exists();
        let bytes = if existed {
            let metadata = fs::metadata(&path).map_err(|error| AppError::io(&path, error))?;
            if metadata.len() > MAX_FINGERPRINT_FILE_BYTES {
                return Err(AppError::InvalidInput(format!(
                    "Refusing to snapshot an oversized deployment target: {}",
                    path.display()
                )));
            }
            fs::read(&path).map_err(|error| AppError::io(&path, error))?
        } else {
            Vec::new()
        };
        snapshots.push(FileSnapshot {
            target_id: target.id.clone(),
            before_fingerprint: fingerprint_path(&path)?,
            path,
            existed,
            bytes,
            after_fingerprint: None,
        });
    }
    Ok(snapshots)
}

pub fn mark_snapshot_applied(
    snapshots: &mut [FileSnapshot],
    target_id: &str,
) -> AppResult<Option<String>> {
    let Some(snapshot) = snapshots
        .iter_mut()
        .find(|snapshot| snapshot.target_id == target_id)
    else {
        return Ok(None);
    };
    let fingerprint = fingerprint_path(&snapshot.path)?;
    snapshot.after_fingerprint = Some(fingerprint.clone());
    Ok(Some(fingerprint))
}

pub fn rollback_applied_files(snapshots: &[FileSnapshot]) -> AppResult<()> {
    for snapshot in snapshots.iter().rev() {
        let Some(after) = snapshot.after_fingerprint.as_deref() else {
            continue;
        };
        let current = fingerprint_path(&snapshot.path)?;
        if current != after {
            return Err(AppError::Config(format!(
                "Refusing rollback because {} changed after PilotWeave wrote it",
                snapshot.path.display()
            )));
        }
        if snapshot.existed {
            atomic_write_private(&snapshot.path, &snapshot.bytes)?;
            let restored = fingerprint_path(&snapshot.path)?;
            if restored != snapshot.before_fingerprint {
                return Err(AppError::Config(format!(
                    "Rollback verification failed for {}",
                    snapshot.path.display()
                )));
            }
        } else if snapshot.path.exists() {
            fs::remove_file(&snapshot.path).map_err(|error| AppError::io(&snapshot.path, error))?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalTarget {
    target_id: String,
    target_kind: ClientKind,
    before_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_backup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentJournal {
    version: u32,
    plan_id: String,
    connection_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    targets: Vec<JournalTarget>,
}

pub struct JournalHandle {
    path: PathBuf,
    backup_dir: PathBuf,
    journal: DeploymentJournal,
}

pub fn recovery_required(state_path: &Path) -> bool {
    journal_path(state_path).exists()
}

pub fn ensure_no_pending_journal(state_path: &Path) -> AppResult<()> {
    if recovery_required(state_path) {
        return Err(AppError::Config(format!(
            "An interrupted deployment journal exists at {}; managed writes are disabled until recovery is reviewed",
            journal_path(state_path).display()
        )));
    }
    Ok(())
}

pub fn begin_journal(
    state_path: &Path,
    stored: &StoredPlan,
    targets: &[ClientTarget],
    snapshots: &[FileSnapshot],
) -> AppResult<JournalHandle> {
    ensure_no_pending_journal(state_path)?;
    let parent = state_path
        .parent()
        .ok_or_else(|| AppError::Config("State path has no parent directory".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    let backup_dir = parent.join("deployment-snapshots").join(&stored.plan.id);
    fs::create_dir_all(&backup_dir).map_err(|error| AppError::io(&backup_dir, error))?;

    let mut journal_targets = Vec::new();
    for operation in &stored.plan.operations {
        let target = targets
            .iter()
            .find(|target| target.id == operation.target_id)
            .ok_or_else(|| AppError::Config("Journal target disappeared".into()))?;
        let before_fingerprint = fingerprint_target(target)?;
        let rollback_backup = snapshots
            .iter()
            .position(|snapshot| snapshot.target_id == target.id)
            .map(|index| -> AppResult<String> {
                let backup_path = backup_dir.join(format!("{index}.bak"));
                atomic_write_private(&backup_path, &snapshots[index].bytes)?;
                Ok(backup_path.to_string_lossy().to_string())
            })
            .transpose()?;
        journal_targets.push(JournalTarget {
            target_id: target.id.clone(),
            target_kind: target.kind,
            before_fingerprint,
            after_fingerprint: None,
            rollback_backup,
        });
    }

    let now = Utc::now();
    let journal = DeploymentJournal {
        version: JOURNAL_VERSION,
        plan_id: stored.plan.id.clone(),
        connection_id: stored.plan.connection_id.clone(),
        created_at: now,
        updated_at: now,
        targets: journal_targets,
    };
    let mut handle = JournalHandle {
        path: journal_path(state_path),
        backup_dir,
        journal,
    };
    handle.persist()?;
    Ok(handle)
}

impl JournalHandle {
    pub fn mark_applied(&mut self, target_id: &str, after_fingerprint: String) -> AppResult<()> {
        if let Some(target) = self
            .journal
            .targets
            .iter_mut()
            .find(|target| target.target_id == target_id)
        {
            target.after_fingerprint = Some(after_fingerprint);
        }
        self.journal.updated_at = Utc::now();
        self.persist()
    }

    pub fn clear(self) -> AppResult<()> {
        if self.path.exists() {
            fs::remove_file(&self.path).map_err(|error| AppError::io(&self.path, error))?;
        }
        if self.backup_dir.exists() {
            fs::remove_dir_all(&self.backup_dir)
                .map_err(|error| AppError::io(&self.backup_dir, error))?;
        }
        Ok(())
    }

    fn persist(&mut self) -> AppResult<()> {
        let bytes = serde_json::to_vec_pretty(&self.journal).map_err(|error| {
            AppError::Config(format!("Failed to serialize deployment journal: {error}"))
        })?;
        atomic_write_private(&self.path, &bytes)
    }
}

pub fn fingerprint_target(target: &ClientTarget) -> AppResult<String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    target.id.hash(&mut hasher);
    target.kind.hash(&mut hasher);
    target.name.hash(&mut hasher);
    target.detail.hash(&mut hasher);
    target.detected.hash(&mut hasher);
    target.supports_write.hash(&mut hasher);
    target.path.hash(&mut hasher);
    if let Some(path) = target.path.as_deref() {
        fingerprint_path_into(Path::new(path), &mut hasher)?;
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn fingerprint_path(path: &Path) -> AppResult<String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    fingerprint_path_into(path, &mut hasher)?;
    Ok(format!("{:016x}", hasher.finish()))
}

fn fingerprint_path_into(path: &Path, hasher: &mut impl Hasher) -> AppResult<()> {
    path.to_string_lossy().hash(hasher);
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            metadata.file_type().is_symlink().hash(hasher);
            metadata.is_file().hash(hasher);
            metadata.is_dir().hash(hasher);
            metadata.len().hash(hasher);
            if metadata.is_file() && metadata.len() <= MAX_FINGERPRINT_FILE_BYTES {
                fs::read(path)
                    .map_err(|error| AppError::io(path, error))?
                    .hash(hasher);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "missing".hash(hasher);
        }
        Err(error) => return Err(AppError::io(path, error)),
    }
    Ok(())
}

fn ensure_regular_or_missing(path: &Path) -> AppResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(AppError::InvalidInput(format!(
                "Refusing to modify a non-regular target: {}",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn journal_path(state_path: &Path) -> PathBuf {
    state_path.with_file_name("deployment-journal.json")
}

fn canonical_target_ids(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Target path has no parent directory".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    ensure_regular_or_missing(path)?;
    let temp = parent.join(format!(".pilotweave-{}.tmp", Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut file = options
            .open(&temp)
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| AppError::io(&temp, error))?;
        }
        file.write_all(bytes)
            .map_err(|error| AppError::io(&temp, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
        }
        fs::rename(&temp, path).map_err(|error| AppError::io(path, error))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ApiProtocol, ClientStatus, ModelCapabilities, ModelSpec, ProviderKind};
    use std::collections::BTreeMap;

    fn connection() -> Connection {
        let now = Utc::now();
        Connection {
            id: "one".to_string(),
            name: "One".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: ProviderKind::Openai,
            protocol: ApiProtocol::ChatCompletions,
            headers: BTreeMap::new(),
            models: vec![ModelSpec {
                id: "model".to_string(),
                model_id: "model-a".to_string(),
                name: "Model A".to_string(),
                enabled: true,
                capabilities: ModelCapabilities::default(),
            }],
            secret_ref: "connection:one".to_string(),
            has_secret: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn target(path: &Path) -> ClientTarget {
        ClientTarget {
            id: "vscode:test".to_string(),
            kind: ClientKind::VsCodeCopilot,
            name: "VS Code test".to_string(),
            detail: "test".to_string(),
            path: Some(path.to_string_lossy().to_string()),
            detected: true,
            supports_write: true,
            status: ClientStatus::Available,
            diagnostic: None,
        }
    }

    fn plan(connection: &Connection, target: &ClientTarget) -> DeploymentPlan {
        DeploymentPlan {
            id: "plan".to_string(),
            connection_id: connection.id.clone(),
            connection_name: connection.name.clone(),
            target_ids: vec![target.id.clone()],
            operations: vec![crate::domain::DeploymentOperation {
                id: "op".to_string(),
                target_id: target.id.clone(),
                target_kind: target.kind,
                title: "test".to_string(),
                description: "test".to_string(),
                changes: vec![],
                supported: true,
                requires_restart: false,
            }],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn stored_plan_is_one_shot() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("config.json");
        fs::write(&path, b"[]").expect("seed");
        let connection = connection();
        let target = target(&path);
        let plan = plan(&connection, &target);
        let mut store = PlanStore::default();
        store
            .insert(&connection, plan.clone(), std::slice::from_ref(&target))
            .expect("insert");
        assert_eq!(store.consume(&plan.id).expect("consume").plan.id, plan.id);
        assert!(store.consume(&plan.id).is_err());
    }

    #[test]
    fn target_change_after_preview_is_rejected() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("config.json");
        fs::write(&path, b"[]").expect("seed");
        let connection = connection();
        let target = target(&path);
        let plan = plan(&connection, &target);
        let mut store = PlanStore::default();
        store
            .insert(&connection, plan.clone(), std::slice::from_ref(&target))
            .expect("insert");
        fs::write(&path, b"[1]").expect("external change");
        let stored = store.consume(&plan.id).expect("consume");
        assert!(validate_plan(&stored, &connection, &[target]).is_err());
    }

    #[test]
    fn rollback_refuses_external_post_write_change() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("config.json");
        fs::write(&path, b"before").expect("seed");
        let connection = connection();
        let target = target(&path);
        let plan = plan(&connection, &target);
        let mut snapshots =
            capture_file_snapshots(&plan, std::slice::from_ref(&target)).expect("snapshot");
        fs::write(&path, b"pilotweave").expect("write");
        mark_snapshot_applied(&mut snapshots, &target.id).expect("mark");
        fs::write(&path, b"external").expect("external");
        assert!(rollback_applied_files(&snapshots).is_err());
        assert_eq!(fs::read(&path).expect("read"), b"external");
    }
}
