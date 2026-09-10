use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;

pub const SENSITIVE_CHILD_ENV: &[&str] = &[
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_HEADERS",
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_TYPE",
    "COPILOT_PROVIDER_WIRE_API",
    "COPILOT_PROVIDER_MODEL_ID",
    "COPILOT_PROVIDER_WIRE_MODEL",
    "COPILOT_PROVIDER_TRANSPORT",
    "COPILOT_MODEL",
    "COPILOT_OFFLINE",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GH_DEBUG",
    "NODE_OPTIONS",
    "ELECTRON_RUN_AS_NODE",
    "VSCODE_DEV",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureMode {
    Standard,
    #[cfg(windows)]
    VsCodeCli,
}

#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub fn resolve_on_path(names: &[&str]) -> Option<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return names
            .iter()
            .find_map(|name| crate::test_support::executable(name));
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find_map(resolve_regular_file)
}

pub fn resolve_candidates<I, P>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    candidates.into_iter().find_map(resolve_regular_file)
}

pub fn resolve_regular_file(path: impl AsRef<Path>) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::symlink_metadata(&canonical).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink()).then_some(canonical)
}

/// Full SHA-256, not head/tail sampling. This detects replacement, not publisher identity.
pub fn fingerprint_regular_file(path: &Path) -> AppResult<String> {
    let canonical = resolve_regular_file(path)
        .ok_or_else(|| AppError::InvalidInput("Missing or non-regular executable".into()))?;
    let mut file = fs::File::open(&canonical).map_err(|error| AppError::io(&canonical, error))?;
    let before = file
        .metadata()
        .map_err(|error| AppError::io(&canonical, error))?;
    if before.len() > MAX_EXECUTABLE_BYTES {
        return Err(AppError::InvalidInput(
            "Executable exceeds fingerprint size limit".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(canonical.to_string_lossy().as_bytes());
    let mut bytes = [0; 16384];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut bytes)
            .map_err(|error| AppError::io(&canonical, error))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_EXECUTABLE_BYTES {
            return Err(AppError::InvalidInput(
                "Executable grew past fingerprint limit".into(),
            ));
        }
        digest.update(&bytes[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| AppError::io(&canonical, error))?;
    if before.len() != total
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(AppError::Config(
            "Executable changed while fingerprinting".into(),
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn run_capture_bounded(
    executable: &Path,
    args: &[&OsStr],
    timeout: Duration,
    max_output_bytes: usize,
) -> AppResult<CapturedOutput> {
    run_capture_with_mode(
        executable,
        args,
        timeout,
        max_output_bytes,
        CaptureMode::Standard,
    )
}

pub(crate) fn run_capture_with_mode(
    executable: &Path,
    args: &[&OsStr],
    timeout: Duration,
    max_output_bytes: usize,
    mode: CaptureMode,
) -> AppResult<CapturedOutput> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::process(executable, args, mode);
    }

    if timeout.is_zero() || max_output_bytes == 0 || max_output_bytes > MAX_CAPTURE_BYTES {
        return Err(AppError::InvalidInput(
            "Invalid process timeout or capture limit".into(),
        ));
    }
    let executable = resolve_regular_file(executable)
        .ok_or_else(|| AppError::InvalidInput("Missing or non-regular executable".into()))?;
    let mut command = Command::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_capture(&mut command, mode);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| AppError::io(&executable, error))?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Config("Missing stdout pipe".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Config("Missing stderr pipe".into()))?;
    let out = start_reader(stdout, max_output_bytes);
    let err = start_reader(stderr, max_output_bytes);
    let deadline = Instant::now() + timeout;
    let result = (|| -> AppResult<CapturedOutput> {
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| AppError::io(&executable, error))?
            {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(timeout_error());
            }
            thread::sleep(Duration::from_millis(20));
        };
        // The same deadline includes pipe drains. Descendants cannot turn an
        // exited parent into an unbounded reader-thread join.
        let (stdout, stdout_truncated) = receive_reader(out, deadline)?;
        let (stderr, stderr_truncated) = receive_reader(err, deadline)?;
        Ok(CapturedOutput {
            status,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            stdout_truncated,
            stderr_truncated,
        })
    })();
    drop(tree); // terminate any remaining members, including inherited-pipe holders
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

fn configure_capture(command: &mut Command, mode: CaptureMode) {
    sanitize_child_environment(command, false);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
        if mode == CaptureMode::VsCodeCli {
            // Mirror the official code.cmd without running a shell or changing
            // the parent's environment. Code.exe alone is the GUI entry point.
            command.env("ELECTRON_RUN_AS_NODE", "1");
        }
    }
    #[cfg(not(windows))]
    let _ = mode;
}

pub fn spawn_detached(executable: &Path, args: &[&OsStr]) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        crate::test_support::constrain(executable)?;
        return crate::test_support::fault("login");
    }

    let executable = resolve_regular_file(executable)
        .ok_or_else(|| AppError::InvalidInput("Missing or non-regular executable".into()))?;
    let mut command = Command::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sanitize_child_environment(&mut command, true);
    let mut child = command
        .spawn()
        .map_err(|error| AppError::io(&executable, error))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(crate) fn sanitize_child_environment(command: &mut Command, interactive: bool) {
    for name in SENSITIVE_CHILD_ENV {
        command.env_remove(name);
    }
    if interactive {
        command.env_remove("GH_PROMPT_DISABLED");
    } else {
        command.env("GH_PROMPT_DISABLED", "1");
    }
    command.env("NO_COLOR", "1");
}

type ReaderResult = AppResult<(Vec<u8>, bool)>;
fn start_reader(reader: impl Read + Send + 'static, limit: usize) -> Receiver<ReaderResult> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(drain_capped(reader, limit));
    });
    receiver
}

fn receive_reader(receiver: Receiver<ReaderResult>, deadline: Instant) -> ReaderResult {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| timeout_error())?
}

fn timeout_error() -> AppError {
    AppError::Config("Native process or its output pipes exceeded the time limit".into())
}

fn drain_capped(mut reader: impl Read, limit: usize) -> ReaderResult {
    let mut retained = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| AppError::Config("Cannot read native process output".into()))?;
        if read == 0 {
            break;
        }
        let keep = read.min(limit.saturating_sub(retained.len()));
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

#[cfg(unix)]
struct ProcessTree {
    pid: i32,
}
#[cfg(unix)]
impl ProcessTree {
    fn attach(child: &Child) -> AppResult<Self> {
        Ok(Self {
            pid: child.id() as i32,
        })
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        unsafe {
            kill(-self.pid, 9);
        }
    }
}

#[cfg(windows)]
struct ProcessTree {
    handle: windows_sys::Win32::Foundation::HANDLE,
}
#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &Child) -> AppResult<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(AppError::Config("Cannot create child process job".into()));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            if configured == 0 || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0 {
                CloseHandle(handle);
                return Err(AppError::Config(
                    "Cannot confine the child process tree".into(),
                ));
            }
            Ok(Self { handle })
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn electron_mode_is_child_scoped_and_graphical_login_clears_it() {
        let mut capture = Command::new("not-executed");
        configure_capture(&mut capture, CaptureMode::VsCodeCli);
        let env: std::collections::BTreeMap<_, _> = capture.get_envs().collect();
        assert_eq!(
            env[OsStr::new("ELECTRON_RUN_AS_NODE")],
            Some(OsStr::new("1"))
        );
        assert_eq!(env[OsStr::new("VSCODE_DEV")], None);
        assert_eq!(env[OsStr::new("NODE_OPTIONS")], None);
        let mut login = Command::new("not-executed");
        sanitize_child_environment(&mut login, true);
        let env: std::collections::BTreeMap<_, _> = login.get_envs().collect();
        assert_eq!(env[OsStr::new("ELECTRON_RUN_AS_NODE")], None);
    }
    use std::io::Cursor;

    #[test]
    fn bounded_reader_drains_but_only_keeps_prefix() {
        let (bytes, truncated) = drain_capped(Cursor::new(b"abcdefgh"), 4).expect("read");
        assert_eq!(bytes, b"abcd");
        assert!(truncated);
    }

    #[test]
    fn full_fingerprint_detects_middle_only_change() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("program");
        let mut bytes = vec![0u8; 300000];
        fs::write(&path, &bytes).expect("seed");
        let before = fingerprint_regular_file(&path).expect("hash");
        bytes[150000] = 1;
        fs::write(&path, bytes).expect("change");
        assert_ne!(before, fingerprint_regular_file(&path).expect("hash"));
    }

    #[test]
    fn interactive_mode_does_not_disable_official_prompts() {
        let mut command = Command::new("not-executed");
        sanitize_child_environment(&mut command, true);
        let vars = command
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(vars.get(OsStr::new("GH_PROMPT_DISABLED")), Some(&None));
        assert_eq!(
            vars.get(OsStr::new("COPILOT_PROVIDER_BEARER_TOKEN")),
            Some(&None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn descendant_holding_stdout_does_not_defeat_timeout() {
        let started = Instant::now();
        let result = run_capture_bounded(
            Path::new("/bin/sh"),
            &[OsStr::new("-c"), OsStr::new("sleep 30 & exit 0")],
            Duration::from_millis(200),
            1024,
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
