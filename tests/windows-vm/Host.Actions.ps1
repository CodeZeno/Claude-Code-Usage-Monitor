# Loaded only by Invoke-Lab after VM identity/checkpoint validation and lock acquisition.
# Request arguments never contain executable commands. Each case has an allowlist.
function Assert-HostRequest($Request, $Scenario, [string]$RunId) {
    if ($Request.id -cnotmatch '^[a-f0-9]{32}$' -or $Request.runId -ne $RunId -or $Request.scenarioId -ne $Scenario.id) { throw 'Host request identity mismatch.' }
    $allowed = @()
    if ($Scenario.test.PSObject.Properties['hostActions']) { $allowed = @($Scenario.test.hostActions) }
    if ($Request.action -notin $allowed) { throw "Host action is not permitted for this case: $($Request.action)" }
}

function Invoke-LabHostRequest($Request, $Scenario, $Target, [ref]$Session, $Credential, [string]$GuestRoot, [hashtable]$HostState) {
    Assert-HostRequest $Request $Scenario $runId
    $data = $null
    switch ($Request.action) {
        'display-modes' {
            if (-not $HostState.ContainsKey('video')) {
                $video = Get-VMVideo -VM $Target.VM
                $HostState.video = @{ResolutionType=[string]$video.ResolutionType; HorizontalResolution=$video.HorizontalResolution; VerticalResolution=$video.VerticalResolution}
            }
            # Hyper-V requires an OFF VM. Apply this during the following reboot,
            # after the guest has saved its continuation and logon trigger.
            $HostState.displayModesPending = $true
            $data = @{resolutionType='Maximum'; pendingReboot=$true}
        }
        'network-disconnect' {
            if ($HostState.ContainsKey('adapters')) { throw 'Network is already disconnected.' }
            $adapters = @(Get-VMNetworkAdapter -VM $Target.VM | Where-Object SwitchId -NE ([guid]::Empty))
            if (-not $adapters.Count) { throw 'VM has no connected network adapter.' }
            # Snapshot scalar values: Hyper-V may refresh objects passed to its
            # mutation cmdlets, losing their former SwitchName.
            $HostState.adapters = @($adapters | ForEach-Object { @{Id=$_.Id; SwitchName=$_.SwitchName} })
            foreach ($adapter in $adapters) { Disconnect-VMNetworkAdapter -VMNetworkAdapter $adapter }
            $data = @{ disconnected=$HostState.adapters.Count }
        }
        'network-connect' {
            if (-not $HostState.ContainsKey('adapters')) { throw 'No saved adapter connections.' }
            foreach ($adapter in $HostState.adapters) {
                Connect-VMNetworkAdapter -VMNetworkAdapter (Get-VMNetworkAdapter -VM $Target.VM | Where-Object Id -EQ $adapter.Id) -SwitchName $adapter.SwitchName
            }
            $HostState.Remove('adapters')
            $data = @{connected=$true}
        }
        'pause-resume' {
            # PowerShell Direct transports may break when their VM is suspended.
            # End our control connection first; the interactive scenario task
            # continues independently and receives its response after reconnect.
            Remove-PSSession $Session.Value
            $Session.Value = $null
            Suspend-VM -VM $Target.VM
            try { Start-Sleep -Seconds 10 } finally { Resume-VM -VM $Target.VM }
            $Session.Value = New-PSSession -VMId $Target.VM.Id -Credential $Credential -ErrorAction Stop
            $data = @{pausedSeconds=10}
        }
        'clock-forward' {
            $HostState.timeServices = @(Get-VMIntegrationService -VM $Target.VM | Where-Object { $_.Id -match '2497f4de-e9fa-4204-80e4-4b75c46419c0' -and $_.Enabled })
            foreach ($service in $HostState.timeServices) { Disable-VMIntegrationService -VMIntegrationService $service }
            $data = Invoke-Command -Session $Session.Value -ScriptBlock {
                $before = Get-Date
                Set-Date -Date $before.AddHours(2) | Out-Null
                @{before=$before.ToUniversalTime().ToString('o'); after=(Get-Date).ToUniversalTime().ToString('o')}
            }
        }
        'audit-start' {
            $data = Invoke-Command -Session $Session.Value -ScriptBlock {
                param($root)
                # Filtering Platform Connection captures short-lived TCP and UDP activity,
                # including blocked attempts, unlike polling Get-NetTCPConnection.
                & auditpol.exe /set '/subcategory:{0CCE9226-69AE-11D9-BED3-505054503030}' /success:enable /failure:enable | Out-Null
                if ($LASTEXITCODE -ne 0) { throw 'Cannot enable WFP connection auditing.' }
                & auditpol.exe /set '/subcategory:{0CCE922B-69AE-11D9-BED3-505054503030}' /success:enable | Out-Null
                if ($LASTEXITCODE -ne 0) { throw 'Cannot enable process creation auditing.' }
                $record = Get-WinEvent -LogName Security -MaxEvents 1
                $record.RecordId | Set-Content "$root\audit-start.txt"
                @{recordId=$record.RecordId}
            } -ArgumentList $GuestRoot
        }
        'audit-stop' {
            $data = Invoke-Command -Session $Session.Value -ScriptBlock {
                param($root)
                $start = [long](Get-Content "$root\audit-start.txt")
                $earliest = Get-WinEvent -LogName Security -Oldest -MaxEvents 1
                if ($earliest.RecordId -gt $start) { throw 'Security log wrapped during capture; evidence is incomplete.' }
                $queryErrors = @()
                $events = @(Get-WinEvent -LogName Security -FilterXPath "*[System[(EventID=5156 or EventID=5157 or EventID=4688) and EventRecordID > $start]]" -ErrorAction SilentlyContinue -ErrorVariable queryErrors | Sort-Object RecordId)
                foreach ($queryError in $queryErrors) {
                    if ($queryError.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*') { throw $queryError }
                }
                $tracked = @{}
                $connections = @($events | ForEach-Object {
                    [xml]$xml = $_.ToXml(); $fields = @{}
                    foreach ($field in $xml.Event.EventData.Data) { $fields[$field.Name] = $field.'#text' }
                    if ($_.Id -eq 4688) {
                        $createdId = [long]$fields.NewProcessId
                        $parentId = [long]$fields.ProcessId
                        # Reassign on PID reuse. Do not collect process command lines.
                        $tracked[$createdId] = ($fields.NewProcessName -like '*\claude-code-usage-monitor.exe' -or ($tracked.ContainsKey($parentId) -and $tracked[$parentId]))
                    } elseif (($fields.Application -like '*\claude-code-usage-monitor.exe' -or ($tracked.ContainsKey([long]$fields.ProcessID) -and $tracked[[long]$fields.ProcessID])) -and $fields.Direction -eq '%%14593') {
                        [pscustomobject]@{recordId=$_.RecordId; application=$fields.Application; pid=$fields.ProcessID; destination=$fields.DestAddress; port=$fields.DestPort; protocol=$fields.Protocol; eventId=$_.Id}
                    }
                })
                ConvertTo-Json -InputObject $connections -Depth 6 | Set-Content "$root\evidence\outbound-connections.json" -Encoding UTF8
                @{connections=$connections; observedEvents=$events.Count; startRecord=$start; endRecord=(Get-WinEvent -LogName Security -MaxEvents 1).RecordId}
            } -ArgumentList $GuestRoot
        }
        'reboot' {
            # Only disposable guests, using the already supplied lab credential.
            # The password is never serialized into requests, evidence, or transcripts.
            $coldBoot = $HostState.ContainsKey('displayModesPending')
            $previousBoot = Invoke-Command -Session $Session.Value -ScriptBlock {
                param($credential, $root, $coldBoot)
                $key = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
                Set-ItemProperty $key AutoAdminLogon '1'
                Set-ItemProperty $key DefaultUserName ($credential.UserName -split '\\')[-1]
                Set-ItemProperty $key DefaultDomainName $env:COMPUTERNAME
                Set-ItemProperty $key DefaultPassword $credential.GetNetworkCredential().Password
                Set-ItemProperty $key AutoLogonCount 1 -Type DWord
                $trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:COMPUTERNAME\$(($credential.UserName -split '\\')[-1])"
                Set-ScheduledTask -TaskName CCUM-Lab-Scenario -Trigger $trigger | Out-Null
                Remove-Item -LiteralPath "$root\host-request.json"
                $boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToFileTimeUtc()
                # Graceful OS reboot flushes continuation and Winlogon state.
                if ($coldBoot) { & shutdown.exe /s /t 3 /f } else { & shutdown.exe /r /t 3 /f }
                if ($LASTEXITCODE -ne 0) { throw 'Unable to schedule guest reboot.' }
                $boot
            } -ArgumentList $Credential, $GuestRoot, $coldBoot
            Remove-PSSession $Session.Value
            $Session.Value = $null
            if ($coldBoot) {
                $shutdownWatch = [Diagnostics.Stopwatch]::StartNew()
                while ((Get-VM -Id $Target.VM.Id).State -ne 'Off' -and $shutdownWatch.Elapsed.TotalSeconds -lt 150) { Start-Sleep -Seconds 2 }
                if ((Get-VM -Id $Target.VM.Id).State -ne 'Off') { throw 'Guest did not shut down for display configuration.' }
                Set-VMVideo -VM $Target.VM -ResolutionType Maximum -HorizontalResolution 2560 -VerticalResolution 1440
                $HostState.Remove('displayModesPending')
                Start-VM -VM $Target.VM | Out-Null
            }
            $watch = [Diagnostics.Stopwatch]::StartNew()
            do {
                $reconnected = $null
                try {
                    $reconnected = New-PSSession -VMId $Target.VM.Id -Credential $Credential -ErrorAction Stop
                    $boot = Invoke-Command -Session $reconnected -ScriptBlock { (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToFileTimeUtc() }
                    if ($boot -ne $previousBoot) { $Session.Value = $reconnected; $reconnected = $null }
                } catch { Write-Verbose 'Waiting for the guest to boot.' }
                finally { if ($reconnected) { Remove-PSSession $reconnected -ErrorAction SilentlyContinue } }
                if (-not $Session.Value) { Start-Sleep -Seconds 2 }
            } until ($Session.Value -or $watch.Elapsed.TotalSeconds -gt 150)
            if (-not $Session.Value) { throw 'PowerShell Direct did not recover after reboot.' }
            Invoke-Command -Session $Session.Value -ScriptBlock {
                $key = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
                # Wait for actual interactive logon before removing its one-time secret.
                $watch = [Diagnostics.Stopwatch]::StartNew()
                while (-not @(Get-Process explorer -ErrorAction SilentlyContinue).Count -and $watch.Elapsed.TotalSeconds -lt 90) { Start-Sleep -Seconds 1 }
                Set-ItemProperty $key AutoAdminLogon '0'
                Remove-ItemProperty $key DefaultPassword -ErrorAction SilentlyContinue
                Remove-ItemProperty $key AutoLogonCount -ErrorAction SilentlyContinue
                if (-not @(Get-Process explorer -ErrorAction SilentlyContinue).Count) { throw 'Automatic guest sign-in did not complete.' }
            }
            return
        }
        default { throw "Unknown host action: $($Request.action)" }
    }
    Invoke-Command -Session $Session.Value -ScriptBlock {
        param($root, $id, $data)
        @{id=$id; ok=$true; error=$null; data=$data} | ConvertTo-Json -Depth 10 | Set-Content "$root\host-response-$id.tmp" -Encoding UTF8
        Move-Item "$root\host-response-$id.tmp" "$root\host-response-$id.json" -Force
        Remove-Item -LiteralPath "$root\host-request.json"
    } -ArgumentList $GuestRoot, $Request.id, $data
}

function Restore-LabHostState($Target, [hashtable]$HostState) {
    # Checkpoints do not restore all host-owned adapter/integration settings.
    if ((Get-VM -Id $Target.VM.Id).State -eq 'Paused') { Resume-VM -VM $Target.VM }
    if ($HostState.ContainsKey('video')) {
        $current = Get-VMVideo -VM $Target.VM
        $original = $HostState.video
        if ($current.ResolutionType -ne $original.ResolutionType -or $current.HorizontalResolution -ne $original.HorizontalResolution -or $current.VerticalResolution -ne $original.VerticalResolution) {
            # The baseline checkpoint is restored immediately after host cleanup.
            if ((Get-VM -Id $Target.VM.Id).State -ne 'Off') { Stop-VM -VM $Target.VM -TurnOff -Confirm:$false }
            Set-VMVideo -VM $Target.VM -ResolutionType $original.ResolutionType -HorizontalResolution $original.HorizontalResolution -VerticalResolution $original.VerticalResolution
        }
    }
    if ($HostState.ContainsKey('adapters')) {
        foreach ($adapter in $HostState.adapters) {
            Connect-VMNetworkAdapter -VMNetworkAdapter (Get-VMNetworkAdapter -VM $Target.VM | Where-Object Id -EQ $adapter.Id) -SwitchName $adapter.SwitchName
        }
    }
    if ($HostState.ContainsKey('timeServices')) {
        foreach ($service in $HostState.timeServices) { Enable-VMIntegrationService -VMIntegrationService $service }
    }
}
