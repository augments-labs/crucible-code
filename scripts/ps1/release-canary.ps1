# Installs the newest published release on Windows with the install.ps1 that
# release published, into a disposable directory, runs both names it installs,
# removes the three executables it installed, and proves the directory is left
# empty. Windows ships no uninstaller, so removing what install.ps1 installed
# is the uninstall.
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$releases = 'https://github.com/augments-labs/crucible-code/releases'
$installed = @('crucible.exe', 'crucible-sandbox-broker.exe', 'cru.exe')

function Stop-Canary([string]$Message) {
    [Console]::Error.WriteLine("release-canary.ps1: $Message")
    exit 1
}

$root = Join-Path ([IO.Path]::GetTempPath()) ('crucible-canary-' + [guid]::NewGuid().ToString('N'))
$bin = Join-Path $root 'bin'
$installer = Join-Path $root 'install.ps1'
New-Item -ItemType Directory -Path $bin -Force | Out-Null
try {
    Invoke-WebRequest -UseBasicParsing -Uri "$releases/latest/download/install.ps1" -OutFile $installer
    $shell = (Get-Process -Id $PID).Path
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installer -Dir $bin
    if ($LASTEXITCODE -ne 0) { Stop-Canary "the published install.ps1 exited $LASTEXITCODE" }

    foreach ($name in $installed) {
        if (-not (Test-Path -LiteralPath (Join-Path $bin $name) -PathType Leaf)) {
            Stop-Canary "the latest release installed no $name"
        }
    }
    $version = ''
    foreach ($name in 'crucible.exe', 'cru.exe') {
        $said = [string](& (Join-Path $bin $name) --version)
        if ($LASTEXITCODE -ne 0 -or $said -notmatch '^crucible \d+\.\d+\.\d+') {
            Stop-Canary "$name --version printed an unexpected version: '$said'"
        }
        if ($version -and $said -ne $version) { Stop-Canary "cru.exe said '$said' where crucible.exe said '$version'" }
        $version = $said
    }

    foreach ($name in $installed) {
        Remove-Item -LiteralPath (Join-Path $bin $name) -Force
    }
    $left = @(Get-ChildItem -LiteralPath $bin -Force)
    if ($left.Count -ne 0) {
        Stop-Canary "the uninstall left $(($left | ForEach-Object { $_.Name }) -join ', ')"
    }
    Write-Output "$version installed, ran and uninstalled cleanly"
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
