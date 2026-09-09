//! Prepared writes and crash-recoverable compensation. Journal bytes are private:
//! they may contain materialized provider credentials and never cross the IPC boundary.
use crate::error::{AppError, AppResult};
use crate::safe_io;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_RESOURCE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WRITES: usize = 160;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "location", rename_all = "camelCase")]
pub enum Resource {
    File(PathBuf),
    #[cfg(windows)]
    UserEnvironment(String),
}

impl Resource {
    pub fn key(&self) -> String {
        match self {
            Self::File(path) => {
                let key = format!("file:{}", path.display());
                #[cfg(windows)]
                {
                    key.to_lowercase()
                }
                #[cfg(not(windows))]
                {
                    key
                }
            }
            #[cfg(windows)]
            Self::UserEnvironment(name) => format!("HKCU/Environment/{name}"),
        }
    }

    pub fn read(&self) -> AppResult<Option<Vec<u8>>> {
        match self {
            Self::File(path) => safe_io::read_optional(path, MAX_RESOURCE_BYTES),
            #[cfg(windows)]
            Self::UserEnvironment(name) => {
                use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
                let root = winreg::RegKey::predef(HKEY_CURRENT_USER);
                let key = match root.open_subkey_with_flags("Environment", KEY_READ) {
                    Ok(key) => key,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => {
                        return Err(AppError::Config(format!(
                            "Cannot read user environment: {error}"
                        )))
                    }
                };
                match key.get_raw_value(name) {
                    Ok(value) => {
                        if value.bytes.len() as u64 > MAX_RESOURCE_BYTES {
                            return Err(AppError::InvalidInput("Oversized registry value".into()));
                        }
                        let mut bytes = (value.vtype as u32).to_le_bytes().to_vec();
                        bytes.extend(value.bytes);
                        Ok(Some(bytes))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(AppError::Config(format!(
                        "Cannot read user environment: {error}"
                    ))),
                }
            }
        }
    }

    fn write(&self, bytes: Option<&[u8]>, mode: Option<u32>) -> AppResult<()> {
        match self {
            Self::File(path) => match bytes {
                Some(bytes) => {
                    #[cfg(unix)]
                    let permissions = {
                        use std::os::unix::fs::PermissionsExt;
                        mode.map(fs::Permissions::from_mode)
                    };
                    #[cfg(not(unix))]
                    let permissions = {
                        let _ = mode;
                        None
                    };
                    safe_io::write_with_permissions(path, bytes, permissions)
                }
                None => {
                    safe_io::ensure_regular_or_missing(path)?;
                    match fs::remove_file(path) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(AppError::io(path, error)),
                    }
                }
            },
            #[cfg(windows)]
            Self::UserEnvironment(name) => {
                use winreg::enums::{HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ};
                let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
                    .create_subkey("Environment")
                    .map_err(|error| {
                        AppError::Config(format!("Cannot open user environment: {error}"))
                    })?;
                let result = match bytes {
                    Some(bytes) => {
                        if bytes.len() < 4 {
                            return Err(AppError::Config("Invalid registry snapshot".into()));
                        }
                        let kind =
                            u32::from_le_bytes(bytes[..4].try_into().map_err(|_| {
                                AppError::Config("Invalid registry snapshot".into())
                            })?);
                        let vtype = match kind {
                            1 => REG_SZ,
                            2 => REG_EXPAND_SZ,
                            _ => {
                                return Err(AppError::Unsupported(
                                    "Non-string environment values require manual recovery".into(),
                                ))
                            }
                        };
                        key.set_raw_value(
                            name,
                            &winreg::RegValue {
                                vtype,
                                bytes: bytes[4..].to_vec(),
                            },
                        )
                    }
                    None => key.delete_value(name),
                };
                match result {
                    Ok(()) => Ok(()),
                    Err(error)
                        if bytes.is_none() && error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        Ok(())
                    }
                    Err(error) => Err(AppError::Config(format!(
                        "Cannot write user environment: {error}"
                    ))),
                }
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PreparedWrite {
    pub resource: Resource,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub restore_mode: Option<u32>,
    pub write_mode: Option<u32>,
}

impl PreparedWrite {
    pub fn file(path: &Path, after: Option<Vec<u8>>, preserve_mode: bool) -> AppResult<Self> {
        let path = safe_io::resource_path(path)?;
        let resource = Resource::File(path.clone());
        let before = resource.read()?;
        #[cfg(unix)]
        let restore_mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&path)
                .ok()
                .map(|value| value.permissions().mode() & 0o777)
        };
        #[cfg(not(unix))]
        let restore_mode = None;
        Ok(Self {
            resource,
            before,
            after,
            restore_mode,
            write_mode: if preserve_mode { restore_mode } else { None },
        })
    }

    pub fn changed(&self) -> bool {
        self.before != self.after
    }

    fn validate(&self) -> AppResult<()> {
        if self
            .before
            .as_ref()
            .is_some_and(|value| value.len() as u64 > MAX_RESOURCE_BYTES)
            || self
                .after
                .as_ref()
                .is_some_and(|value| value.len() as u64 > MAX_RESOURCE_BYTES)
        {
            return Err(AppError::InvalidInput(
                "Prepared resource exceeds size limit".into(),
            ));
        }
        for mode in [self.restore_mode, self.write_mode].into_iter().flatten() {
            if mode & !0o777 != 0 {
                return Err(AppError::InvalidInput(
                    "Invalid snapshot permissions".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Collapse default/profile aliases into one physical write. Conflicting desired
/// bytes never get resolved by last-writer-wins.
pub fn deduplicate(writes: Vec<PreparedWrite>) -> AppResult<Vec<PreparedWrite>> {
    let mut unique: BTreeMap<String, PreparedWrite> = BTreeMap::new();
    for write in writes {
        write.validate()?;
        let key = write.resource.key();
        if let Some(previous) = unique.get(&key) {
            if previous.before != write.before || previous.after != write.after {
                return Err(AppError::Config(
                    "Conflicting projections for one physical resource".into(),
                ));
            }
        } else {
            unique.insert(key, write);
        }
    }
    if unique.len() > MAX_WRITES {
        return Err(AppError::InvalidInput("Too many prepared writes".into()));
    }
    let total: usize = unique
        .values()
        .map(|value| {
            value.before.as_ref().map_or(0, Vec::len) + value.after.as_ref().map_or(0, Vec::len)
        })
        .sum();
    if total > 16 * 1024 * 1024 {
        return Err(AppError::InvalidInput(
            "Prepared rollback data exceeds the 16 MiB budget".into(),
        ));
    }
    Ok(unique.into_values().collect())
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Prepared,
    Applying,
    Committed,
    RolledBack,
    RecoveryRequired,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    plan_id: String,
    phase: Phase,
    attempted: usize,
    writes: Vec<PreparedWrite>,
}

pub struct Transaction {
    path: PathBuf,
    journal: Journal,
}

impl Transaction {
    pub fn begin(path: PathBuf, plan_id: &str, writes: Vec<PreparedWrite>) -> AppResult<Self> {
        if safe_io::read_optional(&path, MAX_JOURNAL_BYTES)?.is_some() {
            return Err(AppError::Config(
                "Interrupted deployment must be recovered first".into(),
            ));
        }
        let writes = deduplicate(writes)?;
        for write in &writes {
            if write.resource.read()? != write.before {
                return Err(AppError::Config(
                    "Prepared target changed before transaction start".into(),
                ));
            }
        }
        let value = Self {
            path,
            journal: Journal {
                version: 2,
                plan_id: plan_id.into(),
                phase: Phase::Prepared,
                attempted: 0,
                writes,
            },
        };
        value.persist()?;
        Ok(value)
    }

    pub fn apply(&mut self) -> AppResult<()> {
        for index in 0..self.journal.writes.len() {
            let write = &self.journal.writes[index];
            if write.resource.read()? != write.before {
                return Err(AppError::Config(
                    "Target changed immediately before its write".into(),
                ));
            }
            // Record the intent BEFORE the mutation, including its expected after
            // bytes. Recovery also handles a crash between write and journal update.
            self.journal.attempted = index + 1;
            self.journal.phase = Phase::Applying;
            self.persist()?;
            let write = &self.journal.writes[index];
            if write.changed() {
                write
                    .resource
                    .write(write.after.as_deref(), write.write_mode)?;
                if write.resource.read()? != write.after {
                    return Err(AppError::Config("Post-write verification failed".into()));
                }
            }
        }
        Ok(())
    }

    /// Try every independent compensation, retaining the journal on ANY conflict.
    /// Caller must keep this journal when final audit persistence also failed.
    pub fn rollback(&mut self) -> AppResult<()> {
        let mut failures = 0usize;
        for write in self.journal.writes[..self.journal.attempted].iter().rev() {
            if !write.changed() {
                continue;
            }
            let result = (|| -> AppResult<()> {
                let current = write.resource.read()?;
                if current == write.before {
                    return Ok(());
                }
                if current != write.after {
                    return Err(AppError::Config(
                        "External edit prevents safe rollback".into(),
                    ));
                }
                write
                    .resource
                    .write(write.before.as_deref(), write.restore_mode)?;
                if write.resource.read()? != write.before {
                    return Err(AppError::Config("Rollback verification failed".into()));
                }
                Ok(())
            })();
            if result.is_err() {
                failures += 1;
            }
        }
        self.journal.phase = if failures == 0 {
            Phase::RolledBack
        } else {
            Phase::RecoveryRequired
        };
        self.persist()?;
        if failures != 0 {
            return Err(AppError::Config(format!(
                "{failures} resource(s) could not be restored safely; recovery journal retained"
            )));
        }
        Ok(())
    }

    pub fn complete(mut self) -> AppResult<()> {
        // Only after audit state is durable may recovery treat this as committed.
        self.journal.phase = Phase::Committed;
        self.persist()?;
        self.clear()
    }

    pub fn clear(self) -> AppResult<()> {
        fs::remove_file(&self.path).map_err(|error| AppError::io(&self.path, error))
    }

    fn persist(&self) -> AppResult<()> {
        let bytes = serde_json::to_vec(&self.journal)
            .map_err(|_| AppError::Config("Cannot serialize deployment journal".into()))?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(AppError::InvalidInput(
                "Deployment rollback journal exceeds storage limit".into(),
            ));
        }
        safe_io::write_private(&self.path, &bytes)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryView {
    pub deployment_plan_id: String,
    pub digest: String,
    pub committed: bool,
    pub resource_count: usize,
    pub conflict_count: usize,
    pub unknown_resource_count: usize,
    pub unreadable_resource_count: usize,
}

// Validate journal metadata without following any resource named by the journal.
// This also permits a reviewed, target-free dismissal when a client disappeared.
fn load_checked(path: &Path) -> AppResult<Transaction> {
    let bytes = safe_io::read_optional(path, MAX_JOURNAL_BYTES)?
        .ok_or_else(|| AppError::InvalidInput("No deployment recovery journal exists".into()))?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Config("Unrecognized or corrupt recovery journal; no writes performed".into())
    })?;
    if journal.version != 2
        || journal.plan_id.is_empty()
        || journal.plan_id.len() > 256
        || journal.attempted > journal.writes.len()
        || journal.writes.len() > MAX_WRITES
    {
        return Err(AppError::Config(
            "Unsupported recovery journal version or bounds".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for write in &journal.writes {
        write.validate()?;
        let key = write.resource.key();
        if !seen.insert(key) {
            return Err(AppError::Config(
                "Recovery journal references a duplicate physical resource; no writes performed"
                    .into(),
            ));
        }
    }
    // Reuse aggregate limits, but do not reorder the recorded attempt sequence.
    deduplicate(journal.writes.clone())?;
    Ok(Transaction {
        path: path.to_path_buf(),
        journal,
    })
}

pub fn recovery_view(path: &Path, allowed: &[Resource]) -> AppResult<RecoveryView> {
    use sha2::{Digest, Sha256};
    let tx = load_checked(path)?;
    let bytes = serde_json::to_vec(&tx.journal)
        .map_err(|_| AppError::Config("Cannot fingerprint recovery journal".into()))?;
    let mut conflicts = 0;
    let mut unknown = 0;
    let mut unreadable = 0;
    let allowed = allowed
        .iter()
        .map(Resource::key)
        .collect::<std::collections::HashSet<_>>();
    for (index, write) in tx.journal.writes.iter().enumerate() {
        if !allowed.contains(&write.resource.key()) {
            unknown += 1;
            continue; // Never inspect a path/registry value outside native discovery.
        }
        if index < tx.journal.attempted && !matches!(tx.journal.phase, Phase::Committed) {
            match write.resource.read() {
                Ok(current) if current != write.before && current != write.after => conflicts += 1,
                Err(_) => unreadable += 1,
                _ => {}
            }
        }
    }
    Ok(RecoveryView {
        deployment_plan_id: tx.journal.plan_id.clone(),
        digest: format!("{:x}", Sha256::digest(bytes)),
        committed: tx.journal.phase == Phase::Committed,
        resource_count: tx.journal.writes.len(),
        conflict_count: conflicts,
        unknown_resource_count: unknown,
        unreadable_resource_count: unreadable,
    })
}

pub fn recover(path: &Path, expected_digest: &str, allowed: &[Resource]) -> AppResult<()> {
    let current = recovery_view(path, allowed)?;
    if current.digest != expected_digest {
        return Err(AppError::InvalidInput(
            "Recovery journal changed after preview".into(),
        ));
    }
    let mut tx = load_checked(path)?;
    if tx.journal.phase != Phase::Committed {
        let allowed = allowed
            .iter()
            .map(Resource::key)
            .collect::<std::collections::HashSet<_>>();
        if tx
            .journal
            .writes
            .iter()
            .any(|write| !allowed.contains(&write.resource.key()))
        {
            return Err(AppError::Config("A recovery target is no longer available. Review keeping current files to end recovery without restoring targets.".into()));
        }
        tx.rollback()?;
    }
    tx.clear()
}

/// Explicitly abandon rollback after a separate native-held review. Only the
/// bounded, backend-owned journal is removed. Target paths are never accessed,
/// and no deployment is recorded as successful by this operation.
pub fn keep_current(path: &Path, expected_digest: &str) -> AppResult<()> {
    let current = recovery_view(path, &[])?;
    if current.digest != expected_digest {
        return Err(AppError::InvalidInput(
            "Recovery journal changed after preview".into(),
        ));
    }
    load_checked(path)?.clear()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_recovers_attempted_writes_without_overwriting_other_paths() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("models.json");
        fs::write(&file, b"before").expect("seed");
        let prepared = PreparedWrite::file(&file, Some(b"after".to_vec()), false).expect("prepare");
        let allowed = vec![prepared.resource.clone()];
        let path = dir.path().join("journal");
        let mut tx = Transaction::begin(path.clone(), "id", vec![prepared]).expect("begin");
        tx.apply().expect("apply");
        drop(tx);
        let view = recovery_view(&path, &allowed).expect("preview");
        assert!(recover(&path, &view.digest, &[]).is_err());
        assert_eq!(fs::read(&file).expect("read"), b"after");
        recover(&path, &view.digest, &allowed).expect("recover");
        assert_eq!(fs::read(&file).expect("read"), b"before");
        assert!(!path.exists());
    }

    #[test]
    fn reviewed_retention_resolves_external_conflicts_without_changing_any_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("models.json");
        fs::write(&file, b"before-private-secret").unwrap();
        let write =
            PreparedWrite::file(&file, Some(b"after-private-secret".to_vec()), false).unwrap();
        let allowed = vec![write.resource.clone()];
        let path = dir.path().join("journal");
        let mut tx = Transaction::begin(path.clone(), "id", vec![write]).unwrap();
        tx.apply().unwrap();
        fs::write(&file, b"external-edit").unwrap();
        let view = recovery_view(&path, &allowed).unwrap();
        assert_eq!(view.conflict_count, 1);
        assert!(!serde_json::to_string(&view)
            .unwrap()
            .contains("private-secret"));
        assert!(recover(&path, &view.digest, &allowed).is_err());
        assert!(path.exists());
        // Failed restoration persisted a new journal; the old review cannot dismiss it.
        assert!(keep_current(&path, &view.digest).is_err());
        let reviewed = recovery_view(&path, &[]).unwrap();
        keep_current(&path, &reviewed.digest).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&file).unwrap(), b"external-edit");
        assert!(keep_current(&path, &reviewed.digest).is_err());
        // The resolved journal no longer blocks a fresh, separately reviewed transaction.
        Transaction::begin(
            path,
            "next",
            vec![PreparedWrite::file(&file, Some(b"next".to_vec()), false).unwrap()],
        )
        .unwrap()
        .clear()
        .unwrap();
    }

    #[test]
    fn removed_targets_can_be_dismissed_without_reading_journal_resource_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("models.json");
        let path = dir.path().join("journal");
        let mut tx = Transaction::begin(
            path.clone(),
            "id",
            vec![PreparedWrite::file(&file, Some(b"after".to_vec()), false).unwrap()],
        )
        .unwrap();
        tx.apply().unwrap();
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap(); // Reading this as a source would fail; never follow it.
        let view = recovery_view(&path, &[]).unwrap();
        assert_eq!(view.unknown_resource_count, 1);
        assert_eq!(view.unreadable_resource_count, 0);
        assert!(recover(&path, &view.digest, &[]).is_err());
        keep_current(&path, &view.digest).unwrap();
        assert!(file.is_dir());
        assert!(!path.exists());
    }

    #[test]
    fn unreadable_allowed_target_still_has_a_target_free_retention_exit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("models.json");
        let path = dir.path().join("journal");
        let write = PreparedWrite::file(&file, Some(b"after".to_vec()), false).unwrap();
        let allowed = vec![write.resource.clone()];
        let mut tx = Transaction::begin(path.clone(), "id", vec![write]).unwrap();
        tx.apply().unwrap();
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        let view = recovery_view(&path, &allowed).unwrap();
        assert_eq!(view.unknown_resource_count, 0);
        assert_eq!(view.unreadable_resource_count, 1);
        keep_current(&path, &view.digest).unwrap();
        assert!(file.is_dir());
        assert!(!path.exists());
    }

    #[test]
    fn changed_or_corrupt_journal_cannot_be_dismissed_with_a_prior_review() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        let mut tx = Transaction::begin(path.clone(), "id", vec![]).unwrap();
        let view = recovery_view(&path, &[]).unwrap();
        tx.rollback().unwrap();
        assert!(keep_current(&path, &view.digest).is_err());
        assert!(path.exists());
        fs::write(&path, b"invalid").unwrap();
        assert!(keep_current(&path, &view.digest).is_err());
        assert_eq!(fs::read(path).unwrap(), b"invalid");
    }

    #[test]
    fn aliases_are_one_write_and_noop_preserves_original_bytes() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("models.json");
        fs::write(&file, b"// original formatting\n[]").expect("seed");
        let bytes = fs::read(&file).expect("read");
        let write = PreparedWrite::file(&file, Some(bytes.clone()), true).expect("prepare");
        let mut tx =
            Transaction::begin(dir.path().join("journal"), "id", vec![write.clone(), write])
                .expect("begin");
        assert_eq!(tx.journal.writes.len(), 1);
        tx.apply().expect("apply");
        tx.complete().expect("complete");
        assert_eq!(fs::read(&file).expect("read"), bytes);
    }

    #[test]
    fn later_target_failure_restores_already_written_file() {
        let dir = tempfile::tempdir().expect("temp");
        let first = dir.path().join("a");
        let second = dir.path().join("b");
        fs::write(&first, b"before").expect("seed");
        fs::write(&second, b"before").expect("seed");
        let writes = vec![
            PreparedWrite::file(&first, Some(b"after".to_vec()), false).expect("prepare"),
            PreparedWrite::file(&second, Some(b"after".to_vec()), false).expect("prepare"),
        ];
        let mut tx = Transaction::begin(dir.path().join("journal"), "id", writes).expect("begin");
        fs::write(&second, b"external").expect("edit");
        assert!(tx.apply().is_err());
        tx.rollback().expect("rollback");
        assert_eq!(fs::read(first).expect("read"), b"before");
        assert_eq!(fs::read(second).expect("read"), b"external");
    }

    #[test]
    fn rollback_refuses_newer_work_but_restores_independent_targets() {
        let dir = tempfile::tempdir().expect("temp");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, b"before").expect("seed");
        fs::write(&b, b"before").expect("seed");
        let mut tx = Transaction::begin(
            dir.path().join("journal"),
            "id",
            vec![
                PreparedWrite::file(&a, Some(b"after".to_vec()), false).expect("prepare"),
                PreparedWrite::file(&b, Some(b"after".to_vec()), false).expect("prepare"),
            ],
        )
        .expect("begin");
        tx.apply().expect("apply");
        fs::write(&b, b"external").expect("edit");
        assert!(tx.rollback().is_err());
        assert_eq!(fs::read(a).expect("read"), b"before");
        assert_eq!(fs::read(b).expect("read"), b"external");
        assert!(tx.path.exists());
    }

    #[test]
    fn audit_failure_can_restore_newly_created_files() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("new");
        let mut tx = Transaction::begin(
            dir.path().join("journal"),
            "id",
            vec![PreparedWrite::file(&file, Some(b"after".to_vec()), false).expect("prepare")],
        )
        .expect("begin");
        tx.apply().expect("apply");
        // Simulate audit failure: compensate, but do NOT clear the durable evidence.
        tx.rollback().expect("rollback");
        assert!(!file.exists());
        assert!(tx.path.exists());
    }
}
