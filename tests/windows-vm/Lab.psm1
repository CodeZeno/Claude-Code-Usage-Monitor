Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Read-LabConfig {
    param([Parameter(Mandatory)][string]$Path)
    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.schemaVersion -ne 1) { throw 'Unsupported lab schemaVersion.' }
    if (@($config.vms).Count -ne 2) { throw 'The proof of concept requires exactly two VMs.' }
    $names = @{}
    $systems = @{}
    foreach ($vm in $config.vms) {
        if ($vm.name -notmatch '^CCUM-[A-Za-z0-9-]+$') { throw 'VM names must start with CCUM- and contain only letters, digits and hyphens.' }
        if ($names.ContainsKey($vm.name)) { throw 'Duplicate VM name.' }
        $names[$vm.name] = $true
        if ($vm.os -notin @('windows10', 'windows11') -or $systems.ContainsKey($vm.os)) { throw 'Configure one windows10 and one windows11 VM.' }
        $systems[$vm.os] = $true
        if ([string]::IsNullOrWhiteSpace($vm.checkpoint) -or [string]::IsNullOrWhiteSpace($vm.guestUser)) { throw 'checkpoint and guestUser are required.' }
    }
    return $config
}

function Assert-CatalogList {
    param($Value, [string]$Name, [switch]$AllowEmpty)
    if ($Value -isnot [array] -or (-not $AllowEmpty -and $Value.Count -eq 0)) { throw "$Name must be a JSON array$(if (-not $AllowEmpty) { ' with at least one entry' })." }
    foreach ($item in $Value) { if ($item -isnot [string] -or [string]::IsNullOrWhiteSpace($item)) { throw "$Name entries must be nonempty strings." } }
}

function Read-LabCatalog {
    param([string]$Path = "$PSScriptRoot\tests.json")
    $catalog = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($catalog.schemaVersion -ne 1) { throw 'Unsupported test catalogue schemaVersion.' }
    if ($catalog.tests -isnot [array] -or $catalog.suites -isnot [array]) { throw 'Catalogue tests and suites must be JSON arrays.' }
    $suiteIds = @{}
    foreach ($suite in $catalog.suites) {
        if ($suite.id -cnotmatch '^[a-z0-9]+(-[a-z0-9]+)*$' -or $suiteIds.ContainsKey($suite.id)) { throw 'Invalid or duplicate suite ID.' }
        $suiteIds[$suite.id] = $true
        Assert-CatalogList $suite.taskbars 'Suite taskbars'
        foreach ($bar in $suite.taskbars) { if ($bar -notin @('baseline', 'auto-hide', 'left', 'center')) { throw "Unknown taskbar: $bar" } }
    }
    $ids = @{}
    if (@($catalog.tests).Count -eq 0) { throw 'Test catalogue cannot be empty.' }
    foreach ($test in $catalog.tests) {
        if ($test.id -cnotmatch '^[a-z0-9]+(-[a-z0-9]+)*$' -or $ids.ContainsKey($test.id)) { throw 'Invalid or duplicate test ID.' }
        $ids[$test.id] = $true
        if ([string]::IsNullOrWhiteSpace($test.description)) { throw 'Test description is required.' }
        if ($test.script -cnotmatch '^cases/[a-z0-9-]+\.ps1$') { throw "Invalid case script path: $($test.script)" }
        if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $test.script) -PathType Leaf)) { throw "Missing case script: $($test.script)" }
        if ($test.timeoutSeconds -isnot [int] -or $test.timeoutSeconds -lt 60 -or $test.timeoutSeconds -gt 7200) { throw 'Test timeoutSeconds must be an integer from 60 to 7200.' }
        if ($test.PSObject.Properties['hostActions']) {
            Assert-CatalogList $test.hostActions 'Test hostActions' -AllowEmpty
            foreach ($action in $test.hostActions) {
                if ($action -notin @('reboot', 'network-disconnect', 'network-connect', 'pause-resume', 'clock-forward', 'audit-start', 'audit-stop', 'display-modes')) { throw "Unknown host action: $action" }
            }
        }
        foreach ($field in @('suites', 'operatingSystems', 'taskbars')) {
            Assert-CatalogList $test.$field "Test $field"
        }
        Assert-CatalogList $test.requires 'Test requires' -AllowEmpty
        foreach ($suite in $test.suites) { if (-not $suiteIds.ContainsKey($suite)) { throw "Unknown suite: $suite" } }
        foreach ($os in $test.operatingSystems) { if ($os -notin @('windows10', 'windows11')) { throw "Unknown OS: $os" } }
        foreach ($bar in $test.taskbars) { if ($bar -notin @('baseline', 'auto-hide', 'left', 'center')) { throw "Unknown taskbar: $bar" } }
        foreach ($inputName in $test.requires) {
            if ($inputName -notin @('candidateExe', 'previousExe', 'candidateVersion', 'previousVersion')) { throw "Unknown required input: $inputName" }
        }
        if ('previousExe' -in $test.requires -and 'candidateExe' -notin $test.requires) { throw 'previousExe requires candidateExe.' }
        if ('previousVersion' -in $test.requires -and 'candidateVersion' -notin $test.requires) { throw 'previousVersion requires candidateVersion.' }
    }
    return $catalog
}

function Get-LabPlan {
    param([Parameter(Mandatory)]$Config,
        [Alias('Flows')][string[]]$Tests,
        [string[]]$Taskbars,
        [string[]]$Suite,
        $Catalog = (Read-LabCatalog))
    if ($Tests -and $Suite) { throw 'Select either -Tests (or -Flows) or -Suite.' }
    if (-not $Tests -and -not $Suite) { $Suite = @('smoke') }
    foreach ($id in $Tests) { if ($id -notin $Catalog.tests.id) { throw "Unknown test: $id" } }
    foreach ($id in $Suite) { if ($id -notin $Catalog.suites.id) { throw "Unknown suite: $id" } }
    foreach ($bar in $Taskbars) { if ($bar -notin @('baseline', 'auto-hide', 'left', 'center')) { throw "Unknown taskbar: $bar" } }
    # Catalogue order is execution order, regardless of selection order.
    foreach ($test in $Catalog.tests) {
        $selectedSuites = @($Catalog.suites | Where-Object { $_.id -in $Suite -and $_.id -in $test.suites })
        if ($test.id -notin $Tests -and $selectedSuites.Count -eq 0) { continue }
        $bars = if ($Taskbars) { $Taskbars } elseif ($Suite) { @($selectedSuites | ForEach-Object taskbars) } else { $test.taskbars }
        foreach ($vm in $Config.vms) {
            foreach ($bar in ($bars | Select-Object -Unique)) {
                $reason = $null
                if ($vm.os -notin $test.operatingSystems) { $reason = 'Test does not support this OS.' }
                elseif ($bar -notin $test.taskbars) { $reason = 'Test does not support this taskbar mode.' }
                elseif ($vm.os -eq 'windows10' -and $bar -in @('left', 'center')) { $reason = 'Icon alignment is supported only on Windows 11.' }
                [pscustomobject]@{
                    id = "$($vm.name)-$($test.id)-$bar"; vm = $vm; flow = $test.id; test = $test
                    taskbar = $bar; status = $(if ($reason) { 'skipped' } else { 'pending' }); reason = $reason
                }
            }
        }
    }
}

function Assert-LabInputs {
    param([object[]]$Plan, [hashtable]$Inputs)
    $required = @($Plan | Where-Object status -NE 'skipped' | ForEach-Object { $_.test.requires } | Select-Object -Unique)
    foreach ($name in $required) {
        if ([string]::IsNullOrWhiteSpace($Inputs[$name])) { throw "Selected tests require -$name." }
        if ($name -like '*Exe') {
            if (-not (Test-Path -LiteralPath $Inputs[$name] -PathType Leaf)) { throw "Missing executable for -${name}: $($Inputs[$name])" }
            $Inputs[$name] = (Resolve-Path -LiteralPath $Inputs[$name]).Path
        } elseif ($Inputs[$name] -notmatch '^\d+\.\d+\.\d+$') { throw "-$name must be a published version (x.y.z)." }
    }
    if ('previousExe' -in $required) {
        if ((Get-FileHash -LiteralPath $Inputs.previousExe).Hash -eq (Get-FileHash -LiteralPath $Inputs.candidateExe).Hash) { throw 'Old and candidate binaries must differ.' }
        $oldVersion = [version](Get-Item -LiteralPath $Inputs.previousExe).VersionInfo.ProductVersion
        $newVersion = [version](Get-Item -LiteralPath $Inputs.candidateExe).VersionInfo.ProductVersion
        if ($newVersion -le $oldVersion) { throw 'Candidate product version must be newer than PreviousExe.' }
    }
    if ('previousVersion' -in $required -and [version]$Inputs.previousVersion -ge [version]$Inputs.candidateVersion) { throw 'PreviousVersion must be older than CandidateVersion.' }
}

function New-LabResults {
    param([object[]]$Plan, [string]$RunRoot)
    foreach ($scenario in $Plan) {
        [pscustomobject][ordered]@{
            id = $scenario.id; test = $scenario.flow; vm = $scenario.vm.name; taskbar = $scenario.taskbar
            status = $(if ($scenario.status -eq 'skipped') { 'skipped' } else { 'not-run' })
            reason = $scenario.reason; startedUtc = $null; durationSeconds = 0
            error = $null; failedCheck = $null; cleanupError = $null; evidenceErrors = @()
            evidence = (Join-Path $RunRoot $scenario.id)
        }
    }
}

function Write-LabReport {
    param([object[]]$Results, [string]$RunRoot)
    ConvertTo-Json -InputObject @($Results) -Depth 8 | Set-Content -LiteralPath "$RunRoot\summary.tmp" -Encoding UTF8
    Move-Item -LiteralPath "$RunRoot\summary.tmp" -Destination "$RunRoot\summary.json" -Force
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('# Windows VM test results')
    $lines.Add('')
    $counts = @($Results | Group-Object status | ForEach-Object { "$($_.Count) $($_.Name)" })
    $lines.Add(($counts -join ', '))
    $lines.Add('')
    $lines.Add('| Scenario | Status | Seconds | Detail | Evidence |')
    $lines.Add('| --- | --- | ---: | --- | --- |')
    foreach ($result in $Results) {
        $detail = (@($result.failedCheck, $result.error, $result.reason, $result.cleanupError) + @($result.evidenceErrors) | Where-Object { $_ }) -join '; '
        $detail = $detail.Replace('|', '\|') -replace '\r?\n', ' '
        $link = if (Test-Path -LiteralPath "$($result.evidence)\evidence") { "[files](./$($result.id)/evidence/)" } else { '-' }
        $lines.Add("| $($result.id) | $($result.status) | $($result.durationSeconds) | $detail | $link |")
    }
    foreach ($result in $Results) {
        $gallery = "$($result.evidence)\evidence\gallery.json"
        if (Test-Path -LiteralPath $gallery) {
            $lines.Add('')
            $lines.Add("## $($result.id) gallery")
            foreach ($entry in (Get-Content -LiteralPath $gallery -Raw | ConvertFrom-Json)) {
                if ($entry.file -notmatch '^[a-zA-Z0-9-]+\.png$') { throw 'Invalid gallery filename.' }
                $label = ([string]$entry.label) -replace '[\[\]\r\n]', ' '
                $lines.Add('')
                $lines.Add("![$label](./$($result.id)/evidence/$($entry.file))")
            }
        }
    }
    $lines | Set-Content -LiteralPath "$RunRoot\summary.md" -Encoding UTF8
}

function Copy-LabEvidence {
    param($Session, [string]$GuestRoot, [string]$Destination)
    $null = New-Item -ItemType Directory -Path $Destination -Force
    # A locked transcript must not prevent screenshots/results being recovered.
    $files = @(Invoke-Command -Session $Session -ScriptBlock {
        param($root)
        Get-ChildItem -LiteralPath "$root\evidence" -File -Recurse -ErrorAction Stop | ForEach-Object {
            $_.FullName.Substring(("$root\evidence\").Length)
        }
    } -ArgumentList $GuestRoot)
    foreach ($relative in $files) {
        $destinationFile = Join-Path $Destination $relative
        $null = New-Item -ItemType Directory -Path (Split-Path -Parent $destinationFile) -Force
        $copyError = $null
        foreach ($attempt in 1..3) {
            try {
                Copy-Item -FromSession $Session -LiteralPath "$GuestRoot\evidence\$relative" -Destination $destinationFile -Force -ErrorAction Stop
                $copyError = $null
                break
            } catch { $copyError = $_.Exception.Message; if ($attempt -lt 3) { Start-Sleep -Milliseconds 500 } }
        }
        if ($copyError) { "${relative}: $copyError" }
    }
}

function Assert-LabVM {
    param([Parameter(Mandatory)]$Definition)
    # Resolve by exact name: do not let Hyper-V wildcard matching select other VMs.
    $matches = @(Get-VM -ErrorAction Stop | Where-Object Name -EQ $Definition.name)
    if ($matches.Count -ne 1) { throw "Expected exactly one VM named $($Definition.name)." }
    $vm = $matches[0]
    if ($vm.Notes -notmatch '(?m)^CCUM-DISPOSABLE-LAB\s*$') { throw "VM $($vm.Name) is missing the CCUM-DISPOSABLE-LAB notes marker." }
    $snapshots = @(Get-VMSnapshot -VM $vm | Where-Object Name -EQ $Definition.checkpoint)
    if ($snapshots.Count -ne 1) { throw "Expected exactly one checkpoint named $($Definition.checkpoint)." }
    if ([string]$snapshots[0].SnapshotType -ne 'Standard') { throw 'Use a standard checkpoint that preserves the signed-in desktop.' }
    # SnapshotType classifies user/recovery/replica snapshots, not the VM's
    # Standard/Production checkpoint setting. Production and powered-off
    # checkpoints have no desktop memory to resume.
    if ([string]$snapshots[0].State -notin @('Running', 'Saved', 'Paused')) {
        throw 'Use a checkpoint with saved desktop memory, captured while the guest is signed in and unlocked.'
    }
    [pscustomobject]@{ VM = $vm; Checkpoint = $snapshots[0] }
}

Export-ModuleMember -Function Read-LabConfig, Read-LabCatalog, Get-LabPlan, Assert-LabInputs, Assert-LabVM, New-LabResults, Write-LabReport, Copy-LabEvidence
