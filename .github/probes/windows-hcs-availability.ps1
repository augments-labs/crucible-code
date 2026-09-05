# Fixed read-only prerequisite queries; no container or system state changes.
$ErrorActionPreference = 'Stop'
./.github/probes/windows-native-availability.ps1
try {
    $states = @(Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -ErrorAction Stop)
    if ($states.Count -ne 1) { throw 'feature result' }
    Write-Output ('SERVER-FEATURE name=Microsoft-Hyper-V state=' + $states[0].State)
} catch { Write-Output 'SERVER-FEATURE name=Microsoft-Hyper-V query_unavailable=true' }
foreach ($name in @('vmcompute', 'docker')) {
    try {
        $services = @(Get-Service -Name $name -ErrorAction Stop)
        if ($services.Count -ne 1) { throw 'service result' }
        Write-Output ('SERVICE name=' + $name + ' status=' + $services[0].Status)
    } catch { Write-Output ('SERVICE name=' + $name + ' query_unavailable=true') }
}
$docker = Get-Command docker.exe -CommandType Application -ErrorAction SilentlyContinue
if (-not $docker) { Write-Output 'DOCKER local_query_unavailable=true' } else {
    $process = [Diagnostics.Process]::new()
    $started = $false
    try {
        $process.StartInfo.FileName = $docker.Source
        $process.StartInfo.UseShellExecute = $false
        $process.StartInfo.RedirectStandardOutput = $true
        $process.StartInfo.RedirectStandardError = $true
        foreach ($arg in @('-H', 'npipe:////./pipe/docker_engine', 'info', '--format', '{{.OSType}} {{.Driver}}')) { [void]$process.StartInfo.ArgumentList.Add($arg) }
        [void]$process.Start()
        $started = $true
        # The fixed Docker template is tiny. Bound reads before storing output;
        # discard stderr bytes and never expose Docker config or environment.
        $stdoutBuffer = [char[]]::new(4097)
        $stdout = $process.StandardOutput.ReadAsync($stdoutBuffer, 0, 4097)
        $stderrBuffer = [char[]]::new(4097)
        $stderr = $process.StandardError.ReadAsync($stderrBuffer, 0, 4097)
        if (-not $process.WaitForExit(15000)) { $process.Kill(); $process.WaitForExit(); throw 'local query timeout' }
        $count = $stdout.GetAwaiter().GetResult()
        $errorCount = $stderr.GetAwaiter().GetResult()
        Write-Output ('DOCKER query_exit=' + $process.ExitCode + ' output_characters=' + $count + ' error_characters=' + $errorCount)
        if ($count -ge 4097 -or $errorCount -ge 4097 -or $process.StandardOutput.Read() -ne -1 -or $process.StandardError.Read() -ne -1) { throw 'query output incomplete or exceeded bound' }
        $value = (-join $stdoutBuffer[0..([Math]::Max(0, $count - 1))]).Trim()
        if ($process.ExitCode -eq 0 -and $value -match '^(windows|linux) ([a-zA-Z0-9_-]{1,64})$') {
            Write-Output ('DOCKER server_os=' + $Matches[1] + ' driver=' + $Matches[2])
        } else { Write-Output 'DOCKER server_fields_unavailable=true' }
    } catch { Write-Output ('DOCKER query_failed=true type=' + $_.Exception.GetType().Name) }
    finally {
        if ($started -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $process.Dispose()
    }
}
Write-Output 'HCS-INVENTORY-COMPLETE state_changed=false guest_started=false sandbox_validated=false'
