use crate::error::{AppError, AppResult};
use keyring::{Entry, Error as KeyringError};

const SERVICE: &str = "dev.pilotweave.connections";

fn entry(secret_ref: &str) -> AppResult<Entry> {
    Entry::new(SERVICE, secret_ref).map_err(|error| AppError::Secret(error.to_string()))
}

pub fn set(secret_ref: &str, value: &str) -> AppResult<()> {
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
    match entry(secret_ref)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(AppError::Secret(error.to_string())),
    }
}

pub fn exists(secret_ref: &str) -> bool {
    matches!(get(secret_ref), Ok(Some(_)))
}

pub fn delete(secret_ref: &str) -> AppResult<()> {
    match entry(secret_ref)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(AppError::Secret(error.to_string())),
    }
}
