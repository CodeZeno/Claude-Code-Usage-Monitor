use super::*;

fn usage_with_session_percent(percentage: f64) -> UsageData {
    UsageData {
        session: UsageSection {
            percentage,
            resets_at: None,
        },
        weekly: UsageSection::default(),
        weekly_label: None,
        monthly: None,
        credits: None,
        stale: false,
    }
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
