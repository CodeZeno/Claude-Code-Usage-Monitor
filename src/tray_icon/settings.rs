//! Narrow migration of Windows 11's legacy, position-based tray cache.
//! This cache is undocumented: missing or unfamiliar values are left alone.
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

const CACHE_PATH: &str = r"Control Panel\NotifyIconSettings";

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn open(parent: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<Key> {
    let path = wide(path);
    let mut key = HKEY::default();
    unsafe {
        RegOpenKeyExW(parent, PCWSTR(path.as_ptr()), None, access, &mut key)
            .ok()
            .ok()?;
    }
    Some(Key(key))
}

#[cfg(test)]
fn create(parent: HKEY, path: &str) -> Option<Key> {
    let path = wide(path);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            parent,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut key,
            None,
        )
        .ok()
        .ok()?;
    }
    Some(Key(key))
}

fn string(key: HKEY, name: &str) -> Option<String> {
    let name = wide(name);
    // Paths and GUIDs are small. Reject unexpected types, excessive lengths,
    // invalid UTF-16 and races rather than making a destructive guess.
    let mut buffer = vec![0u16; 32_768];
    let mut bytes = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .ok()
        .ok()?;
    }
    if bytes < 2 || !bytes.is_multiple_of(2) {
        return None;
    }
    let value = String::from_utf16(&buffer[..bytes as usize / 2]).ok()?;
    Some(value.trim_end_matches('\0').to_owned())
}

fn has_value(key: HKEY, name: &str) -> bool {
    let name = wide(name);
    // Treat errors other than an absent value as a reason to preserve the entry.
    unsafe {
        RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, None) != ERROR_FILE_NOT_FOUND
    }
}

fn path_key(path: &str) -> String {
    // Explorer can store executable paths relative to a Windows known-folder
    // GUID (notably Program Files for WinGet installations).
    let expanded = path.strip_prefix('{').and_then(|rest| {
        let (folder, suffix) = rest.split_once('}')?;
        let guid = windows::core::GUID::try_from(folder).ok()?;
        unsafe {
            let value = windows::Win32::UI::Shell::SHGetKnownFolderPath(
                &guid,
                windows::Win32::UI::Shell::KNOWN_FOLDER_FLAG(0),
                None,
            )
            .ok()?;
            let folder = value.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(value.0.cast()));
            folder.map(|folder| format!("{folder}{suffix}"))
        }
    });
    let path = expanded.as_deref().unwrap_or(path);
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('/', "\\")
        .to_lowercase()
}

fn owned_legacy_entry(key: HKEY, executable: &str) -> bool {
    !has_value(key, "IconGuid")
        && dword(key, "UID").is_some()
        && string(key, "ExecutablePath").is_some_and(|path| path_key(&path) == path_key(executable))
}

// A complete snapshot across sessions avoids removing another running copy's
// preferences. If a matching process cannot be inspected, skip cross-path cleanup.
fn running_copies() -> Option<Vec<String>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let result = (|| {
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            Process32FirstW(snapshot, &mut entry).ok()?;
            let mut paths = Vec::new();
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|ch| *ch == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("claude-code-usage-monitor.exe")
                {
                    let handle = OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ProcessID,
                    )
                    .ok()?;
                    let mut buffer = vec![0u16; 32_768];
                    let mut length = buffer.len() as u32;
                    let queried = QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_WIN32,
                        PWSTR(buffer.as_mut_ptr()),
                        &mut length,
                    );
                    let _ = CloseHandle(handle);
                    queried.ok()?;
                    paths.push(path_key(
                        &String::from_utf16(&buffer[..length as usize]).ok()?,
                    ));
                }
                if let Err(error) = Process32NextW(snapshot, &mut entry) {
                    if error.code() != ERROR_NO_MORE_FILES.to_hresult() {
                        return None;
                    }
                    break;
                }
            }
            Some(paths)
        })();
        let _ = CloseHandle(snapshot);
        result
    }
}

fn verified_monitor(path: &str) -> bool {
    unsafe {
        let path = wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if size == 0 || size > 1024 * 1024 {
            return false;
        }
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        if GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, data.as_mut_ptr().cast()).is_err()
        {
            return false;
        }
        let mut translations = std::ptr::null_mut();
        let mut length = 0;
        if !VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(wide(r"\VarFileInfo\Translation").as_ptr()),
            &mut translations,
            &mut length,
        )
        .as_bool()
            || translations.is_null()
            || length < 4
            || length > size
        {
            return false;
        }
        std::slice::from_raw_parts(translations.cast::<u16>(), length as usize / 2)
            .chunks_exact(2)
            .any(|pair| {
                [
                    ("CompanyName", "Code Zeno Pty Ltd"),
                    ("ProductName", "Claude Code Usage Monitor"),
                ]
                .into_iter()
                .all(|(name, expected)| {
                    let query = wide(&format!(
                        r"\StringFileInfo\{:04x}{:04x}\{name}",
                        pair[0], pair[1]
                    ));
                    let mut value = std::ptr::null_mut();
                    let mut chars = 0;
                    VerQueryValueW(
                        data.as_ptr().cast(),
                        PCWSTR(query.as_ptr()),
                        &mut value,
                        &mut chars,
                    )
                    .as_bool()
                        && !value.is_null()
                        && chars > 0
                        && chars as usize <= data.len() * 2
                        && String::from_utf16(std::slice::from_raw_parts(
                            value.cast::<u16>(),
                            chars as usize,
                        ))
                        .is_ok_and(|text| text.trim_end_matches('\0') == expected)
                })
            })
    }
}

fn stale_copy(path: &str, running: &[String]) -> bool {
    let path = path_key(path);
    if path
        .rsplit('\\')
        .next()
        .is_none_or(|name| name != "claude-code-usage-monitor.exe")
        || running.contains(&path)
    {
        return false;
    }
    match std::fs::metadata(&path) {
        Ok(_) => verified_monitor(&path),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn dword(key: HKEY, name: &str) -> Option<u32> {
    let name = wide(name);
    let mut value = 0u32;
    let mut bytes = 4;
    unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut bytes),
        )
        .ok()
        .ok()?;
    }
    Some(value)
}

fn numeric_subkeys(cache: HKEY) -> Option<Vec<String>> {
    // Snapshot names first: deleting while enumerating skips adjacent entries.
    let mut names = Vec::new();
    for index in 0.. {
        let mut buffer = [0u16; 256];
        let mut length = buffer.len() as u32;
        let result = unsafe {
            RegEnumKeyExW(
                cache,
                index,
                Some(PWSTR(buffer.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            break;
        }
        if result.is_err() {
            return None;
        }
        if let Ok(name) = String::from_utf16(&buffer[..length as usize]) {
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                names.push(name);
            }
        }
    }
    Some(names)
}

fn binary(key: HKEY, name: &str) -> Option<Vec<u8>> {
    let name = wide(name);
    let mut length = 0;
    unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut length),
        )
        .ok()
        .ok()?;
        if length > 2 * 1024 * 1024 {
            return None;
        }
        let mut value = vec![0; length as usize];
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut length),
        )
        .ok()
        .ok()?;
        value.truncate(length as usize);
        Some(value)
    }
}

fn set_binary(key: HKEY, name: &str, bytes: &[u8]) -> bool {
    let name = wide(name);
    unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_BINARY, Some(bytes)).is_ok() }
}

fn prune_order(cache: HKEY, removed: &[u64]) {
    let Some(order) = binary(cache, "UIOrderList") else {
        return;
    };
    // Never reset the shared ordering list or infer ownership from a missing
    // key. Remove only IDs whose app-owned entries we actually deleted.
    if !order.len().is_multiple_of(8) {
        return;
    }
    let retained: Vec<u8> = order
        .chunks_exact(8)
        .filter(|chunk| !removed.contains(&u64::from_le_bytes((*chunk).try_into().unwrap())))
        .flatten()
        .copied()
        .collect();
    if retained != order {
        set_binary(cache, "UIOrderList", &retained);
    }
}

fn update_snapshots(cache: HKEY, executable: &str, snapshots: &[(GUID, &[u8])]) {
    let Some(names) = numeric_subkeys(cache) else {
        return;
    };
    for name in names {
        let Some(entry) = open(cache, &name, KEY_READ | KEY_SET_VALUE) else {
            continue;
        };
        if string(entry.0, "ExecutablePath")
            .is_none_or(|path| path_key(&path) != path_key(executable))
        {
            continue;
        }
        let Some(guid) = string(entry.0, "IconGuid")
            .and_then(|value| GUID::try_from(value.trim_matches(['{', '}'])).ok())
        else {
            continue;
        };
        if let Some((_, png)) = snapshots.iter().find(|(id, _)| *id == guid) {
            if binary(entry.0, "IconSnapshot").as_deref() != Some(*png) {
                set_binary(entry.0, "IconSnapshot", png);
            }
        }
    }
}

pub(super) fn sync_provider_snapshots(snapshots: &[(GUID, &[u8])]) {
    if snapshots.is_empty() {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(cache) = open(HKEY_CURRENT_USER, CACHE_PATH, KEY_READ) else {
        return;
    };
    update_snapshots(cache.0, &executable.to_string_lossy(), snapshots);
}

#[cfg(test)]
fn migrate(cache: HKEY, executable: &str) -> usize {
    migrate_with(cache, executable, |_| false)
}

fn migrate_with(cache: HKEY, executable: &str, mut stale: impl FnMut(&str) -> bool) -> usize {
    let Some(names) = numeric_subkeys(cache) else {
        return 0;
    };
    let mut removed = 0;
    let mut removed_ids = Vec::new();
    for name in names {
        let Some(entry) = open(cache, &name, KEY_READ) else {
            continue;
        };
        let old_copy = string(entry.0, "ExecutablePath")
            .is_some_and(|path| path_key(&path) != path_key(executable) && stale(&path));
        if !owned_legacy_entry(entry.0, executable) && !old_copy {
            continue;
        }
        // Delete only this leaf; unexpected
        // children make RegDeleteKey fail instead of deleting additional data.
        drop(entry);
        let Ok(id) = name.parse::<u64>() else {
            continue;
        };
        let key_name = wide(&name);
        if unsafe { RegDeleteKeyW(cache, PCWSTR(key_name.as_ptr())) }.is_ok() {
            removed += 1;
            removed_ids.push(id);
        }
    }
    prune_order(cache, &removed_ids);
    removed
}

/// Run after acquiring the instance mutex and before registering any new icons.
pub(crate) fn cleanup_legacy_entries() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(cache) = open(HKEY_CURRENT_USER, CACHE_PATH, KEY_READ | KEY_WRITE) else {
        return;
    };
    let running = running_copies();
    let mut checked = std::collections::HashMap::new();
    let removed = migrate_with(cache.0, &executable.to_string_lossy(), |path| {
        *checked.entry(path.to_owned()).or_insert_with(|| {
            running
                .as_ref()
                .is_some_and(|running| stale_copy(path, running))
        })
    });
    crate::diagnose::log(format!(
        "tray cache: removed {removed} legacy entries for this executable"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_string(key: HKEY, name: &str, value: &str) {
        let name = wide(name);
        let data: Vec<_> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
        unsafe {
            RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data))
                .ok()
                .unwrap();
        }
    }

    #[test]
    fn cleanup_removes_only_legacy_entries_for_this_exact_executable() {
        let path = format!(
            r"Software\CodeZeno\Tests\TrayMigration-{}",
            std::process::id()
        );
        struct Fixture(String);
        impl Drop for Fixture {
            fn drop(&mut self) {
                unsafe {
                    let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&self.0).as_ptr()));
                }
            }
        }
        let _fixture = Fixture(path.clone());
        let root = create(HKEY_CURRENT_USER, &path).unwrap();
        let cache = create(root.0, "cache").unwrap();
        for (name, executable, guid) in [
            ("1", r"C:\app\monitor.exe", false),
            ("2", r"c:\APP\monitor.exe", false),
            ("3", r"C:\other\monitor.exe", false),
            ("4", r"C:\app\monitor.exe", true),
            ("6", r"C:\running\monitor.exe", true),
        ] {
            let entry = create(cache.0, name).unwrap();
            set_string(entry.0, "ExecutablePath", executable);
            unsafe {
                RegSetValueExW(
                    entry.0,
                    PCWSTR(wide("UID").as_ptr()),
                    None,
                    REG_DWORD,
                    Some(&1002u32.to_le_bytes()),
                )
                .ok()
                .unwrap();
            }
            set_string(entry.0, "IsPromoted", "1");
            if guid {
                set_string(
                    entry.0,
                    "IconGuid",
                    "{2AD06097-B0FE-0E3A-8790-C274861A5CED}",
                );
                set_binary(entry.0, "IconSnapshot", b"old image");
            }
        }
        let unknown = create(cache.0, "5").unwrap();
        set_string(unknown.0, "ExecutablePath", r"C:\app\monitor.exe");
        let order: Vec<_> = [1u64, 2, 3, 4, 5, 6, 999]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect();
        set_binary(cache.0, "UIOrderList", &order);
        assert_eq!(migrate(cache.0, r"C:\app\monitor.exe"), 2);
        let expected: Vec<_> = [3u64, 4, 5, 6, 999]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect();
        assert_eq!(binary(cache.0, "UIOrderList").unwrap(), expected);
        for name in ["1", "2"] {
            assert!(open(cache.0, name, KEY_READ).is_none());
        }
        for name in ["3", "4", "5", "6"] {
            assert!(open(cache.0, name, KEY_READ).is_some());
        }
        assert_eq!(migrate(cache.0, r"C:\app\monitor.exe"), 0);
        let guid = GUID::from_u128(0x2ad06097_b0fe_0e3a_8790_c274861a5ced);
        let png =
            super::super::provider_artwork::provider_snapshot(crate::providers::ProviderId::Codex)
                .unwrap();
        update_snapshots(cache.0, r"C:\app\monitor.exe", &[(guid, png)]);
        let current = open(cache.0, "4", KEY_READ).unwrap();
        assert_eq!(binary(current.0, "IconSnapshot").unwrap(), png);
        assert_eq!(string(current.0, "IsPromoted").as_deref(), Some("1"));
        let other = open(cache.0, "6", KEY_READ).unwrap();
        assert_eq!(binary(other.0, "IconSnapshot").unwrap(), b"old image");
        assert_eq!(
            migrate_with(cache.0, r"C:\app\monitor.exe", |path| path
                == r"C:\other\monitor.exe"),
            1
        );
        assert!(open(cache.0, "3", KEY_READ).is_none());
        assert!(open(cache.0, "6", KEY_READ).is_some());
        set_binary(cache.0, "UIOrderList", &[1, 2, 3]);
        prune_order(cache.0, &[1]);
        assert_eq!(binary(cache.0, "UIOrderList").unwrap(), [1, 2, 3]);
    }

    #[test]
    fn retired_installations_are_identified_from_their_product_metadata() {
        assert!(verified_monitor(
            &std::env::current_exe().unwrap().to_string_lossy()
        ));
        assert!(!verified_monitor(r"C:\file-that-does-not-exist.exe"));
        assert!(!stale_copy(r"C:\missing\unrelated.exe", &[]));
        let running = path_key(r"C:\old\claude-code-usage-monitor.exe");
        assert!(!stale_copy(&running, std::slice::from_ref(&running)));
    }
}
