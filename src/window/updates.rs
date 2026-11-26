//! Checking for, prompting about and installing app updates.
#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use super::*;

#[derive(Clone, Debug)]
pub(super) enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(AvailableUpdate),
}

pub(super) fn publish_update_status(state: &AppState) {
    use crate::dashboard::UpdateStatus as DashboardStatus;
    let status = match &state.update_status {
        UpdateStatus::Idle | UpdateStatus::UpToDate => DashboardStatus::Idle,
        UpdateStatus::Checking => DashboardStatus::Checking,
        UpdateStatus::Applying => DashboardStatus::Applying,
        UpdateStatus::Available(update) => DashboardStatus::Available(update.version().to_owned()),
    };
    crate::dashboard::publish_update_status(controller_for(state.hwnd.to_hwnd()), status);
}

pub(super) fn perform_update_action(hwnd: HWND) {
    let update = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return;
        };
        if matches!(
            state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            return;
        }
        match &state.update_status {
            UpdateStatus::Available(update) => Some(update.clone()),
            _ => None,
        }
    };
    match update {
        Some(update) => begin_update_install(hwnd, update),
        None => begin_update_check(hwnd, true),
    }
}

fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

pub(super) fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

pub(super) fn schedule_auto_update_check(hwnd: HWND) {
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };

        if auto_update_check_due(s.last_update_check_unix) {
            None
        } else {
            let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
            let remaining_secs = update_check_interval().as_secs().saturating_sub(elapsed);
            Some((remaining_secs.saturating_mul(1000)).min(u32::MAX as u64) as u32)
        }
    };

    // SAFETY: set_controller_timer/kill_controller_timer forward to SetTimer/KillTimer
    // with no TIMERPROC, so no callback pointer must stay valid; the HWND is
    // passed by value and Windows rejects a stale one.
    unsafe {
        let _ = kill_controller_timer(Some(hwnd), TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            set_controller_timer(Some(hwnd), TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
}

fn begin_update_install(hwnd: HWND, update: AvailableUpdate) {
    match update {
        AvailableUpdate::Release(release) => begin_update_apply(hwnd, release),
        AvailableUpdate::Winget { .. } => begin_winget_update(hwnd),
    }
}

fn show_update_prompt(hwnd: HWND, strings: Strings, update: &AvailableUpdate) -> bool {
    let message = strings
        .update_prompt_now
        .replace("{version}", update.version());

    // SAFETY: both strings are NUL-terminated UTF-16 buffers kept alive by locals until
    // MessageBoxW returns; the owner HWND is only used by value.
    unsafe {
        let title_wide = native_interop::wide_str(strings.update_available);
        let message_wide = native_interop::wide_str(&message);
        MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

pub(super) fn begin_update_check(hwnd: HWND, interactive: bool) {
    let send_hwnd = SendHwnd::from_hwnd(controller_for(hwnd));
    let (language, install_channel) = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            if interactive {
                show_info_message(
                    hwnd,
                    app_state.language.strings().updates,
                    app_state.language.strings().update_in_progress,
                );
            }
            return;
        }

        app_state.update_status = UpdateStatus::Checking;
        publish_update_status(app_state);
        (app_state.language, app_state.install_channel)
    };
    let strings = language.strings();

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates(install_channel) {
            Ok(UpdateCheckResult::ManagedExternally) => {
                if let Some(s) = lock_state().as_mut() {
                    s.update_status = UpdateStatus::Idle;
                    s.last_update_check_unix = Some(checked_at);
                    publish_update_status(s);
                }
                save_state_settings();
                if interactive {
                    show_info_message(
                        hwnd,
                        strings.updates,
                        language.text("Updates are managed by Chocolatey. Save your work and quit the monitor and dashboard, then run this command in an administrator terminal:\n\nchoco upgrade claude-code-usage-monitor\n\nStart the monitor again after the upgrade completes."),
                    );
                }
            }
            Ok(UpdateCheckResult::UpToDate) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::UpToDate;
                        s.last_update_check_unix = Some(checked_at);
                        publish_update_status(s);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
            }
            Ok(UpdateCheckResult::Available(update)) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(update.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive && show_update_prompt(hwnd, strings, &update) {
                    begin_update_install(hwnd, update);
                }
                // Keep the dashboard busy until the install prompt is dismissed.
                if let Some(state) = lock_state().as_ref() {
                    publish_update_status(state);
                }
            }
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Idle;
                        s.last_update_check_unix = Some(checked_at);
                        publish_update_status(s);
                    }
                }
                save_state_settings();
                if interactive {
                    let message = format!("{}.\n\n{}", strings.update_failed, error);
                    show_error_message(hwnd, strings.updates, &message);
                }
            }
        }
        // SAFETY: PostMessageW may be called from this worker thread for the controller
        // window; the HWND is passed by value and the message carries no pointers.
        unsafe {
            let _ = PostMessageW(
                Some(hwnd),
                WM_APP_UPDATE_CHECK_COMPLETE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
}

fn begin_update_apply(hwnd: HWND, release: ReleaseDescriptor) {
    let send_hwnd = SendHwnd::from_hwnd(controller_for(hwnd));
    let strings = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            show_info_message(
                hwnd,
                app_state.language.strings().updates,
                app_state.language.strings().update_in_progress,
            );
            return;
        }

        app_state.update_status = UpdateStatus::Applying;
        publish_update_status(app_state);
        app_state.language.strings()
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        match updater::begin_self_update(&release) {
            // SAFETY: PostMessageW may be called from this worker thread for the controller
            // window; the HWND is passed by value and WM_CLOSE carries no pointers.
            Ok(()) => unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status =
                            UpdateStatus::Available(AvailableUpdate::Release(release));
                        publish_update_status(s);
                    }
                }
                let message = format!("{}.\n\n{}", strings.update_failed, error);
                show_error_message(hwnd, strings.updates, &message);
                // SAFETY: PostMessageW may be called from this worker thread for the controller
                // window; the HWND is passed by value and the message carries no pointers.
                unsafe {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
    });
}

pub(super) fn begin_winget_update(hwnd: HWND) {
    let (strings, previous_status) = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        if matches!(
            state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            return;
        }
        let previous_status = std::mem::replace(&mut state.update_status, UpdateStatus::Applying);
        publish_update_status(state);
        (state.language.strings(), previous_status)
    };

    match updater::begin_winget_update() {
        // SAFETY: PostMessageW takes the HWND by value (Windows rejects a stale one) and
        // WM_CLOSE carries no pointers.
        Ok(()) => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            if let Some(state) = lock_state().as_mut() {
                state.update_status = previous_status;
                publish_update_status(state);
            }
            let message = format!("{}.\n\n{}", strings.update_failed, error);
            show_error_message(hwnd, strings.updates, &message);
        }
    }
}
