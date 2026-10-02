//! Bounded Windows identity observations. Version resources are data, not proof
//! of a publisher; installer execution additionally requires OS Authenticode.
use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};

pub(super) struct ProductIdentity {
    pub product: String,
    pub version: String,
}

fn unavailable() -> AppError {
    AppError::Unsupported(
        "Executable identity or Windows publisher could not be verified; no installer was launched"
            .into(),
    )
}

pub(super) fn product(path: &Path) -> AppResult<ProductIdentity> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        // The test backend recognizes only exact files below its validated root.
        if crate::test_support::executable("github-copilot.exe").as_deref() == Some(path) {
            return Ok(ProductIdentity {
                product: "GitHub Copilot".into(),
                version: "1.1.25".into(),
            });
        }
        return Err(unavailable());
    }
    let path = crate::native_process::resolve_regular_file(path).ok_or_else(unavailable)?;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW};
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the UTF-16 path is terminated and both buffers live through the calls.
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut()) };
    if size == 0 || size > 1024 * 1024 {
        return Err(unavailable());
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return Err(unavailable());
    }
    let translation = query(&data, "\\VarFileInfo\\Translation", 1)?;
    if translation.len() < 4 || translation.len() % 4 != 0 {
        return Err(unavailable());
    }
    for pair in translation.chunks_exact(4).take(16) {
        let language = u16::from_le_bytes([pair[0], pair[1]]);
        let codepage = u16::from_le_bytes([pair[2], pair[3]]);
        let prefix = format!("\\StringFileInfo\\{language:04x}{codepage:04x}");
        let fields = (
            query_text(&data, &format!("{prefix}\\ProductName")),
            query_text(&data, &format!("{prefix}\\ProductVersion")),
        );
        if let (Ok(product), Ok(version)) = fields {
            return Ok(ProductIdentity { product, version });
        }
    }
    Err(unavailable())
}

fn query<'a>(data: &'a [u8], key: &str, unit_bytes: usize) -> AppResult<&'a [u8]> {
    use windows_sys::Win32::Storage::FileSystem::VerQueryValueW;
    let wide = key.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut pointer = std::ptr::null_mut();
    let mut units = 0u32;
    // SAFETY: Windows returns a view into this version-resource allocation. Its
    // complete address range is checked before constructing a Rust slice.
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            wide.as_ptr(),
            &mut pointer,
            &mut units,
        )
    } == 0
    {
        return Err(unavailable());
    }
    let length = (units as usize)
        .checked_mul(unit_bytes)
        .ok_or_else(unavailable)?;
    let start = (pointer as usize)
        .checked_sub(data.as_ptr() as usize)
        .ok_or_else(unavailable)?;
    let end = start.checked_add(length).ok_or_else(unavailable)?;
    data.get(start..end).ok_or_else(unavailable)
}

fn query_text(data: &[u8], key: &str) -> AppResult<String> {
    let bytes = query(data, key, 2)?;
    if bytes.len() > 2048 || bytes.len() % 2 != 0 {
        return Err(unavailable());
    }
    let units = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect::<Vec<_>>();
    let units = units.strip_suffix(&[0]).ok_or_else(unavailable)?;
    let value = String::from_utf16(units).map_err(|_| unavailable())?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(unavailable());
    }
    Ok(value)
}

pub(super) fn verify_winget(path: &Path) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return if crate::test_support::executable("winget.exe").as_deref() == Some(path) {
            Ok(())
        } else {
            Err(unavailable())
        };
    }
    verify_with(path, trusted_publisher)
}

pub(super) fn winget_path() -> AppResult<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::executable("winget.exe").ok_or_else(unavailable);
    }
    // Prefer the OS-reported registered package, not a zero-byte AppExecutionAlias.
    // A portable PATH installation is accepted only after the same publisher check.
    for path in packaged_winget()?
        .into_iter()
        .chain(crate::native_process::resolve_on_path(&["winget.exe"]))
    {
        if verify_winget(&path).is_ok() {
            return Ok(path);
        }
    }
    Err(unavailable())
}

fn packaged_winget() -> AppResult<Vec<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
    use windows_sys::Win32::Storage::Packaging::Appx::{
        GetPackagePathByFullName, GetPackagesByPackageFamily,
    };
    let family = windows_sys::core::w!("Microsoft.DesktopAppInstaller_8wekyb3d8bbwe");
    let mut count = 0u32;
    let mut length = 0u32;
    // SAFETY: documented two-call package query; allocations and returned ranges
    // are bounded before using any OS-returned package name or path.
    let status = unsafe {
        GetPackagesByPackageFamily(
            family,
            &mut count,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
        )
    };
    if status == ERROR_SUCCESS && count == 0 {
        return Ok(Vec::new());
    }
    if status != ERROR_INSUFFICIENT_BUFFER || count == 0 || count > 16 || length > 32768 {
        return Err(unavailable());
    }
    let mut names = vec![std::ptr::null_mut(); count as usize];
    let mut buffer = vec![0u16; length as usize];
    if unsafe {
        GetPackagesByPackageFamily(
            family,
            &mut count,
            names.as_mut_ptr(),
            &mut length,
            buffer.as_mut_ptr(),
        )
    } != ERROR_SUCCESS
        || count as usize > names.len()
    {
        return Err(unavailable());
    }
    let mut paths = Vec::new();
    for name in names.into_iter().take(count as usize) {
        let offset = (name as usize)
            .checked_sub(buffer.as_ptr() as usize)
            .ok_or_else(unavailable)?;
        if offset % 2 != 0 {
            return Err(unavailable());
        }
        let remaining = buffer.get(offset / 2..).ok_or_else(unavailable)?;
        if !remaining.contains(&0) {
            return Err(unavailable());
        }
        let mut path_length = 0u32;
        if unsafe { GetPackagePathByFullName(name, &mut path_length, std::ptr::null_mut()) }
            != ERROR_INSUFFICIENT_BUFFER
            || path_length == 0
            || path_length > 32768
        {
            return Err(unavailable());
        }
        let mut path = vec![0u16; path_length as usize];
        if unsafe { GetPackagePathByFullName(name, &mut path_length, path.as_mut_ptr()) }
            != ERROR_SUCCESS
            || path_length as usize > path.len()
        {
            return Err(unavailable());
        }
        let end = path.iter().position(|c| *c == 0).ok_or_else(unavailable)?;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&path[..end])).join("winget.exe");
        if let Some(path) = crate::native_process::resolve_regular_file(path) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn verify_with(path: &Path, verifier: impl FnOnce(&Path) -> AppResult<String>) -> AppResult<()> {
    // A signed but unrelated program is not an allowlisted package manager.
    if path
        .file_name()
        .is_none_or(|name| !name.to_string_lossy().eq_ignore_ascii_case("winget.exe"))
    {
        return Err(unavailable());
    }
    let publisher = verifier(path)?;
    if publisher != "Microsoft Corporation" {
        return Err(unavailable());
    }
    Ok(())
}

fn trusted_publisher(path: &Path) -> AppResult<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::*;
    let canonical = crate::native_process::resolve_regular_file(path).ok_or_else(unavailable)?;
    let wide = canonical
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: all pointer-backed buffers outlive verification, extraction and
    // CLOSE. Only zero is success; cached chain/revocation failures stay unknown.
    let status = unsafe { WinVerifyTrustEx((-1isize) as _, &mut action, &mut data) };
    let result = (|| {
        if status != 0 {
            return Err(unavailable());
        }
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return Err(unavailable());
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() || unsafe { (*signer).dwError != 0 || (*signer).csCertChain == 0 } {
            return Err(unavailable());
        }
        let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
        if certificate.is_null() || unsafe { (*certificate).pCert.is_null() } {
            return Err(unavailable());
        }
        let mut subject = [0u16; 512];
        let length = unsafe {
            CertGetNameStringW(
                (*certificate).pCert,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                std::ptr::null(),
                subject.as_mut_ptr(),
                subject.len() as u32,
            )
        };
        if length <= 1 || length as usize > subject.len() {
            return Err(unavailable());
        }
        String::from_utf16(&subject[..length as usize - 1]).map_err(|_| unavailable())
    })();
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrustEx((-1isize) as _, &mut action, &mut data);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_or_untrusted_publishers_never_authorize_an_installer() {
        assert!(verify_with(Path::new("winget.exe"), |_| Ok(
            "Microsoft Corporation".into()
        ))
        .is_ok());
        assert!(verify_with(
            Path::new("winget.exe"),
            |_| Ok("Unknown Corporation".into())
        )
        .is_err());
        assert!(verify_with(Path::new("evil.exe"), |_| panic!("not allowlisted")).is_err());
        assert!(verify_with(Path::new("winget.exe"), |_| Err(unavailable())).is_err());
    }
    #[test]
    fn unsigned_or_invalid_executable_stays_unknown() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("winget.exe");
        std::fs::write(&path, b"synthetic unsigned executable").unwrap();
        assert!(verify_winget(&path).is_err());
        assert!(product(&path).is_err());
    }
}
