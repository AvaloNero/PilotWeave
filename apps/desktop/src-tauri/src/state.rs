use crate::domain::{
    Connection, ConnectionInput, DeploymentRecord, PersistentState, STATE_VERSION,
};
use crate::error::{AppError, AppResult};
use crate::secrets;
use chrono::Utc;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DEPLOYMENT_RECORDS: usize = 200;

pub struct StateStore {
    path: PathBuf,
    state: PersistentState,
}

impl StateStore {
    pub fn open() -> AppResult<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| AppError::Config("Cannot resolve the user config directory".into()))?
            .join("PilotWeave");
        Self::open_at(config_dir.join("state.json"))
    }

    pub fn open_at(path: PathBuf) -> AppResult<Self> {
        let mut state = load_state(&path)?;
        if state.version > STATE_VERSION {
            return Err(AppError::Config(format!(
                "State version {} is newer than this PilotWeave build supports ({STATE_VERSION})",
                state.version
            )));
        }
        state.version = STATE_VERSION;
        for connection in &mut state.connections {
            connection.normalize();
            connection.has_secret = secrets::exists(&connection.secret_ref);
        }
        Ok(Self { path, state })
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
        secrets::get(&connection.secret_ref)
    }

    pub fn upsert_connection(&mut self, input: ConnectionInput) -> AppResult<Connection> {
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
        connection.validate()?;

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
        connection.has_secret = secrets::exists(&connection.secret_ref);

        let old_state = self.state.clone();
        if let Some(index) = self
            .state
            .connections
            .iter()
            .position(|candidate| candidate.id == connection.id)
        {
            self.state.connections[index] = connection.clone();
        } else {
            self.state.connections.push(connection.clone());
        }
        self.state.connections.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });

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

    pub fn delete_connection(&mut self, id: &str) -> AppResult<()> {
        let index = self
            .state
            .connections
            .iter()
            .position(|connection| connection.id == id)
            .ok_or_else(|| AppError::InvalidInput(format!("Unknown connection: {id}")))?;
        let connection = self.state.connections[index].clone();
        let old_state = self.state.clone();

        self.state.connections.remove(index);
        self.state
            .deployments
            .retain(|record| record.connection_id != id);
        if let Err(error) = self.persist() {
            self.state = old_state;
            return Err(error);
        }

        secrets::delete(&connection.secret_ref)
    }

    pub fn record_deployments(&mut self, records: Vec<DeploymentRecord>) -> AppResult<()> {
        self.state.deployments.extend(records);
        self.state
            .deployments
            .sort_by(|left, right| right.created_at.cmp(&left.created_at));
        self.state.deployments.truncate(MAX_DEPLOYMENT_RECORDS);
        self.persist()
    }

    fn persist(&self) -> AppResult<()> {
        write_state(&self.path, &self.state)
    }
}

fn load_state(path: &Path) -> AppResult<PersistentState> {
    if !path.exists() {
        return Ok(PersistentState::default());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::InvalidInput(format!(
            "PilotWeave state must be a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_STATE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "PilotWeave state exceeds {} MiB",
            MAX_STATE_BYTES / 1024 / 1024
        )));
    }
    let bytes = fs::read(path).map_err(|error| AppError::io(path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| AppError::json(path, error))
}

fn write_state(path: &Path, state: &PersistentState) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("State path has no parent directory".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| AppError::Config(format!("Failed to serialize state: {error}")))?;
    let temp_path = parent.join(format!(".state-{}.tmp", Uuid::new_v4()));

    let result = (|| -> AppResult<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        let mut file = options
            .open(&temp_path)
            .map_err(|error| AppError::io(&temp_path, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| AppError::io(&temp_path, error))?;
        }
        file.write_all(&bytes)
            .map_err(|error| AppError::io(&temp_path, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp_path, error))?;

        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
        }
        fs::rename(&temp_path, path).map_err(|error| AppError::io(path, error))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_missing_state_as_empty() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store =
            StateStore::open_at(directory.path().join("state.json")).expect("open empty store");
        assert!(store.connections().is_empty());
        assert_eq!(store.deployments().len(), 0);
    }
}
