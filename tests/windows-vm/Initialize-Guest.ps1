#Requires -Version 5.1
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ((Get-CimInstance Win32_ComputerSystem).Model -ne 'Virtual Machine' -or $env:COMPUTERNAME -notmatch '^CCUM-') { throw 'Disposable CCUM guests only.' }
Start-Transcript -LiteralPath C:\CCUM-Lab\initialize.log | Out-Null
try {
    $null = New-Item -ItemType File -Path C:\CCUM-Lab\guest-enabled -Force
    Set-LocalUser -Name LabUser -PasswordNeverExpires $true
    foreach ($setting in 'monitor-timeout-ac', 'standby-timeout-ac', 'hibernate-timeout-ac') {
        & powercfg.exe /change $setting 0
        if ($LASTEXITCODE -ne 0) { throw "powercfg failed for $setting" }
    }
    Set-ItemProperty 'HKCU:\Control Panel\Desktop' -Name ScreenSaveActive -Value '0'
    $policy = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
    Set-ItemProperty $policy -Name InactivityTimeoutSecs -Value 0 -Type DWord
    $updates = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU'
    $null = New-Item -Path $updates -Force
    Set-ItemProperty $updates -Name NoAutoUpdate -Value 1 -Type DWord
    $winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
    Set-ItemProperty $winlogon -Name AutoAdminLogon -Value '0'
    Remove-ItemProperty $winlogon -Name DefaultPassword -ErrorAction SilentlyContinue
    Remove-ItemProperty $winlogon -Name AutoLogonCount -ErrorAction SilentlyContinue
    # Fixed, guest-only answer paths. Remove the temporary installation password.
    foreach ($path in 'C:\Windows\Panther\unattend.xml', 'C:\Windows\Panther\Unattend\unattend.xml') {
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    $os = Get-CimInstance Win32_OperatingSystem
    @{ computer = $env:COMPUTERNAME; caption = $os.Caption; build = $os.BuildNumber; initializedUtc = [DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath C:\CCUM-Lab\initialized.json -Encoding UTF8
} finally { Stop-Transcript | Out-Null }
