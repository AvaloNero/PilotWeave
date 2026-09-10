//! VS Code cold starts can restore every previous window, including hot-exit
//! backups. Account orchestration may focus an existing window, never start it.
use std::path::Path;

pub const MANUAL: &str = "Open VS Code yourself, then use Accounts to sign in to GitHub. PilotWeave did not start VS Code because a cold start can restore many saved windows.";
pub const SWITCH: &str = "Switch to your existing VS Code window and use Accounts to verify or sign in to GitHub. No new VS Code process was started.";
#[cfg(any(windows, test, feature = "local-e2e"))]
pub const FOCUSED: &str = "Your existing VS Code window was brought forward. Use Accounts to verify or sign in to GitHub. No new VS Code process was started.";

pub fn focus_existing(executable: &Path) -> &'static str {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return if crate::test_support::root()
            .is_some_and(|root| root.join("private/vscode-window-open").is_file())
        {
            FOCUSED
        } else {
            MANUAL
        };
    }
    #[cfg(windows)]
    {
        windows::focus(executable)
    }
    #[cfg(not(windows))]
    {
        let _ = executable;
        MANUAL
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindowAsync, GW_OWNER, SW_RESTORE,
    };

    struct Search<'a> {
        expected: &'a Path,
        found: HWND,
        inspected: usize,
    }
    unsafe extern "system" fn visit(window: HWND, parameter: LPARAM) -> i32 {
        // SAFETY: EnumWindows calls synchronously with the live Search passed below.
        let search = unsafe { &mut *(parameter as *mut Search<'_>) };
        search.inspected += 1;
        if search.inspected > 4096 {
            return 0;
        }
        // Class/visibility/owner metadata does not ask the target window to
        // process a message. Never query its title, even just the title length.
        let mut class = [0u16; 64];
        // SAFETY: the API receives a correctly sized writable buffer and valid HWND.
        let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        if length <= 0
            || String::from_utf16_lossy(&class[..length as usize]) != "Chrome_WidgetWin_1"
        {
            return 1;
        }
        if unsafe { IsWindowVisible(window) } == 0
            || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
        {
            return 1;
        }
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return 1;
        }
        let mut image = [0u16; 32768];
        let mut length = image.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut length) };
        unsafe {
            CloseHandle(process);
        }
        if ok == 0 {
            return 1;
        }
        let image = std::path::PathBuf::from(String::from_utf16_lossy(&image[..length as usize]));
        if crate::native_process::resolve_regular_file(image).as_deref() == Some(search.expected) {
            search.found = window;
            return 0;
        }
        1
    }

    pub(super) fn focus(executable: &Path) -> &'static str {
        let Some(executable) = crate::native_process::resolve_regular_file(executable) else {
            return MANUAL;
        };
        let mut search = Search {
            expected: &executable,
            found: std::ptr::null_mut(),
            inspected: 0,
        };
        // SAFETY: the callback keeps no pointers after the synchronous enumeration.
        unsafe {
            EnumWindows(Some(visit), &mut search as *mut Search<'_> as LPARAM);
        }
        if search.found.is_null() {
            return MANUAL;
        }
        // These calls only focus/restore an existing verified editor window.
        if unsafe { IsIconic(search.found) } != 0 {
            unsafe {
                // Queue restoration; do not wait on a hung editor UI thread.
                ShowWindowAsync(search.found, SW_RESTORE);
            }
        }
        if unsafe { SetForegroundWindow(search.found) } != 0 {
            FOCUSED
        } else {
            SWITCH
        }
    }
}

#[cfg(all(test, windows))]
mod tests;
