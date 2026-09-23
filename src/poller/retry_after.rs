use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use ureq::http::Request;
use ureq::middleware::MiddlewareNext;
use ureq::SendBody;

use super::HttpResponse;

/// Header probes need fresh responses even while the account is rate limited.
/// This request extension is local metadata and is never sent to the server.
#[derive(Clone, Copy)]
pub(super) struct BypassCooldown;

#[derive(Clone, Copy)]
struct Cooldown {
    received: Instant,
    last_used: Instant,
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
    cooldowns: Mutex<HashMap<[u8; 32], Cooldown>>,
}

fn shared() -> &'static RetryAfter {
    static RETRY_AFTER: OnceLock<RetryAfter> = OnceLock::new();
    RETRY_AFTER.get_or_init(RetryAfter::default)
}

// Keep credentials out of stored keys. Include headers to distinguish accounts
// using the same URL (including cookie and workspace-based authentication).
fn request_key(request: &Request<SendBody>) -> [u8; 32] {
    let mut digest = Sha256::new();
    for value in [request.method().as_str(), &request.uri().to_string()] {
        digest.update(value.len().to_le_bytes());
        digest.update(value.as_bytes());
    }
    for (name, value) in request.headers() {
        digest.update(name.as_str().len().to_le_bytes());
        digest.update(name.as_str().as_bytes());
        digest.update(value.as_bytes().len().to_le_bytes());
        digest.update(value.as_bytes());
    }
    digest.finalize().into()
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        // Saturate enormous valid delays rather than wrapping into a fast retry.
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let deadline = httpdate::parse_http_date(value).ok()?;
    Some(deadline.duration_since(now).unwrap_or_default())
}

impl RetryAfter {
    fn handle(
        &self,
        request: Request<SendBody>,
        send: impl FnOnce(Request<SendBody>) -> Result<HttpResponse, ureq::Error>,
    ) -> Result<HttpResponse, ureq::Error> {
        if request.extensions().get::<BypassCooldown>().is_some() {
            return send(request);
        }
        let key = request_key(&request);
        {
            let now = Instant::now();
            let mut cooldowns = self.cooldowns.lock().unwrap_or_else(|e| e.into_inner());
            cooldowns.retain(|_, cooldown| !cooldown.remaining(now).is_zero());
            if let Some(cooldown) = cooldowns.get_mut(&key) {
                // Applies to manual refreshes, reset timers, and partial polls,
                // even when another account succeeded in the previous cycle.
                cooldown.last_used = now;
                return Err(ureq::Error::StatusCode(cooldown.status));
            }
        }

        let response = send(request)?;
        let status = response.status();
        if status.as_u16() == 429 || status.is_server_error() {
            if let Some(delay) = response
                .headers()
                .get(ureq::http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, SystemTime::now()))
                .filter(|delay| !delay.is_zero())
            {
                let received = Instant::now();
                let mut cooldowns = self.cooldowns.lock().unwrap_or_else(|e| e.into_inner());
                let cooldown = cooldowns.entry(key).or_insert(Cooldown {
                    received,
                    last_used: received,
                    delay,
                    status: status.as_u16(),
                });
                // Concurrent responses must not shorten an existing cooldown.
                if delay > cooldown.remaining(received) {
                    *cooldown = Cooldown {
                        received,
                        last_used: received,
                        delay,
                        status: status.as_u16(),
                    };
                }
            }
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
        // Round up to avoid retrying just before the server's deadline. Win32
        // caps SetTimer at USER_TIMER_MAXIMUM; the request guard above keeps
        // enforcing longer delays across as many timer ticks as necessary.
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
        assert_eq!(
            parse_retry_after("999999999999999999999999999999", now),
            Some(Duration::from_secs(u64::MAX))
        );
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
        assert_eq!(state.retry_delay_ms(30_000, started, Instant::now()), 30_000);

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
    fn absent_invalid_and_expired_headers_keep_fallback() {
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
            assert_eq!(
                state.retry_delay_ms(30_000, started, Instant::now()),
                30_000
            );
            assert!(state
                .handle(request("first"), |_| Ok(response(200, None)))
                .is_ok());
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
