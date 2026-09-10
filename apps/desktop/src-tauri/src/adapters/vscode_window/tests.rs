use super::*;
use std::{sync::mpsc, thread, time::Duration};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};

#[test]
fn unresponsive_window_does_not_block_account_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("fixture.exe");
    std::fs::write(&fixture, b"inert fixture, never executed").unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let window_thread = thread::spawn(move || {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "PilotWeave test fixture\0".encode_utf16().collect();
        // Own an off-screen, non-activating 1px window in this test process.
        // It deliberately has no message loop. No real client is opened.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                -32000,
                -32000,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        ready_tx.send(!window.is_null()).unwrap();
        if !window.is_null() {
            // Bound even the failure path; the owner thread always destroys it.
            let _ = stop_rx.recv_timeout(Duration::from_secs(10));
            unsafe { DestroyWindow(window) };
        }
    });
    let ready = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or(false);
    let (result_tx, result_rx) = mpsc::channel();
    let probe = thread::spawn(move || {
        let _ = result_tx.send(windows::focus(&fixture));
    });
    let result = result_rx.recv_timeout(Duration::from_secs(2));
    let _ = stop_tx.send(());
    window_thread.join().unwrap();
    probe.join().unwrap();
    assert!(ready, "The owned window fixture must be created");
    assert_eq!(result.unwrap(), MANUAL);
}
