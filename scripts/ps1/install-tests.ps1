# Runs scripts/ps1/install.ps1 against a local release archive built here, in
# the PowerShell running this file, and checks what it installs and prints.
# CI runs it once under Windows PowerShell 5.1 and once under PowerShell 7.
# Nothing is downloaded; everything is written below a temporary directory,
# except the -AddToPath case, which puts the user's PATH back as it found it.
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$installer = Join-Path $PSScriptRoot 'install.ps1'
$shell = (Get-Process -Id $PID).Path
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
$esc = [string][char]27
$version = '9.8.7'

function Stop-Test([string]$Message) {
    [Console]::Error.WriteLine("install-tests.ps1: $Message")
    exit 1
}

function Assert-Contains([string]$Text, [string]$Wanted, [string]$Case) {
    if (-not $Text.Contains($Wanted)) { Stop-Test "$Case`: expected '$Wanted' in:`n$Text" }
}

function Assert-Lacks([string]$Text, [string]$Unwanted, [string]$Case) {
    if ($Text.Contains($Unwanted)) { Stop-Test "$Case`: did not expect '$Unwanted' in:`n$Text" }
}

# Runs the installer with `Arguments`, its output redirected to files, and
# returns its status and what it printed on each stream.
function Invoke-Installer([string[]]$Arguments) {
    $out = Join-Path $root 'stdout.txt'
    $err = Join-Path $root 'stderr.txt'
    $quoted = @($Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_ + '"' } else { $_ } })
    $line = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$installer`" " + ($quoted -join ' ')
    $process = Start-Process -FilePath $shell -ArgumentList $line -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $out -RedirectStandardError $err
    return [pscustomobject]@{
        Status = $process.ExitCode
        Out = [IO.File]::ReadAllText($out)
        Err = [IO.File]::ReadAllText($err)
    }
}

# The installer is read as the system's ANSI code page by Windows PowerShell
# 5.1 when it has no byte order mark, so every byte of it is ASCII.
foreach ($script in $installer, $PSCommandPath) {
    if (@([IO.File]::ReadAllBytes($script) | Where-Object { $_ -gt 127 }).Count -ne 0) {
        Stop-Test "$script contains a byte that is not ASCII"
    }
}

$architecture = ''
try {
    $machine = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
} catch {
    $machine = $env:PROCESSOR_ARCHITEW6432
    if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
}
switch ($machine.ToUpperInvariant()) {
    'X64' { $architecture = 'x86_64' }
    'AMD64' { $architecture = 'x86_64' }
    'ARM64' { $architecture = 'aarch64' }
    default { Stop-Test "no fixture for architecture $machine" }
}

$root = Join-Path ([IO.Path]::GetTempPath()) ('crucible-install-tests-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $root
$savedNoColor = $env:NO_COLOR
try {
    # A release archive as release.yml packages one, with stand-in executables.
    $platform = "windows-$architecture"
    $stem = "crucible-$version-$platform"
    $staging = Join-Path $root 'staging'
    $null = New-Item -ItemType Directory -Path (Join-Path $staging $stem)
    foreach ($file in 'crucible.exe', 'crucible-sandbox-broker.exe', 'README.md', 'LICENSE', 'install.sh', 'uninstall.sh') {
        [IO.File]::WriteAllText((Join-Path $staging "$stem\$file"), "$file $version")
    }
    $archive = Join-Path $root "$stem.tar.gz"
    & $tar -czf $archive -C $staging $stem
    if ($LASTEXITCODE -ne 0) { Stop-Test 'could not build the fixture archive' }
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    $sums = Join-Path $root 'SHA256SUMS'
    [IO.File]::WriteAllText($sums, "$hash  $stem.tar.gz`n")
    $wrongSums = Join-Path $root 'SHA256SUMS.wrong'
    [IO.File]::WriteAllText($wrongSums, ('0' * 64) + "  $stem.tar.gz`n")
    $release = @('-Version', $version, '-Archive', $archive)

    # Redirected output gets one plain line per step and no escape sequences.
    $dir = Join-Path $root 'bin'
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $dir))
    if ($run.Status -ne 0) { Stop-Test "a verified install exited $($run.Status): $($run.Err)" }
    Assert-Lacks $run.Out $esc 'redirected install'
    Assert-Contains $run.Out "install: crucible $version for $platform" 'redirected install'
    Assert-Contains $run.Out "install: detect platform: $platform" 'redirected install'
    Assert-Contains $run.Out 'install: verify checksum: ok' 'redirected install'
    Assert-Contains $run.Out 'install: unpack: ok' 'redirected install'
    Assert-Contains $run.Out "install: install: $dir" 'redirected install'
    Assert-Contains $run.Out "Installed crucible.exe, crucible-sandbox-broker.exe and cru.exe in $dir" 'redirected install'
    Assert-Contains $run.Out "Add $dir to PATH to run crucible." 'redirected install'
    foreach ($file in 'crucible.exe', 'crucible-sandbox-broker.exe', 'cru.exe') {
        if (-not (Test-Path -LiteralPath (Join-Path $dir $file) -PathType Leaf)) { Stop-Test "$file was not installed" }
    }
    if ([IO.File]::ReadAllText((Join-Path $dir 'cru.exe')) -ne "crucible.exe $version") {
        Stop-Test 'cru.exe is not a copy of crucible.exe'
    }
    if (@(Get-ChildItem -LiteralPath $dir -Force).Count -ne 3) { Stop-Test 'the install left files beside the executables' }

    # Installing again replaces what the last install put there.
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $dir))
    if ($run.Status -ne 0) { Stop-Test "a second install exited $($run.Status): $($run.Err)" }
    if (@(Get-ChildItem -LiteralPath $dir -Force).Count -ne 3) { Stop-Test 'the second install left files behind' }

    # NO_COLOR, like a redirect, gets no escape sequences.
    $env:NO_COLOR = '1'
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', (Join-Path $root 'no-color')))
    $env:NO_COLOR = $savedNoColor
    if ($run.Status -ne 0) { Stop-Test "an install under NO_COLOR exited $($run.Status): $($run.Err)" }
    Assert-Lacks $run.Out $esc 'NO_COLOR install'
    Assert-Lacks $run.Err $esc 'NO_COLOR install'
    Assert-Contains $run.Out 'install: verify checksum: ok' 'NO_COLOR install'

    # A checksum mismatch names the step, says why, and installs nothing.
    $refused = Join-Path $root 'refused'
    $run = Invoke-Installer ($release + @('-Checksums', $wrongSums, '-Dir', $refused))
    if ($run.Status -ne 1) { Stop-Test "a checksum mismatch exited $($run.Status)" }
    Assert-Contains $run.Out 'install: verify checksum: failed' 'checksum mismatch'
    Assert-Contains $run.Err 'install: archive checksum does not match SHA256SUMS' 'checksum mismatch'
    Assert-Lacks $run.Out 'install: unpack' 'checksum mismatch'
    if (Test-Path -LiteralPath $refused) { Stop-Test 'a checksum mismatch created the installation directory' }

    # A cru.exe that is not this crucible.exe is someone else's, and stays.
    $taken = Join-Path $root 'taken'
    $null = New-Item -ItemType Directory -Path $taken
    [IO.File]::WriteAllText((Join-Path $taken 'cru.exe'), 'another program')
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $taken))
    if ($run.Status -ne 1) { Stop-Test "an unrelated cru.exe let the install exit $($run.Status)" }
    Assert-Contains $run.Out 'install: install: failed' 'unrelated cru.exe'
    Assert-Contains $run.Err 'refusing to replace unrelated' 'unrelated cru.exe'
    if (@(Get-ChildItem -LiteralPath $taken -Force).Count -ne 1 -or
        [IO.File]::ReadAllText((Join-Path $taken 'cru.exe')) -ne 'another program') {
        Stop-Test 'a refused install changed the directory'
    }

    # A dry run says what it would do and changes nothing.
    $dry = Join-Path $root 'dry'
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $dry, '-DryRun'))
    if ($run.Status -ne 0) { Stop-Test "a dry run exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Out "install: install: $dry (dry run)" 'dry run'
    Assert-Contains $run.Out "Would install crucible $version in $dry" 'dry run'
    if (Test-Path -LiteralPath $dry) { Stop-Test 'a dry run created the installation directory' }

    # Arguments the installer does not know, and an archive without its
    # checksums, are refused with status 2 before anything else happens.
    $run = Invoke-Installer @('-Bogus')
    if ($run.Status -ne 2) { Stop-Test "an unknown argument exited $($run.Status)" }
    Assert-Contains $run.Err 'install: unknown argument -Bogus' 'unknown argument'
    $run = Invoke-Installer $release
    if ($run.Status -ne 2) { Stop-Test "an archive without checksums exited $($run.Status)" }
    Assert-Contains $run.Err 'install: -Archive requires -Checksums and -Version' 'archive without checksums'

    # PATH is left alone unless -AddToPath asks, which puts the directory first
    # on the user's PATH and keeps the value's registry kind.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    $hadPath = $key.GetValueNames() -contains 'Path'
    $savedPath = $null
    $savedKind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($hadPath) {
        $savedPath = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $savedKind = $key.GetValueKind('Path')
    }
    try {
        if ([string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) -ne
            [string]$savedPath) {
            Stop-Test 'an install without -AddToPath changed the user PATH'
        }
        $added = Join-Path $root 'added'
        $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $added, '-AddToPath'))
        if ($run.Status -ne 0) { Stop-Test "an install with -AddToPath exited $($run.Status): $($run.Err)" }
        Assert-Contains $run.Out "Added $added to your user PATH; open a new terminal to run crucible." '-AddToPath'
        $now = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $wanted = $added
        if ($savedPath) { $wanted = "$added;$savedPath" }
        if ($now -ne $wanted) { Stop-Test "-AddToPath left the user PATH as '$now'" }
        if ($key.GetValueKind('Path') -ne $savedKind) { Stop-Test '-AddToPath changed the registry kind of the user PATH' }
        # A second run finds the directory on PATH and adds nothing.
        $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $added, '-AddToPath'))
        if ($run.Status -ne 0) { Stop-Test "a second install with -AddToPath exited $($run.Status): $($run.Err)" }
        Assert-Lacks $run.Out 'PATH' 'second -AddToPath'
        if ([string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) -ne
            $wanted) {
            Stop-Test 'a second -AddToPath added the directory again'
        }
    } finally {
        if ($hadPath) { $key.SetValue('Path', $savedPath, $savedKind) } else { $key.DeleteValue('Path', $false) }
        $key.Dispose()
    }
} finally {
    $env:NO_COLOR = $savedNoColor
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
[Console]::Out.WriteLine('install-tests.ps1: ok')
