param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
function Get-DisplaySettingsWindow {
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Settings')
    return [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $condition)
}
function Find-DisplayCombo([string]$Pattern) {
    $window = Get-DisplaySettingsWindow
    if (-not $window) { return $null }
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::ComboBox)
    $elements = $window.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
    foreach ($element in $elements) {
        if (($element.Current.AutomationId + ' ' + $element.Current.Name) -match $Pattern -or
            ($Pattern -match 'Scal' -and $element.Current.Name -match '^\d+%')) { return $element }
    }
    return $null
}
function Set-DisplayChoice([string]$Pattern, [string]$Choice) {
    $ready = Wait-Condition { $null -ne (Find-DisplayCombo $Pattern) } 20
    Assert-Check 'Windows display control available' $ready $Pattern
    $enabled = Wait-Condition { $control = Find-DisplayCombo $Pattern; $control -and $control.Current.IsEnabled } 20
    Assert-Check 'Windows display control enabled' $enabled 'If resolution is disabled, capture a checkpoint booted with Hyper-V ResolutionType Maximum.'
    $combo = Find-DisplayCombo $Pattern
    $combo.GetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern).Expand()
    Start-Sleep -Milliseconds 500
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::ListItem)
    $items = (Get-DisplaySettingsWindow).FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
    $target = @($items | Where-Object { $_.Current.Name -match $Choice -and -not $_.Current.IsOffscreen })
    Assert-Check 'requested display choice supported' ($target.Count -eq 1) @{choice=$Choice; available=@($items | ForEach-Object { $_.Current.Name })}
    $target[0].GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Select()
    Start-Sleep -Seconds 2
    # Windows offers a confirmation after resolution changes. Invoke by AutomationId
    # where possible; the English label is a fallback for English lab checkpoints.
    $buttons = (Get-DisplaySettingsWindow).FindAll([Windows.Automation.TreeScope]::Descendants,
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button))
    foreach ($button in $buttons) {
        if ($button.Current.AutomationId -match 'KeepChanges' -or $button.Current.Name -eq 'Keep changes') { $button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke() }
    }
}
