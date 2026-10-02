//! Reviewed WinGet source-export object schema v1. A source name alone is not
//! repository identity; custom sources named `winget` must never authorize work.
use crate::error::{AppError, AppResult};
use std::{path::Path, sync::atomic::AtomicBool};

fn unsupported() -> AppError {
    AppError::Unsupported("The configured WinGet repository does not match the reviewed Microsoft source-export schema; no installer was launched".into())
}

pub(super) fn observe(executable: &Path, cancel: Option<&AtomicBool>) -> AppResult<String> {
    let output = crate::native_process::run_capture_cancelable(
        executable,
        &[
            std::ffi::OsStr::new("source"),
            std::ffi::OsStr::new("export"),
            std::ffi::OsStr::new("winget"),
        ],
        std::time::Duration::from_secs(15),
        64 * 1024,
        crate::native_process::CaptureMode::Standard,
        cancel,
    )?;
    if !output.status.success() || output.stdout_truncated || output.stderr_truncated {
        return Err(unsupported());
    }
    parse(output.stdout.trim_start_matches('\u{feff}').as_bytes())
}

fn parse(bytes: &[u8]) -> AppResult<String> {
    if bytes.len() > 64 * 1024 {
        return Err(unsupported());
    }
    let object: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| unsupported())?;
    let fields = object.as_object().ok_or_else(unsupported)?;
    if fields.keys().any(|k| {
        !matches!(
            k.as_str(),
            "Name"
                | "Arg"
                | "Type"
                | "Data"
                | "Identifier"
                | "TrustLevel"
                | "Explicit"
                | "Priority"
        )
    }) {
        return Err(unsupported());
    }
    for (key, expected) in [
        ("Name", "winget"),
        ("Arg", "https://cdn.winget.microsoft.com/cache"),
        ("Type", "Microsoft.PreIndexed.Package"),
        ("Identifier", "Microsoft.Winget.Source_8wekyb3d8bbwe"),
        ("Data", "Microsoft.Winget.Source_8wekyb3d8bbwe"),
    ] {
        if object[key].as_str() != Some(expected) {
            return Err(unsupported());
        }
    }
    if object.get("Explicit").is_some_and(|v| !v.is_boolean())
        || object.get("Priority").is_some_and(|v| v.as_i64().is_none())
        || object.get("TrustLevel").is_some_and(|v| {
            v.as_array().is_none_or(|a| {
                a.len() > 4 || a.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 64))
            })
        })
    {
        return Err(unsupported());
    }
    crate::fingerprint::json("winget-source-export-v1", &object)
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/installer/winget-source-export-v1.json");
    #[test]
    fn source_name_is_not_sufficient_and_unknown_schemas_are_not_adopted() {
        assert_eq!(parse(FIXTURE).unwrap().len(), 64);
        let mut value: serde_json::Value = serde_json::from_slice(FIXTURE).unwrap();
        value["Arg"] = "https://private-fixture.invalid/cache?token=do-not-echo".into();
        let error = parse(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-fixture"));
        assert!(!error.contains("do-not-echo"));
        assert!(parse(br#"{"Name":"winget"}"#).is_err());
        assert!(parse(b"[]").is_err());
        value = serde_json::from_slice(FIXTURE).unwrap();
        value["FutureSchema"] = true.into();
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
