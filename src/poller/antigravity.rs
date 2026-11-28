use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, windows_credentials, PollError};
use crate::diagnose;
use crate::models::{limit_slug, UsageData, UsageLimit, UsageSection};

const ANTIGRAVITY_CREDENTIAL_TARGET: &str = "gemini:antigravity";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const EXPIRY_SKEW: Duration = Duration::from_secs(60);
// The daily summary returns the "Gemini 4 Argon" group only for the agy CLI
// form of the UA; "antigravity", "antigravity/cli/0.0.1" and other forms omit it.
const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.4.0";
const ANTIGRAVITY_ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
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
pub(super) struct AntigravityModelsResponse {
    pub(super) models: HashMap<String, AntigravityModelInfo>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityModelInfo {
    #[serde(rename = "quotaInfo")]
    pub(super) quota_info: Option<AntigravityQuotaInfo>,
}

#[derive(Clone, Deserialize)]
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

type OAuthClients = Vec<(String, String)>;

/// The OAuth clients found in an installed Antigravity binary, kept with the
/// binary's size and modification time so an update is rescanned instead of
/// reading the whole binary (about 200 MB) on every token refresh.
struct CachedOAuthClients {
    path: PathBuf,
    stamp: FileStamp,
    clients: OAuthClients,
}

#[derive(Clone, Debug, PartialEq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

static CACHED_OAUTH_CLIENTS: Mutex<Option<CachedOAuthClients>> = Mutex::new(None);

fn oauth_client_candidate_paths(
    home: Option<PathBuf>,
    local: Option<PathBuf>,
    program_files: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join(".gemini/bin/agy.exe"));
    }
    if let Some(local) = local {
        paths.push(local.join("Programs/Antigravity/resources/bin/language_server.exe"));
        paths.push(local.join("Programs/Antigravity IDE/resources/bin/language_server.exe"));
        paths.push(local.join("agy/bin/agy.exe"));
    }
    if let Some(program_files) = program_files {
        paths.push(program_files.join("Antigravity/resources/bin/language_server.exe"));
    }
    paths
}

fn installed_oauth_clients() -> OAuthClients {
    let home = crate::known_folders::home_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from));
    let local = crate::known_folders::local_app_data_dir()
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from));
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    cached_oauth_clients(
        &CACHED_OAUTH_CLIENTS,
        &oauth_client_candidate_paths(home, local, program_files),
    )
}

/// Returns the cached clients while their binary is unchanged; otherwise scans
/// the candidates in order and caches the first binary that yields clients.
fn cached_oauth_clients(
    cache: &Mutex<Option<CachedOAuthClients>>,
    candidates: &[PathBuf],
) -> OAuthClients {
    let Ok(mut cache) = cache.lock() else {
        return Vec::new();
    };
    if let Some(cached) = cache.as_ref() {
        if file_stamp(&cached.path).as_ref() == Some(&cached.stamp) {
            return cached.clients.clone();
        }
    }
    *cache = None;
    for path in candidates {
        let Some(stamp) = file_stamp(path) else {
            continue;
        };
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let clients = oauth_clients_from_binary(&bytes);
        if !clients.is_empty() {
            *cache = Some(CachedOAuthClients {
                path: path.clone(),
                stamp,
                clients: clients.clone(),
            });
            return clients;
        }
    }
    Vec::new()
}

fn oauth_clients_from_binary(bytes: &[u8]) -> OAuthClients {
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
    refresh_from_clients(installed_oauth_clients(), |client_id, client_secret| {
        let form = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        parse_refresh_response(agent.post(GOOGLE_TOKEN_URL).send_form(form))
    })
}

fn refresh_from_clients(
    clients: OAuthClients,
    mut exchange: impl FnMut(&str, &str) -> Result<String, PollError>,
) -> Result<String, PollError> {
    if clients.is_empty() {
        diagnose::log("Antigravity OAuth refresh failed: no installed OAuth client found");
        return Err(PollError::AuthRequired);
    }
    for (client_id, client_secret) in clients {
        match exchange(&client_id, &client_secret) {
            // A rejected client pairing may not be the one that issued this token.
            Err(PollError::AuthRequired) => continue,
            // Network, rate-limit, and server failures must remain retryable.
            result => return result,
        }
    }
    diagnose::log("Antigravity OAuth refresh failed");
    Err(PollError::AuthRequired)
}

fn parse_refresh_response(
    response: Result<super::HttpResponse, ureq::Error>,
) -> Result<String, PollError> {
    let mut response = response.map_err(|error| match error {
        ureq::Error::StatusCode(401 | 403) => PollError::AuthRequired,
        ureq::Error::StatusCode(code) => PollError::HttpStatus(code),
        _ => PollError::NetworkError,
    })?;
    match response.status().as_u16() {
        400 => {
            #[derive(Deserialize)]
            struct OAuthError {
                error: String,
            }
            let error: OAuthError = response
                .body_mut()
                .read_json()
                .map_err(|_| PollError::UnexpectedResponse)?;
            return Err(match error.error.as_str() {
                "invalid_grant" | "invalid_client" | "unauthorized_client" => {
                    PollError::AuthRequired
                }
                _ => PollError::HttpStatus(400),
            });
        }
        401 | 403 => return Err(PollError::AuthRequired),
        200..=299 => {}
        code => return Err(PollError::HttpStatus(code)),
    }
    let token: RefreshResponse = response
        .body_mut()
        .read_json()
        .map_err(|_| PollError::UnexpectedResponse)?;
    if token.access_token.is_empty() {
        return Err(PollError::UnexpectedResponse);
    }
    Ok(token.access_token)
}

pub(super) fn antigravity_credential_watch_signature() -> String {
    let Some(content) = windows_credentials::read_generic(ANTIGRAVITY_CREDENTIAL_TARGET) else {
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
    select_antigravity_usage(ANTIGRAVITY_ENDPOINTS, |base_url| {
        fetch_antigravity_usage_from_endpoint(base_url, token)
    })
}

pub(super) fn select_antigravity_usage(
    endpoints: &[&str],
    mut fetch: impl FnMut(&str) -> Result<(UsageData, AntigravitySource), PollError>,
) -> Result<UsageData, PollError> {
    let mut model_fallback: Option<UsageData> = None;
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in endpoints {
        match fetch(base_url) {
            // A quota summary names every window it has; it is authoritative
            // for both pools, so later endpoints are neither asked nor merged.
            Ok((data, AntigravitySource::Summary)) => return Ok(data),
            // Model quotas do not name their window; keep them only as a last
            // resort in case no endpoint answers with a summary.
            Ok((data, AntigravitySource::Models)) => {
                model_fallback.get_or_insert(data);
            }
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    if let Some(data) = model_fallback {
        return Ok(data);
    }
    if auth_error {
        Err(PollError::AuthRequired)
    } else {
        Err(last_error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AntigravitySource {
    Summary,
    Models,
}

pub(super) fn fetch_antigravity_usage_from_endpoint(
    base_url: &str,
    token: &str,
) -> Result<(UsageData, AntigravitySource), PollError> {
    let project = fetch_antigravity_project(base_url, token)?;
    let summary_result = if let Some(project) = project.as_deref() {
        match fetch_antigravity_quota_summary(base_url, token, project) {
            Ok(data) => Some(data),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => {
                diagnose::log(format!(
                    "Antigravity retrieveUserQuotaSummary failed: {error:?}"
                ));
                None
            }
        }
    } else {
        None
    };

    // Per-model quotas do not say which window (5h or weekly) they belong to,
    // so when the summary answered, both pools are taken from it alone and the
    // models request is skipped.
    if let Some(data) = summary_result {
        return Ok((data, AntigravitySource::Summary));
    }
    let models_result = fetch_antigravity_models_response(base_url, token, project.as_deref());

    match models_result {
        Ok(models) => antigravity_usage_from_models(&models)
            .map(|data| (data, AntigravitySource::Models))
            .ok_or(PollError::RequestFailed),
        Err(err) => Err(err),
    }
}

/// Fallback when the quota summary is unavailable: Gemini models only.
pub(super) fn antigravity_usage_from_models(
    models: &AntigravityModelsResponse,
) -> Option<UsageData> {
    let gemini = best_gemini_model_quota(models);
    // Claude/GPT model quotas do not say whether they are the 5h or the weekly
    // window, so the external pool is left as "no limit data" here rather
    // than guessed.
    Some(UsageData {
        session: gemini?,
        ..Default::default()
    })
}

pub(super) fn best_gemini_model_quota(models: &AntigravityModelsResponse) -> Option<UsageSection> {
    best_antigravity_section(models.models.iter().filter_map(|(model, info)| {
        if !model.starts_with("gemini") {
            return None;
        }
        let quota = info.quota_info.as_ref().cloned()?;
        antigravity_section_from_quota(quota)
    }))
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
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
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

pub(super) fn fetch_antigravity_models_response(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<AntigravityModelsResponse, PollError> {
    let agent = build_agent()?;
    let body = match project {
        Some(project) => serde_json::json!({ "project": project }),
        None => serde_json::json!({}),
    };

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:fetchAvailableModels"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
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

    Ok(response)
}

#[allow(dead_code)]
pub(super) fn fetch_antigravity_model_quota(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<UsageSection, PollError> {
    let response = fetch_antigravity_models_response(base_url, token, project)?;

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
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
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

/// Parses a saved response body as a poll would, for the provider contract test
/// in `poller::tests`.
#[cfg(test)]
pub(super) fn usage_from_fixture(body: &str) -> Option<UsageData> {
    antigravity_usage_from_summary(serde_json::from_str(body).ok()?)
}

pub(super) fn antigravity_usage_from_summary(
    response: AntigravityQuotaSummaryResponse,
) -> Option<UsageData> {
    let mut gemini_usage = None;
    let mut external_usage = None;
    let mut has_named_pool = false;
    let mut other_groups = Vec::new();
    // Only a stand-in: when Google folds Argon into the main pool, the sole Gemini group can
    // mention Argon, and skipping it would leave the Gemini pool empty.
    let mut argon_usage = None;

    // A summary that returns a group list (even an empty one) answered
    // authoritatively: no recognised window means "no limit data", not a
    // failure to be papered over with model quotas or another endpoint.
    let answered = response.groups.is_some();
    let groups = response.groups.unwrap_or_default();
    let limits = antigravity_limits_from_summary_groups(&groups);
    for group in groups {
        // Argon has its own pool; it must not stand in for the main Gemini pool.
        if is_antigravity_separate_gemini_group(&group) {
            // Its presence also rules out assigning an unrelated sole group
            // through the unnamed-pool fallback below.
            has_named_pool = true;
            if argon_usage.is_none() && has_generic_gemini_buckets(&group) {
                argon_usage = antigravity_usage_from_summary_group(group);
            }
            continue;
        }
        let is_gemini = is_antigravity_gemini_summary_group(&group);
        let is_external = is_antigravity_external_summary_group(&group);
        has_named_pool |= is_gemini || is_external;
        let usage = antigravity_usage_from_summary_group(group);

        if is_gemini {
            if gemini_usage.is_none() {
                gemini_usage = usage;
            }
        } else if is_external {
            if external_usage.is_none() {
                external_usage = usage;
            }
        } else if let Some(usage) = usage {
            other_groups.push(usage);
        }
    }

    // An unrecognised group is assigned only when it is the sole group with
    // quota and no named pool exists; otherwise a position guess could show one
    // pool's usage under the other's label.
    if !has_named_pool && other_groups.len() == 1 {
        gemini_usage = other_groups.pop();
    }

    if gemini_usage.is_none() {
        gemini_usage = argon_usage;
    }

    let mut usage = if let Some(mut gemini) = gemini_usage {
        if let Some(external) = external_usage {
            gemini.external_pool = Some(Box::new(external));
        }
        gemini
    } else if let Some(external) = external_usage {
        let mut gemini = UsageData::default();
        gemini.session.available = false;
        gemini.weekly.available = false;
        gemini.external_pool = Some(Box::new(external));
        gemini
    } else if answered {
        UsageData::default()
    } else {
        return None;
    };
    usage.limits = limits;
    Some(usage)
}

// The merged pool's buckets are `gemini-5h` / `gemini-weekly`; a real Argon pool names Argon there.
fn has_generic_gemini_buckets(group: &AntigravityQuotaSummaryGroup) -> bool {
    group.buckets.as_ref().is_some_and(|buckets| {
        buckets.iter().any(|bucket| {
            bucket.bucket_id.as_deref().is_some_and(|id| {
                let id = id.to_ascii_lowercase();
                id.starts_with("gemini-") && !id.contains("argon")
            })
        })
    })
}

pub(super) fn is_antigravity_separate_gemini_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    let is_argon = |value: Option<&str>| {
        value.is_some_and(|value| value.to_ascii_lowercase().contains("argon"))
    };
    is_argon(group.display_name.as_deref())
        || is_argon(group.description.as_deref())
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                is_argon(bucket.bucket_id.as_deref()) || is_argon(bucket.display_name.as_deref())
            })
        })
}

// The main and external pools show two groups only; themes need every group
// (e.g. "Gemini 4 Argon") and every window to show per-group quota.
pub(super) fn antigravity_limits_from_summary_groups(
    groups: &[AntigravityQuotaSummaryGroup],
) -> Vec<UsageLimit> {
    let mut limits = Vec::new();
    for group in groups {
        let Some(name) = group
            .display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        else {
            continue;
        };
        for bucket in group.buckets.iter().flatten() {
            let kind = match bucket.window.as_deref() {
                Some(window) if window.eq_ignore_ascii_case("5h") => "session",
                Some(window) if window.eq_ignore_ascii_case("weekly") => "weekly",
                _ => continue,
            };
            let Some(usage) = antigravity_section_from_summary_bucket(bucket) else {
                continue;
            };
            limits.push(UsageLimit {
                key: format!("{}_{kind}", limit_slug(name)),
                kind: kind.to_string(),
                label: name.to_string(),
                model: Some(name.to_string()),
                model_id: bucket.bucket_id.clone(),
                scope: None,
                is_active: false,
                usage,
            });
        }
    }
    limits
}

pub(super) fn is_antigravity_external_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    group.display_name.as_deref().is_some_and(|name| {
        let n = name.to_ascii_lowercase();
        n.contains("claude") || n.contains("gpt") || n.contains("external")
    }) || group.description.as_deref().is_some_and(|description| {
        let d = description.to_ascii_lowercase();
        d.contains("claude") || d.contains("gpt") || d.contains("external")
    }) || group.buckets.as_ref().is_some_and(|buckets| {
        buckets.iter().any(|bucket| {
            bucket.bucket_id.as_deref().is_some_and(|id| {
                let id = id.to_ascii_lowercase();
                id.starts_with("claude-") || id.starts_with("gpt-") || id.starts_with("3p-")
            }) || bucket.display_name.as_deref().is_some_and(|name| {
                let n = name.to_ascii_lowercase();
                n.contains("claude") || n.contains("gpt")
            })
        })
    })
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
    let content = windows_credentials::read_generic(ANTIGRAVITY_CREDENTIAL_TARGET)?;
    let auth: AntigravityAuthFile = serde_json::from_str(&content).ok()?;
    (!auth.token.access_token.is_empty()
        || auth
            .token
            .refresh_token
            .as_deref()
            .is_some_and(|value| !value.is_empty()))
    .then_some(auth.token)
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use std::cell::Cell;
    use std::time::UNIX_EPOCH;

    // Built at runtime so no literal looks like a real OAuth client to secret scanners.
    fn fake_client_id() -> String {
        format!(
            "{}-{}.apps.googleusercontent.com",
            "1".repeat(12),
            "c".repeat(32)
        )
    }

    fn fake_client_secret(fill: char) -> String {
        format!("GOCSPX-{}", fill.to_string().repeat(28))
    }

    fn scratch_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "antigravity-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    fn fake_binary(client_secret: &str, padding: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; padding];
        let client_id = fake_client_id();
        bytes.extend_from_slice(format!("\0{client_id}\0{client_secret}\0").as_bytes());
        bytes
    }

    #[test]
    fn oauth_clients_are_cached_until_the_binary_changes() {
        let directory = scratch_directory("oauth-cache");
        let missing = directory.join("missing.exe");
        let binary = directory.join("language_server.exe");
        let cache = Mutex::new(None);
        let candidates = [missing, binary.clone()];

        assert!(cached_oauth_clients(&cache, &candidates).is_empty());
        assert!(
            cache.lock().unwrap().is_none(),
            "no clients must not be cached"
        );

        fs::write(&binary, fake_binary(&fake_client_secret('a'), 16)).unwrap();
        let old = vec![(fake_client_id(), fake_client_secret('a'))];
        assert_eq!(cached_oauth_clients(&cache, &candidates), old);

        // The cached clients are returned without reading the binary again.
        cache.lock().unwrap().as_mut().unwrap().clients = vec![("cached".into(), "c".into())];
        assert_eq!(cached_oauth_clients(&cache, &candidates)[0].0, "cached");

        // An updated Antigravity install is rescanned.
        fs::write(&binary, fake_binary(&fake_client_secret('b'), 64)).unwrap();
        let new = vec![(fake_client_id(), fake_client_secret('b'))];
        assert_eq!(cached_oauth_clients(&cache, &candidates), new);

        // A removed install drops the cache.
        fs::remove_file(&binary).unwrap();
        assert!(cached_oauth_clients(&cache, &candidates).is_empty());
        assert!(cache.lock().unwrap().is_none());

        fs::remove_dir_all(&directory).unwrap();
    }

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
    fn refresh_responses_distinguish_rejected_credentials_from_retryable_failures() {
        let response = |status, body: &str| {
            Ok(ureq::http::Response::builder()
                .status(status)
                .body(ureq::Body::builder().data(body.as_bytes().to_vec()))
                .unwrap())
        };
        assert_eq!(
            parse_refresh_response(response(200, r#"{"access_token":"new"}"#)),
            Ok("new".into())
        );
        for code in [401, 403] {
            assert_eq!(
                parse_refresh_response(response(code, "")),
                Err(PollError::AuthRequired)
            );
        }
        for error in ["invalid_grant", "invalid_client", "unauthorized_client"] {
            assert_eq!(
                parse_refresh_response(response(400, &format!(r#"{{"error":"{error}"}}"#))),
                Err(PollError::AuthRequired)
            );
        }
        for code in [429, 500, 503] {
            assert_eq!(
                parse_refresh_response(response(code, "")),
                Err(PollError::HttpStatus(code))
            );
            assert_eq!(
                parse_refresh_response(Err(ureq::Error::StatusCode(code))),
                Err(PollError::HttpStatus(code))
            );
        }
        for body in ["not JSON", "{}", r#"{"access_token":""}"#] {
            assert_eq!(
                parse_refresh_response(response(200, body)),
                Err(PollError::UnexpectedResponse)
            );
        }
        assert_eq!(
            parse_refresh_response(response(400, r#"{"error":"temporarily_unavailable"}"#)),
            Err(PollError::HttpStatus(400))
        );
        assert_eq!(
            parse_refresh_response(Err(ureq::Error::Io(std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused
            )))),
            Err(PollError::NetworkError)
        );
    }

    #[test]
    fn transient_refresh_failures_stop_client_attempts_and_keep_polling_retryable() {
        let clients = || {
            vec![
                ("first".into(), "secret".into()),
                ("second".into(), "secret".into()),
            ]
        };
        for error in [
            PollError::NetworkError,
            PollError::HttpStatus(429),
            PollError::HttpStatus(503),
            PollError::UnexpectedResponse,
        ] {
            let attempts = Cell::new(0);
            let result = poll_with_refresh(
                &credentials(Some("2020-01-01T00:00:00Z")),
                |_| panic!("usage must wait for a refreshed token"),
                |_| {
                    refresh_from_clients(clients(), |_, _| {
                        attempts.set(attempts.get() + 1);
                        Err(error)
                    })
                },
            );
            assert_eq!(result.unwrap_err(), error);
            assert!(error.is_transient());
            assert_eq!(attempts.get(), 1);
        }
        assert_eq!(
            refresh_from_clients(clients(), |id, _| if id == "first" {
                Err(PollError::AuthRequired)
            } else {
                Ok("new".into())
            }),
            Ok("new".into())
        );
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
    fn oauth_client_candidate_paths_include_user_gemini_agy_binary() {
        let paths = oauth_client_candidate_paths(
            Some(PathBuf::from(r"C:\Users\dev")),
            Some(PathBuf::from(r"C:\Users\dev\AppData\Local")),
            Some(PathBuf::from(r"C:\Program Files")),
        );
        assert!(paths.contains(&PathBuf::from(r"C:\Users\dev").join(".gemini/bin/agy.exe")));
        assert!(paths.contains(
            &PathBuf::from(r"C:\Users\dev\AppData\Local")
                .join("Programs/Antigravity IDE/resources/bin/language_server.exe")
        ));
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
