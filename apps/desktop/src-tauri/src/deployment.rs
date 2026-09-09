use crate::domain::{ClientKind, ClientTarget, Connection, DeploymentPlan};
use crate::error::{AppError, AppResult};
use crate::{adapters, safe_io, transaction};
#[cfg(test)]
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
pub fn recovery_resources() -> AppResult<Vec<transaction::Resource>> {
    let mut resources = adapters::copilot_cli::observed_resources()?;
    for target in adapters::discover_all() {
        if target.kind == ClientKind::VsCodeCopilot && target.detected {
            if let Some(path) = target.path {
                let path = safe_io::resource_path(Path::new(&path))?;
                resources.push(transaction::Resource::File(safe_io::resource_path(
                    &path.with_extension("json.pilotweave.bak"),
                )?));
                resources.push(transaction::Resource::File(path));
            }
        }
    }
    Ok(resources)
}

#[cfg(test)]
use std::fs;

mod planner;
pub use planner::{validate_plan, PlanContext, PlanStore, StoredPlan};

pub(super) fn prepare_writes(
    connection: &Connection,
    secret: Option<&str>,
    plan: &mut DeploymentPlan,
    targets: &[ClientTarget],
) -> AppResult<Vec<transaction::PreparedWrite>> {
    let mut writes = Vec::new();
    for operation in &mut plan.operations {
        let target = targets
            .iter()
            .find(|value| value.id == operation.target_id)
            .ok_or_else(|| AppError::InvalidInput("Deployment target disappeared".into()))?;
        if !operation.supported {
            continue;
        }
        if !target.detected || !target.supports_write {
            return Err(AppError::Unsupported("Target is not writable".into()));
        }
        let prepared = match target.kind {
            ClientKind::VsCodeCopilot => {
                let path = Path::new(target.path.as_deref().ok_or_else(|| {
                    AppError::Config("Missing VS Code configuration path".into())
                })?);
                let write = adapters::vscode::prepare(connection, secret, path)?;
                let mut prepared = Vec::new();
                if write.changed() {
                    if let Some(before) = &write.before {
                        if let Some(backup) = adapters::vscode::original_backup(path, before)? {
                            prepared.push(backup);
                        }
                    }
                }
                prepared.push(write);
                prepared
            }
            ClientKind::CopilotCli => adapters::copilot_cli::prepare(connection, secret)?,
            ClientKind::GithubCopilotApp => {
                return Err(AppError::Unsupported(
                    "Copilot app remains manual/read-only".into(),
                ))
            }
        };
        let changed = prepared.iter().filter(|write| write.changed()).count();
        operation.changes.push(format!(
            "{changed} physical resource(s) require changes; identical data is not rewritten"
        ));
        writes.extend(prepared);
    }
    transaction::deduplicate(writes)
}

pub fn fingerprint_target(target: &ClientTarget) -> AppResult<String> {
    fingerprint_target_with_writes(target, None)
}

pub(super) fn fingerprint_target_with_writes(
    target: &ClientTarget,
    writes: Option<&[transaction::PreparedWrite]>,
) -> AppResult<String> {
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(target)
            .map_err(|_| AppError::Config("Cannot fingerprint target".into()))?,
    );
    let resources = match target.kind {
        ClientKind::CopilotCli if target.detected => adapters::copilot_cli::observed_resources()?,
        ClientKind::VsCodeCopilot if target.detected => vec![transaction::Resource::File(
            safe_io::resource_path(Path::new(
                target
                    .path
                    .as_deref()
                    .ok_or_else(|| AppError::Config("Missing target path".into()))?,
            ))?,
        )],
        _ => Vec::new(),
    };
    for resource in resources {
        hash.update(resource.key().as_bytes());
        let bytes = match writes.and_then(|writes| {
            writes
                .iter()
                .find(|write| write.resource.key() == resource.key())
        }) {
            Some(write) => write.after.clone(),
            None => resource.read()?,
        };
        match bytes {
            Some(bytes) => {
                hash.update([1]);
                hash.update(bytes);
            }
            None => hash.update([0]),
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn journal_path(state_path: &Path) -> PathBuf {
    state_path.with_file_name("deployment-journal.json")
}

pub fn recovery_required(state_path: &Path) -> bool {
    // Broken links and unreadable journal paths also block writes.
    std::fs::symlink_metadata(journal_path(state_path))
        .map(|_| true)
        .unwrap_or_else(|error| error.kind() != std::io::ErrorKind::NotFound)
}

pub fn ensure_no_pending_journal(state_path: &Path) -> AppResult<()> {
    if recovery_required(state_path) {
        return Err(AppError::Config(
            "An interrupted deployment requires recovery; managed writes are disabled".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ApiProtocol, ClientStatus, ModelCapabilities, ModelSpec, ProviderKind};
    use std::collections::BTreeMap;

    pub(super) fn connection() -> Connection {
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

    pub(super) fn target(path: &Path) -> ClientTarget {
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

    pub(super) fn plan(connection: &Connection, target: &ClientTarget) -> DeploymentPlan {
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
        let reviewed = store
            .insert(
                &connection,
                None,
                plan.clone(),
                std::slice::from_ref(&target),
                PlanContext::new("test-owner", "state".into(), None),
            )
            .expect("insert");
        assert_eq!(
            store.consume(&reviewed.id).expect("consume").plan.id,
            reviewed.id
        );
        assert!(store.consume(&reviewed.id).is_err());
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
        let reviewed = store
            .insert(
                &connection,
                None,
                plan.clone(),
                std::slice::from_ref(&target),
                PlanContext::new("test-owner", "state".into(), None),
            )
            .expect("insert");
        fs::write(&path, b"[1]").expect("external change");
        let stored = store.consume(&reviewed.id).expect("consume");
        assert!(validate_plan(
            &stored,
            &connection,
            &[target],
            &PlanContext::new("test-owner", "state".into(), None)
        )
        .is_err());
    }

    #[test]
    fn prepared_output_fingerprint_never_adopts_a_later_external_edit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        fs::write(&path, b"[]").unwrap();
        let connection = connection();
        let target = target(&path);
        let mut plans = PlanStore::default();
        let reviewed = plans
            .insert(
                &connection,
                None,
                plan(&connection, &target),
                std::slice::from_ref(&target),
                PlanContext::new("test-owner", "state".into(), None),
            )
            .unwrap();
        let stored = plans.consume(&reviewed.id).unwrap();
        let expected = stored.after_fingerprints[&target.id].clone();
        let mut transaction = transaction::Transaction::begin(
            root.path().join("journal.json"),
            &reviewed.id,
            stored.writes,
        )
        .unwrap();
        transaction.apply().unwrap();
        assert_eq!(fingerprint_target(&target).unwrap(), expected);
        fs::write(&path, b"[{\"external\":true}]").unwrap();
        assert_ne!(fingerprint_target(&target).unwrap(), expected);
        assert!(transaction.rollback().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"[{\"external\":true}]");
    }
}
