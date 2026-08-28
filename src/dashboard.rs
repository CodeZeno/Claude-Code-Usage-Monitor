//! Launches and focuses the single GPU-rendered dashboard process.

use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, MessageBoxW, PostMessageW, SetForegroundWindow, ShowWindow, MB_ICONERROR, MB_OK,
    SW_RESTORE, WM_CLOSE,
};

const DASHBOARD_TITLE: &str = "Usage Monitor";
const DASHBOARD_MUTEX: &str = "Local\\ClaudeCodeUsageMonitorDashboard";
const DASHBOARD_REQUEST_EVENT: &str = "Local\\ClaudeCodeUsageMonitorOpenDashboard";

fn language() -> crate::localization::LanguageId {
    let settings = crate::app_settings::load_settings();
    crate::localization::resolve_language(
        settings
            .language
            .as_deref()
            .and_then(crate::localization::LanguageId::from_code),
    )
}

pub fn show(owner: HWND) {
    if focus_existing() {
        return;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            report_launch_failure(
                owner,
                &format!(
                    "{}: {error}",
                    language().text("Unable to locate the application")
                ),
            );
            return;
        }
    };
    let mut command = std::process::Command::new(executable);
    command
        .arg("--studio")
        .arg("--owner")
        .arg((owner.0 as isize).to_string());
    if crate::diagnose::is_enabled() {
        command.arg("--diagnose").arg("--diagnose-append");
    }
    if let Err(error) = command.spawn() {
        report_launch_failure(
            owner,
            &format!(
                "{}: {error}",
                language().text("Unable to start the dashboard")
            ),
        );
    }
}

/// Claim the dashboard process slot. A second process exits after restoring
/// the existing window, which also closes the rapid-click startup race.
pub fn claim_instance() -> Result<Option<HANDLE>, String> {
    let name = crate::native_interop::wide_str(DASHBOARD_MUTEX);
    unsafe {
        let handle =
            CreateMutexW(None, true, PCWSTR::from_raw(name.as_ptr())).map_err(|error| {
                format!(
                    "{}: {error}",
                    language().text("Unable to create the dashboard instance guard")
                )
            })?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            focus_existing();
            Ok(None)
        } else {
            Ok(Some(handle))
        }
    }
}

/// Listen for dashboard requests through a named event. The monitor window can
/// be embedded as a taskbar child, so it cannot be found reliably with the
/// top-level FindWindow APIs used for ordinary application windows.
pub fn start_request_listener(owner: HWND) -> Result<(), String> {
    let event_name = crate::native_interop::wide_str(DASHBOARD_REQUEST_EVENT);
    let event = unsafe { CreateEventW(None, false, false, PCWSTR::from_raw(event_name.as_ptr())) }
        .map_err(|error| format!("Unable to create the dashboard request event: {error}"))?;
    let event_value = event.0 as isize;
    let owner_value = owner.0 as isize;
    std::thread::spawn(move || loop {
        let event = HANDLE(event_value as *mut _);
        if unsafe { WaitForSingleObject(event, INFINITE) } != WAIT_OBJECT_0 {
            break;
        }
        let owner = HWND(owner_value as *mut _);
        if unsafe {
            PostMessageW(
                Some(owner),
                crate::native_interop::WM_APP_OPEN_DASHBOARD,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .is_err()
        {
            break;
        }
    });
    Ok(())
}

/// Ask an already-running monitor process to open its dashboard. The short
/// retry handles the startup interval after the mutex is created but before the
/// named request event is installed.
pub fn request_from_existing_monitor() -> Result<(), String> {
    let event_name = crate::native_interop::wide_str(DASHBOARD_REQUEST_EVENT);
    for _ in 0..40 {
        unsafe {
            if let Ok(event) = OpenEventW(
                EVENT_MODIFY_STATE,
                false,
                PCWSTR::from_raw(event_name.as_ptr()),
            ) {
                let result = SetEvent(event).map_err(|error| {
                    format!(
                        "{}: {error}",
                        language().text("Unable to signal the running monitor")
                    )
                });
                let _ = CloseHandle(event);
                return result;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(language()
        .text("The monitor is running, but its dashboard request channel was not found")
        .into())
}

pub fn report_launch_failure(owner: HWND, detail: &str) {
    crate::diagnose::log(format!("dashboard launch failed: {detail}"));
    unsafe {
        let title = crate::native_interop::wide_str(language().text("Unable to open dashboard"));
        let message = crate::native_interop::wide_str(detail);
        let _ = MessageBoxW(
            Some(owner),
            PCWSTR::from_raw(message.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn focus_existing() -> bool {
    let title = crate::native_interop::wide_str(DASHBOARD_TITLE);
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr())) else {
            return false;
        };
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        true
    }
}

pub fn close_existing() -> bool {
    let title = crate::native_interop::wide_str(DASHBOARD_TITLE);
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr())) else {
            return false;
        };
        PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
    }
}

pub fn toggle(owner: HWND) {
    if !close_existing() {
        show(owner);
    }
}
