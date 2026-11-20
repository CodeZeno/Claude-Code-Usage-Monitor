# Chocolatey distribution

The package ID is `claude-code-usage-monitor`. The first public submission still
needs approval from the Chocolatey community repository. Package ownership and
availability must be confirmed there; preparing these files does not reserve the
name or publish a package.

## Installation and updates

Once the package is approved, run these commands in an administrator terminal:

```powershell
choco install claude-code-usage-monitor
claude-code-usage-monitor
# Open settings and Theme Studio directly:
claude-code-usage-monitor --dashboard
```

For a `packages.config` file, add:

```xml
<package id="claude-code-usage-monitor" />
```

Save your work and quit both the widget and dashboard before either command:

```powershell
choco upgrade claude-code-usage-monitor
choco uninstall claude-code-usage-monitor
```

Upgrades and uninstalls refuse to proceed if this package's executable is still
running. The before-modify script reports this early, but Chocolatey may treat
its failure as a warning, so the new version's install script and the uninstall
script repeat the check before changing files or startup entries. A running copy
from another location does not block them. They do not terminate applications or
restart the app automatically. After upgrading, launch the monitor again.

Windows 10 or 11 x64 is required, or Windows 11 on Arm, which runs the x64
executable through emulation. Install refuses 32-bit Windows, Windows 10 on Arm,
and older Windows versions. Provider software and credentials are not installed
by this package.

Chocolatey owns this installation's updates. The app's update button displays
the upgrade instructions and does not check GitHub, report that the version is
current, or replace the executable directly. The package includes a marker for
this behavior, including when Chocolatey uses a custom root directory.

Uninstall preserves settings in `%APPDATA%\ClaudeCodeUsageMonitor` and provider
credentials. It removes startup Run values only when they point to this package's
executable, across loaded local, domain, and Microsoft Entra ID user profiles
accessible to the administrator. Offline
user profiles are not loaded or edited; users of those profiles may need to remove
a stale startup entry after uninstalling. Other installations' entries are left
alone. Upgrades retain the stable executable path and startup registration.

## Release automation

Set the GitHub Actions **repository secret** `CHOCOLATEY_API_KEY` to the API key
of the Chocolatey account that owns this package. Do not put it in source files.
The nuspec's authors/owners metadata does not grant repository ownership.

The release workflow runs the Chocolatey job after a successful release build:

1. Download the executable from the exact published stable tag.
2. Verify its embedded version and x64 architecture, calculate SHA-256, and
   render the package with an exact versioned URL and pinned checksum.
3. Test package lifecycle safeguards and generation without modifying the machine.
4. Run `choco pack` and upload the `.nupkg` as the `chocolatey-package` workflow
   artifact.
5. Submit to `https://push.chocolatey.org/` if the API key is configured.

No AU dependency, release polling, or separate packaging repository is needed.
Prerelease/build-metadata tags and releases from forks are excluded. A missing
key skips only submission; the package is still built for review. Submission
errors fail the job, leaving the GitHub release and WinGet job independent.
GitHub publication does not guarantee immediate Chocolatey availability.

The initial submission goes through automated checks and human moderation.
Monitor the package page for required changes. If a package is in moderation,
resolve feedback and resubmit the corrected package as appropriate rather than
blindly rerunning successful submissions. The downloaded workflow artifact can
be inspected or used for manual resubmission. Do not publish the same version
with a different executable or download checksum.

## Local verification

```powershell
# Run in Windows PowerShell 5.1, which Chocolatey uses for package scripts.
./tests/Test-ChocolateyPackage.ps1 -BinaryPath target/release/claude-code-usage-monitor.exe
./packaging/chocolatey/New-Package.ps1 -Version 2.25.0 -BinaryPath target/release/claude-code-usage-monitor.exe -OutputDirectory target/chocolatey-package
choco pack target/chocolatey-package/claude-code-usage-monitor.nuspec --output-directory target/chocolatey-output
```

Use a new output directory each time; the generator refuses to mix stale files.
Output belongs under ignored `target/`. The source package includes only scripts,
metadata, and marker files; the binary is downloaded and checked during install.

The Hyper-V lab's `chocolatey-package` case automates most of the checks below
from a custom root with spaces; see [Windows VM testing](windows-vm-testing.md).
Test actual installs in a disposable Windows VM with Chocolatey installed:

- Install from a local package source; check the shim launches the widget and
  forwards `--dashboard`, without leaving a console waiting for the GUI.
- Attempt upgrade/uninstall with the widget or dashboard open; require a clear
  refusal without process termination or file changes.
- Close both processes, upgrade from an older package, and verify the version,
  settings, theme, and enabled startup entry survive.
- Check the update button shows Chocolatey instructions, with no portable update.
- Uninstall after enabling startup; verify the shim and package disappear, the
  matching startup value is removed, settings survive, and another portable or
  WinGet copy and its startup entry remain untouched.
- Repeat with a custom Chocolatey root, paths with spaces, and elevation using
  a different administrator account. Check startup removal for loaded users.

References: [package creation](https://docs.chocolatey.org/en-us/create/create-packages/),
[download helper](https://docs.chocolatey.org/en-us/create/functions/get-chocolateywebfile/),
[submission](https://docs.chocolatey.org/en-us/create/commands/push/), and
[moderation](https://docs.chocolatey.org/en-us/community-repository/moderation/).
