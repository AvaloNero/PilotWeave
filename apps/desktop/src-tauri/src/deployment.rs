use crate::domain::{ClientKind, ClientTarget, Connection, DeploymentPlan};
use crate::error::{AppError, AppResult};
use crate::{adapters, safe_io, transaction, validation};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
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

const PLAN_TTL_SECONDS: i64 = 15 * 60;

pub struct StoredPlan {
    pub plan: DeploymentPlan,
    pub writes: Vec<transaction::PreparedWrite>,
    connection_digest: String,
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
        secret: Option<&str>,
        mut plan: DeploymentPlan,
        targets: &[ClientTarget],
    ) -> AppResult<DeploymentPlan> {
        self.purge_expired();
        if self.plans.len() >= 16 {
            return Err(AppError::InvalidInput(
                "Too many pending deployment plans".into(),
            ));
        }
        validation::validate_connection(connection)?;
        let mut fingerprints = BTreeMap::new();
        let mut writes = Vec::new();
        for operation in &mut plan.operations {
            let target = targets
                .iter()
                .find(|value| value.id == operation.target_id)
                .ok_or_else(|| AppError::InvalidInput("Deployment target disappeared".into()))?;
            fingerprints.insert(target.id.clone(), fingerprint_target(target)?);
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
        let writes = transaction::deduplicate(writes)?;
        let stored = StoredPlan {
            connection_digest: connection_digest(connection, secret)?,
            plan: plan.clone(),
            writes,
            target_fingerprints: fingerprints,
        };
        self.plans.insert(plan.id.clone(), stored);
        Ok(plan)
    }

    pub fn consume(&mut self, id: &str) -> AppResult<StoredPlan> {
        self.purge_expired();
        self.plans.remove(id).ok_or_else(|| {
            AppError::InvalidInput(
                "Deployment plan is missing, expired or already consumed; preview again".into(),
            )
        })
    }

    fn purge_expired(&mut self) {
        let cutoff = Utc::now() - Duration::seconds(PLAN_TTL_SECONDS);
        self.plans.retain(|_, value| value.plan.created_at > cutoff);
    }
}

pub fn validate_plan(
    stored: &StoredPlan,
    connection: &Connection,
    secret: Option<&str>,
    targets: &[ClientTarget],
) -> AppResult<()> {
    if Utc::now() - stored.plan.created_at >= Duration::seconds(PLAN_TTL_SECONDS)
        || connection_digest(connection, secret)? != stored.connection_digest
    {
        return Err(AppError::InvalidInput(
            "Connection, credential or plan lifetime changed; preview again".into(),
        ));
    }
    for (id, expected) in &stored.target_fingerprints {
        let target = targets
            .iter()
            .find(|value| &value.id == id)
            .ok_or_else(|| AppError::InvalidInput("Target disappeared after preview".into()))?;
        if fingerprint_target(target)? != *expected {
            return Err(AppError::InvalidInput(
                "Target changed after preview; preview again".into(),
            ));
        }
    }
    for write in &stored.writes {
        if write.resource.read()? != write.before {
            return Err(AppError::InvalidInput(
                "Prepared configuration changed; preview again".into(),
            ));
        }
    }
    Ok(())
}

fn connection_digest(connection: &Connection, secret: Option<&str>) -> AppResult<String> {
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(connection)
            .map_err(|_| AppError::Config("Cannot fingerprint connection".into()))?,
    );
    hash.update([0]);
    hash.update(secret.unwrap_or_default().as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

pub fn fingerprint_target(target: &ClientTarget) -> AppResult<String> {
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
        match resource.read()? {
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
            .insert(
                &connection,
                None,
                plan.clone(),
                std::slice::from_ref(&target),
            )
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
            .insert(
                &connection,
                None,
                plan.clone(),
                std::slice::from_ref(&target),
            )
            .expect("insert");
        fs::write(&path, b"[1]").expect("external change");
        let stored = store.consume(&plan.id).expect("consume");
        assert!(validate_plan(&stored, &connection, None, &[target]).is_err());
    }
}
