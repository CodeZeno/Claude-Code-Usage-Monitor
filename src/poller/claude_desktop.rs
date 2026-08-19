//! Reads the OAuth token that the Claude desktop app keeps for its bundled
//! Claude Code build.
//!
//! Machines that only ever ran Claude Code through the desktop app have no
//! `~/.claude/.credentials.json`, because that file is written by the
//! standalone CLI login flow. The desktop app is an Electron application and
//! stores its token cache with Chromium's OSCrypt scheme instead: an
//! AES-256-GCM key sits DPAPI-wrapped in `Local State`, and each encrypted
//! value is `"v10" || nonce || ciphertext || tag`.
//!
//! Everything here is read-only, runs as the signed-in user, and degrades to
//! `None` whenever the layout is not what we expect.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use crate::diagnose;

const TOKEN_CACHE_KEY: &str = "oauth:tokenCache";
const DPAPI_KEY_PREFIX: &[u8] = b"DPAPI";
const OS_CRYPT_PREFIX: &[u8] = b"v10";
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;
/// Desktop entries are keyed `"<install>:<user>:<base url>:<scopes>"`; the
/// inference scope marks the token the usage endpoint accepts.
const INFERENCE_SCOPE: &str = "user:inference";
const BCRYPT_INIT_AUTH_MODE_INFO_VERSION: u32 = 1;

pub(super) struct DesktopToken {
    pub(super) access_token: String,
    pub(super) expires_at: Option<i64>,
}

pub(super) fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Claude").join("config.json"))
}

fn local_state_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("Local State")
}

pub(super) fn read_token(config_path: &Path) -> Option<DesktopToken> {
    let config = match std::fs::read_to_string(config_path) {
        Ok(config) => config,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!(
                        "unable to read Claude desktop config at {}",
                        config_path.display()
                    ),
                    error,
                );
            }
            return None;
        }
    };

    let cache = token_cache_value(&config)?;
    let key = os_crypt_key(&local_state_path(config_path))?;
    let plaintext = decrypt_os_crypt_value(&cache, &key)?;
    let plaintext = String::from_utf8(plaintext).ok()?;
    let token = select_token(&plaintext);
    if token.is_none() {
        diagnose::log("Claude desktop token cache held no usable inference token");
    }
    token
}

/// Signature over the encrypted cache rather than the file's mtime: the
/// desktop app rewrites `config.json` for unrelated state such as window
/// placement, and that must not read as a credential change.
pub(super) fn watch_signature(config_path: &Path) -> String {
    let key = format!("desktop:{}", config_path.display());
    match std::fs::read_to_string(config_path)
        .ok()
        .and_then(|config| token_cache_value(&config))
    {
        Some(cache) => format!("{key}|present|{}", fnv1a(cache.as_bytes())),
        None => format!("{key}|missing"),
    }
}

fn token_cache_value(config: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(config).ok()?;
    Some(json.get(TOKEN_CACHE_KEY)?.as_str()?.to_string())
}

/// Picks the freshest entry that carries the inference scope, falling back to
/// the freshest entry of any scope so a future key layout still resolves.
fn select_token(plaintext: &str) -> Option<DesktopToken> {
    let json: serde_json::Value = serde_json::from_str(plaintext).ok()?;
    let entries = json.as_object()?;

    let mut best: Option<(bool, i64, DesktopToken)> = None;
    for (key, entry) in entries {
        let Some(access_token) = entry.get("token").and_then(|value| value.as_str()) else {
            continue;
        };
        if access_token.is_empty() {
            continue;
        }
        let expires_at = entry.get("expiresAt").and_then(|value| value.as_i64());
        let rank = (
            key.contains(INFERENCE_SCOPE),
            expires_at.unwrap_or(i64::MIN),
        );
        if best
            .as_ref()
            .is_some_and(|(scoped, expiry, _)| (*scoped, *expiry) >= rank)
        {
            continue;
        }
        best = Some((
            rank.0,
            rank.1,
            DesktopToken {
                access_token: access_token.to_string(),
                expires_at,
            },
        ));
    }

    best.map(|(_, _, token)| token)
}

fn os_crypt_key(local_state_path: &Path) -> Option<Vec<u8>> {
    let local_state = std::fs::read_to_string(local_state_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&local_state).ok()?;
    let encoded = json.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let wrapped = base64_decode(encoded)?;
    let wrapped = wrapped.strip_prefix(DPAPI_KEY_PREFIX)?;
    dpapi_unprotect(wrapped)
}

fn decrypt_os_crypt_value(value: &str, key: &[u8]) -> Option<Vec<u8>> {
    let blob = base64_decode(value)?;
    let body = blob.strip_prefix(OS_CRYPT_PREFIX)?;
    if body.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
        return None;
    }
    let (nonce, rest) = body.split_at(GCM_NONCE_LEN);
    let (ciphertext, tag) = rest.split_at(rest.len() - GCM_TAG_LEN);
    aes_gcm_decrypt(key, nonce, ciphertext, tag)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim_end_matches('=');
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    let padding_mask = (1u32 << bits).saturating_sub(1);
    (buffer & padding_mask == 0).then_some(output)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[repr(C)]
struct CryptIntegerBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[repr(C)]
struct AuthenticatedCipherModeInfo {
    cb_size: u32,
    dw_info_version: u32,
    pb_nonce: *mut u8,
    cb_nonce: u32,
    pb_auth_data: *mut u8,
    cb_auth_data: u32,
    pb_tag: *mut u8,
    cb_tag: u32,
    pb_mac_context: *mut u8,
    cb_mac_context: u32,
    cb_aad: u32,
    cb_data: u64,
    dw_flags: u32,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptUnprotectData(
        data_in: *const CryptIntegerBlob,
        data_description: *mut *mut u16,
        optional_entropy: *const CryptIntegerBlob,
        reserved: *mut c_void,
        prompt_struct: *mut c_void,
        flags: u32,
        data_out: *mut CryptIntegerBlob,
    ) -> i32;
}

extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(
        algorithm: *mut *mut c_void,
        id: *const u16,
        implementation: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptCloseAlgorithmProvider(algorithm: *mut c_void, flags: u32) -> i32;
    fn BCryptGetProperty(
        object: *mut c_void,
        property: *const u16,
        output: *mut u8,
        output_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    fn BCryptSetProperty(
        object: *mut c_void,
        property: *const u16,
        input: *const u8,
        input_len: u32,
        flags: u32,
    ) -> i32;
    fn BCryptGenerateSymmetricKey(
        algorithm: *mut c_void,
        key: *mut *mut c_void,
        key_object: *mut u8,
        key_object_len: u32,
        secret: *const u8,
        secret_len: u32,
        flags: u32,
    ) -> i32;
    fn BCryptDestroyKey(key: *mut c_void) -> i32;
    fn BCryptDecrypt(
        key: *mut c_void,
        input: *const u8,
        input_len: u32,
        padding_info: *const c_void,
        iv: *mut u8,
        iv_len: u32,
        output: *mut u8,
        output_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
}

fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let input = CryptIntegerBlob {
        cb_data: u32::try_from(data.len()).ok()?,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut output = CryptIntegerBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };

    if ok == 0 || output.pb_data.is_null() {
        diagnose::log("unable to unwrap the Claude desktop OSCrypt key with DPAPI");
        return None;
    }

    Some(unsafe {
        let key = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        LocalFree(output.pb_data as *mut c_void);
        key
    })
}

fn aes_gcm_decrypt(key: &[u8], nonce: &[u8], ciphertext: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
    let algorithm_id = wide("AES");
    let mut algorithm: *mut c_void = std::ptr::null_mut();
    if unsafe {
        BCryptOpenAlgorithmProvider(&mut algorithm, algorithm_id.as_ptr(), std::ptr::null(), 0)
    } != 0
    {
        diagnose::log("unable to open the AES provider for the Claude desktop token cache");
        return None;
    }

    let plaintext = with_gcm_key(algorithm, key, |key_handle| {
        decrypt_with_key(key_handle, nonce, ciphertext, tag)
    });

    unsafe { BCryptCloseAlgorithmProvider(algorithm, 0) };
    plaintext
}

fn with_gcm_key(
    algorithm: *mut c_void,
    key: &[u8],
    decrypt: impl FnOnce(*mut c_void) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    let chaining_property = wide("ChainingMode");
    let chaining_gcm = wide("ChainingModeGCM");
    if unsafe {
        BCryptSetProperty(
            algorithm,
            chaining_property.as_ptr(),
            chaining_gcm.as_ptr() as *const u8,
            u32::try_from(std::mem::size_of_val(chaining_gcm.as_slice())).ok()?,
            0,
        )
    } != 0
    {
        diagnose::log("unable to select GCM chaining for the Claude desktop token cache");
        return None;
    }

    let object_length_property = wide("ObjectLength");
    let mut object_length = 0u32;
    let mut written = 0u32;
    if unsafe {
        BCryptGetProperty(
            algorithm,
            object_length_property.as_ptr(),
            &mut object_length as *mut u32 as *mut u8,
            u32::try_from(std::mem::size_of::<u32>()).ok()?,
            &mut written,
            0,
        )
    } != 0
    {
        return None;
    }

    // The key object buffer must outlive the key handle it backs.
    let mut key_object = vec![0u8; object_length as usize];
    let mut key_handle: *mut c_void = std::ptr::null_mut();
    if unsafe {
        BCryptGenerateSymmetricKey(
            algorithm,
            &mut key_handle,
            key_object.as_mut_ptr(),
            object_length,
            key.as_ptr(),
            u32::try_from(key.len()).ok()?,
            0,
        )
    } != 0
    {
        diagnose::log("unable to import the Claude desktop OSCrypt key");
        return None;
    }

    let plaintext = decrypt(key_handle);
    unsafe { BCryptDestroyKey(key_handle) };
    drop(key_object);
    plaintext
}

fn decrypt_with_key(
    key_handle: *mut c_void,
    nonce: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Option<Vec<u8>> {
    let mut nonce = nonce.to_vec();
    let mut tag = tag.to_vec();
    let mode_info = AuthenticatedCipherModeInfo {
        cb_size: u32::try_from(std::mem::size_of::<AuthenticatedCipherModeInfo>()).ok()?,
        dw_info_version: BCRYPT_INIT_AUTH_MODE_INFO_VERSION,
        pb_nonce: nonce.as_mut_ptr(),
        cb_nonce: u32::try_from(nonce.len()).ok()?,
        pb_auth_data: std::ptr::null_mut(),
        cb_auth_data: 0,
        pb_tag: tag.as_mut_ptr(),
        cb_tag: u32::try_from(tag.len()).ok()?,
        pb_mac_context: std::ptr::null_mut(),
        cb_mac_context: 0,
        cb_aad: 0,
        cb_data: 0,
        dw_flags: 0,
    };

    let mut plaintext = vec![0u8; ciphertext.len()];
    let mut written = 0u32;
    let status = unsafe {
        BCryptDecrypt(
            key_handle,
            ciphertext.as_ptr(),
            u32::try_from(ciphertext.len()).ok()?,
            &mode_info as *const AuthenticatedCipherModeInfo as *const c_void,
            std::ptr::null_mut(),
            0,
            plaintext.as_mut_ptr(),
            u32::try_from(plaintext.len()).ok()?,
            &mut written,
            0,
        )
    };

    if status != 0 {
        diagnose::log("Claude desktop token cache failed AES-GCM authentication");
        return None;
    }

    plaintext.truncate(written as usize);
    Some(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_freshest_inference_scoped_token() {
        let plaintext = r#"{
            "install:user:https://api.anthropic.com:user:profile": {
                "token": "profile-only",
                "expiresAt": 9000000000000
            },
            "install:user:https://api.anthropic.com:user:inference user:profile": {
                "token": "current",
                "expiresAt": 1818644595762
            },
            "install:old:https://api.anthropic.com:user:inference": {
                "token": "stale",
                "expiresAt": 1518644595762
            }
        }"#;

        let token = select_token(plaintext).expect("an inference token should be selected");
        assert_eq!(token.access_token, "current");
        assert_eq!(token.expires_at, Some(1818644595762));
    }

    #[test]
    fn ignores_entries_without_a_usable_token() {
        assert!(select_token(r#"{"install:user:scope": {"expiresAt": 1}}"#).is_none());
        assert!(select_token(r#"{"install:user:scope": {"token": ""}}"#).is_none());
        assert!(select_token("not json").is_none());
    }

    #[test]
    fn reads_the_token_cache_out_of_a_desktop_config() {
        let config = r#"{"locale": "en-US", "oauth:tokenCache": "djEwYWJj"}"#;
        assert_eq!(token_cache_value(config).as_deref(), Some("djEwYWJj"));
        assert!(token_cache_value(r#"{"locale": "en-US"}"#).is_none());
    }

    #[test]
    fn rejects_blobs_that_are_not_os_crypt_v10() {
        // Valid base64, but the version prefix is not "v10".
        assert!(decrypt_os_crypt_value("bm90LXYxMC1kYXRh", &[0u8; 32]).is_none());
        // Right prefix, too short to hold a nonce and a tag.
        assert!(decrypt_os_crypt_value("djEwc2hvcnQ", &[0u8; 32]).is_none());
        assert!(decrypt_os_crypt_value("!!!", &[0u8; 32]).is_none());
    }

    #[test]
    fn decodes_standard_base64_with_and_without_padding() {
        assert_eq!(base64_decode("djEw").unwrap(), b"v10");
        assert_eq!(base64_decode("YWJjZA==").unwrap(), b"abcd");
        assert!(base64_decode("a").is_none());
        assert!(base64_decode("a-b_").is_none());
    }

    /// Ignored by default: this one proves the real DPAPI + AES-GCM path
    /// against whatever the Claude desktop app has on the current machine.
    /// Run it with `cargo test -- --ignored` while signed in to the app.
    #[test]
    #[ignore = "requires a signed-in Claude desktop app on this machine"]
    fn reads_a_token_from_the_installed_desktop_app() {
        let path = config_path().expect("a roaming config directory");
        let token = read_token(&path).expect("the desktop app should expose a token");
        assert!(token.access_token.starts_with("sk-ant-"));
        assert!(token.expires_at.unwrap_or_default() > 0);
    }

    #[test]
    fn watch_signature_reports_missing_config() {
        let signature = watch_signature(Path::new("C:/nonexistent/Claude/config.json"));
        assert!(signature.ends_with("|missing"), "{signature}");
    }
}
