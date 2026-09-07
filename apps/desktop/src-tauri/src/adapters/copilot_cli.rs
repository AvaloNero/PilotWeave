use crate::domain::{
    ApiProtocol, ClientKind, ClientStatus, ClientTarget, Connection, DeploymentOperation,
    ProviderKind,
};
use crate::error::{AppError, AppResult};
use std::collections::BTreeMap;
use std::env;
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use uuid::Uuid;

const MANAGED_VARIABLES: &[&str] = &[
    "COPILOT_PROVIDER_TYPE",
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_WIRE_API",
    "COPILOT_PROVIDER_HEADERS",
    "COPILOT_MODEL",
    "COPILOT_PROVIDER_MODEL_ID",
    "COPILOT_PROVIDER_WIRE_MODEL",
    "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS",
];

#[cfg(unix)]
const BLOCK_START: &str = "# >>> PilotWeave Copilot CLI >>>";
#[cfg(unix)]
const BLOCK_END: &str = "# <<< PilotWeave Copilot CLI <<<";

pub fn discover_target() -> ClientTarget {
    let executable = find_executable("copilot");
    let detected = executable.is_some();
    ClientTarget {
        id: "copilot-cli:user-environment".to_string(),
        kind: ClientKind::CopilotCli,
        name: "GitHub Copilot CLI".to_string(),
        detail: if detected {
            "User-level provider environment".to_string()
        } else {
            "The copilot executable was not found on PATH".to_string()
        },
        path: executable.map(|path| path.to_string_lossy().to_string()),
        detected,
        supports_write: detected,
        status: if detected {
            ClientStatus::Available
        } else {
            ClientStatus::NotInstalled
        },
        diagnostic: None,
    }
}

pub fn preview(connection: &Connection, target: &ClientTarget) -> DeploymentOperation {
    let model = connection
        .default_model()
        .map(|model| model.model_id.as_str())
        .unwrap_or("<no enabled model>");
    DeploymentOperation {
        id: Uuid::new_v4().to_string(),
        target_id: target.id.clone(),
        target_kind: ClientKind::CopilotCli,
        title: format!("Activate {} for Copilot CLI", connection.name),
        description: format!("Set the user-level CLI provider and default model to {model}"),
        changes: vec![
            "Write COPILOT_PROVIDER_* routing variables".to_string(),
            format!("Set COPILOT_MODEL and wire model to {model}"),
            "Store the materialized credential only in the platform user environment".to_string(),
            "New terminal processes will inherit the change".to_string(),
        ],
        supported: target.detected && target.supports_write,
        requires_restart: true,
    }
}

pub fn apply(
    connection: &Connection,
    secret: Option<&str>,
    _target: &ClientTarget,
) -> AppResult<String> {
    crate::validation::validate_connection(connection)?;
    let values = desired_environment(connection, secret)?;
    #[cfg(windows)]
    apply_windows(&values)?;
    #[cfg(unix)]
    apply_unix(&values)?;
    #[cfg(not(any(windows, unix)))]
    return Err(AppError::Unsupported(
        "Copilot CLI environment deployment is not implemented on this platform".into(),
    ));

    Ok("Activated the provider for newly opened Copilot CLI processes".to_string())
}

/// Compute the user-environment projection for `connection`. This is the exact
/// variable set that [`apply`] persists through the platform backend, so tests
/// can verify provider/model switching without touching the real environment.
pub fn desired_environment(
    connection: &Connection,
    secret: Option<&str>,
) -> AppResult<BTreeMap<String, Option<String>>> {
    let model = connection.default_model().ok_or_else(|| {
        AppError::InvalidInput("Copilot CLI requires at least one enabled model".to_string())
    })?;
    let provider_type = match connection.provider_kind {
        ProviderKind::Azure => "azure",
        ProviderKind::Anthropic => "anthropic",
        _ if connection.protocol == ApiProtocol::Messages => "anthropic",
        _ => "openai",
    };
    let wire_api = if provider_type == "anthropic" {
        None
    } else {
        Some(match connection.protocol {
            ApiProtocol::Responses => "responses".to_string(),
            ApiProtocol::ChatCompletions => "completions".to_string(),
            ApiProtocol::Messages => {
                return Err(AppError::InvalidInput(
                    "Provider does not support Messages".into(),
                ))
            }
        })
    };

    let headers = render_headers(connection, secret);
    let mut values = MANAGED_VARIABLES
        .iter()
        .map(|name| ((*name).to_string(), None))
        .collect::<BTreeMap<_, _>>();
    values.insert(
        "COPILOT_PROVIDER_TYPE".to_string(),
        Some(provider_type.to_string()),
    );
    values.insert(
        "COPILOT_PROVIDER_BASE_URL".to_string(),
        Some(connection.base_url.clone()),
    );
    values.insert(
        "COPILOT_PROVIDER_API_KEY".to_string(),
        secret.filter(|value| !value.is_empty()).map(str::to_string),
    );
    values.insert("COPILOT_PROVIDER_WIRE_API".to_string(), wire_api);
    values.insert(
        "COPILOT_PROVIDER_HEADERS".to_string(),
        (!headers.is_empty()).then_some(headers),
    );
    for name in [
        "COPILOT_MODEL",
        "COPILOT_PROVIDER_MODEL_ID",
        "COPILOT_PROVIDER_WIRE_MODEL",
    ] {
        values.insert(name.to_string(), Some(model.model_id.clone()));
    }
    values.insert(
        "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS".to_string(),
        model
            .capabilities
            .max_output_tokens
            .map(|value| value.to_string()),
    );
    Ok(values)
}

pub(crate) fn prepare(
    connection: &Connection,
    secret: Option<&str>,
) -> AppResult<Vec<crate::transaction::PreparedWrite>> {
    let values = desired_environment(connection, secret)?;
    #[cfg(windows)]
    {
        use winreg::types::ToRegValue;
        values
            .into_iter()
            .map(|(name, value)| {
                let resource = crate::transaction::Resource::UserEnvironment(name);
                let before = resource.read()?;
                if let Some(bytes) = &before {
                    if bytes.len() < 4
                        || !matches!(
                            u32::from_le_bytes(
                                bytes[..4].try_into().map_err(|_| AppError::Config(
                                    "Invalid registry value".into()
                                ))?
                            ),
                            1 | 2
                        )
                    {
                        return Err(AppError::Unsupported(
                            "Non-string provider environment requires manual review".into(),
                        ));
                    }
                }
                let after = value.map(|value| {
                    let raw = value.to_reg_value();
                    let mut bytes = (raw.vtype as u32).to_le_bytes().to_vec();
                    bytes.extend(raw.bytes);
                    bytes
                });
                Ok(crate::transaction::PreparedWrite {
                    resource,
                    before,
                    after,
                    restore_mode: None,
                    write_mode: None,
                })
            })
            .collect()
    }
    #[cfg(unix)]
    {
        let home =
            dirs::home_dir().ok_or_else(|| AppError::Config("Cannot resolve home".into()))?;
        prepare_unix_at(&home, &values)
    }
}

pub(crate) fn observed_resources() -> AppResult<Vec<crate::transaction::Resource>> {
    #[cfg(windows)]
    {
        Ok(MANAGED_VARIABLES
            .iter()
            .map(|name| crate::transaction::Resource::UserEnvironment((*name).into()))
            .collect())
    }
    #[cfg(unix)]
    {
        let home =
            dirs::home_dir().ok_or_else(|| AppError::Config("Cannot resolve home".into()))?;
        unix_paths(&home)
            .into_iter()
            .map(|path| {
                Ok(crate::transaction::Resource::File(
                    crate::safe_io::resource_path(&path)?,
                ))
            })
            .collect()
    }
}

pub(crate) fn notify_environment() -> Option<String> {
    #[cfg(windows)]
    if broadcast_environment_change().is_err() {
        return Some("Configuration is saved, but the environment-change notification failed; restart the terminal or sign out/in".into());
    }
    None
}

#[cfg(unix)]
fn unix_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".pilotweave/copilot-cli-env.sh"),
        home.join(".pilotweave/copilot-cli-env.fish"),
        home.join(".profile"),
        home.join(".bashrc"),
        home.join(".zshrc"),
        home.join(".config/fish/conf.d/pilotweave-copilot.fish"),
    ]
}

#[cfg(unix)]
fn prepare_unix_at(
    home: &Path,
    values: &BTreeMap<String, Option<String>>,
) -> AppResult<Vec<crate::transaction::PreparedWrite>> {
    use crate::transaction::PreparedWrite;
    let paths = unix_paths(home);
    let mut writes = vec![
        PreparedWrite::file(
            &paths[0],
            Some(render_posix_env(values).into_bytes()),
            false,
        )?,
        PreparedWrite::file(&paths[1], Some(render_fish_env(values).into_bytes()), false)?,
    ];
    let block = format!(
        "{BLOCK_START}\n. {}\n{BLOCK_END}",
        shell_quote(&paths[0].to_string_lossy())
    );
    for path in &paths[2..5] {
        let before = crate::safe_io::read_optional(path, crate::transaction::MAX_RESOURCE_BYTES)?;
        let text = std::str::from_utf8(before.as_deref().unwrap_or_default())
            .map_err(|_| AppError::Config("Shell profile is not UTF-8".into()))?;
        let after = replace_bounded_block(text, &block)?.into_bytes();
        let write = PreparedWrite::file(path, Some(after), true)?;
        if before != write.before {
            return Err(AppError::Config(
                "Shell profile changed during preparation".into(),
            ));
        }
        writes.push(write);
    }
    let fish = format!("source {}\n", fish_quote(&paths[1].to_string_lossy()));
    writes.push(PreparedWrite::file(
        &paths[5],
        Some(fish.into_bytes()),
        false,
    )?);
    Ok(writes)
}

fn render_headers(connection: &Connection, secret: Option<&str>) -> String {
    connection
        .headers
        .iter()
        .map(|(name, value)| {
            let value = secret
                .map(|secret| value.replace("${apiKey}", secret))
                .unwrap_or_else(|| value.clone());
            format!("{name}: {value}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions = executable_extensions();
    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        let raw = env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
        let mut extensions = raw
            .to_string_lossy()
            .split(';')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        extensions.insert(0, String::new());
        extensions
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

/// Writable user-environment backend. Production uses the Windows registry;
/// tests substitute an in-memory fake so the deployment transaction is never
/// exercised against the real user environment.
#[cfg(any(windows, test))]
trait UserEnvStore {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&mut self, name: &str, value: &str) -> std::io::Result<()>;
    fn delete(&mut self, name: &str) -> std::io::Result<()>;
}

/// Snapshot the managed variables, apply every change, and restore the
/// snapshot if any write fails so a partial environment is never left behind.
#[cfg(any(windows, test))]
fn apply_env_values(
    store: &mut dyn UserEnvStore,
    values: &BTreeMap<String, Option<String>>,
) -> AppResult<()> {
    let mut before = BTreeMap::new();
    for name in MANAGED_VARIABLES {
        before.insert((*name).to_string(), store.get(name));
    }

    for name in MANAGED_VARIABLES {
        let result = match values.get(*name).and_then(Option::as_deref) {
            Some(value) => store.set(name, value),
            None => match store.delete(name) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = result {
            let mut rollback_failed = false;
            for rollback_name in MANAGED_VARIABLES {
                match before.get(*rollback_name).and_then(Option::as_deref) {
                    Some(value) => {
                        rollback_failed |= store.set(rollback_name, value).is_err();
                    }
                    None => {
                        rollback_failed |= store.delete(rollback_name).is_err();
                    }
                }
            }
            return Err(AppError::Config(format!(
                "Failed to update {name}; rollback status: {}: {error}",
                if rollback_failed {
                    "incomplete; manual recovery required"
                } else {
                    "restored"
                }
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
struct RegistryEnvStore(winreg::RegKey);

#[cfg(windows)]
impl UserEnvStore for RegistryEnvStore {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get_value::<String, _>(name).ok()
    }

    fn set(&mut self, name: &str, value: &str) -> std::io::Result<()> {
        self.0.set_value(name, &value)
    }

    fn delete(&mut self, name: &str) -> std::io::Result<()> {
        self.0.delete_value(name)
    }
}

#[cfg(windows)]
fn apply_windows(values: &BTreeMap<String, Option<String>>) -> AppResult<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Environment")
        .map_err(|error| AppError::Config(format!("Failed to open HKCU\\Environment: {error}")))?;
    let mut store = RegistryEnvStore(key);
    apply_env_values(&mut store, values)?;
    if broadcast_environment_change().is_err() {
        log::warn!("Environment saved; notification failed. Open a new login session if needed");
    }
    Ok(())
}

#[cfg(windows)]
fn broadcast_environment_change() -> AppResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    let environment = std::ffi::OsStr::new("Environment")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut result = 0usize;
    let delivered = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            &mut result,
        )
    };
    if delivered == 0 {
        return Err(AppError::Config(
            "Updated HKCU\\Environment but failed to broadcast WM_SETTINGCHANGE".to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn apply_unix(values: &BTreeMap<String, Option<String>>) -> AppResult<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Config("Cannot resolve the user home directory".into()))?;
    apply_unix_at(&home, values)
}

/// Write the managed environment files and shell-profile blocks under `home`.
/// Split from [`apply_unix`] so tests can target a temporary home directory.
#[cfg(unix)]
fn apply_unix_at(home: &Path, values: &BTreeMap<String, Option<String>>) -> AppResult<()> {
    let writes = prepare_unix_at(home, values)?;
    let path = home.join(".pilotweave/cli-adapter-journal.json");
    let mut transaction =
        crate::transaction::Transaction::begin(path, &Uuid::new_v4().to_string(), writes)?;
    if let Err(error) = transaction.apply() {
        transaction.rollback()?;
        transaction.clear()?;
        return Err(error);
    }
    transaction.complete()
}

#[cfg(unix)]
fn render_posix_env(values: &BTreeMap<String, Option<String>>) -> String {
    let mut output =
        String::from("# Managed by PilotWeave. Do not edit while management is enabled.\n");
    for name in MANAGED_VARIABLES {
        if let Some(value) = values.get(*name).and_then(Option::as_deref) {
            output.push_str(&format!("export {name}={}\n", shell_quote(value)));
        } else {
            output.push_str(&format!("unset {name}\n"));
        }
    }
    output
}

#[cfg(unix)]
fn render_fish_env(values: &BTreeMap<String, Option<String>>) -> String {
    let mut output =
        String::from("# Managed by PilotWeave. Do not edit while management is enabled.\n");
    for name in MANAGED_VARIABLES {
        if let Some(value) = values.get(*name).and_then(Option::as_deref) {
            output.push_str(&format!("set -gx {name} {}\n", fish_quote(value)));
        } else {
            output.push_str(&format!("set -e {name}\n"));
        }
    }
    output
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace("'", "'\"'\"'"))
}

#[cfg(unix)]
fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace("\\", "\\\\").replace("'", "\\'"))
}

#[cfg(unix)]
fn replace_bounded_block(existing: &str, block: &str) -> AppResult<String> {
    if existing.matches(BLOCK_START).count() > 1 || existing.matches(BLOCK_END).count() > 1 {
        return Err(AppError::Config(
            "Multiple managed shell blocks require manual review".into(),
        ));
    }
    match (existing.find(BLOCK_START), existing.find(BLOCK_END)) {
        (None, None) => {
            let mut output = existing.trim_end_matches('\n').to_string();
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(block);
            output.push('\n');
            Ok(output)
        }
        (Some(start), Some(end_start)) if end_start >= start => {
            let end = end_start + BLOCK_END.len();
            let mut output = String::new();
            output.push_str(&existing[..start]);
            output.push_str(block);
            output.push_str(&existing[end..]);
            if !output.ends_with('\n') {
                output.push('\n');
            }
            Ok(output)
        }
        _ => Err(AppError::Config(
            "Shell profile contains an incomplete PilotWeave block".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ModelCapabilities, ModelSpec};
    use chrono::Utc;
    use std::io;

    fn model(model_id: &str, enabled: bool) -> ModelSpec {
        ModelSpec {
            id: format!("id-{model_id}"),
            model_id: model_id.to_string(),
            name: format!("Model {model_id}"),
            enabled,
            capabilities: ModelCapabilities::default(),
        }
    }

    fn connection_with(
        provider_kind: ProviderKind,
        protocol: ApiProtocol,
        models: Vec<ModelSpec>,
    ) -> Connection {
        Connection {
            id: "one".to_string(),
            name: "One".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind,
            protocol,
            headers: BTreeMap::new(),
            models,
            secret_ref: "connection:one".to_string(),
            has_secret: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn connection() -> Connection {
        let mut spec = model("gpt-example", true);
        spec.capabilities.max_output_tokens = Some(16_384);
        connection_with(ProviderKind::Openai, ApiProtocol::Responses, vec![spec])
    }

    #[test]
    fn maps_responses_provider_environment() {
        let values = desired_environment(&connection(), Some("secret")).expect("environment");
        assert_eq!(
            values["COPILOT_PROVIDER_WIRE_API"].as_deref(),
            Some("responses")
        );
        assert_eq!(values["COPILOT_MODEL"].as_deref(), Some("gpt-example"));
        assert_eq!(
            values["COPILOT_PROVIDER_API_KEY"].as_deref(),
            Some("secret")
        );
        assert_eq!(
            values["COPILOT_PROVIDER_MAX_OUTPUT_TOKENS"].as_deref(),
            Some("16384")
        );
    }

    #[test]
    fn anthropic_protocol_uses_anthropic_provider_type() {
        let connection = connection_with(
            ProviderKind::Custom,
            ApiProtocol::Messages,
            vec![model("m", true)],
        );
        let values = desired_environment(&connection, Some("secret")).expect("environment");
        assert_eq!(
            values["COPILOT_PROVIDER_TYPE"].as_deref(),
            Some("anthropic")
        );
        assert_eq!(values["COPILOT_PROVIDER_WIRE_API"], None);
    }

    #[test]
    fn anthropic_provider_kind_forces_anthropic_type() {
        let connection = connection_with(
            ProviderKind::Anthropic,
            ApiProtocol::ChatCompletions,
            vec![model("m", true)],
        );
        let values = desired_environment(&connection, Some("secret")).expect("environment");
        assert_eq!(
            values["COPILOT_PROVIDER_TYPE"].as_deref(),
            Some("anthropic")
        );
        assert_eq!(values["COPILOT_PROVIDER_WIRE_API"], None);
    }

    #[test]
    fn azure_connections_use_azure_provider_type() {
        let connection = connection_with(
            ProviderKind::Azure,
            ApiProtocol::Responses,
            vec![model("m", true)],
        );
        let values = desired_environment(&connection, None).expect("environment");
        assert_eq!(values["COPILOT_PROVIDER_TYPE"].as_deref(), Some("azure"));
        assert_eq!(
            values["COPILOT_PROVIDER_WIRE_API"].as_deref(),
            Some("responses")
        );
    }

    #[test]
    fn chat_completions_maps_to_completions_wire_api() {
        let connection = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("m", true)],
        );
        let values = desired_environment(&connection, None).expect("environment");
        assert_eq!(values["COPILOT_PROVIDER_TYPE"].as_deref(), Some("openai"));
        assert_eq!(
            values["COPILOT_PROVIDER_WIRE_API"].as_deref(),
            Some("completions")
        );
    }

    #[test]
    fn first_enabled_model_drives_every_model_variable() {
        let connection = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("disabled-model", false), model("active-model", true)],
        );
        let values = desired_environment(&connection, None).expect("environment");
        for name in [
            "COPILOT_MODEL",
            "COPILOT_PROVIDER_MODEL_ID",
            "COPILOT_PROVIDER_WIRE_MODEL",
        ] {
            assert_eq!(values[name].as_deref(), Some("active-model"), "{name}");
        }
    }

    #[test]
    fn switching_models_rewrites_every_model_variable() {
        let before = desired_environment(
            &connection_with(
                ProviderKind::Openai,
                ApiProtocol::ChatCompletions,
                vec![model("model-a", true)],
            ),
            Some("secret"),
        )
        .expect("environment for model-a");
        let after = desired_environment(
            &connection_with(
                ProviderKind::Openai,
                ApiProtocol::ChatCompletions,
                vec![model("model-b", true)],
            ),
            Some("secret"),
        )
        .expect("environment for model-b");

        for name in [
            "COPILOT_MODEL",
            "COPILOT_PROVIDER_MODEL_ID",
            "COPILOT_PROVIDER_WIRE_MODEL",
        ] {
            assert_eq!(before[name].as_deref(), Some("model-a"), "{name} before");
            assert_eq!(after[name].as_deref(), Some("model-b"), "{name} after");
        }
        // Routing variables stay stable across a pure model switch.
        assert_eq!(
            before["COPILOT_PROVIDER_TYPE"],
            after["COPILOT_PROVIDER_TYPE"]
        );
        assert_eq!(
            before["COPILOT_PROVIDER_BASE_URL"],
            after["COPILOT_PROVIDER_BASE_URL"]
        );
    }

    #[test]
    fn empty_secret_clears_api_key_but_keeps_routing() {
        let values = desired_environment(&connection(), Some("")).expect("environment");
        assert_eq!(values["COPILOT_PROVIDER_API_KEY"], None);
        assert_eq!(
            values["COPILOT_PROVIDER_BASE_URL"].as_deref(),
            Some("https://example.invalid/v1")
        );
    }

    #[test]
    fn header_placeholders_are_materialized_with_the_secret() {
        let mut connection = connection();
        connection
            .headers
            .insert("X-Custom-Auth".to_string(), "Bearer ${apiKey}".to_string());
        let values = desired_environment(&connection, Some("sekret")).expect("environment");
        assert_eq!(
            values["COPILOT_PROVIDER_HEADERS"].as_deref(),
            Some("X-Custom-Auth: Bearer sekret")
        );
    }

    #[test]
    fn a_connection_without_enabled_models_is_rejected() {
        let connection = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("m", false)],
        );
        let error = desired_environment(&connection, None).expect_err("must fail");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[derive(Default)]
    struct FakeEnvStore {
        values: BTreeMap<String, String>,
        fail_on_set: Option<String>,
    }

    impl UserEnvStore for FakeEnvStore {
        fn get(&self, name: &str) -> Option<String> {
            self.values.get(name).cloned()
        }

        fn set(&mut self, name: &str, value: &str) -> io::Result<()> {
            if self.fail_on_set.as_deref() == Some(name) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"));
            }
            self.values.insert(name.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&mut self, name: &str) -> io::Result<()> {
            match self.values.remove(name) {
                Some(_) => Ok(()),
                // Mirror the registry: deleting an absent value reports NotFound.
                None => Err(io::Error::new(io::ErrorKind::NotFound, "absent")),
            }
        }
    }

    fn populated_store() -> FakeEnvStore {
        let mut store = FakeEnvStore::default();
        store.values.insert(
            "COPILOT_PROVIDER_BASE_URL".to_string(),
            "https://old.invalid/v1".to_string(),
        );
        store
            .values
            .insert("COPILOT_MODEL".to_string(), "old-model".to_string());
        store
            .values
            .insert("UNRELATED_VARIABLE".to_string(), "must survive".to_string());
        store
    }

    #[test]
    fn apply_env_values_projects_and_clears_variables() {
        let mut store = populated_store();
        let values = desired_environment(
            &connection_with(
                ProviderKind::Openai,
                ApiProtocol::ChatCompletions,
                vec![model("new-model", true)],
            ),
            Some("new-secret"),
        )
        .expect("environment");
        apply_env_values(&mut store, &values).expect("apply");

        assert_eq!(
            store.values["COPILOT_PROVIDER_BASE_URL"],
            "https://example.invalid/v1"
        );
        assert_eq!(store.values["COPILOT_MODEL"], "new-model");
        assert_eq!(store.values["COPILOT_PROVIDER_API_KEY"], "new-secret");
        assert_eq!(store.values["COPILOT_PROVIDER_WIRE_API"], "completions");
        // Desired `None` entries clear stale variables.
        assert!(!store.values.contains_key("COPILOT_PROVIDER_HEADERS"));
        assert!(!store
            .values
            .contains_key("COPILOT_PROVIDER_MAX_OUTPUT_TOKENS"));
        // Variables PilotWeave does not manage are never touched.
        assert_eq!(store.values["UNRELATED_VARIABLE"], "must survive");
    }

    #[test]
    fn apply_env_values_restores_the_snapshot_on_failure() {
        let mut store = populated_store();
        store.fail_on_set = Some("COPILOT_MODEL".to_string());
        let values = desired_environment(
            &connection_with(
                ProviderKind::Openai,
                ApiProtocol::ChatCompletions,
                vec![model("new-model", true)],
            ),
            Some("new-secret"),
        )
        .expect("environment");

        let error = apply_env_values(&mut store, &values).expect_err("must fail");
        assert!(error.to_string().contains("COPILOT_MODEL"));

        // Every managed variable is back to its pre-apply state.
        assert_eq!(
            store.values["COPILOT_PROVIDER_BASE_URL"],
            "https://old.invalid/v1"
        );
        assert_eq!(store.values["COPILOT_MODEL"], "old-model");
        assert!(!store.values.contains_key("COPILOT_PROVIDER_API_KEY"));
        assert!(!store.values.contains_key("COPILOT_PROVIDER_TYPE"));
        assert_eq!(store.values["UNRELATED_VARIABLE"], "must survive");
    }

    #[test]
    fn repeated_apply_of_the_same_model_is_idempotent() {
        let mut store = FakeEnvStore::default();
        let values = desired_environment(&connection(), Some("secret")).expect("environment");
        apply_env_values(&mut store, &values).expect("first apply");
        let once = store.values.clone();
        apply_env_values(&mut store, &values).expect("second apply");
        assert_eq!(once, store.values);
    }

    #[cfg(unix)]
    #[test]
    fn updates_a_bounded_shell_block_idempotently() {
        let block = format!("{BLOCK_START}\n. '/tmp/env'\n{BLOCK_END}");
        let once = replace_bounded_block("export BEFORE=1\n", &block).expect("insert block");
        let twice = replace_bounded_block(&once, &block).expect("replace block");
        assert_eq!(once, twice);
        assert!(once.contains("export BEFORE=1"));
    }

    #[cfg(unix)]
    #[test]
    fn apply_unix_at_writes_managed_files_under_the_given_home() {
        let home = tempfile::tempdir().expect("temp home");
        let values = desired_environment(&connection(), Some("secret")).expect("environment");
        apply_unix_at(home.path(), &values).expect("first apply");
        // Re-applying must be idempotent for both env files and profile blocks.
        apply_unix_at(home.path(), &values).expect("second apply");

        let env_file = home.path().join(".pilotweave/copilot-cli-env.sh");
        let env_text = std::fs::read_to_string(&env_file).expect("env file");
        assert!(env_text.contains("export COPILOT_PROVIDER_TYPE='openai'"));
        assert!(env_text.contains("export COPILOT_MODEL='gpt-example'"));
        assert!(env_text.contains("export COPILOT_PROVIDER_API_KEY='secret'"));
        assert!(env_text.contains("unset COPILOT_PROVIDER_HEADERS"));

        let fish_file = home.path().join(".pilotweave/copilot-cli-env.fish");
        let fish_text = std::fs::read_to_string(&fish_file).expect("fish env file");
        assert!(fish_text.contains("set -gx COPILOT_MODEL 'gpt-example'"));

        for name in [".profile", ".bashrc", ".zshrc"] {
            let text = std::fs::read_to_string(home.path().join(name)).expect("profile file");
            assert_eq!(text.matches(BLOCK_START).count(), 1, "{name}");
            assert!(text.contains(". '"), "{name}");
        }
        let fish_hook = home
            .path()
            .join(".config/fish/conf.d/pilotweave-copilot.fish");
        assert!(fish_hook.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn apply_unix_at_refuses_a_symlinked_profile() {
        let home = tempfile::tempdir().expect("temp home");
        let outside = home.path().join("elsewhere");
        std::fs::write(&outside, "export REAL=1\n").expect("outside file");
        std::os::unix::fs::symlink(&outside, home.path().join(".bashrc")).expect("symlink");

        let values = desired_environment(&connection(), Some("secret")).expect("environment");
        let error = apply_unix_at(home.path(), &values).expect_err("must refuse symlink");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }
}
