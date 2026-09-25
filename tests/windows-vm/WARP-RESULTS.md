# OpenGL / WARP verification — 2026-09-25

Evidence was collected on `feature/opengl-warp-fallback`, based on local `main`
at `2d47a74`, using package version `2.15.17`. The implementation is incorporated
into the revised `v2.15.16` follow-up commit on `main`, preserving the original
PR #141 author commit and the subsequent `v2.15.17` segment-scaling changes.
The executable identity below describes the tested build before that history update.

## Result

The dashboard retains OpenGL as its primary renderer and now falls back to native
D3D11 WARP without wgpu. Both Hyper-V guests encountered a real OpenGL 2.0
initialization failure and automatically opened a working WARP dashboard using
Microsoft Basic Render Driver. Forced WARP startup was tested separately.

The release executable is **7,440,896 bytes** (7.44 MB), compared with
10,235,392 bytes (10.24 MB) before the change: **2,794,496 bytes smaller**.
The prior OpenGL-only control was 7,408,640 bytes, so this fallback adds
32,256 bytes. All sizes use the repository's normal release profile and output.

Tested executable SHA-256:
`3131880C1F2C80C3D48AFF72A654C64B10D278297C1A8B0B28FB39DC1A001A67`.

## Final VM results

| Guest | OS build | Assertions | Automatic fallback | Forced WARP | Idle CPU seconds / 3 seconds (automatic / forced) |
| --- | --- | --- | --- | --- | --- |
| CCUM-Win10 | 19045 | 54 passed | Passed | Passed | 0.141 / 0.281 |
| CCUM-Win11 | 26200 | 54 passed | Passed | Passed | 0.219 / 0.203 |

Both ran at 1920 × 1080, 100% display scaling, using the prepared
`ccum-runtime-desktop` checkpoint. The scenarios verified:

- Transport hash, executable identity, retained owner window, WARP adapter and
  first successful presentation after an actual OpenGL failure.
- Settings and theme-studio screenshots, including the asynchronously rendered
  theme preview. Screenshots were visually inspected on both OS versions.
- Resize, minimize/restore, second-launch focus with one dashboard, graceful
  close (including while minimized), reopening, and persisted window dimensions.
- Pointer navigation, keyboard editing of the poll interval, clipboard copy and
  paste, and preservation of language and settings after monitor restart.
- An idle CPU check that rejects a continuous rendering loop.

Raw local evidence is in the ignored `artifacts/` directory:

- Windows 10: `c7b5b866500d4e6b81077b3121909828/CCUM-Win10-portable-dashboard-warp-baseline/evidence`.
- Windows 11: `e2744eb49c314b2aa3e4f69f8f59c38c/CCUM-Win11-portable-dashboard-warp-baseline/evidence`.

The first expanded run exposed a redraw event scheduling another redraw. This
was fixed in the host; the final idle checks above exercise that fix. One later
Windows 11 run exceeded a 240-second harness timeout and could not collect its
open transcript. It was not counted as a pass. The final Windows 11 run used the
harness's normal 600-second allowance and passed with checkpoint cleanup.

Both guests were restored to their baseline checkpoints and returned to their
original Saved power state. Other VMs were not modified.

## Local checks and coverage limits

- `cargo build --release`, `cargo clippy`, `cargo fmt --check`: passed.
- `cargo test`: 462 passed, 3 ignored, no failures. Includes a real off-screen
  WARP texture readback test for partial updates, sampling, 2× zoom and texture
  deletion, plus fallback/error and close-cancellation regression coverage.
- `Test-Harness.ps1`: all 15 checks passed.
- Host OpenGL smoke test: responsive dashboard opened without fallback and
  closed normally, using isolated APPDATA/TEMP directories under
  `scratch/host-opengl-fc30b1e44c3e4339a189c4b76f10c4ab`.
- Binary import inspection: Windows system DLLs only; no DXC, D3DCompiler,
  VC++ runtime DLL or bundled graphics-runtime DLL is required by this change.

These VM runs do not establish coverage for mixed-DPI/multiple-monitor moves,
all older Windows builds, IME composition, or every image format. The software
renderer uses more CPU than hardware OpenGL. Custom GPU callbacks are not
supported by the native painter; the current dashboard does not use them.
