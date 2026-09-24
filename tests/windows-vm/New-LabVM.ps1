#Requires -Version 5.1
#Requires -RunAsAdministrator
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^CCUM-[A-Za-z0-9-]+$')][string]$VMName,
    [Parameter(Mandatory)][string]$IsoPath,
    [Parameter(Mandatory)][string]$ImageName,
    [Parameter(Mandatory)][System.Management.Automation.PSCredential]$Credential,
    [string]$LabRoot = 'C:\work\CCUM-Lab',
    [string]$SwitchName = 'Default Switch',
    [switch]$Resume
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Hyper-V
if ($VMName.Length -gt 15) { throw 'VMName must fit the Windows computer name limit (15 characters).' }
if ($Credential.UserName -ne 'LabUser') { throw 'Provisioning expects the local account LabUser.' }
if (@(Get-VM | Where-Object Name -EQ $VMName).Count) { throw "VM already exists: $VMName" }
$switch = @(Get-VMSwitch | Where-Object Name -EQ $SwitchName)
if ($switch.Count -ne 1) { throw 'Expected exactly one matching virtual switch.' }
$vmRoot = Join-Path ([IO.Path]::GetFullPath($LabRoot)) $VMName
if ((Test-Path -LiteralPath $vmRoot) -and -not $Resume) { throw "Provisioning directory already exists: $vmRoot (use -Resume only after image application completed)." }
$IsoPath = (Resolve-Path -LiteralPath $IsoPath).Path
$isoMountedHere = $false
$vhdMounted = $false
$vhdPath = Join-Path $vmRoot 'system.vhdx'
try {
    $iso = Get-DiskImage -ImagePath $IsoPath
    if (-not $iso.Attached) { $iso = Mount-DiskImage -ImagePath $IsoPath -Access ReadOnly -PassThru; $isoMountedHere = $true }
    $volume = $iso | Get-Volume
    $media = '{0}:\' -f $volume.DriveLetter
    $images = @(Get-ChildItem -LiteralPath (Join-Path $media 'sources') -File | Where-Object Name -Match '^install\.(wim|esd)$')
    if ($images.Count -ne 1) { throw 'Expected one installation WIM or ESD.' }
    $image = @(Get-WindowsImage -ImagePath $images[0].FullName | Where-Object ImageName -EQ $ImageName)
    if ($image.Count -ne 1) { throw "Image not found: $ImageName" }
    $marker = Join-Path $vmRoot 'image-applied.json'
    if ($Resume) {
        $saved = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
        if ($saved.iso -ne $IsoPath -or $saved.image -ne $ImageName -or $saved.vm -ne $VMName) { throw 'Resume image identity mismatch.' }
    } else {
        $null = New-Item -ItemType Directory -Path $vmRoot
        $null = New-VHD -Path $vhdPath -SizeBytes 64GB -Dynamic
    }
    $vhd = Mount-VHD -Path $vhdPath -PassThru
    $vhdMounted = $true
    $disk = Get-Disk -Number $vhd.DiskNumber
    # Partition only the just-created, empty virtual disk; never resolve a physical disk by guesswork.
    $verified = Get-VHD -DiskNumber $disk.Number
    if ($verified.Path -ne $vhdPath -or $disk.IsBoot -or $disk.IsSystem -or (-not $Resume -and $disk.PartitionStyle -ne 'RAW')) {
        throw 'New virtual disk identity/safety check failed.'
    }
    if (-not $Resume) {
        Initialize-Disk -Number $disk.Number -PartitionStyle GPT
        $efi = New-Partition -DiskNumber $disk.Number -Size 260MB -GptType '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' -AssignDriveLetter
        $null = Format-Volume -Partition $efi -FileSystem FAT32 -NewFileSystemLabel 'CCUM EFI' -Confirm:$false
        $null = New-Partition -DiskNumber $disk.Number -Size 16MB -GptType '{e3c9e316-0b5c-4db8-817d-f92df00215ae}'
        $windows = New-Partition -DiskNumber $disk.Number -UseMaximumSize -AssignDriveLetter
        $null = Format-Volume -Partition $windows -FileSystem NTFS -NewFileSystemLabel $VMName -Confirm:$false
        $osRoot = '{0}:\' -f $windows.DriveLetter
        $efiRoot = '{0}:' -f $efi.DriveLetter
        Write-Host "Applying $ImageName to $vhdPath"
        $null = Expand-WindowsImage -ImagePath $images[0].FullName -Index $image[0].ImageIndex -ApplyPath $osRoot -CheckIntegrity
        @{ iso = $IsoPath; image = $ImageName; vm = $VMName } | ConvertTo-Json | Set-Content -LiteralPath $marker -Encoding UTF8
    } else {
        $partitions = @(Get-Partition -DiskNumber $disk.Number | Where-Object DriveLetter)
        $osPartitions = @($partitions | Where-Object Size -GT 1GB)
        $efiPartitions = @($partitions | Where-Object GptType -EQ '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}')
        if ($osPartitions.Count -ne 1 -or $efiPartitions.Count -ne 1) { throw 'Prepared disk partition layout is unexpected.' }
        $osRoot = '{0}:\' -f $osPartitions[0].DriveLetter
        $efiRoot = '{0}:' -f $efiPartitions[0].DriveLetter
    }
    # Prefer the host utility; older images may need their own matching boot-resource reader.
    & bcdboot.exe (Join-Path $osRoot 'Windows') /s $efiRoot /f UEFI
    if ($LASTEXITCODE -ne 0) {
        & (Join-Path $osRoot 'Windows\System32\bcdboot.exe') (Join-Path $osRoot 'Windows') /s $efiRoot /f UEFI
    }
    if ($LASTEXITCODE -ne 0) { throw "BCDBoot failed: $LASTEXITCODE" }
    $guestLab = Join-Path $osRoot 'CCUM-Lab'
    $null = New-Item -ItemType Directory -Path $guestLab -Force
    Copy-Item -LiteralPath "$PSScriptRoot\Initialize-Guest.ps1" -Destination "$guestLab\Initialize-Guest.ps1"
    $password = [Security.SecurityElement]::Escape($Credential.GetNetworkCredential().Password)
    $answer = @"
<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>$VMName</ComputerName><TimeZone>E. Australia Standard Time</TimeZone>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <InputLocale>0409:00000409</InputLocale><SystemLocale>en-US</SystemLocale><UILanguage>en-US</UILanguage><UserLocale>en-AU</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <OOBE><HideEULAPage>true</HideEULAPage><HideOEMRegistrationScreen>true</HideOEMRegistrationScreen><HideOnlineAccountScreens>true</HideOnlineAccountScreens><HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE><ProtectYourPC>3</ProtectYourPC></OOBE>
      <UserAccounts><LocalAccounts><LocalAccount wcm:action="add"><Name>LabUser</Name><DisplayName>LabUser</DisplayName><Group>Administrators</Group><Password><Value>$password</Value><PlainText>true</PlainText></Password></LocalAccount></LocalAccounts></UserAccounts>
      <AutoLogon><Enabled>true</Enabled><Username>LabUser</Username><LogonCount>1</LogonCount><Password><Value>$password</Value><PlainText>true</PlainText></Password></AutoLogon>
      <FirstLogonCommands><SynchronousCommand wcm:action="add"><Order>1</Order><Description>Prepare disposable CCUM test desktop</Description><CommandLine>powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\CCUM-Lab\Initialize-Guest.ps1</CommandLine></SynchronousCommand></FirstLogonCommands>
    </component>
  </settings>
</unattend>
"@
    $null = [xml]$answer
    $panther = Join-Path $osRoot 'Windows\Panther'
    $null = New-Item -ItemType Directory -Path $panther -Force
    [IO.File]::WriteAllText((Join-Path $panther 'unattend.xml'), $answer, [Text.UTF8Encoding]::new($false))
    $answer = $null; $password = $null
    Dismount-VHD -Path $vhdPath
    $vhdMounted = $false
    $vm = New-VM -Name $VMName -Generation 2 -MemoryStartupBytes 4GB -VHDPath $vhdPath -Path $vmRoot -SwitchName $SwitchName
    Set-VM -VM $vm -ProcessorCount 2 -Notes 'CCUM-DISPOSABLE-LAB' -CheckpointType Standard -AutomaticCheckpointsEnabled $false
    Set-VMFirmware -VM $vm -EnableSecureBoot On -SecureBootTemplate MicrosoftWindows
    Set-VMKeyProtector -VM $vm -NewLocalKeyProtector
    Enable-VMTPM -VM $vm
    Set-VMVideo -VM $vm -HorizontalResolution 1920 -VerticalResolution 1080 -ResolutionType Single
    Start-VM -VM $vm
    Write-Host "Started $VMName. Wait for C:\CCUM-Lab\initialized.json before capturing a checkpoint."
} finally {
    if ($vhdMounted) { Dismount-VHD -Path $vhdPath }
    if ($isoMountedHere) { Dismount-DiskImage -ImagePath $IsoPath | Out-Null }
}
