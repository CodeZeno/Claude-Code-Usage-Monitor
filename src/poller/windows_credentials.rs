use std::ffi::c_void;

use crate::diagnose;

const CRED_TYPE_GENERIC: u32 = 1;

#[repr(C)]
struct CredentialW {
    flags: u32,
    type_: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: u64,
    credential_blob_size: u32,
    credential_blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "Advapi32")]
extern "system" {
    fn CredReadW(
        target_name: *const u16,
        type_: u32,
        reserved_flags: u32,
        credential: *mut *mut CredentialW,
    ) -> i32;
    fn CredEnumerateW(
        filter: *const u16,
        flags: u32,
        count: *mut u32,
        credentials: *mut *mut *mut CredentialW,
    ) -> i32;
    fn CredFree(buffer: *mut c_void);
}

/// A generic Windows credential whose secret decoded as text.
pub(super) struct StoredCredential {
    pub target: String,
    /// FILETIME of the last write, so callers can prefer the newest login.
    pub last_written: u64,
    pub secret: String,
}

pub(super) fn read_generic(target: &str) -> Option<String> {
    let target_wide = wide(target);
    let mut credential: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    if ok == 0 || credential.is_null() {
        diagnose::log(format!(
            "unable to read Windows generic credential target {target}"
        ));
        return None;
    }

    let secret = unsafe { credential_secret(&*credential) };
    unsafe { CredFree(credential as *mut c_void) };
    secret
}

/// Every generic credential whose target starts with `prefix`. An empty
/// result covers both "none stored" and "the vault could not be read".
pub(super) fn enumerate_generic(prefix: &str) -> Vec<StoredCredential> {
    let filter = wide(&format!("{prefix}*"));
    let mut count = 0u32;
    let mut credentials: *mut *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut credentials) };
    if ok == 0 || credentials.is_null() {
        return Vec::new();
    }

    let mut found = Vec::new();
    unsafe {
        for &credential in std::slice::from_raw_parts(credentials, count as usize) {
            let Some(credential) = credential.as_ref() else {
                continue;
            };
            if credential.type_ != CRED_TYPE_GENERIC {
                continue;
            }
            let (Some(target), Some(secret)) = (
                wide_ptr_to_string(credential.target_name),
                credential_secret(credential),
            ) else {
                continue;
            };
            found.push(StoredCredential {
                target,
                last_written: credential.last_written,
                secret,
            });
        }
        CredFree(credentials as *mut c_void);
    }
    found
}

unsafe fn credential_secret(credential: &CredentialW) -> Option<String> {
    if credential.credential_blob_size == 0 || credential.credential_blob.is_null() {
        return None;
    }
    decode_secret(std::slice::from_raw_parts(
        credential.credential_blob,
        credential.credential_blob_size as usize,
    ))
}

/// Most tools store UTF-8, but some Node keyrings write the UTF-16 that
/// `CredWriteW` callers naturally hold. ASCII secrets in UTF-16LE have a zero
/// in every high byte, which UTF-8 text never does.
fn decode_secret(bytes: &[u8]) -> Option<String> {
    if bytes.len() >= 2
        && bytes.len().is_multiple_of(2)
        && bytes.iter().skip(1).step_by(2).all(|byte| *byte == 0)
    {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units).ok();
    }
    String::from_utf8(bytes.to_vec()).ok()
}

unsafe fn wide_ptr_to_string(value: *const u16) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut length = 0;
    while *value.add(length) != 0 {
        length += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(value, length)).ok()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_decode_from_utf8_and_utf16() {
        assert_eq!(decode_secret(b"gho_token").as_deref(), Some("gho_token"));
        let utf16 = "gho_token"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(decode_secret(&utf16).as_deref(), Some("gho_token"));
        assert_eq!(
            decode_secret(br#"{"token":{"access_token":"a"}}"#).as_deref(),
            Some(r#"{"token":{"access_token":"a"}}"#)
        );
        assert!(decode_secret(&[0xff, 0xfe, 0xfd]).is_none());
    }
}
