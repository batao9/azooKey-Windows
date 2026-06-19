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
  [Parameter(Mandatory = $true)][string]$WorkDir
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

  Copy-Item -LiteralPath $InstallerPath -Destination $pseudoInstaller -Force
  $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $pseudoInstaller).Hash.ToLowerInvariant()
  Set-Content -LiteralPath $pseudoSha -Encoding ASCII -Value "$hash  azookey-setup.exe"

  $baseUrl = "http://127.0.0.1:$Port"
  @"
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
"@ | Set-Content -LiteralPath $pseudoJson -Encoding UTF8

  $job = Start-Job -ArgumentList $pseudoDir, $Port -ScriptBlock {
    param([string]$Root, [int]$ListenPort)
    $maxRequests = 4
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

Test-ReleaseDownload -ReleaseApiUrl "https://api.github.com/repos/batao9/azooKey-Windows/releases/latest" -Prefix "official"
$pseudo = Start-LocalPseudoRelease -InstallerPath $LocalInstallerPath -Port $PseudoReleasePort
try {
  Test-ReleaseDownload -ReleaseApiUrl $pseudo.ApiUrl -Prefix "pseudo" -RunInstaller
} finally {
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
  ensure_preconditions
  trap cleanup EXIT

  log "疑似 release は VM 内 localhost に準備します: $(basename "$installer")"

  log "検証用 VM にインストーラーを事前インストールします"
  SHUTDOWN_AFTER_INSTALL=0 "$REPO_ROOT/scripts/vm_stage_for_manual_test.sh" "$installer"
  if ! wait_for_ssh; then
    log "VM への SSH 接続に失敗しました"
    exit 1
  fi

  TMP_REMOTE_PS="$(mktemp /tmp/azookey-updater-smoke.XXXXXX.ps1)"
  create_smoke_ps1 "$TMP_REMOTE_PS"
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"

  local remote_work_dir="$REMOTE_TMP_WIN\\azookey-updater-smoke-$TIMESTAMP"
  log "VM 上で official latest download/hash と疑似 release install を検証します"
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -LocalInstallerPath \"$REMOTE_TMP_WIN\\azookey-setup-under-test.exe\" -PseudoReleasePort $PSEUDO_RELEASE_PORT -WorkDir \"$remote_work_dir\""
  log "updater smoke が完了しました"
}

main "$@"
