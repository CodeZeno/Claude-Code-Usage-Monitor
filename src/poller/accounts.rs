use super::*;
use crate::accounts::{file_signature, fingerprint, AccountProfile, AccountSettings};
use crate::models::AccountUsage;
use std::path::PathBuf;

#[derive(Clone)]
struct Target {
    provider: ProviderId,
    profile: Option<AccountProfile>,
    path: Result<Option<PathBuf>, String>,
}

impl Target {
    fn signature(&self) -> String {
        if self.profile.is_none() {
            return String::new();
        }
        match &self.path {
            Ok(Some(path)) => file_signature(path),
            Ok(None) => fingerprint(&format!(
                "{:?}",
                credential_watch_snapshot(CredentialWatchMode::ActiveSource(self.provider))
            )),
            Err(error) => fingerprint(error),
        }
    }
}

pub(super) fn poll_accounts(
    enabled: ProviderSet,
    settings: &AccountSettings,
) -> Result<AppUsageData, PollFailure> {
    poll_accounts_with(enabled, settings, |provider, path| match path {
        Some(path) => match provider {
            ProviderId::Claude => claude::poll_account(path),
            ProviderId::Codex => codex::poll_account(path),
            _ => poll_provider(provider),
        },
        None => poll_provider(provider),
    })
}

fn poll_accounts_with<F>(
    enabled: ProviderSet,
    settings: &AccountSettings,
    poll: F,
) -> Result<AppUsageData, PollFailure>
where
    F: Fn(ProviderId, Option<&std::path::Path>) -> Result<UsageData, PollError> + Sync,
{
    let mut targets = Vec::new();
    for provider in enabled.iter() {
        if let Some(accounts) = settings.get(provider) {
            for profile in accounts.profiles.iter().filter(|profile| profile.enabled) {
                let path = profile.credential_path(provider).map(|path| {
                    path.or_else(|| match provider {
                        ProviderId::Codex => codex::codex_auth_path(),
                        ProviderId::Claude => crate::accounts::environment_directory(provider)
                            .map(|directory| directory.join(".credentials.json"))
                            .or_else(claude::native_credential_path),
                        _ => None,
                    })
                });
                targets.push(Target {
                    provider,
                    profile: Some(profile.clone()),
                    path,
                });
            }
        } else {
            targets.push(Target {
                provider,
                profile: None,
                path: Ok(None),
            });
        }
    }
    // Profiles sharing a source must not race token refresh or credit writes.
    // Each distinct source has one job, while unrelated accounts run concurrently.
    let mut groups: Vec<Vec<Target>> = Vec::new();
    for target in targets {
        if let Some(group) = groups.iter_mut().find(|group| {
            group[0].provider == target.provider
                && source_key(&group[0].path) == source_key(&target.path)
        }) {
            group.push(target);
        } else {
            groups.push(vec![target]);
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results = std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel();
        for _ in 0..groups.len().min(MAX_CONCURRENT_PROVIDER_POLLS) {
            let sender = sender.clone();
            let groups = &groups;
            let next = &next;
            let poll = &poll;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(group) = groups.get(index) else {
                    break;
                };
                let target = &group[0];
                let mut signature = target.signature();
                let mut result = Err(PollError::RequestFailed);
                for _ in 0..2 {
                    result = match &target.path {
                        Err(_) => Err(PollError::NoCredentials),
                        Ok(path) => poll(target.provider, path.as_deref()),
                    };
                    let current_signature = target.signature();
                    if signature == current_signature {
                        break;
                    }
                    // Re-read once after login/token rotation; never associate
                    // a response (or a stale reading) with replaced credentials.
                    signature = current_signature;
                    result = Err(PollError::RequestFailed);
                }
                for target in group {
                    let _ = sender.send((index, target.clone(), signature.clone(), result.clone()));
                }
            });
        }
        drop(sender);
        receiver.into_iter().collect::<Vec<_>>()
    });
    results.sort_by_key(|(index, _, _, _)| *index);
    let mut data = AppUsageData::default();
    let mut first_error = None;
    for (_, target, signature, result) in results {
        if let Err(error) = &result {
            crate::diagnose::log(format!(
                "{} account {} usage poll failed: {error:?}",
                target.provider.descriptor().display_name,
                target
                    .profile
                    .as_ref()
                    .map(|profile| profile.name.as_str())
                    .unwrap_or("default")
            ));
            first_error.get_or_insert(PollFailure {
                provider: target.provider,
                error: *error,
            });
        }
        if let Some(profile) = target.profile {
            let error = result.as_ref().err().copied();
            data.accounts.push(AccountUsage {
                provider: target.provider,
                profile,
                source_signature: signature,
                source_path: target.path.ok().flatten(),
                usage: result.ok(),
                error,
                selected: false,
            });
        } else if let Ok(usage) = result {
            data.insert(target.provider, usage);
        }
    }
    data.select_accounts(settings);
    // Account failures are data too: publish their status to the dashboard,
    // and keep polling other accounts instead of pausing the whole provider.
    match first_error {
        Some(error) if data.accounts.is_empty() && data.is_empty() => Err(error),
        _ => Ok(data),
    }
}

fn source_key(path: &Result<Option<PathBuf>, String>) -> String {
    match path {
        Ok(Some(path)) => crate::accounts::source_key(path),
        other => format!("{other:?}"),
    }
}

pub(super) fn carry_accounts(fresh: &mut AppUsageData, previous: &AppUsageData) {
    for account in &mut fresh.accounts {
        if account.usage.is_some() {
            continue;
        }
        // Only transient failures may keep a reading, and only for the same
        // configured source and unchanged credentials file.
        if account.error != Some(PollError::RequestFailed) {
            continue;
        }
        if let Some(last) = previous
            .accounts
            .iter()
            .find(|last| {
                last.provider == account.provider
                    && last.profile.same_source(&account.profile)
                    && last.source_signature == account.source_signature
            })
            .and_then(|last| last.usage.as_ref())
        {
            let mut carried = last.clone();
            carried.stale = true;
            account.usage = Some(carried);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::ProviderAccounts;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn settings() -> AccountSettings {
        AccountSettings {
            claude: ProviderAccounts {
                profiles: ["personal", "work"]
                    .into_iter()
                    .map(|name| AccountProfile {
                        id: name.into(),
                        name: name.into(),
                        config_dir: format!("C:\\account-tests\\{name}"),
                        ..Default::default()
                    })
                    .collect(),
                selected: "work".into(),
            },
            ..Default::default()
        }
    }

    fn usage(percentage: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                available: true,
                percentage,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn failing_selected_account_never_borrows_another_accounts_usage() {
        let settings = settings();
        let data = poll_accounts_with(ProviderSet::default(), &settings, |_, path| {
            if path.unwrap().to_string_lossy().contains("work") {
                Err(PollError::AuthRequired)
            } else {
                Ok(usage(25.0))
            }
        })
        .unwrap();
        assert_eq!(data.accounts.len(), 2);
        assert!(data.get(ProviderId::Claude).is_none());
        assert_eq!(
            data.accounts[0].usage.as_ref().unwrap().session.percentage,
            25.0
        );
        assert_eq!(data.accounts[1].error, Some(PollError::AuthRequired));
        assert_eq!(data.selected_account_name(ProviderId::Claude), Some("work"));
    }

    #[test]
    fn stale_data_requires_the_same_profile_source_and_transient_error() {
        let settings = settings();
        let previous =
            poll_accounts_with(ProviderSet::default(), &settings, |_, _| Ok(usage(42.0))).unwrap();
        for (error, change_source, change_path, expected) in [
            (PollError::RequestFailed, false, false, true),
            (PollError::RequestFailed, true, false, false),
            (PollError::RequestFailed, false, true, false),
            (PollError::AuthRequired, false, false, false),
            (PollError::TokenExpired, false, false, false),
            (PollError::NoCredentials, false, false, false),
        ] {
            let mut fresh =
                poll_accounts_with(ProviderSet::default(), &settings, |_, _| Err(error)).unwrap();
            if change_source {
                fresh.accounts[1].source_signature.push('x');
            }
            if change_path {
                fresh.accounts[1].profile.config_dir.push('x');
            }
            carry_accounts(&mut fresh, &previous);
            assert_eq!(fresh.accounts[1].usage.is_some(), expected);
            if expected {
                assert!(fresh.accounts[1].usage.as_ref().unwrap().stale);
            }
        }
    }

    #[test]
    fn cache_round_trip_selection_removal_and_path_changes_are_isolated() {
        let mut settings = settings();
        let mut data = poll_accounts_with(ProviderSet::default(), &settings, |_, path| {
            Ok(usage(if path.unwrap().to_string_lossy().contains("work") {
                80.0
            } else {
                20.0
            }))
        })
        .unwrap();
        let json = serde_json::to_string(&data).unwrap();
        data = serde_json::from_str(&json).unwrap();
        assert_eq!(
            data.get(ProviderId::Claude).unwrap().session.percentage,
            80.0
        );
        settings.claude.selected = "personal".into();
        data.select_accounts(&settings);
        assert_eq!(
            data.get(ProviderId::Claude).unwrap().session.percentage,
            20.0
        );
        settings.claude.profiles[0].config_dir = "C:\\a-different-account".into();
        data.select_accounts(&settings);
        assert!(data.get(ProviderId::Claude).is_none());
        assert_eq!(data.accounts.len(), 1);
        settings.claude.profiles.clear();
        data.select_accounts(&settings);
        assert!(data.is_empty());
        assert!(data.accounts.is_empty());
    }

    #[test]
    fn invalid_or_disabled_profiles_make_no_requests() {
        let mut settings = settings();
        settings.claude.profiles[0].config_dir.clear();
        settings.claude.profiles[1].enabled = false;
        let data = poll_accounts_with(ProviderSet::default(), &settings, |_, _| {
            panic!("invalid profile polled")
        })
        .unwrap();
        assert_eq!(data.accounts.len(), 1);
        assert_eq!(data.accounts[0].error, Some(PollError::NoCredentials));
        assert!(data.accounts[0].usage.is_none());
    }

    #[test]
    fn account_concurrency_is_bounded_and_duplicate_sources_are_polled_once() {
        let mut settings = settings();
        for index in 2..6 {
            settings.claude.profiles.push(AccountProfile {
                id: format!("profile_{index}"),
                config_dir: format!("C:\\account-tests\\{index}"),
                ..Default::default()
            });
        }
        let mut duplicate = settings.claude.profiles[0].clone();
        duplicate.id = "alias".into();
        duplicate.config_dir = duplicate.config_dir.to_uppercase();
        settings.claude.profiles.push(duplicate);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        let data = poll_accounts_with(ProviderSet::default(), &settings, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(usage(0.0))
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_eq!(data.accounts.len(), 7);
        assert!(peak.load(Ordering::SeqCst) > 1);
        assert!(peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROVIDER_POLLS);
    }

    #[test]
    fn credential_rotation_during_poll_retries_before_publishing() {
        let directory = std::env::temp_dir().join(format!(
            "usage-account-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join(".credentials.json");
        std::fs::write(&path, "fixture before rotation").unwrap();
        let mut settings = settings();
        settings.claude.profiles.truncate(1);
        settings.claude.profiles[0].config_dir = directory.to_string_lossy().into_owned();
        let calls = AtomicUsize::new(0);
        let data = poll_accounts_with(ProviderSet::default(), &settings, |_, _| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                std::fs::write(&path, "fixture after token rotation, different length").unwrap();
                Ok(usage(10.0))
            } else {
                Ok(usage(80.0))
            }
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            data.get(ProviderId::Claude).unwrap().session.percentage,
            80.0
        );
        assert_eq!(data.accounts[0].source_signature, file_signature(&path));
        let mut cached = data;
        std::fs::write(&path, "another login").unwrap();
        cached.invalidate_changed_credentials();
        cached.select_accounts(&settings);
        assert!(cached.get(ProviderId::Claude).is_none());
        assert!(cached.accounts[0].usage.is_none());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
