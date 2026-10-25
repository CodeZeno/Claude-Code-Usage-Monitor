//! Windows Credential Manager integration.
//!
//! Secrets are stored as generic credentials in the per-user vault, protected
//! by DPAPI. The blob is the raw UTF-8 encoding of the secret value.

use std::fmt;

use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// Error returned by Windows Credential Manager operations.
///
/// Contains the operation name and Win32 error code; the secret value is
/// never included.
#[derive(Debug)]
pub struct SecretError {
    operation: &'static str,
    code: u32,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "credential store {} failed: Win32 error {:#010x}",
            self.operation, self.code
        )
    }
}

impl std::error::Error for SecretError {}

impl SecretError {
    fn new(operation: &'static str, code: u32) -> Self {
        Self { operation, code }
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Store `secret` under `target` in the Windows Credential Manager.
///
/// Uses `CRED_TYPE_GENERIC` and `CRED_PERSIST_LOCAL_MACHINE`. An existing
/// credential with the same target is silently overwritten.
///
/// Returns an error if `secret` is empty or contains only whitespace.
pub fn write_secret(target: &str, secret: &str) -> Result<(), SecretError> {
    if secret.trim().is_empty() {
        return Err(SecretError::new("write", 87)); // ERROR_INVALID_PARAMETER
    }

    let target_wide = to_wide_null(target);
    let blob = secret.as_bytes();

    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(target_wide.as_ptr() as *mut u16),
        Comment: windows::core::PWSTR::null(),
        LastWritten: Default::default(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: windows::core::PWSTR::null(),
        UserName: windows::core::PWSTR::null(),
    };

    unsafe { CredWriteW(&cred, 0) }.map_err(|e| SecretError::new("write", e.code().0 as u32))
}

/// Retrieve the secret stored under `target` from the Windows Credential Manager.
///
/// Returns `Ok(None)` when the credential does not exist.
pub fn read_secret(target: &str) -> Result<Option<String>, SecretError> {
    let target_wide = to_wide_null(target);
    let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();

    let result = unsafe {
        CredReadW(
            windows::core::PCWSTR(target_wide.as_ptr()),
            CRED_TYPE_GENERIC,
            Some(0),
            &mut pcred,
        )
    };

    match result {
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
        Err(e) => Err(SecretError::new("read", e.code().0 as u32)),
        Ok(()) => {
            // Safety: CredReadW returned Ok so pcred points to a valid CREDENTIALW
            // allocated by the system. CredFree must be called regardless of outcome.
            let decoded = unsafe {
                let cred = &*pcred;
                let blob = if cred.CredentialBlobSize == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(
                        cred.CredentialBlob,
                        cred.CredentialBlobSize as usize,
                    )
                };
                let value = std::str::from_utf8(blob)
                    .map(str::to_owned)
                    .map_err(|_| SecretError::new("read", 13)); // ERROR_INVALID_DATA
                CredFree(pcred as *const _);
                value
            };
            Ok(Some(decoded?))
        }
    }
}

/// Remove the secret stored under `target` from the Windows Credential Manager.
///
/// A missing credential is treated as success (`Ok(())`).
pub fn delete_secret(target: &str) -> Result<(), SecretError> {
    let target_wide = to_wide_null(target);

    let result = unsafe {
        CredDeleteW(
            windows::core::PCWSTR(target_wide.as_ptr()),
            CRED_TYPE_GENERIC,
            Some(0),
        )
    };

    match result {
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        Err(e) => Err(SecretError::new("delete", e.code().0 as u32)),
        Ok(()) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credential vault is a shared OS resource and does not enjoy six
    /// concurrent readers/writers, so the tests take turns.
    static VAULT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn vault_guard() -> std::sync::MutexGuard<'static, ()> {
        VAULT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn test_target(name: &str) -> String {
        format!(
            "ClaudeCodeUsageMonitor/test/{}/{}",
            std::process::id(),
            name
        )
    }

    #[test]
    fn write_then_read_returns_same_value() {
        let _guard = vault_guard();
        let target = test_target("write_read");
        write_secret(&target, "test-value-abc").unwrap();
        let result = read_secret(&target).unwrap();
        assert_eq!(result.as_deref(), Some("test-value-abc"));
        delete_secret(&target).unwrap();
    }

    #[test]
    fn read_unknown_target_returns_none() {
        let _guard = vault_guard();
        let target = test_target("read_unknown");
        let result = read_secret(&target).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn write_delete_read_returns_none() {
        let _guard = vault_guard();
        let target = test_target("write_delete_read");
        write_secret(&target, "ephemeral-secret").unwrap();
        delete_secret(&target).unwrap();
        let result = read_secret(&target).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn write_twice_overwrites() {
        let _guard = vault_guard();
        let target = test_target("overwrite");
        write_secret(&target, "first-value").unwrap();
        write_secret(&target, "second-value").unwrap();
        let result = read_secret(&target).unwrap();
        assert_eq!(result.as_deref(), Some("second-value"));
        delete_secret(&target).unwrap();
    }

    #[test]
    fn delete_unknown_target_is_ok() {
        let _guard = vault_guard();
        let target = test_target("delete_unknown");
        assert!(delete_secret(&target).is_ok());
    }

    #[test]
    fn non_ascii_secret_round_trips() {
        let _guard = vault_guard();
        let target = test_target("non_ascii");
        let secret = "日本語テスト🔑";
        write_secret(&target, secret).unwrap();
        let result = read_secret(&target).unwrap();
        assert_eq!(result.as_deref(), Some(secret));
        delete_secret(&target).unwrap();
    }
}
