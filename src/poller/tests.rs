use super::*;

fn usage_with_session_percent(percentage: f64) -> UsageData {
    UsageData {
        limits: Vec::new(),
        session: UsageSection {
            available: true,
            percentage,
            resets_at: None,
        },
        weekly: UsageSection::default(),
        weekly_label: None,
        monthly: None,
        credits: None,
        stale: false,
        ..Default::default()
    }
}

#[test]
fn antigravity_keeps_reported_idle_windows_without_resets() {
    let quota = serde_json::from_str(r#"{"remainingFraction":1}"#).unwrap();
    let section = super::antigravity::antigravity_section_from_quota(quota).unwrap();
    assert!(section.available);
    assert_eq!(section.percentage, 0.0);
    assert!(section.resets_at.is_none());
    let summary = serde_json::from_str(
        r#"{
        "groups":[{"displayName":"Gemini","buckets":[{"window":"5h","remainingFraction":1}]}]
    }"#,
    )
    .unwrap();
    let data = super::antigravity::antigravity_usage_from_summary(summary).unwrap();
    assert!(data.session.available);
    assert_eq!(data.session.percentage, 0.0);
    assert!(data.session.resets_at.is_none());
    assert!(!data.weekly.available);
}

#[test]
fn idle_window_presence_survives_cached_poll_failures() {
    let previous = AppUsageData::from_iter([(ProviderId::Claude, usage_with_session_percent(0.0))]);
    let cached: AppUsageData =
        serde_json::from_str(&serde_json::to_string(&previous).unwrap()).unwrap();
    let carried = carry_forward_failures(
        AppUsageData::default(),
        &cached,
        ProviderSet::from_enabled([ProviderId::Claude]),
    );
    let usage = carried.get(ProviderId::Claude).unwrap();
    assert!(usage.stale);
    assert!(usage.session.available);
    assert_eq!(usage.session.percentage, 0.0);
    assert!(usage.session.resets_at.is_none());
    assert!(!usage.weekly.available);
}

#[test]
fn configured_https_transport_does_not_panic() {
    crate::https_test::assert_tls_handshake(build_agent().expect("HTTP agent should build"));
}

#[test]
#[ignore = "requires direct Internet access to github.com and badssl.com"]
fn configured_https_transport_validates_certificates() {
    crate::https_test::assert_https_certificate_validation(
        build_agent().expect("HTTP agent should build"),
    );
}

#[test]
fn stale_usage_does_not_trigger_reset_polling() {
    let mut usage = usage_with_session_percent(42.0);
    usage.session.resets_at = Some(UNIX_EPOCH);
    usage.stale = true;

    assert!(!is_past_reset(&usage));

    let mut app_usage = AppUsageData::default();
    app_usage.insert(ProviderId::Claude, usage);
    assert!(!app_is_past_reset(&app_usage));
}

#[test]
fn iso8601_parser_applies_timezone_offsets() {
    assert_eq!(
        parse_iso8601(Some("1970-01-01T01:00:00+01:00")),
        Some(UNIX_EPOCH)
    );
    assert_eq!(
        parse_iso8601(Some("1970-01-01T00:00:00-01:00")),
        Some(UNIX_EPOCH + Duration::from_secs(3_600))
    );
    assert_eq!(
        parse_iso8601(Some("2026-03-05T08:00:00.321598Z")),
        parse_iso8601(Some("2026-03-05T08:00:00+00:00"))
    );
}

#[test]
fn iso8601_parser_validates_calendar_and_time_fields() {
    assert!(parse_iso8601(Some("2024-02-29T23:59:59Z")).is_some());
    for invalid in [
        "2023-02-29T00:00:00Z",
        "2026-00-01T00:00:00Z",
        "2026-14-01T00:00:00Z",
        "2026-01-00T00:00:00Z",
        "2026-01-01T24:00:00Z",
        "2026-01-01T00:60:00Z",
        "2026-01-01T00:00:60Z",
        "2026-01-01T00:00:00.Z",
        "2026-01-01T00:00:00+24:00",
        "1969-12-31T23:59:59Z",
    ] {
        assert_eq!(parse_iso8601(Some(invalid)), None, "accepted {invalid}");
    }
}

#[test]
fn every_registered_provider_has_a_poller() {
    for provider in ProviderId::ALL {
        assert!(
            provider_poller(provider).is_some(),
            "{} is missing a poller registration",
            provider.descriptor().key
        );
    }
}

#[test]
fn claude_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| match provider {
            ProviderId::Claude => Err(PollError::AuthRequired),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Antigravity => unreachable!("antigravity is disabled"),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
            ProviderId::Copilot => unreachable!("Copilot is disabled"),
        },
    )
    .expect("codex data should keep the poll successful");

    assert!(data.get(ProviderId::Claude).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn codex_failure_does_not_block_claude_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| match provider {
            ProviderId::Claude => Ok(usage_with_session_percent(64.0)),
            ProviderId::Codex => Err(PollError::RequestFailed),
            ProviderId::Antigravity => unreachable!("antigravity is disabled"),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
            ProviderId::Copilot => unreachable!("Copilot is disabled"),
        },
    )
    .expect("claude data should keep the poll successful");

    assert_eq!(
        data.get(ProviderId::Claude).unwrap().session.percentage,
        64.0
    );
    assert!(data.get(ProviderId::Codex).is_none());
}

#[test]
fn returns_first_error_when_no_enabled_provider_succeeds() {
    let error = poll_with(
        ProviderSet::from_enabled(ProviderId::ALL),
        |provider| match provider {
            ProviderId::Claude => Err(PollError::AuthRequired),
            ProviderId::Codex => Err(PollError::RequestFailed),
            ProviderId::Antigravity => Err(PollError::NoCredentials),
            ProviderId::OpenCode => Err(PollError::NoCredentials),
            ProviderId::Cursor => Err(PollError::NoCredentials),
            ProviderId::Grok => Err(PollError::NoCredentials),
            ProviderId::Copilot => Err(PollError::NoCredentials),
        },
    )
    .expect_err("all-provider failure should return an error");

    assert_eq!(
        error,
        PollFailure {
            provider: ProviderId::Claude,
            error: PollError::AuthRequired,
        }
    );
}

#[test]
fn ready_providers_are_published_before_slow_providers_finish() {
    let (release, wait) = std::sync::mpsc::channel();
    let wait = std::sync::Mutex::new(wait);
    let mut published = Vec::new();
    let data = poll_concurrently_with_progress(
        ProviderSet::from_enabled([ProviderId::Cursor, ProviderId::OpenCode]),
        |provider| {
            if provider == ProviderId::OpenCode {
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            Ok(usage_with_session_percent(42.0))
        },
        |update| {
            let provider = update.iter().next().unwrap().0;
            published.push(provider);
            if provider == ProviderId::Cursor {
                release.send(()).unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(published, [ProviderId::Cursor, ProviderId::OpenCode]);
    assert_eq!(data.iter().count(), 2);
}

#[test]
fn progress_keeps_pending_provider_readings_until_they_finish() {
    let previous = AppUsageData::from_iter([
        (ProviderId::Cursor, usage_with_session_percent(10.0)),
        (ProviderId::OpenCode, usage_with_session_percent(20.0)),
    ]);
    let update = AppUsageData::from_iter([(ProviderId::Cursor, usage_with_session_percent(15.0))]);
    let merged = merge_poll_progress(
        update,
        &previous,
        &crate::accounts::AccountSettings::default(),
    );
    assert_eq!(
        merged.get(ProviderId::Cursor).unwrap().session.percentage,
        15.0
    );
    assert_eq!(
        merged.get(ProviderId::OpenCode),
        previous.get(ProviderId::OpenCode)
    );
}

#[test]
fn concurrent_polling_is_bounded_and_preserves_results() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let data = poll_concurrently_with(ProviderSet::from_enabled(ProviderId::ALL), |provider| {
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        active.fetch_sub(1, Ordering::SeqCst);
        Ok(usage_with_session_percent(f64::from(provider as u8)))
    })
    .expect("all concurrent provider polls should succeed");

    assert_eq!(data.iter().count(), ProviderId::ALL.len());
    assert!(peak.load(Ordering::SeqCst) > 1);
    assert!(peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROVIDER_POLLS);
}

#[test]
fn concurrent_polling_reports_the_first_provider_error_deterministically() {
    let error = poll_concurrently_with(
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
        |provider| {
            if provider == ProviderId::Claude {
                std::thread::sleep(Duration::from_millis(20));
                Err(PollError::AuthRequired)
            } else {
                Err(PollError::RequestFailed)
            }
        },
    )
    .expect_err("both provider polls should fail");

    assert_eq!(
        error,
        PollFailure {
            provider: ProviderId::Claude,
            error: PollError::AuthRequired,
        }
    );
}

#[test]
fn antigravity_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Antigravity]),
        |provider| match provider {
            ProviderId::Claude => unreachable!("claude code is disabled"),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Antigravity => Err(PollError::NoCredentials),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
            ProviderId::Copilot => unreachable!("Copilot is disabled"),
        },
    )
    .expect("codex data should keep the poll successful");

    assert!(data.get(ProviderId::Antigravity).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn opencode_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::OpenCode]),
        |provider| match provider {
            ProviderId::Claude => unreachable!("Claude Code is disabled"),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Antigravity => unreachable!("Antigravity is disabled"),
            ProviderId::OpenCode => Err(PollError::NoCredentials),
            ProviderId::Cursor => unreachable!("Cursor is disabled"),
            ProviderId::Grok => unreachable!("Grok is disabled"),
            ProviderId::Copilot => unreachable!("Copilot is disabled"),
        },
    )
    .expect("Codex data should keep the poll successful");

    assert!(data.get(ProviderId::OpenCode).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn cursor_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Cursor]),
        |provider| match provider {
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Cursor => Err(PollError::NoCredentials),
            _ => unreachable!("provider is disabled"),
        },
    )
    .expect("Codex data should keep the poll successful");

    assert!(data.get(ProviderId::Cursor).is_none());
    assert_eq!(
        data.get(ProviderId::Codex).unwrap().session.percentage,
        42.0
    );
}

#[test]
fn antigravity_summary_prefers_gemini_group() {
    let response: antigravity::AntigravityQuotaSummaryResponse = serde_json::from_str(
        r#"{
                "groups": [
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "bucketId": "3p-weekly",
                                "window": "weekly",
                                "resetTime": "2026-06-20T18:32:02Z",
                                "remainingFraction": 1
                            },
                            {
                                "bucketId": "3p-5h",
                                "window": "5h",
                                "resetTime": "2026-06-13T23:32:02Z",
                                "remainingFraction": 1
                            }
                        ]
                    },
                    {
                        "displayName": "Gemini Models",
                        "description": "Models within this group: Gemini Flash, Gemini Pro",
                        "buckets": [
                            {
                                "bucketId": "gemini-weekly",
                                "displayName": "Weekly Limit",
                                "window": "weekly",
                                "resetTime": "2026-06-20T17:08:54Z",
                                "remainingFraction": 0.99304295
                            },
                            {
                                "bucketId": "gemini-5h",
                                "displayName": "Five Hour Limit",
                                "window": "5h",
                                "resetTime": "2026-06-13T22:08:54Z",
                                "remainingFraction": 0.9582575
                            }
                        ]
                    }
                ]
            }"#,
    )
    .expect("summary response should deserialize");

    let usage = antigravity::antigravity_usage_from_summary(response)
        .expect("Gemini quota should be selected");

    assert!((usage.weekly.percentage - 0.695705).abs() < 0.000001);
    assert!((usage.session.percentage - 4.17425).abs() < 0.000001);
    assert!(usage.weekly.resets_at.is_some());
    assert!(usage.session.resets_at.is_some());
}

#[test]
fn one_provider_failing_does_not_blank_its_row() {
    let previous: AppUsageData = [
        (ProviderId::Claude, usage_with_session_percent(21.0)),
        (ProviderId::Codex, usage_with_session_percent(3.0)),
    ]
    .into_iter()
    .collect();

    // Only Codex answered this cycle.
    let fresh: AppUsageData = [(ProviderId::Codex, usage_with_session_percent(9.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        fresh,
        &previous,
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
    );

    let claude = merged.get(ProviderId::Claude).expect("claude is kept");
    assert_eq!(claude.session.percentage, 21.0);
    assert!(claude.stale, "a carried reading must be marked stale");

    let codex = merged.get(ProviderId::Codex).expect("codex refreshed");
    assert_eq!(codex.session.percentage, 9.0);
    assert!(!codex.stale, "a fresh reading must not be marked stale");
}

#[test]
fn a_disabled_provider_is_not_resurrected() {
    let previous: AppUsageData = [(ProviderId::Claude, usage_with_session_percent(21.0))]
        .into_iter()
        .collect();
    let fresh: AppUsageData = [(ProviderId::Codex, usage_with_session_percent(9.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        fresh,
        &previous,
        ProviderSet::from_enabled([ProviderId::Codex]),
    );

    assert!(merged.get(ProviderId::Claude).is_none());
}

#[test]
fn all_failed_providers_can_carry_their_previous_readings() {
    let previous: AppUsageData = [(ProviderId::Claude, usage_with_session_percent(21.0))]
        .into_iter()
        .collect();

    let merged = carry_forward_failures(
        AppUsageData::default(),
        &previous,
        ProviderSet::from_enabled([ProviderId::Claude]),
    );

    let claude = merged.get(ProviderId::Claude).expect("claude is kept");
    assert_eq!(claude.session.percentage, 21.0);
    assert!(claude.stale, "the carried reading must be marked stale");
}

#[test]
fn antigravity_gemini_weekly_reset_projection_tests() {
    use std::time::{Duration, SystemTime};

    fn format_iso8601_for_test(time: SystemTime) -> String {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let days = secs / 86400;
        let rem = secs % 86400;
        let hours = rem / 3600;
        let minutes = (rem % 3600) / 60;
        let seconds = rem % 60;
        let mut year = 1970;
        let mut d = days;
        loop {
            let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            let y_days = if leap { 366 } else { 365 };
            if d < y_days {
                let mut month = 1;
                let month_days = [
                    31,
                    if leap { 29 } else { 28 },
                    31,
                    30,
                    31,
                    30,
                    31,
                    31,
                    30,
                    31,
                    30,
                    31,
                ];
                for &m_len in &month_days {
                    if d < m_len {
                        return format!(
                            "{year:04}-{month:02}-{:02}T{hours:02}:{minutes:02}:{seconds:02}Z",
                            d + 1
                        );
                    }
                    d -= m_len;
                    month += 1;
                }
            }
            d -= y_days;
            year += 1;
        }
    }

    let now = SystemTime::now();

    // 1. A weekly reset that is close is reported as-is: the API's resetTime is
    //    authoritative and no "+6 days" projection is applied.
    let reset_16h = now + Duration::from_secs(16 * 3600);
    let reset_iso_16h = format_iso8601_for_test(reset_16h);
    let bucket_json = serde_json::json!({
        "bucketId": "gemini-weekly",
        "displayName": "Weekly Limit",
        "window": "weekly",
        "resetTime": reset_iso_16h,
        "remainingFraction": 0.80
    });
    let bucket: antigravity::AntigravityQuotaSummaryBucket =
        serde_json::from_value(bucket_json).unwrap();
    let section =
        antigravity::antigravity_section_from_summary_bucket(&bucket).expect("section exists");
    assert!((section.percentage - 20.0).abs() < 0.001);
    let reported_reset = section.resets_at.expect("reset exists");
    let diff = reported_reset
        .duration_since(reset_16h)
        .unwrap_or_else(|e| e.duration());
    assert!(
        diff < Duration::from_secs(2),
        "close weekly reset must not be projected"
    );

    // 2. gemini-weekly with reset in the past (-1 hour)
    let reset_past = now - Duration::from_secs(3600);
    let reset_iso_past = format_iso8601_for_test(reset_past);
    let bucket_past_json = serde_json::json!({
        "bucketId": "gemini-weekly",
        "window": "weekly",
        "resetTime": reset_iso_past,
        "remainingFraction": 0.80
    });
    let bucket_past: antigravity::AntigravityQuotaSummaryBucket =
        serde_json::from_value(bucket_past_json).unwrap();
    let section_past =
        antigravity::antigravity_section_from_summary_bucket(&bucket_past).expect("section exists");
    let past_reset = section_past.resets_at.expect("reset exists");
    let diff_past = past_reset
        .duration_since(reset_past)
        .unwrap_or_else(|e| e.duration());
    assert!(
        diff_past < Duration::from_secs(2),
        "Past reset must not have +6d added"
    );

    // 3. 3p-weekly with reset in 16 hours (MUST NOT have +6d added)
    let bucket_3p_json = serde_json::json!({
        "bucketId": "3p-weekly",
        "window": "weekly",
        "resetTime": reset_iso_16h,
        "remainingFraction": 0.50
    });
    let bucket_3p: antigravity::AntigravityQuotaSummaryBucket =
        serde_json::from_value(bucket_3p_json).unwrap();
    let section_3p =
        antigravity::antigravity_section_from_summary_bucket(&bucket_3p).expect("section exists");
    let reset_3p = section_3p.resets_at.expect("reset exists");
    let diff_3p = reset_3p
        .duration_since(reset_16h)
        .unwrap_or_else(|e| e.duration());
    assert!(
        diff_3p < Duration::from_secs(2),
        "3p-weekly must not have +6d added"
    );
}

#[test]
fn antigravity_models_quota_extracts_gemini_and_external_correctly() {
    let models_json = r#"{
        "models": {
            "gemini-2.5-flash": {
                "quotaInfo": {
                    "remainingFraction": 0.66,
                    "resetTime": "2026-10-17T16:00:00Z"
                }
            },
            "gemini-3-flash": {
                "quotaInfo": {
                    "remainingFraction": 0.80,
                    "resetTime": "2026-10-17T16:00:00Z"
                }
            },
            "claude-sonnet-4-6": {
                "quotaInfo": {
                    "remainingFraction": 0.02,
                    "resetTime": "2099-10-22T18:00:00Z"
                }
            }
        }
    }"#;
    let resp: antigravity::AntigravityModelsResponse = serde_json::from_str(models_json).unwrap();
    let gemini = antigravity::best_gemini_model_quota(&resp).expect("gemini quota exists");
    assert!((gemini.percentage - 34.0).abs() < 0.001);
}

#[test]
fn antigravity_first_summary_wins_and_model_quotas_are_only_a_last_resort() {
    use antigravity::AntigravitySource::{Models, Summary};
    use std::cell::RefCell;

    let with_session = |percentage| UsageData {
        session: UsageSection {
            available: true,
            percentage,
            resets_at: None,
        },
        ..Default::default()
    };
    let endpoints = ["a", "b", "c"];

    // A weekly-only summary is final: the next endpoint's model quota must not
    // fill its 5h window, and later endpoints are not asked at all.
    let asked = RefCell::new(Vec::new());
    let data = antigravity::select_antigravity_usage(&endpoints, |url| {
        asked.borrow_mut().push(url.to_string());
        match url {
            "a" => Err(PollError::RequestFailed),
            "b" => Ok((
                UsageData {
                    weekly: with_session(40.0).session,
                    ..Default::default()
                },
                Summary,
            )),
            _ => Ok((with_session(80.0), Models)),
        }
    })
    .unwrap();
    assert!(!data.session.available);
    assert!(data.weekly.available);
    assert_eq!(data.weekly.percentage, 40.0);
    assert_eq!(*asked.borrow(), ["a", "b"]);

    // An authoritative empty summary overrides an earlier model fallback.
    let data = antigravity::select_antigravity_usage(&endpoints, |url| match url {
        "a" => Ok((with_session(30.0), Models)),
        "b" => Ok((UsageData::default(), Summary)),
        _ => panic!("later endpoints must not be asked after a summary"),
    })
    .unwrap();
    assert!(!data.session.available && !data.weekly.available);

    // Without any summary the model quota is used.
    let data = antigravity::select_antigravity_usage(&endpoints, |url| match url {
        "a" => Ok((with_session(30.0), Models)),
        _ => Err(PollError::RequestFailed),
    })
    .unwrap();
    assert!((data.session.percentage - 30.0).abs() < 0.001);
}

#[test]
fn antigravity_model_fallback_reports_only_gemini_models() {
    let models: antigravity::AntigravityModelsResponse = serde_json::from_str(
        r#"{"models":{
            "gemini-3-pro":{"quotaInfo":{"remainingFraction":0.9}},
            "claude-opus-4-6-thinking":{"quotaInfo":{"remainingFraction":0.1}},
            "gpt-oss-120b-medium":{"quotaInfo":{"remainingFraction":0.5}},
            "imagen-4":{"quotaInfo":{"remainingFraction":0.0}}
        }}"#,
    )
    .unwrap();
    let usage = antigravity::antigravity_usage_from_models(&models).expect("usage");
    assert!(
        (usage.session.percentage - 10.0).abs() < 0.001,
        "Gemini pool must ignore Claude/GPT/image models"
    );
    // Model quotas do not name their window, so the external pool is not guessed.
    assert!(usage.external_pool.is_none());
}

#[test]
fn antigravity_summary_does_not_guess_pools_by_position() {
    // An unnamed group with 3p-* buckets is the external pool, never Gemini.
    let response: antigravity::AntigravityQuotaSummaryResponse = serde_json::from_str(
        r#"{"groups":[{"buckets":[
            {"bucketId":"3p-weekly","window":"weekly","remainingFraction":0.4},
            {"bucketId":"3p-5h","window":"5h","remainingFraction":1}
        ]}]}"#,
    )
    .unwrap();
    let usage = antigravity::antigravity_usage_from_summary(response).expect("usage");
    assert!(
        !usage.session.available && !usage.weekly.available,
        "Gemini pool must stay empty"
    );
    let external = usage.external_pool.expect("external pool");
    assert!((external.weekly.percentage - 60.0).abs() < 0.001);
}

#[test]
fn antigravity_summary_without_windows_is_an_answer_not_a_failure() {
    let response: antigravity::AntigravityQuotaSummaryResponse =
        serde_json::from_str(r#"{"groups":[{"displayName":"Gemini models","buckets":[]}]}"#)
            .unwrap();
    let usage = antigravity::antigravity_usage_from_summary(response).expect("authoritative");
    assert!(!usage.session.available && !usage.weekly.available);
    assert!(usage.external_pool.is_none());

    let empty: antigravity::AntigravityQuotaSummaryResponse =
        serde_json::from_str(r#"{"groups":[]}"#).unwrap();
    assert!(antigravity::antigravity_usage_from_summary(empty).is_some());
}

#[test]
fn antigravity_extracts_both_gemini_and_external_groups_from_summary() {
    let summary = serde_json::from_str(
        r#"{
        "groups": [
            {
                "displayName": "Claude and GPT models",
                "buckets": [
                    {"window": "5h", "remainingFraction": 0.4},
                    {"window": "weekly", "remainingFraction": 0.2}
                ]
            },
            {
                "displayName": "Gemini Models",
                "buckets": [
                    {"window": "5h", "remainingFraction": 0.85},
                    {"window": "weekly", "remainingFraction": 0.70}
                ]
            }
        ]
    }"#,
    )
    .unwrap();
    let data = super::antigravity::antigravity_usage_from_summary(summary).unwrap();
    assert!(data.session.available);
    assert!((data.session.percentage - 15.0).abs() < 1e-4);
    assert!(data.weekly.available);
    assert!((data.weekly.percentage - 30.0).abs() < 1e-4);

    let ext = data
        .external_pool
        .as_deref()
        .expect("external pool extracted");
    assert!(ext.session.available);
    assert!((ext.session.percentage - 60.0).abs() < 1e-4);
    assert!(ext.weekly.available);
    assert!((ext.weekly.percentage - 80.0).abs() < 1e-4);
}

#[test]
fn antigravity_empty_named_pool_blocks_unnamed_quota_fallback() {
    for name in ["Gemini models", "Claude and GPT models"] {
        for buckets in [
            serde_json::json!([]),
            serde_json::json!([
                {"window":"weekly"}
            ]),
        ] {
            let response = serde_json::from_value(serde_json::json!({
                "groups": [
                    {"displayName": name, "buckets": buckets},
                    {"buckets": [{"window":"5h", "remainingFraction":0.2}]}
                ]
            }))
            .unwrap();
            let usage = antigravity::antigravity_usage_from_summary(response).unwrap();
            assert!(
                !usage.session.available && !usage.weekly.available,
                "an empty named pool ({name}) must not use an unnamed pool's quota"
            );
        }
    }
}

#[test]
fn antigravity_unnamed_quota_fallback_requires_a_single_pool() {
    for count in [1, 2] {
        let response = serde_json::from_value(serde_json::json!({
            "groups": vec![serde_json::json!({
                "buckets": [{"window":"5h", "remainingFraction":0.2}]
            }); count]
        }))
        .unwrap();
        let usage = antigravity::antigravity_usage_from_summary(response).unwrap();
        assert_eq!(usage.session.available, count == 1);
        if count == 1 {
            assert!((usage.session.percentage - 80.0).abs() < 0.001);
        }
    }
}

#[test]
fn failed_antigravity_poll_preserves_and_marks_both_pools_stale() {
    let mut gemini = usage_with_session_percent(20.0);
    let mut external = usage_with_session_percent(70.0);
    external.session.resets_at = Some(UNIX_EPOCH);
    gemini.external_pool = Some(Box::new(external));
    let previous = AppUsageData::from_iter([(ProviderId::Antigravity, gemini)]);
    assert!(
        app_is_past_reset(&previous),
        "external resets must be polled too"
    );
    let carried = carry_forward_failures(
        AppUsageData::default(),
        &previous,
        ProviderSet::from_enabled([ProviderId::Antigravity]),
    );
    let cached: AppUsageData =
        serde_json::from_value(serde_json::to_value(carried).unwrap()).unwrap();
    let gemini = cached.get(ProviderId::Antigravity).unwrap();
    let external = cached.antigravity_external().unwrap();
    assert!(gemini.stale && external.stale);
    assert_eq!(gemini.session.percentage, 20.0);
    assert_eq!(external.session.percentage, 70.0);
    assert!(
        !app_is_past_reset(&cached),
        "stale external resets must not cause a poll loop"
    );
}

#[test]
fn antigravity_summary_exposes_every_group_and_window_as_limits() {
    let summary = serde_json::from_value(serde_json::json!({
        "groups": [
            {"displayName": "Gemini 4 Argon",
             "buckets": [{"bucketId": "gemini-4-argon", "window": "weekly", "remainingFraction": 0.9}]},
            {"displayName": "Claude and GPT models",
             "buckets": [{"window": "5h", "remainingFraction": 0.4},
                         {"window": "weekly", "remainingFraction": 0.2}]},
            {"displayName": "Gemini Models",
             "buckets": [{"window": "5h", "remainingFraction": 0.85},
                         {"window": "weekly", "remainingFraction": 0.70}]}
        ]
    }))
    .unwrap();
    let data = super::antigravity::antigravity_usage_from_summary(summary).unwrap();

    // Argon comes first but has its own pool; the main pool stays "Gemini Models".
    assert!((data.session.percentage - 15.0).abs() < 1e-4);
    assert!((data.weekly.percentage - 30.0).abs() < 1e-4);

    let keys: Vec<&str> = data.limits.iter().map(|limit| limit.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "gemini_4_argon_weekly",
            "claude_and_gpt_models_session",
            "claude_and_gpt_models_weekly",
            "gemini_models_session",
            "gemini_models_weekly",
        ]
    );
    let argon = &data.limits[0];
    assert_eq!(argon.kind, "weekly");
    assert_eq!(argon.model_id.as_deref(), Some("gemini-4-argon"));
    assert!((argon.usage.percentage - 10.0).abs() < 1e-4);

    // The cache and theme context must retain every limit alongside the two pools.
    let app = AppUsageData::from_iter([(ProviderId::Antigravity, data)]);
    let restored: AppUsageData =
        serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
    assert_eq!(restored, app);
    let context = crate::theme_engine::DataContext::from_usage(
        Some(&restored),
        &crate::theme_engine::Canvas::default(),
    );
    for (key, percentage) in [
        ("gemini_4_argon_weekly", 10.0),
        ("claude_and_gpt_models_session", 60.0),
        ("claude_and_gpt_models_weekly", 80.0),
        ("gemini_models_session", 15.0),
        ("gemini_models_weekly", 30.0),
    ] {
        let value = context
            .get(&format!("antigravity.limits.{key}.percentage"))
            .expect("theme binding");
        assert!((value - percentage).abs() < 1e-4, "{key}");
    }
}

#[test]
fn antigravity_argon_never_fills_the_main_pool_without_other_named_pools() {
    for (identity, bucket_identity) in [
        (
            serde_json::json!({"displayName": "Gemini 4 Argon"}),
            serde_json::json!({}),
        ),
        (
            serde_json::json!({"description": "Gemini 4 ARGON models"}),
            serde_json::json!({}),
        ),
        (
            serde_json::json!({}),
            serde_json::json!({"bucketId": "gemini-4-argon-weekly"}),
        ),
        (
            serde_json::json!({}),
            serde_json::json!({"displayName": "Gemini 4 Argon Weekly"}),
        ),
    ] {
        for include_unknown in [false, true] {
            let mut argon = identity.clone();
            let mut bucket = bucket_identity.clone();
            bucket["window"] = serde_json::json!("weekly");
            bucket["remainingFraction"] = serde_json::json!(0.1);
            argon["buckets"] = serde_json::json!([bucket]);
            let mut groups = vec![argon];
            if include_unknown {
                groups.push(serde_json::json!({
                    "displayName": "Unknown pool",
                    "buckets": [{"window": "5h", "remainingFraction": 0.2}]
                }));
            }
            let summary = serde_json::from_value(serde_json::json!({"groups": groups})).unwrap();
            let usage = antigravity::antigravity_usage_from_summary(summary).unwrap();
            assert!(
                !usage.session.available && !usage.weekly.available,
                "Argon and unrelated groups must not become the main Gemini pool: {identity}"
            );
            assert!(usage.external_pool.is_none());
        }
    }
}

#[test]
fn antigravity_limits_skip_unusable_buckets_and_preserve_reported_windows() {
    let summary = serde_json::from_value(serde_json::json!({"groups": [
        {"displayName": " ", "buckets": [{"window": "5h", "remainingFraction": 0.1}]},
        {"displayName": "Gemini Models", "buckets": [
            {"window": "daily", "remainingFraction": 0.2},
            {"window": "weekly"},
            {"window": "5H", "remainingFraction": 1.5, "resetTime": "2099-01-01T00:00:00Z"},
            {"window": "WEEKLY", "remainingFraction": -0.5}
        ]}
    ]}))
    .unwrap();
    let usage = antigravity::antigravity_usage_from_summary(summary).unwrap();
    assert_eq!(usage.limits.len(), 2);
    assert_eq!(usage.limits[0].key, "gemini_models_session");
    assert_eq!(usage.limits[0].usage, usage.session);
    assert_eq!(usage.limits[0].usage.percentage, 0.0);
    assert_eq!(
        usage.limits[0].usage.resets_at,
        parse_iso8601(Some("2099-01-01T00:00:00Z"))
    );
    assert_eq!(usage.limits[1].key, "gemini_models_weekly");
    assert_eq!(usage.limits[1].usage, usage.weekly);
    assert_eq!(usage.limits[1].usage.percentage, 100.0);
}
