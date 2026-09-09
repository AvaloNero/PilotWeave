//! Crash-released cross-process exclusion for PilotWeave's managed mutations.
//! The empty lock file is never deleted or replaced while instances may use it.
use crate::error::{AppError, AppResult};
use crate::safe_file;
use std::fs::{File, OpenOptions};
use std::path::Path;

pub struct WriteLock {
    _file: File,
}

impl WriteLock {
    pub fn acquire(state_path: &Path) -> AppResult<Self> {
        let path = state_path.with_file_name("managed-writes.lock");
        safe_file::create_parent(&path)?;
        let file = match safe_file::create_private(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut options = OpenOptions::new();
                options.write(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
                    options
                        .share_mode(0)
                        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
                }
                options
                    .open(&path)
                    .map_err(|error| lock_error(&path, error))?
            }
            Err(error) => return Err(lock_error(&path, error)),
        };
        let metadata = file
            .metadata()
            .map_err(|error| AppError::io(&path, error))?;
        if !metadata.is_file() || safe_file::is_link(&metadata) || metadata.len() != 0 {
            return Err(AppError::Config(
                "Managed write lock is not a valid empty regular file".into(),
            ));
        }
        safe_file::ensure_regular_or_missing(&path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the descriptor stays alive in this guard; flock is nonblocking.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(lock_error(&path, std::io::Error::last_os_error()));
            }
        }
        Ok(Self { _file: file })
    }
}

fn lock_error(path: &Path, error: std::io::Error) -> AppError {
    #[cfg(windows)]
    if error.raw_os_error() == Some(32) {
        return AppError::Busy;
    }
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return AppError::Busy;
    }
    AppError::io(path, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_handles_exclude_each_other_and_drop_releases_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("nested/state.json");
        let first = WriteLock::acquire(&state).unwrap();
        assert!(matches!(WriteLock::acquire(&state), Err(AppError::Busy)));
        drop(first);
        let second = WriteLock::acquire(&state).unwrap();
        assert!(matches!(WriteLock::acquire(&state), Err(AppError::Busy)));
        drop(second);
        assert!(WriteLock::acquire(&state).is_ok());
    }

    #[test]
    fn an_invalid_lock_file_is_not_truncated_or_deleted() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state.json");
        let lock = state.with_file_name("managed-writes.lock");
        std::fs::write(&lock, b"unrelated data").unwrap();
        assert!(WriteLock::acquire(&state).is_err());
        assert_eq!(std::fs::read(lock).unwrap(), b"unrelated data");
    }
}
