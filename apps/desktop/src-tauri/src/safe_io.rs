//! Bounded file IO and replace-without-unlink semantics shared by native stores.
use crate::error::{AppError, AppResult};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn ensure_regular_or_missing(path: &Path) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        crate::test_support::constrain(path)?;
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(AppError::InvalidInput(format!(
                        "Refusing a reparse-point target: {}",
                        path.display()
                    )));
                }
            }
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(AppError::InvalidInput(format!(
                    "Expected a regular file: {}",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

pub fn read_optional(path: &Path, limit: u64) -> AppResult<Option<Vec<u8>>> {
    ensure_regular_or_missing(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::io(path, error)),
    };
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(AppError::InvalidInput(format!(
            "File is not regular or exceeds the {limit}-byte limit: {}",
            path.display()
        )));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(AppError::InvalidInput(
                "Refusing a reparse-point handle".into(),
            ));
        }
    }
    ensure_regular_or_missing(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
        if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            return Err(AppError::Config("File changed while opening it".into()));
        }
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > limit {
        return Err(AppError::InvalidInput(
            "File grew beyond its read limit".into(),
        ));
    }
    Ok(Some(bytes))
}

/// Resolve parent aliases once and preserve the leaf's no-symlink boundary.
pub fn resource_path(path: &Path) -> AppResult<PathBuf> {
    ensure_regular_or_missing(path)?;
    let mut parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Missing parent".into()))?;
    let mut suffix = vec![path
        .file_name()
        .ok_or_else(|| AppError::Config("Missing filename".into()))?
        .to_owned()];
    while !parent.exists() {
        suffix.push(
            parent
                .file_name()
                .ok_or_else(|| AppError::Config("Invalid parent".into()))?
                .to_owned(),
        );
        parent = parent
            .parent()
            .ok_or_else(|| AppError::Config("Missing ancestor".into()))?;
    }
    let mut resolved = fs::canonicalize(parent).map_err(|error| AppError::io(parent, error))?;
    for part in suffix.iter().rev() {
        resolved.push(part);
    }
    ensure_regular_or_missing(&resolved)?;
    Ok(resolved)
}

pub fn write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    write_with_permissions(path, bytes, None)
}

pub fn write_with_permissions(
    path: &Path,
    bytes: &[u8],
    mode: Option<Permissions>,
) -> AppResult<()> {
    write_using(path, bytes, mode, replace)
}

fn write_using(
    path: &Path,
    bytes: &[u8],
    mode: Option<Permissions>,
    replace_file: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> AppResult<()> {
    let path = resource_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Missing parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    let temp = parent.join(format!(".pilotweave-{}.tmp", Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|error| AppError::io(&temp, error))?;
        if let Some(mode) = mode {
            file.set_permissions(mode)
                .map_err(|error| AppError::io(&temp, error))?;
        }
        file.write_all(bytes)
            .map_err(|error| AppError::io(&temp, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp, error))?;
        drop(file);
        ensure_regular_or_missing(&path)?;
        replace_file(&temp, &path).map_err(|error| AppError::io(&path, error))?;
        // Replacement has committed at this point. Do not report an ordinary
        // write failure that would restore a credential against NEW metadata.
        #[cfg(unix)]
        if File::open(parent).and_then(|file| file.sync_all()).is_err() {
            log::warn!("File replacement committed; directory synchronization was unavailable");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // REPLACE_EXISTING | WRITE_THROUGH; never COPY_ALLOWED or unlink.
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// OS-held lease: dropping the handle (including process exit) releases ownership.
/// The lock file is deliberately never unlinked, preventing inode lock splitting.
pub struct Lease {
    _file: std::fs::File,
}

impl Lease {
    pub fn acquire(path: &Path) -> AppResult<Self> {
        let path = resource_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| AppError::Config("Lease needs a parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
        ensure_regular_or_missing(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0).custom_flags(0x00200000);
        }
        let file = options.open(&path).map_err(|_| {
            AppError::Config(
                "Another process holds the managed-write lease, or the lease is unavailable".into(),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(AppError::Config(
                    "Another process holds the managed-write lease".into(),
                ));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if file
                .metadata()
                .map_err(|error| AppError::io(&path, error))?
                .file_attributes()
                & 0x400
                != 0
            {
                return Err(AppError::InvalidInput(
                    "Refusing a reparse-point lease".into(),
                ));
            }
        }
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_failure_preserves_original_and_cleans_temp() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("state.json");
        fs::write(&path, b"original").expect("seed");
        assert!(write_using(&path, b"new", None, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected",
            ))
        })
        .is_err());
        assert_eq!(fs::read(&path).expect("read"), b"original");
        assert_eq!(fs::read_dir(dir.path()).expect("list").count(), 1);
    }

    #[test]
    fn leases_exclude_other_writers_and_release_on_drop() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("write.lock");
        let lease = Lease::acquire(&path).expect("lease");
        assert!(Lease::acquire(&path).is_err());
        drop(lease);
        assert!(Lease::acquire(&path).is_ok());
    }

    #[test]
    fn replacement_and_size_limit() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("nested/state.json");
        write_private(&path, b"one").expect("first");
        write_private(&path, b"two").expect("replace");
        assert_eq!(
            read_optional(&path, 3).expect("read"),
            Some(b"two".to_vec())
        );
        assert!(read_optional(&path, 2).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_creates_private_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("original");
        let link = dir.path().join("link");
        write_private(&path, b"secret").expect("write");
        symlink(&path, &link).expect("link");
        assert!(write_private(&link, b"overwrite").is_err());
        assert!(read_optional(&link, 100).is_err());
        assert_eq!(fs::read(&path).expect("read"), b"secret");
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
}
