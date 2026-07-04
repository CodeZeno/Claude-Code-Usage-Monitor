use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::diagnose;
use crate::models::{AppUsageData, UsageData};
use crate::poller;

const NOTIFIER_FILE: &str = "notifier.json";
const MESSANGI_DEFAULT_BASE_URL: &str = "https://elastic.messangi.me";
const MESSANGI_NOTIFY_PATH: &str = "/oberyn/v2/notification";
const MESSANGI_EMAIL_PATH: &str = "/crowsnest/v2/emails";
const MESSANGI_WHATSAPP_PATH: &str = "/balerion/v3/messages";
const DEFAULT_KEEP_ALIVE_HOURS: u64 = 23;
const DEFAULT_MIN_ALERT_SECS: u64 = 60;
const HTTP_TIMEOUT_SECS: u64 = 15;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotifierConfig {
    #[serde(default)]
    pub messangi_api_key: Option<String>,
    #[serde(default)]
    pub messangi_base_url: Option<String>,
    #[serde(default)]
    pub sms: ChannelConfig,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,
    #[serde(default)]
    pub thresholds: HashMap<String, ProviderThresholds>,
    #[serde(default)]
    pub keep_alive_hours: Option<u64>,
    #[serde(default)]
    pub min_alert_interval_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub shortcode: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EmailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProviderThresholds {
    #[serde(default)]
    pub session: Vec<u8>,
    #[serde(default)]
    pub weekly: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotifierState {
    #[serde(default)]
    pub whatsapp_opted_in_at: Option<i64>,
    #[serde(default)]
    pub whatsapp_last_message_at: Option<i64>,
    #[serde(default)]
    pub last_alert_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotifierFile {
    #[serde(default)]
    pub config: NotifierConfig,
    #[serde(default)]
    pub state: NotifierState,
    #[serde(default)]
    pub notify_sms: Option<bool>,
    #[serde(default)]
    pub notify_email: Option<bool>,
    #[serde(default)]
    pub notify_whatsapp: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ThresholdTracker {
    last_values: HashMap<(String, String), f64>,
    notified: HashMap<(String, String, u8), bool>,
}

pub struct Notifier {
    pub config: NotifierConfig,
    pub state: NotifierState,
    pub notify_sms: bool,
    pub notify_email: bool,
    pub notify_whatsapp: bool,
    pub tracker: ThresholdTracker,
    pub file_path: PathBuf,
}

impl Notifier {
    pub fn load() -> Self {
        let file_path = notifier_file_path();
        let (config, state, notify_sms, notify_email, notify_whatsapp) =
            match std::fs::read_to_string(&file_path)
                .ok()
                .and_then(|s| serde_json::from_str::<NotifierFile>(&s).ok())
            {
                Some(file) => (
                    file.config,
                    file.state,
                    file.notify_sms.unwrap_or(false),
                    file.notify_email.unwrap_or(false),
                    file.notify_whatsapp.unwrap_or(false),
                ),
                None => Default::default(),
            };
        Notifier {
            config,
            state,
            notify_sms,
            notify_email,
            notify_whatsapp,
            tracker: ThresholdTracker::default(),
            file_path,
        }
    }

    pub fn save(&self) {
        let file = NotifierFile {
            config: self.config.clone(),
            state: self.state.clone(),
            notify_sms: Some(self.notify_sms),
            notify_email: Some(self.notify_email),
            notify_whatsapp: Some(self.notify_whatsapp),
        };
        if let Ok(content) = serde_json::to_string_pretty(&file) {
            if let Some(parent) = self.file_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(error) = std::fs::write(&self.file_path, content) {
                diagnose::log_error("unable to write notifier config", error);
            }
        }
    }

    pub fn reload(&mut self) {
        let fresh = Self::load();
        self.config = fresh.config;
        self.state = fresh.state;
        self.notify_sms = fresh.notify_sms;
        self.notify_email = fresh.notify_email;
        self.notify_whatsapp = fresh.notify_whatsapp;
        self.tracker = ThresholdTracker::default();
    }

    pub fn has_api_key(&self) -> bool {
        self.config
            .messangi_api_key
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn is_whatsapp_opted_in(&self) -> bool {
        self.state.whatsapp_opted_in_at.is_some()
    }

    pub fn mark_whatsapp_opted_in(&mut self) {
        self.state.whatsapp_opted_in_at = Some(now_unix_secs());
        self.state.whatsapp_last_message_at = Some(now_unix_secs());
        self.save();
    }

    pub fn reset_whatsapp_opt_in(&mut self) {
        self.state.whatsapp_opted_in_at = None;
        self.state.whatsapp_last_message_at = None;
        self.save();
    }

    pub fn is_configured_for(&self, channel: Channel) -> bool {
        match channel {
            Channel::Sms => {
                self.config.sms.enabled
                    && self.config.sms.to.is_some()
                    && self.config.sms.shortcode.is_some()
            }
            Channel::Email => {
                self.config.email.enabled
                    && self.config.email.to.is_some()
                    && self.config.email.from.is_some()
            }
            Channel::WhatsApp => {
                self.config.whatsapp.enabled
                    && self.config.whatsapp.from.is_some()
                    && self.config.whatsapp.to.is_some()
            }
        }
    }

    pub fn is_channel_active(&self, channel: Channel) -> bool {
        match channel {
            Channel::Sms => self.notify_sms,
            Channel::Email => self.notify_email,
            Channel::WhatsApp => self.notify_whatsapp,
        }
    }

    pub fn set_channel_active(&mut self, channel: Channel, active: bool) {
        match channel {
            Channel::Sms => self.notify_sms = active,
            Channel::Email => self.notify_email = active,
            Channel::WhatsApp => self.notify_whatsapp = active,
        }
        self.save();
    }

    pub fn check_and_notify(&mut self, data: &AppUsageData) {
        if !self.has_api_key() {
            return;
        }

        let providers: [(&str, Option<&UsageData>); 4] = [
            ("claude_code", data.claude_code.as_ref()),
            ("codex", data.codex.as_ref()),
            ("antigravity", data.antigravity.as_ref()),
            ("opencode", data.opencode.as_ref()),
        ];

        for (provider_id, provider_data) in providers {
            let Some(provider_data) = provider_data else {
                continue;
            };
            let Some(thresholds) = self.config.thresholds.get(provider_id).cloned() else {
                continue;
            };
            self.check_provider(provider_id, provider_data, &thresholds);
        }

        if self.is_channel_active(Channel::WhatsApp)
            && self.config.whatsapp.enabled
            && self.is_whatsapp_opted_in()
        {
            self.maybe_send_whatsapp_keep_alive();
        }
    }

    fn check_provider(
        &mut self,
        provider_id: &str,
        data: &UsageData,
        thresholds: &ProviderThresholds,
    ) {
        let windows: [(&str, f64); 2] = [("session", data.session.percentage), ("weekly", data.weekly.percentage)];

        for (window_name, current) in windows {
            let key = (provider_id.to_string(), window_name.to_string());
            let window_thresholds: &[u8] = match window_name {
                "session" => &thresholds.session,
                "weekly" => &thresholds.weekly,
                _ => &[],
            };

            for &threshold in window_thresholds {
                let notify_key = (provider_id.to_string(), window_name.to_string(), threshold);
                let was_notified = self
                    .tracker
                    .notified
                    .get(&notify_key)
                    .copied()
                    .unwrap_or(false);
                let t = threshold as f64;
                if current >= t && !was_notified {
                    self.tracker.notified.insert(notify_key, true);
                    self.maybe_dispatch_alert(provider_id, window_name, threshold, current);
                } else if current < t && was_notified {
                    self.tracker.notified.remove(&notify_key);
                }
            }

            self.tracker.last_values.insert(key, current);
        }
    }

    /// Dispatch an alert to all enabled channels, gated by the
    /// `min_alert_interval_secs` rate limit.
    fn maybe_dispatch_alert(
        &mut self,
        provider_id: &str,
        window_name: &str,
        threshold: u8,
        current: f64,
    ) {
        let min_interval = Duration::from_secs(
            self.config
                .min_alert_interval_secs
                .unwrap_or(DEFAULT_MIN_ALERT_SECS),
        );
        if let Some(last) = self.state.last_alert_at {
            let elapsed = now_unix_secs().saturating_sub(last);
            if elapsed < min_interval.as_secs() as i64 {
                return;
            }
        }
        if self.dispatch_alert(provider_id, window_name, threshold, current) {
            self.state.last_alert_at = Some(now_unix_secs());
            self.save();
        }
    }

    /// Send a notification to all enabled channels. Returns true if at least one
    /// channel accepted the alert.
    fn dispatch_alert(
        &mut self,
        provider_id: &str,
        window_name: &str,
        threshold: u8,
        current: f64,
    ) -> bool {
        let body = format!(
            "{} {} crossed {:.0}% (now {:.0}%)",
            humanize_provider(provider_id),
            window_name,
            threshold as f64,
            current
        );
        let subject = format!(
            "Claude Code Usage Monitor: {} {} alert",
            humanize_provider(provider_id),
            window_name
        );

        let mut sent = false;

        if self.is_channel_active(Channel::Sms) && self.is_configured_for(Channel::Sms) {
            match self.send_sms(&body) {
                Ok(_) => sent = true,
                Err(error) => diagnose::log(format!("SMS alert failed: {error}")),
            }
        }

        if self.is_channel_active(Channel::Email) && self.is_configured_for(Channel::Email) {
            match self.send_email(&subject, &body) {
                Ok(_) => sent = true,
                Err(error) => diagnose::log(format!("Email alert failed: {error}")),
            }
        }

        if self.is_channel_active(Channel::WhatsApp)
            && self.is_configured_for(Channel::WhatsApp)
            && self.is_whatsapp_opted_in()
        {
            match self.send_whatsapp_session(&body) {
                Ok(_) => {
                    sent = true;
                    self.state.whatsapp_last_message_at = Some(now_unix_secs());
                }
                Err(NotifError::SessionExpired) => {
                    diagnose::log("WhatsApp session expired; clearing opt-in");
                    self.reset_whatsapp_opt_in();
                }
                Err(error) => diagnose::log(format!("WhatsApp alert failed: {error}")),
            }
        }

        sent
    }

    fn maybe_send_whatsapp_keep_alive(&mut self) {
        let keep_alive_hours = self
            .config
            .keep_alive_hours
            .unwrap_or(DEFAULT_KEEP_ALIVE_HOURS);
        let now = now_unix_secs();
        let last = self
            .state
            .whatsapp_last_message_at
            .unwrap_or(self.state.whatsapp_opted_in_at.unwrap_or(now));
        let elapsed_hours = (now - last) / 3600;
        if elapsed_hours < keep_alive_hours as i64 {
            return;
        }
        let body = format!(
            "Claude Code Usage Monitor keep-alive. Reply with any message to confirm you still want alerts."
        );
        match self.send_whatsapp_session(&body) {
            Ok(_) => {
                self.state.whatsapp_last_message_at = Some(now);
                self.save();
            }
            Err(NotifError::SessionExpired) => {
                diagnose::log("WhatsApp keep-alive: session expired; clearing opt-in");
                self.reset_whatsapp_opt_in();
            }
            Err(error) => diagnose::log(format!("WhatsApp keep-alive failed: {error}")),
        }
    }

    pub fn send_test(&self) -> Result<NotifReport, NotifError> {
        if !self.has_api_key() {
            return Err(NotifError::NotConfigured);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let body = format!("Claude Code Usage Monitor test message at unix={now}");
        let subject = "Claude Code Usage Monitor: test notification";
        let mut report = NotifReport::default();

        if self.is_channel_active(Channel::Sms) && self.is_configured_for(Channel::Sms) {
            match self.send_sms(&body) {
                Ok(_) => report.sms = Some(Ok(())),
                Err(error) => report.sms = Some(Err(error.to_string())),
            }
        }
        if self.is_channel_active(Channel::Email) && self.is_configured_for(Channel::Email) {
            match self.send_email(subject, &body) {
                Ok(_) => report.email = Some(Ok(())),
                Err(error) => report.email = Some(Err(error.to_string())),
            }
        }
        if self.is_channel_active(Channel::WhatsApp)
            && self.is_configured_for(Channel::WhatsApp)
        {
            if !self.is_whatsapp_opted_in() {
                report.whatsapp = Some(Err(
                    "WhatsApp is not opted in. Use the menu to mark opt-in after sending the first message."
                        .to_string(),
                ));
            } else {
                match self.send_whatsapp_session(&body) {
                    Ok(_) => report.whatsapp = Some(Ok(())),
                    Err(NotifError::SessionExpired) => {
                        report.whatsapp = Some(Err("WhatsApp session expired (24h window closed). Re-trigger opt-in.".to_string()));
                    }
                    Err(error) => report.whatsapp = Some(Err(error.to_string())),
                }
            }
        }

        Ok(report)
    }

    fn send_sms(&self, body: &str) -> Result<(), NotifError> {
        let api_key = self.api_key()?;
        let to = self
            .config
            .sms
            .to
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;
        let shortcode = self
            .config
            .sms
            .shortcode
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;

        let payload = serde_json::json!({
            "channel": "SMS",
            "request": {
                "to": to,
                "shortcode": shortcode,
                "body": body,
            }
        });
        self.post_json(MESSANGI_NOTIFY_PATH, &api_key, &payload)
    }

    fn send_email(&self, subject: &str, body: &str) -> Result<(), NotifError> {
        let api_key = self.api_key()?;
        let to = self
            .config
            .email
            .to
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;
        let from = self
            .config
            .email
            .from
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;
        let external_id = format!("ccum-{}", now_unix_secs());

        let payload = serde_json::json!({
            "from": from,
            "to": to,
            "subject": subject,
            "text": body,
            "externalId": external_id,
        });
        self.post_json(MESSANGI_EMAIL_PATH, &api_key, &payload)
    }

    fn send_whatsapp_session(&self, body: &str) -> Result<(), NotifError> {
        let api_key = self.api_key()?;
        let from = self
            .config
            .whatsapp
            .from
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;
        let to = self
            .config
            .whatsapp
            .to
            .as_deref()
            .ok_or(NotifError::NotConfigured)?;

        let payload = serde_json::json!({
            "from": from,
            "to": to,
            "type": "text",
            "body": body,
        });
        self.post_json(MESSANGI_WHATSAPP_PATH, &api_key, &payload)
    }

    fn api_key(&self) -> Result<String, NotifError> {
        self.config
            .messangi_api_key
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or(NotifError::NotConfigured)
    }

    fn post_json(&self, path: &str, api_key: &str, payload: &serde_json::Value) -> Result<(), NotifError> {
        let base = self
            .config
            .messangi_base_url
            .as_deref()
            .unwrap_or(MESSANGI_DEFAULT_BASE_URL);
        let url = format!("{base}{path}");
        let agent = build_agent()?;
        let response = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json")
            .send_json(payload);
        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response
                    .into_string()
                    .unwrap_or_default();
                if is_session_expired(&detail) || code == 410 {
                    Err(NotifError::SessionExpired)
                } else {
                    Err(NotifError::Http { code, detail })
                }
            }
            Err(error) => Err(NotifError::Network(error.to_string())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Sms,
    Email,
    WhatsApp,
}

#[derive(Debug)]
pub enum NotifError {
    NotConfigured,
    SessionExpired,
    Network(String),
    Http { code: u16, detail: String },
}

impl std::fmt::Display for NotifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifError::NotConfigured => f.write_str("notifier not configured"),
            NotifError::SessionExpired => f.write_str("WhatsApp 24h session expired"),
            NotifError::Network(s) => write!(f, "network error: {s}"),
            NotifError::Http { code, detail } => {
                write!(f, "HTTP {code}: {}", truncate(detail, 200))
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct NotifReport {
    pub sms: Option<Result<(), String>>,
    pub email: Option<Result<(), String>>,
    pub whatsapp: Option<Result<(), String>>,
}

fn notifier_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("claude-code-usage-monitor")
        .join(NOTIFIER_FILE)
}

fn build_agent() -> Result<ureq::Agent, NotifError> {
    let tls = native_tls::TlsConnector::new().map_err(|e| NotifError::Network(e.to_string()))?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn is_session_expired(detail: &str) -> bool {
    detail.to_ascii_lowercase().contains("24 hours")
        || detail.to_ascii_lowercase().contains("re-engagement")
        || detail.to_ascii_lowercase().contains("session window")
}

fn humanize_provider(provider_id: &str) -> &'static str {
    match provider_id {
        "claude_code" => "Claude Code",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        "opencode" => "OpenCode",
        _ => "Usage",
    }
}

// Quiet the "unused" warning for the poller import alias; kept in case we
// reuse poll-level helpers later.
#[allow(dead_code)]
fn _force_link_pollar() {
    let _ = poller::format_line;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageData;

    fn make_data(claude_pct: f64) -> AppUsageData {
        AppUsageData {
            claude_code: Some(UsageData {
                session: crate::models::UsageSection {
                    percentage: claude_pct,
                    resets_at: None,
                },
                weekly: crate::models::UsageSection {
                    percentage: 0.0,
                    resets_at: None,
                },
                weekly_label: None,
            }),
            ..Default::default()
        }
    }

    fn make_notifier_with_thresholds() -> Notifier {
        let mut config = NotifierConfig::default();
        config.messangi_api_key = Some("test-key".to_string());
        let mut thresholds = std::collections::HashMap::new();
        thresholds.insert(
            "claude_code".to_string(),
            ProviderThresholds {
                session: vec![80],
                weekly: vec![],
            },
        );
        config.thresholds = thresholds;
        Notifier {
            config,
            state: NotifierState::default(),
            notify_sms: false,
            notify_email: false,
            notify_whatsapp: false,
            tracker: ThresholdTracker::default(),
            file_path: std::env::temp_dir().join("ccum-notifier-test.json"),
        }
    }

    #[test]
    fn threshold_rising_edge_marks_notified() {
        let mut n = make_notifier_with_thresholds();
        n.check_and_notify(&make_data(50.0));
        // 50% < 80%, no notification
        assert!(n.tracker.notified.is_empty());

        n.check_and_notify(&make_data(85.0));
        // 85% crossed 80%, but no channels are enabled so sent=false.
        // The internal tracker should still record the edge so a future
        // enable won't re-notify immediately.
        assert!(!n.tracker.notified.is_empty());
    }

    #[test]
    fn threshold_falling_edge_clears_notified() {
        let mut n = make_notifier_with_thresholds();
        n.check_and_notify(&make_data(85.0));
        assert!(!n.tracker.notified.is_empty());

        n.check_and_notify(&make_data(70.0));
        assert!(n.tracker.notified.is_empty());
    }

    #[test]
    fn no_alerts_without_api_key() {
        let mut n = make_notifier_with_thresholds();
        n.config.messangi_api_key = None;
        n.check_and_notify(&make_data(95.0));
        assert!(n.tracker.notified.is_empty());
    }

    #[test]
    fn file_roundtrip_preserves_config_and_state() {
        let path = std::env::temp_dir().join("ccum-notifier-roundtrip.json");
        let _ = std::fs::remove_file(&path);
        let mut n = Notifier {
            config: NotifierConfig::default(),
            state: NotifierState {
                whatsapp_opted_in_at: Some(1234),
                whatsapp_last_message_at: Some(5678),
                last_alert_at: None,
            },
            notify_sms: true,
            notify_email: false,
            notify_whatsapp: true,
            tracker: ThresholdTracker::default(),
            file_path: path.clone(),
        };
        n.config.messangi_api_key = Some("abc".to_string());
        n.config.thresholds.insert(
            "claude_code".to_string(),
            ProviderThresholds {
                session: vec![80, 95],
                weekly: vec![50],
            },
        );
        n.save();

        // Reload via the public API by re-opening the same path.
        let content = std::fs::read_to_string(&path).expect("read file");
        let file: NotifierFile = serde_json::from_str(&content).expect("parse");
        assert_eq!(file.config.messangi_api_key.as_deref(), Some("abc"));
        assert_eq!(file.state.whatsapp_opted_in_at, Some(1234));
        assert_eq!(file.notify_sms, Some(true));
        assert_eq!(file.notify_whatsapp, Some(true));
        let t = file
            .config
            .thresholds
            .get("claude_code")
            .expect("claude_code thresholds");
        assert_eq!(t.session, vec![80, 95]);
        assert_eq!(t.weekly, vec![50]);
        let _ = std::fs::remove_file(&path);
    }
}
