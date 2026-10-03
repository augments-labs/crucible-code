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

# Runs the installer, or `Script` when given, with `Arguments`, its output
# redirected to files, and returns its status and what it printed on each
# stream.
function Invoke-Installer([string[]]$Arguments, [string]$Script = $installer) {
    $out = Join-Path $root 'stdout.txt'
    $err = Join-Path $root 'stderr.txt'
    $quoted = @($Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_ + '"' } else { $_ } })
    $line = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$Script`" " + ($quoted -join ' ')
    $process = Start-Process -FilePath $shell -ArgumentList $line -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $out -RedirectStandardError $err
    return [pscustomobject]@{
        Status = $process.ExitCode
        Out = [IO.File]::ReadAllText($out)
        Err = [IO.File]::ReadAllText($err)
    }
}

# Windows PowerShell 5.1 reads a script with no byte order mark in the
# system's ANSI code page, so every byte of these two files is ASCII.
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

# The user's PATH as this run found it, value and registry kind, read before
# any install so that one changing it shows.
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
function Get-UserPath {
    if ($key.GetValueNames() -notcontains 'Path') { return $null }
    return [pscustomobject]@{
        Value = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        Kind = $key.GetValueKind('Path')
    }
}
$foundPath = Get-UserPath
function Test-UserPathAsFound {
    $now = Get-UserPath
    if ($null -eq $foundPath) { return $null -eq $now }
    return $null -ne $now -and $now.Value -ceq $foundPath.Value -and $now.Kind -eq $foundPath.Kind
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
    Assert-Lacks $run.Err ' replace ' 'redirected install'

    # A directory other users can change is named, with the one inside it
    # that inherits their access, and the install still lands.
    $shared = Join-Path $root 'shared'
    $null = New-Item -ItemType Directory -Path $shared
    $null = & icacls.exe $shared /grant '*S-1-5-32-545:(OI)(CI)M'
    if ($LASTEXITCODE -ne 0) { Stop-Test 'could not let Users modify the shared directory' }
    $sharedBin = Join-Path $shared 'bin'
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $sharedBin))
    if ($run.Status -ne 0) { Stop-Test "an install below a shared directory exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Err "install: $shared is writable by other users, who could replace " 'shared directory'
    Assert-Contains $run.Err "install: $sharedBin is writable by other users, who could replace " 'shared directory'
    if (-not (Test-Path -LiteralPath (Join-Path $sharedBin 'crucible.exe') -PathType Leaf)) {
        Stop-Test 'an install below a shared directory did not land'
    }

    # An entry that passes only to the files in a directory still lets other
    # users replace what lands there, so the directory is named for it.
    $passed = Join-Path $root 'passed'
    $null = New-Item -ItemType Directory -Path $passed
    $null = & icacls.exe $passed /grant '*S-1-5-32-545:(OI)(IO)M'
    if ($LASTEXITCODE -ne 0) { Stop-Test 'could not let Users modify the files in a directory' }
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $passed))
    if ($run.Status -ne 0) { Stop-Test "an install into a directory passing access to its files exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Err "install: $passed is writable by other users, who could replace " 'access passed to files'
    Assert-Lacks $run.Err "install: $root " 'access passed to files'
    if (-not (Test-Path -LiteralPath (Join-Path $passed 'crucible.exe') -PathType Leaf)) {
        Stop-Test 'an install into a directory passing access to its files did not land'
    }

    # Installing again replaces what the last install put there, and removes
    # what an earlier one moved aside and could not remove, by its exact name.
    $leftover = Join-Path $dir ('.crucible.exe.previous.' + [Guid]::NewGuid().ToString('N'))
    [IO.File]::WriteAllText($leftover, 'replaced')
    $mine = Join-Path $dir '.crucible.exe.previous.mine'
    [IO.File]::WriteAllText($mine, 'kept')
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $dir))
    if ($run.Status -ne 0) { Stop-Test "a second install exited $($run.Status): $($run.Err)" }
    if (Test-Path -LiteralPath $leftover) { Stop-Test 'the second install kept a copy an earlier one moved aside' }
    if (-not (Test-Path -LiteralPath $mine)) { Stop-Test 'the second install removed a file it did not name' }
    Remove-Item -LiteralPath $mine
    if (@(Get-ChildItem -LiteralPath $dir -Force).Count -ne 3) { Stop-Test 'the second install left files behind' }

    # One still in use stays, and the install says so.
    $held = [IO.File]::Open($leftover, [IO.FileMode]::Create, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $dir))
    } finally {
        $held.Dispose()
    }
    if ($run.Status -ne 0) { Stop-Test "an install beside a copy in use exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Err 'install: could not remove ' 'a copy in use'
    Assert-Contains $run.Err "$(Split-Path -Leaf $leftover), a replaced copy still in use" 'a copy in use'
    Remove-Item -LiteralPath $leftover

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

    # SHA256SUMS lines are matched to the archive by exact name. A line for
    # another file whose name differs only by U+00AD SOFT HYPHEN, which a
    # culture-aware comparison ignores, is not a second line for this one.
    $hyphenSums = Join-Path $root 'SHA256SUMS.hyphen'
    [IO.File]::WriteAllText($hyphenSums,
        "$hash  $stem.tar.gz`n" + ('0' * 64) + "  $([char]0x00AD)$stem.tar.gz`n", (New-Object Text.UTF8Encoding $true))
    $run = Invoke-Installer ($release + @('-Checksums', $hyphenSums, '-Dir', (Join-Path $root 'hyphen-sums')))
    if ($run.Status -ne 0) { Stop-Test "a checksum line for a name with a soft hyphen exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Out 'install: verify checksum: ok' 'soft hyphen in SHA256SUMS'

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

    # A reason never hands the console a control character to act on.
    $run = Invoke-Installer @('-Version', "9.8.7$esc[31m")
    if ($run.Status -ne 2) { Stop-Test "a version with an escape sequence exited $($run.Status)" }
    Assert-Contains $run.Err 'install: invalid version 9.8.7?[31m' 'escape sequence in a reason'
    Assert-Lacks $run.Err $esc 'escape sequence in a reason'

    # A version holds ASCII letters only. U+212A KELVIN SIGN folds to k in a
    # match that ignores case, so it is refused only by one that does not.
    # The archive and checksums are local, so a version let through installs
    # from them rather than reaching the network.
    $kelvin = Join-Path $root 'kelvin'
    $run = Invoke-Installer @('-Version', "$version-$([char]0x212A)", '-Archive', $archive,
        '-Checksums', $sums, '-Dir', $kelvin)
    if ($run.Status -ne 2) { Stop-Test "a version holding a Kelvin sign exited $($run.Status)" }
    Assert-Contains $run.Err "install: invalid version $version-" 'Kelvin sign in a version'
    if (Test-Path -LiteralPath $kelvin) { Stop-Test 'a refused version created the installation directory' }

    # A version is matched to its very end, so one with a newline after it is
    # refused. Passed from a script, since a command line would not keep it.
    $newline = Join-Path $root 'newline'
    $trailing = Join-Path $root 'trailing.ps1'
    [IO.File]::WriteAllText($trailing, @'
param([string]$Installer, [string]$Version, [string]$Archive, [string]$Checksums, [string]$Dir)
& $Installer -Version "$Version`n" -Archive $Archive -Checksums $Checksums -Dir $Dir
exit $LASTEXITCODE
'@)
    $run = Invoke-Installer @('-Installer', $installer, '-Version', $version, '-Archive', $archive,
        '-Checksums', $sums, '-Dir', $newline) $trailing
    if ($run.Status -ne 2) { Stop-Test "a version ending in a newline exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Err "install: invalid version $version" 'newline after a version'
    if (Test-Path -LiteralPath $newline) { Stop-Test 'a version ending in a newline created the installation directory' }

    # Run as a script block in the caller's PowerShell, as the documented
    # `& ([scriptblock]::Create((irm ...)))` runs it, the installer leaves
    # its status in LASTEXITCODE and returns to the caller, which goes on to
    # choose its own exit status.
    $caller = Join-Path $root 'caller.ps1'
    [IO.File]::WriteAllText($caller, @'
param([string]$Installer, [string]$Version, [string]$Archive, [string]$Checksums, [string]$Dir)
$global:LASTEXITCODE = 99
& ([scriptblock]::Create([IO.File]::ReadAllText($Installer))) -Version $Version -Archive $Archive -Checksums $Checksums -Dir $Dir
[Console]::Out.WriteLine("returned with $LASTEXITCODE")
exit 7
'@)
    $block = Join-Path $root 'script-block'
    $run = Invoke-Installer @('-Installer', $installer, '-Version', $version, '-Archive', $archive,
        '-Checksums', $sums, '-Dir', $block) $caller
    if ($run.Status -ne 7) { Stop-Test "the installer as a script block ended its caller with $($run.Status): $($run.Err)" }
    Assert-Contains $run.Out 'returned with 0' 'script block'
    foreach ($file in 'crucible.exe', 'crucible-sandbox-broker.exe', 'cru.exe') {
        if (-not (Test-Path -LiteralPath (Join-Path $block $file) -PathType Leaf)) {
            Stop-Test "the installer as a script block did not install $file"
        }
    }

    # Every install above ran without -AddToPath, and left PATH alone.
    if (-not (Test-UserPathAsFound)) { Stop-Test 'an install without -AddToPath changed the user PATH' }

    # -AddToPath puts the directory first on the user's PATH and keeps the
    # value's registry kind.
    $added = Join-Path $root 'added'
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $added, '-AddToPath'))
    if ($run.Status -ne 0) { Stop-Test "an install with -AddToPath exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Out "Added $added to your user PATH; open a new terminal to run crucible." '-AddToPath'
    # Proves Test-UserPathAsFound can see a change, so the earlier pass was
    # not vacuous.
    if (Test-UserPathAsFound) { Stop-Test 'the user PATH reads as found after -AddToPath changed it' }
    $wanted = $added
    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($null -ne $foundPath) {
        if ($foundPath.Value) { $wanted = "$added;$($foundPath.Value)" }
        $kind = $foundPath.Kind
    }
    $now = Get-UserPath
    if ($null -eq $now) { Stop-Test '-AddToPath left no user PATH' }
    if ($now.Value -cne $wanted) { Stop-Test "-AddToPath left the user PATH as '$($now.Value)'" }
    if ($now.Kind -ne $kind) { Stop-Test '-AddToPath changed the registry kind of the user PATH' }
    # A second run finds the directory on PATH and adds nothing.
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $added, '-AddToPath'))
    if ($run.Status -ne 0) { Stop-Test "a second install with -AddToPath exited $($run.Status): $($run.Err)" }
    Assert-Lacks $run.Out 'PATH' 'second -AddToPath'
    $now = Get-UserPath
    if ($null -eq $now -or $now.Value -cne $wanted) { Stop-Test 'a second -AddToPath changed the user PATH again' }

    # A PATH entry is the directory only when their names agree ignoring case
    # alone: one that differs by a soft hyphen is another directory.
    $hyphen = Join-Path $root 'hyphen-path'
    $key.SetValue('Path', "$hyphen$([char]0x00AD)", [Microsoft.Win32.RegistryValueKind]::ExpandString)
    $run = Invoke-Installer ($release + @('-Checksums', $sums, '-Dir', $hyphen, '-AddToPath'))
    if ($run.Status -ne 0) { Stop-Test "an install beside a PATH entry with a soft hyphen exited $($run.Status): $($run.Err)" }
    Assert-Contains $run.Out "Added $hyphen to your user PATH" 'soft hyphen in PATH'
} finally {
    $env:NO_COLOR = $savedNoColor
    if (-not (Test-UserPathAsFound)) {
        if ($null -ne $foundPath) {
            $key.SetValue('Path', $foundPath.Value, $foundPath.Kind)
        } else {
            $key.DeleteValue('Path', $false)
        }
    }
    $key.Dispose()
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
[Console]::Out.WriteLine('install-tests.ps1: ok')
