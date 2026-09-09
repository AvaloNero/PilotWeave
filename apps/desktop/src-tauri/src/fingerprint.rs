//! Domain-separated, length-delimited SHA-256 fingerprints. Never log inputs.
use crate::error::{AppError, AppResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn bytes(domain: &str, value: Option<&[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_le_bytes());
    hasher.update(domain.as_bytes());
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value);
        }
        None => hasher.update([0]),
    }
    format!("{:x}", hasher.finalize())
}

pub fn json(domain: &str, value: &impl Serialize) -> AppResult<String> {
    let serialized = serde_json::to_vec(value)
        .map_err(|_| AppError::Config("Could not fingerprint native state".into()))?;
    Ok(bytes(domain, Some(&serialized)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_presence_and_content_are_distinct() {
        let empty = bytes("file", Some(b""));
        assert_eq!(empty.len(), 64);
        assert_ne!(empty, bytes("file", None));
        assert_ne!(empty, bytes("credential", Some(b"")));
        assert_ne!(empty, bytes("file", Some(b"changed")));
        assert_eq!(empty, bytes("file", Some(b"")));
    }
}
