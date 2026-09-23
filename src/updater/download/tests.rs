use super::*;

const ABC_DIGEST: &str = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn integrity() -> AssetIntegrity {
    AssetIntegrity::new(3, Some(ABC_DIGEST)).unwrap()
}

#[test]
fn validates_digest_format_and_size_limits() {
    for digest in [
        "sha256:00".to_owned(),
        format!("sha256:{}", "g".repeat(64)),
        format!("sha256:{}", "é".repeat(32)),
        ABC_DIGEST.replace("sha256:", "sha512:"),
    ] {
        assert!(AssetIntegrity::new(3, Some(&digest)).is_err());
    }
    assert!(AssetIntegrity::new(3, None).is_err());
    assert!(AssetIntegrity::new(0, Some(ABC_DIGEST)).is_err());
    assert!(AssetIntegrity::new(MAX_DOWNLOAD_BYTES + 1, Some(ABC_DIGEST)).is_err());
    assert!(AssetIntegrity::new(u64::MAX, Some(ABC_DIGEST)).is_err());
    assert!(AssetIntegrity::new(MAX_DOWNLOAD_BYTES, Some(ABC_DIGEST)).is_ok());
}

#[test]
fn accepts_uppercase_hex_and_roundtrips_helper_arguments() {
    let expected = AssetIntegrity::new(
        3,
        Some(&format!("sha256:{}", ABC_DIGEST[7..].to_uppercase())),
    )
    .unwrap();
    assert_eq!(expected.size_arg(), "3");
    assert_eq!(expected.digest_arg(), ABC_DIGEST);
    copy_verified(&b"abc"[..], io::sink(), &expected).unwrap();
}

#[test]
fn verifies_a_known_sha256_vector() {
    let mut output = Vec::new();
    copy_verified(&b"abc"[..], &mut output, &integrity()).unwrap();
    assert_eq!(output, b"abc");
}

#[test]
fn rejects_corruption_and_truncated_or_empty_streams() {
    assert!(copy_verified(&b"abd"[..], io::sink(), &integrity())
        .unwrap_err()
        .contains("SHA-256"));
    for bytes in [&b"ab"[..], &b""[..]] {
        assert!(copy_verified(bytes, io::sink(), &integrity())
            .unwrap_err()
            .contains("size"));
    }
}

#[test]
fn stops_an_unbounded_stream_after_expected_size_plus_one() {
    struct CountingReader {
        read: usize,
    }
    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            buffer.fill(b'a');
            self.read += buffer.len();
            Ok(buffer.len())
        }
    }
    let mut reader = CountingReader { read: 0 };
    let mut written = Vec::new();
    assert!(copy_verified(&mut reader, &mut written, &integrity())
        .unwrap_err()
        .contains("exceeds"));
    assert_eq!(reader.read, 4);
    assert!(written.len() <= 3);
}

#[test]
fn handles_short_reads_and_interrupted_reads() {
    struct ShortReader {
        calls: usize,
        bytes: &'static [u8],
    }
    impl Read for ShortReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = self.bytes.read(&mut buffer[..1])?;
            Ok(count)
        }
    }
    copy_verified(
        ShortReader {
            calls: 0,
            bytes: b"abc",
        },
        io::sink(),
        &integrity(),
    )
    .unwrap();
}

#[test]
fn propagates_write_errors() {
    struct FailedWriter;
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert!(copy_verified(&b"abc"[..], FailedWriter, &integrity())
        .unwrap_err()
        .contains("disk full"));
}

#[test]
fn promotes_only_verified_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("update.part");
    let final_path = dir.path().join("update.exe");
    stage_verified_download(&b"abc"[..], &integrity(), &partial, &final_path).unwrap();
    assert!(!partial.exists());
    assert_eq!(std::fs::read(final_path).unwrap(), b"abc");
}

#[test]
fn removes_partial_files_and_preserves_existing_target_on_validation_failure() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("update.part");
    let final_path = dir.path().join("update.exe");
    std::fs::write(&final_path, b"existing").unwrap();
    for bytes in [&b"abd"[..], &b"ab"[..], &b"abcd"[..]] {
        assert!(stage_verified_download(bytes, &integrity(), &partial, &final_path).is_err());
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&final_path).unwrap(), b"existing");
    }
}

#[test]
fn cleans_up_after_read_and_rename_failures() {
    struct FailedReader;
    impl Read for FailedReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("connection lost"))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("update.part");
    let final_path = dir.path().join("missing").join("update.exe");
    let error =
        stage_verified_download(FailedReader, &integrity(), &partial, &final_path).unwrap_err();
    assert!(error.contains("connection lost"));
    assert!(!partial.exists());
    assert!(stage_verified_download(&b"abc"[..], &integrity(), &partial, &final_path).is_err());
    assert!(!partial.exists());
    assert!(!final_path.exists());
}

#[test]
fn refuses_to_overwrite_an_existing_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("update.part");
    let final_path = dir.path().join("update.exe");
    std::fs::write(&partial, b"existing").unwrap();
    assert!(stage_verified_download(&b"abc"[..], &integrity(), &partial, &final_path).is_err());
    assert_eq!(std::fs::read(partial).unwrap(), b"existing");
    assert!(!final_path.exists());
}

#[test]
fn helper_rechecks_integrity_and_locks_the_source_until_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.exe");
    let target = dir.path().join("target.exe");
    std::fs::write(&source, b"abd").unwrap();
    assert!(open_verified_source(&source, &integrity()).is_err());
    std::fs::write(&source, b"abc").unwrap();
    let mut verified = open_verified_source(&source, &integrity()).unwrap();
    assert!(OpenOptions::new().write(true).open(&source).is_err());
    assert!(std::fs::remove_file(&source).is_err());
    std::fs::copy(&source, &target).unwrap();
    let mut bytes = Vec::new();
    verified.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"abc");
    assert_eq!(std::fs::read(target).unwrap(), b"abc");
    drop(verified);
    std::fs::remove_file(source).unwrap();
}

#[test]
fn helper_refuses_a_source_already_open_for_writing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.exe");
    std::fs::write(&source, b"abc").unwrap();
    let _writer = OpenOptions::new().write(true).open(&source).unwrap();
    assert!(open_verified_source(&source, &integrity()).is_err());
}
