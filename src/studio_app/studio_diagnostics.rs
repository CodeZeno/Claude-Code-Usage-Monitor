use super::*;
use crate::ui::components::layout::{setting_row_with_control_width, settings_group};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindow};

const LOG_TAIL_BYTES: u64 = 128 * 1024;

pub(super) struct DiagnosticsView {
    content: String,
    display_content: String,
    error: Option<String>,
    last_read: Option<Instant>,
    follow: bool,
    recording_error: Option<String>,
}

impl DiagnosticsView {
    pub(super) fn new() -> Self {
        Self {
            content: String::new(),
            display_content: String::new(),
            error: None,
            last_read: None,
            follow: true,
            recording_error: None,
        }
    }

    fn refresh(&mut self, recording: bool) {
        if !recording
            || !self.follow
            || self
                .last_read
                .is_some_and(|last| last.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_read = Some(Instant::now());
        match crate::diagnose::read_tail(&crate::diagnose::log_path(), LOG_TAIL_BYTES) {
            Ok(content) => {
                self.display_content = format_log_for_display(&content);
                self.content = content;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Unable to read diagnostic log: {error}")),
        }
    }
}

// Keep the stored and copied log untouched; local timestamps belong only to the viewer.
fn format_log_for_display(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let timestamp = line.strip_prefix('[').and_then(|line| line.split_once(']'));
        if let Some((local, rest)) = timestamp
            .and_then(|(seconds, rest)| Some((local_log_timestamp(seconds.parse().ok()?)?, rest)))
        {
            output.push_str(&local);
            output.push_str(rest);
        } else {
            output.push_str(line);
        }
    }
    output
}

fn local_log_timestamp(seconds: u64) -> Option<String> {
    let ticks = seconds
        .checked_add(11_644_473_600)?
        .checked_mul(10_000_000)?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).ok()?;
    }
    Some(format!(
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}]",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    ))
}

fn set_recording(owner: isize, enabled: bool) -> Result<(), String> {
    let message = if enabled {
        crate::diagnose::init_append()?;
        crate::diagnose::log("diagnostic recording enabled from dashboard");
        native_interop::WM_APP_ENABLE_DIAGNOSTICS
    } else {
        crate::diagnose::disable();
        native_interop::WM_APP_DISABLE_DIAGNOSTICS
    };
    send_owner_message(owner, message)
}

fn monitor_available(owner: isize) -> bool {
    if owner == 0 {
        return false;
    }
    let hwnd = HWND(owner as *mut _);
    let mut class_name = [0u16; 64];
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        let length = GetClassNameW(hwnd, &mut class_name).max(0) as usize;
        String::from_utf16_lossy(&class_name[..length]) == "ClaudeCodeUsageMonitor"
    }
}

pub(super) fn send_owner_message(owner: isize, message: u32) -> Result<(), String> {
    if !monitor_available(owner) {
        return Err("Dashboard is not connected to a running monitor. Open Settings from the monitor's tray menu.".into());
    }
    unsafe {
        PostMessageW(Some(HWND(owner as *mut _)), message, WPARAM(0), LPARAM(0))
            .map_err(|error| format!("Unable to send request to monitor: {error}"))
    }
}

impl StudioApp {
    pub(super) fn diagnostics_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let mut recording = crate::diagnose::is_enabled();
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(32, 8))
            .show(ui, |ui| {
                settings_group(ui, |ui| {
                    let description = language
                        .text("Write diagnostic events to {log path}")
                        .replace(
                            "{log path}",
                            &crate::diagnose::log_path().display().to_string(),
                        );
                    setting_row_with_control_width(
                        ui,
                        language.text("Logging"),
                        &description,
                        134.0,
                        |ui| {
                            if Toggle::new(&mut recording)
                                .labels(language.text("Enabled"), language.text("Disabled"))
                                .show(ui)
                                .changed()
                            {
                                self.diagnostics.recording_error =
                                    set_recording(self.owner, recording).err();
                                recording = crate::diagnose::is_enabled();
                                self.diagnostics.last_read = None;
                            }
                        },
                    );
                    if let Some(error) = &self.diagnostics.recording_error {
                        ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
                    }
                });
                ui.add_space(18.0);
                settings_group(ui, |ui| {
                    let mut resume_follow = false;
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(lucide_labeled_button(
                                LucideIcon::RefreshCw,
                                language.text("Refresh usage"),
                            ))
                            .clicked()
                        {
                            self.request_refresh();
                        }
                        if ui
                            .add(
                                lucide_labeled_button(
                                    LucideIcon::ArrowDownToLine,
                                    language.text("Follow latest event"),
                                )
                                .selected(self.diagnostics.follow),
                            )
                            .clicked()
                        {
                            self.diagnostics.follow = !self.diagnostics.follow;
                            self.diagnostics.last_read = None;
                            resume_follow = self.diagnostics.follow;
                        }
                        self.diagnostics.refresh(recording);
                        if ui
                            .add(lucide_labeled_button(
                                LucideIcon::Copy,
                                language.text("Copy log"),
                            ))
                            .clicked()
                        {
                            ui.ctx().copy_text(self.diagnostics.content.clone());
                        }
                    });
                    ui.add_space(8.0);
                    setting_separator(ui);
                    ui.add_space(8.0);
                    if let Some(error) = &self.settings_error {
                        ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
                    }
                    if let Some(error) = &self.diagnostics.error {
                        ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
                    }
                    let mut scroll = egui::ScrollArea::both()
                        .id_salt("diagnostics_output")
                        .auto_shrink([false, false])
                        .max_height(ui.available_height())
                        .stick_to_bottom(self.diagnostics.follow);
                    if resume_follow {
                        scroll = scroll.vertical_scroll_offset(f32::INFINITY);
                    }
                    scroll.show(ui, |ui| {
                        let mut text = self.diagnostics.display_content.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
            });
        if recording && self.diagnostics.follow {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_formats_event_time_in_local_timezone_and_preserves_log_text() {
        let utc = SYSTEMTIME {
            wYear: 1970,
            wMonth: 1,
            wDay: 2,
            ..Default::default()
        };
        let mut local = SYSTEMTIME::default();
        unsafe { SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).unwrap() };
        let raw = "[86400] [pid 42] first\r\n[86400] [pid 42] second";
        let prefix = format!(
            "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}]",
            local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
        );
        assert_eq!(
            format_log_for_display(raw),
            format!("{prefix} [pid 42] first\r\n{prefix} [pid 42] second")
        );
        let unrecognized = "continuation\n[invalid] event\n[18446744073709551615] event\n";
        assert_eq!(format_log_for_display(unrecognized), unrecognized);
        assert_eq!(format_log_for_display(""), "");
    }

    #[test]
    fn disabled_recording_does_not_read_logs_or_discard_visible_output() {
        let mut view = DiagnosticsView::new();
        view.refresh(false);
        assert!(view.last_read.is_none());
        view.content = "last result".into();
        view.display_content = "visible result".into();
        view.refresh(false);
        assert_eq!(view.content, "last result");
        assert_eq!(view.display_content, "visible result");
        assert!(view.last_read.is_none());
        view.follow = false;
        view.refresh(true);
        assert!(view.last_read.is_none());
        assert_eq!(view.content, "last result");
        assert_eq!(view.display_content, "visible result");
    }

    #[test]
    fn refresh_without_monitor_reports_failure_instead_of_silently_succeeding() {
        assert!(send_owner_message(0, WM_APP_REFRESH_NOW)
            .unwrap_err()
            .contains("not connected"));
        assert!(send_owner_message(-1, WM_APP_REFRESH_NOW).is_err());
    }
}
