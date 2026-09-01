const SENSITIVE_MARKERS: &[&str] = &[
    "authorization",
    "api-key",
    "api_key",
    "x-api-key",
    "anthropic-api-key",
    "copilot_provider_api_key",
    "bearer ",
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
    if let Some(index) = line.find(':') {
        return format!("{}: [REDACTED]", &line[..index]);
    }
    if let Some(index) = line.find('=') {
        return format!("{}=[REDACTED]", &line[..index]);
    }
    "[REDACTED]".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
