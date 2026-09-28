param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
foreach ($mode in 'wrong-sha256','wrong-size','locked-target','killed-helper') {
    Invoke-Subcase $mode {
        Initialize-TestSettings
        $executable = Install-PortableApp
        # Update integrity does not depend on credential discovery or live services.
        $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
        Assert-App $executable 'initial'
        Assert-Settings
        $owner = (Get-AppProcesses)[0].Id
        $originalHash = (Get-FileHash $executable).Hash
        Copy-Item $Context.CandidateExe "$Root\updater-helper.exe" -Force
        Copy-Item $Context.CandidateExe "$Root\update-source.exe" -Force
        if ($mode -eq 'killed-helper') {
            # A PE overlay makes the real copy observable even on fast VM storage.
            # The helper receives the correct digest/size for these valid PE bytes.
            $source = [IO.File]::OpenWrite("$Root\update-source.exe")
            try {
                $source.Position = $source.Length
                $random = [Random]::new(125)
                $buffer = New-Object byte[] 1048576
                while ($source.Length -lt 96MB) {
                    # Materialize the overlay: extending EOF with zeroes lets
                    # Windows optimize the copy so much that interruption misses it.
                    $random.NextBytes($buffer)
                    $source.Write($buffer, 0, [int][Math]::Min($buffer.Length, 96MB-$source.Length))
                }
            } finally { $source.Dispose() }
        }
        $size = (Get-Item "$Root\update-source.exe").Length
        $hash = (Get-FileHash "$Root\update-source.exe").Hash.ToLowerInvariant()
        if ($mode -eq 'wrong-sha256') { $hash = '0' * 64 }
        if ($mode -eq 'wrong-size') { $size++ }
        $lock = $null; $helper = $null
        try {
            if ($mode -in 'locked-target','killed-helper') {
                # Deny FILE_SHARE_DELETE/WRITE while still allowing executable reads.
                $lock = [IO.File]::Open($executable, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
            }
            Stop-App
            # Freeze the baseline after shutdown: an in-flight update check may
            # legitimately persist its timestamp while the original is running.
            $settingsHash = (Get-FileHash $settingsPath).Hash
            # Pass the real, now-exited owner: PID 0 is rejected by the CLI parser.
            $arguments = '--apply-update "{0}" "{1}" {2} {3} sha256:{4}' -f $executable, "$Root\update-source.exe", $owner, $size, $hash
            $helper = Start-UpdateHelper $arguments
            $null = $helper.Handle
            if ($mode -eq 'killed-helper') {
                # Hold replacement until validation has had time to complete, then
                # observe the backup and kill during the actual destructive copy.
                Start-Sleep -Seconds 3
                Assert-Check 'helper reached retry window and is alive' (-not $helper.HasExited) $helper.Id
                $stage = [CCUMScenarioDesktop]::KillDuringReplacement($helper.Id, $executable, $size, $lock)
                $lock = $null
                Assert-Check 'helper interrupted during replacement' $true $stage
            } else {
                # Failed helpers display a modal error. Dismiss only this helper's dialog.
                $null = Wait-Condition { [CCUMScenarioDesktop]::ReadUpdateError($helper.Id).Length -gt 0 } 180
                $helper.Refresh()
                $message = [CCUMScenarioDesktop]::ReadUpdateError($helper.Id)
                $shown = $message.Length -gt 0
                @{pid=$helper.Id; exited=$helper.HasExited; title=$helper.MainWindowTitle; message=$message} |
                    ConvertTo-Json | Set-Content "$evidence\$mode-helper.json" -Encoding UTF8
                if (-not $shown) { Save-Screenshot "$mode-helper-timeout" }
                Assert-Check "$mode error dialog" $shown $helper.Id
                $expected = switch ($mode) {
                    'wrong-sha256' { 'SHA-256' }
                    'wrong-size' { 'size does not match' }
                    'locked-target' { 'Unable to replace' }
                }
                Assert-Check "$mode specific rejection reason" ($message -match $expected) $message
                Assert-Check "$mode error acknowledged" (Wait-Condition { [CCUMScenarioDesktop]::DismissUpdateError($helper.Id) } 10) 'helper OK button'
                $exited = $helper.WaitForExit(30000)
                $exitCode = if ($exited) { $helper.ExitCode } else { $null }
                Assert-Check "$mode helper exits unsuccessfully" ($exited -and $null -ne $exitCode -and $exitCode -ne 0) @{pid=$helper.Id; exited=$exited; exitCode=$exitCode}
            }
        } finally {
            if ($lock) { $lock.Dispose() }
            if ($helper -and -not $helper.HasExited) { Stop-Process -Id $helper.Id -Force }
        }
        $actualHash = if (Test-Path $executable) { (Get-FileHash $executable).Hash } else { 'missing' }
        if ($mode -eq 'killed-helper') {
            # A backup's presence alone does not prove CopyFile was interrupted:
            # it also exists briefly after a successful copy. Reject that late kill.
            Assert-Check 'interruption occurred before complete candidate copy' ($actualHash -ne $hash) @{completeCandidate=$hash; actual=$actualHash}
        }
        $actualSettingsHash = if (Test-Path $settingsPath) { (Get-FileHash $settingsPath).Hash } else { 'missing' }
        Assert-Check "$mode original bytes unchanged" ($actualHash -eq $originalHash) @{expected=$originalHash; actual=$actualHash} -ContinueOnFailure
        Assert-Check "$mode settings bytes unchanged" ($actualSettingsHash -eq $settingsHash) @{expected=$settingsHash; actual=$actualSettingsHash} -ContinueOnFailure
        if ($actualHash -ne $originalHash) { throw 'Original target was not preserved; recovery launch was not attempted.' }
        # Recovery launch proves the old binary remains usable; automatic rollback
        # relaunch is deliberately not claimed (a killed helper cannot launch it).
        $null = Start-Process $executable -ArgumentList '--diagnose --diagnose-append --no-poll'
        Assert-App $executable $mode
        Assert-Settings
        Assert-NoPanic $mode
    }
}
Complete-Subcases
