# Local full-trust MSIX trial

This directory is for local sideload compatibility testing only. It does not publish or submit anything.

The package uses the current release feature set explicitly:

```powershell
cargo build --release --locked --no-default-features --features antigravity
```

`self-update` is deliberately excluded. The package uses a `win32App` / `mediumIL` full-trust application and declares the `AIUsageMonitorStartup` StartupTask. The unpackaged build continues to use the existing HKCU Run entry.

Build and sign with a local-only test certificate from an elevated PowerShell session:

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\msix\build-msix.ps1 `
  -CreateAndTrustLocalTestCertificate
```

The script converts Cargo `major.minor.patch` into MSIX `major.minor.patch.revision`. Use `-Revision` for a locally simulated package update. All generated package files and the exported public certificate are written under `target\msix`, which is already ignored by Git. The private key remains only in `CurrentUser\My` and is never exported to the repository. The public `.cer` is trusted through `LocalMachine\TrustedPeople`; neither the current-user nor local-machine Root store is used. The machine-store import requires administrator rights and should only be used on a local test PC.

Install the generated package for the current user:

```powershell
Add-AppxPackage .\target\msix\AIUsageMonitor.LocalTrial_1.4.9.0_x64.msix
```

Remove the local trial package:

```powershell
Get-AppxPackage Ysawase.AIUsageMonitor.LocalTrial | Remove-AppxPackage
```

After local testing, remove the public certificate from `LocalMachine\TrustedPeople` by its recorded thumbprint in an elevated PowerShell session, remove the matching certificate/private key from `CurrentUser\My`, and delete `target\msix`. Verify the exact thumbprint before removal. No PFX is created by this workflow.

For a Store package, replace the local identity and publisher with Partner Center values, replace the local certificate flow, produce final Store assets, and run Windows App Certification Kit validation.
