#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use windows::core::PCWSTR;
use windows::Win32::System::Registry::*;

use crate::native_interop::wide_str;

const REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const REGISTRY_KEY: &str = "SystemUsesLightTheme";

/// Check if the system is in dark mode by reading the registry
pub fn is_dark_mode() -> bool {
    #[cfg(test)]
    if let Some(dark) = DARK_MODE_OVERRIDE.with(std::cell::Cell::get) {
        return dark;
    }
    !is_light_theme()
}

#[cfg(test)]
thread_local! {
    static DARK_MODE_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with this thread reporting the given Windows appearance, so tests
/// can render both modes without depending on the machine's setting.
#[cfg(test)]
pub fn with_dark_mode<T>(dark: bool, f: impl FnOnce() -> T) -> T {
    let previous = DARK_MODE_OVERRIDE.with(|cell| cell.replace(Some(dark)));
    let result = f();
    DARK_MODE_OVERRIDE.with(|cell| cell.set(previous));
    result
}

fn is_light_theme() -> bool {
    // SAFETY: `path` and `key_name` are NUL-terminated wide strings held by locals; `hkey` is a
    // live out pointer, closed once only after a successful open. The value buffer is the live
    // `data` u32 with `data_size` set to its size, so larger values fail rather than overflow.
    unsafe {
        let path = wide_str(REGISTRY_PATH);
        let key_name = wide_str(REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        );

        if result.is_err() {
            return false; // Default to dark mode
        }

        let mut data: u32 = 0;
        let mut data_size: u32 = std::mem::size_of::<u32>() as u32;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut data_size),
        );

        let _ = RegCloseKey(hkey);

        if result.is_err() {
            return false; // Default to dark mode
        }

        data == 1
    }
}
