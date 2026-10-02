//! Public BYOK registry precedence (documented registry v1, 2026-10-02).
//! Count declarations only; never forward provider contents or credentials.
use crate::error::{AppError, AppResult};
use std::path::Path;

pub(super) fn has_declarations(path: &Path) -> AppResult<bool> {
    let Some(bytes) = crate::safe_file::read_optional(path, 1024 * 1024)? else {
        return Ok(false);
    };
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> AppResult<bool> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| schema_error())?;
    let object = value.as_object().ok_or_else(schema_error)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "providers" | "models" | "$schema"))
    {
        return Err(schema_error());
    }
    let mut declared = false;
    for key in ["providers", "models"] {
        if let Some(value) = object.get(key) {
            let count = match value {
                serde_json::Value::Object(values) => values.len(),
                serde_json::Value::Array(values) => values.len(),
                _ => return Err(schema_error()),
            };
            if count > 256 {
                return Err(schema_error());
            }
            declared |= count > 0;
        }
    }
    Ok(declared)
}

fn schema_error() -> AppError {
    AppError::Unsupported("Unsupported public Copilot providers.json schema; legacy environment deployment was not attempted. Review this file in the official client".into())
}

pub(super) fn routing_fingerprint() -> AppResult<String> {
    let home = crate::platform::home_dir().ok_or_else(|| {
        AppError::Config("Cannot resolve Copilot provider configuration root".into())
    })?;
    let bytes =
        crate::safe_file::read_optional(&home.join(".copilot/providers.json"), 1024 * 1024)?;
    crate::fingerprint::json(
        "cli-routing-precedence-v1",
        &(
            crate::fingerprint::bytes("public-provider-registry-v1", bytes.as_deref()),
            override_presence()?,
        ),
    )
}

pub(super) fn ensure_environment_is_effective() -> AppResult<()> {
    if override_presence()?.into_iter().any(|value| value) {
        return Err(AppError::Unsupported("Copilot has a custom public configuration root. Manage provider routing in the official client; PilotWeave did not change those overrides".into()));
    }
    let home = crate::platform::home_dir().ok_or_else(|| {
        AppError::Config("Cannot resolve Copilot provider configuration root".into())
    })?;
    if has_declarations(&home.join(".copilot/providers.json"))? {
        return Err(AppError::Unsupported("Copilot providers.json takes precedence over legacy COPILOT_PROVIDER_* variables. Its existing declarations are not owned by PilotWeave; use the official client to review them before environment deployment".into()));
    }
    Ok(())
}

pub(super) fn override_presence() -> AppResult<[bool; 2]> {
    let inherited = [
        std::env::var_os("COPILOT_HOME").is_some(),
        std::env::var_os("COPILOT_PROVIDERS_CONFIG").is_some(),
    ];
    #[cfg(windows)]
    {
        let current = super::env_snapshot::routing_overrides()?;
        Ok([inherited[0] || current[0], inherited[1] || current[1]])
    }
    #[cfg(not(windows))]
    Ok(inherited)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declarations_shadow_environment_and_unknown_schema_is_not_empty() {
        assert!(!parse(b"{}").unwrap());
        assert!(!parse(br#"{"providers":{},"models":[]}"#).unwrap());
        assert!(parse(br#"{"providers":{"one":{"apiKey":"private-fixture"}}}"#).unwrap());
        assert!(parse(br#"{"models":[{"id":"one"}]}"#).unwrap());
        for bytes in [
            br#"{"providers":"private-fixture"}"#.as_slice(),
            br#"{"future":"private-fixture"}"#,
            b"[]",
        ] {
            let error = parse(bytes).unwrap_err().to_string();
            assert!(!error.contains("private-fixture"));
        }
    }
    #[test]
    fn public_registry_is_bounded_and_missing_is_not_a_declared_provider() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        assert!(!has_declarations(&path).unwrap());
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(has_declarations(&path).is_err());
    }
}
