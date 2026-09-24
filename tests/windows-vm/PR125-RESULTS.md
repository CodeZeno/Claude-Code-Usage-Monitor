# PR #125 VM verification — 2026-09-24

Comparison: local `v2.14.63` (`9d17a44`) against `v2.14.64`, including
PR #125's original commits and two review fixes: retain collision detection for
app controls partially overlapping the notification area, and apply the tray
clamp only to docked surfaces.

Result: the tray-induced position jump was reproduced on both OS versions, and
the jump followed by undocking was reproduced with Classic on Windows 11. All
five candidate VM scenarios passed; the widget stayed docked and stationary
during tray growth and returned to the same position after restart.

Executable SHA-256:

- Baseline: `15F644678937078EE0546C93EDBB1559DF7A944DE1D47E9E909D083C9FB5772B`
- Candidate: `6F0C64B67A5FB01D45720CC5F36FBEE4A5274B37E286314B3E88C00F70F8B077`

## Environment and method

- Disposable Hyper-V guests `CCUM-Win10` (build 19045) and `CCUM-Win11`
  (build 26200), restored from `ccum-runtime-desktop` before and after each case.
- 1920 x 1080 desktop at 100% scaling; real Explorer taskbar and notification icons.
- `portable-taskbar-tray` sends drag messages to the real widget window procedure,
  adds two notification icons, samples window parenting/geometry every 100 ms,
  removes the icons, and checks restoration. A run fails if tray growth is absent.
- The compact fixture is 120 x 32 pixels so it fits Windows 10's 40-pixel taskbar.
  The default Classic theme is 46 pixels high and cannot redock on that stock bar;
  this existing size constraint is separate from PR #125.
- Windows 10 uses `EnableAutoTray=0` followed by an Explorer restart; Windows 11
  promotes only the fixture's notification icons with `IsPromoted=1`.

## Evidence

Local raw evidence is under the ignored `artifacts/` directory. Each run includes
assertions, screenshots, settings, diagnostics, OS build and geometry samples.

- `2d94de614f91467ca733cbda48633a68`: baseline Windows 11 Classic theme reproduced
  the reported sequence. The tray grew 64 px (left edge 1618 to 1554), the widget
  jumped from x=1281 to x=1217, then undocked (parent window became zero). It returned
  after the icons were removed. The Windows 10 case in this exploratory run failed
  the default theme's height prerequisite, so it is not tray-regression evidence.
- `73c206f3268f4e57a4a8f96525e85970`: baseline Windows 11 compact fixture reproduced
  the jump from x=1338 to x=1274 with 64 px tray growth, despite free space. Windows 10
  did not grow the tray before the fixture's Explorer-restart setup was added; that
  case is inconclusive, not a pass.
- `52f76d2df6a34e5fb5e6c06622ae17cf`: all four candidate compact-fixture scenarios
  passed, including restart and retained settings; checkpoint cleanup succeeded.
- `1d2e49d226be4c4190d7c412ac246a5a`: baseline Windows 10 with the final fixture setup
  reproduced a 48 px jump (x=1313 to x=1265) when the tray grew from left=1593 to
  left=1545, despite free space. The matched candidate stayed at x=1313.
- `9f4b08035fe646289e6df6c773c9f3a1`: candidate Windows 11 Classic passed the same
  configuration that reproduced jumping/undocking in the baseline. It stayed at
  x=1281 with 64 px tray growth, never undocked, and retained its position on restart.

| Candidate case | Tray growth | Widget movement | Undocking | Restart |
| --- | --- | --- | --- | --- |
| Windows 10 baseline | 48 px | 0 px (x=1313) | None | Same position |
| Windows 11 baseline | 64 px | 0 px (x=1338) | None | Same position |
| Windows 11 left alignment | 64 px | 0 px (x=1338) | None | Same position |
| Windows 11 center alignment | 64 px | 0 px (x=1338) | None | Same position |
| Windows 11 Classic | 64 px | 0 px (x=1281) | None | Same position |

## Local checks

`cargo build --release`, `cargo clippy`, and `cargo fmt --check` passed.
`cargo test`: 422 passed, zero failed, three ignored. The build emits its existing
font-subsetting informational warnings. `Test-Harness.ps1`: all 15 checks passed.
Regression tests cover stationary placement,
clamping/restoration, tray snapping, vertical placement, fractional DPI, narrow bars,
partially overlapping app controls and floating surfaces.

The VM cases exercise visible horizontal taskbars. They do not establish coverage
for vertical taskbars, multiple monitors, DPI transitions, auto-hide animation,
or insufficient-space clamping; those placement calculations have Rust coverage.
