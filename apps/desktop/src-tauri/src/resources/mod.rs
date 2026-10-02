//! Client-neutral authored resources. Paths and projection formats belong to native adapters.
use crate::{
    domain::ClientTarget,
    error::{AppError, AppResult},
    fingerprint, transaction,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) mod commands;
#[cfg(test)]
mod tests;

const MAX_RESOURCES: usize = 64;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_BINDINGS: usize = 192;
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceKind {
    Mcp,
    Skill,
    Instructions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedResource {
    pub id: String,
    pub name: String,
    pub kind: ResourceKind,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceInput {
    pub id: Option<String>,
    pub name: String,
    pub kind: ResourceKind,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceBinding {
    pub owner_id: String,
    pub resource_id: String,
    pub target_id: String,
    pub path_identity: String,
    pub projection_fingerprint: String,
    pub target_fingerprint: String,
    pub deployed_at: DateTime<Utc>,
}

pub fn validate_resource(record: &SharedResource) -> AppResult<()> {
    if Uuid::parse_str(&record.id).map_or(true, |id| id.is_nil())
        || record.name.trim().is_empty()
        || record.name.len() > 128
        || record.name.chars().any(char::is_control)
        || record.body.len() > MAX_BODY_BYTES
        || record.body.contains('\0')
        || record
            .body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        || (!record.archived && record.body.trim().is_empty())
    {
        return Err(AppError::InvalidInput(
            "Resource identity, name or content exceeds its supported bounds".into(),
        ));
    }
    if record.kind == ResourceKind::Mcp && !record.archived {
        let url = url::Url::parse(record.body.trim()).map_err(|_| {
            AppError::InvalidInput("MCP requires a credential-free HTTPS URL".into())
        })?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || record.body.len() > 2048
            || record.body.contains(['\r', '\n', '$', '{', '}'])
        {
            return Err(AppError::InvalidInput("MCP URL must use HTTPS with no credentials, query, fragment or interpolation. Secret-bearing and local-process configurations require manual client setup".into()));
        }
    }
    Ok(())
}

pub fn validate_state(
    records: &[SharedResource],
    bindings: &[ResourceBinding],
    owner: &str,
) -> AppResult<()> {
    if records.len() > MAX_RESOURCES * 2
        || records.iter().filter(|r| !r.archived).count() > MAX_RESOURCES
        || bindings.len() > MAX_BINDINGS
    {
        return Err(AppError::InvalidInput(
            "Resource catalog exceeds its bounds".into(),
        ));
    }
    let mut ids = HashSet::new();
    for record in records {
        validate_resource(record)?;
        if !ids.insert(&record.id) {
            return Err(AppError::InvalidInput("Duplicate resource identity".into()));
        }
    }
    let mut keys = HashSet::new();
    for binding in bindings {
        if binding.owner_id != owner
            || !records
                .iter()
                .any(|r| r.id == binding.resource_id && !r.archived)
            || binding.target_id.is_empty()
            || binding.target_id.len() > 128
            || binding.target_id.chars().any(char::is_control)
            || !keys.insert((&binding.resource_id, &binding.target_id))
            || [
                &binding.path_identity,
                &binding.projection_fingerprint,
                &binding.target_fingerprint,
            ]
            .iter()
            .any(|value| value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AppError::InvalidInput(
                "Invalid resource ownership proof".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOperation {
    pub target_id: String,
    pub title: String,
    pub supported: bool,
    pub changed: bool,
    pub detail: String,
    pub change: ResourceChange,
    pub destination: Option<String>,
    /// Only this resource's reviewed projection, never a whole client config.
    pub content_preview: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceChange {
    Create,
    Update,
    Remove,
    Unchanged,
    Manual,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePlan {
    pub id: String,
    pub resource_id: String,
    pub resource_name: String,
    pub revoke_and_delete: bool,
    pub operations: Vec<ResourceOperation>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOverview {
    pub resources: Vec<SharedResource>,
    pub bindings: Vec<ResourceBinding>,
    pub detail: String,
}

#[derive(Clone, Serialize)]
pub(super) struct Target {
    id: String,
    label: String,
    path: Option<PathBuf>,
    mcp_key: Option<&'static str>,
    supported: bool,
    detail: String,
}

fn roots() -> AppResult<(PathBuf, PathBuf)> {
    let home = crate::platform::home_dir()
        .ok_or_else(|| AppError::Config("Cannot resolve resource home".into()))?;
    let config = crate::platform::config_dir()
        .ok_or_else(|| AppError::Config("Cannot resolve profile configuration root".into()))?;
    Ok((home.join(".copilot"), config))
}

fn targets_at(
    record: &SharedResource,
    clients: &[ClientTarget],
    copilot: &std::path::Path,
    config: &std::path::Path,
    custom_home: bool,
) -> Vec<Target> {
    use crate::domain::ClientKind;
    let cli = clients
        .iter()
        .any(|t| t.kind == ClientKind::CopilotCli && t.detected);
    let vscode = clients
        .iter()
        .any(|t| t.kind == ClientKind::VsCodeCopilot && t.detected);
    let file = format!("pilotweave-{}.instructions.md", record.id);
    let mut result = Vec::new();
    if record.kind == ResourceKind::Skill {
        result.push(Target { id: "copilot-shared:skills".into(), label: "Shared personal Skills (VS Code / Copilot CLI)".into(),
            path: Some(copilot.join("skills").join(format!("pilotweave-{}", record.id)).join("SKILL.md")), mcp_key: None,
            supported: (cli || vscode) && !custom_home,
            detail: "Public personal skill root. Repository skills and client settings may override discovery; no asset/script directory is copied".into() });
    } else {
        result.push(Target { id: "copilot-cli:user-resources".into(), label: "Copilot CLI personal resources".into(),
            path: Some(if record.kind == ResourceKind::Mcp { copilot.join("mcp-config.json") } else { copilot.join("instructions").join(&file) }),
            mcp_key: (record.kind == ResourceKind::Mcp).then_some("mcpServers"), supported: cli && !custom_home,
            detail: "Public user-level configuration; repository-level definitions take precedence. Custom COPILOT_HOME is manual".into() });
        for (edition, folder) in [("stable", "Code"), ("insiders", "Code - Insiders")] {
            let id = format!("vscode:{edition}:default");
            let detected = clients.iter().any(|t| t.id == id && t.detected);
            result.push(Target { id, label: format!("VS Code {edition} default profile"),
                path: Some(if record.kind == ResourceKind::Mcp { config.join(folder).join("User/mcp.json") }
                    else { config.join(folder).join("User/prompts").join(&file) }),
                mcp_key: (record.kind == ResourceKind::Mcp).then_some("servers"), supported: detected,
                detail: "Public default-profile resource path. Named profiles and workspace overrides are manual, not reported as synchronized".into() });
        }
    }
    result.push(Target { id: "github-copilot-app:manual".into(), label: "GitHub Copilot app".into(), path: None, mcp_key: None,
        supported: false, detail: "Manual: this adapter has no verified stable resource-write interface; private app stores are never inspected".into() });
    result
}

fn targets(record: &SharedResource, clients: &[ClientTarget]) -> AppResult<Vec<Target>> {
    let (copilot, config) = roots()?;
    Ok(targets_at(
        record,
        clients,
        &copilot,
        &config,
        crate::adapters::copilot_cli::has_custom_home()?,
    ))
}

fn key(owner: &str, record: &SharedResource) -> String {
    format!("pilotweave-{owner}-{}", record.id)
}
fn markdown(owner: &str, record: &SharedResource) -> AppResult<Vec<u8>> {
    let name = format!("pilotweave-{}", record.id);
    let description = serde_json::to_string(&record.name)
        .map_err(|_| AppError::Config("Cannot render resource name".into()))?;
    let front = if record.kind == ResourceKind::Skill {
        format!("---\nname: {name}\ndescription: {description}\n---\n")
    } else {
        "---\napplyTo: \"**\"\n---\n".into()
    };
    Ok(format!(
        "{front}\n<!-- PilotWeave owner={owner} resource={} -->\n\n{}\n",
        record.id,
        record.body.trim_end()
    )
    .into_bytes())
}

fn parse_config(bytes: Option<&[u8]>) -> AppResult<serde_json::Value> {
    let text = std::str::from_utf8(bytes.unwrap_or(b"{}"))
        .map_err(|_| AppError::Config("Resource configuration is not UTF-8".into()))?;
    let value: serde_json::Value = json5::from_str(text).map_err(|_| {
        AppError::Config(
            "Unsupported/malformed MCP configuration; no client data was written".into(),
        )
    })?;
    if !value.is_object() {
        return Err(AppError::Config(
            "MCP configuration must be an object".into(),
        ));
    }
    Ok(value)
}

fn prepare(
    record: &SharedResource,
    owner: &str,
    target: &Target,
    bindings: &[ResourceBinding],
    revoke: bool,
) -> AppResult<(transaction::PreparedWrite, Option<ResourceBinding>)> {
    let path = target
        .path
        .as_ref()
        .ok_or_else(|| AppError::Unsupported(target.detail.clone()))?;
    let path = crate::safe_io::resource_path(path)?;
    let resource = transaction::Resource::File(path.clone());
    let path_identity = crate::deployment::ownership::resource_id(&resource);
    let before = crate::safe_io::read_optional(&path, MAX_CONFIG_BYTES)?;
    let proof = bindings
        .iter()
        .find(|b| b.resource_id == record.id && b.target_id == target.id);
    let server_key = key(owner, record);
    let (current, after, projection) = if let Some(field) = target.mcp_key {
        let mut value = parse_config(before.as_deref())?;
        if value.get(field).is_some_and(|v| !v.is_object()) {
            return Err(AppError::Config(
                "Unsupported MCP server-list schema; no data was written".into(),
            ));
        }
        let current = value
            .get(field)
            .and_then(|v| v.get(&server_key))
            .map(|v| fingerprint::json("resource-projection-v1", v))
            .transpose()?;
        let desired = if field == "mcpServers" {
            serde_json::json!({ "type": "http", "url": record.body.trim(), "tools": ["*"] })
        } else {
            serde_json::json!({ "type": "http", "url": record.body.trim() })
        };
        let projection = (!revoke)
            .then(|| fingerprint::json("resource-projection-v1", &desired))
            .transpose()?;
        let mut changed = false;
        if revoke {
            if let Some(servers) = value.get_mut(field).and_then(|v| v.as_object_mut()) {
                changed = servers.remove(&server_key).is_some();
            }
        } else {
            if value.get(field).is_none() {
                value[field] = serde_json::json!({});
            }
            changed = value[field].get(&server_key) != Some(&desired);
            value[field][&server_key] = desired;
        }
        let after = if !changed {
            before.clone()
        } else {
            let mut bytes = serde_json::to_vec_pretty(&value)
                .map_err(|_| AppError::Config("Cannot render MCP configuration".into()))?;
            bytes.push(b'\n');
            Some(bytes)
        };
        (current, after, projection)
    } else {
        let current = before
            .as_deref()
            .map(|bytes| fingerprint::bytes("resource-projection-v1", Some(bytes)));
        let after = if revoke {
            None
        } else {
            Some(markdown(owner, record)?)
        };
        let projection = after
            .as_deref()
            .map(|bytes| fingerprint::bytes("resource-projection-v1", Some(bytes)));
        (current, after, projection)
    };
    match (&current, proof) {
        (Some(current), Some(proof))
            if proof.owner_id == owner
                && proof.path_identity == path_identity
                && &proof.projection_fingerprint == current => {}
        (None, None) => {}
        (None, Some(proof))
            if revoke && proof.owner_id == owner && proof.path_identity == path_identity => {}
        _ => return Err(crate::deployment::ownership::conflict()),
    }
    let write = transaction::PreparedWrite::file(&path, after, false)?;
    if write.before != before {
        return Err(AppError::PlanChanged);
    }
    let binding = projection.map(|projection_fingerprint| ResourceBinding {
        owner_id: owner.into(),
        resource_id: record.id.clone(),
        target_id: target.id.clone(),
        path_identity,
        projection_fingerprint,
        target_fingerprint: fingerprint::bytes("resource-target-v1", write.after.as_deref()),
        deployed_at: Utc::now(),
    });
    Ok((write, binding))
}

fn describe_projection(
    operation: &mut ResourceOperation,
    write: &transaction::PreparedWrite,
    target: &Target,
    owner: &str,
    record: &SharedResource,
    revoke: bool,
) -> AppResult<()> {
    operation.destination = Some(write.resource.key());
    let existed = if let Some(field) = target.mcp_key {
        parse_config(write.before.as_deref())?[field]
            .get(key(owner, record))
            .is_some()
    } else {
        write.before.is_some()
    };
    operation.change = if !write.changed() {
        ResourceChange::Unchanged
    } else if revoke {
        ResourceChange::Remove
    } else if existed {
        ResourceChange::Update
    } else {
        ResourceChange::Create
    };
    if !revoke {
        operation.content_preview =
            if let Some(field) = target.mcp_key {
                // Unmanaged MCP entries can contain credentials. Return only the
                // native-generated owned entry, even for an unchanged projection.
                let server_key = key(owner, record);
                let after = parse_config(write.after.as_deref())?;
                let mut owned = serde_json::json!({});
                owned[field] = serde_json::json!({});
                owned[field][&server_key] = after[field][&server_key].clone();
                Some(serde_json::to_string_pretty(&owned).map_err(|_| {
                    AppError::Config("Cannot render the owned resource preview".into())
                })?)
            } else {
                write
                    .after
                    .as_ref()
                    .map(|bytes| {
                        String::from_utf8(bytes.clone()).map_err(|_| {
                            AppError::Config("Prepared resource content is not UTF-8".into())
                        })
                    })
                    .transpose()?
            };
    }
    Ok(())
}

pub struct StoredResourcePlan {
    pub plan: ResourcePlan,
    pub writes: Vec<transaction::PreparedWrite>,
    pub bindings_after: Vec<ResourceBinding>,
    state_revision: String,
    targets_revision: String,
    expires: Instant,
}

#[derive(Default)]
pub struct ResourcePlans {
    plans: HashMap<String, StoredResourcePlan>,
}
impl ResourcePlans {
    pub fn preview(
        &mut self,
        record: &SharedResource,
        owner: &str,
        revision: String,
        clients: &[ClientTarget],
        bindings: &[ResourceBinding],
        revoke: bool,
    ) -> AppResult<ResourcePlan> {
        let available = targets(record, clients)?;
        self.preview_resolved(record, owner, revision, available, bindings, revoke)
    }
    fn preview_resolved(
        &mut self,
        record: &SharedResource,
        owner: &str,
        revision: String,
        available: Vec<Target>,
        bindings: &[ResourceBinding],
        revoke: bool,
    ) -> AppResult<ResourcePlan> {
        validate_resource(record)?;
        self.plans.retain(|_, plan| Instant::now() < plan.expires);
        if self.plans.len() >= 16 {
            return Err(AppError::Busy);
        }
        let created_at = Utc::now();
        let mut plan = ResourcePlan {
            id: Uuid::new_v4().to_string(),
            resource_id: record.id.clone(),
            resource_name: record.name.clone(),
            revoke_and_delete: revoke,
            created_at,
            expires_at: created_at + chrono::Duration::minutes(15),
            operations: Vec::new(),
        };
        let mut writes = Vec::new();
        let mut bindings_after = bindings.to_vec();
        for target in &available {
            let bound = bindings
                .iter()
                .any(|b| b.resource_id == record.id && b.target_id == target.id);
            if revoke && !bound {
                continue;
            }
            if revoke && !target.supported {
                return Err(AppError::Unsupported("An owned resource target is unavailable. Rediscover before revocation; no ownership was discarded".into()));
            }
            let mut operation = ResourceOperation {
                target_id: target.id.clone(),
                title: target.label.clone(),
                supported: target.supported,
                changed: false,
                detail: target.detail.clone(),
                change: ResourceChange::Manual,
                destination: None,
                content_preview: None,
            };
            if target.supported {
                let (write, binding) = prepare(record, owner, target, bindings, revoke)?;
                operation.changed = write.changed();
                describe_projection(&mut operation, &write, target, owner, record, revoke)?;
                bindings_after
                    .retain(|b| !(b.resource_id == record.id && b.target_id == target.id));
                if let Some(binding) = binding {
                    bindings_after.push(binding);
                }
                writes.push(write);
            }
            plan.operations.push(operation);
        }
        if !revoke && writes.is_empty() {
            return Err(AppError::Unsupported("No supported resource target was detected. The authored record remains local; no client data was written".into()));
        }
        if revoke && bindings_after.iter().any(|b| b.resource_id == record.id) {
            return Err(AppError::Config(
                "Unresolved resource bindings prevent deletion".into(),
            ));
        }
        let stored = StoredResourcePlan {
            plan: plan.clone(),
            writes: transaction::deduplicate(writes)?,
            bindings_after,
            state_revision: revision,
            targets_revision: fingerprint::json("resource-targets-v1", &available)?,
            expires: Instant::now() + Duration::from_secs(900),
        };
        self.plans.insert(plan.id.clone(), stored);
        Ok(plan)
    }
    pub fn consume(&mut self, id: &str) -> AppResult<StoredResourcePlan> {
        let value = self.plans.remove(id).ok_or(AppError::PlanUnavailable)?;
        if Instant::now() >= value.expires {
            return Err(AppError::PlanUnavailable);
        }
        Ok(value)
    }
}

impl StoredResourcePlan {
    pub fn validate(
        &self,
        revision: &str,
        record: &SharedResource,
        clients: &[ClientTarget],
    ) -> AppResult<()> {
        self.validate_resolved(revision, &targets(record, clients)?)
    }
    fn validate_resolved(&self, revision: &str, available: &[Target]) -> AppResult<()> {
        if Instant::now() >= self.expires
            || revision != self.state_revision
            || fingerprint::json("resource-targets-v1", &available)? != self.targets_revision
        {
            return Err(AppError::PlanChanged);
        }
        for write in &self.writes {
            if write.resource.read()? != write.before {
                return Err(AppError::PlanChanged);
            }
        }
        Ok(())
    }
}

pub fn recovery_resources(records: &[SharedResource]) -> AppResult<Vec<transaction::Resource>> {
    let (copilot, config) = roots()?;
    let mut result = Vec::new();
    for record in records {
        for target in targets_at(record, &[], &copilot, &config, false) {
            if let Some(path) = target.path {
                result.push(transaction::Resource::File(crate::safe_io::resource_path(
                    &path,
                )?));
            }
        }
    }
    Ok(result)
}
