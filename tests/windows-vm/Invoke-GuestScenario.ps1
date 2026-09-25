#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# This script changes taskbar settings and installs software. Refuse host execution.
if ($Root -notmatch '^C:\\CCUM-Lab\\[a-f0-9]{32}$' -or
    -not (Test-Path -LiteralPath 'C:\CCUM-Lab\guest-enabled') -or
    (Get-CimInstance Win32_ComputerSystem).Model -ne 'Virtual Machine') {
    throw 'Run only inside an explicitly prepared disposable Hyper-V guest.'
}
$request = Get-Content -LiteralPath "$Root\request.json" -Raw | ConvertFrom-Json
$evidence = "$Root\evidence"
$result = [ordered]@{ runId = $request.runId; id = $request.id; status = 'failed'; error = $null; checks = @(); startedUtc = [DateTime]::UtcNow.ToString('o') }
$script:checks = [Collections.Generic.List[object]]::new()
$sessionId = (Get-Process -Id $PID).SessionId
$appName = 'claude-code-usage-monitor'
$packageId = 'CodeZeno.ClaudeCodeUsageMonitor'
$settingsPath = "$env:APPDATA\ClaudeCodeUsageMonitor\settings.json"
$transcribing = $false

function Assert-Check([string]$Name, [bool]$Condition, $Detail) {
    $script:checks.Add([pscustomobject]@{ name = $Name; passed = $Condition; detail = $Detail })
    # Keep the last completed assertion available even if a desktop API stalls
    # and the host must time out the scheduled task before its finally block.
    @{ name = $Name; passed = $Condition; atUtc = [DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath "$evidence\progress.json" -Encoding UTF8
    if (-not $Condition) { throw "Assertion failed: $Name ($Detail)" }
}
function Get-AppProcesses {
    @(Get-Process -Name $appName -ErrorAction SilentlyContinue | Where-Object SessionId -EQ $sessionId)
}
function Stop-App {
    foreach ($process in (Get-AppProcesses)) {
        Stop-Process -Id $process.Id -Force
        Wait-Process -Id $process.Id -Timeout 15 -ErrorAction SilentlyContinue
    }
}
function Save-Screenshot([string]$Name) {
    if (-not [CCUMLabDesktop]::IsDefaultDesktop()) { throw 'The input desktop is locked or unavailable.' }
    $bounds = [Windows.Forms.SystemInformation]::VirtualScreen
    $bitmap = [Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
        $bitmap.Save("$evidence\$Name.png", [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function Assert-App([string]$Executable, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(40)
    do {
        $processes = @(Get-AppProcesses)
        $windows = @()
        if ($processes.Count -eq 1) { $windows = @([CCUMLabDesktop]::WindowsForProcess($processes[0].Id)) }
        $visible = @($windows | Where-Object { $_.Visible -and $_.Right -gt $_.Left -and $_.Bottom -gt $_.Top })
        if ($visible.Count -gt 0) { break }
        Start-Sleep -Milliseconds 500
    } until ([DateTime]::UtcNow -gt $deadline)
    Assert-Check "$Label single instance" ($processes.Count -eq 1) $processes.Count
    Assert-Check "$Label executable path" ($processes[0].Path -eq $Executable) $processes[0].Path
    Assert-Check "$Label visible widget" ($visible.Count -gt 0) $windows
    # Auto-hide may put the widget off-screen by design. Record, do not mislabel it as clipping.
    if ($request.taskbar -ne 'auto-hide') {
        $screen = [Windows.Forms.SystemInformation]::VirtualScreen
        $intersecting = @($visible | Where-Object { $_.Right -gt $screen.Left -and $_.Left -lt $screen.Right -and $_.Bottom -gt $screen.Top -and $_.Top -lt $screen.Bottom })
        Assert-Check "$Label intersects display" ($intersecting.Count -gt 0) $visible
    }
    ConvertTo-Json -InputObject $windows -Depth 5 | Set-Content -LiteralPath "$evidence\$Label-windows.json" -Encoding UTF8
    Save-Screenshot $Label
}
function Assert-Settings {
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    Assert-Check 'language preserved' ($settings.language -eq 'de') $settings.language
    Assert-Check 'poll interval preserved' ($settings.poll_interval_ms -eq 900000) $settings.poll_interval_ms
}
function Invoke-WinGet([string[]]$Arguments) {
    $output = & winget @Arguments 2>&1
    $code = $LASTEXITCODE
    Add-Content -LiteralPath "$evidence\winget.log" -Value (($Arguments -join ' ') + "`r`n" + ($output -join "`r`n") + "`r`nExit: $code")
    Assert-Check ('winget ' + $Arguments[0]) ($code -eq 0) $code
}
function Find-WinGetExe {
    $packages = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages"
    $matches = @(Get-ChildItem -LiteralPath $packages -Directory | Where-Object Name -Like "$packageId`_*" |
        Get-ChildItem -Recurse -File -Filter "$appName.exe")
    if ($matches.Count -ne 1) { throw "Expected one per-user WinGet executable; found $($matches.Count)." }
    return $matches[0].FullName
}
function Assert-Version([string]$Executable, [string]$Expected) {
    $version = (Get-Item -LiteralPath $Executable).VersionInfo.ProductVersion
    Assert-Check 'installed product version' ($version -eq $Expected -or $version -eq "$Expected.0") $version
}

try {
    Start-Transcript -LiteralPath "$evidence\guest.log" | Out-Null
    $transcribing = $true
    Assert-Check 'interactive session' ($sessionId -gt 0) $sessionId
    $os = Get-CimInstance Win32_OperatingSystem
    $build = [int]$os.BuildNumber
    Assert-Check 'expected guest OS' (($request.os -eq 'windows10' -and $build -ge 19041 -and $build -lt 22000) -or ($request.os -eq 'windows11' -and $build -ge 22000)) $build
    $os | Select-Object Caption, Version, BuildNumber | ConvertTo-Json | Set-Content -LiteralPath "$evidence\os.json" -Encoding UTF8
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class CCUMLabDesktop {
    public class WindowInfo { public long Handle, Parent; public int Left, Top, Right, Bottom; public bool Visible; }
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct APPBARDATA { public uint cbSize; public IntPtr hWnd; public uint callback, edge; public RECT rect; public IntPtr param; }
    delegate bool EnumProc(IntPtr hwnd, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr hwnd, EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern IntPtr GetParent(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    public static WindowInfo Tray() {
        IntPtr h = FindWindowEx(FindWindow("Shell_TrayWnd", null), IntPtr.Zero, "TrayNotifyWnd", null);
        RECT r; if (h == IntPtr.Zero || !GetWindowRect(h, out r)) throw new Exception("Tray unavailable");
        return new WindowInfo { Handle=h.ToInt64(), Left=r.Left, Top=r.Top, Right=r.Right, Bottom=r.Bottom };
    }
    public static void Drag(WindowInfo window, int dx) {
        IntPtr h = new IntPtr(window.Handle), result;
        int x = window.Left + 15, y = (window.Top + window.Bottom) / 2;
        SetCursorPos(x, y);
        if (SendMessageTimeout(h, 0x201, new IntPtr(1), IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag down timed out");
        SetCursorPos(x + dx, Tray().Bottom - (window.Bottom - window.Top) / 2);
        if (SendMessageTimeout(h, 0x200, new IntPtr(1), IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag move timed out");
        if (SendMessageTimeout(h, 0x202, IntPtr.Zero, IntPtr.Zero, 2, 3000, out result) == IntPtr.Zero) throw new Exception("Drag release timed out");
        SetCursorPos(10, 10);
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h, int index, StringBuilder text, uint length, out uint needed);
    [DllImport("shell32.dll")] static extern UIntPtr SHAppBarMessage(uint message, ref APPBARDATA data);
    public static bool IsDefaultDesktop() {
        IntPtr h = OpenInputDesktop(0, false, 1);
        if (h == IntPtr.Zero) return false;
        try { uint needed; var name = new StringBuilder(256); return GetUserObjectInformation(h, 2, name, 512, out needed) && name.ToString() == "Default"; }
        finally { CloseDesktop(h); }
    }
    public static ulong TaskbarState(bool autoHide) {
        var data = new APPBARDATA(); data.cbSize = (uint)Marshal.SizeOf(data); data.hWnd = FindWindow("Shell_TrayWnd", null);
        if (data.hWnd == IntPtr.Zero) throw new Exception("Explorer taskbar unavailable");
        ulong state = SHAppBarMessage(4, ref data).ToUInt64();
        data.param = new IntPtr((long)(autoHide ? state | 1UL : state & ~1UL));
        SHAppBarMessage(10, ref data);
        return SHAppBarMessage(4, ref data).ToUInt64();
    }
    public static WindowInfo[] WindowsForProcess(int pid) {
        var list = new List<WindowInfo>(); var seen = new HashSet<IntPtr>();
        EnumProc collect = delegate(IntPtr h, IntPtr p) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            var name = new StringBuilder(256); GetClassName(h, name, name.Capacity);
            RECT r;
            if (owner == pid && name.ToString() == "ClaudeCodeUsageMonitor" && seen.Add(h) && GetWindowRect(h, out r))
                list.Add(new WindowInfo { Handle=h.ToInt64(), Parent=GetParent(h).ToInt64(), Left=r.Left, Top=r.Top, Right=r.Right, Bottom=r.Bottom, Visible=IsWindowVisible(h) });
            return true;
        };
        EnumWindows(delegate(IntPtr h, IntPtr p) { collect(h,p); EnumChildWindows(h, collect, p); return true; }, IntPtr.Zero);
        return list.ToArray();
    }
}
'@
    Assert-Check 'unlocked input desktop' ([CCUMLabDesktop]::IsDefaultDesktop()) $sessionId
    Assert-Check 'clean app processes' (@(Get-AppProcesses).Count -eq 0) 'baseline must have no running monitor'
    Assert-Check 'clean settings' (-not (Test-Path -LiteralPath $settingsPath)) $settingsPath
    if ($request.taskbar -in @('left', 'center') -or ($request.flow -eq 'portable-taskbar-tray' -and $request.os -eq 'windows10')) {
        if ($request.taskbar -in @('left', 'center')) {
            Assert-Check 'alignment supported' ($request.os -eq 'windows11') $request.os
            $alignment = if ($request.taskbar -eq 'left') { 0 } else { 1 }
            $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'
            Set-ItemProperty -LiteralPath $key -Name TaskbarAl -Value $alignment -Type DWord
        } else {
            # Windows 10 reads this on Explorer startup. Make new icons visible.
            Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer' -Name EnableAutoTray -Value 0 -Type DWord
        }
        Get-Process explorer | Where-Object SessionId -EQ $sessionId | Stop-Process -Force
        Start-Sleep -Seconds 3
        if (-not @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object SessionId -EQ $sessionId).Count) {
            Start-Process explorer.exe
        }
        Start-Sleep -Seconds 5
        if ($request.taskbar -in @('left', 'center')) {
            Assert-Check 'alignment preference applied' ((Get-ItemProperty -LiteralPath $key).TaskbarAl -eq $alignment) $alignment
        }
    }
    $state = [CCUMLabDesktop]::TaskbarState($request.taskbar -eq 'auto-hide')
    Assert-Check 'expected auto-hide state' ((($state -band 1) -eq 1) -eq ($request.taskbar -eq 'auto-hide')) $state
    @{ taskbar = $request.taskbar; appbarState = $state; screen = [Windows.Forms.SystemInformation]::VirtualScreen.ToString() } |
        ConvertTo-Json | Set-Content -LiteralPath "$evidence\desktop.json" -Encoding UTF8
    $null = New-Item -ItemType Directory -Path (Split-Path -Parent $settingsPath) -Force
    '{"language":"de","poll_interval_ms":900000}' | Set-Content -LiteralPath $settingsPath -Encoding ASCII
    if ($request.flow -eq 'portable-taskbar-tray' -and $request.trayTheme -eq 'compact') {
        # Fit the native 40px Windows 10 bar as well as Windows 11's 48px bar.
        $themePath = "$Root\tray-fixture.json"
        @{
            schema_version=1; id='tray-fixture'; name='Tray regression fixture'
            surfaces=@(@{
                id='main'; name='Tray regression'; width='120'; height='32'
                background=@{type='colour'; colour=@{color='#2080D0FF'}}
                placement=@{reference=@{region='system_tray'; display=0}; nest='taskbar'; horizontal='left'; vertical='bottom'; surface_horizontal='right'; surface_vertical='bottom'; offset_x=-80}
            })
        } | ConvertTo-Json -Depth 8 | Set-Content $themePath -Encoding ASCII
        @{language='de'; poll_interval_ms=900000; custom_theme_enabled=$true; active_theme_path=$themePath} |
            ConvertTo-Json | Set-Content $settingsPath -Encoding ASCII
    }
    $executable = $null
    switch ($request.flow) {
        { $_ -in 'portable-launch', 'portable-dashboard-warp', 'portable-taskbar-tray', 'portable-update-helper' } {
            Assert-Check 'candidate transport hash' ((Get-FileHash -LiteralPath "$Root\candidate.exe").Hash -eq $request.candidateHash) $request.candidateHash
            $null = New-Item -ItemType Directory -Path "$Root\App With Spaces"
            $executable = "$Root\App With Spaces\$appName.exe"
            $initial = if ($request.flow -eq 'portable-update-helper') { "$Root\previous.exe" } else { "$Root\candidate.exe" }
            Copy-Item -LiteralPath $initial -Destination $executable
        }
        { $_ -in 'winget-install', 'winget-upgrade' } {
            $null = Get-Command winget -ErrorAction Stop
            $existing = & winget list --id $packageId --exact --source winget --disable-interactivity --accept-source-agreements 2>&1
            # APPINSTALLER_CLI_ERROR_NO_APPLICATIONS_FOUND. Other failures are not a clean baseline.
            Assert-Check 'package absent before install' ($LASTEXITCODE -eq -1978335212) ($existing -join "`n")
            $installVersion = if ($request.flow -eq 'winget-upgrade') { $request.previousVersion } else { $request.candidateVersion }
            Invoke-WinGet -Arguments @('install', '--id', $packageId, '--exact', '--version', $installVersion, '--source', 'winget', '--scope', 'user', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity')
            $executable = Find-WinGetExe
            Assert-Version $executable $installVersion
        }
        default { throw "Unsupported flow: $($request.flow)" }
    }
    $app = Start-Process -FilePath $executable -ArgumentList '--diagnose' -PassThru
    Assert-App $executable 'initial'
    Assert-Settings
    Copy-Item -LiteralPath "$env:TEMP\claude-code-usage-monitor.log" -Destination "$evidence\initial-diagnostics.log"
    if ($request.flow -eq 'portable-taskbar-tray') {
        . "$Root\Test-TaskbarTray.ps1"
        Test-TaskbarTray
    }
    if ($request.flow -eq 'portable-dashboard-warp') {
        . "$Root\Test-DashboardWarp.ps1"
        Test-DashboardWarp
    }
    if ($request.flow -eq 'portable-update-helper') {
        # Tests the real replacement/relaunch helper, not the release check/download/UI.
        Copy-Item -LiteralPath "$Root\previous.exe" -Destination "$Root\updater-helper.exe"
        $size = (Get-Item -LiteralPath "$Root\candidate.exe").Length
        $helperArgs = '--apply-update "{0}" "{1}" {2} {3} sha256:{4}' -f $executable, "$Root\candidate.exe", $app.Id, $size, $request.candidateHash.ToLowerInvariant()
        $helper = Start-Process -FilePath "$Root\updater-helper.exe" -ArgumentList $helperArgs -PassThru
        Stop-App
        if (-not $helper.WaitForExit(90000)) { throw 'Updater helper timed out.' }
        Assert-Check 'helper exit' ($helper.ExitCode -eq 0) $helper.ExitCode
        Assert-Check 'candidate replaced executable' ((Get-FileHash -LiteralPath $executable).Hash -eq $request.candidateHash) $request.candidateHash
        Assert-App $executable 'updated'
        Assert-Settings
    } elseif ($request.flow -eq 'winget-upgrade') {
        Stop-App
        Invoke-WinGet -Arguments @('upgrade', '--id', $packageId, '--exact', '--version', $request.candidateVersion, '--source', 'winget', '--scope', 'user', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity')
        $executable = Find-WinGetExe
        Assert-Version $executable $request.candidateVersion
        $null = Start-Process -FilePath $executable -ArgumentList '--diagnose-append', '--diagnose' -PassThru
        Assert-App $executable 'updated'
        Assert-Settings
    }
    Stop-App
    $null = Start-Process -FilePath $executable -ArgumentList '--diagnose-append', '--diagnose' -PassThru
    Assert-App $executable 'restarted'
    Assert-Settings
    @{ path = $executable; sha256 = (Get-FileHash -LiteralPath $executable).Hash; productVersion = (Get-Item -LiteralPath $executable).VersionInfo.ProductVersion } |
        ConvertTo-Json | Set-Content -LiteralPath "$evidence\executable.json" -Encoding UTF8
    if ($request.flow -like 'winget-*') {
        Invoke-WinGet -Arguments @('list', '--id', $packageId, '--exact', '--source', 'winget', '--disable-interactivity', '--accept-source-agreements')
        Stop-App
        Invoke-WinGet -Arguments @('uninstall', '--id', $packageId, '--exact', '--source', 'winget', '--scope', 'user', '--disable-interactivity')
        Assert-Check 'uninstalled executable' (-not (Test-Path -LiteralPath $executable)) $executable
    }
    $result.status = 'passed'
} catch {
    $result.error = $_.Exception.Message
    try { Save-Screenshot 'failure' } catch { $result['screenshotError'] = $_.Exception.Message }
} finally {
    try { Stop-App } catch { $result.status = 'failed'; $result['cleanupError'] = $_.Exception.Message }
    foreach ($file in @($settingsPath, "$env:TEMP\claude-code-usage-monitor.log")) {
        if (Test-Path -LiteralPath $file) { Copy-Item -LiteralPath $file -Destination $evidence -Force -ErrorAction Continue }
    }
    $result.checks = @($script:checks.ToArray())
    $result['finishedUtc'] = [DateTime]::UtcNow.ToString('o')
    if ($transcribing) { Stop-Transcript | Out-Null }
    # Publish completion only after all evidence is flushed.
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$evidence\result.tmp" -Encoding UTF8
    Move-Item -LiteralPath "$evidence\result.tmp" -Destination "$evidence\result.json" -Force
}
if ($result.status -ne 'passed') { exit 1 }
