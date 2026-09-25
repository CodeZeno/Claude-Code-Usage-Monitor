# Dashboard rendering

The dashboard tries its existing eframe/OpenGL renderer first. If OpenGL context,
configuration or painter initialization fails, it starts the same executable with
`--studio --dashboard-warp`, retaining the owner window, initial page and diagnostic
logging. Application errors do not trigger a renderer retry.

The WARP dashboard uses D3D11 with `D3D_DRIVER_TYPE_WARP` and feature level 11.0.
Windows 10 and Windows 11 supply the CPU rasterizer. It requires no OpenGL driver,
DirectX SDK, downloadable runtime, shader compiler DLL or companion executable.
The two small shader-model-5 binaries are included with their source in
`vendor/egui-directx11/shaders`. The project continues to link the MSVC runtime
statically. `wgpu` and its backend/shader-translation dependencies are removed.

Winit permits only one event loop per process, and eframe owns that loop after
trying OpenGL. The failed dashboard therefore releases its named instance guard
and launches a fresh process, which claims the same guard. The original process
exits rather than waiting. Concurrent requests still pass through that guard;
the WARP process never retries OpenGL. If WARP initialization also fails, its
error dialog and diagnostic log include the original OpenGL error, passed in the
child's `CCUM_DASHBOARD_OPENGL_ERROR` environment variable.

Both renderers share the dashboard UI, fonts, theme preview, settings, native
titlebar styling and save-on-close logic. The WARP host uses the existing patched
egui-winit integration for keyboard, pointer, IME, clipboard and file-drop input.
It embeds secondary egui viewports in the main window. The application does not
use custom GPU paint callbacks; the vendored painter does not support those.
The painter adaptation and upstream attribution are recorded in
`vendor/egui-directx11/PATCH.md`.

## Testing

To force software rendering for diagnosis:

```powershell
.\target\release\claude-code-usage-monitor.exe --studio --dashboard-warp --diagnose
```

The log records the D3D11 WARP adapter and first successful presentation. Normally
omit `--dashboard-warp` to exercise OpenGL and automatic fallback. The software
path uses more CPU than hardware rendering; it schedules redraws when needed.

`cargo test` includes a real off-screen WARP readback test covering odd-width
partial texture updates, nearest sampling, zoom and deferred texture deletion,
plus fallback selection, original-error propagation and close cancellation tests.

The VM flow exercises stock Hyper-V graphics where OpenGL initialization fails:

```powershell
.\tests\windows-vm\Invoke-Lab.ps1 `
  -ConfigPath C:\work\CCUM-Lab\lab.runtime.json `
  -Credential (Import-Clixml C:\work\CCUM-Lab\credential.clixml) `
  -CandidateExe .\target\release\claude-code-usage-monitor.exe `
  -Flows portable-dashboard-warp -Taskbars baseline
```

It restores only the configured disposable VM checkpoints before and after each
scenario. Evidence includes renderer logs, OS/build and binary identity,
assertions, and screenshots of settings and the theme studio. See the dated
WARP results document in `tests/windows-vm` for the tested build and limitations.
