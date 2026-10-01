param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [ValidateSet("none", "x64-only", "both")]
    [string]$InitialRuntimeState,

    [string]$LogPath = (Join-Path $env:TEMP "azookey-vcredist-install.log"),

    [ValidateRange(60, 3600)]
    [int]$TimeoutSec = 1200
)

# Run on a clean Windows VM snapshot with the specified runtime state.
# This test installs Azookey; restore the snapshot between matrix cases.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-VCRuntimeVersion {
    param([string]$Arch)

    foreach ($root in @("SOFTWARE", "SOFTWARE\WOW6432Node")) {
        $key = "HKLM:\$root\Microsoft\VisualStudio\14.0\VC\Runtimes\$Arch"
        if (Test-Path -LiteralPath $key) {
            $runtime = Get-ItemProperty -LiteralPath $key
            if ($runtime.Installed -eq 1) {
                return [version]$runtime.Version.TrimStart("v")
            }
        }
    }
    return $null
}

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$minimumVersion = [version]"14.42.34433.0"
foreach ($arch in @("x64", "x86")) {
    $version = Get-VCRuntimeVersion -Arch $arch
    $expectedInstalled = ($InitialRuntimeState -eq "both") -or
        (($InitialRuntimeState -eq "x64-only") -and ($arch -eq "x64"))
    if (($null -ne $version) -ne $expectedInstalled) {
        throw "Initial runtime state mismatch for ${arch}: version=$version expected=$InitialRuntimeState"
    }
    if ($expectedInstalled -and ($version -lt $minimumVersion)) {
        throw "Initial $arch runtime is older than $minimumVersion"
    }
    Write-Host "Before: $arch version=$version"
}

$process = Start-Process -FilePath $resolvedInstaller -ArgumentList @(
    "/SP-", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART",
    "/RESTARTEXITCODE=3010", "/LOG=`"$LogPath`""
) -PassThru
if (-not $process.WaitForExit($TimeoutSec * 1000)) {
    & taskkill.exe /PID $process.Id /T /F | Out-Null
    throw "Installer timed out after $TimeoutSec seconds; log=$LogPath"
}
if ($process.ExitCode -notin @(0, 3010)) {
    throw "Installer failed with exit code $($process.ExitCode); log=$LogPath"
}

foreach ($arch in @("x64", "x86")) {
    $version = Get-VCRuntimeVersion -Arch $arch
    if (($null -eq $version) -or ($version -lt $minimumVersion)) {
        throw "Required $arch runtime is missing after install: version=$version; log=$LogPath"
    }
    Write-Host "After: $arch version=$version"
}
Write-Host "PASS: $InitialRuntimeState -> both runtimes installed; exit=$($process.ExitCode); log=$LogPath"
