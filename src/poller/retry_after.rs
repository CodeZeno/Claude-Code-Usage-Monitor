use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use ureq::http::Request;
use ureq::middleware::MiddlewareNext;
use ureq::SendBody;

use super::HttpResponse;

// Bound the stored cooldown so a bad header cannot lock out an account.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(15 * 60);
const MANUAL_RETRY_AFTER: Duration = Duration::from_secs(60);
const DEFAULT_RATE_LIMIT_DELAY: Duration = Duration::from_secs(60);

thread_local! {
    static MANUAL_REFRESH: Cell<bool> = const { Cell::new(false) };
}

// Provider requests run synchronously on poll workers, so keep the manual
// refresh context on that worker rather than on the shared HTTP agent.
pub(super) fn with_manual_refresh<T>(manual: bool, poll: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            MANUAL_REFRESH.set(self.0);
        }
    }
    let _reset = Reset(MANUAL_REFRESH.replace(manual));
    poll()
}

/// Header probes need fresh responses even while the account is rate limited.
/// This request extension is local metadata and is never sent to the server.
#[derive(Clone, Copy)]
pub(super) struct BypassCooldown;

#[derive(Clone, Copy)]
struct Cooldown {
    received: Instant,
    last_used: Instant,
    last_probe: Option<Instant>,
    delay: Duration,
    status: u16,
}

impl Cooldown {
    fn remaining(self, now: Instant) -> Duration {
        self.delay.saturating_sub(now.duration_since(self.received))
    }
}

#[derive(Default)]
struct RetryAfter {
    cooldowns: Mutex<HashMap<u64, Cooldown>>,
}

fn shared() -> &'static RetryAfter {
    static RETRY_AFTER: OnceLock<RetryAfter> = OnceLock::new();
    RETRY_AFTER.get_or_init(RetryAfter::default)
}

// Keep credentials out of stored keys. Include headers to distinguish accounts
// using the same URL (including cookie and workspace-based authentication).
// Keys never leave the process, so a randomly keyed SipHash is sufficient.
fn request_key(request: &Request<SendBody>) -> u64 {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    let mut hasher = KEYS.get_or_init(RandomState::new).build_hasher();
    request.method().as_str().hash(&mut hasher);
    request.uri().to_string().hash(&mut hasher);
    for (name, value) in request.headers() {
        name.as_str().hash(&mut hasher);
        value.as_bytes().hash(&mut hasher);
    }
    hasher.finish()
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    let delay = if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Duration::from_secs(value.parse().unwrap_or(MAX_RETRY_AFTER.as_secs()))
    } else {
        let deadline = httpdate::parse_http_date(value).ok()?;
        deadline.duration_since(now).unwrap_or_default()
    };
    Some(delay.min(MAX_RETRY_AFTER))
}

impl RetryAfter {
    fn handle(
        &self,
        request: Request<SendBody>,
        send: impl FnOnce(Request<SendBody>) -> Result<HttpResponse, ureq::Error>,
    ) -> Result<HttpResponse, ureq::Error> {
        // Host and path only: queries and headers can carry account data.
        let target = format!(
            "{} {}{}",
            request.method(),
            request.uri().host().unwrap_or(""),
            request.uri().path()
        );
        if request.extensions().get::<BypassCooldown>().is_some() {
            let response = send(request);
            log_http(&target, "header probe", &response);
            return response;
        }
        let key = request_key(&request);
        let manual_probe = {
            let now = Instant::now();
            let mut cooldowns = self.cooldowns.lock().unwrap_or_else(|e| e.into_inner());
            cooldowns.retain(|_, cooldown| !cooldown.remaining(now).is_zero());
            if let Some(cooldown) = cooldowns.get_mut(&key) {
                cooldown.last_used = now;
                if !MANUAL_REFRESH.get()
                    || now.duration_since(cooldown.last_probe.unwrap_or(cooldown.received))
                        <= MANUAL_RETRY_AFTER
                {
                    crate::diagnose::log(format!(
                        "http {target}: not sent, cooldown {}s left",
                        cooldown.remaining(now).as_secs()
                    ));
                    return Err(ureq::Error::StatusCode(cooldown.status));
                }
                // Admit one manual probe per minute without extending the
                // automatic cooldown, even if the request fails to connect.
                cooldown.last_probe = Some(now);
                true
            } else {
                false
            }
        };

        let response = send(request);
        log_http(&target, "", &response);
        let response = response?;
        let status = response.status();
        // A probe tests recovery from an existing cooldown; a failed probe
        // must keep its original deadline, even with a new Retry-After header.
        if !manual_probe && (status.as_u16() == 429 || status.is_server_error()) {
            if let Some(delay) = response
                .headers()
                .get(ureq::http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, SystemTime::now()))
                .filter(|delay| !delay.is_zero())
                // Some rate-limit responses omit Retry-After. Still throttle
                // manual refreshes, which can bypass the poll retry timer.
                .or_else(|| (status.as_u16() == 429).then_some(DEFAULT_RATE_LIMIT_DELAY))
            {
                let received = Instant::now();
                let mut cooldowns = self.cooldowns.lock().unwrap_or_else(|e| e.into_inner());
                let cooldown = cooldowns.entry(key).or_insert(Cooldown {
                    received,
                    last_used: received,
                    last_probe: None,
                    delay,
                    status: status.as_u16(),
                });
                // Concurrent responses must not shorten an existing cooldown.
                if delay > cooldown.remaining(received) {
                    *cooldown = Cooldown {
                        received,
                        last_used: received,
                        last_probe: None,
                        delay,
                        status: status.as_u16(),
                    };
                }
            }
        } else if status.is_success() {
            self.cooldowns
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        }
        Ok(response)
    }

    fn retry_delay_ms(&self, fallback_ms: u32, poll_started: Instant, now: Instant) -> u32 {
        let cooldowns = self.cooldowns.lock().unwrap_or_else(|e| e.into_inner());
        let remaining = cooldowns
            .values()
            // Old or disabled accounts must not delay this poll's retries.
            .filter(|cooldown| cooldown.last_used >= poll_started)
            .map(|cooldown| cooldown.remaining(now))
            .max()
            .unwrap_or_default();
        // Round up to avoid retrying just before the capped deadline, and keep
        // the timer within Win32's USER_TIMER_MAXIMUM (including the fallback).
        let millis = remaining.as_nanos().div_ceil(1_000_000);
        millis.max(fallback_ms as u128).min(0x7fff_ffff) as u32
    }
}

pub(super) fn middleware(
    request: Request<SendBody>,
    next: MiddlewareNext,
) -> Result<HttpResponse, ureq::Error> {
    shared().handle(request, |request| next.handle(request))
}

pub fn retry_delay_ms(fallback_ms: u32, poll_started: Instant) -> u32 {
    shared().retry_delay_ms(fallback_ms, poll_started, Instant::now())
}

fn log_http(target: &str, note: &str, response: &Result<HttpResponse, ureq::Error>) {
    let outcome = match response {
        Ok(response) => response.status().as_u16().to_string(),
        Err(ureq::Error::StatusCode(status)) => status.to_string(),
        Err(_) => "failed".to_string(),
    };
    let note = if note.is_empty() {
        String::new()
    } else {
        format!(" ({note})")
    };
    crate::diagnose::log(format!("http {target}{note} -> {outcome}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::http::Response;

    fn request(account: &str) -> Request<SendBody<'static>> {
        Request::builder()
            .uri("https://example.com/usage")
            .header("Authorization", account)
            .body(SendBody::none())
            .unwrap()
    }

    fn response(status: u16, retry_after: Option<&str>) -> HttpResponse {
        let mut response = Response::builder().status(status);
        if let Some(value) = retry_after {
            response = response.header("Retry-After", value);
        }
        response.body(ureq::Body::builder().data(b"{}")).unwrap()
    }

    #[test]
    fn parses_seconds_dates_and_invalid_values() {
        let now = httpdate::parse_http_date("Wed, 23 Sep 2026 00:00:00 GMT").unwrap();
        for value in ["120", " 120 ", "Wed, 23 Sep 2026 00:02:00 GMT"] {
            assert_eq!(
                parse_retry_after(value, now),
                Some(Duration::from_secs(120))
            );
        }
        for value in ["0", "Tue, 22 Sep 2026 00:00:00 GMT"] {
            assert_eq!(parse_retry_after(value, now), Some(Duration::ZERO));
        }
        for value in ["", " ", "-1", "+1", "1.5", "tomorrow"] {
            assert_eq!(parse_retry_after(value, now), None);
        }
    }

    #[test]
    fn caps_seconds_and_http_dates_at_fifteen_minutes() {
        let now = httpdate::parse_http_date("Wed, 23 Sep 2026 00:00:00 GMT").unwrap();
        for (seconds, date) in [
            (899, "Wed, 23 Sep 2026 00:14:59 GMT"),
            (900, "Wed, 23 Sep 2026 00:15:00 GMT"),
            (901, "Wed, 23 Sep 2026 00:15:01 GMT"),
        ] {
            let expected = Some(Duration::from_secs(seconds.min(900)));
            assert_eq!(parse_retry_after(&seconds.to_string(), now), expected);
            assert_eq!(parse_retry_after(date, now), expected);
        }
        for value in [
            "18446744073709551615",
            "999999999999999999999999999999",
            "Fri, 31 Dec 9999 23:59:59 GMT",
        ] {
            assert_eq!(parse_retry_after(value, now), Some(MAX_RETRY_AFTER));
        }
    }

    #[test]
    fn oversized_headers_store_bounded_cooldowns_and_requests_resume_after_expiry() {
        let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(48 * 60 * 60));
        for status in [429, 503] {
            for header in [
                "18446744073709551615",
                "999999999999999999999999999999",
                future.as_str(),
            ] {
                let state = RetryAfter::default();
                let started = Instant::now();
                state
                    .handle(request("first"), |_| Ok(response(status, Some(header))))
                    .unwrap();
                let key = request_key(&request("first"));
                let cooldown = state.cooldowns.lock().unwrap()[&key];
                assert_eq!(cooldown.delay, Duration::from_secs(900));
                assert_eq!(
                    state.retry_delay_ms(30_000, started, cooldown.received),
                    900_000
                );
                assert!(matches!(
                    state.handle(request("first"), |_| panic!("sent during cooldown")),
                    Err(ureq::Error::StatusCode(code)) if code == status
                ));

                // Advance the cooldown's age without sleeping or restarting.
                {
                    let mut cooldowns = state.cooldowns.lock().unwrap();
                    cooldowns.get_mut(&key).unwrap().received =
                        Instant::now() - Duration::from_secs(900);
                }
                let mut sent = false;
                state
                    .handle(request("first"), |_| {
                        sent = true;
                        Ok(response(200, None))
                    })
                    .unwrap();
                assert!(sent, "request must resume after the capped delay");
                assert!(state.cooldowns.lock().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn cooldown_blocks_same_account_but_not_other_accounts() {
        for status in [429, 503] {
            let state = RetryAfter::default();
            let started = Instant::now();
            let result = state.handle(request("first"), |_| Ok(response(status, Some("120"))));
            assert!(
                matches!(result.and_then(super::super::check_http_status), Err(ureq::Error::StatusCode(code)) if code == status)
            );
            assert!(
                matches!(state.handle(request("first"), |_| panic!("sent during cooldown")), Err(ureq::Error::StatusCode(code)) if code == status)
            );
            assert!(state
                .handle(request("second"), |_| Ok(response(200, None)))
                .is_ok());
            assert!(state.retry_delay_ms(30_000, started, Instant::now()) > 119_000);
        }
    }

    #[test]
    fn manual_refresh_waits_a_minute_then_probes_and_clears_on_success() {
        let state = RetryAfter::default();
        let key = request_key(&request("first"));
        state
            .handle(request("first"), |_| Ok(response(429, Some("900"))))
            .unwrap();
        state
            .cooldowns
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .received = Instant::now() - Duration::from_secs(59);
        assert!(matches!(
            with_manual_refresh(true, || state.handle(request("first"), |_| {
                panic!("manual refresh sent within the first minute")
            })),
            Err(ureq::Error::StatusCode(429))
        ));

        state
            .cooldowns
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .received = Instant::now() - Duration::from_secs(61);
        assert!(matches!(
            state.handle(request("first"), |_| panic!(
                "automatic poll bypassed cooldown"
            )),
            Err(ureq::Error::StatusCode(429))
        ));
        let mut sent = 0;
        with_manual_refresh(true, || {
            state
                .handle(request("first"), |_| {
                    sent += 1;
                    Ok(response(200, None))
                })
                .unwrap();
        });
        assert_eq!(sent, 1);
        assert!(!state.cooldowns.lock().unwrap().contains_key(&key));
        assert!(state
            .handle(request("first"), |_| Ok(response(200, None)))
            .is_ok());
    }

    #[test]
    fn failed_manual_probe_keeps_cooldown_and_cannot_be_repeated_immediately() {
        let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(7200));
        for status in [429, 503] {
            for header in [
                None,
                Some("900"),
                Some("999999999999999999999999999999"),
                Some(future.as_str()),
            ] {
                let state = RetryAfter::default();
                let key = request_key(&request("first"));
                state
                    .handle(request("first"), |_| Ok(response(status, Some("900"))))
                    .unwrap();
                state
                    .cooldowns
                    .lock()
                    .unwrap()
                    .get_mut(&key)
                    .unwrap()
                    .received = Instant::now() - Duration::from_secs(61);
                let original = state.cooldowns.lock().unwrap()[&key];
                with_manual_refresh(true, || {
                    let result = state.handle(request("first"), |_| Ok(response(status, header)));
                    assert_eq!(result.unwrap().status().as_u16(), status);
                    let after_probe = state.cooldowns.lock().unwrap()[&key];
                    assert_eq!(
                        after_probe.received, original.received,
                        "probe extended the deadline"
                    );
                    assert_eq!(after_probe.delay, original.delay);
                    assert!(after_probe.last_probe.is_some());
                    assert!(matches!(
                        state.handle(request("first"), |_| panic!("repeated manual probe")),
                        Err(ureq::Error::StatusCode(code)) if code == status
                    ));
                });
                assert!(matches!(
                    state.handle(request("first"), |_| panic!("automatic poll bypassed cooldown")),
                    Err(ureq::Error::StatusCode(code)) if code == status
                ));
                assert!(state.cooldowns.lock().unwrap().contains_key(&key));
                // After a minute, another manual probe is allowed, while the
                // original automatic deadline still permits recovery on expiry.
                state
                    .cooldowns
                    .lock()
                    .unwrap()
                    .get_mut(&key)
                    .unwrap()
                    .last_probe = Some(Instant::now() - Duration::from_secs(61));
                with_manual_refresh(true, || {
                    state
                        .handle(request("first"), |_| Ok(response(status, header)))
                        .unwrap();
                });
                assert_eq!(
                    state.cooldowns.lock().unwrap()[&key].received,
                    original.received
                );
                state
                    .cooldowns
                    .lock()
                    .unwrap()
                    .get_mut(&key)
                    .unwrap()
                    .received = Instant::now() - MAX_RETRY_AFTER;
                assert!(state
                    .handle(request("first"), |_| Ok(response(200, None)))
                    .is_ok());
                assert!(state.cooldowns.lock().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn disconnected_manual_probe_preserves_deadline_and_probe_throttle() {
        let state = RetryAfter::default();
        let key = request_key(&request("first"));
        state
            .handle(request("first"), |_| Ok(response(429, Some("900"))))
            .unwrap();
        state
            .cooldowns
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .received = Instant::now() - Duration::from_secs(61);
        let original = state.cooldowns.lock().unwrap()[&key];
        with_manual_refresh(true, || {
            assert!(matches!(
                state.handle(request("first"), |_| Err(ureq::Error::Io(
                    std::io::Error::new(std::io::ErrorKind::ConnectionReset, "disconnected probe")
                ))),
                Err(ureq::Error::Io(_))
            ));
            let after_probe = state.cooldowns.lock().unwrap()[&key];
            assert_eq!(after_probe.received, original.received);
            assert_eq!(after_probe.delay, original.delay);
            assert!(after_probe.last_probe.is_some());
            assert!(matches!(
                state.handle(request("first"), |_| panic!("repeated disconnected probe")),
                Err(ureq::Error::StatusCode(429))
            ));
        });
    }

    #[test]
    fn bypass_neither_records_nor_obeys_cooldowns() {
        let state = RetryAfter::default();
        let started = Instant::now();
        let probe = || {
            let mut request = request("first");
            request.extensions_mut().insert(BypassCooldown);
            request
        };
        for _ in 0..2 {
            let response = state
                .handle(probe(), |_| Ok(response(429, Some("7200"))))
                .unwrap();
            assert_eq!(response.status(), 429);
        }
        assert!(state.cooldowns.lock().unwrap().is_empty());
        assert_eq!(
            state.retry_delay_ms(30_000, started, Instant::now()),
            30_000
        );

        // A probe also passes through an existing cooldown for the same key,
        // without clearing it for ordinary requests.
        state
            .handle(request("first"), |_| Ok(response(429, Some("7200"))))
            .unwrap();
        assert!(state
            .handle(probe(), |_| Ok(response(429, Some("7200"))))
            .is_ok());
        assert!(matches!(
            state.handle(request("first"), |_| panic!("sent during cooldown")),
            Err(ureq::Error::StatusCode(429))
        ));
    }

    #[test]
    fn rate_limits_without_usable_headers_throttle_automatic_and_manual_refreshes() {
        for header in [
            None,
            Some("invalid"),
            Some("0"),
            Some("Sun, 06 Nov 1994 08:49:37 GMT"),
        ] {
            let state = RetryAfter::default();
            let started = Instant::now();
            state
                .handle(request("first"), |_| Ok(response(429, header)))
                .unwrap();
            let key = request_key(&request("first"));
            let cooldown = state.cooldowns.lock().unwrap()[&key];
            assert_eq!(cooldown.delay, DEFAULT_RATE_LIMIT_DELAY);
            assert_eq!(
                state.retry_delay_ms(30_000, started, cooldown.received),
                60_000
            );
            for manual in [false, true] {
                assert!(matches!(
                    with_manual_refresh(manual, || state.handle(request("first"), |_| {
                        panic!("sent during fallback cooldown")
                    })),
                    Err(ureq::Error::StatusCode(429))
                ));
            }
            assert!(state
                .handle(request("second"), |_| Ok(response(200, None)))
                .is_ok());
            state
                .cooldowns
                .lock()
                .unwrap()
                .get_mut(&key)
                .unwrap()
                .received = Instant::now() - DEFAULT_RATE_LIMIT_DELAY;
            assert!(state
                .handle(request("first"), |_| Ok(response(200, None)))
                .is_ok());
            assert!(state.cooldowns.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn server_errors_without_usable_headers_keep_poll_timer_fallback() {
        for header in [None, Some("invalid"), Some("0")] {
            let state = RetryAfter::default();
            let started = Instant::now();
            state
                .handle(request("first"), |_| Ok(response(503, header)))
                .unwrap();
            assert_eq!(
                state.retry_delay_ms(30_000, started, Instant::now()),
                30_000
            );
            assert!(state.cooldowns.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn expiry_restores_requests_and_timer_handles_large_delays() {
        let state = RetryAfter::default();
        let started = Instant::now();
        let now = Instant::now();
        let key = request_key(&request("first"));
        state.cooldowns.lock().unwrap().insert(
            key,
            Cooldown {
                received: now,
                last_used: now,
                last_probe: None,
                delay: Duration::from_micros(30_000_001),
                status: 429,
            },
        );
        assert_eq!(state.retry_delay_ms(30_000, started, now), 30_001);
        assert_eq!(state.retry_delay_ms(60_000, started, now), 60_000);
        assert_eq!(
            state.retry_delay_ms(30_000, started, now + Duration::from_secs(31)),
            30_000
        );
        state.cooldowns.lock().unwrap().get_mut(&key).unwrap().delay =
            Duration::from_secs(u64::MAX);
        assert_eq!(state.retry_delay_ms(30_000, started, now), 0x7fff_ffff);
        assert_eq!(
            state.retry_delay_ms(
                30_000,
                now + Duration::from_secs(1),
                now + Duration::from_secs(1)
            ),
            30_000,
            "an account not used in this poll must not delay it"
        );
        state.cooldowns.lock().unwrap().get_mut(&key).unwrap().delay = Duration::ZERO;
        assert!(state
            .handle(request("first"), |_| Ok(response(200, None)))
            .is_ok());
    }

    #[test]
    fn production_agent_preserves_header_before_status_conversion() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "client never connected");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
        });
        let agent = super::super::build_agent().unwrap();
        let fetch = || {
            agent
                .get(format!("http://{address}/usage"))
                .config()
                .proxy(None)
                .timeout_global(Some(Duration::from_secs(5)))
                .build()
                .call()
        };
        let response = fetch().unwrap();
        server.join().unwrap();
        assert_eq!(response.headers()["Retry-After"], "120");
        assert!(matches!(
            super::super::check_http_status(response),
            Err(ureq::Error::StatusCode(429))
        ));
        // The listener is closed: an actual retry would fail to connect.
        assert!(matches!(fetch(), Err(ureq::Error::StatusCode(429))));
    }
}
