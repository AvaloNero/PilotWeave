use crate::domain::{
    ApiProtocol, ClientKind, ClientStatus, ClientTarget, Connection, DeploymentOperation,
};
use crate::error::{AppError, AppResult};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const LANGUAGE_MODELS_FILE: &str = "chatLanguageModels.json";
const PROFILE_STORAGE_FILE: &str = "storage.json";
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DISCOVERED_TARGETS: usize = 64;
const MANAGED_MARKER: &str = "pilotWeaveManaged";
const CONNECTION_ID_FIELD: &str = "pilotWeaveConnectionId";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsCodeStorage {
    #[serde(default)]
    user_data_profiles: Vec<VsCodeStoredProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsCodeStoredProfile {
    #[serde(default)]
    location: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    use_default_flags: Option<VsCodeUseDefaultFlags>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsCodeUseDefaultFlags {
    #[serde(default)]
    language_models: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Edition {
    Stable,
    Insiders,
}

impl Edition {
    fn slug(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Insiders => "insiders",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Stable => "Visual Studio Code",
            Self::Insiders => "Visual Studio Code Insiders",
        }
    }
}

pub fn discover_targets() -> AppResult<Vec<ClientTarget>> {
    let roots = default_user_roots();
    let mut targets = Vec::new();

    for (edition, user_dir) in roots {
        if !user_dir.is_dir() {
            continue;
        }

        targets.push(make_target(
            edition, &user_dir, None, "Default", &user_dir, false,
        ));
        if targets.len() >= MAX_DISCOVERED_TARGETS {
            break;
        }

        let profiles_dir = user_dir.join("profiles");
        if !profiles_dir.is_dir() {
            continue;
        }

        for profile in read_stored_profiles(&user_dir).unwrap_or_else(|error| {
            log::warn!(
                "Failed to read VS Code profile metadata from {}: {error}",
                user_dir.display()
            );
            Vec::new()
        }) {
            if targets.len() >= MAX_DISCOVERED_TARGETS {
                break;
            }
            let location = profile.location.trim();
            let name = profile.name.trim();
            if location.is_empty() || name.is_empty() {
                continue;
            }
            let Some(profile_dir) = resolve_profile_dir(&profiles_dir, location) else {
                continue;
            };
            let inherits_models = if location.replace('\\', "/") == "builtin/agents" {
                true
            } else {
                profile
                    .use_default_flags
                    .unwrap_or_default()
                    .language_models
            };
            targets.push(make_target(
                edition,
                &user_dir,
                Some(location),
                name,
                &profile_dir,
                inherits_models,
            ));
        }
    }

    if targets.is_empty() {
        targets.push(ClientTarget {
            id: "vscode:not-installed".to_string(),
            kind: ClientKind::VsCodeCopilot,
            name: "VS Code Copilot".to_string(),
            detail: "No VS Code user data directory was detected".to_string(),
            path: None,
            detected: false,
            supports_write: false,
            status: ClientStatus::NotInstalled,
            diagnostic: None,
        });
    }

    targets.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(targets)
}

pub fn preview(connection: &Connection, target: &ClientTarget) -> DeploymentOperation {
    let model_count = connection.enabled_models().count();
    DeploymentOperation {
        id: Uuid::new_v4().to_string(),
        target_id: target.id.clone(),
        target_kind: ClientKind::VsCodeCopilot,
        title: format!("Deploy {} to {}", connection.name, target.name),
        description: target
            .path
            .as_deref()
            .map(|path| format!("Update {path}"))
            .unwrap_or_else(|| "VS Code is not available".to_string()),
        changes: vec![
            format!("Publish {model_count} enabled model(s) as one Custom Endpoint group"),
            "Preserve every group not owned by this PilotWeave connection".to_string(),
            "Create a private rollback backup before the first write".to_string(),
            "Materialize the credential only in the native client configuration".to_string(),
        ],
        supported: target.detected && target.supports_write,
        requires_restart: false,
    }
}

pub fn apply(
    connection: &Connection,
    secret: Option<&str>,
    target: &ClientTarget,
) -> AppResult<String> {
    let path =
        target.path.as_deref().map(PathBuf::from).ok_or_else(|| {
            AppError::Unsupported("VS Code target has no configuration path".into())
        })?;
    ensure_regular_file_or_missing(&path)?;
    let mut groups = read_groups(&path)?;
    groups.retain(|group| !is_owned_group(group, &connection.id));
    if connection.enabled_models().next().is_some() {
        groups.push(render_group(connection, secret));
    }

    let bytes = serde_json::to_vec_pretty(&Value::Array(groups)).map_err(|error| {
        AppError::Config(format!("Failed to serialize VS Code models: {error}"))
    })?;
    let mut bytes_with_newline = bytes;
    bytes_with_newline.push(b'\n');
    create_backup_once(&path)?;
    atomic_write_private(&path, &bytes_with_newline)?;
    Ok(format!("Updated {}", path.display()))
}

fn default_user_roots() -> Vec<(Edition, PathBuf)> {
    let Some(config_dir) = dirs::config_dir() else {
        return Vec::new();
    };
    vec![
        (Edition::Stable, config_dir.join("Code").join("User")),
        (
            Edition::Insiders,
            config_dir.join("Code - Insiders").join("User"),
        ),
    ]
}

fn make_target(
    edition: Edition,
    user_dir: &Path,
    profile_id: Option<&str>,
    profile_name: &str,
    profile_dir: &Path,
    inherits_models: bool,
) -> ClientTarget {
    let model_home = if inherits_models {
        user_dir
    } else {
        profile_dir
    };
    let path = model_home.join(LANGUAGE_MODELS_FILE);
    let id = profile_id
        .map(|profile| format!("vscode:{}:profile:{profile}", edition.slug()))
        .unwrap_or_else(|| format!("vscode:{}:default", edition.slug()));
    let name = if profile_id.is_some() {
        format!("{} · {}", edition.display_name(), profile_name)
    } else {
        format!("{} · Default", edition.display_name())
    };
    let detail = if inherits_models {
        "Named profile inheriting default language models".to_string()
    } else if profile_id.is_some() {
        "Named profile with its own language-model catalog".to_string()
    } else {
        "Default profile language-model catalog".to_string()
    };
    ClientTarget {
        id,
        kind: ClientKind::VsCodeCopilot,
        name,
        detail,
        path: Some(path.to_string_lossy().to_string()),
        detected: true,
        supports_write: true,
        status: ClientStatus::Available,
        diagnostic: None,
    }
}

fn read_stored_profiles(user_dir: &Path) -> AppResult<Vec<VsCodeStoredProfile>> {
    let path = user_dir.join("globalStorage").join(PROFILE_STORAGE_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    ensure_regular_file_or_missing(&path)?;
    let metadata = fs::metadata(&path).map_err(|error| AppError::io(&path, error))?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(AppError::InvalidInput(format!(
            "VS Code profile metadata exceeds {} MiB",
            MAX_CONFIG_BYTES / 1024 / 1024
        )));
    }
    let bytes = fs::read(&path).map_err(|error| AppError::io(&path, error))?;
    serde_json::from_slice::<VsCodeStorage>(&bytes)
        .map(|storage| storage.user_data_profiles)
        .map_err(|error| AppError::json(&path, error))
}

fn resolve_profile_dir(profiles_dir: &Path, location: &str) -> Option<PathBuf> {
    let relative = Path::new(location);
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let profile_dir = profiles_dir.join(relative);
    if !profile_dir.is_dir() {
        return None;
    }
    let canonical_root = fs::canonicalize(profiles_dir).ok()?;
    let canonical_profile = fs::canonicalize(&profile_dir).ok()?;
    canonical_profile
        .starts_with(canonical_root)
        .then_some(profile_dir)
}

fn read_groups(path: &Path) -> AppResult<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(path).map_err(|error| AppError::io(path, error))?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(AppError::InvalidInput(format!(
            "VS Code model configuration exceeds {} MiB",
            MAX_CONFIG_BYTES / 1024 / 1024
        )));
    }
    let text = fs::read_to_string(path).map_err(|error| AppError::io(path, error))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = json5::from_str(&text).map_err(|error| {
        AppError::Config(format!(
            "Failed to parse VS Code model configuration {}: {error}",
            path.display()
        ))
    })?;
    match value {
        Value::Array(groups) => Ok(groups),
        Value::Object(_) => Ok(vec![value]),
        _ => Err(AppError::InvalidInput(format!(
            "VS Code model configuration must be an array or object: {}",
            path.display()
        ))),
    }
}

fn is_owned_group(value: &Value, connection_id: &str) -> bool {
    value.get("vendor").and_then(Value::as_str) == Some("customendpoint")
        && value.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true)
        && value.get(CONNECTION_ID_FIELD).and_then(Value::as_str) == Some(connection_id)
}

fn render_group(connection: &Connection, secret: Option<&str>) -> Value {
    let headers = effective_headers(connection, secret);
    let models = connection
        .enabled_models()
        .map(|model| {
            let mut rendered = Map::new();
            rendered.insert("id".to_string(), json!(model.model_id));
            rendered.insert("name".to_string(), json!(model.name));
            rendered.insert("url".to_string(), json!(connection.base_url));
            rendered.insert(
                "toolCalling".to_string(),
                json!(model.capabilities.tool_calling.unwrap_or(true)),
            );
            if let Some(vision) = model.capabilities.vision {
                rendered.insert("vision".to_string(), json!(vision));
            }
            if let Some(reasoning) = model.capabilities.reasoning {
                rendered.insert("thinking".to_string(), json!(reasoning));
            }
            if let Some(context_window) = model.capabilities.context_window {
                rendered.insert("contextWindow".to_string(), json!(context_window));
            }
            if let Some(max_output_tokens) = model.capabilities.max_output_tokens {
                rendered.insert("maxOutputTokens".to_string(), json!(max_output_tokens));
            }
            if !headers.is_empty() {
                rendered.insert("requestHeaders".to_string(), json!(headers));
            }
            Value::Object(rendered)
        })
        .collect::<Vec<_>>();

    json!({
        "name": connection.name,
        "vendor": "customendpoint",
        "apiKey": secret.unwrap_or_default(),
        "apiType": connection.protocol.as_str(),
        "pilotWeaveManaged": true,
        "pilotWeaveConnectionId": connection.id,
        "models": models
    })
}

fn effective_headers(connection: &Connection, secret: Option<&str>) -> BTreeMap<String, String> {
    let mut headers = connection
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                secret
                    .map(|secret| value.replace("${apiKey}", secret))
                    .unwrap_or_else(|| value.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let has_auth = headers.keys().any(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "api-key" | "x-api-key" | "anthropic-api-key" | "x-goog-api-key"
        )
    });
    if !has_auth {
        if let Some(secret) = secret.filter(|value| !value.is_empty()) {
            if connection.protocol == ApiProtocol::Messages {
                headers.insert("x-api-key".to_string(), secret.to_string());
            } else {
                headers.insert("Authorization".to_string(), format!("Bearer {secret}"));
            }
        }
    }
    if connection.protocol == ApiProtocol::Messages
        && !headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("anthropic-version"))
    {
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
    }
    headers
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.pilotweave.bak")
}

fn create_backup_once(path: &Path) -> AppResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let backup = backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    let contents = fs::read(path).map_err(|error| AppError::io(path, error))?;
    atomic_write_private(&backup, &contents)
}

fn ensure_regular_file_or_missing(path: &Path) -> AppResult<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::InvalidInput(format!(
                "Refusing to modify a non-regular file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Target path has no parent directory".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    ensure_regular_file_or_missing(path)?;
    let temp = parent.join(format!(".pilotweave-{}.tmp", Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| AppError::io(&temp, error))?;
        }
        file.write_all(bytes)
            .map_err(|error| AppError::io(&temp, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp, error))?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|error| AppError::io(path, error))?;
        }
        fs::rename(&temp, path).map_err(|error| AppError::io(path, error))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ModelCapabilities, ModelSpec, ProviderKind};
    use chrono::Utc;

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
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.example/v1/chat/completions".to_string(),
            provider_kind,
            protocol,
            headers: BTreeMap::new(),
            models,
            secret_ref: "connection:openrouter".to_string(),
            has_secret: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn connection() -> Connection {
        connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("example/model", true)],
        )
    }

    fn target_for(path: &Path) -> ClientTarget {
        ClientTarget {
            id: "vscode:stable:default".to_string(),
            kind: ClientKind::VsCodeCopilot,
            name: "Visual Studio Code · Default".to_string(),
            detail: "test target".to_string(),
            path: Some(path.to_string_lossy().to_string()),
            detected: true,
            supports_write: true,
            status: ClientStatus::Available,
            diagnostic: None,
        }
    }

    fn read_config(path: &Path) -> Vec<Value> {
        let text = fs::read_to_string(path).expect("config file");
        serde_json::from_str(&text).expect("config must be valid JSON after a write")
    }

    fn managed_group<'a>(groups: &'a [Value], connection_id: &str) -> &'a Value {
        groups
            .iter()
            .find(|group| is_owned_group(group, connection_id))
            .expect("a group owned by the connection")
    }

    #[test]
    fn replaces_only_the_same_connection_group() {
        let connection = connection();
        let mut groups = vec![
            json!({"name":"User","vendor":"customendpoint"}),
            json!({"name":"Old","vendor":"customendpoint", "pilotWeaveManaged":true, "pilotWeaveConnectionId":"openrouter"}),
            json!({"name":"Other","vendor":"customendpoint", "pilotWeaveManaged":true, "pilotWeaveConnectionId":"other"}),
        ];
        groups.retain(|group| !is_owned_group(group, &connection.id));
        groups.push(render_group(&connection, Some("secret")));

        assert_eq!(groups.len(), 3);
        assert!(groups.iter().any(|value| value["name"] == "User"));
        assert!(groups
            .iter()
            .any(|value| value[CONNECTION_ID_FIELD] == "other"));
        assert!(groups
            .iter()
            .any(|value| value[CONNECTION_ID_FIELD] == "openrouter"));
    }

    #[test]
    fn apply_publishes_enabled_models_to_a_fresh_config() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        let connection = connection();

        apply(&connection, Some("secret"), &target_for(&path)).expect("apply");

        let groups = read_config(&path);
        assert_eq!(groups.len(), 1);
        let group = managed_group(&groups, "openrouter");
        assert_eq!(group["name"], "OpenRouter");
        assert_eq!(group["vendor"], "customendpoint");
        assert_eq!(group["apiType"], "chat-completions");
        assert_eq!(group["apiKey"], "secret");

        let models = group["models"].as_array().expect("models array");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "example/model");
        assert_eq!(models[0]["name"], "Model example/model");
        assert_eq!(
            models[0]["url"],
            "https://openrouter.example/v1/chat/completions"
        );
        assert_eq!(
            models[0]["requestHeaders"]["Authorization"],
            "Bearer secret"
        );

        // Nothing existed before, so there is nothing to roll back to.
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn apply_switches_models_and_preserves_foreign_groups() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        let seed =
            r#"[{"name":"User Group","vendor":"customendpoint","models":[{"id":"user/model"}]}]"#;
        fs::write(&path, seed).expect("seed config");
        let target = target_for(&path);

        // First activation publishes model-a next to the user's group.
        let first = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("upstream/model-a", true)],
        );
        apply(&first, Some("secret"), &target).expect("first apply");
        let groups = read_config(&path);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            managed_group(&groups, "openrouter")["models"][0]["id"],
            "upstream/model-a"
        );

        // The backup captured the pre-PilotWeave file exactly once.
        let backup = fs::read_to_string(backup_path(&path)).expect("backup after first write");
        assert_eq!(backup, seed);

        // Switching the connection to model-b replaces only the managed group.
        let switched = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("upstream/model-b", true)],
        );
        apply(&switched, Some("secret"), &target).expect("second apply");
        let groups = read_config(&path);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|group| group["name"] == "User Group"));
        let managed = managed_group(&groups, "openrouter");
        assert_eq!(managed["models"].as_array().expect("models").len(), 1);
        assert_eq!(managed["models"][0]["id"], "upstream/model-b");
        let rendered = serde_json::to_string(&groups).expect("serialize groups");
        assert!(!rendered.contains("upstream/model-a"));

        // The rollback backup still holds the original user configuration.
        let backup = fs::read_to_string(backup_path(&path)).expect("backup after second write");
        assert_eq!(backup, seed);
    }

    #[test]
    fn apply_without_enabled_models_removes_the_managed_group() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        fs::write(
            &path,
            r#"[{"name":"User Group","vendor":"customendpoint"}]"#,
        )
        .expect("seed config");
        let target = target_for(&path);

        apply(&connection(), Some("secret"), &target).expect("first apply");
        assert_eq!(read_config(&path).len(), 2);

        let disabled = connection_with(
            ProviderKind::Openai,
            ApiProtocol::ChatCompletions,
            vec![model("example/model", false)],
        );
        apply(&disabled, Some("secret"), &target).expect("second apply");

        let groups = read_config(&path);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["name"], "User Group");
    }

    #[test]
    fn apply_materializes_anthropic_headers() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        let connection = connection_with(
            ProviderKind::Anthropic,
            ApiProtocol::Messages,
            vec![model("claude-example", true)],
        );

        apply(&connection, Some("sk-ant"), &target_for(&path)).expect("apply");

        let groups = read_config(&path);
        let group = managed_group(&groups, "openrouter");
        assert_eq!(group["apiType"], "messages");
        let headers = &group["models"][0]["requestHeaders"];
        assert_eq!(headers["x-api-key"], "sk-ant");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
    }

    #[test]
    fn apply_without_a_secret_writes_no_authorization_header() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);

        apply(&connection(), None, &target_for(&path)).expect("apply");

        let groups = read_config(&path);
        let group = managed_group(&groups, "openrouter");
        assert_eq!(group["apiKey"], "");
        assert!(group["models"][0].get("requestHeaders").is_none());
    }

    #[test]
    fn apply_tolerates_json5_comments_in_existing_config() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        fs::write(
            &path,
            "[\n  // user-owned group\n  { name: 'User Group', vendor: 'customendpoint', },\n]\n",
        )
        .expect("seed json5 config");

        apply(&connection(), Some("secret"), &target_for(&path)).expect("apply");

        let groups = read_config(&path);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|group| group["name"] == "User Group"));
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_a_symlinked_config() {
        let directory = tempfile::tempdir().expect("temp directory");
        let real = directory.path().join("real.json");
        fs::write(&real, "[]").expect("real file");
        let path = directory.path().join(LANGUAGE_MODELS_FILE);
        std::os::unix::fs::symlink(&real, &path).expect("symlink");

        let error = apply(&connection(), Some("secret"), &target_for(&path))
            .expect_err("must refuse symlink");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }
}
