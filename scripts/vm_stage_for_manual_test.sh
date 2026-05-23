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
VERIFY_LEGACY_NSIS_MIGRATION="${VERIFY_LEGACY_NSIS_MIGRATION:-0}"

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
  [switch]$VerifyLegacyNsisMigration,
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

function Install-FakeLegacyNsisInstall {
  $legacyRoot = Join-Path $env:LOCALAPPDATA "Azookey"
  $legacyUninstallKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey"
  $legacyStateKey = "HKCU:\SOFTWARE\batao9\Azookey"

  Remove-Item -LiteralPath $legacyRoot -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $legacyUninstallKey -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $legacyStateKey -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath (Join-Path $env:TEMP "azookey-legacy-nsis-uninstalled.txt") -Force -ErrorAction SilentlyContinue

  New-Item -ItemType Directory -Force -Path $legacyRoot | Out-Null
  Set-Content -LiteralPath (Join-Path $legacyRoot "legacy-payload.txt") -Encoding UTF8 -Value "legacy nsis payload"

  $fakeUninstallerSource = @"
using System;
using System.Diagnostics;
using System.IO;
using Microsoft.Win32;

public static class Program {
  public static int Main(string[] args) {
    var installRoot = AppDomain.CurrentDomain.BaseDirectory.TrimEnd(Path.DirectorySeparatorChar);
    var sentinelPath = Path.Combine(Path.GetTempPath(), "azookey-legacy-nsis-uninstalled.txt");
    File.WriteAllText(sentinelPath, string.Join(" ", args));
    Registry.CurrentUser.DeleteSubKeyTree(@"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey", false);
    Registry.CurrentUser.DeleteSubKeyTree(@"SOFTWARE\batao9\Azookey", false);
    Process.Start(new ProcessStartInfo("cmd.exe", "/C ping 127.0.0.1 -n 2 > nul & rmdir /S /Q \"" + installRoot + "\"") {
      CreateNoWindow = true,
      UseShellExecute = false,
      WorkingDirectory = Path.GetTempPath()
    });
    return 0;
  }
}
"@

  Add-Type -TypeDefinition $fakeUninstallerSource -OutputAssembly (Join-Path $legacyRoot "uninstall.exe") -OutputType ConsoleApplication

  New-Item -ItemType Directory -Force -Path $legacyUninstallKey | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "DisplayName" -Value "Azookey" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "Publisher" -Value "batao9" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "DisplayVersion" -Value "0.1.0-legacy" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "InstallLocation" -Value "`"$legacyRoot`"" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "UninstallString" -Value "`"$(Join-Path $legacyRoot "uninstall.exe")`"" -PropertyType String -Force | Out-Null

  New-Item -ItemType Directory -Force -Path $legacyStateKey | Out-Null
  Set-Item -Path $legacyStateKey -Value $legacyRoot

  Write-Host "created fake legacy NSIS install: $legacyRoot"
}

function Assert-FakeLegacyNsisMigrated {
  $legacyRoot = Join-Path $env:LOCALAPPDATA "Azookey"
  $legacyUninstallKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey"
  $legacyStateKey = "HKCU:\SOFTWARE\batao9\Azookey"
  $sentinelPath = Join-Path $env:TEMP "azookey-legacy-nsis-uninstalled.txt"

  $deadline = (Get-Date).AddSeconds(30)
  while ((Test-Path -LiteralPath $legacyRoot) -and ((Get-Date) -lt $deadline)) {
    Start-Sleep -Seconds 1
  }

  if (Test-Path -LiteralPath $legacyRoot) {
    throw "Legacy NSIS install directory remains after migration: $legacyRoot"
  }
  if (Test-Path -LiteralPath $legacyUninstallKey) {
    throw "Legacy NSIS uninstall key remains after migration: $legacyUninstallKey"
  }
  if (Test-Path -LiteralPath $legacyStateKey) {
    throw "Legacy NSIS state key remains after migration: $legacyStateKey"
  }
  if (!(Test-Path -LiteralPath $sentinelPath)) {
    throw "Legacy NSIS uninstaller was not executed."
  }

  $sentinel = Get-Content -LiteralPath $sentinelPath -Raw
  if ($sentinel -notmatch "(^|\s)/S(\s|$)") {
    throw "Legacy NSIS uninstaller was not invoked silently: $sentinel"
  }

  Write-Host "legacy NSIS install migrated before new install"
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

function Assert-OnlySettingsRemainAfterUninstall {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  if (!(Test-Path -LiteralPath $InstallLocation)) {
    Write-Host "install directory removed after uninstall"
    return
  }

  $allowedSettingsPath = Join-Path $InstallLocation "settings.json"
  $remaining = @(
    Get-ChildItem -LiteralPath $InstallLocation -Force -Recurse -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -ne $allowedSettingsPath }
  )

  if ($remaining.Count -gt 0) {
    $sample = @($remaining | Select-Object -First 20 | ForEach-Object {
      $_.FullName.Substring($InstallLocation.Length).TrimStart("\")
    })
    throw "Unexpected files remain after uninstall: $($sample -join ', ')"
  }

  Write-Host "install directory contains only settings.json after uninstall"
}

function Wait-ForUninstallerSelfCleanup {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $deadline = (Get-Date).AddSeconds(30)
  do {
    $remainingUninstallerFiles = @()
    foreach ($pattern in @("unins*.exe", "unins*.dat")) {
      $remainingUninstallerFiles += @(
        Get-ChildItem -Path (Join-Path $InstallLocation $pattern) -Force -ErrorAction SilentlyContinue
      )
    }
    if ($remainingUninstallerFiles.Count -eq 0) {
      return
    }
    Start-Sleep -Seconds 1
  } while ((Get-Date) -lt $deadline)
}

function Ensure-SettingsFileForUninstallVerification {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $settingsPath = Join-Path $InstallLocation "settings.json"
  if (!(Test-Path -LiteralPath $settingsPath)) {
    Set-Content -LiteralPath $settingsPath -Encoding UTF8 -Value '{"createdBy":"vm_stage_for_manual_test"}'
    Write-Host "created settings.json sentinel for uninstall verification"
  }
}

function Ensure-GeneratedAppDataForUninstallVerification {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  foreach ($relativePath in @(
    "EngineRuntime",
    "frontend.exe.WebView2",
    "logs",
    "ui.exe.WebView2"
  )) {
    $dir = Join-Path $InstallLocation $relativePath
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-Content -LiteralPath (Join-Path $dir "uninstall-sentinel.txt") -Encoding UTF8 -Value "generated app data cleanup sentinel"
  }

  Write-Host "created generated app data sentinels for uninstall verification"
}

function Assert-NoExternalAzookeyDirectoriesAfterUninstall {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $candidateRoots = @()
  if (![string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    $candidateRoots += Join-Path $env:LOCALAPPDATA "Azookey"
  }
  foreach ($tempRoot in @($env:TEMP, $env:TMP)) {
    if (![string]::IsNullOrWhiteSpace($tempRoot)) {
      $candidateRoots += Join-Path $tempRoot "Azookey"
    }
  }

  foreach ($path in @($candidateRoots | Select-Object -Unique)) {
    if ($path -eq $InstallLocation) {
      continue
    }
    if (Test-Path -LiteralPath $path) {
      throw "Unexpected external Azookey directory remains after uninstall: $path"
    }
  }

  Write-Host "no external Azookey directories remain after uninstall"
}

if (-not (Test-VCRuntimeInstalled -Arch "x64")) {
  throw "VC++ runtime x64 is missing. Restore a VC-ready snapshot first."
}
if (-not (Test-VCRuntimeInstalled -Arch "x86")) {
  throw "VC++ runtime x86 is missing. Restore a VC-ready snapshot first."
}

if ($VerifyLegacyNsisMigration) {
  Install-FakeLegacyNsisInstall
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

if ($VerifyLegacyNsisMigration) {
  Assert-FakeLegacyNsisMigrated
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
  Ensure-SettingsFileForUninstallVerification -InstallLocation $installLocation
  Ensure-GeneratedAppDataForUninstallVerification -InstallLocation $installLocation
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

  Wait-ForUninstallerSelfCleanup -InstallLocation $installLocation
  Assert-NoExternalAzookeyDirectoriesAfterUninstall -InstallLocation $installLocation
  foreach ($relativePath in @("frontend.exe", "azookey.dll", "azookey32.dll", "launcher.exe")) {
    $path = Join-Path $installLocation $relativePath
    if (Test-Path $path) {
      throw "Installed file still exists after uninstall: $path"
    }
  }

  Assert-OnlySettingsRemainAfterUninstall -InstallLocation $installLocation
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
  local legacy_nsis_switch=""
  case "${UNINSTALL_AFTER_INSTALL,,}" in
    1|true|yes|on)
      uninstall_switch="-UninstallAfterInstall"
      ;;
  esac
  case "${VERIFY_LEGACY_NSIS_MIGRATION,,}" in
    1|true|yes|on)
      legacy_nsis_switch="-VerifyLegacyNsisMigration"
      ;;
  esac
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -InstallerPath \"$REMOTE_INSTALLER_WIN\" -InstallLogPath \"$REMOTE_INSTALL_LOG_WIN\" -UninstallLogPath \"$REMOTE_UNINSTALL_LOG_WIN\" -InstallerTimeoutSec $INSTALL_TIMEOUT_SEC $legacy_nsis_switch $uninstall_switch" || install_rc=$?
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
