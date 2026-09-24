//! Windows known-folder lookups used to locate user and provider data.
//!
//! Resolves the same folders as `SHGetKnownFolderPath`-based crates, so paths
//! follow folder redirection instead of trusting environment variables.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_RoamingAppData, SHGetKnownFolderPath,
    KNOWN_FOLDER_FLAG,
};

/// The user's profile directory, for example `C:\Users\Name`.
pub fn home_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_Profile)
}

/// The roaming application data directory (`%APPDATA%`).
pub fn roaming_app_data_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_RoamingAppData)
}

/// The local application data directory (`%LOCALAPPDATA%`).
pub fn local_app_data_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_LocalAppData)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let wide = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const _));
        (!wide.is_empty()).then(|| PathBuf::from(wide))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_absolute_user_folders() {
        for folder in [home_dir(), roaming_app_data_dir(), local_app_data_dir()] {
            let folder = folder.expect("known folder should resolve");
            assert!(folder.is_absolute(), "{}", folder.display());
        }
    }
}
