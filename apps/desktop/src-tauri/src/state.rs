use crate::domain::{
    Connection, ConnectionInput, DeploymentRecord, PersistentState, STATE_VERSION,
};
use crate::error::{AppError, AppResult};
use crate::secrets;
use crate::validation;
use chrono::Utc;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DEPLOYMENT_RECORDS: usize = 200;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConnectionResult {
    pub deleted: bool,
    pub credential_cleanup_warning: Option<String>,
}

pub struct StateStore {
    path: PathBuf,
    state: PersistentState,
    loaded_bytes: Option<Vec<u8>>,
    /// When set, the primary state file could not be loaded and the store is
    /// in explicit read-only recovery: reads return validated last-known-good
    /// data when available and writes cannot overwrite the unreadable file.
    recovery: Option<String>,
}

impl StateStore {
    pub fn unavailable(error: AppError) -> Self {
        Self {
            path: PathBuf::new(),
            state: PersistentState::default(),
            loaded_bytes: None,
            recovery: Some(error.to_string()),
        }
    }
    pub fn open() -> AppResult<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| AppError::Config("Cannot resolve the user config directory".into()))?
            .join("PilotWeave");
        // Loading remains pure, including recovery. Dashboard observations report
        // credential availability separately, without changing persisted intent.
        Ok(Self::open_at(config_dir.join("state.json")))
    }

    pub fn open_at(path: PathBuf) -> Self {
        let primary = crate::safe_file::read_optional(&path, MAX_STATE_BYTES);
        let loaded_bytes = primary.as_ref().ok().cloned().flatten();
        match Self::try_load(&path, primary) {
            Ok((state, recovery)) => Self {
                path,
                state,
                loaded_bytes,
                recovery,
            },
            Err(error) => Self {
                path,
                state: PersistentState::default(),
                loaded_bytes,
                recovery: Some(error.to_string()),
            },
        }
    }

    fn try_load(
        path: &Path,
        primary: AppResult<Option<Vec<u8>>>,
    ) -> AppResult<(PersistentState, Option<String>)> {
        let primary_error = match primary.and_then(|bytes| parse_state(path, bytes.as_deref())) {
            Ok(Some(state)) => return Ok((state, None)),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        };
        match load_state(&last_good_path(path)) {
            Ok(Some(state)) => Ok((state, Some(format!(
                "The primary state is missing or invalid; showing validated last-known-good data in read-only recovery. {}",
                primary_error.as_deref().unwrap_or("The primary file is missing.")
            )))),
            Ok(None) if primary_error.is_none() => Ok((PersistentState::default(), None)),
            Ok(None) => Err(AppError::Config(primary_error.unwrap_or_default())),
            Err(error) => Err(AppError::Config(format!(
                "Primary and last-known-good state are unavailable; read-only recovery is required. Primary: {}. Backup: {error}",
                primary_error.as_deref().unwrap_or("missing")
            ))),
        }
    }

    /// Reason the store is in read-only recovery, if the primary file could
    /// not be loaded.
    pub fn recovery(&self) -> Option<&str> {
        self.recovery.as_deref()
    }

    pub(crate) fn ensure_writable(&self) -> AppResult<()> {
        if let Some(reason) = &self.recovery {
            return Err(AppError::Unsupported(format!(
                "PilotWeave state is in read-only recovery ({reason}); review recovery before resuming managed writes"
            )));
        }
        if crate::safe_file::read_optional(&self.path, MAX_STATE_BYTES)? != self.loaded_bytes {
            return Err(AppError::InvalidInput(
                "Application state changed outside PilotWeave; reopen and review before writing"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn installation_owner_id(&self) -> &str {
        &self.state.installation_owner_id
    }

    pub fn revision(&self) -> AppResult<String> {
        self.ensure_writable()?;
        crate::fingerprint::json("application-state-v1", &self.state)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connections(&self) -> &[Connection] {
        &self.state.connections
    }

    pub fn deployments(&self) -> &[DeploymentRecord] {
        &self.state.deployments
    }

    pub fn connection(&self, id: &str) -> AppResult<Connection> {
        self.state
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput(format!("Unknown connection: {id}")))
    }

    pub fn secret_for(&self, connection: &Connection) -> AppResult<Option<String>> {
        validation::validate_connection(connection)?;
        secrets::get(&format!("connection:{}", connection.id))
    }

    pub fn upsert_connection(&mut self, input: ConnectionInput) -> AppResult<Connection> {
        self.ensure_writable()?;
        validation::validate_api_key(input.api_key.as_deref())?;
        if input.clear_secret
            && input
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(AppError::InvalidInput(
                "Cannot set and clear a credential together".into(),
            ));
        }

        let now = Utc::now();
        let requested_id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let existing = requested_id.as_deref().and_then(|id| {
            self.state
                .connections
                .iter()
                .find(|connection| connection.id == id)
                .cloned()
        });
        if let Some(id) = requested_id.as_deref() {
            if existing.is_none() {
                return Err(AppError::InvalidInput(format!(
                    "Cannot update an unknown connection id: {id}"
                )));
            }
        }
        if existing.is_none() && self.state.connections.len() >= validation::MAX_CONNECTIONS {
            return Err(AppError::InvalidInput(format!(
                "PilotWeave supports at most {} connections",
                validation::MAX_CONNECTIONS
            )));
        }

        let id = requested_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let secret_ref = existing
            .as_ref()
            .map(|connection| connection.secret_ref.clone())
            .unwrap_or_else(|| format!("connection:{id}"));
        let created_at = existing
            .as_ref()
            .map(|connection| connection.created_at)
            .unwrap_or(now);

        let mut connection = Connection {
            id,
            name: input.name,
            base_url: input.base_url,
            provider_kind: input.provider_kind,
            protocol: input.protocol,
            headers: input.headers,
            models: input.models,
            secret_ref,
            has_secret: false,
            created_at,
            updated_at: now,
        };
        connection.normalize();
        validation::validate_connection(&connection)?;

        let mut next_state = self.state.clone();
        if let Some(index) = next_state
            .connections
            .iter()
            .position(|candidate| candidate.id == connection.id)
        {
            next_state.connections[index] = connection.clone();
        } else {
            next_state.connections.push(connection.clone());
        }
        next_state.connections.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        serialized_state(&next_state)?;

        let old_secret = secrets::get(&connection.secret_ref)?;
        let new_secret = input
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if input.clear_secret {
            secrets::delete(&connection.secret_ref)?;
        } else if let Some(value) = new_secret {
            secrets::set(&connection.secret_ref, value)?;
        }
        connection.has_secret =
            !input.clear_secret && new_secret.or(old_secret.as_deref()).is_some();
        if let Some(index) = next_state
            .connections
            .iter()
            .position(|candidate| candidate.id == connection.id)
        {
            next_state.connections[index].has_secret = connection.has_secret;
        }

        let old_state = std::mem::replace(&mut self.state, next_state);
        if let Err(error) = self.persist() {
            self.state = old_state;
            let rollback = match old_secret {
                Some(secret) => secrets::set(&connection.secret_ref, &secret),
                None => secrets::delete(&connection.secret_ref),
            };
            if let Err(rollback_error) = rollback {
                return Err(AppError::Config(format!(
                    "{error}; additionally failed to restore the previous credential: {rollback_error}"
                )));
            }
            return Err(error);
        }

        Ok(connection)
    }

    pub fn delete_connection(&mut self, id: &str) -> AppResult<DeleteConnectionResult> {
        self.ensure_writable()?;
        let index = self
            .state
            .connections
            .iter()
            .position(|connection| connection.id == id)
            .ok_or_else(|| AppError::InvalidInput(format!("Unknown connection: {id}")))?;
        let connection = self.state.connections[index].clone();
        let mut next_state = self.state.clone();

        next_state.connections.remove(index);
        next_state
            .deployments
            .retain(|record| record.connection_id != id);
        validation::validate_persistent_state(&next_state)?;

        let old_state = std::mem::replace(&mut self.state, next_state);
        if let Err(error) = self.persist() {
            self.state = old_state;
            return Err(error);
        }

        let credential_cleanup_warning = secrets::delete(&format!("connection:{}", connection.id))
            .err()
            .map(|_| {
                "Connection deletion completed, but its OS credential could not be removed"
                    .to_string()
            });
        Ok(DeleteConnectionResult {
            deleted: true,
            credential_cleanup_warning,
        })
    }

    pub fn record_deployments(&mut self, records: Vec<DeploymentRecord>) -> AppResult<()> {
        self.ensure_writable()?;
        let mut next_state = self.state.clone();
        next_state.deployments.extend(records);
        next_state
            .deployments
            .sort_by_key(|record| std::cmp::Reverse(record.created_at));
        next_state.deployments.truncate(MAX_DEPLOYMENT_RECORDS);
        validation::validate_persistent_state(&next_state)?;

        let old_state = std::mem::replace(&mut self.state, next_state);
        if let Err(error) = self.persist() {
            self.state = old_state;
            return Err(error);
        }
        Ok(())
    }

    fn persist(&mut self) -> AppResult<()> {
        let bytes = write_state_checked(&self.path, &self.state, self.loaded_bytes.as_deref())?;
        self.loaded_bytes = Some(bytes);
        Ok(())
    }
}

fn last_good_path(path: &Path) -> PathBuf {
    path.with_file_name("state.json.last-good")
}

fn load_state(path: &Path) -> AppResult<Option<PersistentState>> {
    let bytes = crate::safe_file::read_optional(path, MAX_STATE_BYTES)?;
    parse_state(path, bytes.as_deref())
}

fn parse_state(path: &Path, bytes: Option<&[u8]>) -> AppResult<Option<PersistentState>> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let mut state: PersistentState =
        serde_json::from_slice(bytes).map_err(|error| AppError::json(path, error))?;
    if state.version > STATE_VERSION {
        return Err(AppError::Config(format!(
            "State version {} is newer than this PilotWeave build supports ({STATE_VERSION})",
            state.version
        )));
    }
    validation::validate_persisted_identities(&state)?;
    state.version = STATE_VERSION;
    for connection in &mut state.connections {
        connection.normalize();
        validation::validate_connection(connection)?;
    }
    validation::validate_persistent_state(&state)?;
    Ok(Some(state))
}

fn serialized_state(state: &PersistentState) -> AppResult<Vec<u8>> {
    validation::validate_persistent_state(state)?;
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| AppError::Config(format!("Failed to serialize state: {error}")))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(AppError::InvalidInput(
            "Complete state exceeds the 8 MiB reload limit".into(),
        ));
    }
    Ok(bytes)
}

fn write_state_checked(
    path: &Path,
    state: &PersistentState,
    expected: Option<&[u8]>,
) -> AppResult<Vec<u8>> {
    validation::validate_persistent_state(state)?;
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| AppError::Config(format!("Failed to serialize state: {error}")))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(AppError::InvalidInput(
            "State exceeds its storage limit".into(),
        ));
    }
    // Preserve the validated previous revision before touching the primary.
    // Backup failure therefore cannot be reported after a successful commit.
    if crate::safe_file::read_optional(path, MAX_STATE_BYTES)?.as_deref() != expected {
        return Err(AppError::InvalidInput("State changed before commit".into()));
    }
    let mut previous = parse_state(path, expected)?.unwrap_or_else(|| PersistentState {
        installation_owner_id: state.installation_owner_id.clone(),
        ..PersistentState::default()
    });
    if let Some(expected) = expected {
        let document: serde_json::Value =
            serde_json::from_slice(expected).map_err(|error| AppError::json(path, error))?;
        if document.get("installationOwnerId").is_none() {
            previous.installation_owner_id = state.installation_owner_id.clone();
        }
    }
    let previous = serde_json::to_vec_pretty(&previous).map_err(|error| {
        AppError::Config(format!(
            "Failed to serialize last-known-good state: {error}"
        ))
    })?;
    crate::safe_file::atomic_write_private(&last_good_path(path), &previous)?;
    crate::safe_file::atomic_write_private_if(path, &bytes, expected)?;
    Ok(bytes)
}

#[cfg(test)]
fn write_state(path: &Path, state: &PersistentState) -> AppResult<()> {
    let before = crate::safe_file::read_optional(path, MAX_STATE_BYTES)?;
    write_state_checked(path, state, before.as_deref()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ModelSpec;

    #[test]
    fn full_state_reload_limit_is_checked_before_any_write() {
        let mut state = PersistentState::default();
        for n in 0..40 {
            state.connections.push(Connection {
                id: format!("connection-{n}"),
                name: format!("Connection {n}"),
                base_url: "https://example.invalid/v1".into(),
                provider_kind: crate::domain::ProviderKind::Openai,
                protocol: crate::domain::ApiProtocol::ChatCompletions,
                headers: (0..32)
                    .map(|h| (format!("X-Test-{h}"), "x".repeat(8192)))
                    .collect(),
                models: vec![ModelSpec {
                    id: "m".into(),
                    model_id: "m".into(),
                    name: "M".into(),
                    enabled: true,
                    capabilities: Default::default(),
                }],
                secret_ref: format!("connection:connection-{n}"),
                has_secret: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });
        }
        assert!(validation::validate_persistent_state(&state).is_ok());
        assert!(serialized_state(&state).is_err());
    }

    #[test]
    fn external_state_changes_and_deletions_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        write_state(&path, &fixture_state("Original")).unwrap();
        let mut store = StateStore::open_at(path.clone());
        let mut external = fixture_state("External");
        external.installation_owner_id = store.installation_owner_id().to_string();
        let bytes = serde_json::to_vec_pretty(&external).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(store.ensure_writable().is_err());
        assert!(store.record_deployments(Vec::new()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(&path).unwrap();
        assert!(store.record_deployments(Vec::new()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn legacy_owner_identity_is_persisted_once_and_survives_reopen_and_recovery() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        let mut legacy = serde_json::to_value(fixture_state("Legacy")).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("installationOwnerId");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut store = StateStore::open_at(path.clone());
        let owner = store.installation_owner_id().to_string();
        store.record_deployments(Vec::new()).unwrap();
        assert_eq!(
            StateStore::open_at(path.clone()).installation_owner_id(),
            owner
        );
        fs::write(&path, b"broken").unwrap();
        let recovered = StateStore::open_at(path);
        assert_eq!(recovered.installation_owner_id(), owner);
        assert!(recovered.ensure_writable().is_err());
    }

    #[test]
    fn missing_config_directory_opens_read_only_instead_of_panicking() {
        let store = StateStore::unavailable(AppError::Config("No config directory".into()));
        assert!(store.ensure_writable().is_err());
        assert!(store.connections().is_empty());
    }

    fn fixture_state(name: &str) -> PersistentState {
        let now = Utc::now();
        PersistentState {
            connections: vec![Connection {
                id: "fixture".into(),
                name: name.into(),
                base_url: "https://example.invalid/v1".into(),
                provider_kind: crate::domain::ProviderKind::Openai,
                protocol: crate::domain::ApiProtocol::ChatCompletions,
                headers: Default::default(),
                models: vec![ModelSpec {
                    id: "fixture-model".into(),
                    model_id: "fixture-model".into(),
                    name: "Fixture".into(),
                    enabled: true,
                    capabilities: Default::default(),
                }],
                secret_ref: "connection:fixture".into(),
                has_secret: false,
                created_at: now,
                updated_at: now,
            }],
            ..PersistentState::default()
        }
    }

    #[test]
    fn corrupt_primary_shows_validated_previous_revision_without_writes_or_keyring_reads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        write_state(&path, &fixture_state("Previous")).unwrap();
        write_state(&path, &fixture_state("Current")).unwrap();
        assert_eq!(
            load_state(&path).unwrap().unwrap().connections[0].name,
            "Current"
        );
        fs::write(&path, b"corrupt").unwrap();
        let mut store = StateStore::open_at(path.clone());
        assert_eq!(store.connections()[0].name, "Previous");
        assert!(store.recovery().unwrap().contains("last-known-good"));
        assert!(store.record_deployments(Vec::new()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"corrupt");
    }

    #[test]
    fn missing_primary_with_backup_does_not_open_empty_writable_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(
            last_good_path(&path),
            serde_json::to_vec(&fixture_state("Recovered")).unwrap(),
        )
        .unwrap();
        let store = StateStore::open_at(path.clone());
        assert_eq!(store.connections()[0].name, "Recovered");
        assert!(store.recovery().is_some());
        assert!(!path.exists());
    }

    #[test]
    fn invalid_backup_is_never_used_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"corrupt primary").unwrap();
        let mut invalid = fixture_state("Invalid");
        invalid.connections[0].base_url = "http://remote.example/v1".into();
        let bytes = serde_json::to_vec(&invalid).unwrap();
        fs::write(last_good_path(&path), &bytes).unwrap();
        let store = StateStore::open_at(path.clone());
        assert!(store.connections().is_empty());
        assert!(store.recovery().unwrap().contains("loopback"));
        assert_eq!(fs::read(last_good_path(&path)).unwrap(), bytes);
        assert_eq!(fs::read(path).unwrap(), b"corrupt primary");
    }

    #[test]
    fn backup_failure_leaves_primary_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        let before = serde_json::to_vec(&fixture_state("Original")).unwrap();
        fs::write(&path, &before).unwrap();
        fs::create_dir(last_good_path(&path)).unwrap();
        assert!(write_state(&path, &fixture_state("New")).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn opens_missing_state_as_empty() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = StateStore::open_at(directory.path().join("state.json"));
        assert!(store.connections().is_empty());
        assert_eq!(store.deployments().len(), 0);
        assert_eq!(store.recovery(), None);
    }

    #[test]
    fn corrupt_state_enters_read_only_recovery_without_overwriting() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("state.json");
        fs::write(&path, b"{ not valid json").expect("write corrupt state");

        let mut store = StateStore::open_at(path.clone());
        assert!(store.recovery().is_some());
        assert!(store.connections().is_empty());

        let input = ConnectionInput {
            id: None,
            name: "Blocked".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: crate::domain::ProviderKind::Openai,
            protocol: crate::domain::ApiProtocol::ChatCompletions,
            headers: Default::default(),
            models: vec![ModelSpec {
                id: "m".to_string(),
                model_id: "m".to_string(),
                name: "m".to_string(),
                enabled: true,
                capabilities: Default::default(),
            }],
            api_key: None,
            clear_secret: false,
        };
        let error = store
            .upsert_connection(input)
            .expect_err("writes must be rejected in recovery");
        assert!(matches!(error, AppError::Unsupported(_)));
        assert!(store.delete_connection("anything").is_err());
        assert!(store.record_deployments(Vec::new()).is_err());

        // The unreadable primary file is never overwritten by recovery mode.
        let bytes = fs::read(&path).expect("state file");
        assert_eq!(bytes, b"{ not valid json");
    }

    #[test]
    fn newer_state_version_enters_read_only_recovery() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("state.json");
        fs::write(
            &path,
            format!(
                r#"{{"version":{},"connections":[],"deployments":[]}}"#,
                STATE_VERSION + 1
            ),
        )
        .expect("write newer state");

        let store = StateStore::open_at(path);
        assert!(store.recovery().expect("recovery reason").contains("newer"));
    }

    #[test]
    fn semantically_invalid_state_enters_recovery() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("state.json");
        fs::write(
            &path,
            r#"{
              "version": 1,
              "connections": [{
                "id": "one",
                "name": "One",
                "baseUrl": "http://example.com/v1",
                "providerKind": "openai",
                "protocol": "chat-completions",
                "headers": {},
                "models": [{
                  "id": "model",
                  "modelId": "vendor/model",
                  "name": "Model",
                  "enabled": true,
                  "capabilities": {}
                }],
                "secretRef": "connection:one",
                "hasSecret": false,
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-01T00:00:00Z"
              }],
              "deployments": []
            }"#,
        )
        .expect("write state");

        let store = StateStore::open_at(path);
        assert!(store.recovery().expect("recovery").contains("loopback"));
    }

    #[test]
    fn update_cannot_inject_an_unknown_connection_id() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut store = StateStore::open_at(directory.path().join("state.json"));
        let input = ConnectionInput {
            id: Some("attacker-controlled".to_string()),
            name: "Blocked".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: crate::domain::ProviderKind::Openai,
            protocol: crate::domain::ApiProtocol::ChatCompletions,
            headers: Default::default(),
            models: vec![ModelSpec {
                id: "m".to_string(),
                model_id: "m".to_string(),
                name: "m".to_string(),
                enabled: true,
                capabilities: Default::default(),
            }],
            api_key: None,
            clear_secret: false,
        };
        let error = store
            .upsert_connection(input)
            .expect_err("unknown update id must fail before keyring access");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }
    #[test]
    fn corrupt_primary_uses_last_good_read_only() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("state.json");
        let bytes = serialized_state(&PersistentState::default()).expect("serialize");
        fs::write(crate::safe_io::sibling(&path, ".last-good"), bytes).expect("backup");
        fs::write(&path, b"invalid").expect("corrupt");
        let mut store = StateStore::open_at(path.clone());
        assert!(store
            .recovery()
            .expect("recovery")
            .contains("last-known-good"));
        assert!(store.record_deployments(Vec::new()).is_err());
        assert_eq!(fs::read(&path).expect("read"), b"invalid");
    }

    #[test]
    fn external_state_edit_is_not_overwritten() {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("state.json");
        let mut store = StateStore::open_at(path.clone());
        fs::write(&path, b"external").expect("external");
        assert!(store.record_deployments(Vec::new()).is_err());
        assert_eq!(fs::read(&path).expect("read"), b"external");
    }
}
