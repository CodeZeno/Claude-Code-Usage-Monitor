use super::*;

#[test]
fn configured_https_transport_does_not_panic() {
    crate::https_test::assert_tls_handshake(super::build_agent().expect("HTTP agent should build"));
}

const ABC_DIGEST: &str = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn helper_args() -> Vec<String> {
    [
        "helper.exe",
        "--apply-update",
        "target.exe",
        "source.exe",
        "1234",
        "3",
        ABC_DIGEST,
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn helper_requires_integrity_metadata() {
    let args = helper_args();
    let (target, source, pid, integrity) = parse_apply_update_args(&args).unwrap();
    assert_eq!(target, PathBuf::from("target.exe"));
    assert_eq!(source, PathBuf::from("source.exe"));
    assert_eq!(pid, 1234);
    assert_eq!(integrity.digest_arg(), ABC_DIGEST);
    assert_eq!(integrity.size_arg(), "3");
    for len in 0..args.len() {
        assert!(parse_apply_update_args(&args[..len]).is_err());
    }
    let mut extra = args;
    extra.push("unexpected".into());
    assert!(parse_apply_update_args(&extra).is_err());
}

#[test]
fn helper_rejects_invalid_process_size_and_hash_arguments() {
    for (index, value) in [
        (4, "0"),
        (4, "bad"),
        (5, "0"),
        (5, "-1"),
        (5, "104857601"),
        (6, "sha256:00"),
    ] {
        let mut args = helper_args();
        args[index] = value.into();
        assert!(parse_apply_update_args(&args).is_err());
    }
}

#[test]
fn corrupt_update_never_replaces_the_installed_binary() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("download.exe");
    let target = dir.path().join("app.exe");
    std::fs::write(&source, b"abd").unwrap();
    std::fs::write(&target, b"original").unwrap();
    let integrity = AssetIntegrity::new(3, Some(ABC_DIGEST)).unwrap();
    let error = apply_update(target.clone(), source, 0, &integrity).unwrap_err();
    assert!(error.contains("SHA-256"));
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert!(!backup_path_for(&target).exists());
}

#[test]
fn verified_source_can_replace_the_installed_binary() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("download.exe");
    let target = dir.path().join("app.exe");
    std::fs::write(&source, b"abc").unwrap();
    std::fs::write(&target, b"original").unwrap();
    let integrity = AssetIntegrity::new(3, Some(ABC_DIGEST)).unwrap();
    let _guard = open_verified_source(&source, &integrity).unwrap();
    replace_target_binary(&target, &source).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"abc");
    assert!(!backup_path_for(&target).exists());
}

#[test]
fn updater_agent_rejects_plain_http() {
    let error = build_agent()
        .unwrap()
        .get("http://127.0.0.1:1/update.exe")
        .call()
        .unwrap_err();
    assert!(matches!(error, ureq::Error::RequireHttpsOnly(_)));
}

#[test]
fn is_winget_install_path_accepts_configured_roots_and_descendants() {
    // Read the configured roots without mutating process-wide environment variables.
    let roots = winget_install_roots();
    assert!(!roots.is_empty());
    for root in roots {
        assert!(is_winget_install_path(&root), "root: {}", root.display());
        let executable = root
            .join("CodeZeno.ClaudeCodeUsageMonitor_source")
            .join("app.exe");
        let path = executable.to_string_lossy();
        for variant in [
            path.to_string(),
            path.to_ascii_uppercase(),
            path.replace('\\', "/"),
            format!("{path}\\"),
            format!(r"\\?\{path}"),
        ] {
            assert!(
                is_winget_install_path(Path::new(&variant)),
                "path: {variant}"
            );
        }
    }
}

#[test]
fn is_winget_install_path_rejects_siblings_with_the_same_prefix() {
    for root in winget_install_roots() {
        for suffix in ["-old", "Backup", ".old"] {
            let sibling = format!("{}{suffix}\\app.exe", root.display());
            assert!(
                !is_winget_install_path(Path::new(&sibling)),
                "path: {sibling}"
            );
        }
        assert!(!is_winget_install_path(root.parent().unwrap()));
    }
}

#[test]
fn is_winget_install_path_rejects_portable_and_empty_paths() {
    for path in [
        "",
        "app.exe",
        r"C:\Portable\app.exe",
        r"C:\Downloads\WinGet\Packages\app.exe",
    ] {
        assert!(!is_winget_install_path(Path::new(path)), "path: {path:?}");
    }
}

#[test]
fn winget_upgrade_command_waits_upgrades_and_restarts_only_on_success() {
    let command = winget_upgrade_command(1234, r"C:\Usage Monitor\app.exe", r"C:\Usage Monitor");
    assert_eq!(
        command,
        concat!(
            "$ErrorActionPreference = 'Stop'; ",
            "$pidToWait = 1234; ",
            "$target = 'C:\\Usage Monitor\\app.exe'; ",
            "$workingDir = 'C:\\Usage Monitor'; ",
            "try { Wait-Process -Id $pidToWait -Timeout 30 -ErrorAction Stop } catch { }; ",
            "winget upgrade --id CodeZeno.ClaudeCodeUsageMonitor --exact --source winget; ",
            "$exitCode = $LASTEXITCODE; ",
            "if ($exitCode -eq 0) { ",
            "Start-Sleep -Seconds 2; ",
            "Start-Process -FilePath $target -WorkingDirectory $workingDir; ",
            "exit 0 }; ",
            "Write-Host ''; ",
            "Write-Host 'WinGet update failed with exit code' $exitCode; ",
            "Read-Host 'Press Enter to close'; ",
            "exit $exitCode",
        )
    );
}

#[test]
fn winget_upgrade_command_quotes_each_path_as_a_powershell_literal() {
    for (input, quoted) in [
        ("", "''"),
        (r"C:\O'Brien\app.exe", r"'C:\O''Brien\app.exe'"),
        ("a''b", "'a''''b'"),
        (
            r#"C:\$HOME\$(whoami);`echo "hi"\app.exe"#,
            r#"'C:\$HOME\$(whoami);`echo "hi"\app.exe'"#,
        ),
        ("'; Write-Host 'injected", "'''; Write-Host ''injected'"),
    ] {
        // Check each parameter independently so accidentally reusing either one fails.
        let target_command = winget_upgrade_command(0, input, r"C:\work");
        assert!(
            target_command.contains(&format!(
                "$pidToWait = 0; $target = {quoted}; $workingDir = 'C:\\work'; "
            )),
            "target: {input:?}: {target_command}"
        );
        let working_dir_command = winget_upgrade_command(u32::MAX, r"C:\app.exe", input);
        assert!(
            working_dir_command.contains(&format!(
                "$pidToWait = 4294967295; $target = 'C:\\app.exe'; $workingDir = {quoted}; "
            )),
            "working directory: {input:?}: {working_dir_command}"
        );
    }
}

#[test]
fn winget_show_args_list_versions_non_interactively() {
    assert_eq!(
        WINGET_SHOW_VERSIONS_ARGS.join(" "),
        "show --id CodeZeno.ClaudeCodeUsageMonitor --exact --versions --source winget --accept-source-agreements --disable-interactivity"
    );
}

#[test]
fn winget_versions_outcome_parses_versions_under_localized_headers() {
    let stdout = "Trouvé Claude Code Usage Monitor [CodeZeno.ClaudeCodeUsageMonitor]\r\n\
        Version\r\n-------\r\n2.15.14\r\n2.15.0\r\n  2.14.55  \r\n1.3\r\n";
    assert_eq!(
        winget_versions_outcome(Some(0), stdout),
        Ok(vec![v("2.15.14"), v("2.15.0"), v("2.14.55")])
    );
    assert_eq!(
        winget_versions_outcome(Some(WINGET_NO_APPLICATIONS_FOUND as i32), ""),
        Ok(Vec::new())
    );
    assert_eq!(
        winget_versions_outcome(Some(0x8A15_0001_u32 as i32), ""),
        Err("WinGet could not list available versions (exit code 0x8A150001).".into())
    );
    assert!(winget_versions_outcome(Some(1), "2.15.14").is_err());
    assert!(winget_versions_outcome(None, "2.15.14").is_err());
    assert!(winget_versions_outcome(Some(0), "").is_err());
    assert!(winget_versions_outcome(Some(0), "Version\n-------\ninvalid").is_err());
}

fn v(version: &str) -> Version {
    Version::parse(version).unwrap()
}

fn winget_result(current: &str, listed: &[&str]) -> String {
    let listed = listed.iter().map(|version| v(version)).collect::<Vec<_>>();
    match winget_update_result(&v(current), &listed) {
        UpdateCheckResult::UpToDate => "up to date".into(),
        UpdateCheckResult::Available(AvailableUpdate::Winget { version }) => {
            format!("winget {version}")
        }
        UpdateCheckResult::Available(AvailableUpdate::Release(_)) => "release".into(),
    }
}

#[test]
fn winget_offers_the_newest_stable_listed_version() {
    assert_eq!(
        winget_result("2.15.14", &["2.15.0", "2.15.19", "2.15.14", "2.16.0-beta1"]),
        "winget 2.15.19"
    );
}

#[test]
fn winget_compares_semver_precedence_instead_of_text_or_build_metadata() {
    assert_eq!(
        winget_result("2.9.0", &["2.9.0", "2.15.18", "2.15.17"]),
        "winget 2.15.18"
    );
    assert_eq!(
        winget_result("2.15.19+local", &["2.15.19+build"]),
        "up to date"
    );
    assert_eq!(
        winget_result("2.15.19-beta1", &["2.15.19"]),
        "winget 2.15.19"
    );
}

#[test]
fn winget_is_up_to_date_when_nothing_newer_is_listed() {
    assert_eq!(
        winget_result("2.15.14", &["2.15.0", "2.15.14"]),
        "up to date"
    );
    assert_eq!(winget_result("2.15.14", &[]), "up to date");
    assert_eq!(winget_result("2.15.19", &["2.15.18"]), "up to date");
    assert_eq!(winget_result("2.15.14", &["2.15.18-beta1"]), "up to date");
}

#[test]
fn portable_checks_only_github_and_returns_its_verified_release() {
    let release = ReleaseDescriptor {
        latest_version: "99.0.0".into(),
        asset_url: "https://example.invalid/update.exe".into(),
        integrity: AssetIntegrity::new(3, Some(ABC_DIGEST)).unwrap(),
    };
    let result = check_channel_updates(
        InstallChannel::Portable,
        || Ok(Some(release)),
        || panic!("Portable checks must not invoke WinGet"),
    )
    .unwrap();
    let UpdateCheckResult::Available(AvailableUpdate::Release(release)) = result else {
        panic!("Expected a portable release");
    };
    assert_eq!(release.latest_version, "99.0.0");
    assert_eq!(release.integrity.digest_arg(), ABC_DIGEST);
    assert!(matches!(
        check_channel_updates(
            InstallChannel::Portable,
            || Ok(None),
            || panic!("Portable checks must not invoke WinGet"),
        )
        .unwrap(),
        UpdateCheckResult::UpToDate
    ));
}

#[test]
fn winget_checks_only_its_source_for_available_and_current_versions() {
    for (listed, available) in [(vec![v("99.0.0")], true), (vec![v("1.0.0")], false)] {
        let result = check_channel_updates(
            InstallChannel::Winget,
            || panic!("WinGet checks must not contact GitHub"),
            || Ok(listed),
        )
        .unwrap();
        match result {
            UpdateCheckResult::Available(AvailableUpdate::Winget { version }) => {
                assert!(available);
                assert_eq!(version, "99.0.0");
            }
            UpdateCheckResult::UpToDate => assert!(!available),
            other => panic!("Unexpected update: {other:?}"),
        }
    }
}

#[test]
fn source_errors_never_fall_back_to_the_other_channel() {
    let portable = check_channel_updates(
        InstallChannel::Portable,
        || Err("GitHub unavailable".into()),
        || panic!("Must not fall back to WinGet"),
    );
    assert_eq!(portable.unwrap_err(), "GitHub unavailable");
    let winget = check_channel_updates(
        InstallChannel::Winget,
        || panic!("Must not fall back to GitHub"),
        || Err("WinGet unavailable".into()),
    );
    assert_eq!(winget.unwrap_err(), "WinGet unavailable");
}

#[test]
#[ignore = "runs winget.exe against the live WinGet source"]
fn winget_lists_published_versions_from_the_live_source() {
    let versions = winget_listed_versions().unwrap();
    assert!(versions.contains(&v("2.15.0")), "{versions:?}");
}
