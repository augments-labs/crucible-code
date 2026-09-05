# Query one preinstalled service; preserve its stable original state.
$ErrorActionPreference = 'Stop'
$service = Get-Service -Name docker -ErrorAction Stop
$original = $service.Status
$started = $false
try {
    Write-Output "DOCKER-SERVICE original=$original"
    if ($original -ne 'Stopped' -and $original -ne 'Running') {
        throw 'Original Docker service state is transitional or unsupported; no mutation'
    }
    if ($original -eq 'Stopped') {
        $started = $true
        Start-Service -Name docker -ErrorAction Stop
        $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(30))
    }
    Write-Output "DOCKER-SERVICE started=$started"
    python .github/probes/windows-container-query.py
    if ($LASTEXITCODE -ne 0) { throw 'Bounded local query failed' }
} finally {
    try {
        if ($started) {
            $service.Refresh()
            if ($service.Status -ne 'Stopped') { Stop-Service -Name docker -ErrorAction Stop }
            $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(30))
            $service.Refresh()
            if ($service.Status -ne 'Stopped') { throw 'QUARANTINE service restoration unknown' }
            Write-Output 'DOCKER-SERVICE restored=Stopped'
        } elseif ($original -eq 'Running') {
            $service.Refresh()
            if ($service.Status -ne 'Running') { throw 'Originally running service changed state; no mutation attempted' }
            Write-Output 'DOCKER-SERVICE preserved=Running'
        }
    } finally {
        $service.Dispose()
    }
}
Write-Output 'CONTAINER-SERVICE-PREREQUISITE-COMPLETE guest_started=false image_pulled=false sandbox_validated=false'
