# User guide

Start here to customise Claude Code Usage Monitor and adjust its everyday settings.
For installation and provider sign-in requirements, see the [README](README.md).
The instructions below use the app's English labels.

## Contents

- [Open the dashboard](#open-the-dashboard)
- [Duplicate and customise a built-in theme](#duplicate-and-customise-a-built-in-theme)
- [Choose providers and refresh usage](#choose-providers-and-refresh-usage)
- [Show used or remaining allowance](#show-used-or-remaining-allowance)
- [Windows VM testing for developers](#windows-vm-testing-for-developers)

## Open the dashboard

With the default theme, right-click a provider icon in the Windows system tray
and choose **Open Dashboard**. If the icon is hidden, look in the tray's overflow
menu beside the clock.

You can also open the dashboard from PowerShell:

```powershell
claude-code-usage-monitor --dashboard
```

For a portable download, run `.\claude-code-usage-monitor.exe --dashboard` from
the folder containing the executable.

## Duplicate and customise a built-in theme

Built-in themes are read-only. Create an editable copy to make your own design;
the original stays available to switch back to.

### Create your copy

1. Open **Theme Studio** from the dashboard's navigation.
2. In the **Theme** dropdown at the top, select the theme you want to start from,
   such as **Compact Fluent Quad**. Built-in themes are marked **(built-in)**.
3. Click the toolbar's copy icon beside the theme dropdown. Hover over it to see
   the **Duplicate...** tooltip. Use this top toolbar control to copy the whole
   theme; the **Duplicate** control beneath the layer list copies a layer.
4. In **Duplicate theme**, enter a name such as `My theme`, then click
   **Create copy**.

The app saves your copy, selects it in Theme Studio, and makes it the active
theme. Its controls are now editable.

### Make and save a change

1. Select a layer in the tree on the left. Expand its arrow to find nested layers.
2. Use the inspector on the right to edit the selected layer. Start with a small
   change, such as a colour or text size, and check the preview in the centre.
   **Appearance** contains appearance controls; **Positioning** contains layout
   controls. The available fields depend on the selected layer.
3. Click the toolbar's disk icon, with the **Save** tooltip, to save your changes
   and apply them to the running widget.

**Live apply** is disabled by default. With it disabled, edits update the Studio
preview until you click **Save**. Enable it to save and apply each edit
automatically. Enabling it also saves any changes already waiting in the editor.

Use the toolbar's **Undo** and **Redo** icons, or **Ctrl+Z** and **Ctrl+Y**, to
reverse or restore edits. With Live apply disabled, save again to apply an undo
to the running widget. If you close the dashboard or switch themes with unsaved
changes, choose **Save and continue**, **Discard changes**, or **Cancel**.

### Switch back to a built-in theme

Select the original theme from the **Theme** dropdown in Theme Studio. You can
also choose it under **Settings > Appearance > Active theme**. Switching applies
the selected theme; your saved custom copy remains available in the dropdown.

Custom theme files are stored in:

```text
%APPDATA%\ClaudeCodeUsageMonitor\themes
```

## Choose providers and refresh usage

1. Sign in to the provider you want to monitor using its own app or CLI. See
   [Provider setup](README.md#provider-setup) for provider-specific requirements.
2. Open **Settings** and find **Providers**. Enable the providers you want to
   monitor and disable any you do not use.
3. Under **Settings > General**, set **Update frequency** to the number of
   minutes between usage refreshes. This controls usage polling, not checks for
   new app versions.
4. Click **Refresh now** beside that setting when you want a fresh reading
   without waiting for the next scheduled refresh.

These settings save automatically. The default theme adapts to enabled providers;
a custom theme must include layers for the providers you want to display.

## Show used or remaining allowance

1. Open **Settings > Display**.
2. Set **Usage direction** to **Used** or **Remaining**.

| Option | What the percentage means | Example |
| --- | --- | --- |
| **Used** (default) | How much of the allowance you have consumed. | After using 30%, the display reads 30%. |
| **Remaining** | How much allowance is left. | After using 30%, the display reads 70%. |

The setting saves automatically. The default theme and Compact Fluent Quad
support both directions. Custom themes need to support this setting too; a
theme that always displays consumed usage may stay unchanged. See the
[theme binding notes](README.md#usage) if you are editing usage expressions.

## Claude extra limits in custom themes

Claude may report extra quotas in its usage API, including model-specific caps.
These are available to custom themes. Built-in themes and the existing session,
weekly, and headline bindings retain their current behaviour.

After refreshing Claude usage, open the text editor's **Provider values** list or
the expression editor's **Variables** panel and look under **Claude Code**.
The text editor lists each reported quota with its summary, label, usage,
remaining allowance, and reset formats. Select a value and format, then click
**Insert value**. The expression editor also lists the exact binding keys. API quotas vary by account and may disappear or change over time.

| Binding | Meaning |
| --- | --- |
| `claude.limits.count` | Number of parsed quotas, including standard windows when supplied in `limits[]`. |
| `claude.limits.weekly_scoped_fable.*` | A specific quota from `limits[]`, using its kind and model name. |
| `claude.model.fable.*` | Shortcut for a model's weekly quota, when unambiguous. |
| `claude.scoped.*` | The single scoped quota marked `is_active` by the API. Unavailable if none or multiple are active. |
| `claude.limits.seven_day_cowork.*` | An older optional top-level quota bucket, when reported. |

The examples are illustrative; only quotas actually returned by the API have
`available = 1`. No quota is inferred from a model name or subscription plan.
Model names become lowercase keys with punctuation replaced by underscores:
`Future Model 2` becomes `future_model_2`. Non-model scopes and colliding names
have a stable hash suffix; use the exact key shown in the Variables panel.
The array takes precedence over legacy Opus/Sonnet buckets for the same model.
If multiple weekly quotas share a model key, use the full `limits` keys instead
of the ambiguous model shortcut.

Each quota exposes:

- `available`, `percentage` (used), `remaining`, and `display` (follows Usage direction).
- `is_active`, as supplied by the API; it is not inferred from the highest percentage.
- `reset.unix`, `reset.seconds`, `reset.minutes`, `reset.hours`, and `reset.days`.
- Text fields `label`, `kind`, `key`, `model_id`, and `scope` (the scope JSON, or empty).

For a custom Fable bar, use `claude.model.fable.available` as the layer's
**Render** expression and `claude.model.fable.display` as its progress value.
A text layer can use:

```text
{claude.model.fable.label} {claude.model.fable.display:usage_line}
```

Omit `.display` to always show used usage. The `usage_badge` format is also
supported. For a specific account, replace `claude` with its account binding,
for example `accounts.claude.work.model.fable.percentage`. Each account keeps its
own quotas; changing the default account updates the plain `claude.*` bindings.

Missing quotas have `available = 0`, zero used usage/reset values, and empty
metadata. Always gate optional layers on `available` so a missing quota is not
presented as an unused allowance. These bindings validate even before login.
Cached limits follow the existing stale-data behaviour; check `claude.stale`
(or the account's `.stale`) when freshness matters.

## Windows VM testing for developers

Run this harness locally on a developer machine with Hyper-V. The scripts live in
`tests/windows-vm/`. This proof of concept provisions two disposable guests and runs desktop
scenarios against known checkpoints. `New-LabVM.ps1` applies an installation image
to a new VHDX, prepares a local account and boots the guest. `Invoke-Lab.ps1` runs
the test matrix.

### Coverage

| Flow | What the runner exercises | Inputs |
| --- | --- | --- |
| `portable-launch` (default) | Launch from a path with spaces, visible widget, one instance, restart, retained language and poll interval | Candidate EXE |
| `portable-update-helper` | Old EXE launches; its real `--apply-update` helper verifies, replaces and relaunches the candidate; candidate SHA-256 and retained settings checked | Previous and newer candidate EXEs |
| `winget-install` | Public-source installation of a pinned version, package detection, launch, restart, uninstall | Published candidate version |
| `winget-upgrade` | Public-source installation of a pinned old version, CLI upgrade to a pinned candidate, retained settings, launch and uninstall | Two published versions |

The helper scenario does **not** cover the app's update button, GitHub metadata
check, download, or staging. The old EXE must support the current helper arguments
(size and SHA-256). WinGet scenarios test `winget` commands, not the app's WinGet
update action. Unpublished candidates are supported only in portable scenarios.
Neither result should be reported as full updater end-to-end coverage.

The default taskbar matrix is baseline and auto-hide on both OS versions, plus
left and centre alignment on Windows 11: six scenarios per flow, 24 for all four.
`left` means Windows 11 icon alignment, not a Windows 10 taskbar edge.
Use selected `-Flows` and `-Taskbars` for quick checks.
Use `-VMNames CCUM-Win10` or `-VMNames CCUM-Win11` to select a single guest.

Assertions inspect process count/path, widget class and visibility, nonempty bounds,
display intersection (except with auto-hide), hashes/versions where applicable,
settings values, and command exit codes. Screenshots support visual review; passing
does not establish freedom from clipping, overlap, rendering defects, or correct
auto-hide animation. Alignment checks verify the registry preference, not icon positions.
Restarts currently terminate the app process; graceful shutdown is not covered.

### Try the harness without VMs

From the repository root in Windows PowerShell 5.1:

```powershell
.\tests\windows-vm\Test-Harness.ps1
.\tests\windows-vm\Invoke-Lab.ps1 -PlanOnly
.\tests\windows-vm\Invoke-Lab.ps1 -PlanOnly `
  -Flows portable-launch,portable-update-helper,winget-install,winget-upgrade
```

`-PlanOnly` validates the configuration and emits the scenario matrix as JSON,
without loading Hyper-V, checking binaries, or requiring credentials.
The harness checks validate configuration, matrix generation, VM/checkpoint refusal
paths, PowerShell parsing and C# bridge compilation. They do not simulate Windows UI.

### Provision from installation media

From an elevated Windows PowerShell session, supply a lab-only account credential:

```powershell
$credential = Get-Credential -UserName LabUser -Message 'New disposable VM account'
.\tests\windows-vm\New-LabVM.ps1 -VMName CCUM-Win10 `
  -IsoPath C:\work\ISO\Windows10.iso -ImageName 'Windows 10 Pro' `
  -Credential $credential
.\tests\windows-vm\New-LabVM.ps1 -VMName CCUM-Win11 `
  -IsoPath C:\work\ISO\Windows11.iso -ImageName 'Windows 11 Enterprise Evaluation' `
  -Credential $credential
```

Choose image names actually present in your media. The Windows 11 media supplied
for the first run contains Enterprise Evaluation. The script does not activate
Windows or read product-key files. VM files default to `C:\work\CCUM-Lab`.
Existing VM names and provisioning directories are refused. `-Resume` can continue
after image application if `image-applied.json` matches the selected VM and media.
It never reformats an existing disk when resuming. Interrupted image application
without this marker requires separate inspection, not an automatic retry.

The new account signs in once to run `Initialize-Guest.ps1`. It disables automatic
locking/sleep and automatic Windows updates in the guest for a reproducible lab,
then removes temporary autologon credentials and the answer file. A successful
setup writes `C:\CCUM-Lab\initialized.json` inside the VM. Wait for that marker and
a settled desktop before taking checkpoints. Setup can reboot several times.
`Save-VMConsole.ps1 -VMName CCUM-Win10 -Path C:\work\win10.png` captures the console
through Hyper-V even before the guest bridge is available.

The account password temporarily exists in the guest's unattended installation
file during setup. For unattended host runs, a PSCredential can be saved with
`Export-Clixml` outside the repository; Windows encrypts it for that host user.
The first run used `C:\work\CCUM-Lab\credential.clixml` with a restricted ACL.
Never print the decrypted credential or put it in source control.

### Prepare checkpoints

Suggested initial allocation: 2 vCPUs, 4 GB RAM and a 64 GB dynamically expanding
disk per guest. Run scenarios sequentially. Use x64 Windows 10 22H2 and a current
supported Windows 11 build; record the exact editions, builds and patch dates.
Windows 11 needs a generation 2 VM with Secure Boot and virtual TPM enabled.
Use your own installation media and licences. Activation remains separate.

The host must have Hyper-V and a PowerShell session with Hyper-V administrative
access. On each VM:

1. Use the provisioned local administrator named `LabUser`, with the same lab-only credential
   on both guests. Leave it signed into an **unlocked console desktop** with Explorer
   running. Use the basic VMConnect console; an RDP session that locks/disconnects
   can invalidate desktop tests. The runner will not automatically log in or unlock.
2. Install and initialise App Installer/WinGet for that user. Keep its default
   per-user portable package directory and public `winget` source. The guest needs
   internet access for WinGet. Do not install the monitor or sign into real provider
   accounts; the baseline must have no monitor settings or running monitor process.
   `Initialize-WinGet.ps1` must run in the interactive desktop. `-UseInstalled`
   keeps an existing usable client. Otherwise stage Microsoft's official bundle at
   `C:\CCUM-Lab\downloads\Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle`
   and its x64 dependencies under `C:\CCUM-Lab\downloads\x64`. It records the
   actual version and package lookup result; a source-update exit code alone is
   insufficient because WinGet can report zero after a cancelled update.
3. Use a known display resolution/scaling and a bottom, visible taskbar. Disable
   sleep, screen saver/automatic locking, and automatic monitor startup in this
   disposable test profile. Complete pending OS updates/reboots before capture.
4. In the guest, create the explicit opt-in marker:

   ```powershell
   New-Item -ItemType Directory -Path C:\CCUM-Lab -Force
   New-Item -ItemType File -Path C:\CCUM-Lab\guest-enabled -Force
   ```

5. On the host, mark **only the disposable test VMs** and take standard checkpoints
   while their console desktops are signed in and unlocked:

   ```powershell
   foreach ($name in 'CCUM-Win10', 'CCUM-Win11') {
     Set-VM -Name $name -Notes 'CCUM-DISPOSABLE-LAB' `
       -CheckpointType Standard -AutomaticCheckpointsEnabled $false
     Checkpoint-VM -Name $name -SnapshotName 'ccum-clean-desktop'
   }
   ```

Standard checkpoints preserve VM memory, which lets this POC resume the desktop
session without storing autologon passwords. Production checkpoints are refused.
Use exactly one checkpoint with that name per VM. Rebuild/checkpoint updated images
deliberately instead of letting the baseline change between scenarios.

Copy `tests/windows-vm/lab.example.json` to `tests/windows-vm/lab.local.json` to customise VM names, checkpoint names
and usernames. Names must start with `CCUM-`. Local config and default evidence are
gitignored. Credentials are supplied as PSCredential objects, never written to the
JSON config or scenario logs.

Keep a pristine checkpoint **without** the Visual C++ Redistributable for detecting
undeclared runtime dependencies. A separate `ccum-runtime-desktop` checkpoint can
include the signed Microsoft x64 redistributable for exercising the rest of the
matrix. Do not treat passing results on that checkpoint as clean-install success.

### Run against the prepared guests

Build a candidate, then run from an elevated Windows PowerShell host session:

```powershell
cargo build --release --locked
$credential = Get-Credential -UserName LabUser -Message 'Disposable VM local administrator'
.\tests\windows-vm\Invoke-Lab.ps1 `
  -CandidateExe .\target\release\claude-code-usage-monitor.exe `
  -Credential $credential
```

Portable helper coverage (the candidate product version must be newer):

```powershell
.\tests\windows-vm\Invoke-Lab.ps1 `
  -Flows portable-update-helper -Taskbars baseline `
  -PreviousExe C:\LabBinaries\previous\claude-code-usage-monitor.exe `
  -CandidateExe .\target\release\claude-code-usage-monitor.exe `
  -Credential $credential
```

WinGet coverage uses **versions actually available in the public source**, supplied
explicitly, rather than silently testing whichever release is latest:

```powershell
# Set these to the published versions you intend to validate.
$previousVersion = '2.14.54'
$candidateVersion = '2.14.55'
.\tests\windows-vm\Invoke-Lab.ps1 `
  -Flows winget-install,winget-upgrade -Taskbars baseline `
  -PreviousVersion $previousVersion -CandidateVersion $candidateVersion `
  -Credential $credential
```

These version numbers are examples, not an availability assertion. Missing versions,
source/network failures and timeouts are reported as failures. WinGet scenarios do
not use `-CandidateExe`. Add `-ConfigPath .\tests\windows-vm\lab.local.json` when
using local configuration.

For each scenario the host restores the baseline, transfers scripts and binaries
through PowerShell Direct, starts a limited-token interactive scheduled task for
`LabUser`, waits for its result, copies evidence, and restores the baseline again
in `finally`. The guest must match the configured OS. A global mutex prevents
concurrent harness runs from restoring each other's VMs. No inbound guest service,
WinRM network listener or Docker service is required.

The runner restores VM state, discarding changes since the checkpoint. It requires
the exact disposable-VM notes marker before touching any VM. A guest marker and
Hyper-V model check prevent accidental guest-script execution on a physical host.
Abrupt host shutdown or killing PowerShell can prevent `finally`; restore the named
checkpoint manually before reuse if a run is forcibly interrupted.

### Results and AI use

`tests/windows-vm/artifacts/<run-id>/summary.json` records status and evidence paths for each executed
scenario. Each scenario's `evidence` subfolder contains its result/checks, OS build,
window bounds, screenshots, executable version/hash, settings, diagnostics,
transcript and WinGet output when used. The host also records the scheduled task's
exit result. Timeout evidence may be partial. An ordinary failure continues to the
next scenario; checkpoint cleanup failure aborts the run. Any failed scenario makes
the command fail with a nonzero process exit when invoked with `powershell.exe -File`.

An AI can invoke the runner, read JSON/logs, inspect PNGs, change code, and rerun a
selected matrix once host access and VM setup are available. The harness determines
pass/fail. There is no AI agent installed in the guests and no automatic repair or
deployment action. Keep provider credentials and personal data out of the lab;
evidence intentionally includes the test user's settings and desktop.

### Follow-up coverage

- A test-only, explicit HTTPS release endpoint and trusted fixture assets for full
  portable update check/download/staging/UI tests, retaining size/digest validation.
- A private WinGet test source for unpublished old/candidate versions; separate
  app-initiated upgrade and public-source availability tests.
- First launch with no settings file, repeated launch, graceful exit, Explorer
  recovery, reboot, failed/interrupted updates and migration assertions.
- Windows 10 taskbar edges, crowded taskbars, light/dark modes, DPI/resolution
  changes and multi-monitor scenarios with explicit geometry assertions.

Keep the fast Rust tests, especially updater safeguards and placement regression
tests. This POC adds real desktop evidence; it does not justify removing them yet.

### Platform references

- [PowerShell Direct requirements and file transfer](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/powershell-direct)
- [Hyper-V standard and production checkpoints](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/checkpoints)
- [Interactive scheduled-task principals](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtaskprincipal)
- [Windows 11 VM requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements)
- [WinGet upgrade options](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)
