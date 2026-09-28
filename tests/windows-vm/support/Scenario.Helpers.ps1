param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Guest.Helpers.ps1" -Context $Context
if (-not ('CCUMScenarioDesktop' -as [type])) { Add-Type -Path "$Root\support\ScenarioDesktop.cs" -ReferencedAssemblies System.Drawing }
[CCUMScenarioDesktop]::UsePhysicalPixels()
$logPath = "$env:TEMP\claude-code-usage-monitor.log"

function Wait-Condition([scriptblock]$Condition, [int]$Seconds = 40) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 250
    } while ($watch.Elapsed.TotalSeconds -lt $Seconds)
    return $false
}
function Write-Settings($Settings) {
    $null = New-Item -ItemType Directory -Path (Split-Path $settingsPath) -Force
    [IO.File]::WriteAllText($settingsPath, ($Settings | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($false))
}
function Get-Widget {
    $windows = @(Get-AppProcesses | ForEach-Object { [CCUMLabDesktop]::WindowsForProcess($_.Id) } | Where-Object Visible)
    if ($windows.Count -ne 1) { throw "Expected one widget, found $($windows.Count)." }
    return $windows[0]
}
function Send-WidgetMessage([uint32]$Message) { [CCUMScenarioDesktop]::Post((Get-Widget).Handle, $Message, 0, 0) }
function Assert-NoPanic([string]$Label) {
    Assert-Check "$Label diagnostics exist" (Test-Path $logPath) $logPath
    $log = Get-Content $logPath -Raw
    Assert-Check "$Label no panic" ($log -notmatch '(?im)panicked at|fatal runtime error|stack overflow') 'diagnostic log'
}
function Open-Dashboard([string]$Executable, [string]$Label) {
    # Launch directly. Shell activation can block behind Explorer and extend
    # a dashboard cycle beyond the readiness timeout before it even starts.
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Executable
    $info.Arguments = '--dashboard --diagnose --diagnose-append'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $launched = [Diagnostics.Process]::Start($info)
    $launched.Dispose()
    $ready = Wait-Condition { [CCUMScenarioDesktop]::Dashboard($Executable) -ne 0 }
    Assert-Check "$Label dashboard visible" $ready $Executable
    Start-Sleep -Seconds 2
    return [CCUMScenarioDesktop]::Dashboard($Executable)
}
function Close-Dashboard([long]$Handle) {
    [CCUMScenarioDesktop]::Post($Handle, 0x10, 0, 0)
    Assert-Check 'dashboard closes' (Wait-Condition { -not [CCUMScenarioDesktop]::Exists($Handle) } 15) $Handle
}
function Assert-Docked([string]$Label) {
    $w = Get-Widget
    $bar = [CCUMScenarioDesktop]::Taskbar()
    Assert-Check "$Label docked" ($w.Parent -ne 0 -and [CCUMScenarioDesktop]::IsTaskbarChild($w.Handle)) $w
    # A hidden appbar intentionally sits partly outside the monitor.
    if ($request.taskbar -ne 'auto-hide') {
        Assert-Check "$Label inside taskbar" ($w.Left -ge $bar.Left -and $w.Right -le $bar.Right -and $w.Top -ge $bar.Top -and $w.Bottom -le $bar.Bottom) @{widget=$w; taskbar=$bar}
    }
}
function Assert-Tray([string]$Label) {
    Assert-Check "$Label tray registered" (Wait-Condition { (Get-RegisteredTrayIcon) -ge 0 } 20) 'Shell_NotifyIconGetRect for active theme roots'
}
function Get-RegisteredTrayIcon {
    $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
    $ids = @(1)
    if ($settings.PSObject.Properties['active_theme_path'] -and (Test-Path $settings.active_theme_path)) {
        $theme = Get-Content $settings.active_theme_path -Raw | ConvertFrom-Json
        $ids = @(for ($index=0; $index -lt $theme.surfaces.Count; $index++) {
            if ($theme.surfaces[$index].placement.nest -eq 'tray_icon') { 1000 + $index }
        })
        if (-not $ids.Count) { $ids = @(1) }
    }
    $w = Get-Widget
    foreach ($id in $ids) { if ([CCUMScenarioDesktop]::TrayStatus($w.Handle, $id) -ge 0) { return $id } }
    return -1
}
function Show-TestTrayIcon([uint32]$IconId) {
    # Windows 11 may not materialize a hidden icon's HWND/rectangle until its
    # overflow flyout has opened. Query the live shell instead of calling absence
    # from the main taskbar a missing tray registration.
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)
    $taskbarElement = [Windows.Automation.AutomationElement]::FromHandle([CCUMScenarioDesktop]::TaskbarHandle())
    $buttons = $taskbarElement.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
    foreach ($button in $buttons) {
        if (-not $button.Current.IsOffscreen -and $button.Current.Name -match '^(Hidden icon menu|Show hidden icons)$') {
            $button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
            Start-Sleep -Seconds 1
            return
        }
    }
    @($buttons | ForEach-Object { @{name=$_.Current.Name; id=$_.Current.AutomationId; offscreen=$_.Current.IsOffscreen} }) |
        ConvertTo-Json | Set-Content "$evidence\tray-buttons.json" -Encoding UTF8
}
function Get-TrayTooltip([string]$ExpectedPattern = '.') {
    $tooltip = ''
    foreach ($attempt in 1..3) {
        try { $tooltip = Read-TrayTooltipOnce }
        catch [Windows.Automation.ElementNotAvailableException] { $tooltip = '' }
        if ($tooltip -match $ExpectedPattern) { return $tooltip }
        if ($attempt -lt 3) { Start-Sleep -Seconds 1 }
    }
    return $tooltip
}
function Read-TrayTooltipOnce {
    # Read the live shell tooltip, not a persisted NotifyIconSettings registry entry.
    $w = Get-Widget
    $iconId = Get-RegisteredTrayIcon
    if ($iconId -lt 0) { throw 'No active theme tray icon is registered.' }
    try {
        Show-TestTrayIcon $iconId
        $bounds = [CCUMScenarioDesktop]::TrayBounds($w.Handle, $iconId)
        # A hidden icon's Shell_NotifyIconGetRect can identify the chevron,
        # even after the flyout opens. Resolve the live icon in that flyout.
        $shellPid = [Windows.Automation.AutomationElement]::FromHandle([CCUMScenarioDesktop]::TaskbarHandle()).Current.ProcessId
        $shellCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $shellPid)
        $roots = [Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children, $shellCondition)
        $overflowEvidence = [Collections.Generic.List[object]]::new()
        foreach ($popup in $roots) {
            $info = $popup.Current; $rect = $info.BoundingRectangle
            if ($info.IsOffscreen -or $info.ClassName -eq 'Shell_TrayWnd' -or $rect.Width -gt 800 -or $rect.Height -gt 600) { continue }
            $buttons = $popup.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
            foreach ($button in $buttons) {
                $item = $button.Current
                $overflowEvidence.Add(@{popup=$info.ClassName; name=$item.Name; type=$item.ControlType.ProgrammaticName; offscreen=$item.IsOffscreen; bounds=$item.BoundingRectangle.ToString()})
            }
            $icon = @($buttons | Where-Object { -not $_.Current.IsOffscreen -and $_.Current.Name -match '^Claude\s*Code' -and $_.Current.ControlType -in @([Windows.Automation.ControlType]::Button,[Windows.Automation.ControlType]::Image) } | Select-Object -First 1)
            if ($icon.Count) {
                $rect = $icon[0].Current.BoundingRectangle
                $bounds = [CCUMScenarioDesktop+Rect]@{Left=[int]$rect.Left; Top=[int]$rect.Top; Right=[int]$rect.Right; Bottom=[int]$rect.Bottom}
                break
            }
        }
        ConvertTo-Json -InputObject @($overflowEvidence.ToArray()) -Depth 4 | Set-Content "$evidence\tray-overflow-elements-$($Context.Checks.Count).json" -Encoding UTF8
        $bar = [CCUMScenarioDesktop]::Taskbar()
        if ($bounds.Top -ge $bar.Top -and $bounds.Bottom -le $bar.Bottom) { [CCUMScenarioDesktop]::DismissShellFlyout() }
        $bounds | ConvertTo-Json | Set-Content "$evidence\tray-tooltip-bounds.json" -Encoding UTF8
        [CCUMScenarioDesktop]::HoverPoint(($bounds.Left+$bounds.Right)/2, ($bounds.Top+$bounds.Bottom)/2)
        Start-Sleep -Seconds 1
        $tooltip = ''; $hoverWatch = [Diagnostics.Stopwatch]::StartNew()
        do {
            try { $tooltip = [string](Read-LiveTooltip $bounds) }
            catch [Windows.Automation.ElementNotAvailableException] { $tooltip = '' }
            if ($tooltip -match '^(Show hidden icons|Hidden icon menu)$') { $tooltip = '' }
            if ($tooltip) { break }
            Start-Sleep -Milliseconds 250
        } while ($hoverWatch.Elapsed.TotalSeconds -lt 8)
        Save-Screenshot "tray-tooltip-$($Context.Checks.Count)"
        # Preserve the observed text: shell tooltips can expire during capture.
        return $tooltip
    } finally { [CCUMScenarioDesktop]::DismissShellFlyout() }
}
function Read-LiveTooltip($Bounds) {
    $text = [CCUMScenarioDesktop]::Tooltip()
    if ($text) { return $text }
    # XAML exposes the current NotifyIcon tooltip as the icon's accessible name.
    # Query only that live icon; desktop-wide traversal can stall on unrelated UI.
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase
    $tipCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::ToolTip)
    $tips = @([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children, $tipCondition))
    $point = [Windows.Point]::new(($Bounds.Left+$Bounds.Right)/2, ($Bounds.Top+$Bounds.Bottom)/2)
    $element = [Windows.Automation.AutomationElement]::FromPoint($point)
    $shellPid = [Windows.Automation.AutomationElement]::FromHandle([CCUMScenarioDesktop]::TaskbarHandle()).Current.ProcessId
    # Some Win11 builds wrap ToolTip in an unlabelled top-level XAML pane.
    # Limit candidates to small visible shell popups immediately above this icon.
    $roots = [Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition)
    foreach ($popup in $roots) {
        $info = $popup.Current
        if ($info.IsOffscreen -or $info.ProcessId -ne $shellPid) { continue }
        $rect = $info.BoundingRectangle
        if ($rect.Width -gt 20 -and $rect.Width -lt 800 -and $rect.Height -gt 10 -and $rect.Height -lt 250 -and
            $rect.Bottom -le $Bounds.Top -and $rect.Bottom -ge ($Bounds.Top-150) -and $rect.Left -le $point.X -and $rect.Right -ge $point.X) {
            $tips += $popup
        }
    }
    if ($element -and $element.Current.ControlType -in @([Windows.Automation.ControlType]::Pane, [Windows.Automation.ControlType]::Window)) {
        # XAML tooltips can be children of the overflow pane despite rendering
        # outside its bounds. Search only this small shell subtree.
        $tips += @($element.FindAll([Windows.Automation.TreeScope]::Descendants, $tipCondition))
    }
    foreach ($tip in $tips) {
        if ($tip.Current.IsOffscreen) { continue }
        $textCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Text)
        $labels = @($tip.FindAll([Windows.Automation.TreeScope]::Descendants, $textCondition) | ForEach-Object { $_.Current.Name })
        $value = $labels -join ' '
        if ($value.Trim()) { return $value }
        if ($tip.Current.ControlType -eq [Windows.Automation.ControlType]::ToolTip -and $tip.Current.Name) { return $tip.Current.Name }
    }
    # A XAML popup may be omitted from the control tree but still support hit
    # testing. Its text is directly above the hovered notification-area icon.
    $above = [Windows.Automation.AutomationElement]::FromPoint([Windows.Point]::new($point.X, $Bounds.Top-30))
    if ($above -and -not $above.Current.IsOffscreen -and $above.Current.ProcessId -eq $shellPid -and
        $above.Current.ControlType -in @([Windows.Automation.ControlType]::Text, [Windows.Automation.ControlType]::ToolTip)) { return $above.Current.Name }
    if ($element -and -not $element.Current.IsOffscreen -and
        $element.Current.ControlType -in @([Windows.Automation.ControlType]::Button, [Windows.Automation.ControlType]::Image, [Windows.Automation.ControlType]::ListItem)) {
        return $element.Current.Name
    }
    return ''
}
function Assert-Image([string]$Label, [long]$Handle, [double]$MinimumContrast = 1.5) {
    $metrics = [CCUMScenarioDesktop]::Capture($Handle, "$evidence\$Label.png")
    if ($metrics.DistinctColors -lt 8 -or $metrics.ForegroundPixels -lt 20) {
        Copy-Item "$evidence\$Label.png" "$evidence\$Label-initial-blank.png" -Force
        $paintWatch = [Diagnostics.Stopwatch]::StartNew()
        do {
            Start-Sleep -Milliseconds 250
            $metrics = [CCUMScenarioDesktop]::Capture($Handle, "$evidence\$Label.png")
        } while (($metrics.DistinctColors -lt 8 -or $metrics.ForegroundPixels -lt 20) -and $paintWatch.Elapsed.TotalSeconds -lt 15)
    }
    $metrics | ConvertTo-Json | Set-Content "$evidence\$Label-pixels.json" -Encoding UTF8
    Assert-Check "$Label nonblank pixels" ($metrics.DistinctColors -ge 8 -and $metrics.ForegroundPixels -ge 20) $metrics
    Assert-Check "$Label pixel contrast" ($metrics.Contrast -ge $MinimumContrast) $metrics
}
function Invoke-Subcase([string]$Name, [scriptblock]$Body) {
    try { & $Body }
    catch {
        $Context.Checks.Add([pscustomobject]@{name=$Name; passed=$false; detail=$_.Exception.Message})
        try { Save-Screenshot "$Name-failure" } catch { Write-Warning $_ }
    } finally {
        Stop-App
        if (Test-Path $logPath) { Copy-Item $logPath "$evidence\$Name.log" -Force }
    }
}
function Complete-Subcases {
    if (@($Context.Checks | Where-Object passed -EQ $false).Count) { throw 'One or more subcases failed; see checks and per-case evidence.' }
}
function Invoke-HostAction([string]$Action, $Arguments = @{}) {
    $id = [guid]::NewGuid().ToString('N')
    @{id=$id; runId=$request.runId; scenarioId=$request.id; action=$Action; arguments=$Arguments} |
        ConvertTo-Json -Depth 8 | Set-Content "$Root\host-request.tmp" -Encoding UTF8
    Move-Item "$Root\host-request.tmp" "$Root\host-request.json" -Force
    if ($Action -eq 'reboot') {
        # The host reboots; the logon trigger resumes the persisted phase.
        while ($true) { Start-Sleep -Seconds 1 }
    }
    $responsePath = "$Root\host-response-$id.json"
    if (-not (Wait-Condition { Test-Path $responsePath } 180)) { throw "Host action timed out: $Action" }
    $response = Get-Content $responsePath -Raw | ConvertFrom-Json
    if ($response.id -ne $id -or -not $response.ok) { throw "Host action $Action failed: $($response.error)" }
    return $response.data
}
function Save-Continuation([string]$Phase) {
    @{phase=$Phase; checks=@($Context.Checks.ToArray())} | ConvertTo-Json -Depth 12 | Set-Content "$Root\continuation.tmp" -Encoding UTF8
    Move-Item "$Root\continuation.tmp" "$Root\continuation.json" -Force
}
function Toggle-Startup {
    # Invoke the app's real native menu. Never write the Run key on its behalf.
    $w = Get-Widget
    $iconId = Get-RegisteredTrayIcon
    Assert-Check 'startup menu uses a registered tray icon' ($iconId -ge 0) $iconId
    [CCUMScenarioDesktop]::Post($w.Handle, 0x8003, $iconId, 0x205)
    Assert-Check 'startup menu opened' (Wait-Condition { [CCUMScenarioDesktop]::MenuVisible() } 10) 'native popup'
    $language = (Get-Content $settingsPath -Raw | ConvertFrom-Json).language
    $label = if ($language -eq 'de') { 'Mit Windows starten' } else { 'Start with Windows' }
    [CCUMScenarioDesktop]::ChooseMenuItem($label)
    Start-Sleep -Seconds 1
}
function Assert-StartupTarget([string]$Executable, [switch]$ContinueAfterQuoteFailure) {
    $value = Get-ItemPropertyValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name ClaudeCodeUsageMonitor -ErrorAction SilentlyContinue
    $validCommand = $value -eq ('"' + $Executable + '"') -or ($Executable -notmatch '\s' -and $value -eq $Executable)
    if ($ContinueAfterQuoteFailure -and $value -eq $Executable -and -not $validCommand) {
        # Keep the failed assertion, but still exercise reboot/update/uninstall
        # when the path is correct and the only defect is missing quotes.
        $Context.Checks.Add([pscustomobject]@{name='startup command quotes path'; passed=$false; detail=$value})
    } else { Assert-Check 'startup command quotes path when required' $validCommand $value }
    Assert-Check 'startup target exists' (Test-Path -LiteralPath $Executable) $Executable
}
function Get-ResourceSample {
    $p = @(Get-AppProcesses)
    Assert-Check 'resource sample single monitor' ($p.Count -eq 1) $p.Count
    Assert-Check 'monitor window responds' ([CCUMScenarioDesktop]::Responding((Get-Widget).Handle)) $p[0].Id
    $p[0].Refresh()
    [pscustomobject]@{seconds=[Diagnostics.Stopwatch]::GetTimestamp() / [double][Diagnostics.Stopwatch]::Frequency; pid=$p[0].Id; memory=$p[0].PrivateMemorySize64; handles=$p[0].HandleCount; threads=$p[0].Threads.Count; cpu=$p[0].TotalProcessorTime.TotalSeconds}
}
function Assert-StartupDisabled {
    $entry = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    Assert-Check 'startup entry removed' (-not $entry.PSObject.Properties['ClaudeCodeUsageMonitor']) 'HKCU Run'
}
function Invoke-PortableUpdate([string]$Executable) {
    Copy-Item $Context.CandidateExe "$Root\updater-helper.exe" -Force
    Copy-Item $Context.CandidateExe "$Root\update-source.exe" -Force
    $size = (Get-Item "$Root\update-source.exe").Length
    $owner = (Get-AppProcesses)[0].Id
    $arguments = '--apply-update "{0}" "{1}" {2} {3} sha256:{4}' -f $Executable, "$Root\update-source.exe", $owner, $size, $request.candidateHash.ToLowerInvariant()
    $helper = Start-UpdateHelper $arguments
    try {
        Stop-App
        Assert-Check 'portable helper succeeds' ($helper.WaitForExit(90000) -and $helper.ExitCode -eq 0) $helper.Id
        Assert-Check 'portable update installed candidate' ((Get-FileHash $Executable).Hash -eq $request.candidateHash) $Executable
        Assert-App $Executable 'startup-updated'
    } finally { if (-not $helper.HasExited) { Stop-Process -Id $helper.Id -Force } }
}
function Start-UpdateHelper([string]$Arguments) {
    # Match Rust Command/CreateProcess: ShellExecute can apply installer-name
    # heuristics to updater-helper.exe and elevate it above this desktop task.
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = "$Root\updater-helper.exe"
    $start.Arguments = $Arguments
    $start.WorkingDirectory = $Root
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    return [Diagnostics.Process]::Start($start)
}
