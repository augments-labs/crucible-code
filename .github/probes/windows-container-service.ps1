# Start/restore one preinstalled service on an otherwise unused disposable VM.
$ErrorActionPreference = 'Stop'
$service = Get-Service -Name docker -ErrorAction Stop
if ($service.Status -ne 'Stopped') { throw 'Original Docker service state is not Stopped; no mutation' }
$started = $false
try {
    $started = $true
    Start-Service -Name docker -ErrorAction Stop
    $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(30))
    Write-Output 'DOCKER-SERVICE started=true'
    python .github/probes/windows-container-query.py
    if ($LASTEXITCODE -ne 0) { throw 'Bounded local query failed' }
} finally {
    if ($started) {
        $service.Refresh()
        if ($service.Status -ne 'Stopped') { Stop-Service -Name docker -ErrorAction Stop }
        $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(30))
        $service.Refresh()
        if ($service.Status -ne 'Stopped') { throw 'QUARANTINE service restoration unknown' }
        Write-Output 'DOCKER-SERVICE restored=Stopped'
    }
    $service.Dispose()
}
Write-Output 'CONTAINER-SERVICE-PREREQUISITE-COMPLETE guest_started=false image_pulled=false sandbox_validated=false'
