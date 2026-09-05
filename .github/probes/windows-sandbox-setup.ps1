# Only the fresh disposable CI VM is authorized for this setup experiment.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows') { throw 'Disposable Windows CI VM required' }
Write-Output ('OS version=' + [Environment]::OSVersion.Version + ' architecture=' + [Runtime.InteropServices.RuntimeInformation]::OSArchitecture)
$probeFeature = 'Containers-DisposableClientVM'
function Read-ProbeFeature {
    $probeStates = @(Get-WindowsOptionalFeature -Online -FeatureName 'Containers-DisposableClientVM' -ErrorAction Stop)
    if ($probeStates.Count -ne 1 -or [String]::IsNullOrWhiteSpace([String]$probeStates[0].State)) { throw 'Ambiguous or missing feature state' }
    return [String]$probeStates[0].State
}
$probeBefore = Read-ProbeFeature
Write-Output ('FEATURE before=' + $probeBefore)
if ($probeBefore -notin @('Disabled', 'Enabled')) { throw 'Unexpected pending component state; no mutation' }
$probeRoot = Join-Path $env:RUNNER_TEMP ('crucible-wsb-setup-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $probeRoot
$probeRestart = $false
if ($probeBefore -eq 'Disabled') {
    try {
        $probeResult = @(Enable-WindowsOptionalFeature -Online -FeatureName $probeFeature -All -NoRestart -LimitAccess -LogLevel Errors -LogPath (Join-Path $probeRoot 'servicing.log') -ErrorAction Stop)
        if ($probeResult.Count -ne 1 -or $null -eq $probeResult[0].RestartNeeded) { throw 'Missing or ambiguous servicing result' }
        $probeRestart = [bool]$probeResult[0].RestartNeeded
        Write-Output ('SETUP returned=true restart_needed=' + $probeRestart)
    } catch {
        Write-Output ('SETUP returned=false hresult=' + $_.Exception.HResult + ' reason=' + $_.Exception.Message)
        throw
    }
}
$probeAfter = Read-ProbeFeature
Write-Output ('FEATURE after=' + $probeAfter)
$probeSystem = [Environment]::GetFolderPath([Environment+SpecialFolder]::System)
foreach ($probeName in @('WindowsSandbox.exe','wsb.exe')) {
    $probePath = Join-Path $probeSystem $probeName
    Write-Output ('EXECUTABLE name=' + $probeName + ' present=' + (Test-Path -LiteralPath $probePath -PathType Leaf))
}
Write-Output ('COMPLETE guest_started=false rebooted=false feature_changed=' + ($probeBefore -ne $probeAfter) + ' restart_needed=' + $probeRestart + ' sandbox_validated=false')
