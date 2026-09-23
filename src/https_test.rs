use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// Exercise the production connector without credentials or an external server.
pub(crate) fn assert_tls_handshake(agent: ureq::Agent) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "HTTPS client never connected");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unable to accept HTTPS client: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = [0; 3];
        stream.read_exact(&mut header).unwrap();
        // TLS handshake record (22), with the TLS record version's major byte (3).
        // A closed port would fail before ureq ever checks its TLS provider.
        assert_eq!(&header[..2], &[22, 3], "expected a TLS ClientHello");
    });

    let request = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agent
            .get(format!("https://{address}"))
            .config()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .call()
    }));
    let handshake = server.join();
    assert!(request.is_ok(), "the configured HTTPS provider panicked");
    handshake.expect("HTTPS client must send a TLS handshake");
    // The peer closes without a certificate: this must be an ordinary error.
    assert!(request.unwrap().is_err());
}
