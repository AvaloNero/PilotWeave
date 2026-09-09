use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("PlanChanged: the connection, credential, application state, or target changed; preview again")]
    PlanChanged,

    #[error("PlanUnavailable: preview expired, was consumed, or does not belong to this instance; preview again")]
    PlanUnavailable,

    #[error("Another managed write is in progress; wait for it to finish and preview again")]
    Busy,

    #[error("{0}")]
    InvalidInput(String),

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Unsupported(String),

    #[error("credential store error: {0}")]
    Secret(String),

    #[error("failed to access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    // Parser diagnostics can echo hostile source text (including credentials).
    #[error("failed to parse JSON from {path}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("application state lock is poisoned")]
    Lock,
}

impl AppError {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn json(path: impl AsRef<Path>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.as_ref().display().to_string(),
            source,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_errors_do_not_echo_attacker_controlled_credential_like_values() {
        #[derive(Debug, serde::Deserialize)]
        enum Fixture {
            Expected,
        }
        let source = serde_json::from_str::<Fixture>("\"private-fixture-value\"").unwrap_err();
        assert!(source.to_string().contains("private-fixture-value"));
        let rendered = AppError::json("fixture.json", source).to_string();
        assert!(!rendered.contains("private-fixture-value"));
    }
}
