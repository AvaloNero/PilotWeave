const SENSITIVE_MARKERS: &[&str] = &[
    "authorization",
    "api-key",
    "api_key",
    "x-api-key",
    "anthropic-api-key",
    "copilot_provider_api_key",
    "bearer ",
    "github_token",
    "gh_token",
    "refresh_token",
    "access_token",
    "cookie",
    "x-goog-api-key",
];

pub fn redact_text(input: &str) -> String {
    input
        .lines()
        .map(redact_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn redact_with_secret(input: &str, secret: Option<&str>) -> String {
    let mut value = redact_text(input);
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        value = value.replace(secret, "[REDACTED]");
    }
    value
}

fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    if !SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return line.to_string();
    }
    let trimmed = line.trim_start();
    for separator in [':', '='] {
        if let Some(index) = trimmed.find(separator) {
            let label = &trimmed[..index];
            if !label.is_empty()
                && label.len() <= 128
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return if separator == ':' {
                    format!("{label}: [REDACTED]")
                } else {
                    format!("{label}=[REDACTED]")
                };
            }
        }
    }
    "[REDACTED]".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_keep_credentials_in_prefix_before_a_colon() {
        let value = redact_text("Bearer literal-secret: upstream error");
        assert!(!value.contains("literal-secret"));
        assert!(!redact_text("GITHUB_TOKEN=secret").contains("=secret"));
    }

    #[test]
    fn redacts_common_credential_lines() {
        assert_eq!(
            redact_text("Authorization: Bearer abc"),
            "Authorization: [REDACTED]"
        );
        assert_eq!(
            redact_text("COPILOT_PROVIDER_API_KEY=secret"),
            "COPILOT_PROVIDER_API_KEY=[REDACTED]"
        );
    }

    #[test]
    fn redacts_exact_runtime_secret_even_without_a_marker() {
        assert_eq!(
            redact_with_secret("failed while using super-secret", Some("super-secret")),
            "failed while using [REDACTED]"
        );
    }
}
