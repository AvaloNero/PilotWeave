use crate::error::{AppError, AppResult};
use keyring::{Entry, Error as KeyringError};

const SERVICE: &str = "dev.pilotweave.connections";

fn entry(secret_ref: &str) -> AppResult<Entry> {
    Entry::new(SERVICE, secret_ref).map_err(|error| AppError::Secret(error.to_string()))
}

pub fn set(secret_ref: &str, value: &str) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::credential(secret_ref, Some(Some(value))).map(|_| ());
    }

    if value.contains(['\r', '\n']) {
        return Err(AppError::InvalidInput(
            "API keys must not contain newlines".to_string(),
        ));
    }
    entry(secret_ref)?
        .set_password(value)
        .map_err(|error| AppError::Secret(error.to_string()))
}

pub fn get(secret_ref: &str) -> AppResult<Option<String>> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::credential(secret_ref, None);
    }

    match entry(secret_ref)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(AppError::Secret(error.to_string())),
    }
}

pub fn observe(connection_id: &str) -> crate::domain::CredentialObservation {
    use crate::domain::{CredentialObservation, CredentialState};
    let state = match get(&format!("connection:{connection_id}")) {
        Ok(Some(_)) => CredentialState::Stored,
        Ok(None) => CredentialState::Missing,
        Err(_) => CredentialState::Unavailable,
    };
    CredentialObservation { state }
}

pub fn delete(secret_ref: &str) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::credential(secret_ref, Some(None)).map(|_| ());
    }

    match entry(secret_ref)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(AppError::Secret(error.to_string())),
    }
}
