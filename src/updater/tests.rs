use super::*;

#[test]
fn parse_version_reads_numeric_components_and_defaults_missing_ones() {
    for (input, expected) in [
        ("2.13.44", (2, 13, 44)),
        ("002.013.044", (2, 13, 44)),
        ("", (0, 0, 0)),
        ("2", (2, 0, 0)),
        ("2.13", (2, 13, 0)),
        ("2..44", (2, 0, 44)),
        (".13.", (0, 13, 0)),
        (
            "4294967295.4294967295.4294967295",
            (u32::MAX, u32::MAX, u32::MAX),
        ),
    ] {
        assert_eq!(parse_version(input), expected, "version: {input:?}");
    }
}

#[test]
fn parse_version_defaults_invalid_or_overflowing_components_to_zero() {
    for (input, expected) in [
        ("invalid", (0, 0, 0)),
        ("bad.13.44", (0, 13, 44)),
        ("2.bad.44", (2, 0, 44)),
        ("2.13.bad", (2, 13, 0)),
        ("4294967296.13.44", (0, 13, 44)),
        ("2.4294967296.44", (2, 0, 44)),
        ("2.13.4294967296", (2, 13, 0)),
    ] {
        assert_eq!(parse_version(input), expected, "version: {input:?}");
    }
}

#[test]
fn parse_version_ignores_prerelease_suffixes_and_extra_components() {
    // The updater compares numeric triples, without SemVer prerelease ordering.
    for input in ["2.13.44-beta.1", "2.13.44-", "2.13.44.99"] {
        assert_eq!(parse_version(input), (2, 13, 44), "version: {input:?}");
    }
}

#[test]
fn is_version_newer_compares_components_numerically_in_priority_order() {
    for (newer, older) in [
        ("3.0.0", "2.99.99"),
        ("2.14.0", "2.13.99"),
        ("2.13.45", "2.13.44"),
        ("10.0.0", "9.0.0"),
        ("2.10.0", "2.9.0"),
        ("2.13.10", "2.13.9"),
        ("2.13.45-beta.1", "2.13.44"),
    ] {
        assert!(is_version_newer(newer, older), "{newer} > {older}");
        assert!(!is_version_newer(older, newer), "{older} < {newer}");
    }
}

#[test]
fn is_version_newer_rejects_equal_numeric_versions() {
    for (left, right) in [
        ("2.13.44", "2.13.44"),
        ("2", "2.0.0"),
        ("2.13", "2.13.0"),
        ("2.13.44-beta.1", "2.13.44"),
        ("2.13.44.99", "2.13.44"),
        ("invalid", "0.0.0"),
        ("", "0.0.0"),
    ] {
        assert!(!is_version_newer(left, right), "{left:?} == {right:?}");
        assert!(!is_version_newer(right, left), "{right:?} == {left:?}");
    }
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
