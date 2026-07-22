#!/usr/bin/env bash
# Windows VM 上で updater の download/hash/install 起動経路を検証するスクリプトです。
# 詳細出力は .local/logs にリダイレクトして使うことを想定しています。

set -euo pipefail

VM_NAME="${VM_NAME:-}"
SNAPSHOT_NAME="${SNAPSHOT_NAME:-}"
SSH_USER="${SSH_USER:-}"
SSH_PORT="${SSH_PORT:-}"
SSH_KEY="${SSH_KEY:-}"
VBOX_MANAGE="${VBOX_MANAGE:-}"
PSEUDO_RELEASE_PORT="${PSEUDO_RELEASE_PORT:-$((18000 + RANDOM % 1000))}"
SHUTDOWN_AFTER_TEST="${SHUTDOWN_AFTER_TEST:-1}"
UPDATER_TIMEOUT_SEC="${UPDATER_TIMEOUT_SEC:-1200}"
UPDATER_BASE_INSTALLER="${UPDATER_BASE_INSTALLER:-}"

if [[ -z "$VBOX_MANAGE" ]]; then
  if command -v VBoxManage >/dev/null 2>&1; then
    VBOX_MANAGE="$(command -v VBoxManage)"
  elif [[ -x "/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe" ]]; then
    VBOX_MANAGE="/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe"
  fi
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="$REPO_ROOT/.local/artifacts"
LOG_DIR="$REPO_ROOT/.local/logs"
PSEUDO_ROOT="$REPO_ROOT/.local/updater-release"
mkdir -p "$ARTIFACT_DIR" "$LOG_DIR" "$PSEUDO_ROOT"

DEFAULT_GATEWAY_IP="$(ip route | awk '/default/ {print $3; exit}' || true)"
HOST_IP="${SSH_HOST:-${DEFAULT_GATEWAY_IP:-127.0.0.1}}"
FALLBACK_HOST=""
if [[ "$HOST_IP" == "127.0.0.1" && -n "$DEFAULT_GATEWAY_IP" ]]; then
  FALLBACK_HOST="$DEFAULT_GATEWAY_IP"
elif [[ "$HOST_IP" != "127.0.0.1" ]]; then
  FALLBACK_HOST="127.0.0.1"
fi
ACTIVE_HOST="$HOST_IP"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="$LOG_DIR/vm-updater-$TIMESTAMP.log"
REMOTE_TMP_WIN="C:\\Users\\$SSH_USER\\AppData\\Local\\Temp"
REMOTE_PS_WIN="$REMOTE_TMP_WIN\\azookey-updater-smoke.ps1"
REMOTE_PS_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-updater-smoke.ps1"
REMOTE_TARGET_INSTALLER_WIN="$REMOTE_TMP_WIN\\azookey-updater-target.exe"
REMOTE_TARGET_INSTALLER_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-updater-target.exe"
TMP_REMOTE_PS=""

SSH_OPTS=(-i "$SSH_KEY" -p "$SSH_PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)
SCP_OPTS=(-i "$SSH_KEY" -P "$SSH_PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)

exec > >(tee "$LOG_FILE") 2>&1

log() {
  printf '[vm-updater] %s\n' "$*"
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    log "環境変数 $name を設定してください"
    exit 1
  fi
}

vbox() {
  "$VBOX_MANAGE" "$@"
}

matches_fixed() {
  if command -v rg >/dev/null 2>&1; then
    rg -F "$1" -q
  else
    grep -F "$1" -q
  fi
}

is_vm_running() {
  vbox list runningvms | matches_fixed "\"$VM_NAME\""
}

ssh_run() {
  ssh "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" "$@"
}

scp_to_vm() {
  scp "${SCP_OPTS[@]}" "$1" "$SSH_USER@$ACTIVE_HOST:$2"
}

wait_for_ssh() {
  local tries=120
  local hosts=("$HOST_IP")
  if [[ -n "$FALLBACK_HOST" && "$FALLBACK_HOST" != "$HOST_IP" ]]; then
    hosts+=("$FALLBACK_HOST")
  fi

  for ((i=1; i<=tries; i++)); do
    local host
    for host in "${hosts[@]}"; do
      if timeout 10s ssh "${SSH_OPTS[@]}" "$SSH_USER@$host" "echo ready" >/dev/null 2>&1; then
        ACTIVE_HOST="$host"
        log "SSH 接続確認: OK (host=$ACTIVE_HOST, try $i/$tries)"
        return 0
      fi
    done
    sleep 2
  done
  return 1
}

wait_for_vm_poweroff() {
  local tries=60
  for ((i=1; i<=tries; i++)); do
    if ! is_vm_running; then
      return 0
    fi
    sleep 2
  done
  return 1
}

shutdown_vm() {
  if [[ "$SHUTDOWN_AFTER_TEST" != "1" ]] || ! is_vm_running; then
    return 0
  fi
  log "VM を停止します: $VM_NAME"
  vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
  if ! wait_for_vm_poweroff; then
    vbox controlvm "$VM_NAME" poweroff >/dev/null || true
  fi
}

cleanup() {
  local rc=$?
  set +e
  rm -f "${TMP_REMOTE_PS:-}"
  shutdown_vm
  trap - EXIT
  exit "$rc"
}

find_installer() {
  local arg="${1:-latest}"
  if [[ "$arg" != "latest" ]]; then
    realpath "$arg"
    return 0
  fi

  local latest
  latest="$(ls -t "$ARTIFACT_DIR"/azookey-setup-*.exe 2>/dev/null | head -n 1 || true)"
  if [[ -z "$latest" ]]; then
    log "インストーラーが見つかりません。先に VM build を実行してください。"
    exit 1
  fi
  realpath "$latest"
}

ensure_preconditions() {
  require_env VM_NAME
  require_env SNAPSHOT_NAME
  require_env SSH_USER
  require_env SSH_PORT
  require_env SSH_KEY
  if [[ ! -x "$VBOX_MANAGE" ]]; then
    log "VBoxManage が見つかりません。VBOX_MANAGE を設定してください: ${VBOX_MANAGE:-<unset>}"
    exit 1
  fi
  if [[ ! -f "$SSH_KEY" ]]; then
    log "SSH 秘密鍵が見つかりません: $SSH_KEY"
    exit 1
  fi
}

create_smoke_ps1() {
  local ps1="$1"
  cat > "$ps1" <<'PS1'
param(
  [Parameter(Mandatory = $true)][string]$LocalInstallerPath,
  [Parameter(Mandatory = $true)][int]$PseudoReleasePort,
  [Parameter(Mandatory = $true)][string]$WorkDir,
  [Parameter(Mandatory = $true)][int]$UpdaterTimeoutSec,
  [switch]$RequirePayloadChange
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Get-AssetUrl {
  param(
    [Parameter(Mandatory = $true)]$Release,
    [Parameter(Mandatory = $true)][string]$Name
  )
  $asset = @($Release.assets | Where-Object { $_.name -eq $Name })[0]
  if ($null -eq $asset -or [string]::IsNullOrWhiteSpace($asset.browser_download_url)) {
    throw "asset not found: $Name"
  }
  $asset.browser_download_url
}

function Save-Url {
  param(
    [Parameter(Mandatory = $true)][string]$Url,
    [Parameter(Mandatory = $true)][string]$OutFile
  )
  & curl.exe -fsSL -H "User-Agent: azookey-updater-smoke" -o $OutFile $Url
  if ($LASTEXITCODE -ne 0) {
    throw "curl failed: exit=$LASTEXITCODE url=$Url"
  }
}

function Test-ReleaseDownload {
  param(
    [Parameter(Mandatory = $true)][string]$ReleaseApiUrl,
    [Parameter(Mandatory = $true)][string]$Prefix,
    [switch]$RunInstaller
  )

  New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
  $releasePath = Join-Path $WorkDir "$Prefix-release.json"
  Save-Url -Url $ReleaseApiUrl -OutFile $releasePath
  $release = Get-Content -LiteralPath $releasePath -Raw | ConvertFrom-Json
  $installerUrl = Get-AssetUrl -Release $release -Name "azookey-setup.exe"
  $shaUrl = Get-AssetUrl -Release $release -Name "SHA256SUMS.txt"

  $shaPath = Join-Path $WorkDir "$Prefix-SHA256SUMS.txt"
  $installerPath = Join-Path $WorkDir "$Prefix-azookey-setup.exe"
  Save-Url -Url $shaUrl -OutFile $shaPath
  Save-Url -Url $installerUrl -OutFile $installerPath

  $expected = ((Get-Content -LiteralPath $shaPath | Where-Object { $_ -match "azookey-setup\.exe" }) -split "\s+")[0].ToLowerInvariant()
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $installerPath).Hash.ToLowerInvariant()
  if ($expected -ne $actual) {
    throw "$Prefix hash mismatch: expected=$expected actual=$actual"
  }

  if ($RunInstaller) {
    $logPath = Join-Path $WorkDir "$Prefix-install.log"
    $proc = Start-Process -FilePath $installerPath -ArgumentList @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RESTARTEXITCODE=3010", "/LOG=$logPath") -Wait -PassThru
    if (($proc.ExitCode -ne 0) -and ($proc.ExitCode -ne 3010)) {
      throw "$Prefix installer failed: exit=$($proc.ExitCode)"
    }
    Write-Host "$Prefix installer exit code: $($proc.ExitCode)"
  }
}

function Start-LocalPseudoRelease {
  param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][int]$Port
  )

  if (!(Test-Path -LiteralPath $InstallerPath)) {
    throw "local installer not found: $InstallerPath"
  }

  New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
  $pseudoDir = Join-Path $WorkDir "pseudo-release"
  New-Item -ItemType Directory -Force -Path $pseudoDir | Out-Null
  $pseudoInstaller = Join-Path $pseudoDir "azookey-setup.exe"
  $pseudoSha = Join-Path $pseudoDir "SHA256SUMS.txt"
  $pseudoJson = Join-Path $pseudoDir "latest.json"

  # Serve the actual installer so the production helper directly exercises
  # its hash/lock/elevation/argument/result behavior without a wrapper process.
  Copy-Item -LiteralPath $InstallerPath -Destination $pseudoInstaller -Force
  $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $pseudoInstaller).Hash.ToLowerInvariant()
  Set-Content -LiteralPath $pseudoSha -Encoding ASCII -Value "$hash  azookey-setup.exe"

  $baseUrl = "http://127.0.0.1:$Port"
  $pseudoReleaseJson = @"
{
  "tag_name": "v999.0.0-updater-smoke",
  "name": "Updater smoke release",
  "html_url": "$baseUrl/",
  "assets": [
    {
      "name": "azookey-setup.exe",
      "browser_download_url": "$baseUrl/azookey-setup.exe"
    },
    {
      "name": "SHA256SUMS.txt",
      "browser_download_url": "$baseUrl/SHA256SUMS.txt"
    }
  ]
}
"@
  [System.IO.File]::WriteAllText(
    $pseudoJson,
    $pseudoReleaseJson,
    [System.Text.UTF8Encoding]::new($false)
  )

  $job = Start-Job -ArgumentList $pseudoDir, $Port -ScriptBlock {
    param([string]$Root, [int]$ListenPort)
    # One readiness probe plus release metadata, checksum, and installer
    # requests from both the adversarial copy and the production frontend.
    # The copied frontend completes download verification before rejecting its
    # helper outside the protected install root.
    $maxRequests = 7
    $servedRequests = 0
    $listener = [System.Net.HttpListener]::new()
    $listener.Prefixes.Add("http://127.0.0.1:$ListenPort/")
    $listener.Start()
    try {
      while ($servedRequests -lt $maxRequests) {
        $context = $listener.GetContext()
        $servedRequests++
        $name = [System.IO.Path]::GetFileName($context.Request.Url.AbsolutePath)
        if ([string]::IsNullOrWhiteSpace($name)) {
          $name = "latest.json"
        }
        $path = Join-Path $Root $name
        if (!(Test-Path -LiteralPath $path)) {
          $context.Response.StatusCode = 404
          $context.Response.Close()
          continue
        }
        $bytes = [System.IO.File]::ReadAllBytes($path)
        $context.Response.ContentLength64 = $bytes.Length
        $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
        $context.Response.Close()
      }
    } finally {
      $listener.Stop()
    }
  }

  $readyUrl = "$baseUrl/latest.json"
  for ($i = 1; $i -le 30; $i++) {
    if ($job.State -ne "Running") {
      $jobOutput = Receive-Job -Job $job -Keep -ErrorAction Continue | Out-String
      throw "pseudo release server stopped before becoming ready: state=$($job.State) output=$jobOutput"
    }

    & curl.exe -fsSL -H "User-Agent: azookey-updater-smoke" -o NUL $readyUrl
    if ($LASTEXITCODE -eq 0) {
      break
    }

    if ($i -eq 30) {
      throw "pseudo release server did not become ready: $readyUrl"
    }
    Start-Sleep -Seconds 1
  }

  [PSCustomObject]@{
    Job = $job
    ApiUrl = "$baseUrl/latest.json"
  }
}

function Get-AzookeyInstallLocation {
  $uninstallKeyName = "{80B746D4-D74D-4345-8F81-47E06BCAB515}_is1"
  $entry = @(
    "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$uninstallKeyName",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\$uninstallKeyName"
  ) | ForEach-Object {
    Get-ItemProperty -LiteralPath $_ -ErrorAction SilentlyContinue
  } | Select-Object -First 1

  if ($null -eq $entry -or [string]::IsNullOrWhiteSpace($entry.InstallLocation)) {
    throw "Azookey install location was not found after updater install."
  }
  $location = $entry.InstallLocation.Trim().Trim('"')
  Write-Host "located protected install metadata: $location"
  $location
}

function Get-OptionalRegistryValue {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Name
  )

  $item = Get-ItemProperty -LiteralPath $Path -ErrorAction SilentlyContinue
  if ($null -eq $item) {
    return $null
  }
  $property = $item.PSObject.Properties[$Name]
  if ($null -eq $property) {
    return $null
  }
  return $property.Value
}

function Assert-UpdatedProtectedInstall {
  $settingsPath = Join-Path $env:APPDATA "Azookey\settings.json"
  $learningPath = Join-Path $env:APPDATA "Azookey\LearningMemory\updater-sentinel.txt"
  $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
  if ($settings.general.punctuation_commit -ne $true) {
    throw "Updater install did not preserve the settings semantic sentinel."
  }
  if (!(Test-Path -LiteralPath $learningPath)) {
    throw "Updater install removed learning data."
  }

  $installLocation = Get-AzookeyInstallLocation
  $expected = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)) "Azookey"
  if (![string]::Equals($installLocation.TrimEnd("\"), $expected.TrimEnd("\"), [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Updater changed the protected install location: $installLocation"
  }
  $task = Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction Stop
  $expectedLauncher = Join-Path $installLocation "launcher.exe"
  if (![string]::Equals($task.Actions[0].Execute.Trim('"'), $expectedLauncher, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Updater did not recreate the protected launcher task: $($task.Actions[0].Execute)"
  }
  if ($task.Principal.RunLevel -ne "Highest") {
    throw "Updater changed the startup task run level: $($task.Principal.RunLevel)"
  }
  if (Test-Path -LiteralPath (Join-Path $installLocation ".azookey-updater-staging")) {
    throw "Successful updater run left protected installer staging behind."
  }
  $newDownloadStaging = @(
    Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter "azookey-update-*" -ErrorAction SilentlyContinue |
      Where-Object { $script:downloadStagingBefore -notcontains $_.FullName }
  )
  if ($newDownloadStaging.Count -gt 0) {
    throw "Updater left its original downloaded installer staging behind: $($newDownloadStaging.FullName -join ', ')"
  }

  $frontendHashAfterUpdate = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $installLocation "frontend.exe")).Hash
  if ($RequirePayloadChange -and $frontendHashAfterUpdate -eq $script:frontendHashBeforeUpdate) {
    throw "Updater completed without replacing the distinct frontend payload."
  }

  if ($RequirePayloadChange) {
    Write-Host "updater install preserved data, cleaned staging, recreated the protected task, and replaced the distinct payload"
  } else {
    Write-Host "updater install preserved data, cleaned staging, and recreated the protected task"
  }
}

function Invoke-ProductionFrontendUpdater {
  param(
    [Parameter(Mandatory = $true)][string]$ReleaseApiUrl,
    [Parameter(Mandatory = $true)][int]$TimeoutSec
  )

  $installLocation = Get-AzookeyInstallLocation
  $frontendPath = Join-Path $installLocation "frontend.exe"
  $markerPath = Join-Path $installLocation ".azookey-updater-integration-test"
  $resultPath = Join-Path $env:APPDATA "Azookey\update-result.json"
  $launchingUserSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  $resultRegistryPath = "Registry::HKEY_USERS\$launchingUserSid\Software\Azookey"
  $resultRegistryName = "UpdateResultJson"
  $requestRegistryPath = "HKCU:\Software\Azookey"
  $requestRegistryName = "PendingUpdateRequestId"
  Set-Content -LiteralPath $markerPath -Encoding ASCII -Value "VM-only protected updater integration marker"
  Remove-Item -LiteralPath $resultPath -Force -ErrorAction SilentlyContinue
  Remove-ItemProperty -LiteralPath $resultRegistryPath -Name $resultRegistryName -ErrorAction SilentlyContinue
  Remove-ItemProperty -LiteralPath $requestRegistryPath -Name $requestRegistryName -ErrorAction SilentlyContinue

  $previousReleaseApiUrl = $env:AZOOKEY_UPDATE_RELEASE_API_URL
  $previousCurrentVersion = $env:AZOOKEY_UPDATE_CURRENT_VERSION
  try {
    $env:AZOOKEY_UPDATE_RELEASE_API_URL = $ReleaseApiUrl
    $env:AZOOKEY_UPDATE_CURRENT_VERSION = "0.0.1"
    $stdoutPath = Join-Path $WorkDir "production-updater-stdout.log"
    $stderrPath = Join-Path $WorkDir "production-updater-stderr.log"
    Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
    $frontend = Start-Process -FilePath $frontendPath `
      -ArgumentList "--azookey-updater-integration-test" `
      -RedirectStandardOutput $stdoutPath `
      -RedirectStandardError $stderrPath `
      -PassThru
    if (-not $frontend.WaitForExit(120000)) {
      Stop-Process -Id $frontend.Id -Force -ErrorAction SilentlyContinue
      throw "Production frontend updater entry point timed out."
    }
    # With redirected GUI-subsystem processes, PowerShell may leave ExitCode
    # unset after the timed WaitForExit overload until handles are drained and
    # the Process object is refreshed.
    $frontend.WaitForExit()
    $frontend.Refresh()
    $frontendExitCode = $frontend.ExitCode
    $stderr = if (Test-Path -LiteralPath $stderrPath) {
      Get-Content -LiteralPath $stderrPath -Raw
    } else {
      "<stderr unavailable>"
    }
    # The installer deliberately stops frontend.exe after the protected helper
    # has been launched, so this launcher process may be terminated before a
    # stable exit code is observable. The helper's explicit user-hive result
    # below is the authoritative completion signal.
    Write-Host "production updater launcher exited: exit=$frontendExitCode stderr=$stderr"

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $resultEnvelopeJson = $null
    while (($null -eq $resultEnvelopeJson) -and ((Get-Date) -lt $deadline)) {
      $resultEnvelopeJson = Get-OptionalRegistryValue -Path $resultRegistryPath -Name $resultRegistryName
      Start-Sleep -Seconds 1
    }
    if ($null -eq $resultEnvelopeJson) {
      throw "Production frontend updater did not publish its protected registry result. launcher_exit=$frontendExitCode stderr=$stderr"
    }

    $pendingRequestId = Get-OptionalRegistryValue -Path $requestRegistryPath -Name $requestRegistryName
    $resultEnvelope = $resultEnvelopeJson | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace($pendingRequestId) -or
        $resultEnvelope.request_id -ne $pendingRequestId) {
      throw "Explicit launching-user hive result was not bound to its pending request."
    }
    $result = $resultEnvelope.result
    if ($result.status -ne "success") {
      throw "Production frontend updater reported failure: $($result | ConvertTo-Json -Compress)"
    }
    if (($result.exit_code -ne 0) -and ($result.exit_code -ne 3010)) {
      throw "Production frontend updater returned an unexpected installer exit code: $($result.exit_code)"
    }
    Write-Host "production frontend updater completed: exit=$($result.exit_code) restart=$($result.needs_restart)"

    # A GUI process launched over SSH runs outside the logged-on desktop and
    # cannot reliably initialize WebView2. The Rust one-shot reader is covered
    # by unit tests; here remove the explicit user-hive result and request
    # after verifying their correlation.
    Remove-ItemProperty -LiteralPath $resultRegistryPath -Name $resultRegistryName -ErrorAction Stop
    Remove-ItemProperty -LiteralPath $requestRegistryPath -Name $requestRegistryName -ErrorAction Stop
    $remainingResult = Get-OptionalRegistryValue -Path $resultRegistryPath -Name $resultRegistryName
    $remainingRequest = Get-OptionalRegistryValue -Path $requestRegistryPath -Name $requestRegistryName
    if (($null -ne $remainingResult) -or ($null -ne $remainingRequest)) {
      throw "Protected updater result/request could not be removed after verification."
    }
    Write-Host "updater result was published to the explicit launching-user hive and removed without stale state"
  } finally {
    $env:AZOOKEY_UPDATE_RELEASE_API_URL = $previousReleaseApiUrl
    $env:AZOOKEY_UPDATE_CURRENT_VERSION = $previousCurrentVersion
    Remove-Item -LiteralPath $markerPath -Force -ErrorAction SilentlyContinue
  }
}

function Assert-CopiedFrontendCannotElevateHelper {
  param(
    [Parameter(Mandatory = $true)][string]$ReleaseApiUrl
  )

  $installLocation = Get-AzookeyInstallLocation
  $copiedRoot = Join-Path $WorkDir "copied-frontend"
  New-Item -ItemType Directory -Force -Path $copiedRoot | Out-Null
  foreach ($name in @("frontend.exe", "azookey-updater-helper.exe")) {
    Copy-Item -LiteralPath (Join-Path $installLocation $name) -Destination (Join-Path $copiedRoot $name) -Force
  }
  Set-Content -LiteralPath (Join-Path $copiedRoot ".azookey-updater-integration-test") -Encoding ASCII -Value "untrusted copied marker"

  $resultPath = Join-Path $env:APPDATA "Azookey\update-result.json"
  $launchingUserSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  $resultRegistryPath = "Registry::HKEY_USERS\$launchingUserSid\Software\Azookey"
  $resultRegistryName = "UpdateResultJson"
  $requestRegistryPath = "HKCU:\Software\Azookey"
  $requestRegistryName = "PendingUpdateRequestId"
  Remove-Item -LiteralPath $resultPath -Force -ErrorAction SilentlyContinue
  Remove-ItemProperty -LiteralPath $resultRegistryPath -Name $resultRegistryName -ErrorAction SilentlyContinue
  Remove-ItemProperty -LiteralPath $requestRegistryPath -Name $requestRegistryName -ErrorAction SilentlyContinue
  $previousReleaseApiUrl = $env:AZOOKEY_UPDATE_RELEASE_API_URL
  $previousCurrentVersion = $env:AZOOKEY_UPDATE_CURRENT_VERSION
  try {
    $env:AZOOKEY_UPDATE_RELEASE_API_URL = $ReleaseApiUrl
    $env:AZOOKEY_UPDATE_CURRENT_VERSION = "0.0.1"
    $stdoutPath = Join-Path $WorkDir "copied-updater-stdout.log"
    $stderrPath = Join-Path $WorkDir "copied-updater-stderr.log"
    $frontend = Start-Process -FilePath (Join-Path $copiedRoot "frontend.exe") `
      -ArgumentList "--azookey-updater-integration-test" `
      -RedirectStandardOutput $stdoutPath `
      -RedirectStandardError $stderrPath `
      -PassThru
    if (-not $frontend.WaitForExit(120000)) {
      Stop-Process -Id $frontend.Id -Force -ErrorAction SilentlyContinue
      throw "Copied frontend updater rejection timed out."
    }
    $frontend.WaitForExit()
    $frontend.Refresh()
    if ($frontend.ExitCode -eq 0) {
      throw "Copied frontend was allowed to launch an updater helper from a user-writable directory."
    }
    $stderr = if (Test-Path -LiteralPath $stderrPath) {
      Get-Content -LiteralPath $stderrPath -Raw
    } else {
      ""
    }
    if ($stderr.IndexOf("outside the protected install directory") -lt 0) {
      throw "Copied frontend failed for an unexpected reason: exit=$($frontend.ExitCode) stderr=$stderr"
    }
    if ((Test-Path -LiteralPath $resultPath) -or
        ($null -ne (Get-OptionalRegistryValue -Path $resultRegistryPath -Name $resultRegistryName)) -or
        ($null -ne (Get-OptionalRegistryValue -Path $requestRegistryPath -Name $requestRegistryName))) {
      throw "Copied frontend unexpectedly launched the elevated updater helper."
    }
    Write-Host "copied frontend cannot elevate a helper outside Program Files"
  } finally {
    $env:AZOOKEY_UPDATE_RELEASE_API_URL = $previousReleaseApiUrl
    $env:AZOOKEY_UPDATE_CURRENT_VERSION = $previousCurrentVersion
  }
}

Test-ReleaseDownload -ReleaseApiUrl "https://api.github.com/repos/batao9/azooKey-Windows/releases/latest" -Prefix "official"
$script:downloadStagingBefore = @(
  Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter "azookey-update-*" -ErrorAction SilentlyContinue |
    ForEach-Object FullName
)
$initialInstallLocation = Get-AzookeyInstallLocation
$script:frontendHashBeforeUpdate = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $initialInstallLocation "frontend.exe")).Hash
$appDataRoot = Join-Path $env:APPDATA "Azookey"
$learningRoot = Join-Path $appDataRoot "LearningMemory"
New-Item -ItemType Directory -Force -Path $learningRoot | Out-Null
$settingsPath = Join-Path $appDataRoot "settings.json"
$settingsSentinel = '{"version":"0.1.2","zenzai":{"enable":false,"profile":"","backend":"cpu"},"general":{"punctuation_commit":true}}'
[System.IO.File]::WriteAllText($settingsPath, $settingsSentinel, [System.Text.UTF8Encoding]::new($false))
Set-Content -LiteralPath (Join-Path $learningRoot "updater-sentinel.txt") -Encoding UTF8 -Value "preserve learning"
$pseudo = Start-LocalPseudoRelease -InstallerPath $LocalInstallerPath -Port $PseudoReleasePort
try {
  Assert-CopiedFrontendCannotElevateHelper -ReleaseApiUrl $pseudo.ApiUrl
  Invoke-ProductionFrontendUpdater -ReleaseApiUrl $pseudo.ApiUrl -TimeoutSec $UpdaterTimeoutSec
  Assert-UpdatedProtectedInstall
} finally {
  Wait-Job -Job $pseudo.Job -Timeout 10 -ErrorAction SilentlyContinue | Out-Null
  Stop-Job -Job $pseudo.Job -ErrorAction SilentlyContinue
  Remove-Job -Job $pseudo.Job -Force -ErrorAction SilentlyContinue
}
PS1
}

main() {
  if [[ $# -gt 1 ]]; then
    echo "Usage: $0 [installer-path|latest]"
    exit 1
  fi

  local installer
  installer="$(find_installer "${1:-latest}")"
  local base_installer="$installer"
  local payload_change_switch=""
  if [[ -n "$UPDATER_BASE_INSTALLER" ]]; then
    base_installer="$(find_installer "$UPDATER_BASE_INSTALLER")"
    if ! cmp -s "$base_installer" "$installer"; then
      payload_change_switch="-RequirePayloadChange"
    fi
  fi
  ensure_preconditions
  trap cleanup EXIT

  log "疑似 release は VM 内 localhost に準備します: $(basename "$installer")"

  log "検証用 VM にベースインストーラーを事前インストールします: $(basename "$base_installer")"
  SHUTDOWN_AFTER_INSTALL=0 "$REPO_ROOT/scripts/vm_stage_for_manual_test.sh" "$base_installer"
  if ! wait_for_ssh; then
    log "VM への SSH 接続に失敗しました"
    exit 1
  fi

  TMP_REMOTE_PS="$(mktemp /tmp/azookey-updater-smoke.XXXXXX.ps1)"
  create_smoke_ps1 "$TMP_REMOTE_PS"
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"
  scp_to_vm "$installer" "$REMOTE_TARGET_INSTALLER_SCP"

  local remote_work_dir="$REMOTE_TMP_WIN\\azookey-updater-smoke-$TIMESTAMP"
  log "VM 上で official latest download/hash と疑似 release install を検証します"
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -LocalInstallerPath \"$REMOTE_TARGET_INSTALLER_WIN\" -PseudoReleasePort $PSEUDO_RELEASE_PORT -WorkDir \"$remote_work_dir\" -UpdaterTimeoutSec $UPDATER_TIMEOUT_SEC $payload_change_switch"
  log "updater smoke が完了しました"
}

main "$@"
