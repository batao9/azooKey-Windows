#!/usr/bin/env bash
# Windows 上の VirtualBox VM 内でインストーラーのインストールを行うためのスクリプトです。
# 出力が長くなりがちなので、サブエージェントから呼び出すことを推奨します。

set -euo pipefail

VM_NAME="${VM_NAME:-}"
SNAPSHOT_NAME="${SNAPSHOT_NAME:-}"
SSH_USER="${SSH_USER:-}"
SSH_PORT="${SSH_PORT:-}"
SSH_KEY="${SSH_KEY:-}"
VBOX_MANAGE="${VBOX_MANAGE:-}"
INSTALL_TIMEOUT_SEC="${INSTALL_TIMEOUT_SEC:-1200}"
SHUTDOWN_AFTER_INSTALL="${SHUTDOWN_AFTER_INSTALL:-1}"
UNINSTALL_AFTER_INSTALL="${UNINSTALL_AFTER_INSTALL:-0}"

if [[ -z "$VBOX_MANAGE" ]]; then
  if command -v VBoxManage >/dev/null 2>&1; then
    VBOX_MANAGE="$(command -v VBoxManage)"
  elif [[ -x "/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe" ]]; then
    VBOX_MANAGE="/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe"
  fi
fi

DEFAULT_GATEWAY_IP="$(ip route | awk '/default/ {print $3; exit}' || true)"
HOST_IP="${SSH_HOST:-${DEFAULT_GATEWAY_IP:-127.0.0.1}}"
FALLBACK_HOST=""
if [[ "$HOST_IP" == "127.0.0.1" && -n "$DEFAULT_GATEWAY_IP" ]]; then
  FALLBACK_HOST="$DEFAULT_GATEWAY_IP"
elif [[ "$HOST_IP" != "127.0.0.1" ]]; then
  FALLBACK_HOST="127.0.0.1"
fi
ACTIVE_HOST="$HOST_IP"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="$REPO_ROOT/.local/artifacts"
LOG_DIR="$REPO_ROOT/.local/logs"
mkdir -p "$ARTIFACT_DIR" "$LOG_DIR"

REMOTE_TMP_WIN="C:\\Users\\$SSH_USER\\AppData\\Local\\Temp"
REMOTE_INSTALLER_WIN="$REMOTE_TMP_WIN\\azookey-setup-under-test.exe"
REMOTE_PS_WIN="$REMOTE_TMP_WIN\\azookey-install-under-test.ps1"
REMOTE_INSTALL_LOG_WIN="$REMOTE_TMP_WIN\\azookey-install-under-test.log"
REMOTE_UNINSTALL_LOG_WIN="$REMOTE_TMP_WIN\\azookey-uninstall-under-test.log"

REMOTE_INSTALLER_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-setup-under-test.exe"
REMOTE_PS_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-install-under-test.ps1"
REMOTE_INSTALL_LOG_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-install-under-test.log"
REMOTE_UNINSTALL_LOG_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-uninstall-under-test.log"

SESSION_KNOWN_HOSTS="$(mktemp /tmp/vm-stage-known-hosts.XXXXXX)"
TMP_REMOTE_PS=""

SSH_OPTS=(
  -i "$SSH_KEY"
  -p "$SSH_PORT"
  -o "UserKnownHostsFile=$SESSION_KNOWN_HOSTS"
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=8
)
SCP_OPTS=(
  -i "$SSH_KEY"
  -P "$SSH_PORT"
  -o "UserKnownHostsFile=$SESSION_KNOWN_HOSTS"
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=8
)

log() {
  printf '[vm-stage] %s\n' "$*"
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

snapshot_exists() {
  vbox snapshot "$VM_NAME" list --machinereadable | matches_fixed "=\"$SNAPSHOT_NAME\""
}

ssh_run() {
  ssh "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" "$@"
}

scp_to_vm() {
  scp "${SCP_OPTS[@]}" "$1" "$SSH_USER@$ACTIVE_HOST:$2"
}

scp_from_vm() {
  scp "${SCP_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST:$1" "$2"
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

shutdown_vm_after_install() {
  if ! is_vm_running; then
    log "VM '$VM_NAME' はすでに停止しています"
    return 0
  fi

  log "インストール完了後のため VM を停止します: $VM_NAME"
  vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
  if wait_for_vm_poweroff; then
    log "VM を停止しました: $VM_NAME"
    return 0
  fi

  log "通常停止できなかったため poweroff します"
  vbox controlvm "$VM_NAME" poweroff >/dev/null || true
  if wait_for_vm_poweroff; then
    log "VM を停止しました: $VM_NAME"
    return 0
  fi

  log "VM の停止に失敗しました: $VM_NAME"
  return 1
}

cleanup() {
  set +e
  rm -f "${TMP_REMOTE_PS:-}" "$SESSION_KNOWN_HOSTS"
}

find_installer() {
  local arg="${1:-latest}"
  if [[ "$arg" != "latest" ]]; then
    if [[ ! -f "$arg" ]]; then
      log "指定インストーラーが見つかりません: $arg"
      exit 1
    fi
    realpath "$arg"
    return 0
  fi

  local latest
  latest="$(ls -t "$ARTIFACT_DIR"/azookey-setup-*.exe 2>/dev/null | head -n 1 || true)"
  if [[ -z "$latest" ]]; then
    log "インストーラーが見つかりません。先にビルドを実行してください。"
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
  if ! snapshot_exists; then
    log "スナップショットが見つかりません: $SNAPSHOT_NAME"
    exit 1
  fi
}

restore_snapshot_and_boot() {
  if is_vm_running; then
    log "スナップショット復元のため VM を停止します"
    vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
    if ! wait_for_vm_poweroff; then
      vbox controlvm "$VM_NAME" poweroff >/dev/null || true
    fi
  fi

  log "スナップショットへ復元します: $SNAPSHOT_NAME"
  vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null
  # saved state付きスナップショットでも必ずコールドブートさせる
  vbox discardstate "$VM_NAME" >/dev/null 2>&1 || true

  log "VM を起動します: $VM_NAME"
  vbox startvm "$VM_NAME" --type headless >/dev/null

  if ! wait_for_ssh; then
    log "VM への SSH 接続に失敗しました。SSH_HOST/SSH_PORT/SSH_USER を確認してください。"
    exit 1
  fi
}

create_install_ps1() {
  local ps1="$1"
  cat > "$ps1" <<'PS1'
param(
  [Parameter(Mandatory = $true)][string]$InstallerPath,
  [Parameter(Mandatory = $true)][string]$InstallLogPath,
  [Parameter(Mandatory = $true)][string]$UninstallLogPath,
  [Parameter(Mandatory = $true)][int]$InstallerTimeoutSec,
  [switch]$UninstallAfterInstall
)

$ErrorActionPreference = "Stop"

if (!(Test-Path $InstallerPath)) {
  throw "Installer not found: $InstallerPath"
}

function Test-VCRuntimeInstalled {
  param([Parameter(Mandatory = $true)][string]$Arch)
  $keys = @(
    "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\$Arch",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\$Arch"
  )
  foreach ($key in $keys) {
    try {
      $entry = Get-ItemProperty -Path $key -ErrorAction Stop
      if ($entry.Installed -eq 1) {
        return $true
      }
    } catch {
    }
  }
  return $false
}

function Stop-ProcessTree {
  param([Parameter(Mandatory = $true)][int]$RootPid)

  $queue = @($RootPid)
  $allChildren = @()
  while ($queue.Count -gt 0) {
    $current = $queue[0]
    if ($queue.Count -eq 1) {
      $queue = @()
    } else {
      $queue = $queue[1..($queue.Count - 1)]
    }

    $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $current" -ErrorAction SilentlyContinue)
    foreach ($child in $children) {
      $childPid = [int]$child.ProcessId
      $allChildren += $childPid
      $queue += $childPid
    }
  }

  foreach ($processId in ($allChildren | Sort-Object -Descending -Unique)) {
    Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
  }
  Stop-Process -Id $RootPid -Force -ErrorAction SilentlyContinue
}

function Get-AzookeyUninstallEntries {
  @(
    "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*",
    "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*"
  ) | ForEach-Object {
    Get-ItemProperty -Path $_ -ErrorAction SilentlyContinue
  } | Where-Object {
    $_.DisplayName -like "*Azookey*"
  }
}

function Normalize-RegistryPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  $Path.Trim().Trim('"')
}

function Assert-RequiredInstallFiles {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $requiredPaths = @(
    "frontend.exe",
    "azookey-server.exe",
    "ui.exe",
    "launcher.exe",
    "azookey.dll",
    "azookey32.dll",
    "launch.vbs",
    "Dictionary",
    "EmojiDictionary",
    "zenz.gguf",
    "llama_cpu",
    "llama_cuda",
    "llama_vulkan"
  )

  $missing = @()
  foreach ($relativePath in $requiredPaths) {
    $path = Join-Path $InstallLocation $relativePath
    if (!(Test-Path $path)) {
      $missing += $relativePath
    }
  }

  if ($missing.Count -gt 0) {
    throw "Required installed files are missing: $($missing -join ', ')"
  }
}

function Test-WebView2RuntimeInstalled {
  $clientId = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  foreach ($key in @(
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$clientId",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$clientId"
  )) {
    $value = Get-ItemProperty -Path $key -Name "pv" -ErrorAction SilentlyContinue
    if ($value -and ![string]::IsNullOrWhiteSpace($value.pv)) {
      return $true
    }
  }

  return $false
}

if (-not (Test-VCRuntimeInstalled -Arch "x64")) {
  throw "VC++ runtime x64 is missing. Restore a VC-ready snapshot first."
}
if (-not (Test-VCRuntimeInstalled -Arch "x86")) {
  throw "VC++ runtime x86 is missing. Restore a VC-ready snapshot first."
}

$args = @(
  "/SP-",
  "/VERYSILENT",
  "/SUPPRESSMSGBOXES",
  "/NORESTART",
  "/LOG=$InstallLogPath"
)

Write-Host "installing: $InstallerPath"
Write-Host "timeout(sec): $InstallerTimeoutSec"
$proc = Start-Process -FilePath $InstallerPath -ArgumentList $args -PassThru
if (-not $proc.WaitForExit($InstallerTimeoutSec * 1000)) {
  Stop-ProcessTree -RootPid $proc.Id
  throw "Installer timed out."
}
Write-Host "installer exit code: $($proc.ExitCode)"

if ($proc.ExitCode -ne 0) {
  throw "Installer failed. ExitCode=$($proc.ExitCode)"
}

$entries = @(Get-AzookeyUninstallEntries)

if (-not $entries) {
  throw "Azookey uninstall entry not found after install."
}
if ($entries.Count -ne 1) {
  throw "Expected exactly one Azookey uninstall entry after install, found $($entries.Count)."
}

$entry = $entries[0]
if ([string]::IsNullOrWhiteSpace($entry.InstallLocation)) {
  throw "Azookey uninstall entry does not contain InstallLocation."
}
if ($entry.MainBinaryName -ne "frontend.exe") {
  throw "Azookey uninstall entry MainBinaryName is not frontend.exe: $($entry.MainBinaryName)"
}

$installLocation = Normalize-RegistryPath -Path $entry.InstallLocation
Assert-RequiredInstallFiles -InstallLocation $installLocation
if (-not (Test-WebView2RuntimeInstalled)) {
  throw "WebView2 Runtime is missing after install."
}

Write-Host "install complete. entry found: $($entry.PSChildName)"
Write-Host "install location: $installLocation"
Write-Host "WebView2 Runtime installed"

if ($UninstallAfterInstall) {
  $uninstallCommand = $entry.UninstallString

  if ([string]::IsNullOrWhiteSpace($uninstallCommand)) {
    throw "Azookey uninstall command not found."
  }

  $uninstallerPath = Normalize-RegistryPath -Path $uninstallCommand
  $uninstallArgs = @(
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/LOG=$UninstallLogPath"
  )

  Write-Host "uninstalling: $uninstallerPath"
  $uninstallProc = Start-Process -FilePath $uninstallerPath -ArgumentList $uninstallArgs -PassThru
  if (-not $uninstallProc.WaitForExit($InstallerTimeoutSec * 1000)) {
    Stop-ProcessTree -RootPid $uninstallProc.Id
    throw "Uninstaller timed out."
  }
  Write-Host "uninstaller exit code: $($uninstallProc.ExitCode)"

  if ($uninstallProc.ExitCode -ne 0) {
    throw "Uninstaller failed. ExitCode=$($uninstallProc.ExitCode)"
  }

  $remainingEntries = @(Get-AzookeyUninstallEntries)
  if ($remainingEntries.Count -ne 0) {
    throw "Azookey uninstall entry still exists after uninstall. Count=$($remainingEntries.Count)"
  }

  foreach ($relativePath in @("frontend.exe", "azookey.dll", "azookey32.dll", "launcher.exe")) {
    $path = Join-Path $installLocation $relativePath
    if (Test-Path $path) {
      throw "Installed file still exists after uninstall: $path"
    }
  }

  Write-Host "uninstall complete. entries found: 0"
}
PS1
}

main() {
  local installer
  installer="$(find_installer "${1:-latest}")"

  ensure_preconditions
  restore_snapshot_and_boot

  TMP_REMOTE_PS="$(mktemp /tmp/azookey-install-under-test.XXXXXX.ps1)"
  create_install_ps1 "$TMP_REMOTE_PS"

  log "インストーラーを VM に転送します: $(basename "$installer")"
  scp_to_vm "$installer" "$REMOTE_INSTALLER_SCP"
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"

  log "VM でインストーラーを実行します（サイレント）"
  local install_rc=0
  local uninstall_switch=""
  case "${UNINSTALL_AFTER_INSTALL,,}" in
    1|true|yes|on)
      uninstall_switch="-UninstallAfterInstall"
      ;;
  esac
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -InstallerPath \"$REMOTE_INSTALLER_WIN\" -InstallLogPath \"$REMOTE_INSTALL_LOG_WIN\" -UninstallLogPath \"$REMOTE_UNINSTALL_LOG_WIN\" -InstallerTimeoutSec $INSTALL_TIMEOUT_SEC $uninstall_switch" || install_rc=$?
  if [[ "$install_rc" -ne 0 ]]; then
    log "インストーラー実行が失敗しました。ログ回収と VM 後処理を継続します: exit=$install_rc"
  fi

  local ts local_log
  ts="$(date +%Y%m%d-%H%M%S)"
  local_log="$LOG_DIR/vm-install-$ts.log"
  if scp_from_vm "$REMOTE_INSTALL_LOG_SCP" "$local_log" >/dev/null 2>&1; then
    log "インストールログを回収しました: $local_log"
  else
    log "インストールログの回収に失敗しました（処理は継続）"
  fi

  if [[ -n "$uninstall_switch" ]]; then
    local local_uninstall_log
    local_uninstall_log="$LOG_DIR/vm-uninstall-$ts.log"
    if scp_from_vm "$REMOTE_UNINSTALL_LOG_SCP" "$local_uninstall_log" >/dev/null 2>&1; then
      log "アンインストールログを回収しました: $local_uninstall_log"
    else
      log "アンインストールログの回収に失敗しました（処理は継続）"
    fi
  fi

  case "${SHUTDOWN_AFTER_INSTALL,,}" in
    0|false|no|off)
      log "完了: VM '$VM_NAME' は起動したままです。手動検証を開始してください。"
      ;;
    *)
      shutdown_vm_after_install
      log "完了: VM '$VM_NAME' は停止済みです。手動検証で残す場合は SHUTDOWN_AFTER_INSTALL=0 を指定してください。"
      ;;
  esac

  if [[ "$install_rc" -ne 0 ]]; then
    log "インストーラー実行失敗として終了します: exit=$install_rc"
    return "$install_rc"
  fi
}

trap cleanup EXIT
main "$@"
