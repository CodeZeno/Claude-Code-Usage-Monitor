//! Polling providers for usage on a worker thread and retrying failed polls.
#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use super::*;

pub(super) fn request_poll(hwnd: HWND) {
    request_poll_inner(hwnd, true);
}

/// Request a timer-driven poll without extending an already-running poll cycle.
pub(super) fn request_scheduled_poll(hwnd: HWND) {
    request_poll_inner(hwnd, false);
}

fn request_poll_inner(hwnd: HWND, queue_if_busy: bool) {
    if POLL_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        if queue_if_busy {
            POLL_PENDING.store(true, Ordering::Release);
            diagnose::log("poll already running; manual refresh queued");
        }
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(controller_for(hwnd));
    std::thread::spawn(move || poll_worker(send_hwnd, !queue_if_busy));
}

/// Run credential watching under the same in-flight guard as usage polling.
/// Timer ticks cannot pile up workers, and manual refreshes still queue behind
/// a slow credential scan. Credential changes do not synthesize manual actions.
fn poll_worker(send_hwnd: SendHwnd, scheduled: bool) {
    run_poll_worker(&POLL_IN_FLIGHT, &POLL_PENDING, scheduled, |scheduled| {
        if !scheduled || scheduled_poll_needed() {
            do_poll_once(send_hwnd.to_hwnd());
        }
    });
}

fn run_poll_worker(
    in_flight: &AtomicBool,
    pending: &AtomicBool,
    mut scheduled: bool,
    mut poll: impl FnMut(bool),
) {
    loop {
        poll(scheduled);
        // Any queued request is an explicit refresh, not another timer tick.
        scheduled = false;
        if pending.swap(false, Ordering::AcqRel) {
            continue;
        }

        in_flight.store(false, Ordering::Release);
        if !pending.swap(false, Ordering::AcqRel) {
            break;
        }

        // A request can arrive between the pending check and releasing the
        // in-flight flag. Reacquire ownership unless that request already
        // started a replacement worker.
        if in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            break;
        }
    }
}

fn scheduled_poll_needed() -> bool {
    let watch = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return false;
        };
        if !state.auth_error_paused_polling {
            return true;
        }
        (
            state.auth_watch_mode,
            state.auth_watch_snapshot.clone(),
            state.providers,
            state.accounts.clone(),
        )
    };
    // No STATE lock is held while reading files, credentials, or WSL.
    let current = poller::credential_watch_snapshot(watch.0);
    current != watch.1
        && lock_state().as_ref().is_some_and(|state| {
            state.auth_error_paused_polling
                && state.auth_watch_mode == watch.0
                && state.auth_watch_snapshot == watch.1
                && state.providers == watch.2
                && state.accounts == watch.3
        })
}

fn is_partial_poll_retry_error(error: poller::PollError) -> bool {
    matches!(
        error,
        poller::PollError::NetworkError | poller::PollError::HttpStatus(429 | 500..=599)
    )
}

// Successful polls retry only transient account failures, including unselected
// accounts with carried stale data. Missing or stale readings alone do not
// establish that a failure is transient.
fn partial_poll_retry_delay_ms(
    data: &AppUsageData,
    enabled: ProviderSet,
    accounts: &crate::accounts::AccountSettings,
    retry_count: u32,
    interval_ms: u32,
    cooldown_ms: u32,
) -> Option<u32> {
    let needs_retry = enabled.iter().any(|provider| {
        if accounts.get(provider).is_some() {
            data.accounts.iter().any(|account| {
                account.provider == provider
                    && account.profile.enabled
                    && account.error.is_some_and(is_partial_poll_retry_error)
                    && account.usage.as_ref().is_none_or(|usage| usage.stale)
            })
        } else {
            // Providers without profiles do not retain their last error.
            false
        }
    });
    needs_retry.then(|| {
        let backoff =
            RETRY_BASE_MS.saturating_mul(1u32.checked_shl(retry_count).unwrap_or(u32::MAX));
        backoff.min(interval_ms).max(cooldown_ms).min(0x7fff_ffff)
    })
}

fn do_poll_once(hwnd: HWND) {
    let poll_started = Instant::now();
    let (enabled_providers, accounts, previous, force) = {
        let mut state = lock_state();
        state
            .as_mut()
            .map(|state| {
                (
                    state.providers,
                    state.accounts.clone(),
                    state.data.clone(),
                    std::mem::take(&mut state.force_notify_auth_error),
                )
            })
            .unwrap_or_default()
    };

    diagnose::log_lazy(|| format!("poll started providers={enabled_providers:?} force={force}"));
    let result = poller::poll(
        enabled_providers,
        &accounts,
        previous.as_ref(),
        force,
        |update| {
            let cache_data = {
                let mut state = lock_state();
                let Some(state) = state.as_mut() else {
                    return;
                };
                // A result from an old provider/account selection must not be shown.
                if state.providers != enabled_providers || state.accounts != accounts {
                    return;
                }
                let data = poller::merge_poll_progress(
                    update,
                    &state.data.clone().unwrap_or_default(),
                    &accounts,
                );
                state.data = Some(data.clone());
                state.last_poll_ok = true;
                state.last_poll_failure = None;
                data
            };
            // The dashboard runs separately and follows the same cache as the
            // widget. Publish before waiting for slower providers to finish.
            if let Err(error) = app_settings::save_usage_cache(&cache_data, true) {
                diagnose::log_error("unable to save partial usage cache", error);
            }
            // SAFETY: PostMessageW may be called from the poll worker for the controller
            // window; the HWND is passed by value and the message carries no pointers.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        },
    );
    match result {
        Ok(data) => {
            let mut state = lock_state();
            if state
                .as_ref()
                .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
            {
                return;
            }
            let mut data = match state.as_ref().and_then(|s| s.data.as_ref()) {
                Some(previous) => poller::carry_forward_failures(data, previous, enabled_providers),
                None => data,
            };
            data.select_accounts(&accounts);
            let notifications: Vec<_> = data
                .new_auth_failures(previous.as_ref(), force)
                .into_iter()
                .map(|account| (account.provider, account.profile.name.clone()))
                .collect();
            let language = state
                .as_ref()
                .map(|state| state.language)
                .unwrap_or(LanguageId::English);
            let cache_data = data.clone();
            if let Some(s) = state.as_mut() {
                // Stop fast-poll if reset data is now fresh
                if !poller::app_is_past_reset(&data) {
                    // SAFETY: kill_controller_timer forwards to KillTimer, which takes the
                    // controller HWND by value and no pointers; Windows validates it.
                    unsafe {
                        let _ = kill_controller_timer(Some(hwnd), TIMER_RESET_POLL);
                    }
                }

                let retry_ms = partial_poll_retry_delay_ms(
                    &data,
                    enabled_providers,
                    &accounts,
                    s.retry_count,
                    s.poll_interval_ms,
                    poller::retry_delay_ms(0, poll_started),
                );
                s.data = Some(data);
                s.last_poll_ok = true;
                s.last_poll_failure = None;

                // Retry partial failures; restore the normal interval after recovery.
                if let Some(retry_ms) = retry_ms {
                    s.retry_count = s.retry_count.saturating_add(1);
                    // SAFETY: the timer helpers forward to KillTimer/SetTimer with no
                    // TIMERPROC, so no callback pointer must stay valid; the HWND is
                    // passed by value.
                    unsafe {
                        let _ = kill_controller_timer(Some(hwnd), TIMER_RESET_POLL);
                        set_controller_timer(Some(hwnd), TIMER_POLL, retry_ms, None);
                    }
                } else if s.retry_count > 0 {
                    s.retry_count = 0;
                    let interval = s.poll_interval_ms;
                    // SAFETY: set_controller_timer forwards to SetTimer with no TIMERPROC, so no
                    // callback pointer must stay valid; the HWND is passed by value.
                    unsafe {
                        set_controller_timer(Some(hwnd), TIMER_POLL, interval, None);
                    }
                }
                s.auth_error_paused_polling = false;
                s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                    s.providers.first().unwrap_or_default(),
                );
                s.auth_watch_snapshot.clear();
            }
            drop(state);
            match app_settings::save_usage_cache(&cache_data, true) {
                Ok(()) => diagnose::log_lazy(|| {
                    format!(
                        "usage cache saved: accounts={} elapsed_ms={}",
                        cache_data.accounts.len(),
                        poll_started.elapsed().as_millis()
                    )
                }),
                Err(error) => diagnose::log_error("unable to save usage cache", error),
            }
            if !notifications.is_empty() {
                let body = notifications
                    .iter()
                    .map(|(provider, name)| {
                        format!(
                            "{} ({name}): {}",
                            language.text(provider.descriptor().display_name),
                            language.provider_auth_error(*provider).1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                tray_icon::notify_balloon(hwnd, language.text("Sign in again"), &body);
            }

            // SAFETY: PostMessageW may be called from the poll worker for the controller
            // window; the HWND is passed by value and the message carries no pointers.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(failure) => {
            diagnose::log_lazy(|| {
                format!(
                    "poll failed: {failure:?} elapsed_ms={}",
                    poll_started.elapsed().as_millis()
                )
            });
            if lock_state()
                .as_ref()
                .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
            {
                return;
            }
            let auth_watch = match failure.error {
                poller::PollError::AuthRequired
                | poller::PollError::TokenExpired
                | poller::PollError::HttpStatus(401 | 403) => {
                    let mode = poller::CredentialWatchMode::ActiveSource(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::NoCredentials => {
                    let mode = poller::CredentialWatchMode::AllSources(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::RequestFailed
                | poller::PollError::NetworkError
                | poller::PollError::UnexpectedResponse
                | poller::PollError::HttpStatus(_) => None,
            };
            // Distinguish auth-required errors from transient errors.
            let (notify_auth_error, cache_data, cache_poll_ok) = {
                let mut state = lock_state();
                if state
                    .as_ref()
                    .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
                {
                    return;
                }
                let mut should_notify = false;
                if let Some(s) = state.as_mut() {
                    if failure.error.is_transient() {
                        if let Some(previous) = s.data.as_ref() {
                            let carried = poller::carry_forward_failures(
                                AppUsageData::default(),
                                previous,
                                enabled_providers,
                            );
                            s.data = Some(carried);
                        }
                    }
                    s.last_poll_ok = false;
                    s.last_poll_failure = Some(failure);
                    match auth_watch {
                        Some((watch_mode, watch_snapshot)) => {
                            // Only show the balloon on the first failure so it doesn't spam.
                            if s.retry_count == 0 || force {
                                should_notify = true;
                            }
                            s.auth_error_paused_polling = true;
                            s.auth_watch_mode = watch_mode;
                            s.auth_watch_snapshot = watch_snapshot;
                            s.retry_count = s.retry_count.saturating_add(1);
                            // SAFETY: the timer helpers forward to KillTimer/SetTimer with
                            // no TIMERPROC, so no callback pointer must stay valid; the
                            // HWND is passed by value.
                            unsafe {
                                let _ = kill_controller_timer(Some(hwnd), TIMER_POLL);
                                let _ = kill_controller_timer(Some(hwnd), TIMER_RESET_POLL);
                                let _ = kill_controller_timer(Some(hwnd), TIMER_COUNTDOWN);
                                set_controller_timer(
                                    Some(hwnd),
                                    TIMER_POLL,
                                    s.poll_interval_ms,
                                    None,
                                );
                            }
                        }
                        _ => {
                            // Transient errors: exponential backoff, respecting server cooldowns.
                            s.auth_error_paused_polling = false;
                            s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                                s.providers.first().unwrap_or_default(),
                            );
                            s.auth_watch_snapshot.clear();
                            s.retry_count = s.retry_count.saturating_add(1);
                            let backoff = RETRY_BASE_MS.saturating_mul(
                                1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                            );
                            let retry_ms = poller::retry_delay_ms(
                                backoff.min(s.poll_interval_ms),
                                poll_started,
                            );
                            // SAFETY: the timer helpers forward to KillTimer/SetTimer with
                            // no TIMERPROC, so no callback pointer must stay valid; the
                            // HWND is passed by value.
                            unsafe {
                                let _ = kill_controller_timer(Some(hwnd), TIMER_RESET_POLL);
                                set_controller_timer(Some(hwnd), TIMER_POLL, retry_ms, None);
                            }
                        }
                    }
                }
                let cache_data = state
                    .as_ref()
                    .and_then(|state| state.data.clone())
                    .unwrap_or_default();
                let cache_poll_ok = state.as_ref().is_some_and(|state| {
                    poll_display_state(
                        state.last_poll_ok,
                        state.retry_count,
                        state.auth_error_paused_polling,
                        state.data.as_ref(),
                    )
                    .0
                });
                (should_notify, cache_data, cache_poll_ok)
            };
            // Theme Studio is a separate process and follows this cache. A
            // transient failure with usable stale data remains displayable;
            // hard failures and failures without a reading stay errors.
            let _ = app_settings::save_usage_cache(&cache_data, cache_poll_ok);

            if notify_auth_error {
                let balloon = {
                    let state = lock_state();
                    state
                        .as_ref()
                        .map(|state| state.language.provider_auth_error(failure.provider))
                };
                if let Some((title, body)) = balloon {
                    tray_icon::notify_balloon(hwnd, title, body);
                }
            }

            // SAFETY: PostMessageW may be called from the poll worker for the controller
            // window; the HWND is passed by value and the message carries no pointers.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(test)]
mod partial_poll_retry_tests {
    use super::*;
    use crate::accounts::{AccountProfile, AccountSettings};
    use crate::models::{AccountUsage, UsageData};

    fn partial_data() -> AppUsageData {
        let mut data = AppUsageData::from_iter([(ProviderId::Codex, UsageData::default())]);
        data.accounts.push(AccountUsage {
            provider: ProviderId::Claude,
            profile: AccountProfile::default(),
            source_signature: String::new(),
            source_path: None,
            usage: None,
            error: Some(poller::PollError::HttpStatus(429)),
            selected: false,
        });
        data
    }

    #[test]
    fn overall_success_with_failed_account_retries_with_backoff_and_cooldown() {
        let data = partial_data();
        assert!(
            !data.is_empty(),
            "another provider made the poll successful"
        );
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
        let accounts = AccountSettings::default();
        for (count, interval, cooldown, expected) in [
            (0, 900_000, 0, 30_000),
            (1, 900_000, 0, 60_000),
            (2, 900_000, 0, 120_000),
            (10, 900_000, 0, 900_000),
            (u32::MAX, 900_000, 0, 900_000),
            (0, 900_000, 120_000, 120_000),
            (0, 60_000, 120_000, 120_000),
        ] {
            assert_eq!(
                partial_poll_retry_delay_ms(&data, enabled, &accounts, count, interval, cooldown),
                Some(expected)
            );
        }
    }

    #[test]
    fn only_transient_account_failures_trigger_partial_retries() {
        use poller::PollError;

        let mut data = partial_data();
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
        let accounts = AccountSettings::default();
        for (error, expected) in [
            (PollError::NoCredentials, None),
            (PollError::AuthRequired, None),
            (PollError::TokenExpired, None),
            (PollError::RequestFailed, None),
            (PollError::UnexpectedResponse, None),
            (PollError::HttpStatus(400), None),
            (PollError::HttpStatus(401), None),
            (PollError::HttpStatus(403), None),
            (PollError::HttpStatus(404), None),
            (PollError::HttpStatus(408), None),
            (PollError::HttpStatus(499), None),
            (PollError::HttpStatus(600), None),
            (PollError::HttpStatus(429), Some(30_000)),
            (PollError::HttpStatus(500), Some(30_000)),
            (PollError::HttpStatus(503), Some(30_000)),
            (PollError::HttpStatus(599), Some(30_000)),
            (PollError::NetworkError, Some(30_000)),
        ] {
            data.accounts[0].error = Some(error);
            assert_eq!(
                partial_poll_retry_delay_ms(&data, enabled, &accounts, 0, 900_000, 0),
                expected,
                "{error:?}"
            );
        }
    }

    #[test]
    fn permanent_account_failure_does_not_keep_reset_polling_in_backoff() {
        let mut data = partial_data();
        data.accounts[0].error = Some(poller::PollError::NoCredentials);
        let mut healthy = UsageData::default();
        healthy.session.available = true;
        healthy.session.resets_at = Some(std::time::UNIX_EPOCH);
        data.insert(ProviderId::Codex, healthy);
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);

        // None takes the success path that clears retry_count, allowing the
        // countdown timer to schedule the five-second reset poll again.
        assert_eq!(
            partial_poll_retry_delay_ms(&data, enabled, &AccountSettings::default(), 3, 900_000, 0),
            None
        );
        assert!(poller::app_is_past_reset(&data));
    }

    #[test]
    fn stale_failed_accounts_retry_and_healthy_accounts_restore_normal_polling() {
        let mut data = partial_data();
        let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
        let accounts = AccountSettings::default();
        data.accounts[0].usage = Some(UsageData {
            stale: true,
            ..Default::default()
        });
        assert_eq!(
            partial_poll_retry_delay_ms(&data, enabled, &accounts, 1, 900_000, 0),
            Some(60_000)
        );
        data.accounts[0].usage.as_mut().unwrap().stale = false;
        data.accounts[0].error = None;
        assert_eq!(
            partial_poll_retry_delay_ms(&data, enabled, &accounts, 1, 900_000, 120_000),
            None
        );
    }

    #[test]
    fn disabled_accounts_and_providers_do_not_trigger_retries() {
        let mut data = partial_data();
        let accounts = AccountSettings::default();
        assert_eq!(
            partial_poll_retry_delay_ms(
                &data,
                ProviderSet::from_enabled([ProviderId::Codex]),
                &accounts,
                0,
                900_000,
                0,
            ),
            None
        );
        data.accounts[0].profile.enabled = false;
        assert_eq!(
            partial_poll_retry_delay_ms(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]),
                &accounts,
                0,
                900_000,
                0,
            ),
            None
        );
    }

    #[test]
    fn missing_or_stale_provider_without_error_does_not_retry() {
        let mut data = AppUsageData::from_iter([(ProviderId::Codex, UsageData::default())]);
        let enabled = ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Cursor]);
        let accounts = AccountSettings::default();
        assert_eq!(
            partial_poll_retry_delay_ms(&data, enabled, &accounts, 0, 900_000, 0),
            None
        );
        data.insert(
            ProviderId::Cursor,
            UsageData {
                stale: true,
                ..Default::default()
            },
        );
        assert_eq!(
            partial_poll_retry_delay_ms(&data, enabled, &accounts, 1, 900_000, 120_000),
            None
        );
    }
}

#[cfg(test)]
mod credential_watch_worker_tests {
    use super::*;

    #[test]
    fn a_manual_refresh_queued_during_a_slow_watch_is_not_lost() {
        let in_flight = AtomicBool::new(true);
        let pending = AtomicBool::new(false);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let in_flight = &in_flight;
            let pending = &pending;
            let worker = scope.spawn(move || {
                let mut calls = Vec::new();
                run_poll_worker(in_flight, pending, true, |scheduled| {
                    calls.push(scheduled);
                    if scheduled {
                        started_tx.send(()).unwrap();
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                });
                calls
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // A second timer tick cannot acquire the worker while discovery is
            // blocked, but an explicit refresh can queue for that worker.
            assert!(in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err());
            pending.store(true, Ordering::Release);
            resume_tx.send(()).unwrap();
            assert_eq!(worker.join().unwrap(), [true, false]);
        });
        assert!(!in_flight.load(Ordering::Acquire));
        assert!(!pending.load(Ordering::Acquire));
    }
}
