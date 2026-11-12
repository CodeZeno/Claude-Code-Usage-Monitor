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
    FindWindowW, GetClassNameW, GetWindowThreadProcessId, IsWindow, MessageBoxW, PostMessageW,
    SetForegroundWindow, ShowWindowAsync, MB_ICONERROR, MB_OK, SW_RESTORE, WM_CLOSE,
};

pub(crate) const DASHBOARD_TITLE: &str = "Claude Code Usage Monitor";
// Both eframe/OpenGL and our WARP host use winit's default native class.
// The hidden monitor controller has the same title but a different class.
const DASHBOARD_WINDOW_CLASS: &str = "Window Class";
const DASHBOARD_MUTEX: &str = "Local\\ClaudeCodeUsageMonitorDashboard";
const DASHBOARD_REQUEST_EVENT: &str = "Local\\ClaudeCodeUsageMonitorOpenDashboard";

/// Display-only snapshot shared with the dashboard process. The monitor keeps
/// the authoritative release descriptor and chooses the installation channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Applying,
    Available(String),
}

impl UpdateStatus {
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Applying)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UpdateSnapshot {
    owner: isize,
    process_id: u32,
    status: UpdateStatus,
}

fn update_status_path() -> std::path::PathBuf {
    crate::app_settings::app_data_directory().join("update-status.json")
}

pub(crate) fn publish_update_status(owner: HWND, status: UpdateStatus) {
    let snapshot = UpdateSnapshot {
        owner: owner.0 as isize,
        process_id: std::process::id(),
        status,
    };
    if let Err(error) = crate::app_settings::write_json_atomic(&update_status_path(), &snapshot) {
        crate::diagnose::log_error("dashboard update status save failed", error);
    }
}

fn monitor_process_id(owner: isize) -> Option<u32> {
    if owner == 0 {
        return None;
    }
    let hwnd = HWND(owner as *mut _);
    let mut class_name = [0u16; 64];
    let mut process_id = 0;
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let length = GetClassNameW(hwnd, &mut class_name).max(0) as usize;
        if String::from_utf16_lossy(&class_name[..length]) != "ClaudeCodeUsageMonitor" {
            return None;
        }
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }
    (process_id != 0).then_some(process_id)
}

fn read_live_update_snapshot() -> Option<UpdateSnapshot> {
    let content = std::fs::read(update_status_path()).ok()?;
    let snapshot: UpdateSnapshot = serde_json::from_slice(&content).ok()?;
    // A saved HWND may have been destroyed or reused by a different process.
    (monitor_process_id(snapshot.owner) == Some(snapshot.process_id)).then_some(snapshot)
}

/// Keep a live connection, or discover the monitor that replaced a dead owner.
/// Owner zero denotes a standalone editor and must remain disconnected.
pub(crate) fn resolve_monitor_owner(owner: isize) -> Option<isize> {
    if owner == 0 {
        return None;
    }
    if monitor_process_id(owner).is_some() {
        return Some(owner);
    }
    read_live_update_snapshot().map(|snapshot| snapshot.owner)
}

pub(crate) fn read_update_status(owner: isize) -> UpdateStatus {
    read_live_update_snapshot()
        .filter(|snapshot| snapshot.owner == owner)
        .map(|snapshot| snapshot.status)
        .unwrap_or_default()
}

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
    crate::diagnose::log("dashboard requested from monitor");
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

fn find_dashboard_window(title: &str) -> Option<HWND> {
    let title = crate::native_interop::wide_str(title);
    let class = crate::native_interop::wide_str(DASHBOARD_WINDOW_CLASS);
    unsafe {
        FindWindowW(
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
        )
        .ok()
    }
}

pub fn focus_existing() -> bool {
    let Some(hwnd) = find_dashboard_window(DASHBOARD_TITLE) else {
        return false;
    };
    unsafe {
        // The dashboard runs on another UI thread/process. Do not block the
        // monitor (and its taskbar host) if that thread is busy or unresponsive.
        let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        true
    }
}

pub fn close_existing() -> bool {
    let Some(hwnd) = find_dashboard_window(DASHBOARD_TITLE) else {
        return false;
    };
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
}

pub fn toggle(owner: HWND) {
    if !close_existing() {
        show(owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct TestWindow(HWND);

    impl TestWindow {
        fn new(class: &str, title: &str) -> Self {
            unsafe extern "system" fn procedure(
                hwnd: HWND,
                message: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> windows::Win32::Foundation::LRESULT {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
            let class = crate::native_interop::wide_str(class);
            let title = crate::native_interop::wide_str(title);
            unsafe {
                let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
                let registered = RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance.into(),
                    lpszClassName: PCWSTR::from_raw(class.as_ptr()),
                    ..Default::default()
                });
                assert!(registered != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
                Self(
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        PCWSTR::from_raw(class.as_ptr()),
                        PCWSTR::from_raw(title.as_ptr()),
                        WINDOW_STYLE::default(),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(instance.into()),
                        None,
                    )
                    .unwrap(),
                )
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn dashboard_lookup_excludes_the_controller_with_the_same_title() {
        let title = format!("CCUM dashboard lookup regression {}", std::process::id());
        let _controller = TestWindow::new("CCUMDashboardLookupControllerTest", &title);
        assert!(find_dashboard_window(&title).is_none());
        let dashboard = TestWindow::new(DASHBOARD_WINDOW_CLASS, &title);
        assert_eq!(find_dashboard_window(&title), Some(dashboard.0));
        drop(dashboard);
        assert!(find_dashboard_window(&title).is_none());
    }
}
