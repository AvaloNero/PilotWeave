//! Raw, bounded registry observations. Never expand or stringify secret values.
use super::MANAGED_VARIABLES;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_VALUE_BYTES: usize = 128 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct RawValue {
    pub value_type: u32,
    pub bytes: Vec<u8>,
}

fn validate(value: &RawValue) -> AppResult<()> {
    // Preserve REG_SZ versus REG_EXPAND_SZ and even an absent terminator exactly.
    // Other registry types are not supported environment state, not missing data.
    if !matches!(value.value_type, 1 | 2) || value.bytes.len() > MAX_VALUE_BYTES {
        return Err(AppError::InvalidInput(
            "Managed environment value has an unsupported type or exceeds its size limit".into(),
        ));
    }
    Ok(())
}

fn snapshot_with(
    mut read: impl FnMut(&str) -> AppResult<Option<RawValue>>,
) -> AppResult<BTreeMap<String, Option<RawValue>>> {
    let mut result = BTreeMap::new();
    for name in MANAGED_VARIABLES {
        let value = read(name)?;
        if let Some(value) = &value {
            validate(value)?;
        }
        result.insert((*name).into(), value);
    }
    Ok(result)
}

#[cfg(windows)]
pub(super) fn native_snapshot() -> AppResult<BTreeMap<String, Option<RawValue>>> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = match winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Environment", KEY_READ)
    {
        Ok(key) => Some(key),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return Err(AppError::Config(
                "Cannot read the user environment; its state is unknown".into(),
            ))
        }
    };
    snapshot_with(|name| match key.as_ref() {
        Some(key) => read_bounded(key, name),
        None => Ok(None),
    })
}

#[cfg(windows)]
fn read_bounded(key: &winreg::RegKey, name: &str) -> AppResult<Option<RawValue>> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::RegQueryValueExW;
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // A fixed bounded buffer avoids allocating an attacker-controlled registry size.
    let mut bytes = vec![0; MAX_VALUE_BYTES];
    let mut size = bytes.len() as u32;
    let mut value_type = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.raw_handle() as _,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => return Ok(None),
        ERROR_MORE_DATA => {
            return Err(AppError::InvalidInput(
                "Managed environment value exceeds its size limit".into(),
            ))
        }
        ERROR_SUCCESS => {}
        _ => {
            return Err(AppError::Config(
                "Cannot read a managed environment value; its state is unknown".into(),
            ))
        }
    }
    if size as usize > MAX_VALUE_BYTES {
        return Err(AppError::InvalidInput(
            "Managed environment value exceeds its size limit".into(),
        ));
    }
    bytes.truncate(size as usize);
    Ok(Some(RawValue { value_type, bytes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_registry_type_presence_and_secret_changes_affect_the_fingerprint() {
        let observed = |value_type, value: Option<&[u8]>| {
            snapshot_with(|name| {
                Ok(if name == "COPILOT_PROVIDER_API_KEY" {
                    value.map(|bytes| RawValue {
                        value_type,
                        bytes: bytes.into(),
                    })
                } else {
                    None
                })
            })
            .unwrap()
        };
        let digest = |value| crate::fingerprint::json("environment", &value).unwrap();
        let before = digest(observed(1, Some(b"fixture")));
        assert_ne!(before, digest(observed(2, Some(b"fixture"))));
        assert_ne!(before, digest(observed(1, Some(b"rotated"))));
        assert_ne!(digest(observed(1, None)), digest(observed(1, Some(b""))));
    }

    #[test]
    fn access_failure_oversize_and_unknown_type_are_not_missing_values() {
        assert!(snapshot_with(|_| Err(AppError::Config("access denied".into()))).is_err());
        assert!(snapshot_with(|_| Ok(Some(RawValue {
            value_type: 1,
            bytes: vec![0; MAX_VALUE_BYTES + 1]
        })))
        .is_err());
        assert!(snapshot_with(|_| Ok(Some(RawValue {
            value_type: 3,
            bytes: vec![]
        })))
        .is_err());
    }
}
