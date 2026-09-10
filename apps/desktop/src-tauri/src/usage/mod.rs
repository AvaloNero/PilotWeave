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

/// OpenRouter latest aliases have one leading tilde and a provider/model slug.
/// They remain distinct from the concrete model to which they currently route.
pub fn bounded_model_id(value: &str) -> AppResult<String> {
    if let Some(alias) = value.strip_prefix('~') {
        if value.len() > 160
            || alias
                .split_once('/')
                .is_none_or(|(provider, model)| provider.is_empty() || model.is_empty())
        {
            return Err(invalid("Unsupported model alias identity"));
        }
        bounded_id(alias)?;
        Ok(value.into())
    } else {
        bounded_id(value)
    }
}
