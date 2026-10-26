//! Explorer-style native tooltips, owned and accessed only by the window thread.
use std::cell::RefCell;

use windows::core::PWSTR;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, TOOLTIPS_CLASSW,
    TTF_TRACK, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION,
    TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};

use super::*;

pub(super) const REFRESH_TIMER: usize = 15;

thread_local! {
    static TOOLTIP: RefCell<Option<WidgetTooltip>> = const { RefCell::new(None) };
}

struct WidgetTooltip {
    window: HWND,
    owner: HWND,
    controller: HWND,
    text: Vec<u16>,
    dark: bool,
}

impl WidgetTooltip {
    fn tool_info(&mut self) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_TRACK,
            hwnd: self.owner,
            uId: 1,
            lpszText: PWSTR(self.text.as_mut_ptr()),
            ..Default::default()
        }
    }
}

impl Drop for WidgetTooltip {
    fn drop(&mut self) {
        unsafe {
            let _ = kill_controller_timer(Some(self.controller), REFRESH_TIMER);
            let _ = DestroyWindow(self.window);
        }
    }
}

pub(super) fn reset_countdown(reset: SystemTime, now: SystemTime, strings: &Strings) -> String {
    let seconds = reset.duration_since(now).unwrap_or_default().as_secs();
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    if days > 0 {
        format!(
            "{days}{} {hours}{}",
            strings.day_suffix, strings.hour_suffix
        )
    } else if hours > 0 {
        format!(
            "{hours}{} {minutes}{}",
            strings.hour_suffix, strings.minute_suffix
        )
    } else if minutes > 0 {
        format!("{minutes}{}", strings.minute_suffix)
    } else if seconds > 0 {
        format!("{seconds}{}", strings.second_suffix)
    } else {
        strings.now.to_string()
    }
}

fn content(owner: HWND) -> Option<(String, bool, HWND)> {
    let state = lock_state();
    let state = state.as_ref()?;
    if state.dragging || state.pending_drag {
        return None;
    }
    let surface_index = surface_index_for_window(state, owner)?;
    let theme = effective_theme_from_state(state)?;
    let surface = theme.surfaces.get(surface_index)?;
    let runtime = theme_runtime_for_surface(&theme, surface_index, theme_runtime_from_state(state));
    if surface.tooltip != Some(theme_engine::SurfaceTooltip::UsageSummary)
        || !theme_engine::surface_should_render(&theme, surface_index, state.data.as_ref(), runtime)
    {
        return None;
    }
    let errors = tray_error_lines(
        state.data.as_ref(),
        state.last_poll_failure,
        state.providers,
        state.language,
    );
    let text = if !errors.is_empty() {
        errors.join("\n")
    } else if let Some(data) = state.data.as_ref().filter(|_| state.last_poll_ok) {
        usage_summary_lines(
            data,
            state.providers,
            state.language,
            state.usage_countdown,
            state.show_antigravity_gemini,
            state.show_antigravity_external,
            Some(SystemTime::now()),
        )
        .join("\n")
    } else {
        String::new()
    };
    Some((
        if text.is_empty() {
            state.language.strings().window_title.to_string()
        } else {
            text
        },
        theme::is_dark_mode(),
        controller_for(state.hwnd.to_hwnd()),
    ))
}

fn set_theme(window: HWND, dark: bool) {
    let class = native_interop::wide_str(if dark {
        "DarkMode_Explorer"
    } else {
        "Explorer"
    });
    unsafe {
        let _ = SetWindowTheme(window, PCWSTR(class.as_ptr()), PCWSTR::null());
    }
}

pub(super) fn show(owner: HWND) {
    hide();
    let Some(cursor) = cursor_over(owner) else {
        return;
    };
    let Some((text, dark, controller)) = content(owner) else {
        return;
    };
    unsafe {
        let Ok(window) = create_window(owner) else {
            diagnose::log("unable to create widget tooltip");
            return;
        };
        let mut tooltip = WidgetTooltip {
            window,
            owner,
            controller,
            text: native_interop::wide_str(&text),
            dark,
        };
        set_theme(window, dark);
        let info = tooltip.tool_info();
        if SendMessageW(
            window,
            TTM_ADDTOOLW,
            None,
            Some(LPARAM(&info as *const _ as isize)),
        )
        .0 == 0
        {
            diagnose::log("unable to register widget tooltip");
            return;
        }
        let dpi = GetDpiForWindow(owner).max(96);
        SendMessageW(
            window,
            TTM_SETMAXTIPWIDTH,
            None,
            Some(LPARAM((640 * dpi / 96) as isize)),
        );
        let x = cursor.x;
        let y = cursor.y + (20 * dpi / 96) as i32;
        let packed = (x as u32 & 0xffff) | ((y as u32 & 0xffff) << 16);
        SendMessageW(
            window,
            TTM_TRACKPOSITION,
            None,
            Some(LPARAM(packed as isize)),
        );
        SendMessageW(
            window,
            TTM_TRACKACTIVATE,
            Some(WPARAM(1)),
            Some(LPARAM(&info as *const _ as isize)),
        );
        set_controller_timer(Some(controller), REFRESH_TIMER, 1_000, None);
        store(tooltip);
    }
}

fn create_window(owner: HWND) -> windows::core::Result<HWND> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        if !InitCommonControlsEx(&controls).as_bool() {
            return Err(windows::core::Error::from_thread());
        }
        // Common Controls v6 supplies the native frame, font, shadow and
        // screen-edge placement used by Explorer's tray tooltips.
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0 | TTS_ALWAYSTIP | TTS_NOPREFIX),
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            None,
            None,
        )
    }
}

pub(super) fn hide() {
    // Drop outside the RefCell borrow: native destruction can re-enter wnd_proc.
    let previous = TOOLTIP.with(|slot| slot.borrow_mut().take());
    drop(previous);
}

pub(super) fn visible_for(owner: HWND) -> bool {
    TOOLTIP.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|tooltip| tooltip.owner == owner)
    })
}

pub(super) fn hide_for(owner: HWND) {
    let previous = TOOLTIP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|tooltip| tooltip.owner == owner) {
            slot.take()
        } else {
            None
        }
    });
    drop(previous);
}

pub(super) fn refresh() {
    let Some(mut tooltip) = TOOLTIP.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    if cursor_over(tooltip.owner).is_none() {
        return;
    }
    let Some((text, dark, _)) = content(tooltip.owner) else {
        return;
    };
    if tooltip.dark != dark {
        set_theme(tooltip.window, dark);
        tooltip.dark = dark;
    }
    let text = native_interop::wide_str(&text);
    if tooltip.text != text {
        let previous = std::mem::replace(&mut tooltip.text, text);
        let info = tooltip.tool_info();
        unsafe {
            SendMessageW(
                tooltip.window,
                TTM_UPDATETIPTEXTW,
                None,
                Some(LPARAM(&info as *const _ as isize)),
            );
        }
        drop(previous);
    }
    store(tooltip);
}

fn store(tooltip: WidgetTooltip) {
    // A popup shown by a reentrant call is replaced, and destroyed outside the
    // RefCell borrow for the same reason as `hide`.
    let replaced = TOOLTIP.with(|slot| slot.borrow_mut().replace(tooltip));
    drop(replaced);
}

fn cursor_over(owner: HWND) -> Option<POINT> {
    unsafe {
        let mut cursor = POINT::default();
        (IsWindowVisible(owner).as_bool()
            && GetCursorPos(&mut cursor).is_ok()
            && WindowFromPoint(cursor) == owner)
            .then_some(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_popup_accepts_multiline_text_in_both_modes_and_cleans_up() {
        use windows::Win32::UI::Controls::{TTM_GETTEXTW, TTM_GETTOOLCOUNT};
        unsafe {
            let owner = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let focus = GetForegroundWindow();
            let text = "Claude Code 5h: 12.3% (3h 12m) | 7d: 45.6% (2d 3h)\n".repeat(8);
            for dark in [false, true] {
                let window = create_window(owner).unwrap();
                let mut tooltip = WidgetTooltip {
                    window,
                    owner,
                    controller: owner,
                    text: native_interop::wide_str(&text),
                    dark,
                };
                set_theme(window, dark);
                let mut info = tooltip.tool_info();
                assert_ne!(
                    SendMessageW(
                        window,
                        TTM_ADDTOOLW,
                        None,
                        Some(LPARAM(&info as *const _ as isize))
                    )
                    .0,
                    0
                );
                assert_eq!(SendMessageW(window, TTM_GETTOOLCOUNT, None, None).0, 1);
                let mut buffer = vec![0u16; tooltip.text.len()];
                info.lpszText = PWSTR(buffer.as_mut_ptr());
                SendMessageW(
                    window,
                    TTM_GETTEXTW,
                    Some(WPARAM(buffer.len())),
                    Some(LPARAM(&mut info as *mut _ as isize)),
                );
                assert_eq!(buffer, tooltip.text);
                assert_eq!(GetForegroundWindow(), focus);
                drop(tooltip);
                assert!(!IsWindow(Some(window)).as_bool());
            }
            let _ = DestroyWindow(owner);
        }
    }

    #[test]
    fn countdown_keeps_two_units_and_handles_missing_time() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let strings = LanguageId::English.strings();
        for (seconds, expected) in [
            (183_600, "2d 3h"),
            (11_520, "3h 12m"),
            (86_400, "1d 0h"),
            (3_600, "1h 0m"),
            (65, "1m"),
            (59, "59s"),
            (0, "now"),
        ] {
            assert_eq!(
                reset_countdown(now + Duration::from_secs(seconds), now, &strings),
                expected
            );
        }
        assert_eq!(
            reset_countdown(now - Duration::from_secs(1), now, &strings),
            "now"
        );
    }
}
