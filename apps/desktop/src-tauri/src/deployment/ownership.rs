//! Durable ownership proof. A marker alone is never authority to replace data.
//! Resource identities and projections are hashed; materialized keys stay out of state.
use crate::domain::ClientKind;
use crate::error::{AppError, AppResult};
use crate::{fingerprint, transaction};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_OWNERSHIP_RECORDS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetOwnership {
    pub owner_id: String,
    pub connection_id: String,
    pub target_id: String,
    pub target_kind: ClientKind,
    pub resource_id: String,
    pub projection_fingerprint: String,
    pub target_fingerprint: String,
    pub deployed_at: DateTime<Utc>,
}

pub fn resource_id(resource: &transaction::Resource) -> String {
    fingerprint::bytes("owned-resource-v1", Some(resource.key().as_bytes()))
}

pub fn conflict() -> AppError {
    AppError::Config("Configuration ownership is unproven, foreign, or externally changed. No target was written. Review the client configuration manually, or use Detach only to leave it untouched".into())
}

pub fn verify_projection(
    records: &[TargetOwnership],
    owner_id: &str,
    connection_id: &str,
    resource: &str,
    projection: &str,
) -> AppResult<()> {
    if records.iter().any(|record| {
        record.owner_id == owner_id
            && record.connection_id == connection_id
            && record.resource_id == resource
            && record.projection_fingerprint == projection
    }) {
        Ok(())
    } else {
        Err(conflict())
    }
}

pub fn cli_projection(writes: &[transaction::PreparedWrite], after: bool) -> AppResult<String> {
    let mut values = writes
        .iter()
        .map(|write| {
            let bytes = if after { &write.after } else { &write.before };
            (
                resource_id(&write.resource),
                fingerprint::bytes("owned-value-v1", bytes.as_deref()),
            )
        })
        .collect::<Vec<_>>();
    values.sort();
    fingerprint::json("cli-owned-projection-v1", &values)
}

pub fn cli_resource_id() -> String {
    // CLI has a single physical user-environment projection, even with multiple logical consumers.
    fingerprint::bytes("owned-resource-v1", Some(b"copilot-cli:user-environment"))
}

pub fn validate(
    records: &[TargetOwnership],
    owner: &str,
    connections: &HashSet<String>,
) -> AppResult<()> {
    if records.len() > MAX_OWNERSHIP_RECORDS {
        return Err(AppError::InvalidInput(
            "Too many configuration ownership records".into(),
        ));
    }
    let mut identities = HashSet::new();
    for record in records {
        if record.owner_id != owner
            || !connections.contains(&record.connection_id.to_ascii_lowercase())
            || record.target_id.is_empty()
            || record.target_id.len() > 512
            || record.target_id.chars().any(char::is_control)
            || [
                &record.resource_id,
                &record.projection_fingerprint,
                &record.target_fingerprint,
            ]
            .iter()
            .any(|value| value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
            || !identities.insert((&record.target_id, &record.connection_id))
            || record.target_kind == ClientKind::GithubCopilotApp
        {
            return Err(AppError::InvalidInput(
                "Invalid configuration ownership proof; read-only recovery is required".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_without_matching_native_proof_is_not_owned() {
        let record = TargetOwnership {
            owner_id: "owner".into(),
            connection_id: "connection".into(),
            target_id: "target".into(),
            target_kind: ClientKind::VsCodeCopilot,
            resource_id: "resource".into(),
            projection_fingerprint: "projection".into(),
            target_fingerprint: "target-hash".into(),
            deployed_at: Utc::now(),
        };
        assert!(verify_projection(&[], "owner", "connection", "resource", "projection").is_err());
        verify_projection(
            std::slice::from_ref(&record),
            "owner",
            "connection",
            "resource",
            "projection",
        )
        .unwrap();
        for (owner, connection, resource, projection) in [
            ("foreign", "connection", "resource", "projection"),
            ("owner", "other", "resource", "projection"),
            ("owner", "connection", "copied-resource", "projection"),
            ("owner", "connection", "resource", "modified"),
        ] {
            assert!(verify_projection(
                std::slice::from_ref(&record),
                owner,
                connection,
                resource,
                projection
            )
            .is_err());
        }
    }
}
