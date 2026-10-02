use super::*;
use crate::domain::{DeploymentPurpose, PersistentState};
use crate::state::StateStore;
use tests::{connection, plan, target};
use uuid::Uuid;

fn seed(root: &Path) -> StateStore {
    let mut state = PersistentState::default();
    state.connections.push(connection());
    let path = root.join("state.json");
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    StateStore::open_at(path)
}

fn context(store: &StateStore) -> PlanContext {
    PlanContext::new(
        store.installation_owner_id(),
        store.revision().unwrap(),
        Some("fixture-secret"),
    )
    .with_ownership(store.ownership())
}

fn deploy(store: &mut StateStore, target: &ClientTarget) -> StoredPlan {
    let connection = connection();
    let mut plans = PlanStore::default();
    let preview = plans
        .insert(
            &connection,
            Some("fixture-secret"),
            plan(&connection, target),
            std::slice::from_ref(target),
            context(store),
        )
        .unwrap();
    let stored = plans.consume(&preview.id).unwrap();
    validate_plan(
        &stored,
        &connection,
        std::slice::from_ref(target),
        &context(store),
    )
    .unwrap();
    let mut tx = transaction::Transaction::begin(
        journal_path(store.path()),
        &preview.id,
        stored.writes.clone(),
    )
    .unwrap();
    tx.apply().unwrap();
    store
        .commit_deployment(Vec::new(), stored.ownership_after.clone(), None)
        .unwrap();
    tx.complete().unwrap();
    stored
}

#[test]
fn durable_owner_survives_reopen_and_secrets_are_not_in_proof() {
    let root = tempfile::tempdir().unwrap();
    let mut store = seed(root.path());
    let target = target(&root.path().join("models.json"));
    deploy(&mut store, &target);
    let reopened = StateStore::open_at(store.path().to_path_buf());
    assert_eq!(reopened.ownership(), store.ownership());
    assert_eq!(reopened.ownership().len(), 1);
    assert!(!String::from_utf8(fs::read(store.path()).unwrap())
        .unwrap()
        .contains("fixture-secret"));
    let text = fs::read_to_string(target.path.as_ref().unwrap()).unwrap();
    assert!(text.contains(store.installation_owner_id()));
    let mut plans = PlanStore::default();
    plans
        .insert(
            &connection(),
            Some("fixture-secret"),
            plan(&connection(), &target),
            &[target],
            context(&reopened),
        )
        .unwrap();
}

#[test]
fn forged_legacy_foreign_modified_and_copied_groups_never_gain_ownership() {
    let root = tempfile::tempdir().unwrap();
    let mut store = seed(root.path());
    let original = target(&root.path().join("models.json"));
    deploy(&mut store, &original);
    let original_bytes = fs::read(original.path.as_ref().unwrap()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    for mutation in ["legacy", "foreign", "modified"] {
        let mut hostile = value.clone();
        match mutation {
            "legacy" => {
                hostile[0]
                    .as_object_mut()
                    .unwrap()
                    .remove("pilotWeaveInstallationOwnerId");
            }
            "foreign" => {
                hostile[0]["pilotWeaveInstallationOwnerId"] =
                    serde_json::json!(Uuid::new_v4().to_string())
            }
            _ => hostile[0]["models"][0]["id"] = serde_json::json!("external/model"),
        }
        let bytes = serde_json::to_vec(&hostile).unwrap();
        fs::write(original.path.as_ref().unwrap(), &bytes).unwrap();
        assert!(PlanStore::default()
            .insert(
                &connection(),
                None,
                plan(&connection(), &original),
                std::slice::from_ref(&original),
                context(&store)
            )
            .is_err());
        assert_eq!(fs::read(original.path.as_ref().unwrap()).unwrap(), bytes);
    }
    fs::write(original.path.as_ref().unwrap(), &original_bytes).unwrap();
    assert!(PlanStore::default()
        .insert(
            &connection(),
            None,
            plan(&connection(), &original),
            std::slice::from_ref(&original),
            PlanContext::new(store.installation_owner_id(), "unproven".into(), None)
        )
        .is_err());
    let copied = target(&root.path().join("copied.json"));
    fs::write(copied.path.as_ref().unwrap(), &original_bytes).unwrap();
    assert!(PlanStore::default()
        .insert(
            &connection(),
            None,
            plan(&connection(), &copied),
            &[copied],
            context(&store)
        )
        .is_err());
    value[0]["name"] = serde_json::json!("just a changed display name");
    assert!(value.is_array());
}

#[test]
fn unrelated_groups_can_change_and_semantic_noop_preserves_json5_formatting() {
    let root = tempfile::tempdir().unwrap();
    let mut store = seed(root.path());
    let target = target(&root.path().join("models.json"));
    deploy(&mut store, &target);
    let group = fs::read_to_string(target.path.as_ref().unwrap()).unwrap();
    let groups: serde_json::Value = serde_json::from_str(&group).unwrap();
    let styled = format!(
        "[\n// unrelated user comment\n{{ name: 'user', vendor: 'other' }},\n{},\n]\n",
        groups[0]
    );
    fs::write(target.path.as_ref().unwrap(), &styled).unwrap();
    let mut plans = PlanStore::default();
    let preview = plans
        .insert(
            &connection(),
            Some("fixture-secret"),
            plan(&connection(), &target),
            std::slice::from_ref(&target),
            context(&store),
        )
        .unwrap();
    assert!(plans
        .consume(&preview.id)
        .unwrap()
        .writes
        .iter()
        .all(|write| !write.changed()));
    assert_eq!(
        fs::read_to_string(target.path.as_ref().unwrap()).unwrap(),
        styled
    );
}

#[test]
fn revoke_is_reviewed_one_shot_preserves_user_groups_and_deletes_state_after_commit() {
    let root = tempfile::tempdir().unwrap();
    let mut store = seed(root.path());
    let target = target(&root.path().join("models.json"));
    fs::write(
        target.path.as_ref().unwrap(),
        br#"[{"name":"user","vendor":"other"}]"#,
    )
    .unwrap();
    deploy(&mut store, &target);
    assert!(store
        .commit_deployment(Vec::new(), store.ownership().to_vec(), Some("one"))
        .is_err());
    let mut removal = plan(&connection(), &target);
    removal.purpose = DeploymentPurpose::RevokeAndDelete;
    let mut plans = PlanStore::default();
    let preview = plans
        .insert(
            &connection(),
            None,
            removal,
            std::slice::from_ref(&target),
            context(&store),
        )
        .unwrap();
    let stored = plans.consume(&preview.id).unwrap();
    assert!(plans.consume(&preview.id).is_err());
    assert!(stored.ownership_after.is_empty());
    let mut tx =
        transaction::Transaction::begin(journal_path(store.path()), &preview.id, stored.writes)
            .unwrap();
    tx.apply().unwrap();
    store
        .commit_deployment(Vec::new(), stored.ownership_after, Some("one"))
        .unwrap();
    tx.complete().unwrap();
    assert!(store.connections().is_empty());
    let groups: serde_json::Value =
        serde_json::from_slice(&fs::read(target.path.unwrap()).unwrap()).unwrap();
    assert_eq!(groups.as_array().unwrap().len(), 1);
    assert_eq!(groups[0]["name"], "user");
}

#[test]
fn revoke_stale_preview_or_failed_state_commit_never_discards_connection() {
    let root = tempfile::tempdir().unwrap();
    let mut store = seed(root.path());
    let target = target(&root.path().join("models.json"));
    deploy(&mut store, &target);
    let original = fs::read(target.path.as_ref().unwrap()).unwrap();
    let mut removal = plan(&connection(), &target);
    removal.purpose = DeploymentPurpose::RevokeAndDelete;
    let mut plans = PlanStore::default();
    let preview = plans
        .insert(
            &connection(),
            None,
            removal,
            std::slice::from_ref(&target),
            context(&store),
        )
        .unwrap();
    let stored = plans.consume(&preview.id).unwrap();
    fs::write(target.path.as_ref().unwrap(), b"[]").unwrap();
    assert!(validate_plan(
        &stored,
        &connection(),
        std::slice::from_ref(&target),
        &context(&store)
    )
    .is_err());
    assert_eq!(store.connections().len(), 1);
    fs::write(target.path.as_ref().unwrap(), &original).unwrap();
    let mut tx =
        transaction::Transaction::begin(journal_path(store.path()), &preview.id, stored.writes)
            .unwrap();
    tx.apply().unwrap();
    fs::write(store.path(), b"external-state").unwrap();
    assert!(store
        .commit_deployment(Vec::new(), stored.ownership_after, Some("one"))
        .is_err());
    tx.rollback().unwrap();
    assert_eq!(fs::read(target.path.as_ref().unwrap()).unwrap(), original);
    assert_eq!(store.connections().len(), 1);
    assert_eq!(store.ownership().len(), 1);
}
