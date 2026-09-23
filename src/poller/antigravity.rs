use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const ANTIGRAVITY_CREDENTIAL_TARGET: &str = "gemini:antigravity";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const EXPIRY_SKEW: Duration = Duration::from_secs(60);
const ANTIGRAVITY_ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];

#[derive(Deserialize)]
struct AntigravityAuthFile {
    token: AntigravityTokenData,
}

#[derive(Deserialize)]
struct AntigravityTokenData {
    access_token: String,
    refresh_token: Option<String>,
    expiry: Option<String>,
}

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct AntigravityLoadResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<String>,
}

#[derive(Deserialize)]
struct AntigravityModelsResponse {
    models: HashMap<String, AntigravityModelInfo>,
}

#[derive(Deserialize)]
struct AntigravityModelInfo {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<AntigravityQuotaInfo>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryResponse {
    groups: Option<Vec<AntigravityQuotaSummaryGroup>>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryGroup {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    buckets: Option<Vec<AntigravityQuotaSummaryBucket>>,
}

#[derive(Clone, Deserialize)]
pub(super) struct AntigravityQuotaSummaryBucket {
    #[serde(rename = "bucketId")]
    bucket_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    window: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

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
    fn CredFree(buffer: *mut c_void);
}

pub(super) fn poll_antigravity() -> Result<UsageData, PollError> {
    let creds = match read_antigravity_credentials() {
        Some(creds) => creds,
        None => {
            diagnose::log("Antigravity usage poll failed: no Antigravity credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    poll_with_refresh(&creds, fetch_antigravity_usage, refresh_antigravity_token)
}

fn poll_with_refresh<F, R>(
    creds: &AntigravityTokenData,
    fetch: F,
    refresh: R,
) -> Result<UsageData, PollError>
where
    F: Fn(&str) -> Result<UsageData, PollError>,
    R: Fn(&str) -> Result<String, PollError>,
{
    let refresh_token = creds
        .refresh_token
        .as_deref()
        .filter(|value| !value.is_empty());
    let expiring = creds.access_token.is_empty()
        || parse_iso8601(creds.expiry.as_deref())
            .is_some_and(|expiry| expiry <= SystemTime::now() + EXPIRY_SKEW);
    let mut token = creds.access_token.clone();
    let mut refreshed = false;
    if expiring {
        if let Some(secret) = refresh_token {
            token = refresh(secret)?;
            refreshed = true;
        }
    }
    if token.is_empty() {
        return Err(PollError::AuthRequired);
    }
    match fetch(&token) {
        Err(PollError::AuthRequired) if !refreshed => {
            let Some(refresh_token) = refresh_token else {
                return Err(PollError::AuthRequired);
            };
            let token = refresh(refresh_token)?;
            fetch(&token)
        }
        result => result,
    }
}

fn installed_oauth_clients() -> Vec<(String, String)> {
    let mut paths = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        paths.push(local.join("Programs/Antigravity/resources/bin/language_server.exe"));
        paths.push(local.join("agy/bin/agy.exe"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        paths.push(
            PathBuf::from(program_files).join("Antigravity/resources/bin/language_server.exe"),
        );
    }
    for path in paths {
        if let Ok(bytes) = fs::read(path) {
            let clients = oauth_clients_from_binary(&bytes);
            if !clients.is_empty() {
                return clients;
            }
        }
    }
    Vec::new()
}

fn oauth_clients_from_binary(bytes: &[u8]) -> Vec<(String, String)> {
    const CLIENT_ID_SUFFIX: &str = ".apps.googleusercontent.com";
    const CLIENT_SECRET_PREFIX: &str = "GOCSPX-";
    let mut ids = Vec::new();
    let mut secrets = Vec::new();
    for run in
        bytes.split(|byte| !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'-'))
    {
        for (suffix_at, _) in run
            .windows(CLIENT_ID_SUFFIX.len())
            .enumerate()
            .filter(|(_, part)| *part == CLIENT_ID_SUFFIX.as_bytes())
        {
            for (hyphen, byte) in run[..suffix_at].iter().enumerate() {
                if *byte != b'-' {
                    continue;
                }
                let mut start = hyphen;
                while start > 0 && run[start - 1].is_ascii_digit() {
                    start -= 1;
                }
                let client_hash = &run[hyphen + 1..suffix_at];
                if hyphen - start < 10
                    || !(20..=80).contains(&client_hash.len())
                    || !client_hash
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
                {
                    continue;
                }
                if let Ok(client_id) =
                    std::str::from_utf8(&run[start..suffix_at + CLIENT_ID_SUFFIX.len()])
                {
                    if !ids.iter().any(|existing| existing == client_id) {
                        ids.push(client_id.to_owned());
                    }
                }
            }
        }
        for (start, _) in run
            .windows(CLIENT_SECRET_PREFIX.len())
            .enumerate()
            .filter(|(_, part)| *part == CLIENT_SECRET_PREFIX.as_bytes())
        {
            let Some(candidate) = run.get(start..start + 35) else {
                continue;
            };
            if candidate
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
            {
                if let Ok(secret) = std::str::from_utf8(candidate) {
                    if !secrets.iter().any(|existing| existing == secret) {
                        secrets.push(secret.to_owned());
                    }
                }
            }
        }
    }
    ids.into_iter()
        .flat_map(|id| {
            secrets
                .iter()
                .cloned()
                .map(move |secret| (id.clone(), secret))
        })
        .collect()
}

fn refresh_antigravity_token(refresh_token: &str) -> Result<String, PollError> {
    let agent = build_agent()?;
    for (client_id, client_secret) in installed_oauth_clients() {
        let form = [
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        let response = agent
            .post(GOOGLE_TOKEN_URL)
            .send_form(form)
            .and_then(super::check_http_status);
        if let Ok(mut response) = response {
            if let Ok(RefreshResponse { access_token }) =
                response.body_mut().read_json::<RefreshResponse>()
            {
                if !access_token.is_empty() {
                    return Ok(access_token);
                }
            }
        }
    }
    diagnose::log("Antigravity OAuth refresh failed");
    Err(PollError::AuthRequired)
}

pub(super) fn antigravity_credential_watch_signature() -> String {
    let Some(content) = read_windows_generic_credential(ANTIGRAVITY_CREDENTIAL_TARGET) else {
        return format!("{ANTIGRAVITY_CREDENTIAL_TARGET}|missing");
    };

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!(
        "{ANTIGRAVITY_CREDENTIAL_TARGET}|present|{}|{}",
        content.len(),
        hasher.finish()
    )
}

pub(super) fn fetch_antigravity_usage(token: &str) -> Result<UsageData, PollError> {
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in ANTIGRAVITY_ENDPOINTS {
        match fetch_antigravity_usage_from_endpoint(base_url, token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    if auth_error {
        Err(PollError::AuthRequired)
    } else {
        Err(last_error)
    }
}

pub(super) fn fetch_antigravity_usage_from_endpoint(
    base_url: &str,
    token: &str,
) -> Result<UsageData, PollError> {
    let project = fetch_antigravity_project(base_url, token)?;
    if let Some(project) = project.as_deref() {
        match fetch_antigravity_quota_summary(base_url, token, project) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => diagnose::log(format!(
                "Antigravity retrieveUserQuotaSummary failed, falling back to model quota: {error:?}"
            )),
        }
    }

    let session = fetch_antigravity_model_quota(base_url, token, project.as_deref())?;
    let weekly = UsageSection::default();

    Ok(UsageData {
        limits: Vec::new(),
        session,
        weekly,
        weekly_label: None,
        monthly: None,
        credits: None,
        stale: false,
    })
}

pub(super) fn fetch_antigravity_project(
    base_url: &str,
    token: &str,
) -> Result<Option<String>, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:loadCodeAssist"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", "antigravity")
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity loadCodeAssist returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity loadCodeAssist request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityLoadResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Antigravity loadCodeAssist response", error);
            return Err(PollError::RequestFailed);
        }
    };

    Ok(response.project.filter(|project| !project.is_empty()))
}

pub(super) fn fetch_antigravity_model_quota(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<UsageSection, PollError> {
    let agent = build_agent()?;
    let body = match project {
        Some(project) => serde_json::json!({ "project": project }),
        None => serde_json::json!({}),
    };

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:fetchAvailableModels"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", "antigravity")
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity fetchAvailableModels returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity fetchAvailableModels request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityModelsResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity fetchAvailableModels response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    best_antigravity_section(response.models.into_iter().filter_map(|(model, info)| {
        let quota = info.quota_info?;
        if !is_antigravity_display_model(&model) {
            return None;
        }
        antigravity_section_from_quota(quota)
    }))
    .ok_or(PollError::RequestFailed)
}

pub(super) fn fetch_antigravity_quota_summary(
    base_url: &str,
    token: &str,
    project: &str,
) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({ "project": project });

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:retrieveUserQuotaSummary"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", "antigravity")
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity retrieveUserQuotaSummary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityQuotaSummaryResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity retrieveUserQuotaSummary response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    antigravity_usage_from_summary(response).ok_or(PollError::RequestFailed)
}

pub(super) fn antigravity_section_from_quota(quota: AntigravityQuotaInfo) -> Option<UsageSection> {
    let remaining = quota.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        available: true,
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(quota.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_section_from_summary_bucket(
    bucket: &AntigravityQuotaSummaryBucket,
) -> Option<UsageSection> {
    let remaining = bucket.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        available: true,
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(bucket.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_usage_from_summary(
    response: AntigravityQuotaSummaryResponse,
) -> Option<UsageData> {
    let mut fallback = None;

    for group in response.groups.unwrap_or_default() {
        let is_gemini = is_antigravity_gemini_summary_group(&group);
        let usage = antigravity_usage_from_summary_group(group);

        if is_gemini && usage.is_some() {
            return usage;
        }

        if fallback.is_none() {
            fallback = usage;
        }
    }

    fallback
}

pub(super) fn antigravity_usage_from_summary_group(
    group: AntigravityQuotaSummaryGroup,
) -> Option<UsageData> {
    let mut data = UsageData::default();
    let mut has_quota = false;

    for bucket in group.buckets.unwrap_or_default() {
        let Some(section) = antigravity_section_from_summary_bucket(&bucket) else {
            continue;
        };

        match bucket.window.as_deref() {
            Some(window) if window.eq_ignore_ascii_case("5h") => {
                data.session = section;
                has_quota = true;
            }
            Some(window) if window.eq_ignore_ascii_case("weekly") => {
                data.weekly = section;
                has_quota = true;
            }
            _ => {}
        }
    }

    has_quota.then_some(data)
}

pub(super) fn is_antigravity_gemini_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    group
        .display_name
        .as_deref()
        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
        || group
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains("gemini"))
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                bucket
                    .bucket_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_lowercase().starts_with("gemini-"))
                    || bucket
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
            })
        })
}

pub(super) fn best_antigravity_section<I>(sections: I) -> Option<UsageSection>
where
    I: IntoIterator<Item = UsageSection>,
{
    sections.into_iter().max_by(|a, b| {
        a.percentage
            .partial_cmp(&b.percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.resets_at.cmp(&b.resets_at))
    })
}

pub(super) fn is_antigravity_display_model(model: &str) -> bool {
    model.starts_with("gemini")
        || model.starts_with("claude")
        || model.starts_with("gpt")
        || model.starts_with("image")
        || model.starts_with("imagen")
}

fn read_antigravity_credentials() -> Option<AntigravityTokenData> {
    let content = read_windows_generic_credential(ANTIGRAVITY_CREDENTIAL_TARGET)?;
    let auth: AntigravityAuthFile = serde_json::from_str(&content).ok()?;
    (!auth.token.access_token.is_empty()
        || auth
            .token
            .refresh_token
            .as_deref()
            .is_some_and(|value| !value.is_empty()))
    .then_some(auth.token)
}

fn read_windows_generic_credential(target: &str) -> Option<String> {
    const CRED_TYPE_GENERIC: u32 = 1;

    let target_wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut credential: *mut CredentialW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    if ok == 0 || credential.is_null() {
        diagnose::log(format!(
            "unable to read Windows generic credential target {target}"
        ));
        return None;
    }

    unsafe {
        let credentials = &*credential;
        if credentials.credential_blob_size == 0 || credentials.credential_blob.is_null() {
            CredFree(credential as *mut c_void);
            return None;
        }
        let bytes = std::slice::from_raw_parts(
            credentials.credential_blob,
            credentials.credential_blob_size as usize,
        );
        let text = String::from_utf8(bytes.to_vec()).ok();
        CredFree(credential as *mut c_void);
        text
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use std::cell::Cell;

    fn credentials(expiry: Option<&str>) -> AntigravityTokenData {
        AntigravityTokenData {
            access_token: "old".into(),
            refresh_token: Some("refresh".into()),
            expiry: expiry.map(str::to_owned),
        }
    }

    #[test]
    fn valid_token_uses_existing_request() {
        let calls = Cell::new(0);
        let result = poll_with_refresh(
            &credentials(None),
            |token| {
                assert_eq!(token, "old");
                Ok(UsageData::default())
            },
            |_| {
                calls.set(calls.get() + 1);
                Ok("new".into())
            },
        );
        assert!(result.is_ok());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn expired_token_refreshes_before_request() {
        let calls = Cell::new(0);
        let result = poll_with_refresh(
            &credentials(Some("2020-01-01T00:00:00Z")),
            |token| {
                calls.set(calls.get() + 1);
                assert_eq!(token, "new");
                Ok(UsageData::default())
            },
            |_| Ok("new".into()),
        );
        assert!(result.is_ok());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn unauthorized_refreshes_and_retries_once() {
        let calls = Cell::new(0);
        let result = poll_with_refresh(
            &credentials(None),
            |token| {
                calls.set(calls.get() + 1);
                if token == "old" {
                    Err(PollError::AuthRequired)
                } else {
                    Ok(UsageData::default())
                }
            },
            |_| Ok("new".into()),
        );
        assert!(result.is_ok());
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn failed_refresh_is_auth_required() {
        let result = poll_with_refresh(
            &credentials(None),
            |_| Err(PollError::AuthRequired),
            |_| Err(PollError::AuthRequired),
        );
        assert!(matches!(result, Err(PollError::AuthRequired)));
    }

    #[test]
    fn retry_never_refreshes_twice() {
        let refreshes = Cell::new(0);
        let calls = Cell::new(0);
        let result = poll_with_refresh(
            &credentials(None),
            |_| {
                calls.set(calls.get() + 1);
                Err(PollError::AuthRequired)
            },
            |_| {
                refreshes.set(refreshes.get() + 1);
                Ok("new".into())
            },
        );
        assert!(matches!(result, Err(PollError::AuthRequired)));
        assert_eq!(calls.get(), 2);
        assert_eq!(refreshes.get(), 1);
    }

    #[test]
    fn extracts_oauth_metadata_without_literal_credentials() {
        let mut bytes =
            b"123456789012-hash_12345678901234567890.apps.googleusercontent.com".to_vec();
        bytes.extend_from_slice(b"GOCSPX-");
        bytes.resize(bytes.len() + 28, b'1');
        assert_eq!(oauth_clients_from_binary(&bytes).len(), 1);
    }

    #[test]
    fn existing_credential_json_remains_supported() {
        let old: AntigravityAuthFile =
            serde_json::from_str(r#"{"token":{"access_token":"old"}}"#).unwrap();
        assert_eq!(old.token.access_token, "old");
        assert!(old.token.refresh_token.is_none());
        let current: AntigravityAuthFile = serde_json::from_str(r#"{"token":{"access_token":"old","refresh_token":"refresh","expiry":"2026-01-01T00:00:00Z"}}"#).unwrap();
        assert_eq!(current.token.refresh_token.as_deref(), Some("refresh"));
    }
}
