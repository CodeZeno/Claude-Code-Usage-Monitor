//! Platform font discovery used by font-selection UI.

use std::sync::OnceLock;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Graphics::Gdi::{
    EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
};

pub(crate) fn installed_font_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut families = Vec::new();
        unsafe {
            let hdc = GetDC(None);
            if !hdc.is_invalid() {
                let mut query = LOGFONTW {
                    lfCharSet: DEFAULT_CHARSET,
                    ..Default::default()
                };
                EnumFontFamiliesExW(
                    hdc,
                    &raw mut query,
                    Some(collect_font_family),
                    LPARAM((&raw mut families) as isize),
                    0,
                );
                ReleaseDC(None, hdc);
            }
        }
        if families.is_empty() {
            families.extend(
                [
                    "Arial",
                    "Calibri",
                    "Consolas",
                    "Segoe UI",
                    "Times New Roman",
                ]
                .into_iter()
                .map(str::to_string),
            );
        }
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        families
    })
}

unsafe extern "system" fn collect_font_family(
    logfont: *const LOGFONTW,
    _metric: *const TEXTMETRICW,
    _font_type: u32,
    data: LPARAM,
) -> i32 {
    let Some(logfont) = logfont.as_ref() else {
        return 1;
    };
    let length = logfont
        .lfFaceName
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(logfont.lfFaceName.len());
    let name = String::from_utf16_lossy(&logfont.lfFaceName[..length]);
    if !name.is_empty() && !name.starts_with('@') {
        let families = &mut *(data.0 as *mut Vec<String>);
        families.push(name);
    }
    1
}
