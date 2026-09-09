//! Immutable native-held reviewed intent. No caller can select a plan by targets.
use super::{fingerprint_target, fingerprint_target_with_writes, prepare_writes};
use crate::domain::{ClientTarget, Connection, DeploymentPlan};
use crate::error::{AppError, AppResult};
use crate::{fingerprint, validation};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const PLAN_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_PENDING_PLANS: usize = 32;
pub const MAX_PLAN_TARGETS: usize = 64;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanContext {
    owner_id: String,
    state_revision: String,
    credential_revision: String,
}

impl PlanContext {
    pub fn new(owner_id: &str, state_revision: String, secret: Option<&str>) -> Self {
        Self {
            owner_id: owner_id.to_string(),
            state_revision,
            credential_revision: fingerprint::bytes(
                "deployment-credential-v1",
                secret.map(str::as_bytes),
            ),
        }
    }
}

#[derive(Clone)]
pub struct StoredPlan {
    pub plan: DeploymentPlan,
    pub writes: Vec<crate::transaction::PreparedWrite>,
    pub after_fingerprints: BTreeMap<String, String>,
    context: PlanContext,
    connection_revision: String,
    operation_digest: String,
    expires_at: Instant,
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
        context: PlanContext,
    ) -> AppResult<DeploymentPlan> {
        let now = Instant::now();
        self.plans.retain(|_, stored| now < stored.expires_at);
        if self.plans.len() >= MAX_PENDING_PLANS {
            return Err(AppError::InvalidInput(
                "Too many pending previews; wait for expiry or apply a reviewed plan".into(),
            ));
        }
        validation::validate_connection(connection)?;
        validation::validate_api_key(secret)?;
        validate_shape(&plan, connection, targets)?;
        let mut fingerprints = BTreeMap::new();
        for target_id in &plan.target_ids {
            let target = targets
                .iter()
                .find(|target| target.id == *target_id)
                .ok_or(AppError::PlanChanged)?;
            fingerprints.insert(target_id.clone(), fingerprint_target(target)?);
        }
        let writes = prepare_writes(connection, secret, &mut plan, targets)?;
        let mut after_fingerprints = BTreeMap::new();
        for target in targets
            .iter()
            .filter(|target| plan.target_ids.contains(&target.id))
        {
            after_fingerprints.insert(
                target.id.clone(),
                fingerprint_target_with_writes(target, Some(&writes))?,
            );
        }
        // IDs and timestamps are created here, never adopted from frontend data.
        plan.id = Uuid::new_v4().to_string();
        plan.created_at = chrono::Utc::now();
        let stored = StoredPlan {
            writes,
            after_fingerprints,
            connection_revision: fingerprint::json("deployment-connection-v1", connection)?,
            operation_digest: fingerprint::json("deployment-plan-v1", &plan)?,
            plan: plan.clone(),
            context,
            expires_at: now + PLAN_TTL,
            target_fingerprints: fingerprints,
        };
        self.plans.insert(plan.id.clone(), stored);
        Ok(plan)
    }

    pub fn consume(&mut self, plan_id: &str) -> AppResult<StoredPlan> {
        self.consume_at(plan_id, Instant::now())
    }

    fn consume_at(&mut self, plan_id: &str, now: Instant) -> AppResult<StoredPlan> {
        if plan_id.len() != 36 || Uuid::parse_str(plan_id).is_err() {
            return Err(AppError::PlanUnavailable);
        }
        // Remove before expiry validation, authorization checks or any side effect.
        let stored = self
            .plans
            .remove(plan_id)
            .ok_or(AppError::PlanUnavailable)?;
        if now >= stored.expires_at {
            return Err(AppError::PlanUnavailable);
        }
        Ok(stored)
    }
}

fn validate_shape(
    plan: &DeploymentPlan,
    connection: &Connection,
    targets: &[ClientTarget],
) -> AppResult<()> {
    if plan.connection_id != connection.id
        || plan.target_ids.is_empty()
        || plan.target_ids.len() > MAX_PLAN_TARGETS
        || plan.operations.len() != plan.target_ids.len()
    {
        return Err(AppError::PlanChanged);
    }
    let requested: HashSet<_> = plan.target_ids.iter().collect();
    let operations: HashSet<_> = plan
        .operations
        .iter()
        .map(|operation| &operation.target_id)
        .collect();
    if requested.len() != plan.target_ids.len() || requested != operations {
        return Err(AppError::PlanChanged);
    }
    for operation in &plan.operations {
        if !targets
            .iter()
            .any(|target| target.id == operation.target_id && target.kind == operation.target_kind)
        {
            return Err(AppError::PlanChanged);
        }
    }
    Ok(())
}

pub fn validate_plan(
    stored: &StoredPlan,
    connection: &Connection,
    targets: &[ClientTarget],
    context: &PlanContext,
) -> AppResult<()> {
    if Instant::now() >= stored.expires_at {
        return Err(AppError::PlanUnavailable);
    }
    if &stored.context != context
        || stored.connection_revision != fingerprint::json("deployment-connection-v1", connection)?
        || stored.operation_digest != fingerprint::json("deployment-plan-v1", &stored.plan)?
    {
        return Err(AppError::PlanChanged);
    }
    validate_shape(&stored.plan, connection, targets)?;
    for operation in &stored.plan.operations {
        let target = targets
            .iter()
            .find(|target| target.id == operation.target_id)
            .ok_or(AppError::PlanChanged)?;
        if stored.target_fingerprints.get(&target.id) != Some(&fingerprint_target(target)?) {
            return Err(AppError::PlanChanged);
        }
    }
    for write in &stored.writes {
        if write.resource.read()? != write.before {
            return Err(AppError::PlanChanged);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::tests::{connection, plan, target};

    fn context() -> PlanContext {
        PlanContext::new("test-owner", "state-1".into(), Some("fixture-secret"))
    }

    #[test]
    fn expiry_is_fifteen_minutes_and_consumes_the_plan_at_the_boundary() {
        assert_eq!(PLAN_TTL.as_secs(), 900);
        let root = tempfile::tempdir().unwrap();
        let connection = connection();
        let target = target(&root.path().join("config.json"));
        let mut store = PlanStore::default();
        let plan = store
            .insert(
                &connection,
                None,
                plan(&connection, &target),
                &[target],
                context(),
            )
            .unwrap();
        let expiry = store.plans[&plan.id].expires_at;
        assert!(matches!(
            store.consume_at(&plan.id, expiry),
            Err(AppError::PlanUnavailable)
        ));
        assert!(store.plans.is_empty());
    }

    #[test]
    fn connection_content_credentials_owner_state_and_operation_changes_are_stale() {
        let root = tempfile::tempdir().unwrap();
        let connection = connection();
        let targets = [target(&root.path().join("config.json"))];
        let mut store = PlanStore::default();
        let plan = store
            .insert(
                &connection,
                None,
                plan(&connection, &targets[0]),
                &targets,
                context(),
            )
            .unwrap();
        let stored = store.consume(&plan.id).unwrap();
        validate_plan(&stored, &connection, &targets, &context()).unwrap();
        for changed in [
            PlanContext::new("different-owner", "state-1".into(), Some("fixture-secret")),
            PlanContext::new("test-owner", "state-2".into(), Some("fixture-secret")),
            PlanContext::new("test-owner", "state-1".into(), Some("rotated-secret")),
            PlanContext::new("test-owner", "state-1".into(), None),
        ] {
            assert!(matches!(
                validate_plan(&stored, &connection, &targets, &changed),
                Err(AppError::PlanChanged)
            ));
        }
        let mut changed = connection.clone();
        changed.base_url = "https://changed.invalid/v1".into();
        // A forged timestamp equality must not bypass full connection validation.
        assert!(matches!(
            validate_plan(&stored, &changed, &targets, &context()),
            Err(AppError::PlanChanged)
        ));
        let mut tampered = stored.clone();
        tampered.plan.operations[0].supported = false;
        assert!(matches!(
            validate_plan(&tampered, &connection, &targets, &context()),
            Err(AppError::PlanChanged)
        ));
        assert!(store.consume(&plan.id).is_err());
    }

    #[test]
    fn pending_plans_are_bounded_and_instances_do_not_share_ids() {
        let root = tempfile::tempdir().unwrap();
        let connection = connection();
        let targets = [target(&root.path().join("config.json"))];
        let mut store = PlanStore::default();
        for _ in 0..MAX_PENDING_PLANS {
            store
                .insert(
                    &connection,
                    None,
                    plan(&connection, &targets[0]),
                    &targets,
                    context(),
                )
                .unwrap();
        }
        assert!(store
            .insert(
                &connection,
                None,
                plan(&connection, &targets[0]),
                &targets,
                context()
            )
            .is_err());
        let id = store.plans.keys().next().unwrap().clone();
        assert!(PlanStore::default().consume(&id).is_err());
        assert!(store.consume(&id).is_ok());
        assert!(store.consume(&id).is_err());
    }
}
