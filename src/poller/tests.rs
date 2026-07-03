use super::*;

fn usage_with_session_percent(percentage: f64) -> UsageData {
    UsageData {
        session: UsageSection {
            percentage,
            resets_at: None,
        },
        weekly: UsageSection::default(),
        weekly_label: None,
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
fn antigravity_failure_does_not_block_codex_when_both_are_enabled() {
    let data = poll_with(
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Antigravity]),
        |provider| match provider {
            ProviderId::Claude => unreachable!("claude code is disabled"),
            ProviderId::Codex => Ok(usage_with_session_percent(42.0)),
            ProviderId::Antigravity => Err(PollError::NoCredentials),
            ProviderId::OpenCode => unreachable!("OpenCode is disabled"),
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
