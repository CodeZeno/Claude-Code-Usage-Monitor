# Invoked only by the guarded guest runner.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Guest.Helpers.ps1" -Context $Context
function Test-DashboardWarp {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class CCUMWarpDesktop {
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort key, scan; public uint flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int x, y; public uint data, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct INPUTUNION {
        [FieldOffset(0)] public KEYBDINPUT keyboard;
        [FieldOffset(0)] public MOUSEINPUT mouse;
    }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public INPUTUNION data; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    static INPUT KeyEvent(ushort key, ushort scan, uint flags) {
        return new INPUT { type=1, data=new INPUTUNION { keyboard=new KEYBDINPUT { key=key, scan=scan, flags=flags } } };
    }
    static void Send(INPUT[] inputs) {
        if (SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT))) != inputs.Length)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Keyboard injection failed");
    }
    public static void Shortcut(ushort key) {
        Send(new[] { KeyEvent(0x11,0,0), KeyEvent(key,0,0), KeyEvent(key,0,2), KeyEvent(0x11,0,2) });
    }
    public static void Press(ushort key) { Send(new[] { KeyEvent(key,0,0), KeyEvent(key,0,2) }); }
    public static void Text(string text) {
        foreach (char c in text) Send(new[] { KeyEvent(0,c,4), KeyEvent(0,c,6) });
    }
    delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr p);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder text, int count);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left,Top,Right,Bottom; }
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    public static void Activate(IntPtr h) {
        SetForegroundWindow(h);
        if (GetForegroundWindow()==h) return;
        RECT r;
        if (!GetWindowRect(h,out r) || !SetCursorPos(r.Left+100,r.Top+12))
            throw new Exception("Cannot locate dashboard title bar");
        // Real pointer input gives focus when Windows refuses a background
        // process's SetForegroundWindow request. Posted clicks do not activate.
        Send(new[] {
            new INPUT { type=0, data=new INPUTUNION { mouse=new MOUSEINPUT { flags=2 } } },
            new INPUT { type=0, data=new INPUTUNION { mouse=new MOUSEINPUT { flags=4 } } }
        });
    }
    public static int Width(IntPtr h) { RECT r; GetClientRect(h,out r); return r.Right; }
    public static void Click(IntPtr h, int x, int y) {
        var point=new IntPtr((y << 16) | x);
        PostMessage(h,0x0200,IntPtr.Zero,point);
        PostMessage(h,0x0201,new IntPtr(1),point);
        PostMessage(h,0x0202,IntPtr.Zero,point);
    }
    public static IntPtr Find() {
        IntPtr found=IntPtr.Zero;
        EnumWindows((h,p) => { var s=new StringBuilder(256); GetWindowText(h,s,256);
            if (IsWindowVisible(h) && s.ToString()=="Usage Monitor") { found=h; return false; } return true; },IntPtr.Zero);
        return found;
    }
}
'@
    $log = "$env:TEMP\claude-code-usage-monitor.log"
    $owner = @([CCUMLabDesktop]::WindowsForProcess($app.Id) | Where-Object Visible)[0].Handle
    foreach ($mode in @('automatic', 'explicit')) {
        $startOffset = (Get-Content -LiteralPath $log -Raw).Length
        $arguments = "--studio --owner $owner --diagnose --diagnose-append"
        if ($mode -eq 'explicit') { $arguments += ' --dashboard-warp --theme-studio' }
        $null = Start-Process -FilePath $executable -ArgumentList $arguments -PassThru
        $deadline = [DateTime]::UtcNow.AddSeconds(45)
        do {
            Start-Sleep -Milliseconds 500
            $handle = [CCUMWarpDesktop]::Find()
            $newLog = (Get-Content -LiteralPath $log -Raw).Substring($startOffset)
        } until (($handle -ne [IntPtr]::Zero -and $newLog.Contains('dashboard WARP first frame presented')) -or [DateTime]::UtcNow -gt $deadline)
        Assert-Check "$mode WARP first frame" ($newLog.Contains('dashboard WARP first frame presented')) $newLog
        Assert-Check "$mode software adapter" ($newLog -match 'renderer=D3D11 WARP adapter=Microsoft Basic Render') $newLog
        Assert-Check "$mode visible dashboard" ($handle -ne [IntPtr]::Zero) $handle.ToInt64()
        Assert-Check "$mode owner retained" ($newLog.Contains("dashboard started owner=$owner")) $owner
        if ($mode -eq 'automatic') {
            Assert-Check 'OpenGL failure triggers WARP automatically' ($newLog.Contains('dashboard OpenGL initialization failed:')) $newLog
        }
        [uint32]$dashboardPid = 0
        $null = [CCUMWarpDesktop]::GetWindowThreadProcessId($handle, [ref]$dashboardPid)
        $dashboard = Get-Process -Id $dashboardPid
        Assert-Check "$mode candidate dashboard process" ($dashboard.Path -eq $executable) $dashboard.Path
        $null = [CCUMWarpDesktop]::SetForegroundWindow($handle)
        Start-Sleep -Seconds 2
        Save-Screenshot "warp-$mode-initial"
        $null = [CCUMWarpDesktop]::SetWindowPos($handle, [IntPtr]::Zero, 80, 80, 1100, 740, 0x0040)
        Start-Sleep -Seconds 2
        Save-Screenshot "warp-$mode-resized"
        # Navigate with pointer events and edit an existing numeric field with the
        # keyboard and clipboard. Assert persisted values, not merely process survival.
        $focusDeadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            [CCUMWarpDesktop]::Activate($handle)
            Start-Sleep -Milliseconds 200
        } until ([CCUMWarpDesktop]::GetForegroundWindow() -eq $handle -or [DateTime]::UtcNow -gt $focusDeadline)
        Assert-Check "$mode keyboard target foreground" ([CCUMWarpDesktop]::GetForegroundWindow() -eq $handle) @{expected=$handle.ToInt64(); actual=[CCUMWarpDesktop]::GetForegroundWindow().ToInt64()}
        [CCUMWarpDesktop]::Click($handle, 80, 40)
        Start-Sleep -Seconds 1
        [CCUMWarpDesktop]::Click($handle, ([CCUMWarpDesktop]::Width($handle) - 168), 100)
        Start-Sleep -Milliseconds 300
        # Inject without SendWait's cross-process journal-hook waits. Separate
        # selection, text and commit, then observe the persisted result.
        # https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-sendinput
        [CCUMWarpDesktop]::Shortcut(0x41)
        Start-Sleep -Milliseconds 300
        [CCUMWarpDesktop]::Text('16')
        Start-Sleep -Milliseconds 300
        [CCUMWarpDesktop]::Press(0x09)
        $editDeadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            Start-Sleep -Milliseconds 200
            $edited = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
        } until ($edited.poll_interval_ms -eq 960000 -or [DateTime]::UtcNow -gt $editDeadline)
        Assert-Check "$mode mouse and keyboard edit" ($edited.poll_interval_ms -eq 960000) $edited.poll_interval_ms
        [CCUMWarpDesktop]::Click($handle, ([CCUMWarpDesktop]::Width($handle) - 168), 100)
        Start-Sleep -Milliseconds 300
        [CCUMWarpDesktop]::Shortcut(0x41)
        Start-Sleep -Milliseconds 300
        [Windows.Forms.Clipboard]::Clear()
        [CCUMWarpDesktop]::Shortcut(0x43)
        $clipboardDeadline = [DateTime]::UtcNow.AddSeconds(5)
        $clipboardText = $null
        do {
            try { $clipboardText = [Windows.Forms.Clipboard]::GetText() }
            catch {
                # The dashboard may still own the clipboard while handling copy.
                if ($_.Exception.InnerException -isnot [Runtime.InteropServices.ExternalException]) { throw }
            }
            if ($clipboardText -eq '16') { break }
            Start-Sleep -Milliseconds 200
        } until ([DateTime]::UtcNow -gt $clipboardDeadline)
        Assert-Check "$mode clipboard copy" ($clipboardText -eq '16') $clipboardText
        [Windows.Forms.Clipboard]::SetText('15')
        [CCUMWarpDesktop]::Shortcut(0x56)
        Start-Sleep -Milliseconds 300
        [CCUMWarpDesktop]::Press(0x09)
        $editDeadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            Start-Sleep -Milliseconds 200
            $edited = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
        } until ($edited.poll_interval_ms -eq 900000 -or [DateTime]::UtcNow -gt $editDeadline)
        Assert-Settings
        Save-Screenshot "warp-$mode-input"
        $beforeCpu = (Get-Process -Id $dashboardPid).CPU
        Start-Sleep -Seconds 3
        $cpuSeconds = (Get-Process -Id $dashboardPid).CPU - $beforeCpu
        Assert-Check "$mode no idle render loop" ($cpuSeconds -lt 1.5) $cpuSeconds
        # A second launch must focus this dashboard, never create another one.
        $second = Start-Process -FilePath $executable -ArgumentList '--studio --dashboard-warp' -PassThru
        Assert-Check "$mode second launch exits" ($second.WaitForExit(10000)) $second.Id
        Assert-Check "$mode single dashboard retained" ([CCUMWarpDesktop]::Find() -eq $handle) $handle.ToInt64()
        $null = [CCUMWarpDesktop]::ShowWindow($handle, 6)
        Start-Sleep -Seconds 1
        $null = [CCUMWarpDesktop]::ShowWindow($handle, 9)
        Start-Sleep -Seconds 2
        Assert-Check "$mode responsive after restore" ((Get-Process -Id $dashboardPid).Responding) $dashboardPid
        Save-Screenshot "warp-$mode-restored"
        if ($mode -eq 'explicit') { $null = [CCUMWarpDesktop]::ShowWindow($handle, 6) }
        $null = [CCUMWarpDesktop]::PostMessage($handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Assert-Check "$mode graceful close" ($dashboard.WaitForExit(15000)) $dashboardPid
        $saved = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
        Assert-Check "$mode window size saved" ($saved.dashboard_width -gt 900 -and $saved.dashboard_width -lt 1100 -and $saved.dashboard_height -gt 600) $saved
        Assert-Settings
    }
}

Initialize-TestSettings
$executable = Install-PortableApp
$app = Start-TestApp $executable
Test-DashboardWarp
Test-AppRestart $executable
