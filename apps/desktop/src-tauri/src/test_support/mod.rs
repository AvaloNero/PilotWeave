//! Compiled only with `local-e2e`. No production IPC or environment toggle.
//! All external dependencies fail closed into one explicitly owned fixture root.
use crate::{
    error::{AppError, AppResult},
    safe_io,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

static CONTEXT: OnceLock<Context> = OnceLock::new();
struct Context {
    root: PathBuf,
    port: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    version: u32,
    run_id: String,
    http_port: u16,
}

pub fn root() -> Option<&'static Path> {
    CONTEXT.get().map(|c| c.root.as_path())
}
pub fn active() -> bool {
    root().is_some()
}
fn invalid() -> AppError {
    AppError::InvalidInput("Invalid isolated validation context".into())
}

pub fn initialize() -> AppResult<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let path = match args.as_slice() {
        [flag, path] if flag == "--local-e2e-root" => PathBuf::from(path),
        [argument] => argument
            .to_str()
            .and_then(|v| v.strip_prefix("--local-e2e-root="))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    validate_root(&path)?;
    let bytes = safe_io::read_optional(&path.join("context.json"), 2048)?.ok_or_else(invalid)?;
    let m: Manifest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if m.version != 1
        || m.http_port == 0
        || uuid::Uuid::parse_str(&m.run_id).is_err()
        || path.file_name().and_then(|v| v.to_str())
            != Some(format!("sandbox-{}", m.run_id).as_str())
    {
        return Err(invalid());
    }
    CONTEXT
        .set(Context {
            root: std::fs::canonicalize(path).map_err(|_| invalid())?,
            port: m.http_port,
        })
        .map_err(|_| invalid())?;
    for name in [
        "config",
        "home",
        "private",
        "private/credentials",
        "private/registry",
        "bin",
        "webview",
    ] {
        std::fs::create_dir_all(checked(name)?).map_err(|_| invalid())?;
    }
    for name in [
        "winget.exe",
        "gh.exe",
        "copilot.exe",
        "code.exe",
        "github-copilot.exe",
    ] {
        let path = checked(&format!("bin/{name}"))?;
        if !path.exists() {
            safe_io::write_private(&path, b"PilotWeave inert local fixture executable")?;
        }
    }
    for name in ["bin/vscode/bin", "bin/vscode/resources/app/out"] {
        std::fs::create_dir_all(checked(name)?).map_err(|_| invalid())?;
    }
    for (name, bytes) in [
        ("bin/vscode/Code.exe", "PilotWeave inert VS Code fixture"),
        ("bin/vscode/resources/app/out/cli.js", "// Inert fixture, never executed"),
        ("bin/vscode/bin/code.cmd", "@echo off\nsetlocal\nset VSCODE_DEV=\nset ELECTRON_RUN_AS_NODE=1\n\"%~dp0..\\Code.exe\" \"%~dp0..\\resources\\app\\out\\cli.js\" %*\nIF %ERRORLEVEL% NEQ 0 EXIT /b %ERRORLEVEL%\nendlocal\n"),
        ("bin/vscode/resources/app/product.json", r#"{"applicationName":"code","version":"1.137.0","commit":"645f29cc3176500b4b5762ba887cf2a7f0ffdf2c"}"#),
    ] {
        let path = checked(name)?;
        if !path.exists() { safe_io::write_private(&path, bytes.as_bytes())?; }
    }
    Ok(())
}

fn validate_root(path: &Path) -> AppResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    for ancestor in path.ancestors() {
        let m = std::fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

pub fn constrain(path: &Path) -> AppResult<()> {
    if let Some(root) = root() {
        // resource_path can return a verbatim Windows canonical path.
        let normalized = path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase();
        let owned = root
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase();
        if !Path::new(&normalized).starts_with(Path::new(&owned))
            || path.components().any(|p| matches!(p, Component::ParentDir))
        {
            return Err(AppError::InvalidInput(
                "Isolated path is outside the owned root".into(),
            ));
        }
    }
    for ancestor in path.ancestors() {
        if let Ok(m) = std::fs::symlink_metadata(ancestor) {
            if m.file_type().is_symlink() {
                return Err(invalid());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}
pub fn checked(relative: &str) -> AppResult<PathBuf> {
    let path = root().ok_or_else(invalid)?.join(relative);
    constrain(&path)?;
    Ok(path)
}
pub fn credential(reference: &str, change: Option<Option<&str>>) -> AppResult<Option<String>> {
    let key = format!("{:x}", Sha256::digest(reference.as_bytes()));
    let path = checked(&format!("private/credentials/{key}"))?;
    match change {
        Some(Some(value)) => {
            safe_io::write_private(&path, value.as_bytes())?;
            Ok(None)
        }
        Some(None) => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|_| invalid())?;
            }
            Ok(None)
        }
        None => safe_io::read_optional(&path, 16 * 1024)?
            .map(|b| String::from_utf8(b).map_err(|_| invalid()))
            .transpose(),
    }
}
pub fn registry(name: &str, change: Option<Option<&[u8]>>) -> AppResult<Option<Vec<u8>>> {
    if !name.starts_with("COPILOT_") || !name.bytes().all(|c| c.is_ascii_uppercase() || c == b'_') {
        return Err(invalid());
    }
    let path = checked(&format!("private/registry/{name}"))?;
    match change {
        Some(Some(value)) => {
            safe_io::write_private(&path, value)?;
            Ok(None)
        }
        Some(None) => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|_| invalid())?;
            }
            Ok(None)
        }
        None => safe_io::read_optional(&path, 128 * 1024),
    }
}
pub fn installed(name: &str) -> bool {
    checked(&format!("private/installed-{name}")).is_ok_and(|p| p.is_file())
}
pub fn executable(name: &str) -> Option<PathBuf> {
    let file = match name.to_ascii_lowercase().as_str() {
        "winget.exe" => "winget.exe",
        "gh.exe" | "gh" => "gh.exe",
        "copilot.exe" | "copilot" | "copilot.cmd" if installed("copilot-cli") => "copilot.exe",
        "code.exe" | "code" if installed("vscode") => "vscode/Code.exe",
        "github-copilot.exe" if installed("copilot-app") => "github-copilot.exe",
        _ => return None,
    };
    checked(&format!("bin/{file}")).ok()
}
pub fn process(
    executable: &Path,
    args: &[&std::ffi::OsStr],
    mode: crate::native_process::CaptureMode,
) -> AppResult<crate::native_process::CapturedOutput> {
    constrain(executable)?;
    let args: Vec<_> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    #[cfg(windows)]
    let args = {
        let mut args = args;
        if executable.file_name().is_some_and(|n| n == "Code.exe") {
            if mode != crate::native_process::CaptureMode::VsCodeCli
                || args.first().map(Path::new)
                    != Some(checked("bin/vscode/resources/app/out/cli.js")?.as_path())
            {
                return Err(invalid());
            }
            args.remove(0);
        }
        args
    };
    #[cfg(not(windows))]
    let _ = mode;
    let mut stdout = String::new();
    let mut code = 0;
    if args.first().map(String::as_str) == Some("install") {
        let name = match args.get(2).map(String::as_str) {
            Some("Microsoft.VisualStudioCode") => "vscode",
            Some("GitHub.Copilot") => "copilot-cli",
            Some("GitHub.CopilotApp") => "copilot-app",
            _ => return Err(invalid()),
        };
        fault("installer")?;
        if !installed("false-success") {
            safe_io::write_private(&checked(&format!("private/installed-{name}"))?, b"1")?;
        }
        if name == "vscode" {
            std::fs::create_dir_all(root().unwrap().join("config/Code/User"))
                .map_err(|_| invalid())?;
        }
    } else if args.first().map(String::as_str) == Some("--install-extension") {
        if args.get(1).map(String::as_str) != Some("GitHub.copilot-chat") {
            return Err(invalid());
        }
        safe_io::write_private(&checked("private/installed-vscode-copilot")?, b"1")?;
    } else if args.first().map(String::as_str) == Some("--list-extensions") {
        if installed("vscode-probe-error") {
            return Err(invalid());
        }
        if installed("vscode-copilot") {
            stdout = "GitHub.copilot-chat@0.65.0\n".into();
        }
    } else if args.first().map(String::as_str) == Some("api") {
        if installed("signed-in") {
            stdout = r#"{"login":"fixture-user","id":42}"#.into();
        } else {
            code = 1;
        }
    } else {
        return Err(invalid());
    }
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code)
    };
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code as i32 * 256)
    };
    Ok(crate::native_process::CapturedOutput {
        status,
        stdout,
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    })
}
pub fn endpoint(url: &str) -> String {
    let suffix = if let Some(path) = url.strip_prefix("https://api.github.com/") {
        format!("/github/{path}")
    } else if url == "https://openrouter.ai/api/v1/models" {
        "/prices".into()
    } else {
        "/rejected".into()
    };
    format!(
        "http://127.0.0.1:{}{suffix}",
        CONTEXT.get().expect("isolated context").port
    )
}
pub struct Rpc;
impl crate::usage::runtime::ReadOnlyRpc for Rpc {
    fn request(
        &mut self,
        method: &'static str,
        _params: serde_json::Value,
    ) -> crate::usage::runtime::RpcResult<serde_json::Value> {
        use crate::usage::{runtime::RuntimeError, types::DataStatus};
        if !matches!(
            method,
            "connect" | "auth.getStatus" | "account.getQuota" | "models.list"
        ) {
            return Err(RuntimeError(
                DataStatus::Unsupported,
                "Unexpected fixture RPC",
            ));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(std::time::Duration::from_secs(5)))
            .build()
            .into();
        agent
            .get(format!(
                "http://127.0.0.1:{}/rpc/{method}",
                CONTEXT.get().unwrap().port
            ))
            .call()
            .map_err(|_| RuntimeError(DataStatus::NetworkError, "Fixture RPC unavailable"))?
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_json()
            .map_err(|_| RuntimeError(DataStatus::SchemaError, "Fixture RPC schema rejected"))
    }
}

/// One-shot, named, native-owned fault. Never accepts paths or executable input.
pub fn fault(point: &str) -> AppResult<()> {
    if !active() {
        return Ok(());
    }
    let path = checked("fault.json")?;
    let Some(bytes) = safe_io::read_optional(&path, 256)? else {
        return Ok(());
    };
    let name: String = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if name == format!("fail:{point}") || name == format!("crash:{point}") {
        std::fs::remove_file(path).map_err(|_| invalid())?;
        if name.starts_with("crash:") {
            std::process::exit(86);
        }
        return Err(AppError::Config(
            "Injected isolated validation failure".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_rejects_relative_parent_and_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_root(Path::new("relative")).is_err());
        assert!(validate_root(&dir.path().join("..")).is_err());
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(validate_root(&file).is_err());
        assert!(validate_root(dir.path()).is_ok());
    }
}
