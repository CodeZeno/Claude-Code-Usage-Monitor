//! Peak hours configuration dialog.
//!
//! Presents a small Win32 dialog (built from an in-memory DLGTEMPLATE) that lets
//! the user configure start/end times (HH:MM) and an optional UTC offset in hours.
//! Leaving the UTC offset empty means "use system local time".

use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::localization::Strings;
use crate::native_interop::wide_str;

// ── Control IDs ───────────────────────────────────────────────────────────────

const IDC_START: i32 = 101;
const IDC_END: i32 = 102;
const IDC_TZ: i32 = 103;
const IDC_CLEAR: i32 = 104;
const IDC_SHOW_INDICATOR: i32 = 105;

// ── Thread-local dialog state (main thread only) ──────────────────────────────

struct DialogState {
    start: String,
    end: String,
    tz: String,
    show_indicator: bool,
    strings: Strings,
}

static DIALOG_INPUT: Mutex<Option<DialogState>> = Mutex::new(None);
static DIALOG_OUTPUT: Mutex<Option<(String, String, String, bool)>> = Mutex::new(None);

// ── Public API ────────────────────────────────────────────────────────────────

pub struct PeakDialogResult {
    /// "HH:MM" or empty to disable
    pub start: String,
    /// "HH:MM" or empty to disable
    pub end: String,
    /// Hours from UTC (e.g. "10"), or empty for system timezone
    pub tz: String,
    /// Whether to show the peak-hours indicator on the widget
    pub show_indicator: bool,
}

/// Show the peak-hours dialog modally over `parent`.
/// Returns `None` if the user cancelled; `Some(result)` on OK.
pub fn show(
    parent: HWND,
    start: &str,
    end: &str,
    tz: &str,
    show_indicator: bool,
    strings: Strings,
) -> Option<PeakDialogResult> {
    unsafe {
        *DIALOG_INPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(DialogState {
            start: start.to_string(),
            end: end.to_string(),
            tz: tz.to_string(),
            show_indicator,
            strings,
        });
        *DIALOG_OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = None;

        let hmodule = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let hinstance = HINSTANCE(hmodule.0);
        let template = build_template(&strings);

        let ret = DialogBoxIndirectParamW(
            hinstance,
            template.as_ptr() as *const DLGTEMPLATE,
            parent,
            Some(dlg_proc),
            LPARAM(0),
        );

        if ret <= 0 {
            return None;
        }

        DIALOG_OUTPUT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(|(s, e, t, ind)| PeakDialogResult {
                start: s,
                end: e,
                tz: t,
                show_indicator: ind,
            })
    }
}

/// Parse an "HH:MM" string into (hour, minute). Public so window.rs can reuse it.
pub fn parse_time(s: &str) -> Result<(u8, u8), ()> {
    let s = s.trim();
    let (h_str, m_str) = s.split_once(':').ok_or(())?;
    let h: u8 = h_str.trim().parse().map_err(|_| ())?;
    let m: u8 = m_str.trim().parse().map_err(|_| ())?;
    if h > 23 || m > 59 {
        return Err(());
    }
    Ok((h, m))
}

// ── Dialog procedure ──────────────────────────────────────────────────────────

unsafe extern "system" fn dlg_proc(hdlg: HWND, msg: u32, wparam: WPARAM, _lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            let input = DIALOG_INPUT.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = input.as_ref() {
                set_text(hdlg, IDC_START, &state.start);
                set_text(hdlg, IDC_END, &state.end);
                set_text(hdlg, IDC_TZ, &state.tz);
                if state.show_indicator {
                    if let Ok(ctrl) = GetDlgItem(hdlg, IDC_SHOW_INDICATOR) {
                        SendMessageW(ctrl, BM_SETCHECK, WPARAM(1), LPARAM(0)); // BST_CHECKED
                    }
                }
            }
            1 // TRUE: let dialog manager set initial focus
        }

        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            match id {
                1 /* IDOK */ => {
                    let start = get_text(hdlg, IDC_START);
                    let end   = get_text(hdlg, IDC_END);
                    let tz    = get_text(hdlg, IDC_TZ);

                    let strings = {
                        let input = DIALOG_INPUT.lock().unwrap_or_else(|e| e.into_inner());
                        input
                            .as_ref()
                            .map(|s| s.strings)
                            .unwrap_or_else(|| crate::localization::LanguageId::English.strings())
                    };

                    if let Err(err_msg) = validate(&start, &end, &tz, &strings) {
                        let msg_w   = wide_str(&err_msg);
                        let title_w = wide_str(strings.peak_invalid_input);
                        MessageBoxW(
                            hdlg,
                            PCWSTR::from_raw(msg_w.as_ptr()),
                            PCWSTR::from_raw(title_w.as_ptr()),
                            MB_OK | MB_ICONWARNING,
                        );
                        return 1;
                    }

                    *DIALOG_OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = {
                        let checked = GetDlgItem(hdlg, IDC_SHOW_INDICATOR)
                            .map(|ctrl| SendMessageW(ctrl, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1)
                            .unwrap_or(true);
                        Some((start, end, tz, checked))
                    };
                    let _ = EndDialog(hdlg, 1);
                    1
                }
                2 /* IDCANCEL */ => {
                    let _ = EndDialog(hdlg, 0);
                    1
                }
                IDC_CLEAR => {
                    set_text(hdlg, IDC_START, "");
                    set_text(hdlg, IDC_END, "");
                    set_text(hdlg, IDC_TZ, "");
                    1
                }
                _ => 0,
            }
        }

        _ => 0,
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate(start: &str, end: &str, tz: &str, strings: &Strings) -> Result<(), String> {
    if start.is_empty() != end.is_empty() {
        return Err(strings.peak_err_both_required.to_string());
    }
    if !start.is_empty() {
        let s = parse_time(start).map_err(|_| strings.peak_err_start_format.to_string())?;
        if !end.is_empty() {
            let e = parse_time(end).map_err(|_| strings.peak_err_end_format.to_string())?;
            if s == e {
                return Err(strings.peak_err_start_equals_end.to_string());
            }
        }
    }
    if !end.is_empty() {
        parse_time(end).map_err(|_| strings.peak_err_end_format.to_string())?;
    }
    if !tz.is_empty() {
        let v: i32 = tz
            .trim()
            .parse()
            .map_err(|_| strings.peak_err_tz_format.to_string())?;
        if !(-12..=14).contains(&v) {
            return Err(strings.peak_err_tz_range.to_string());
        }
    }
    Ok(())
}

// ── Edit-control helpers ──────────────────────────────────────────────────────

fn set_text(hdlg: HWND, id: i32, text: &str) {
    unsafe {
        let wide = wide_str(text);
        let _ = SetDlgItemTextW(hdlg, id, PCWSTR::from_raw(wide.as_ptr()));
    }
}

fn get_text(hdlg: HWND, id: i32) -> String {
    unsafe {
        let mut buf = [0u16; 64];
        let len = GetDlgItemTextW(hdlg, id, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
            .trim()
            .to_string()
    }
}

// ── System timezone helpers ─────────────────────────────────────────────────────

/// Returns a string like "UTC+10", "UTC-5:30", or "UTC" for the current local offset.
fn local_tz_offset_str() -> String {
    let (utc, local) = unsafe { (GetSystemTime(), GetLocalTime()) };
    let mut offset_min = (local.wHour as i32 - utc.wHour as i32) * 60
        + (local.wMinute as i32 - utc.wMinute as i32);
    // Clamp across day boundaries (offsets are always in [-12h, +14h])
    if offset_min > 14 * 60 {
        offset_min -= 24 * 60;
    } else if offset_min < -12 * 60 {
        offset_min += 24 * 60;
    }
    if offset_min == 0 {
        "UTC".to_string()
    } else {
        let sign = if offset_min > 0 { "+" } else { "-" };
        let abs = offset_min.unsigned_abs();
        let h = abs / 60;
        let m = abs % 60;
        if m == 0 {
            format!("UTC{}{}", sign, h)
        } else {
            format!("UTC{}{}:{:02}", sign, h, m)
        }
    }
}

// ── In-memory DLGTEMPLATE builder ─────────────────────────────────────────────

struct Buf(Vec<u8>);

impl Buf {
    fn new() -> Self {
        Self(Vec::with_capacity(512))
    }
    fn align4(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }
    fn u16v(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32v(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16v(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn wstr(&mut self, s: &str) {
        for c in s.encode_utf16() {
            self.u16v(c);
        }
        self.u16v(0);
    }
    fn patch_u16(&mut self, offset: usize, v: u16) {
        self.0[offset] = (v & 0xFF) as u8;
        self.0[offset + 1] = (v >> 8) as u8;
    }
}

/// Build the DLGTEMPLATE buffer for the peak-hours dialog.
fn build_template(strings: &Strings) -> Vec<u8> {
    // Numeric style constants to avoid extra imports inside this file
    const DIALOG_STYLE: u32 = 0x80000000  // WS_POPUP
        | 0x00C00000  // WS_CAPTION
        | 0x00080000  // WS_SYSMENU
        | 0x0040      // DS_SETFONT
        | 0x0080      // DS_MODALFRAME
        | 0x0800; // DS_CENTER

    const WS_VISIBLE: u32 = 0x10000000;
    const WS_CHILD: u32 = 0x40000000;
    const WS_TABSTOP: u32 = 0x00010000;
    const WS_BORDER: u32 = 0x00800000;
    const ES_AUTOHSCROLL: u32 = 0x0080;
    const BS_DEFPUSHBUTTON: u32 = 0x00000001;

    const ATOM_BUTTON: u16 = 0x0080;
    const ATOM_EDIT: u16 = 0x0081;
    const ATOM_STATIC: u16 = 0x0082;

    let mut b = Buf::new();

    // ── DLGTEMPLATE header (18 bytes fixed) ─────────────────────────────────
    b.u32v(DIALOG_STYLE);
    b.u32v(0); // exStyle
    let cdit_offset = b.0.len();
    b.u16v(0); // cdit – patched below
    b.i16v(0); // x (DS_CENTER overrides)
    b.i16v(0); // y
    b.i16v(210); // cx in dialog units
    b.i16v(115); // cy in dialog units

    b.u16v(0); // menu: none
    b.u16v(0); // class: default dialog class
    b.wstr(strings.peak_hours_title);

    // DS_SETFONT: point size + typeface name
    b.u16v(9);
    b.wstr("Segoe UI");

    // ── Item helper macro ────────────────────────────────────────────────────
    let mut n: u16 = 0;
    macro_rules! item {
        ($style:expr, $x:expr, $y:expr, $cx:expr, $cy:expr,
         $id:expr, $atom:expr, $text:expr) => {{
            b.align4();
            b.u32v($style);
            b.u32v(0u32); // exStyle
            b.i16v($x as i16);
            b.i16v($y as i16);
            b.i16v($cx as i16);
            b.i16v($cy as i16);
            b.u16v($id as u16);
            b.u16v(0xFFFF_u16); // atom-class marker
            b.u16v($atom);
            b.wstr($text);
            b.u16v(0u16); // cbExtra
            n += 1;
        }};
    }

    let lbl = WS_VISIBLE | WS_CHILD;
    let edt = WS_VISIBLE | WS_CHILD | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL;
    let btn = WS_VISIBLE | WS_CHILD | WS_TABSTOP;

    // Row 1 – start time
    item!(
        lbl,
        7,
        14,
        83,
        8,
        0xFFFF_u16,
        ATOM_STATIC,
        strings.peak_start_label
    );
    item!(edt, 93, 12, 55, 12, IDC_START as u16, ATOM_EDIT, "");
    // Row 2 – end time
    item!(
        lbl,
        7,
        30,
        83,
        8,
        0xFFFF_u16,
        ATOM_STATIC,
        strings.peak_end_label
    );
    item!(edt, 93, 28, 55, 12, IDC_END as u16, ATOM_EDIT, "");
    // Row 3 – UTC offset
    item!(
        lbl,
        7,
        46,
        83,
        8,
        0xFFFF_u16,
        ATOM_STATIC,
        strings.peak_tz_label
    );
    item!(edt, 93, 44, 55, 12, IDC_TZ as u16, ATOM_EDIT, "");
    // Hint text (with resolved system timezone appended)
    let hint_text = format!("{} [{}]", strings.peak_tz_hint, local_tz_offset_str());
    item!(
        lbl,
        7,
        59,
        196,
        8,
        0xFFFF_u16,
        ATOM_STATIC,
        &hint_text
    );
    // Show indicator checkbox
    const BS_AUTOCHECKBOX: u32 = 0x00000003;
    item!(
        btn | BS_AUTOCHECKBOX,
        7,
        73,
        196,
        10,
        IDC_SHOW_INDICATOR as u16,
        ATOM_BUTTON,
        strings.peak_show_indicator
    );
    // Buttons
    item!(
        btn,
        7,
        95,
        42,
        14,
        IDC_CLEAR as u16,
        ATOM_BUTTON,
        strings.peak_clear
    );
    item!(
        btn | BS_DEFPUSHBUTTON,
        105,
        95,
        45,
        14,
        1u16,
        ATOM_BUTTON,
        "OK"
    );
    item!(btn, 155, 95, 45, 14, 2u16, ATOM_BUTTON, "Cancel");

    b.patch_u16(cdit_offset, n);
    b.0
}
