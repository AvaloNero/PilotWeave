//! Windows installation metadata and noninteractive VS Code CLI operations.
//! Wrapper schema v1: legacy resources layout or a 10/40-hex commit directory.
//! The wrapper is parsed as data and never executed. Unknown layouts fail closed.
use crate::error::{AppError, AppResult};
use crate::native_process::{self, CaptureMode, CapturedOutput};
use crate::safe_file;
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub const INSTALL_EXTENSION_ID: &str = "GitHub.copilot-chat";
const EXTENSION_IDS: [&str; 2] = [INSTALL_EXTENSION_ID, "GitHub.copilot"];
const METADATA_LIMIT: u64 = 2 * 1024 * 1024;

pub struct VsCodeCli {
    pub executable: PathBuf,
    entry: PathBuf,
    app: PathBuf,
    pub version: String,
    insiders: bool,
}

pub struct InstalledCopilot {
    pub detail: String,
    pub version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Product {
    application_name: String,
    version: String,
    commit: String,
}

#[derive(Deserialize)]
struct Extension {
    publisher: String,
    name: String,
    version: String,
}

fn unsupported() -> AppError {
    AppError::Unsupported("VS Code installation metadata or CLI layout is unsupported; rescan after its update finishes".into())
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
}

fn is_copilot(id: &str) -> bool {
    EXTENSION_IDS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(id))
}

fn installed(id: &str, version: &str, scope: &str) -> AppResult<InstalledCopilot> {
    if !is_copilot(id) || !valid_version(version) {
        return Err(unsupported());
    }
    Ok(InstalledCopilot {
        detail: format!(
            "{id} {version} is installed ({scope}); enablement and sign-in are not verified"
        ),
        version: version.into(),
    })
}

impl VsCodeCli {
    pub fn resolve(executable: &Path) -> AppResult<Self> {
        safe_file::ensure_regular_or_missing(executable)?;
        if !executable.is_file() {
            return Err(unsupported());
        }
        let root = executable.parent().ok_or_else(unsupported)?;
        let name = executable
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(unsupported)?;
        let insiders = name.eq_ignore_ascii_case("Code - Insiders.exe");
        if !insiders && !name.eq_ignore_ascii_case("Code.exe") {
            return Err(unsupported());
        }
        let wrapper = root.join(if insiders {
            "bin/code-insiders.cmd"
        } else {
            "bin/code.cmd"
        });
        let bytes = safe_file::read_optional(&wrapper, 4096)?.ok_or_else(unsupported)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| unsupported())?;
        let relative = wrapper_entry(text, name)?;
        let entry = root.join(&relative);
        safe_file::ensure_regular_or_missing(&entry)?;
        if !entry.is_file() {
            return Err(unsupported());
        }
        let app = entry
            .parent()
            .and_then(Path::parent)
            .ok_or_else(unsupported)?
            .to_path_buf();
        let bytes = safe_file::read_optional(&app.join("product.json"), METADATA_LIMIT)?
            .ok_or_else(unsupported)?;
        let product: Product = serde_json::from_slice(&bytes).map_err(|_| unsupported())?;
        let expected = if insiders { "code-insiders" } else { "code" };
        if product.application_name != expected
            || !valid_version(&product.version)
            || product.commit.len() != 40
            || !product.commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(unsupported());
        }
        let first = relative.components().next().ok_or_else(unsupported)?;
        if let Component::Normal(prefix) = first {
            let prefix = prefix.to_str().ok_or_else(unsupported)?;
            if prefix != "resources" && !product.commit.starts_with(prefix) {
                return Err(unsupported());
            }
        }
        Ok(Self {
            executable: executable.into(),
            entry,
            app,
            version: product.version,
            insiders,
        })
    }

    pub fn args(&self, args: &[&OsStr]) -> Vec<OsString> {
        std::iter::once(self.entry.clone().into_os_string())
            .chain(args.iter().map(|a| a.to_os_string()))
            .collect()
    }

    pub fn copilot(&self) -> AppResult<Option<InstalledCopilot>> {
        match self.observe_with(|args| {
            native_process::run_capture_with_mode(
                &self.executable,
                args,
                Duration::from_secs(15),
                128 * 1024,
                CaptureMode::VsCodeCli,
            )
        })? {
            Some(extension) => Ok(Some(extension)),
            None => self.named_profile_copilot(),
        }
    }

    fn observe_with(
        &self,
        capture: impl FnOnce(&[&OsStr]) -> AppResult<CapturedOutput>,
    ) -> AppResult<Option<InstalledCopilot>> {
        // Current distributions bundle Copilot, which --list-extensions omits.
        for folder in [
            "copilot",
            "copilot-chat",
            "github.copilot-chat",
            "github.copilot",
        ] {
            if let Some(bytes) = safe_file::read_optional(
                &self
                    .app
                    .join("extensions")
                    .join(folder)
                    .join("package.json"),
                METADATA_LIMIT,
            )? {
                let extension: Extension =
                    serde_json::from_slice(&bytes).map_err(|_| unsupported())?;
                let id = format!("{}.{}", extension.publisher, extension.name);
                return installed(&id, &extension.version, "bundled with VS Code").map(Some);
            }
        }
        let args = self.args(&[
            OsStr::new("--list-extensions"),
            OsStr::new("--show-versions"),
        ]);
        let result = capture(&args.iter().map(OsString::as_os_str).collect::<Vec<_>>())?;
        if !result.status.success() || result.stdout_truncated || result.stderr_truncated {
            return Err(AppError::Config("VS Code extension probe failed or exceeded its output limit; installation status is unknown".into()));
        }
        parse_extension_list(&result.stdout)
    }

    fn named_profile_copilot(&self) -> AppResult<Option<InstalledCopilot>> {
        #[cfg(feature = "local-e2e")]
        if crate::test_support::active() {
            return Ok(None);
        }
        let root = self.executable.parent().ok_or_else(unsupported)?;
        let portable = root.join("data/user-data/User");
        let user = if root.join("data").is_dir() {
            portable
        } else {
            PathBuf::from(std::env::var_os("APPDATA").ok_or_else(unsupported)?).join(
                if self.insiders {
                    "Code - Insiders/User"
                } else {
                    "Code/User"
                },
            )
        };
        named_profile_copilot(&user)
    }
}

fn wrapper_entry(text: &str, executable_name: &str) -> AppResult<PathBuf> {
    let lines: Vec<_> = text.lines().map(str::trim).collect();
    if lines.len() != 7 {
        return Err(unsupported());
    }
    for (index, expected) in [
        (0, "@echo off"),
        (1, "setlocal"),
        (2, "set VSCODE_DEV="),
        (3, "set ELECTRON_RUN_AS_NODE=1"),
        (5, "IF %ERRORLEVEL% NEQ 0 EXIT /b %ERRORLEVEL%"),
        (6, "endlocal"),
    ] {
        if !lines[index].eq_ignore_ascii_case(expected) {
            return Err(unsupported());
        }
    }
    let prefix = format!("\"%~dp0..\\{executable_name}\" \"%~dp0..\\");
    if !lines[4]
        .get(..prefix.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(&prefix))
    {
        return Err(unsupported());
    }
    let relative = lines[4][prefix.len()..]
        .strip_suffix("\" %*")
        .ok_or_else(unsupported)?;
    let relative = relative.replace('\\', "/");
    if relative != "resources/app/out/cli.js" {
        let prefix = relative
            .strip_suffix("/resources/app/out/cli.js")
            .ok_or_else(unsupported)?;
        if ![10, 40].contains(&prefix.len()) || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(unsupported());
        }
    }
    Ok(PathBuf::from(relative))
}

fn parse_extension_list(text: &str) -> AppResult<Option<InstalledCopilot>> {
    let mut found = None;
    for (index, line) in text.lines().enumerate() {
        if index >= 2048 || line.len() > 256 {
            return Err(unsupported());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (id, version) = line.split_once('@').ok_or_else(unsupported)?;
        if !id.contains('.')
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            || !valid_version(version)
        {
            return Err(unsupported());
        }
        if is_copilot(id) {
            found = Some(installed(id, version, "default profile")?);
        }
    }
    Ok(found)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profiles {
    #[serde(default)]
    user_data_profiles: Vec<Profile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    location: String,
    #[serde(default)]
    use_default_flags: ProfileDefaults,
}
#[derive(Default, Deserialize)]
struct ProfileDefaults {
    #[serde(default)]
    extensions: bool,
}
#[derive(Deserialize)]
struct ProfileExtension {
    identifier: Identifier,
    version: String,
}
#[derive(Deserialize)]
struct Identifier {
    id: String,
}

fn named_profile_copilot(user: &Path) -> AppResult<Option<InstalledCopilot>> {
    let Some(bytes) =
        safe_file::read_optional(&user.join("globalStorage/storage.json"), METADATA_LIMIT)?
    else {
        return Ok(None);
    };
    let profiles: Profiles = serde_json::from_slice(&bytes).map_err(|_| unsupported())?;
    if profiles.user_data_profiles.len() > 64 {
        return Err(unsupported());
    }
    for profile in profiles.user_data_profiles {
        let relative = Path::new(&profile.location);
        if profile.location.len() > 256
            || relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(unsupported());
        }
        let path = user.join("profiles").join(relative).join("extensions.json");
        if let Some(bytes) = safe_file::read_optional(&path, METADATA_LIMIT)? {
            let entries: Vec<ProfileExtension> =
                serde_json::from_slice(&bytes).map_err(|_| unsupported())?;
            if entries.len() > 2048 {
                return Err(unsupported());
            }
            for entry in entries {
                if is_copilot(&entry.identifier.id) {
                    return installed(
                        &entry.identifier.id,
                        &entry.version,
                        "a registered named profile",
                    )
                    .map(Some);
                }
            }
        } else if !profile.use_default_flags.extensions {
            return Err(unsupported());
        }
    }
    Ok(None)
}

pub fn find_executable() -> Option<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::executable("code.exe");
    }
    let mut candidates = Vec::new();
    for (folder, exe) in [
        ("Microsoft VS Code", "Code.exe"),
        ("Microsoft VS Code Insiders", "Code - Insiders.exe"),
    ] {
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(root).join("Programs").join(folder).join(exe));
        }
        for key in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(root) = std::env::var_os(key) {
                candidates.push(PathBuf::from(root).join(folder).join(exe));
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for root in std::env::split_paths(&paths).filter(|p| p.is_absolute()) {
            for name in ["Code.exe", "Code - Insiders.exe"] {
                candidates.push(root.join(name));
            }
            if root
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
            {
                if let Some(parent) = root.parent() {
                    for name in ["Code.exe", "Code - Insiders.exe"] {
                        candidates.push(parent.join(name));
                    }
                }
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::process::ExitStatusExt;
    use tempfile::TempDir;

    const COMMIT: &str = "645f29cc3176500b4b5762ba887cf2a7f0ffdf2c";
    const BUNDLED: &str = include_str!("../../tests/fixtures/vscode/bundled-copilot-v1.json");
    const WRAPPER: &str = include_str!("../../tests/fixtures/vscode/code-versioned-v1.cmd");

    fn write(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn fixture(versioned: bool) -> (TempDir, VsCodeCli) {
        let dir = TempDir::new().unwrap();
        let exe = dir.path().join("Code.exe");
        write(&exe, "inert fixture");
        let prefix = if versioned { "645f29cc31/" } else { "" };
        let wrapper = if versioned {
            WRAPPER.into()
        } else {
            WRAPPER.replace("645f29cc31\\", "")
        };
        write(&dir.path().join("bin/code.cmd"), wrapper);
        let app = dir.path().join(format!("{prefix}resources/app"));
        write(&app.join("out/cli.js"), "// never executed");
        write(
            &app.join("product.json"),
            format!(r#"{{"applicationName":"code","version":"1.137.0","commit":"{COMMIT}"}}"#),
        );
        let cli = VsCodeCli::resolve(&exe).unwrap();
        (dir, cli)
    }
    fn output(text: &str) -> CapturedOutput {
        CapturedOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: text.into(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    #[test]
    fn legacy_and_versioned_cli_use_the_script_before_flags() {
        for versioned in [false, true] {
            let (_dir, cli) = fixture(versioned);
            let observed = cli
                .observe_with(|args| {
                    assert_eq!(args[0], cli.entry.as_os_str());
                    assert_eq!(
                        &args[1..],
                        &[
                            OsStr::new("--list-extensions"),
                            OsStr::new("--show-versions")
                        ]
                    );
                    Ok(output("GitHub.copilot-chat@0.65.0\n"))
                })
                .unwrap()
                .unwrap();
            assert_eq!(observed.version, "0.65.0");
            let install = cli.args(&[
                OsStr::new("--install-extension"),
                OsStr::new(INSTALL_EXTENSION_ID),
            ]);
            assert_eq!(install[0], cli.entry);
            assert_eq!(install[2], "GitHub.copilot-chat");
        }
    }

    #[test]
    fn bundled_copilot_is_detected_without_starting_any_process() {
        let (_dir, cli) = fixture(true);
        write(&cli.app.join("extensions/copilot/package.json"), BUNDLED);
        let found = cli
            .observe_with(|_| panic!("Bundled capability must not launch Code.exe"))
            .unwrap()
            .unwrap();
        assert_eq!(found.version, "0.65.0");
        assert!(found.detail.contains("bundled with VS Code"));
    }

    #[test]
    fn probe_failure_truncation_or_unknown_schema_is_never_missing() {
        let (_dir, cli) = fixture(false);
        assert!(cli.observe_with(|_| Err(unsupported())).is_err());
        for result in [
            CapturedOutput {
                status: std::process::ExitStatus::from_raw(1),
                ..output("")
            },
            CapturedOutput {
                stdout_truncated: true,
                ..output("")
            },
            CapturedOutput {
                stderr_truncated: true,
                ..output("")
            },
            output("unrecognized status output"),
        ] {
            assert!(cli.observe_with(|_| Ok(result)).is_err());
        }
        assert!(cli
            .observe_with(|_| Ok(output("other.extension@1.0.0\n")))
            .unwrap()
            .is_none());
        for id in EXTENSION_IDS {
            assert!(parse_extension_list(&format!("{id}@1.2.3\n"))
                .unwrap()
                .is_some());
        }
        assert!(parse_extension_list("attacker.copilot-chat@1.0.0")
            .unwrap()
            .is_none());
    }

    #[test]
    fn rejects_wrapper_commands_traversal_unknown_layout_and_wrong_commit() {
        for text in [
            WRAPPER.replace("645f29cc31", "..\\evil"),
            WRAPPER.replace("645f29cc31", "new-layout"),
            WRAPPER.replace(" %*", " %* & evil.exe"),
            WRAPPER.replace("setlocal", "evil.exe"),
            WRAPPER.replace("cli.js", "other.js"),
        ] {
            assert!(wrapper_entry(&text, "Code.exe").is_err());
        }
        let (dir, cli) = fixture(true);
        write(
            &cli.app.join("product.json"),
            r#"{"applicationName":"code","version":"1.137.0","commit":"0000000000000000000000000000000000000000"}"#,
        );
        assert!(VsCodeCli::resolve(&dir.path().join("Code.exe")).is_err());
        fs::remove_file(&cli.entry).unwrap();
        assert!(VsCodeCli::resolve(&dir.path().join("Code.exe")).is_err());
    }

    #[test]
    fn metadata_is_bounded_and_links_are_rejected() {
        let (dir, cli) = fixture(false);
        let manifest = cli.app.join("extensions/copilot/package.json");
        write(&manifest, vec![b' '; METADATA_LIMIT as usize + 1]);
        assert!(cli.observe_with(|_| panic!("invalid metadata")).is_err());
        fs::remove_file(&manifest).unwrap();
        // Junction creation is unprivileged on Windows and must fail closed.
        let outside = dir.path().join("outside");
        write(&outside.join("package.json"), BUNDLED);
        fs::remove_dir(manifest.parent().unwrap()).unwrap();
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:PILOTWEAVE_LINK_PATH -Target $env:PILOTWEAVE_LINK_TARGET | Out-Null"])
            .env("PILOTWEAVE_LINK_PATH", manifest.parent().unwrap())
            .env("PILOTWEAVE_LINK_TARGET", &outside)
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .status().unwrap();
        assert!(status.success());
        assert!(cli.observe_with(|_| panic!("reparse metadata")).is_err());
        fs::remove_dir(manifest.parent().unwrap()).unwrap();
    }

    #[test]
    fn named_profile_only_installation_and_unknown_profiles_remain_distinct() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join("globalStorage/storage.json"),
            r#"{"userDataProfiles":[{"location":"work"}]}"#,
        );
        assert!(named_profile_copilot(root).is_err());
        write(
            &root.join("profiles/work/extensions.json"),
            r#"[{"identifier":{"id":"GitHub.copilot-chat"},"version":"0.65.0"}]"#,
        );
        assert_eq!(
            named_profile_copilot(root).unwrap().unwrap().version,
            "0.65.0"
        );
        write(
            &root.join("globalStorage/storage.json"),
            r#"{"userDataProfiles":[{"location":"../outside"}]}"#,
        );
        assert!(named_profile_copilot(root).is_err());
        write(
            &root.join("globalStorage/storage.json"),
            r#"{"userDataProfiles":[{"location":"builtin/agents","useDefaultFlags":{"extensions":true}}]}"#,
        );
        assert!(named_profile_copilot(root).unwrap().is_none());
    }
}
