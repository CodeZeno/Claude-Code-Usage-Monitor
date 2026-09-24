#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory)][string]$VMName, [Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
$vm = @(Get-VM | Where-Object Name -EQ $VMName)
if ($vm.Count -ne 1 -or $vm[0].Notes -notmatch '(?m)^CCUM-DISPOSABLE-LAB\s*$') { throw 'Select one marked disposable VM.' }
$namespace = 'root\virtualization\v2'
$system = Get-CimInstance -Namespace $namespace -ClassName Msvm_ComputerSystem -Filter "Name='$($vm[0].Id)'"
$settings = @(Get-CimAssociatedInstance -InputObject $system -ResultClassName Msvm_VirtualSystemSettingData | Where-Object VirtualSystemType -EQ 'Microsoft:Hyper-V:System:Realized')
if ($settings.Count -ne 1) { throw 'Cannot resolve current VM display.' }
$service = Get-CimInstance -Namespace $namespace -ClassName Msvm_VirtualSystemManagementService
$width = 1024; $height = 576
$result = Invoke-CimMethod -InputObject $service -MethodName GetVirtualSystemThumbnailImage -Arguments @{ TargetSystem = $settings[0]; WidthPixels = [uint16]$width; HeightPixels = [uint16]$height }
if ($result.ReturnValue -ne 0) { throw "Console capture failed: $($result.ReturnValue)" }
$pixelBytes = $width * $height * 2
# Some hosts prepend a four-byte thumbnail header. Never copy beyond the bitmap allocation.
$offset = if ($result.ImageData.Length -eq ($pixelBytes + 4)) { 4 } else { 0 }
if (($result.ImageData.Length - $offset) -ne $pixelBytes) { throw 'Unexpected thumbnail byte count.' }
Add-Type -AssemblyName System.Drawing
$bitmap = [Drawing.Bitmap]::new($width, $height, [Drawing.Imaging.PixelFormat]::Format16bppRgb565)
try {
    $data = $bitmap.LockBits([Drawing.Rectangle]::new(0, 0, $width, $height), [Drawing.Imaging.ImageLockMode]::WriteOnly, [Drawing.Imaging.PixelFormat]::Format16bppRgb565)
    try { [Runtime.InteropServices.Marshal]::Copy([byte[]]$result.ImageData, $offset, $data.Scan0, $pixelBytes) }
    finally { $bitmap.UnlockBits($data) }
    $bitmap.Save([IO.Path]::GetFullPath($Path), [Drawing.Imaging.ImageFormat]::Png)
} finally { $bitmap.Dispose() }
