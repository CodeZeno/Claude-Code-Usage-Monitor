//! The start-with-Windows registry entry.
#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use super::*;

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";

/// Run-key command for an executable. Always quote it: Windows otherwise
/// resolves an unquoted path containing spaces by trying each prefix, so
/// `C:\Choco Lab Root\...` could launch `C:\Choco.exe` at sign-in.
fn startup_command(executable: &str) -> String {
    format!("\"{executable}\"")
}

/// Whether a Run-key command launches exactly this executable, accepting the
/// unquoted form written before 2.25.0. Commands with arguments are not ours.
fn startup_command_targets(command: &str, executable: &str) -> bool {
    let command = command.trim();
    let target = command
        .strip_prefix('"')
        .and_then(|quoted| quoted.strip_suffix('"'))
        .unwrap_or(command);
    !target.contains('"') && target.eq_ignore_ascii_case(executable)
}

fn read_startup_command() -> Option<String> {
    // SAFETY: the key path and value name are NUL-terminated buffers that outlive every
    // call; `hkey` is only used after RegOpenKeyExW succeeds and is closed once.
    // The second query passes `buf` with `data_size` equal to its size in bytes,
    // and Windows only shrinks `data_size`, so at most `buf.len()` u16s are read.
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            return None;
        }

        // Query the size of the value
        let mut data_size: u32 = 0;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_size),
        );
        if result.is_err() || data_size == 0 {
            let _ = RegCloseKey(hkey);
            return None;
        }

        // Read the value into a u16 buffer, so the UTF-16 view is aligned.
        let mut buf = vec![0u16; (data_size as usize).div_ceil(2)];
        data_size = (buf.len() * 2) as u32;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut data_size),
        );
        let _ = RegCloseKey(hkey);
        if result.is_err() {
            return None;
        }

        // Convert the registry value (UTF-16) to a string
        let wide_slice = &buf[..(data_size as usize / 2).min(buf.len())];
        Some(
            String::from_utf16_lossy(wide_slice)
                .trim_end_matches('\0')
                .to_string(),
        )
    }
}

fn current_executable_path() -> Option<String> {
    let mut exe_buf = [0u16; 260];
    // SAFETY: the windows crate passes the slice pointer and its exact length, and
    // `exe_buf` outlives the call; a None module means this executable.
    let len = unsafe { GetModuleFileNameW(None, &mut exe_buf) } as usize;
    (len > 0).then(|| String::from_utf16_lossy(&exe_buf[..len]))
}

/// Returns true only if the startup registry value points to this executable.
pub(crate) fn is_startup_enabled() -> bool {
    match (read_startup_command(), current_executable_path()) {
        (Some(command), Some(executable)) => startup_command_targets(&command, &executable),
        _ => false,
    }
}

/// Rewrite this executable's legacy unquoted startup command in quoted form,
/// leaving disabled startup and other installations' commands untouched.
pub(super) fn upgrade_legacy_startup_command() {
    let (Some(command), Some(executable)) = (read_startup_command(), current_executable_path())
    else {
        return;
    };
    if command != startup_command(&executable) && startup_command_targets(&command, &executable) {
        set_startup_enabled(true);
        diagnose::log("startup command upgraded to a quoted path");
    }
}

pub(crate) fn set_startup_enabled(enable: bool) {
    // SAFETY: the key path, value name and command are NUL-terminated buffers that
    // outlive the calls; the byte view of `command` covers exactly its
    // `len() * 2` bytes, and `hkey` is used only after a successful open and
    // closed once.
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if result.is_err() {
            return;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        if enable {
            if let Some(executable) = current_executable_path() {
                // Write the wide string including null terminator
                let command = native_interop::wide_str(&startup_command(&executable));
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    None,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        command.as_ptr() as *const u8,
                        command.len() * 2,
                    )),
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }
}

#[cfg(test)]
mod tests {
    use super::{startup_command, startup_command_targets};

    const EXE: &str =
        r"C:\Choco Lab Root\lib\claude-code-usage-monitor\tools\claude-code-usage-monitor.exe";

    #[test]
    fn startup_command_quotes_paths_with_spaces() {
        let command = startup_command(EXE);
        assert_eq!(command, format!("\"{EXE}\""));
        assert!(startup_command_targets(&command, EXE));
    }

    #[test]
    fn legacy_unquoted_and_case_variant_commands_still_target_this_executable() {
        assert!(startup_command_targets(EXE, EXE));
        assert!(startup_command_targets(&EXE.to_ascii_uppercase(), EXE));
        assert!(startup_command_targets(&format!("  \"{EXE}\"  "), EXE));
    }

    #[test]
    fn other_installations_and_commands_with_arguments_are_not_ours() {
        assert!(!startup_command_targets(
            r"C:\Other\claude-code-usage-monitor.exe",
            EXE
        ));
        assert!(!startup_command_targets(
            &format!("\"{EXE}\" --dashboard"),
            EXE
        ));
        assert!(!startup_command_targets(&format!("\"{EXE}"), EXE));
        assert!(!startup_command_targets(&format!("\"\"{EXE}\"\""), EXE));
        assert!(!startup_command_targets("", EXE));
    }
}
