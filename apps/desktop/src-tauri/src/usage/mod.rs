//! Local observations, official runtime snapshots and API-equivalent pricing.
//! No conversation content or client credentials cross this domain boundary.
pub mod commands;
pub mod importer;
pub mod parsers;
pub mod pricing;
pub mod queries;
pub mod runtime;
pub mod store;
pub mod types;

#[cfg(test)]
mod tests;

use crate::error::{AppError, AppResult};

pub fn invalid(detail: &str) -> AppError {
    AppError::InvalidInput(detail.into())
}

pub fn db_error(_: rusqlite::Error) -> AppError {
    AppError::Config("Usage storage operation failed; existing data was preserved".into())
}

pub fn bounded_id(value: &str) -> AppResult<String> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:@+".contains(&b))
    {
        return Err(invalid("Unsupported or oversized usage identifier"));
    }
    Ok(value.into())
}
