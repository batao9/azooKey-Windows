param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$Sha256Path,

    [string]$LogDirectory = $env:RUNNER_TEMP,

    [switch]$RequireSignature,

    [switch]$SilentInstall,

    [ValidateRange(60, 3600)]
    [int]$TimeoutSec = 900
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Wait-CheckedProcess {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process,

        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    if (-not $Process.WaitForExit($TimeoutSec * 1000)) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
        throw "$Description timed out after $TimeoutSec seconds"
    }

    if ($Process.ExitCode -ne 0) {
        throw "$Description failed with exit code $($Process.ExitCode)"
    }
}

function Get-AzookeyUninstallEntry {
    $registryPaths = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{80B746D4-D74D-4345-8F81-47E06BCAB515}_is1",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{80B746D4-D74D-4345-8F81-47E06BCAB515}_is1"
    )

    foreach ($registryPath in $registryPaths) {
        if (Test-Path -LiteralPath $registryPath) {
            return Get-ItemProperty -LiteralPath $registryPath
        }
    }

    throw "Azookey uninstall registry entry was not created"
}

function Assert-InstallPayload {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallLocation
    )

    $requiredPaths = @(
        "azookey.dll",
        "azookey32.dll",
        "frontend.exe",
        "azookey-updater-helper.exe",
        "azookey-server.exe",
        "launcher.exe",
        "ui.exe",
        "zenz.gguf",
        "Dictionary",
        "EmojiDictionary",
        "llama_cpu",
        "llama_cuda",
        "llama_vulkan"
    )

    foreach ($relativePath in $requiredPaths) {
        $path = Join-Path $InstallLocation $relativePath
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Required install payload is missing: $path"
        }
    }

    $task = Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction Stop
    $launcherPath = Join-Path $InstallLocation "launcher.exe"
    $taskExecutable = [Environment]::ExpandEnvironmentVariables($task.Actions[0].Execute).Trim('"')
    if (-not [string]::Equals(
        [IO.Path]::GetFullPath($taskExecutable),
        [IO.Path]::GetFullPath($launcherPath),
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Azookey Startup task does not execute the installed launcher: $taskExecutable"
    }
}

function Wait-AzookeyUninstalled {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallLocation
    )

    $payloadPaths = @("frontend.exe", "azookey.dll", "azookey32.dll", "launcher.exe") |
        ForEach-Object { Join-Path $InstallLocation $_ }
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    while ($stopwatch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $taskRemains = $null -ne (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue)
        $remainingPayload = @($payloadPaths | Where-Object { Test-Path -LiteralPath $_ })
        if (-not $taskRemains -and $remainingPayload.Count -eq 0) {
            return
        }
        Start-Sleep -Seconds 1
    }

    $taskRemains = $null -ne (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue)
    $remainingPayload = @($payloadPaths | Where-Object { Test-Path -LiteralPath $_ })
    throw "Uninstall cleanup timed out. task_remains=$taskRemains remaining_payload=$($remainingPayload -join ',')"
}

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$installer = Get-Item -LiteralPath $resolvedInstaller
if ($installer.Length -le 0) {
    throw "Installer is empty: $resolvedInstaller"
}

$sha256 = (Get-FileHash -LiteralPath $resolvedInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
if (Test-Path -LiteralPath $Sha256Path) {
    $manifest = (Get-Content -LiteralPath $Sha256Path -Raw).Trim()
    $manifestParts = @($manifest -split '\s+', 2)
    if ($manifestParts.Count -ne 2 -or $manifestParts[0] -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Invalid SHA256 manifest: $Sha256Path"
    }
    $expectedName = $manifestParts[1].TrimStart('*')
    if (-not [string]::Equals($expectedName, $installer.Name, [StringComparison]::Ordinal)) {
        throw "SHA256 manifest names $expectedName instead of $($installer.Name)"
    }
    if ($manifestParts[0].ToLowerInvariant() -ne $sha256) {
        throw "SHA256 mismatch: expected=$($manifestParts[0]) actual=$sha256"
    }
    Write-Host "Verified existing SHA256 manifest: $Sha256Path"
} else {
    $shaDirectory = Split-Path -Parent $Sha256Path
    if ($shaDirectory) {
        New-Item -ItemType Directory -Path $shaDirectory -Force | Out-Null
    }
    [IO.File]::WriteAllText(
        $Sha256Path,
        "$sha256  $($installer.Name)`n",
        [Text.Encoding]::ASCII
    )
}
Write-Host "SHA-256: $sha256"

$signature = Get-AuthenticodeSignature -LiteralPath $resolvedInstaller
Write-Host "Authenticode status: $($signature.Status)"
if ($signature.SignerCertificate) {
    Write-Host "Authenticode signer: $($signature.SignerCertificate.Subject)"
}
if ($RequireSignature -and $signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
    throw "A valid Authenticode signature is required, but status is $($signature.Status)"
}
if ($signature.Status -notin @(
    [System.Management.Automation.SignatureStatus]::Valid,
    [System.Management.Automation.SignatureStatus]::NotSigned
)) {
    throw "Unexpected Authenticode status: $($signature.Status)"
}

if (-not $SilentInstall) {
    exit 0
}

New-Item -ItemType Directory -Path $LogDirectory -Force | Out-Null
$resolvedLogDirectory = (Resolve-Path -LiteralPath $LogDirectory).Path
$installLog = Join-Path $resolvedLogDirectory "azookey-install.log"
$uninstallLog = Join-Path $resolvedLogDirectory "azookey-uninstall.log"
$installed = $false

try {
    $installArguments = @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/SP-",
        "/LOG=`"$installLog`""
    )
    $installProcess = Start-Process -FilePath $resolvedInstaller -ArgumentList $installArguments -PassThru
    Wait-CheckedProcess -Process $installProcess -Description "Silent installer"
    $installed = $true

    $entry = Get-AzookeyUninstallEntry
    if ([string]::IsNullOrWhiteSpace($entry.InstallLocation)) {
        throw "Azookey uninstall entry has no InstallLocation"
    }
    $installLocation = $entry.InstallLocation.TrimEnd("\")
    Assert-InstallPayload -InstallLocation $installLocation

    $uninstaller = Get-ChildItem -LiteralPath $installLocation -Filter "unins*.exe" -File |
        Sort-Object Name |
        Select-Object -First 1
    if (-not $uninstaller) {
        throw "Azookey uninstaller was not created under $installLocation"
    }

    $uninstallArguments = @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/LOG=`"$uninstallLog`""
    )
    $uninstallProcess = Start-Process -FilePath $uninstaller.FullName -ArgumentList $uninstallArguments -PassThru
    Wait-CheckedProcess -Process $uninstallProcess -Description "Silent uninstaller"
    $installed = $false
    Wait-AzookeyUninstalled -InstallLocation $installLocation

    Write-Host "Silent install and uninstall verification passed"
} finally {
    if ($installed) {
        $entry = Get-AzookeyUninstallEntry
        $installLocation = $entry.InstallLocation.TrimEnd("\")
        $uninstaller = Get-ChildItem -LiteralPath $installLocation -Filter "unins*.exe" -File -ErrorAction SilentlyContinue |
            Sort-Object Name |
            Select-Object -First 1
        if ($uninstaller) {
            Start-Process -FilePath $uninstaller.FullName -ArgumentList @(
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART"
            ) -Wait -ErrorAction SilentlyContinue
        }
    }
}
