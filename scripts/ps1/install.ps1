<#
.SYNOPSIS
Installs one verified crucible release for the current user on Windows.

.DESCRIPTION
Downloads the release archive for this machine and the release's SHA256SUMS,
verifies the archive before unpacking it, and installs crucible.exe, the
crucible-sandbox-broker.exe it confines commands with, and a cru.exe copy in a
per-user directory. It never asks for elevation, and changes PATH only when
-AddToPath asks it to.

Straight from the release:

    irm https://github.com/augments-labs/crucible-code/releases/latest/download/install.ps1 | iex

With options:

    & ([scriptblock]::Create((irm https://github.com/augments-labs/crucible-code/releases/latest/download/install.ps1))) -AddToPath

.PARAMETER Version
The release to install, such as 0.45.0. The latest release when omitted.

.PARAMETER Dir
Where the executables go. CRUCIBLE_INSTALL_DIR when set, otherwise
%LOCALAPPDATA%\Programs\crucible\bin.

.PARAMETER Archive
A local release archive to install instead of downloading one. Requires
-Checksums and -Version.

.PARAMETER Checksums
The SHA256SUMS file the local archive is verified against.

.PARAMETER DryRun
Verify and unpack, say what would be installed, and change nothing.

.PARAMETER AddToPath
Put the directory at the front of the user's PATH.
#>
param(
    [string]$Version = '',
    [string]$Dir = '',
    [string]$Archive = '',
    [string]$Checksums = '',
    [switch]$DryRun,
    [switch]$AddToPath
)

# Everything runs inside this function, so the strict mode, error preference
# and variables it sets stay inside it. Run through `iex`, which shares the
# caller's scope, this text still defines the parameters above and the function
# itself there; the lines at the end remove the variable they keep. It returns
# the exit status.
function Invoke-CrucibleInstall {
    param(
        [string]$Version,
        [string]$Dir,
        [string]$Archive,
        [string]$Checksums,
        [bool]$DryRun,
        [bool]$AddToPath,
        [object[]]$Unknown
    )
    Set-StrictMode -Version 3.0
    $ErrorActionPreference = 'Stop'

    $releases = 'https://github.com/augments-labs/crucible-code/releases'
    $esc = [char]27

    # What is printed, and nothing else, depends on where it goes. A console
    # that understands escape sequences gets the step list: a mark per step,
    # colour, a spinner while one runs and a bar while the archive downloads.
    # Redirected output, NO_COLOR or TERM=dumb gets one plain `install:` line
    # per step and the error on standard error.
    $fancy = (-not [Console]::IsOutputRedirected) -and (-not [Console]::IsErrorRedirected) -and
        [string]::IsNullOrEmpty($env:NO_COLOR) -and ($env:TERM -ne 'dumb')
    if ($fancy) {
        try { $fancy = [bool]$Host.UI.SupportsVirtualTerminal } catch { $fancy = $false }
    }
    # A console that cannot show the marks gets the ASCII set.
    if ([Console]::OutputEncoding.CodePage -eq 65001) {
        $frames = @([string][char]0x2733, [string][char]0x273B, [string][char]0x273A, [string][char]0x2731)
        $doneMark = [string][char]0x2713
        $failMark = [string][char]0x2717
        $dot = [string][char]0x00B7
        $fill = [string][char]0x2588
        $rest = [string][char]0x2591
    } else {
        $frames = @('|', '/', '-', '\')
        $doneMark = 'ok'
        $failMark = 'x'
        $dot = '-'
        $fill = '#'
        $rest = '.'
    }
    $markWidth = $doneMark.Length
    $bold = "$esc[1m"
    $dim = "$esc[2m"
    $red = "$esc[31m"
    $green = "$esc[32m"
    $plain = "$esc[0m"
    $columns = 80
    if ($fancy) {
        try { $columns = [int]$Host.UI.RawUI.WindowSize.Width } catch { $columns = 80 }
        if ($columns -lt 20) { $columns = 80 }
    }
    # The detail of a step starts here, after the indent, the mark and the label.
    $detailColumn = 2 + $markWidth + 1 + 20
    # Narrow is set by the banner, which decides once where every detail goes.
    $ui = @{ Step = ''; Frame = 0; Drawn = [DateTime]::MinValue; Status = 0; Narrow = $false }

    function Write-Out([string]$Text) { [Console]::Out.Write($Text) }

    # Text wrapped to the console at spaces, each row indented by `Indent`.
    function Get-Wrapped([string]$Indent, [string]$Text) {
        $room = [Math]::Max(10, $columns - $Indent.Length)
        $rows = New-Object System.Collections.Generic.List[string]
        $row = ''
        foreach ($word in ($Text -split ' ')) {
            while ($word.Length -gt $room) {
                if ($row) { $rows.Add($row); $row = '' }
                $rows.Add($word.Substring(0, $room))
                $word = $word.Substring($room)
            }
            if (-not $row) { $row = $word }
            elseif ($row.Length + 1 + $word.Length -le $room) { $row = "$row $word" }
            else { $rows.Add($row); $row = $word }
        }
        if ($row) { $rows.Add($row) }
        return (($rows | ForEach-Object { "$Indent$_" }) -join [Environment]::NewLine)
    }

    # One step's row: the mark, the label, then the detail in its colour. In
    # a narrow console, and for a detail that does not fit beside the label,
    # the detail goes under it, with each part between its dots on rows of its
    # own.
    function Write-Row([string]$Mark, [string]$Tint, [string]$Detail) {
        Write-Out ("`r$esc[K  " + $Tint + $Mark.PadLeft($markWidth) + $plain + ' ' + $ui.Step)
        if (-not $Detail) {
            Write-Out ([Environment]::NewLine)
        } elseif (-not $ui.Narrow -and $detailColumn + $Detail.Length -le $columns) {
            $detailTint = $dim
            if ($Tint -eq $red) { $detailTint = $red }
            Write-Out ((' ' * (20 - $ui.Step.Length)) + $detailTint + $Detail + $plain + [Environment]::NewLine)
        } else {
            Write-Out ([Environment]::NewLine)
            $detailTint = $dim
            if ($Tint -eq $red) { $detailTint = $red }
            foreach ($part in ($Detail -split [regex]::Escape(" $dot "))) {
                Write-Out ($detailTint + (Get-Wrapped '    ' $part) + $plain + [Environment]::NewLine)
            }
        }
    }

    # Redraws the running step with its next spinner frame, at most ten times
    # a second, followed by `Progress` when there is one. With no step running,
    # as while the latest release is looked up before the banner, the line
    # stays blank.
    function Show-Frame([string]$Progress) {
        if (-not $fancy -or -not $ui.Step) { return }
        $now = [DateTime]::UtcNow
        if (($now - $ui.Drawn).TotalMilliseconds -lt 100) { return }
        $ui.Drawn = $now
        $frame = $frames[$ui.Frame % $frames.Count]
        $ui.Frame++
        $gap = [Math]::Max(1, 20 - $ui.Step.Length)
        Write-Out ("`r$esc[K  " + $frame.PadLeft($markWidth) + ' ' + $ui.Step + (' ' * $gap) + $Progress)
    }

    function Get-Megabytes([long]$Bytes) {
        return ('{0:0.0} MB' -f ($Bytes / 1000000.0)).Replace(',', '.')
    }

    # The download so far against the size its response announced.
    function Get-Bar([long]$Got, [long]$Total) {
        $sizes = ('{0:0.0} / {1:0.0} MB' -f ($Got / 1000000.0), ($Total / 1000000.0)).Replace(',', '.')
        $room = [Math]::Min(28, $columns - $detailColumn - 2 - $sizes.Length)
        if ($room -lt 4) {
            if ($detailColumn + $sizes.Length -le $columns) { return "$dim$sizes$plain" }
            return ''
        }
        $filled = [int][Math]::Floor($Got * $room / $Total)
        return ($fill * $filled) + $dim + ($rest * ($room - $filled)) + $plain + "  $dim$sizes$plain"
    }

    # A console is narrow when the download's row, the widest a run draws,
    # could not hold its detail beside its label.
    function Write-Banner([string]$Platform) {
        $widest = "crucible-$Version-$Platform.tar.gz $dot 999.9 MB"
        $ui.Narrow = $detailColumn + $widest.Length -gt $columns
        if ($fancy) {
            $suffix = ''
            if ($Platform) { $suffix = " $dot $Platform" }
            Write-Out ([Environment]::NewLine + "${bold}crucible $Version$plain$suffix" +
                [Environment]::NewLine + [Environment]::NewLine)
        } else {
            $suffix = ''
            if ($Platform) { $suffix = " for $Platform" }
            Write-Out ("install: crucible $Version$suffix" + [Environment]::NewLine)
        }
    }

    function Start-Step([string]$Label) {
        $ui.Step = $Label
        $ui.Drawn = [DateTime]::MinValue
        Show-Frame ''
    }

    # Ends the running step: the first detail is the console's, the second the
    # plain line's (`ok` when empty).
    function Complete-Step([string]$Detail, [string]$PlainDetail) {
        if ($fancy) {
            Write-Row $doneMark $green $Detail
        } else {
            if (-not $PlainDetail) { $PlainDetail = 'ok' }
            Write-Out ("install: $($ui.Step): $PlainDetail" + [Environment]::NewLine)
        }
        $ui.Step = ''
    }

    # Stops with `Status`, naming the step that was running and why. Before
    # the first step, and on standard error in the plain form, the reason is
    # worded as install.sh words it. A reason can carry a server's words, such
    # as an HTTP reason phrase, so every control character in it is shown as
    # `?` rather than handed to the console to act on.
    function Stop-Install([int]$Status, [string]$Reason) {
        $Reason = $Reason -replace '[\x00-\x1F\x7F-\x9F]', '?'
        if ($ui.Step) {
            if ($fancy) {
                Write-Row $failMark $red $Reason
                Write-Out ([Environment]::NewLine + 'Nothing was installed.' + [Environment]::NewLine)
                $ui.Step = ''
                $ui.Status = $Status
                throw 'crucible-install-stopped'
            }
            Write-Out ("install: $($ui.Step): failed" + [Environment]::NewLine)
            $ui.Step = ''
        }
        [Console]::Error.WriteLine("install: $Reason")
        $ui.Status = $Status
        throw 'crucible-install-stopped'
    }

    # The innermost message of an exception, which is the one that says why.
    function Get-Reason($Exception) {
        while ($null -ne $Exception.InnerException) { $Exception = $Exception.InnerException }
        return $Exception.Message
    }

    # A task waited on in slices, so the spinner keeps turning.
    function Wait-Task($Task, [scriptblock]$Progress) {
        while (-not $Task.Wait(100)) { Show-Frame (& $Progress) }
        return $Task.Result
    }

    # Redirects are followed here, not by the client, so every hop is checked
    # before it is requested: each must be HTTPS, as install.sh's curl
    # --proto '=https' requires, and there are at most ten.
    function Get-Response($Client, [string]$Url, $Method) {
        $uri = New-Object System.Uri($Url)
        $hops = 0
        while ($true) {
            if ($uri.Scheme -ne 'https') { Stop-Install 1 "refusing $uri, which is not HTTPS" }
            $request = New-Object System.Net.Http.HttpRequestMessage($Method, $uri)
            $response = Wait-Task ($Client.SendAsync($request,
                [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead)) { '' }
            $status = [int]$response.StatusCode
            $location = $response.Headers.Location
            if ($status -lt 300 -or $status -gt 399 -or $null -eq $location) { break }
            $response.Dispose()
            $hops++
            if ($hops -gt 10) { Stop-Install 1 "$Url redirected more than 10 times" }
            if (-not $location.IsAbsoluteUri) { $location = New-Object System.Uri($uri, $location) }
            $uri = $location
        }
        if (-not $response.IsSuccessStatusCode) {
            Stop-Install 1 ("{0} answered {1} {2}" -f $Url, [int]$response.StatusCode, $response.ReasonPhrase)
        }
        return $response
    }

    # Saves one release asset and returns its size in bytes.
    function Save-Asset($Client, [string]$Url, [string]$Path) {
        $response = Get-Response $Client $Url ([System.Net.Http.HttpMethod]::Get)
        $total = $response.Content.Headers.ContentLength
        $stream = $null
        $file = $null
        try {
            $stream = Wait-Task ($response.Content.ReadAsStreamAsync()) { '' }
            $file = [System.IO.File]::Create($Path)
            $buffer = New-Object byte[] 65536
            [long]$got = 0
            while ($true) {
                $progress = {
                    if ($null -ne $total -and $total -gt 0) { Get-Bar ([Math]::Min($got, $total)) $total } else { '' }
                }
                $read = Wait-Task ($stream.ReadAsync($buffer, 0, $buffer.Length)) $progress
                if ($read -le 0) { break }
                $file.Write($buffer, 0, $read)
                $got += $read
                Show-Frame (& $progress)
            }
            return $got
        } finally {
            if ($null -ne $file) { $file.Dispose() }
            if ($null -ne $stream) { $stream.Dispose() }
            $response.Dispose()
        }
    }

    # Runs the system's tar.exe, turning the spinner while it works, and
    # returns what it printed, one line per element.
    function Invoke-Tar([string]$Arguments) {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $tar
        $info.Arguments = $Arguments
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $info.RedirectStandardOutput = $true
        $info.RedirectStandardError = $true
        $process = [System.Diagnostics.Process]::Start($info)
        $out = $process.StandardOutput.ReadToEndAsync()
        $err = $process.StandardError.ReadToEndAsync()
        while (-not $process.WaitForExit(100)) { Show-Frame '' }
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) {
            $problem = $err.Result.Trim()
            if (-not $problem) { $problem = "tar stopped with status $($process.ExitCode)" }
            Stop-Install 1 $problem
        }
        return @($out.Result -split "`r?`n" | Where-Object { $_ -ne '' })
    }

    function Test-RegularFile([string]$Path) {
        if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
        $item = Get-Item -LiteralPath $Path -Force
        return -not ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)
    }

    function Get-Sha256([string]$Path) {
        return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    }

    # `%LOCALAPPDATA%` for the per-user data directory, as Windows writes it.
    function Get-Shown([string]$Path) {
        $base = $env:LOCALAPPDATA
        if ($base) {
            $base = $base.TrimEnd('\')
            if ($Path.StartsWith("$base\", [StringComparison]::OrdinalIgnoreCase)) {
                return '%LOCALAPPDATA%' + $Path.Substring($base.Length)
            }
        }
        return $Path
    }

    # The directories a new terminal will find on PATH, for the user and for
    # the machine.
    function Test-OnPath([string]$Path) {
        $want = $Path.TrimEnd('\')
        foreach ($scope in 'User', 'Machine') {
            $value = [Environment]::GetEnvironmentVariable('Path', $scope)
            if (-not $value) { continue }
            foreach ($entry in ($value -split ';')) {
                if (-not $entry) { continue }
                $entry = [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\')
                if ($entry -ieq $want) { return $true }
            }
        }
        return $false
    }

    # The directory and those above it that other users can change, and so
    # could replace the executables in it or, with -AddToPath, put commands
    # of their own first on PATH; install.sh names a directory its group or
    # others can write in the same way. An entry for the user, SYSTEM,
    # Administrators or TrustedInstaller is expected, and so is one that only
    # lets anyone create subdirectories, which every volume root grants and
    # which replaces nothing. An entry that only passes to what is inside is
    # judged where it applies, since the directory below inherits it.
    function Get-Untrusted([string]$Path) {
        $expected = @([Security.Principal.WindowsIdentity]::GetCurrent().User.Value, 'S-1-5-18', 'S-1-5-32-544',
            'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
        $rights = [System.Security.AccessControl.FileSystemRights]
        # Writing, deleting, changing permissions or ownership, and the
        # generic write and all rights.
        [long]$changes = [long]$rights::WriteData -bor [long]$rights::Delete -bor
            [long]$rights::DeleteSubdirectoriesAndFiles -bor [long]$rights::ChangePermissions -bor
            [long]$rights::TakeOwnership -bor 0x10000000 -bor 0x40000000
        $inheritOnly = [int][System.Security.AccessControl.PropagationFlags]::InheritOnly
        $at = $Path
        while ($at) {
            $rules = @()
            try {
                $acl = New-Object System.Security.AccessControl.DirectorySecurity($at,
                    [System.Security.AccessControl.AccessControlSections]::Access)
                $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
            } catch { }
            foreach ($rule in $rules) {
                if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
                    ([int]$rule.PropagationFlags -band $inheritOnly) -ne 0 -or
                    $expected -contains $rule.IdentityReference.Value) { continue }
                if (([long]$rule.FileSystemRights -band $changes) -ne 0) { $at; break }
            }
            $at = [System.IO.Path]::GetDirectoryName($at)
        }
    }

    # Puts the directory first on the user's PATH, keeping the value's
    # registry kind so entries such as %USERPROFILE%\bin keep expanding.
    function Add-UserPath([string]$Path) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            $current = ''
            if ($key.GetValueNames() -contains 'Path') {
                $kind = $key.GetValueKind('Path')
                $current = [string]$key.GetValue('Path', '',
                    [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
            $value = $Path
            if ($current) { $value = "$Path;$current" }
            $key.SetValue('Path', $value, $kind)
        } finally {
            $key.Dispose()
        }
        # Setting and clearing a user variable through .NET announces the
        # change to Windows, so terminals opened from now on see the new PATH.
        [Environment]::SetEnvironmentVariable('CRUCIBLE_INSTALL_PATH_CHANGED', '1', 'User')
        [Environment]::SetEnvironmentVariable('CRUCIBLE_INSTALL_PATH_CHANGED', $null, 'User')
    }

    $work = $null
    $landed = New-Object System.Collections.Generic.List[object]
    try {
        if ($null -ne $Unknown -and $Unknown.Count -gt 0) {
            Stop-Install 2 "unknown argument $($Unknown[0])"
        }
        if (-not $Dir) { $Dir = $env:CRUCIBLE_INSTALL_DIR }
        if (-not $Dir) {
            if (-not $env:LOCALAPPDATA) { Stop-Install 2 'LOCALAPPDATA is not set' }
            $Dir = Join-Path $env:LOCALAPPDATA 'Programs\crucible\bin'
        }
        if (($Dir -split '[\\/]') -contains '..') {
            Stop-Install 2 'the installation directory is unsafe'
        }
        if ($Archive -or $Checksums) {
            if (-not ($Archive -and $Checksums -and $Version)) {
                Stop-Install 2 '-Archive requires -Checksums and -Version'
            }
        }

        $client = $null
        if (-not $Archive) {
            if ($PSVersionTable.PSEdition -ne 'Core') {
                # Windows PowerShell's HttpClient takes certificate checks from
                # this process-wide callback, which a session may have set to
                # accept anything. Both files would then come unchecked from
                # whoever answers, and a matching pair proves nothing.
                if ($null -ne [Net.ServicePointManager]::ServerCertificateValidationCallback) {
                    Stop-Install 1 ('this session replaces certificate checks through ' +
                        '[Net.ServicePointManager]::ServerCertificateValidationCallback; ' +
                        'install from a new PowerShell window')
                }
                Add-Type -AssemblyName System.Net.Http
                [Net.ServicePointManager]::SecurityProtocol =
                    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            }
            $handler = New-Object System.Net.Http.HttpClientHandler
            $handler.AllowAutoRedirect = $false
            $client = New-Object System.Net.Http.HttpClient($handler)
            $client.DefaultRequestHeaders.UserAgent.ParseAdd('crucible-install')
            if (-not $Version) {
                try {
                    $latest = Get-Response $client "$releases/latest" ([System.Net.Http.HttpMethod]::Head)
                } catch {
                    if ($_.Exception.Message -eq 'crucible-install-stopped') { throw }
                    Stop-Install 1 (Get-Reason $_.Exception)
                }
                $Version = $latest.RequestMessage.RequestUri.Segments[-1]
                $latest.Dispose()
            }
        }
        if ($Version.StartsWith('v')) { $Version = $Version.Substring(1) }
        if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$') {
            Stop-Install 2 "invalid version $Version"
        }

        $machine = ''
        try {
            $machine = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        } catch {
            $machine = $env:PROCESSOR_ARCHITEW6432
            if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
        }
        $unsupported = ''
        $architecture = ''
        if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
            $unsupported = "unsupported operating system $([Environment]::OSVersion.Platform)"
        } else {
            switch ($machine.ToUpperInvariant()) {
                'X64' { $architecture = 'x86_64' }
                'AMD64' { $architecture = 'x86_64' }
                'ARM64' { $architecture = 'aarch64' }
                default { $unsupported = "unsupported architecture $machine" }
            }
        }
        if ($unsupported) {
            Write-Banner ''
            Start-Step 'detect platform'
            Stop-Install 1 $unsupported
        }
        $platform = "windows-$architecture"
        Write-Banner $platform
        Start-Step 'detect platform'
        Complete-Step $platform $platform

        $stem = "crucible-$Version-$platform"
        $name = "$stem.tar.gz"
        $work = Join-Path ([System.IO.Path]::GetTempPath()) ('crucible-install-' + [Guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $work
        if (-not $Archive) {
            $Archive = Join-Path $work $name
            $Checksums = Join-Path $work 'SHA256SUMS'
            Start-Step 'download'
            try {
                $size = Save-Asset $client "$releases/download/v$Version/$name" $Archive
                $null = Save-Asset $client "$releases/download/v$Version/SHA256SUMS" $Checksums
            } catch {
                if ($_.Exception.Message -eq 'crucible-install-stopped') { throw }
                Stop-Install 1 (Get-Reason $_.Exception)
            }
            $shownSize = Get-Megabytes $size
            Complete-Step "$name $dot $shownSize" "$name ($shownSize)"
        } else {
            $Archive = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Archive)
            $Checksums = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Checksums)
        }

        Start-Step 'verify checksum'
        if (-not ((Test-Path -LiteralPath $Archive -PathType Leaf) -and
                (Test-Path -LiteralPath $Checksums -PathType Leaf))) {
            Stop-Install 1 'archive or checksum file does not exist'
        }
        # A local archive is copied into the private work directory, and only
        # that copy is hashed and unpacked, so whoever can write where it was
        # cannot swap it between the check and the unpacking.
        $archiveName = Split-Path -Leaf $Archive
        $copy = Join-Path $work $archiveName
        if ($Archive -ne $copy) {
            Copy-Item -LiteralPath $Archive -Destination $copy
            $Archive = $copy
        }
        $expected = @(Get-Content -LiteralPath $Checksums | ForEach-Object {
                $fields = @(($_.Trim()) -split '\s+')
                if ($fields.Count -ge 2 -and ($fields[1] -ceq $archiveName -or $fields[1] -ceq "*$archiveName") -and
                    $fields[0] -match '^[0-9A-Fa-f]+$') {
                    $fields[0].ToLowerInvariant()
                }
            })
        if ($expected.Count -ne 1 -or $expected[0].Length -ne 64) {
            Stop-Install 1 'SHA256SUMS must contain exactly one valid line for the archive'
        }
        if ((Get-Sha256 $Archive) -ne $expected[0]) {
            Stop-Install 1 'archive checksum does not match SHA256SUMS'
        }
        Complete-Step (Split-Path -Leaf $Checksums) 'ok'

        Start-Step 'unpack'
        $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
        if (-not (Test-Path -LiteralPath $tar -PathType Leaf)) {
            Stop-Install 1 'tar.exe is required; it ships with Windows 10 version 1803 and later'
        }
        $members = @(Invoke-Tar "-tzf `"$Archive`"")
        $details = @(Invoke-Tar "-tvzf `"$Archive`"")
        if ($members.Count -ne $details.Count) { Stop-Install 1 'archive listings disagreed' }
        $files = @('crucible.exe', 'crucible-sandbox-broker.exe', 'README.md', 'LICENSE', 'install.sh', 'uninstall.sh')
        for ($i = 0; $i -lt $members.Count; $i++) {
            $member = $members[$i]
            $kind = $details[$i].Substring(0, 1)
            if ($member -ceq $stem -or $member -ceq "$stem/") {
                if ($kind -ne 'd') { Stop-Install 1 "archive directory $member is not a directory" }
            } elseif ($member.StartsWith("$stem/", [StringComparison]::Ordinal) -and
                $files -ccontains $member.Substring($stem.Length + 1)) {
                if ($kind -ne '-') { Stop-Install 1 "archive file $member is not a regular file" }
            } else {
                Stop-Install 1 "unexpected archive member $member"
            }
        }
        foreach ($wanted in 'crucible.exe', 'crucible-sandbox-broker.exe') {
            if (@($members | Where-Object { $_ -ceq "$stem/$wanted" }).Count -ne 1) {
                Stop-Install 1 "archive does not contain exactly one $wanted"
            }
        }
        $null = Invoke-Tar "-xzf `"$Archive`" -C `"$work`""
        $binary = Join-Path $work "$stem\crucible.exe"
        $broker = Join-Path $work "$stem\crucible-sandbox-broker.exe"
        if (-not (Test-RegularFile $binary)) { Stop-Install 1 'crucible.exe in the archive is not a regular file' }
        if (-not (Test-RegularFile $broker)) {
            Stop-Install 1 'the sandbox broker in the archive is not a regular file'
        }
        Complete-Step '' 'ok'

        Start-Step 'install'
        $destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Dir).TrimEnd('\')
        if (-not $destination -or $destination.EndsWith(':') -or
            [System.IO.Path]::GetPathRoot("$destination\") -eq "$destination\") {
            Stop-Install 2 'the installation directory resolves to root'
        }
        $where = Get-Shown $destination
        $binaryPath = Join-Path $destination 'crucible.exe'
        $brokerPath = Join-Path $destination 'crucible-sandbox-broker.exe'
        $aliasPath = Join-Path $destination 'cru.exe'
        foreach ($path in $binaryPath, $brokerPath, $aliasPath) {
            if ((Test-Path -LiteralPath $path) -and -not (Test-RegularFile $path)) {
                Stop-Install 1 "refusing to replace non-regular $path"
            }
        }
        # cru.exe is a copy of crucible.exe, since a link needs privileges a
        # user may not have; one that is no longer that copy is someone else's.
        if (Test-Path -LiteralPath $aliasPath) {
            if (-not (Test-Path -LiteralPath $binaryPath) -or (Get-Sha256 $aliasPath) -ne (Get-Sha256 $binaryPath)) {
                Stop-Install 1 "refusing to replace unrelated $aliasPath"
            }
        }
        if ($DryRun) {
            Complete-Step "$where (dry run)" "$destination (dry run)"
            Write-Out ("Would install crucible $Version in $destination with $brokerPath and $aliasPath beside it" +
                [Environment]::NewLine)
            if ($AddToPath -and -not (Test-OnPath $destination)) {
                Write-Out ("Would add $destination to your user PATH" + [Environment]::NewLine)
            }
            return 0
        }

        $null = New-Item -ItemType Directory -Force -Path $destination
        # A replaced file that was still running when an earlier install moved
        # it aside could not be removed then; it can be now that it has exited.
        # Only the names this installer gives such files are touched.
        $stuck = New-Object System.Collections.Generic.List[string]
        foreach ($item in @(Get-ChildItem -LiteralPath $destination -Force -File)) {
            if ($item.Name -cmatch '^\.(crucible|crucible-sandbox-broker|cru)\.exe\.previous\.[0-9a-f]{32}$') {
                Remove-Item -LiteralPath $item.FullName -Force -ErrorAction SilentlyContinue
                if (Test-Path -LiteralPath $item.FullName) { $stuck.Add($item.FullName) }
            }
        }
        # Either everything lands or nothing changes. A file being replaced is
        # moved aside first, which Windows allows even while it runs, and put
        # back if anything after it fails or the install is interrupted. Each
        # is noted before it is moved, so an interruption at any point leaves
        # nothing that will not be put back. The broker lands first so the
        # executable never runs beside a stale broker.
        foreach ($pair in @(@($broker, $brokerPath), @($binary, $binaryPath), @($binary, $aliasPath))) {
            $source = $pair[0]
            $target = $pair[1]
            $previous = $null
            if (Test-Path -LiteralPath $target) {
                $previous = Join-Path $destination ('.' + (Split-Path -Leaf $target) + '.previous.' +
                    [Guid]::NewGuid().ToString('N'))
            }
            $landed.Add(@($target, $previous))
            if ($previous) { Move-Item -LiteralPath $target -Destination $previous }
            Copy-Item -LiteralPath $source -Destination $target
        }
        # Everything has landed: from here on nothing is put back.
        $replaced = @($landed | ForEach-Object { $_[1] } | Where-Object { $_ })
        $landed.Clear()
        foreach ($previous in $replaced) {
            Remove-Item -LiteralPath $previous -Force -ErrorAction SilentlyContinue
            if (Test-Path -LiteralPath $previous) { $stuck.Add($previous) }
        }
        Complete-Step $where $destination

        $onPath = Test-OnPath $destination
        if ($AddToPath -and -not $onPath) { Add-UserPath $destination }
        $installed = 'Installed crucible.exe, crucible-sandbox-broker.exe and cru.exe in '
        $nl = [Environment]::NewLine
        $warnings = @($stuck | ForEach-Object { "could not remove $_, a replaced copy still in use; the next install removes it" })
        $warnings += @(Get-Untrusted $destination | ForEach-Object {
                "$_ is writable by other users, who could replace $brokerPath; remove their write access to $_"
            })
        # A log gets the whole path; the console gets it as Windows writes it.
        if (-not $fancy) {
            Write-Out ($installed + $destination + $nl)
            foreach ($line in $warnings) { [Console]::Error.WriteLine("install: $line") }
            if ($AddToPath -and -not $onPath) {
                Write-Out ("Added $destination to your user PATH; open a new terminal to run crucible." + $nl)
            } elseif (-not $onPath) {
                Write-Out ("Add $destination to PATH to run crucible." + $nl)
            }
            return 0
        }
        $installed += $where
        Write-Out ($nl + (Get-Wrapped '' $installed) + $nl)
        foreach ($line in $warnings) { [Console]::Error.WriteLine((Get-Wrapped '' $line)) }
        Write-Out $nl
        if ($AddToPath -and -not $onPath) {
            Write-Out ($dim + (Get-Wrapped '' 'That directory is now first on your user PATH.') + $plain + $nl)
        } elseif ($onPath) {
            Write-Out ($dim + (Get-Wrapped '' 'That directory is on your PATH.') + $plain + $nl)
        } else {
            Write-Out ($dim + (Get-Wrapped '' 'That directory is not on your PATH. Add it for your user with:') +
                $plain + $nl + $nl)
            # Pasted whole into PowerShell, so never wrapped beyond its own
            # three lines. PowerShell reads U+2018 to U+201B as single quotes
            # and U+201C to U+201E as double quotes, so a directory is quoted
            # literally with every single quote of any kind doubled, as
            # PowerShell's own code generation does. Below %LOCALAPPDATA% it
            # is named through $env:LOCALAPPDATA instead, but only when the
            # rest holds nothing double quotes would read.
            $literal = "'" + ($destination -replace "['\u2018-\u201B]", '$0$0') + ";'"
            $below = $where.Substring([Math]::Min($where.Length, '%LOCALAPPDATA%'.Length))
            if ($where.StartsWith('%LOCALAPPDATA%') -and $below -notmatch '[`$"\u201C-\u201E]') {
                $literal = '"$env:LOCALAPPDATA' + $below + ';"'
            }
            Write-Out ("  [Environment]::SetEnvironmentVariable('Path'," + $nl +
                "    $literal +" + $nl +
                "    [Environment]::GetEnvironmentVariable('Path','User'), 'User')" + $nl)
        }
        Write-Out ($nl + $dim + 'Then open a new terminal and run: crucible' + $plain + $nl)
        return 0
    } catch {
        if ($_.Exception.Message -ne 'crucible-install-stopped') {
            # Whatever else stops the install still says which step it was.
            $reason = Get-Reason $_.Exception
            try { Stop-Install 1 $reason } catch { }
        }
        return $ui.Status
    } finally {
        # Put back what was moved aside, newest first. This is here and not in
        # catch because Ctrl+C runs finally alone. A file noted but not yet
        # moved aside is still where it was, and stays.
        for ($i = $landed.Count - 1; $i -ge 0; $i--) {
            $target = $landed[$i][0]
            $previous = $landed[$i][1]
            if (-not $previous) {
                Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
            } elseif (Test-Path -LiteralPath $previous) {
                Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
                Move-Item -LiteralPath $previous -Destination $target -Force -ErrorAction SilentlyContinue
            }
        }
        if ($work) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

# Run as this file, or as a script block made from this text, the installer
# has a scope of its own: $args holds the arguments no parameter took, and run
# as a file its status is the process's. Run through `iex` it shares the
# caller's scope, where $args holds the caller's own arguments and exiting would
# close the caller's window or end the caller's script; so it takes no stray
# arguments and leaves the status in LASTEXITCODE.
$crucibleInstallScope = ''
try {
    if (([string]$MyInvocation.MyCommand.ScriptBlock).Contains('function Invoke-CrucibleInstall')) {
        $crucibleInstallScope = [string]$MyInvocation.MyCommand.CommandType
    }
} catch { }
$global:LASTEXITCODE = [int](@(Invoke-CrucibleInstall -Version $Version -Dir $Dir -Archive $Archive `
            -Checksums $Checksums -DryRun $DryRun.IsPresent -AddToPath $AddToPath.IsPresent `
            -Unknown $(if ($crucibleInstallScope) { $args } else { @() })) |
        Select-Object -Last 1)
if ($crucibleInstallScope -eq 'ExternalScript') { exit $global:LASTEXITCODE }
Remove-Variable -Name crucibleInstallScope -ErrorAction SilentlyContinue
