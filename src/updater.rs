use std::fs::File;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";
const HELPER_EXE_NAME: &str = "updater-helper.exe";
const DOWNLOAD_EXE_NAME: &str = "update-download.exe";
const CREATE_NO_WINDOW: u32 = 0x08000000;
const CREATE_NEW_CONSOLE: u32 = 0x00000010;
// Keep this aligned with the package identifier used in winget-pkgs.
const WINGET_PACKAGE_ID: &str = "CodeZeno.ClaudeCodeUsageMonitor";
// `winget show` exit codes for a missing version or package.
const WINGET_NO_MANIFEST_FOUND: u32 = 0x8A15_0017;
const WINGET_NO_APPLICATIONS_FOUND: u32 = 0x8A15_0014;
const WINGET_SHOW_TIMEOUT: Duration = Duration::from_secs(60);

mod download;
mod release;

use download::{download_release_asset, open_verified_source, AssetIntegrity};
pub use release::ReleaseDescriptor;
use release::{release_descriptor, GitHubRelease};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallChannel {
    Portable,
    Winget,
}

#[derive(Debug)]
pub enum UpdateCheckResult {
    UpToDate,
    Available(ReleaseDescriptor),
    /// Released on GitHub, but the WinGet source does not list it yet.
    Pending(String),
}

pub fn handle_cli_mode(args: &[String]) -> Option<i32> {
    if args.get(1).is_some_and(|arg| arg == "--apply-update") {
        return Some(
            match parse_apply_update_args(args).and_then(|(target, source, pid, integrity)| {
                apply_update(target, source, pid, &integrity)
            }) {
                Ok(()) => 0,
                Err(error) => {
                    show_error_message("Update failed", &error);
                    1
                }
            },
        );
    }

    None
}

fn parse_apply_update_args(
    args: &[String],
) -> Result<(PathBuf, PathBuf, u32, AssetIntegrity), String> {
    if args.len() != 7 {
        return Err(
            "The updater helper requires a target, source, process ID, size, and SHA-256 digest."
                .into(),
        );
    }
    let pid = args[4]
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or_else(|| "Invalid updater process ID.".to_string())?;
    let size = args[5]
        .parse::<u64>()
        .map_err(|_| "Invalid update size.".to_string())?;
    let integrity = AssetIntegrity::new(size, Some(&args[6]))?;
    Ok((
        PathBuf::from(&args[2]),
        PathBuf::from(&args[3]),
        pid,
        integrity,
    ))
}

pub fn current_install_channel() -> InstallChannel {
    match std::env::current_exe() {
        Ok(path) if is_winget_install_path(&path) => InstallChannel::Winget,
        _ => InstallChannel::Portable,
    }
}

pub fn check_for_updates(channel: InstallChannel) -> Result<UpdateCheckResult, String> {
    let Some(release) = fetch_latest_release()? else {
        return Ok(UpdateCheckResult::UpToDate);
    };
    // GitHub releases publish before the winget-pkgs manifest PR merges, and
    // `winget upgrade` finds nothing to install until then.
    if channel == InstallChannel::Winget && !winget_has_version(&release.latest_version)? {
        return Ok(UpdateCheckResult::Pending(release.latest_version));
    }
    Ok(UpdateCheckResult::Available(release))
}

pub fn begin_winget_update() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Unable to locate current executable: {e}"))?;
    let current_dir = current_exe
        .parent()
        .ok_or_else(|| "Unable to determine the app directory for restart.".to_string())?;
    let command = winget_upgrade_command(
        std::process::id(),
        &current_exe.to_string_lossy(),
        &current_dir.to_string_lossy(),
    );

    Command::new("powershell.exe")
        .arg("-NoLogo")
        .arg("-Command")
        .arg(&command)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| format!("Unable to launch WinGet update command: {e}"))?;

    Ok(())
}

pub fn begin_self_update(release: &ReleaseDescriptor) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Unable to locate current executable: {e}"))?;
    ensure_target_location_writable(&current_exe)?;

    let stage_dir = updates_dir()?;
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("Unable to create updater working directory: {e}"))?;

    let helper_path = stage_dir.join(HELPER_EXE_NAME);
    let download_path = stage_dir.join(DOWNLOAD_EXE_NAME);
    let partial_download_path = stage_dir.join(format!("{DOWNLOAD_EXE_NAME}.part"));

    if helper_path.exists() {
        let _ = std::fs::remove_file(&helper_path);
    }
    if download_path.exists() {
        let _ = std::fs::remove_file(&download_path);
    }
    if partial_download_path.exists() {
        let _ = std::fs::remove_file(&partial_download_path);
    }

    download_release_asset(release, &partial_download_path, &download_path)?;
    std::fs::copy(&current_exe, &helper_path)
        .map_err(|e| format!("Unable to prepare updater helper: {e}"))?;

    let pid = std::process::id().to_string();
    let target = current_exe.to_string_lossy().to_string();
    let source = download_path.to_string_lossy().to_string();

    Command::new(&helper_path)
        .arg("--apply-update")
        .arg(target)
        .arg(source)
        .arg(pid)
        .arg(release.integrity.size_arg())
        .arg(release.integrity.digest_arg())
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Unable to launch updater helper: {e}"))?;

    Ok(())
}

fn apply_update(
    target: PathBuf,
    source: PathBuf,
    pid: u32,
    integrity: &AssetIntegrity,
) -> Result<(), String> {
    // Keep this handle open through replacement so the checked bytes cannot change.
    let verified_source = open_verified_source(&source, integrity)?;
    let _ = wait_for_process_exit(pid, Duration::from_secs(30));
    replace_target_binary(&target, &source)?;
    relaunch_target(&target)?;
    drop(verified_source);
    let _ = std::fs::remove_file(&source);

    Ok(())
}

fn fetch_latest_release() -> Result<Option<ReleaseDescriptor>, String> {
    let (owner, repo) = github_repo()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let agent = build_agent()?;

    let mut response = agent
        .get(&url)
        .header("Accept", GITHUB_API_ACCEPT)
        .header("User-Agent", user_agent())
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .call()
        .map_err(|e| format!("Unable to check GitHub releases: {e}"))?;

    let release: GitHubRelease = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("Unable to parse GitHub release data: {e}"))?;

    release_descriptor(release, env!("CARGO_PKG_VERSION"))
}

fn build_agent() -> Result<ureq::Agent, String> {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    Ok(ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(tls)
        .build()
        .into())
}

fn replace_target_binary(target: &Path, source: &Path) -> Result<(), String> {
    let backup_path = backup_path_for(target);
    let mut last_error = None;

    for _ in 0..60 {
        let _ = std::fs::remove_file(&backup_path);

        let renamed_existing = match std::fs::rename(target, &backup_path) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };

        match std::fs::copy(source, target) {
            Ok(_) => {
                let _ = std::fs::remove_file(&backup_path);
                return Ok(());
            }
            Err(error) => {
                last_error = Some(error);
                let _ = std::fs::remove_file(target);
                if renamed_existing {
                    let _ = std::fs::rename(&backup_path, target);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Unable to replace {}. {}",
        target.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| {
                "The file may still be locked or the install directory may not be writable."
                    .to_string()
            })
    ))
}

fn relaunch_target(target: &Path) -> Result<(), String> {
    let mut command = Command::new(target);
    if let Some(parent) = target.parent() {
        command.current_dir(parent);
    }

    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "The update was installed, but the app could not be restarted automatically: {e}"
            )
        })?;

    Ok(())
}

fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), String> {
    if pid == 0 {
        return Ok(());
    }

    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, false, pid)
            .map_err(|e| format!("Unable to monitor the running app process: {e}"))?;

        let result = WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128) as u32);
        let _ = windows::Win32::Foundation::CloseHandle(handle);

        if result == WAIT_OBJECT_0 {
            Ok(())
        } else if result == WAIT_TIMEOUT {
            Err("Timed out waiting for the running app to exit.".to_string())
        } else {
            Err("Unable to confirm that the running app has exited.".to_string())
        }
    }
}

fn updates_dir() -> Result<PathBuf, String> {
    crate::known_folders::local_app_data_dir()
        .map(|dir| dir.join("ClaudeCodeUsageMonitor").join("updates"))
        .or_else(|| {
            Some(
                std::env::temp_dir()
                    .join("ClaudeCodeUsageMonitor")
                    .join("updates"),
            )
        })
        .ok_or_else(|| "Unable to resolve a writable local updates directory.".to_string())
}

fn winget_has_version(version: &str) -> Result<bool, String> {
    let mut child = Command::new("winget.exe")
        .args(winget_show_args(version))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Unable to run WinGet: {e}"))?;

    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return winget_show_outcome(status.code()),
            Ok(None) if started.elapsed() < WINGET_SHOW_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(250));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Timed out waiting for WinGet to check the available version.".into());
            }
            Err(error) => return Err(format!("Unable to wait for WinGet: {error}")),
        }
    }
}

fn winget_show_args(version: &str) -> [&str; 10] {
    [
        "show",
        "--id",
        WINGET_PACKAGE_ID,
        "--exact",
        "--version",
        version,
        "--source",
        "winget",
        "--accept-source-agreements",
        "--disable-interactivity",
    ]
}

fn winget_show_outcome(exit_code: Option<i32>) -> Result<bool, String> {
    // Windows exit codes are HRESULTs; Rust reports them as signed values.
    match exit_code.map(|code| code as u32) {
        Some(0) => Ok(true),
        Some(WINGET_NO_MANIFEST_FOUND | WINGET_NO_APPLICATIONS_FOUND) => Ok(false),
        Some(code) => Err(format!(
            "WinGet could not check the available version (exit code 0x{code:08X})."
        )),
        None => Err("WinGet exited without a status code.".into()),
    }
}

fn winget_upgrade_command(pid: u32, target: &str, working_dir: &str) -> String {
    let target = powershell_single_quoted(target);
    let working_dir = powershell_single_quoted(working_dir);
    let package_id = WINGET_PACKAGE_ID;

    format!(
        concat!(
            "$ErrorActionPreference = 'Stop'; ",
            "$pidToWait = {pid}; ",
            "$target = '{target}'; ",
            "$workingDir = '{working_dir}'; ",
            "try {{ Wait-Process -Id $pidToWait -Timeout 30 -ErrorAction Stop }} catch {{ }}; ",
            "winget upgrade --id {package_id} --exact; ",
            "$exitCode = $LASTEXITCODE; ",
            "if ($exitCode -eq 0) {{ ",
            "Start-Sleep -Seconds 2; ",
            "Start-Process -FilePath $target -WorkingDirectory $workingDir; ",
            "exit 0 ",
            "}}; ",
            "Write-Host ''; ",
            "Write-Host 'WinGet update failed with exit code' $exitCode; ",
            "Read-Host 'Press Enter to close'; ",
            "exit $exitCode"
        ),
        pid = pid,
        target = target,
        working_dir = working_dir,
        package_id = package_id,
    )
}

fn powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

fn backup_path_for(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("app.exe");
    target.with_file_name(format!("{file_name}.old"))
}

fn ensure_target_location_writable(target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or_else(|| {
        "Unable to determine the install directory for the current executable.".to_string()
    })?;

    let probe_path = parent.join(".__ccum_update_probe");
    match File::create(&probe_path) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe_path);
            Ok(())
        }
        Err(error) => Err(format!(
            "The current install location is not writable. Move the app to a user-writable folder or install it somewhere outside Program Files. {error}"
        )),
    }
}

fn github_repo() -> Result<(&'static str, &'static str), String> {
    let repository = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let parts: Vec<&str> = repository.split('/').collect();
    if parts.len() < 2 {
        return Err("Package repository URL is not configured for GitHub releases.".to_string());
    }

    let owner = parts[parts.len() - 2];
    let repo = parts[parts.len() - 1];
    if owner.is_empty() || repo.is_empty() {
        return Err("Package repository URL is not configured for GitHub releases.".to_string());
    }

    Ok((owner, repo))
}

fn user_agent() -> &'static str {
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
}

fn is_winget_install_path(path: &Path) -> bool {
    let normalized_path = normalize_path(path);
    winget_install_roots()
        .into_iter()
        .map(|root| normalize_path(&root))
        .any(|root| {
            normalized_path
                .strip_prefix(&root)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('\\'))
        })
}

fn winget_install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages"),
        );
    }

    if let Ok(program_files) = std::env::var("ProgramFiles") {
        roots.push(PathBuf::from(program_files).join("WinGet").join("Packages"));
    } else {
        roots.push(PathBuf::from(r"C:\Program Files\WinGet\Packages"));
    }

    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        roots.push(
            PathBuf::from(program_files_x86)
                .join("WinGet")
                .join("Packages"),
        );
    } else {
        roots.push(PathBuf::from(r"C:\Program Files (x86)\WinGet\Packages"));
    }

    roots
}

fn normalize_path(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();

    normalized
        .strip_prefix("\\\\?\\unc\\")
        .map(|rest| format!("\\\\{rest}"))
        .or_else(|| normalized.strip_prefix("\\\\?\\").map(str::to_owned))
        .unwrap_or(normalized)
}

fn show_error_message(title: &str, message: &str) {
    unsafe {
        let title_wide = wide_str(title);
        let message_wide = wide_str(message);
        let _ = MessageBoxW(
            Some(HWND::default()),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide_str(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests;
