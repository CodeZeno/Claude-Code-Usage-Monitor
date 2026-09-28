# Extending the Windows VM tests

The ordered test catalogue is [`tests/windows-vm/tests.json`](../tests/windows-vm/tests.json).
Each entry points to one case script. The host restores the configured checkpoint,
copies the selected case and shared support files, runs it on the guest desktop,
collects evidence, and restores the checkpoint again. Cases run sequentially and
must not depend on another case having run first.

See the [Windows VM setup guide](../USER_GUIDE.md#windows-vm-testing-for-developers)
for provisioning, credentials, checkpoints, and coverage limitations.

## Discover and select tests

Run from the repository root in Windows PowerShell 5.1:

```powershell
.\tests\windows-vm\Invoke-Lab.ps1 -ListTests
.\tests\windows-vm\Invoke-Lab.ps1 -Suite regression -PlanOnly
.\tests\windows-vm\Invoke-Lab.ps1 -Tests portable-dashboard-warp -Taskbars baseline -PlanOnly
```

Listing needs no VM configuration. Planning needs the configuration but does not
load Hyper-V, require credentials/binaries, or change VM state. Plans include
unsupported combinations as `skipped`, with a reason.

| Suite | Tests | Default taskbars |
| --- | --- | --- |
| `smoke` (default) | Portable launch/restart and clean first launch | Baseline |
| `regression` | Desktop, settings, Explorer, display/theme, network, and locale coverage | Baseline, auto-hide, left, center |
| `update` | Portable update success/failures and sign-in startup | Baseline |
| `published-package` | WinGet install and upgrade | Baseline |
| `nightly` | 30-minute soak, 50 dashboard lifetimes, pause/resume and clock jump | Baseline |

`-Suite` accepts multiple suites and deduplicates their cases/combinations. Use
either `-Suite` or `-Tests`. Existing `-Flows` commands remain valid as an alias
for `-Tests`, with the original flow IDs. Explicit test selection uses each test's
declared taskbars; `-Taskbars` overrides suite/test defaults. `-VMNames` limits the
configured guests. Catalogue order determines test order, followed by configured
VM order and selected taskbar order.

The default smoke suite executes four scenarios. The original six test IDs still
select their original 34 runnable combinations. Use `-PlanOnly` for current suite
counts, including explicit skips for layouts a case does not exercise.

```powershell
$credential = Import-Clixml C:\work\CCUM-Lab\credential.clixml
.\tests\windows-vm\Invoke-Lab.ps1 -Suite smoke `
  -ConfigPath C:\work\CCUM-Lab\lab.runtime.json `
  -CandidateExe .\target\release\claude-code-usage-monitor.exe `
  -Credential $credential
```

The runner uses an existing executable. When building a new release executable,
follow the repository's dependency checks and build workflow first. The runner
validates all selected tests' required inputs before restoring a checkpoint.
Public WinGet tests require explicit published versions; they do not install the
local candidate executable.

## Add a test

1. Add `tests/windows-vm/cases/<test-id>.ps1`.
2. Add its entry to the `tests` array in `tests.json` in the desired execution order.
3. Run `Test-Harness.ps1`, inspect `-PlanOnly`, then run the new test against the
   applicable guests. Review the assertions and screenshots.

Example catalogue entry:

```json
{
  "id": "portable-launch",
  "description": "Launch from a path with spaces, restart, and retain settings",
  "script": "cases/portable-launch.ps1",
  "suites": ["smoke", "regression"],
  "operatingSystems": ["windows10", "windows11"],
  "taskbars": ["baseline", "auto-hide", "left", "center"],
  "requires": ["candidateExe"],
  "timeoutSeconds": 180
}
```

Use unique lowercase, hyphen-separated IDs. Script paths must be directly under
`cases/` and refer to existing `.ps1` files. `requires` can contain `candidateExe`,
`previousExe`, `candidateVersion`, and `previousVersion`, or be empty. A previous
binary/version also requires its candidate. Timeouts are integers from 60 to
7200 seconds; `-ScenarioTimeoutSeconds` overrides them for a run. The nightly
case allows 4200 seconds, including its 30-minute steady-state sampling period.

Supported taskbars are `baseline`, `auto-hide`, `left`, and `center`. The latter
two mean Windows 11 icon alignment and are skipped on Windows 10. Declare only
the modes the test can exercise. Suites are also defined in the catalogue: add
an ID and default taskbars to `suites`, then add that ID to the relevant tests.

A case receives a context and loads common functions into its own scope:

```powershell
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Guest.Helpers.ps1" -Context $Context

Initialize-TestSettings
$executable = Install-PortableApp
$app = Start-TestApp $executable
Test-AppRestart $executable
```

This is the existing launch case. For a new case, replace its actions and
assertions with the behavior being tested. Use `Assert-Check 'stable check name'
$condition $detail` to record an assertion and fail the case when it is false.
An assertion failure ends that case; the host continues to the next scenario
after restoring the checkpoint. Keep dependent steps in one case, such as
install, upgrade, and verify retained settings.

The context supplies `Root`, `Request` (test ID in `flow`, OS, taskbar, versions,
tray theme, candidate hash), `Evidence`, `SessionId`, `SettingsPath`,
`CandidateExe`, `PreviousExe`, and the shared `Checks` collection. A binary path
is usable only when the test declares that input. Place reusable fixtures and
helpers in `support/`; the runner transfers that directory automatically.

The guest runner checks the desktop and clean baseline and applies taskbar mode.
**Settings are created by the case**, not by the runner. First-launch
tests omit `Initialize-TestSettings`; `Start-TestApp` and `Test-AppRestart`
are specifically for the seeded-settings tests, so such cases should use their
own launch/default-settings assertions. Both the runner and shared helpers
refuse execution outside an explicitly prepared disposable guest.

## Results

Each run writes an ignored `tests/windows-vm/artifacts/<run-id>/` directory:

- `plan.json`: every selected combination and its test metadata.
- `summary.json`: machine-readable results for the entire plan, updated after
  each scenario, including duration, failed check, errors, and evidence location.
- `summary.md`: readable result table with relative evidence links.
- `<scenario-id>/evidence/`: guest checks, progress, logs, screenshots, settings,
  OS details, executable identity, and scheduled-task details when available.

| Status | Meaning |
| --- | --- |
| `passed` | Case and required evidence collection completed successfully |
| `failed` | An assertion failed during case execution |
| `error` | Setup, execution, timeout, or infrastructure error; also incomplete evidence after an otherwise passing case |
| `skipped` | Unsupported OS/taskbar combination, with a reason |
| `not-run` | Planned case not reached, for example after checkpoint cleanup failure |

Assertions in guest setup are infrastructure errors. A case assertion may also
represent an unmet test precondition; inspect its name and detail before treating
it as a product regression. Skips never count as passes. Missing required inputs
fail preflight instead of silently skipping coverage. A selection with no
supported scenarios is rejected; inspect its reasons using `-PlanOnly`.

Ordinary case failures/errors allow later cases to run. Checkpoint cleanup
failure stops the lab; remaining planned cases stay `not-run`. Failed cases,
errors, or unexecuted cases make the command fail. Run with `powershell.exe -File`
to propagate a nonzero process exit code to automation.

Evidence files are copied individually, with three attempts per file. A locked
transcript does not prevent recovery of other files. Collection errors are
recorded separately from the original failure. Timeouts may still leave partial
evidence; screenshots alone do not establish that an assertion passed.
`progress.json` is advisory: a concurrent reader can prevent an update without
changing an assertion's outcome. The final `result.json` retains every check.

`Test-Harness.ps1` checks parsing (including nested case/support scripts), desktop
bridge compilation, catalogue validation, selection, required inputs, VM safety
checks, report generation, host-state restoration, and locked evidence/progress
files. These fast
checks require no VM, administrator rights, network, or external test framework.

## Resilience scenarios

| Test ID | Automated checks and evidence |
| --- | --- |
| `first-run-clean-profile` | No credential files/overrides; created defaults; visible, nonblank widget; live tray sign-in message; fresh-settings `--dashboard`; no panic. |
| `explorer-restart-recovery` | Three Explorer kills/restarts; sample PIDs for 60 seconds each and require the final 30 seconds stable; one process, taskbar parent, restored tray, preserved settings. Includes auto-hide and both Win11 alignments. |
| `portable-update-failures` | Real candidate helper with wrong hash, wrong size, a deny-write/delete target handle, and interruption during replacement. Assert original executable/settings hashes; explicitly launch the surviving original and verify it remains usable. |
| `settings-resilience` | Corrupt, truncated, BOM, unknown-field, read-only, and real previous-release-generated settings; preserve known readable values and log save failures. Subcases continue after an assertion failure. Requires `PreviousExe`. |
| `startup-with-windows` | App's native startup menu, exact quoted Run command in a path with spaces, host reboot/automatic sign-in, single widget, portable update target validity, and startup removal using the app. Requires `PreviousExe`. Published-package cases also verify startup after upgrade and removal after uninstall. |
| `display-scale-change` | Windows Settings UI Automation selects 2560x1440 at 100/125/150/200%, then 1280x720 at 100%; assert actual window DPI, screen size, retained PID, docking/containment and nonblank pixels. Desktop/crop images at every step. |
| `system-theme-switch` | Live light/dark/high-contrast/off transitions, docked and floating placement, stable PID, differing light/dark pixels and rendered foreground/background contrast of at least 4.5. |
| `network-and-auth-errors` | Invalid lab-only Claude/Codex tokens, distinct cached errors and shell messages, host disconnect/reconnect, 90-second offline CPU/retry measurement, 60-second outbound WFP connection audit with all provider flags disabled. |
| `locale-and-theme-gallery` | All 14 languages crossed with both built-in themes, widget and dashboard pixel checks and screenshots embedded in `summary.md`; malformed custom-theme fallback. |
| `soak-and-power` | 50 dashboard open/close cycles, handle/thread/private-memory plateau checks, VM pause/resume, two-hour guest clock jump, explicit countdown-fixture precondition, 30 minutes of monotonic idle sampling. |

Example selections:

```powershell
.\tests\windows-vm\Invoke-Lab.ps1 -Tests first-run-clean-profile,explorer-restart-recovery -Taskbars baseline -PlanOnly
.\tests\windows-vm\Invoke-Lab.ps1 -Suite nightly -PlanOnly
# Add ConfigPath, Credential and CandidateExe to execute; settings/startup
# migration tests also need an older PreviousExe with a lower product version.
```

The previous binary is run to generate its settings and to exercise a real
portable upgrade. Its own startup behavior is part of that upgrade path. A
failure before upgrade can therefore originate in the previous release.

`portable-update-failures` pads a candidate copy to 96 MiB (below the helper's
100 MiB ceiling) with a deterministic, fully written PE overlay and
computes its real hash/size. It observes the `.old` backup, kills the helper,
and rejects a late interruption if the destination already matches the complete
candidate. If the incomplete replacement phase cannot be observed, the case
fails rather than claiming interruption coverage. A hash failure is deliberately
not repaired by the test. Relaunch checks mean **the original can be launched**;
they do not claim a killed helper can automatically restart it.

The display scenario needs an English Windows Settings UI and a virtual display
driver exposing both resolutions and all four scale choices at 1440p without sign-out.
Lower resolutions restrict Windows' scale menu, so 720p is tested at 100%.
An already-correct DPI does not require changing a disabled scale selector.
Each measured transition allows five seconds for shell/occupancy updates before
checking DPI and docking, and failed steps do not prevent later transitions.
It shuts down the guest to set Hyper-V `ResolutionType Maximum`, then boots
before launching the monitor so Windows enumerates the available modes, and restores the original
video configuration afterwards. For new display-test
guests, pass `-DisplayResolutionType Maximum` to `New-LabVM.ps1`, then capture
the unlocked desktop checkpoint. [Hyper-V resolution modes](https://learn.microsoft.com/en-us/powershell/module/hyper-v/set-vmvideo)
define whether Windows sees one fixed mode or all modes up to the maximum.
Missing controls/modes fail an explicit capability check. It never substitutes
a registry write or application restart for a live DPI transition. Screenshots
still need review for taskbar-icon overlap, sharpness, individual clipped labels
and missing glyphs; containment alone does not prove unobstructed rendering.
Pixel-population contrast and nonblank checks are useful regression detectors,
not a complete accessibility or font-shaping certification.

The network case uses deliberately invalid credentials, never real tokens.
Recovery means the service is reachable again and rejects those same tokens;
successful authenticated usage recovery needs a separate controlled service
fixture. Update checks are deferred by setting their last-check timestamp to
the current time. The application currently has no persistent update-disable
switch, and normalizes an all-disabled provider set back to Claude. The privacy
subcase explicitly fails that unmet precondition after collecting its trace; it
does not use `--no-poll` to manufacture a passing privacy result. The WFP trace
records TCP/UDP connection events, including short-lived or blocked attempts,
and correlates process-creation events for descendants. An online auth attempt
provides a positive capture control. It is scoped to monitor processes and their
children in the disposable guest, not host traffic.

The countdown probe attempts to seed a cached reset one hour ahead and renders
the real `claude.session.reset.seconds` binding as blue (positive), green (zero),
or red (negative), alongside its text. The current monitor starts without loading
that cache (only the dashboard loads it), so the positive-reset precondition fails
explicitly and is retained in `countdown-coverage.json`. The case also verifies
that the probe theme was selected, so malformed-theme fallback cannot masquerade
as an expired reset. Resource and power checks continue independently; reset recalculation and nonnegative-countdown assertions are
not claimed unless the positive fixture was actually rendered. A controlled live
provider fixture is needed to close this coverage gap. Sampling uses a monotonic stopwatch.
Dashboard-cycle failures stay failed while the case attempts the remaining cycles
and independent idle/power checks, provided the original monitor remains healthy.
`dashboard-cycle-coverage.json` records normal completions, failed cycles and any
forced dashboard cleanup. If a cycle fails, normal-teardown memory/handle/thread
plateau assertions are not claimed; the later idle plateau is measured separately.
The test disables polling at launch to retain that fixture and uses a one-hour
poll interval. Idle CPU must remain below 1% of one core; early/late ten-minute
averages allow at most 4 MiB private-memory, five handles and one thread of growth.

## Host actions and reboot continuation

`hostActions` is an optional catalogue array. Allowed values are `reboot`,
`network-disconnect`, `network-connect`, `pause-resume`, `clock-forward`,
`audit-start`, `audit-stop`, and `display-modes`. The host validates each request's run ID,
scenario ID, request ID and per-case allowlist before doing anything. Requests
cannot contain shell commands. PowerShell Direct remains available when the VM's
virtual network adapters are disconnected.
The pause hook closes its control connection before suspension and creates a new
PowerShell Direct session after resuming the same VM; the interactive scenario
task continues independently. This avoids reusing a transport broken by the pause.

Reboot saves the case phase and completed checks atomically, adds a logon trigger
to the existing guest task, and resumes the same case after sign-in. The host
uses the supplied disposable-lab credential for one-time automatic sign-in,
removes the temporary Winlogon password after Explorer starts, and restores the
baseline checkpoint on completion or failure. Credentials never enter request
JSON, transcripts, or evidence. The clean-process/settings checks apply only to
the initial phase; resumed phases must assert the expected startup process.

Video configuration, adapter connections and previously enabled time-synchronization services are
restored explicitly because they are host settings. Checkpoint restoration is
attempted even if that cleanup fails, and any cleanup failure stops the suite.
WFP auditing is enabled only inside the guest and is reset with the checkpoint.
The clock hook disables Hyper-V time synchronization before advancing guest time.
