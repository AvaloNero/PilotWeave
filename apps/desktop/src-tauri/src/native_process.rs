use crate::error::{AppError, AppResult};
use std::ffi::OsStr;
use std::fs;
use std::hash::Hasher;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const READ_CHUNK_BYTES: usize = 8 * 1_024;
const EXECUTABLE_SAMPLE_BYTES: usize = 64 * 1_024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1_024 * 1_024;

/// Authentication and model-provider variables that must not leak into
/// helper processes launched for discovery or interactive sign-in.
pub const SENSITIVE_CHILD_ENV: &[&str] = &[
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_HEADERS",
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_TYPE",
    "COPILOT_PROVIDER_WIRE_API",
    "COPILOT_PROVIDER_MODEL_ID",
    "COPILOT_PROVIDER_WIRE_MODEL",
    "COPILOT_MODEL",
    "COPILOT_OFFLINE",
];

#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub fn resolve_on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in names {
            if let Some(path) = resolve_regular_file(directory.join(name)) {
                return Some(path);
            }
        }
    }
    None
}

pub fn resolve_candidates<I, P>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    candidates
        .into_iter()
        .find_map(|candidate| resolve_regular_file(candidate.as_ref()))
}

pub fn resolve_regular_file(path: impl AsRef<Path>) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::symlink_metadata(&canonical).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink()).then_some(canonical)
}

pub fn fingerprint_regular_file(path: &Path) -> AppResult<String> {
    let canonical = resolve_regular_file(path).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Expected a regular executable file: {}",
            path.display()
        ))
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| AppError::io(&canonical, error))?;
    if metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Executable exceeds the {} MiB fingerprint limit: {}",
            MAX_EXECUTABLE_BYTES / 1_024 / 1_024,
            canonical.display()
        )));
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(canonical.to_string_lossy().as_bytes());
    hasher.write_u64(metadata.len());
    if let Ok(modified) = metadata.modified() {
        if let Ok(value) = modified.duration_since(std::time::UNIX_EPOCH) {
            hasher.write_u64(value.as_secs());
            hasher.write_u32(value.subsec_nanos());
        }
    }

    let mut file = fs::File::open(&canonical).map_err(|error| AppError::io(&canonical, error))?;
    hash_sample(&mut file, &mut hasher, EXECUTABLE_SAMPLE_BYTES)?;
    if metadata.len() > EXECUTABLE_SAMPLE_BYTES as u64 {
        file.seek(SeekFrom::End(-(EXECUTABLE_SAMPLE_BYTES as i64)))
            .map_err(|error| AppError::io(&canonical, error))?;
        hash_sample(&mut file, &mut hasher, EXECUTABLE_SAMPLE_BYTES)?;
    }
    Ok(format!("{:016x}", hasher.finish()))
}

pub fn run_capture_bounded(
    executable: &Path,
    args: &[&OsStr],
    timeout: Duration,
    max_output_bytes: usize,
) -> AppResult<CapturedOutput> {
    let executable = resolve_regular_file(executable).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Refusing to execute a missing or non-regular file: {}",
            executable.display()
        ))
    })?;
    if timeout.is_zero() || max_output_bytes == 0 {
        return Err(AppError::InvalidInput(
            "Process timeout and output limit must be positive".to_string(),
        ));
    }

    let mut command = Command::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sanitize_child_environment(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| AppError::io(&executable, error))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Config("Failed to capture child stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Config("Failed to capture child stderr".to_string()))?;
    let stdout_reader = thread::spawn(move || drain_capped(stdout, max_output_bytes));
    let stderr_reader = thread::spawn(move || drain_capped(stderr, max_output_bytes));

    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| AppError::io(&executable, error))?
        {
            Some(status) => break status,
            None if started.elapsed() >= timeout => match child.kill() {
                Ok(()) => {
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(AppError::Config(format!(
                        "Process timed out after {} seconds: {}",
                        timeout.as_secs(),
                        executable.display()
                    )));
                }
                Err(kill_error) => match child
                    .try_wait()
                    .map_err(|error| AppError::io(&executable, error))?
                {
                    Some(status) => break status,
                    None => {
                        return Err(AppError::Config(format!(
                            "Process timed out and could not be terminated: {} ({kill_error})",
                            executable.display()
                        )))
                    }
                },
            },
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| AppError::Config("Child stdout reader panicked".to_string()))??;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| AppError::Config("Child stderr reader panicked".to_string()))??;
    Ok(CapturedOutput {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        stdout_truncated,
        stderr_truncated,
    })
}

pub fn spawn_detached(executable: &Path, args: &[&OsStr]) -> AppResult<()> {
    let executable = resolve_regular_file(executable).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Refusing to execute a missing or non-regular file: {}",
            executable.display()
        ))
    })?;
    let mut command = Command::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sanitize_child_environment(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| AppError::io(&executable, error))
}

pub fn sanitize_child_environment(command: &mut Command) {
    for name in SENSITIVE_CHILD_ENV {
        command.env_remove(name);
    }
    command.env("GH_PROMPT_DISABLED", "1");
    command.env("NO_COLOR", "1");
}

fn hash_sample(
    file: &mut fs::File,
    hasher: &mut impl Hasher,
    limit: usize,
) -> AppResult<()> {
    let mut remaining = limit;
    let mut buffer = [0u8; READ_CHUNK_BYTES];
    while remaining > 0 {
        let read = file
            .read(&mut buffer[..remaining.min(READ_CHUNK_BYTES)])
            .map_err(|error| AppError::Config(format!("Failed to fingerprint file: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.write(&buffer[..read]);
        remaining -= read;
    }
    Ok(())
}

fn drain_capped(mut reader: impl Read, limit: usize) -> AppResult<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(READ_CHUNK_BYTES));
    let mut buffer = [0u8; READ_CHUNK_BYTES];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| AppError::Config(format!("Failed to read child output: {error}")))?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = remaining.min(read);
        retained
            .write_all(&buffer[..keep])
            .map_err(|error| AppError::Config(format!("Failed to retain child output: {error}")))?;
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn capped_reader_keeps_prefix_and_drains_remainder() {
        let (value, truncated) = drain_capped(Cursor::new(b"abcdefgh"), 4).expect("read");
        assert_eq!(value, b"abcd");
        assert!(truncated);
    }

    #[test]
    fn capped_reader_preserves_short_output() {
        let (value, truncated) = drain_capped(Cursor::new(b"abc"), 8).expect("read");
        assert_eq!(value, b"abc");
        assert!(!truncated);
    }

    #[test]
    fn missing_regular_file_is_not_resolved() {
        let directory = tempfile::tempdir().expect("directory");
        assert_eq!(resolve_regular_file(directory.path().join("missing")), None);
    }

    #[test]
    fn fingerprint_changes_when_file_samples_change() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("executable");
        fs::write(&path, b"first").expect("write");
        let before = fingerprint_regular_file(&path).expect("fingerprint");
        fs::write(&path, b"second").expect("write");
        let after = fingerprint_regular_file(&path).expect("fingerprint");
        assert_ne!(before, after);
    }
}
