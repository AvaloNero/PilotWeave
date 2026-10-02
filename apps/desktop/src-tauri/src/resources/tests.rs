use super::*;

fn record(kind: ResourceKind) -> SharedResource {
    SharedResource {
        id: Uuid::new_v4().to_string(),
        name: "Shared guide".into(),
        kind,
        body: if kind == ResourceKind::Mcp {
            "https://mcp.example.invalid/mcp".into()
        } else {
            "Use small reviewable changes.".into()
        },
        created_at: Utc::now(),
        updated_at: Utc::now(),
        archived: false,
    }
}
fn target(path: PathBuf, mcp: bool) -> Target {
    Target {
        id: "copilot-cli:user-resources".into(),
        label: "Fixture target".into(),
        path: Some(path),
        mcp_key: mcp.then_some("mcpServers"),
        supported: true,
        detail: "Public fixture adapter v1".into(),
    }
}
fn apply(root: &std::path::Path, stored: &StoredResourcePlan) {
    let mut tx = transaction::Transaction::begin(
        root.join("journal.json"),
        &stored.plan.id,
        stored.writes.clone(),
    )
    .unwrap();
    tx.apply().unwrap();
    tx.complete().unwrap();
}
#[test]
fn mcp_secret_command_and_path_payloads_are_rejected_without_echoing_values() {
    for value in [
        "http://mcp.invalid",
        "https://user:private-key@mcp.invalid",
        "https://mcp.invalid/?token=private-key",
        "https://mcp.invalid/#private-key",
        "file:///private-key",
        "https://mcp.invalid/${apiKey}",
    ] {
        let mut record = record(ResourceKind::Mcp);
        record.body = value.into();
        let error = validate_resource(&record).unwrap_err().to_string();
        assert!(!error.contains("private-key"));
    }
    let input = serde_json::json!({"id":null,"name":"mcp","kind":"mcp","body":"https://mcp.invalid", "command":"not-allowed"});
    assert!(serde_json::from_value::<ResourceInput>(input).is_err());
    let mut oversized = record(ResourceKind::Instructions);
    oversized.body = "x".repeat(MAX_BODY_BYTES + 1);
    assert!(validate_resource(&oversized).is_err());
}
#[test]
fn mcp_preserves_unmanaged_servers_unknown_fields_and_semantic_noop_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mcp.json");
    let bytes = b"{ // comment\n mcpServers: { user: { type: 'http', url: 'https://user.invalid', headers: { authorization: 'private-fixture' } } }, unknown: 17 }";
    std::fs::write(&path, bytes).unwrap();
    let record = record(ResourceKind::Mcp);
    let target = target(path.clone(), true);
    let (write, binding) = prepare(&record, "owner", &target, &[], false).unwrap();
    let value = parse_config(write.after.as_deref()).unwrap();
    assert_eq!(value["unknown"], 17);
    assert_eq!(
        value["mcpServers"]["user"]["headers"]["authorization"],
        "private-fixture"
    );
    let rendered = format!("// keep formatting\n{}\n", value);
    std::fs::write(&path, &rendered).unwrap();
    let (noop, _) = prepare(&record, "owner", &target, &[binding.unwrap()], false).unwrap();
    assert!(!noop.changed());
    assert_eq!(noop.after.unwrap(), rendered.as_bytes());
}
#[test]
fn reviewed_projection_has_exact_change_and_never_exports_foreign_mcp_credentials() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mcp.json");
    std::fs::write(&path, br#"{"mcpServers":{"foreign":{"url":"https://foreign.invalid","headers":{"authorization":"PRIVATE_FOREIGN_FIXTURE"}}},"unknown":"PRIVATE_UNRELATED_FIXTURE"}"#).unwrap();
    let record = record(ResourceKind::Mcp);
    let target = target(path, true);
    let mut plans = ResourcePlans::default();
    let create = plans
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![target.clone()],
            &[],
            false,
        )
        .unwrap();
    assert_eq!(create.operations[0].change, ResourceChange::Create);
    let preview = create.operations[0].content_preview.as_ref().unwrap();
    assert!(preview.contains(&record.body));
    assert!(preview.contains(&key("owner", &record)));
    assert!(!serde_json::to_string(&create).unwrap().contains("PRIVATE_"));
    let stored = plans.consume(&create.id).unwrap();
    apply(root.path(), &stored);
    let noop = plans
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![target.clone()],
            &stored.bindings_after,
            false,
        )
        .unwrap();
    assert_eq!(noop.operations[0].change, ResourceChange::Unchanged);
    assert!(!serde_json::to_string(&noop).unwrap().contains("PRIVATE_"));
    let mut edited = record.clone();
    edited.body = "https://updated.example.invalid/mcp".into();
    let update = plans
        .preview_resolved(
            &edited,
            "owner",
            "state".into(),
            vec![target.clone()],
            &stored.bindings_after,
            false,
        )
        .unwrap();
    assert_eq!(update.operations[0].change, ResourceChange::Update);
    assert!(update.operations[0]
        .content_preview
        .as_ref()
        .unwrap()
        .contains(&edited.body));
    let remove = plans
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![target],
            &stored.bindings_after,
            true,
        )
        .unwrap();
    assert_eq!(remove.operations[0].change, ResourceChange::Remove);
    assert!(remove.operations[0].content_preview.is_none());
    assert!(!serde_json::to_string(&remove).unwrap().contains("PRIVATE_"));
}
#[test]
fn foreign_files_or_forged_markers_never_become_owned() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("guide.instructions.md");
    let record = record(ResourceKind::Instructions);
    let target = target(path.clone(), false);
    std::fs::write(&path, markdown("owner", &record).unwrap()).unwrap();
    assert!(prepare(&record, "owner", &target, &[], false).is_err());
    std::fs::remove_file(&path).unwrap();
    let (write, binding) = prepare(&record, "owner", &target, &[], false).unwrap();
    std::fs::write(&path, write.after.unwrap()).unwrap();
    let binding = binding.unwrap();
    assert!(prepare(
        &record,
        "foreign",
        &target,
        std::slice::from_ref(&binding),
        false
    )
    .is_err());
    std::fs::write(&path, b"external instructions").unwrap();
    assert!(prepare(&record, "owner", &target, &[binding], true).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"external instructions");
}
#[test]
fn resource_plans_are_bound_expiring_one_shot_and_external_changes_are_stale() {
    let root = tempfile::tempdir().unwrap();
    let record = record(ResourceKind::Skill);
    let target = target(root.path().join("skill/SKILL.md"), false);
    let mut plans = ResourcePlans::default();
    let plan = plans
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![target.clone()],
            &[],
            false,
        )
        .unwrap();
    assert_eq!(plan.operations[0].change, ResourceChange::Create);
    assert!(plan.operations[0]
        .content_preview
        .as_ref()
        .unwrap()
        .starts_with("---\nname: pilotweave-"));
    let stored = plans.consume(&plan.id).unwrap();
    assert!(plans.consume(&plan.id).is_err());
    stored
        .validate_resolved("state", std::slice::from_ref(&target))
        .unwrap();
    assert!(stored
        .validate_resolved("changed-state", std::slice::from_ref(&target))
        .is_err());
    apply(root.path(), &stored);
    assert!(stored.validate_resolved("state", &[target]).is_err());
    let mut expired = stored;
    expired.expires = Instant::now();
    plans.plans.insert(plan.id.clone(), expired);
    assert!(plans.consume(&plan.id).is_err());
    assert!(plans.consume(&plan.id).is_err());
}
#[test]
fn all_targets_are_prepared_before_writes_and_manual_is_not_synced() {
    let root = tempfile::tempdir().unwrap();
    let record = record(ResourceKind::Instructions);
    let first = target(root.path().join("first.md"), false);
    let mut second = target(root.path().join("second.md"), false);
    second.id = "vscode:stable:default".into();
    std::fs::write(second.path.as_ref().unwrap(), b"unmanaged").unwrap();
    assert!(ResourcePlans::default()
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![first.clone(), second],
            &[],
            false
        )
        .is_err());
    assert!(!first.path.as_ref().unwrap().exists());
    let mut manual = target(root.path().join("manual.md"), false);
    manual.id = "github-copilot-app:manual".into();
    manual.supported = false;
    let plan = ResourcePlans::default()
        .preview_resolved(
            &record,
            "owner",
            "state".into(),
            vec![first, manual],
            &[],
            false,
        )
        .unwrap();
    assert_eq!(plan.operations.iter().filter(|op| op.supported).count(), 1);
    assert!(!plan.operations[1].supported);
}
#[test]
fn resource_revoke_preserves_other_entries_and_history_metadata_reopens_without_content() {
    let root = tempfile::tempdir().unwrap();
    let state_path = root.path().join("state.json");
    let mut store = crate::state::StateStore::open_at(state_path.clone());
    let record = store
        .upsert_resource(ResourceInput {
            id: None,
            name: "MCP".into(),
            kind: ResourceKind::Mcp,
            body: "https://mcp.example.invalid/mcp".into(),
        })
        .unwrap();
    let path = root.path().join("mcp.json");
    std::fs::write(
        &path,
        br#"{"mcpServers":{"user":{"type":"http","url":"https://user.invalid"}}}"#,
    )
    .unwrap();
    let target = target(path.clone(), true);
    let mut plans = ResourcePlans::default();
    let preview = plans
        .preview_resolved(
            &record,
            store.installation_owner_id(),
            store.revision().unwrap(),
            vec![target.clone()],
            &[],
            false,
        )
        .unwrap();
    let stored = plans.consume(&preview.id).unwrap();
    apply(root.path(), &stored);
    store
        .commit_resource(&record.id, stored.bindings_after, false)
        .unwrap();
    let mut reopened = crate::state::StateStore::open_at(state_path);
    assert_eq!(reopened.resource_bindings().len(), 1);
    let preview = plans
        .preview_resolved(
            &record,
            reopened.installation_owner_id(),
            reopened.revision().unwrap(),
            vec![target],
            reopened.resource_bindings(),
            true,
        )
        .unwrap();
    let stored = plans.consume(&preview.id).unwrap();
    apply(root.path(), &stored);
    reopened
        .commit_resource(&record.id, stored.bindings_after, true)
        .unwrap();
    assert!(reopened.resource_bindings().is_empty());
    assert!(reopened.resources()[0].archived);
    assert!(reopened.resources()[0].body.is_empty());
    assert!(reopened.resource(&record.id).is_err());
    let value = parse_config(Some(&std::fs::read(path).unwrap())).unwrap();
    assert_eq!(value["mcpServers"].as_object().unwrap().len(), 1);
    assert!(value["mcpServers"].get("user").is_some());
}
#[test]
fn shared_skill_is_one_physical_projection_and_custom_home_remains_manual() {
    let root = tempfile::tempdir().unwrap();
    let record = record(ResourceKind::Skill);
    let client = ClientTarget {
        id: "vscode:stable:default".into(),
        kind: crate::domain::ClientKind::VsCodeCopilot,
        name: "Fixture".into(),
        detail: "Fixture".into(),
        path: Some(root.path().join("models.json").to_string_lossy().into()),
        detected: true,
        supports_write: true,
        status: crate::domain::ClientStatus::Available,
        diagnostic: None,
    };
    let targets = targets_at(
        &record,
        std::slice::from_ref(&client),
        root.path(),
        root.path(),
        false,
    );
    assert_eq!(targets.iter().filter(|t| t.supported).count(), 1);
    assert_eq!(targets.iter().filter(|t| t.path.is_some()).count(), 1);
    assert!(
        targets_at(&record, &[client], root.path(), root.path(), true)
            .iter()
            .all(|t| !t.supported)
    );
    let rendered = String::from_utf8(markdown("owner", &record).unwrap()).unwrap();
    assert!(rendered.starts_with("---\nname: pilotweave-"));
}
