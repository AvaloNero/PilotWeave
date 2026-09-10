//! Bounded file access and same-directory replacement for native private state.
//! No caller deletes a destination before replacing it.
use crate::error::{AppError, AppResult};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path};
use uuid::Uuid;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PATH_COMPONENTS: usize = 128;

pub(crate) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Check the requested path before canonicalization, including every existing
/// parent. Canonicalizing first would conceal a junction or symbolic link.
pub fn ensure_regular_or_missing(path: &Path) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        crate::test_support::constrain(path)?;
    }
    if !path.is_absolute()
        || path.components().count() > MAX_PATH_COMPONENTS
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || path.file_name().is_none()
    {
        return Err(AppError::InvalidInput("Unsafe managed file path".into()));
    }
    #[cfg(windows)]
    for part in path.components() {
        if let Component::Normal(name) = part {
            let name = name.to_string_lossy();
            if name.contains(':') || name.ends_with(['.', ' ']) {
                return Err(AppError::InvalidInput(
                    "Alternate streams and ambiguous Windows paths are not supported".into(),
                ));
            }
        }
    }
    for (index, candidate) in path.ancestors().enumerate() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if is_link(&metadata) && !is_system_alias(candidate) {
                    return Err(AppError::InvalidInput(
                        "A managed file or parent is a symbolic link or reparse point".into(),
                    ));
                }
                if index == 0 && !metadata.is_file() {
                    return Err(AppError::InvalidInput(
                        "A managed file must be a regular file".into(),
                    ));
                }
                if index != 0 && !metadata.is_dir() && !is_system_alias(candidate) {
                    return Err(AppError::InvalidInput(
                        "A managed file parent must be a directory".into(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::io(candidate, error)),
        }
    }
    Ok(())
}

// macOS uses root-owned aliases for the system temporary directory. Only these
// exact aliases are accepted, and only while they resolve to the system path.
fn is_system_alias(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let expected = match path.to_str() {
            Some("/var") => "/private/var",
            Some("/tmp") => "/private/tmp",
            _ => return false,
        };
        fs::canonicalize(path).is_ok_and(|actual| actual == Path::new(expected))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

pub fn read_optional(path: &Path, limit: u64) -> AppResult<Option<Vec<u8>>> {
    ensure_regular_or_missing(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::io(path, error)),
    };
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(AppError::InvalidInput(
            "Refusing a non-regular managed file".into(),
        ));
    }
    let limit = limit.min(MAX_FILE_BYTES);
    if metadata.len() > limit {
        return Err(AppError::InvalidInput(
            "Managed file exceeds its size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > limit {
        return Err(AppError::InvalidInput(
            "Managed file grew beyond its size limit".into(),
        ));
    }
    ensure_regular_or_missing(path)?;
    Ok(Some(bytes))
}

pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    write_with(path, bytes, false, replace_file)
}

/// Reject a stale caller snapshot both before staging and immediately before replacement.
pub fn atomic_write_private_if(
    path: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
) -> AppResult<()> {
    write_expected(path, bytes, false, expected, replace_file)
}

#[cfg(all(unix, test))]
pub fn atomic_write_preserving_mode(path: &Path, bytes: &[u8]) -> AppResult<()> {
    write_with(path, bytes, true, replace_file)
}

fn write_with(
    path: &Path,
    bytes: &[u8],
    preserve_mode: bool,
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> AppResult<()> {
    let before = read_optional(path, MAX_FILE_BYTES)?;
    write_expected(path, bytes, preserve_mode, before.as_deref(), replace)
}

fn write_expected(
    path: &Path,
    bytes: &[u8],
    preserve_mode: bool,
    expected: Option<&[u8]>,
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> AppResult<()> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(AppError::InvalidInput(
            "Managed write exceeds its size limit".into(),
        ));
    }
    let before = read_optional(path, MAX_FILE_BYTES)?;
    if before.as_deref() != expected {
        return Err(AppError::InvalidInput(
            "Managed file changed since it was read".into(),
        ));
    }
    create_parent(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("File has no parent".into()))?;
    let temp = parent.join(format!(".pilotweave-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = create_private(&temp).map_err(|error| AppError::io(&temp, error))?;
        #[cfg(unix)]
        if preserve_mode {
            match fs::metadata(path) {
                Ok(metadata) => file
                    .set_permissions(metadata.permissions())
                    .map_err(|error| AppError::io(&temp, error))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::io(path, error)),
            }
        }
        #[cfg(not(unix))]
        let _ = preserve_mode;
        file.write_all(bytes)
            .map_err(|error| AppError::io(&temp, error))?;
        file.sync_all()
            .map_err(|error| AppError::io(&temp, error))?;
        drop(file);
        if read_optional(path, MAX_FILE_BYTES)? != before {
            return Err(AppError::Config(
                "Managed file changed while its replacement was prepared; retry from a fresh preview".into(),
            ));
        }
        replace(&temp, path).map_err(|error| AppError::io(path, error))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| AppError::io(parent, error))?;
        Ok(())
    })();
    if result.is_err() {
        // This is only our randomly named staging file, never the destination.
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(crate) fn create_parent(path: &Path) -> AppResult<()> {
    ensure_regular_or_missing(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("File has no parent".into()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|error| AppError::io(parent, error))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    ensure_regular_or_missing(path)
}

#[cfg(windows)]
pub(crate) fn create_private(path: &Path) -> std::io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // A protected DACL: only the file owner and LocalSystem have access.
    let sddl = "D:P(A;;FA;;;SY)(A;;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: all strings are NUL-terminated; descriptor is freed after creation.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: descriptor and path remain valid for this call.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = std::io::Error::last_os_error();
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE {
        Err(error)
    } else {
        // SAFETY: ownership of the valid new file handle transfers to File.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

#[cfg(not(windows))]
pub(crate) fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
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
    // The staging file is flushed, closed, and on the same volume. Do not allow
    // a copy/delete fallback or delayed replacement.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_replacement_never_adopts_an_external_change() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"external").unwrap();
        assert!(atomic_write_private_if(&path, b"new", Some(b"old")).is_err());
        assert!(atomic_write_private_if(&path, b"new", None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_replacement_keeps_original_and_removes_only_staging_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"original").unwrap();
        let result = write_with(&path, b"new", false, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected failure",
            ))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn missing_and_existing_files_are_replaced_without_truncation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nested/state.json");
        atomic_write_private(&path, b"first").unwrap();
        atomic_write_private(&path, b"second").unwrap();
        assert_eq!(read_optional(&path, 6).unwrap().unwrap(), b"second");
        assert!(read_optional(&path, 5).is_err());
    }

    #[test]
    fn rejects_non_regular_targets_and_traversal() {
        let root = tempfile::tempdir().unwrap();
        assert!(atomic_write_private(root.path(), b"no").is_err());
        assert!(atomic_write_private(&root.path().join("../escape"), b"no").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn locked_destination_survives_failed_windows_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"original").unwrap();
        let _locked = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        assert!(atomic_write_private(&path, b"new").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_links_and_linked_parents_without_writing_through() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let linked = root.path().join("linked");
        symlink(outside.path(), &linked).unwrap();
        assert!(atomic_write_private(&linked.join("state.json"), b"no").is_err());
        assert!(!outside.path().join("state.json").exists());
        let dangling = root.path().join("dangling");
        symlink(root.path().join("missing"), &dangling).unwrap();
        assert!(read_optional(&dangling, 10).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_and_shell_modes_can_be_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        atomic_write_private(&path, b"secret").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write_preserving_mode(&path, b"profile").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
