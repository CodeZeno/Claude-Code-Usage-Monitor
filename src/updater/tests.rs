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
            "winget upgrade --id CodeZeno.ClaudeCodeUsageMonitor --exact; ",
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
    let stdout = concat!(
        "Trouvé Claude Code Usage Monitor [CodeZeno.ClaudeCodeUsageMonitor]
",
        "Version
-------
2.15.14
2.15.0
  2.14.55  
1.3
",
    );
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
}

fn v(version: &str) -> Version {
    Version::parse(version).unwrap()
}

fn winget_result(current: &str, released: &str, listed: &[&str]) -> String {
    let listed = listed.iter().map(|version| v(version)).collect::<Vec<_>>();
    match winget_update_result(&v(current), &v(released), &listed) {
        UpdateCheckResult::UpToDate => "up to date".into(),
        UpdateCheckResult::Pending(released) => format!("pending {released}"),
        UpdateCheckResult::Available(AvailableUpdate::Winget {
            version,
            unlisted_release,
        }) => format!("winget {version} unlisted {unlisted_release:?}"),
        UpdateCheckResult::Available(AvailableUpdate::Release(_)) => "release".into(),
    }
}

#[test]
fn winget_offers_the_released_version_once_listed() {
    assert_eq!(
        winget_result("2.15.14", "2.15.19", &["2.15.0", "2.15.19", "2.15.14"]),
        "winget 2.15.19 unlisted None"
    );
}

#[test]
fn winget_offers_the_newest_listed_version_while_the_release_is_unlisted() {
    assert_eq!(
        winget_result("2.15.14", "2.15.19", &["2.15.14", "2.15.18", "2.15.17"]),
        "winget 2.15.18 unlisted Some(\"2.15.19\")"
    );
}

#[test]
fn winget_is_pending_when_nothing_newer_is_listed() {
    assert_eq!(
        winget_result("2.15.14", "2.15.19", &["2.15.0", "2.15.14"]),
        "pending 2.15.19"
    );
    assert_eq!(winget_result("2.15.14", "2.15.19", &[]), "pending 2.15.19");
    assert_eq!(
        winget_result("2.15.14", "2.15.19", &["2.15.18-beta1"]),
        "pending 2.15.19"
    );
}

#[test]
#[ignore = "runs winget.exe against the live WinGet source"]
fn winget_lists_published_versions_from_the_live_source() {
    let versions = winget_listed_versions().unwrap();
    assert!(versions.contains(&v("2.15.0")), "{versions:?}");
}
