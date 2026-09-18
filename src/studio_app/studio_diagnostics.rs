use super::*;
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindow};

const LOG_TAIL_BYTES: u64 = 128 * 1024;

pub(super) struct DiagnosticsView {
    content: String,
    error: Option<String>,
    last_read: Option<Instant>,
    paused: bool,
    follow: bool,
}

impl DiagnosticsView {
    pub(super) fn new(error: Option<String>) -> Self {
        Self {
            content: String::new(),
            error,
            last_read: None,
            paused: false,
            follow: true,
        }
    }

    fn refresh(&mut self) {
        if self.paused
            || self
                .last_read
                .is_some_and(|last| last.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_read = Some(Instant::now());
        match crate::diagnose::read_tail(&crate::diagnose::log_path(), LOG_TAIL_BYTES) {
            Ok(content) => {
                self.content = content;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Unable to read diagnostic log: {error}")),
        }
    }
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
        Self::page_header(ui, language.text("Diagnostics"),
            language.text("Live monitor and dashboard activity, including refresh requests and usage results."));
        ui.label(format!("Usage Monitor v{}", env!("CARGO_PKG_VERSION")));
        ui.weak(format!("Dashboard process: {}", std::process::id()));
        let connected = monitor_available(self.owner);
        ui.label(if connected {
            "Monitor connected"
        } else {
            "Monitor disconnected — open Settings from the monitor's tray menu"
        });
        ui.weak(crate::diagnose::log_path().display().to_string());
        ui.horizontal(|ui| {
            if ui.button(language.text("Refresh now")).clicked() {
                self.request_refresh();
            }
            ui.checkbox(&mut self.diagnostics.paused, language.text("Pause output"));
            ui.checkbox(&mut self.diagnostics.follow, language.text("Follow latest"));
            if ui.button(language.text("Copy output")).clicked() {
                ui.ctx().copy_text(self.diagnostics.content.clone());
            }
        });
        if let Some(error) = &self.settings_error {
            ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
        }
        self.diagnostics.refresh();
        if let Some(error) = &self.diagnostics.error {
            ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
        }
        ui.add_space(8.0);
        egui::ScrollArea::both()
            .id_salt("diagnostics_output")
            .auto_shrink([false, false])
            .stick_to_bottom(self.diagnostics.follow)
            .show(ui, |ui| {
                if self.diagnostics.content.is_empty() {
                    ui.weak("Waiting for diagnostic output");
                } else {
                    let mut text = self.diagnostics.content.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                }
            });
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_without_monitor_reports_failure_instead_of_silently_succeeding() {
        assert!(send_owner_message(0, WM_APP_REFRESH_NOW)
            .unwrap_err()
            .contains("not connected"));
        assert!(send_owner_message(-1, WM_APP_REFRESH_NOW).is_err());
    }
}
