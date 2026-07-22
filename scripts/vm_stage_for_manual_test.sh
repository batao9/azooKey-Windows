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
VERIFY_MISSING_LEGACY_NSIS_UNINSTALLER="${VERIFY_MISSING_LEGACY_NSIS_UNINSTALLER:-0}"
VERIFY_LOCKED_FILE_UPGRADE="${VERIFY_LOCKED_FILE_UPGRADE:-0}"
VERIFY_PREVIOUS_INNO_MIGRATION="${VERIFY_PREVIOUS_INNO_MIGRATION:-0}"
VERIFY_LOCKED_PREVIOUS_INNO_MIGRATION="${VERIFY_LOCKED_PREVIOUS_INNO_MIGRATION:-0}"
PREVIOUS_INNO_INSTALLER="${PREVIOUS_INNO_INSTALLER:-}"
VERIFY_MISSING_PREVIOUS_INNO_UNINSTALLER="${VERIFY_MISSING_PREVIOUS_INNO_UNINSTALLER:-0}"
VERIFY_TAMPERED_PREVIOUS_INNO_UNINSTALLER="${VERIFY_TAMPERED_PREVIOUS_INNO_UNINSTALLER:-0}"
VERIFY_TAMPERED_PREVIOUS_INNO_DATA="${VERIFY_TAMPERED_PREVIOUS_INNO_DATA:-0}"
VERIFY_PREVIOUS_INNO_REGISTRY_DELETE_FAILURE="${VERIFY_PREVIOUS_INNO_REGISTRY_DELETE_FAILURE:-0}"
VERIFY_SAFE_REINSTALL="${VERIFY_SAFE_REINSTALL:-0}"
VERIFY_TASK_CREATION_FAILURE="${VERIFY_TASK_CREATION_FAILURE:-0}"
VERIFY_TASK_RUN_FAILURE="${VERIFY_TASK_RUN_FAILURE:-0}"

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
REMOTE_PREVIOUS_INSTALLER_WIN="$REMOTE_TMP_WIN\\azookey-previous-inno-setup.exe"

REMOTE_INSTALLER_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-setup-under-test.exe"
REMOTE_PS_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-install-under-test.ps1"
REMOTE_INSTALL_LOG_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-install-under-test.log"
REMOTE_UNINSTALL_LOG_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-uninstall-under-test.log"
REMOTE_PREVIOUS_INSTALLER_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-previous-inno-setup.exe"

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
  if { [[ "${VERIFY_PREVIOUS_INNO_MIGRATION,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_LOCKED_PREVIOUS_INNO_MIGRATION,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_MISSING_PREVIOUS_INNO_UNINSTALLER,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_TAMPERED_PREVIOUS_INNO_UNINSTALLER,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_TAMPERED_PREVIOUS_INNO_DATA,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_PREVIOUS_INNO_REGISTRY_DELETE_FAILURE,,}" =~ ^(1|true|yes|on)$ ]]; } && [[ ! -f "$PREVIOUS_INNO_INSTALLER" ]]; then
    log "旧 Inno installer が見つかりません。PREVIOUS_INNO_INSTALLER を設定してください: ${PREVIOUS_INNO_INSTALLER:-<unset>}"
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
  [switch]$VerifyMissingLegacyNsisUninstaller,
  [switch]$VerifyLockedFileUpgrade,
  [switch]$VerifyPreviousInnoMigration,
  [switch]$VerifyLockedPreviousInnoMigration,
  [string]$PreviousInnoInstallerPath,
  [switch]$VerifyMissingPreviousInnoUninstaller,
  [switch]$VerifyTamperedPreviousInnoUninstaller,
  [switch]$VerifyTamperedPreviousInnoData,
  [switch]$VerifyPreviousInnoRegistryDeleteFailure,
  [switch]$VerifySafeReinstall,
  [switch]$VerifyTaskCreationFailure,
  [switch]$VerifyTaskRunFailure,
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

function Assert-FakeLegacyNsisInstallRejectedSafely {
  $legacyRoot = Join-Path $env:LOCALAPPDATA "Azookey"
  $legacyUninstallKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey"
  $legacyStateKey = "HKCU:\SOFTWARE\batao9\Azookey"
  $sentinelPath = Join-Path $env:TEMP "azookey-legacy-nsis-uninstalled.txt"

  if (!(Test-Path -LiteralPath $legacyRoot)) {
    throw "Untrusted legacy NSIS install directory was removed."
  }
  if (!(Test-Path -LiteralPath $legacyUninstallKey)) {
    throw "Untrusted legacy NSIS uninstall key was removed."
  }
  if (!(Test-Path -LiteralPath $legacyStateKey)) {
    throw "Untrusted legacy NSIS state key was removed."
  }
  if (Test-Path -LiteralPath $sentinelPath) {
    throw "Untrusted legacy NSIS uninstaller was executed."
  }

  Write-Host "untrusted legacy NSIS install was rejected without executing user-writable code"
}

function Install-BrokenLegacyNsisInstall {
  $legacyRoot = Join-Path $env:LOCALAPPDATA "Azookey"
  $legacyUninstallKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey"

  Remove-Item -LiteralPath $legacyRoot -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $legacyUninstallKey -Recurse -Force -ErrorAction SilentlyContinue

  New-Item -ItemType Directory -Force -Path $legacyRoot | Out-Null
  Set-Content -LiteralPath (Join-Path $legacyRoot "legacy-payload.txt") -Encoding UTF8 -Value "legacy nsis payload without uninstaller"

  New-Item -ItemType Directory -Force -Path $legacyUninstallKey | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "DisplayName" -Value "Azookey" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "Publisher" -Value "batao9" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "DisplayVersion" -Value "0.1.0-legacy-broken" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "InstallLocation" -Value "`"$legacyRoot`"" -PropertyType String -Force | Out-Null
  New-ItemProperty -Path $legacyUninstallKey -Name "UninstallString" -Value "`"$(Join-Path $legacyRoot "uninstall.exe")`"" -PropertyType String -Force | Out-Null

  Write-Host "created broken legacy NSIS install without uninstaller: $legacyRoot"
}

function Assert-BrokenLegacyNsisInstallPreserved {
  $legacyRoot = Join-Path $env:LOCALAPPDATA "Azookey"
  $legacyUninstallKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Azookey"
  $legacyPayload = Join-Path $legacyRoot "legacy-payload.txt"

  if (!(Test-Path -LiteralPath $legacyRoot)) {
    throw "Broken legacy NSIS install directory was removed after failed migration: $legacyRoot"
  }
  if (!(Test-Path -LiteralPath $legacyPayload)) {
    throw "Broken legacy NSIS payload was removed after failed migration: $legacyPayload"
  }
  if (!(Test-Path -LiteralPath $legacyUninstallKey)) {
    throw "Broken legacy NSIS uninstall key was removed after failed migration: $legacyUninstallKey"
  }

  Write-Host "broken legacy NSIS install preserved after migration failure"
}

function Install-PreviousInnoVersion {
  param(
    [Parameter(Mandatory = $true)][string]$PreviousInstallerPath,
    [Parameter(Mandatory = $true)][int]$TimeoutSec
  )

  if (!(Test-Path -LiteralPath $PreviousInstallerPath)) {
    throw "Previous Inno installer not found: $PreviousInstallerPath"
  }

  $previousLogPath = Join-Path $env:TEMP "azookey-previous-inno-install.log"
  $previousProc = Start-Process -FilePath $PreviousInstallerPath -ArgumentList @(
    "/SP-",
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/RESTARTEXITCODE=3010",
    "/LOG=$previousLogPath"
  ) -PassThru
  if (-not $previousProc.WaitForExit($TimeoutSec * 1000)) {
    Stop-ProcessTree -RootPid $previousProc.Id
    throw "Previous Inno installer timed out."
  }
  if (($previousProc.ExitCode -ne 0) -and ($previousProc.ExitCode -ne 3010)) {
    throw "Previous Inno installer failed. ExitCode=$($previousProc.ExitCode)"
  }

  $entries = @(Get-AzookeyUninstallEntries)
  if ($entries.Count -ne 1) {
    throw "Expected one previous Inno uninstall entry, found $($entries.Count)."
  }
  $legacyInstallLocation = Normalize-RegistryPath -Path $entries[0].InstallLocation
  $expectedLegacyLocation = Join-Path $env:APPDATA "Azookey"
  if (![string]::Equals($legacyInstallLocation, $expectedLegacyLocation, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Previous Inno install did not use the expected APPDATA location: $legacyInstallLocation"
  }

  $legacyTask = Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction Stop
  $legacyAction = "$($legacyTask.Actions[0].Execute) $($legacyTask.Actions[0].Arguments)".ToLowerInvariant()
  if (!$legacyAction.Contains("wscript") -or !$legacyAction.Contains(".vbs")) {
    throw "Previous Inno task does not reproduce the legacy VBS action: $legacyAction"
  }

  $learningRoot = Join-Path $legacyInstallLocation "LearningMemory"
  New-Item -ItemType Directory -Force -Path $learningRoot | Out-Null
  $settingsPath = Join-Path $legacyInstallLocation "settings.json"
  $settingsSentinel = '{"version":"0.1.2","zenzai":{"enable":false,"profile":"","backend":"cpu"},"general":{"punctuation_commit":true}}'
  [System.IO.File]::WriteAllText($settingsPath, $settingsSentinel, [System.Text.UTF8Encoding]::new($false))
  Set-Content -LiteralPath (Join-Path $legacyInstallLocation "settings.json.broken-20260720123456") -Encoding UTF8 -Value "settings backup sentinel"
  Set-Content -LiteralPath (Join-Path $learningRoot "migration-sentinel.txt") -Encoding UTF8 -Value "learning preserved"

  Write-Host "installed previous Inno version and created migration sentinels"
}

function Assert-PreviousInnoMigrated {
  param(
    [Parameter(Mandatory = $true)][string]$InstallLogPath,
    [switch]$AllowScheduledLegacyDll
  )

  $appDataRoot = Join-Path $env:APPDATA "Azookey"
  $settingsPath = Join-Path $appDataRoot "settings.json"
  $settingsBackupPath = Join-Path $appDataRoot "settings.json.broken-20260720123456"
  $learningPath = Join-Path $appDataRoot "LearningMemory\migration-sentinel.txt"
  foreach ($path in @($settingsPath, $settingsBackupPath, $learningPath)) {
    if (!(Test-Path -LiteralPath $path)) {
      throw "Migration did not preserve user data: $path"
    }
  }
  $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
  if ($settings.general.punctuation_commit -ne $true) {
    throw "Migration did not preserve the settings semantic sentinel."
  }

  $legacyPayload = @(
    Get-ChildItem -LiteralPath $appDataRoot -Recurse -Force -ErrorAction SilentlyContinue |
      Where-Object {
        $isScheduledLockedDll = $AllowScheduledLegacyDll -and
          [string]::Equals($_.FullName, (Join-Path $appDataRoot "azookey.dll"), [System.StringComparison]::OrdinalIgnoreCase)
        ((!$_.PSIsContainer -and $_.Extension -in @(".exe", ".dll", ".vbs", ".gguf") -and !$isScheduledLockedDll) -or
        ($_.PSIsContainer -and $_.Name -in @("Dictionary", "EmojiDictionary", "llama_cpu", "llama_cuda", "llama_vulkan", "backend")))
      }
  )
  if ($legacyPayload.Count -gt 0) {
    throw "Legacy executable payload remains under APPDATA: $($legacyPayload.FullName -join ', ')"
  }

  $broadSids = @("S-1-1-0", "S-1-5-11", "S-1-5-32-545")
  $writeRights = [System.Security.AccessControl.FileSystemRights]::WriteData -bor
    [System.Security.AccessControl.FileSystemRights]::AppendData -bor
    [System.Security.AccessControl.FileSystemRights]::WriteExtendedAttributes -bor
    [System.Security.AccessControl.FileSystemRights]::WriteAttributes -bor
    [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
    [System.Security.AccessControl.FileSystemRights]::Delete -bor
    [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
    [System.Security.AccessControl.FileSystemRights]::TakeOwnership
  # The running frontend can create and atomically remove settings.json.tmp-* while
  # this assertion is executing.  Check only the stable paths whose ACLs are part
  # of the migration contract so a harmless atomic-save race cannot fail the VM
  # test between Get-ChildItem and Get-Acl.
  $migratedPaths = @(
    Get-Item -LiteralPath $appDataRoot -Force
    Get-Item -LiteralPath (Join-Path $appDataRoot "LearningMemory") -Force
    Get-Item -LiteralPath $settingsPath -Force
    Get-Item -LiteralPath $settingsBackupPath -Force
    Get-Item -LiteralPath $learningPath -Force
  )
  foreach ($migratedPath in $migratedPaths) {
    $broadWriteRules = @(
      (Get-Acl -LiteralPath $migratedPath.FullName).Access | Where-Object {
        try {
          $identitySid = $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
        } catch {
          $identitySid = $_.IdentityReference.Value
        }
        $_.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and
        $broadSids -contains $identitySid -and
        (($_.FileSystemRights -band $writeRights) -ne 0)
      }
    )
    if ($broadWriteRules.Count -gt 0) {
      throw "Migrated user data grants write access to a broad local identity: path=$($migratedPath.FullName) rules=$($broadWriteRules | Out-String)"
    }
  }
  $aclWriteSentinel = Join-Path $appDataRoot ".migration-acl-write-sentinel"
  Set-Content -LiteralPath $aclWriteSentinel -Encoding ASCII -Value "current user can write"
  Remove-Item -LiteralPath $aclWriteSentinel -Force

  $installLog = Get-Content -LiteralPath $InstallLogPath -Raw
  $taskDeleteIndex = $installLog.IndexOf("Startup task was deleted and its absence was verified.")
  $migrationIndex = $installLog.IndexOf("Migrating previous Inno install without executing its user-writable uninstall data")
  if (($taskDeleteIndex -lt 0) -or ($migrationIndex -lt 0) -or ($taskDeleteIndex -gt $migrationIndex)) {
    throw "Installer log does not prove that the legacy task was deleted before migration."
  }

  Write-Host "previous Inno payload migrated with private writable ACL while settings/backup/learning were preserved"
}

function Assert-RequiredInstallFiles {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $requiredPaths = @(
    "frontend.exe",
    "azookey-updater-helper.exe",
    "azookey-server.exe",
    "ui.exe",
    "launcher.exe",
    "azookey.dll",
    "azookey32.dll",
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

  foreach ($forbiddenPath in @("launch.vbs", ".azookey-startup-task.xml")) {
    if (Test-Path -LiteralPath (Join-Path $InstallLocation $forbiddenPath)) {
      throw "Forbidden startup helper remains in install location: $forbiddenPath"
    }
  }
  $userSidHelpers = @(Get-ChildItem -LiteralPath $InstallLocation -Filter ".legacy-user-sid-*" -Force -ErrorAction SilentlyContinue)
  if ($userSidHelpers.Count -gt 0) {
    throw "Protected migration user SID helper was not cleaned up: $($userSidHelpers.FullName -join ', ')"
  }
}

function Assert-NoUntrustedWriteAccess {
  param([Parameter(Mandatory = $true)][string]$Path)
  $unsafeSids = @(
    "S-1-1-0",
    "S-1-5-11",
    "S-1-5-32-545",
    [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  )
  # Do not OR composite rights such as FullControl into this mask: FullControl
  # also contains ReadAndExecute/Synchronize and would classify the normal
  # BUILTIN\Users read-only Program Files ACL as writable.
  $writeRights = [System.Security.AccessControl.FileSystemRights]::WriteData -bor
    [System.Security.AccessControl.FileSystemRights]::AppendData -bor
    [System.Security.AccessControl.FileSystemRights]::WriteExtendedAttributes -bor
    [System.Security.AccessControl.FileSystemRights]::WriteAttributes -bor
    [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
    [System.Security.AccessControl.FileSystemRights]::Delete -bor
    [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
    [System.Security.AccessControl.FileSystemRights]::TakeOwnership
  $unsafeRules = @(
    (Get-Acl -LiteralPath $Path).Access | Where-Object {
      try {
        $identitySid = $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
      } catch {
        $identitySid = $_.IdentityReference.Value
      }
      $_.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and
      $unsafeSids -contains $identitySid -and
      (($_.FileSystemRights -band $writeRights) -ne 0)
    }
  )
  if ($unsafeRules.Count -gt 0) {
    throw "Protected path grants write access to an untrusted identity: path=$Path rules=$($unsafeRules | Out-String)"
  }
}

function Assert-ProtectedInstallLocation {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $expected = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)) "Azookey"
  if (![string]::Equals(
      [System.IO.Path]::GetFullPath($InstallLocation).TrimEnd("\"),
      [System.IO.Path]::GetFullPath($expected).TrimEnd("\"),
      [System.StringComparison]::OrdinalIgnoreCase
  )) {
    throw "Install location is not fixed to Program Files: actual=$InstallLocation expected=$expected"
  }

  Assert-NoUntrustedWriteAccess -Path $InstallLocation

  Write-Host "install location is fixed and protected: $InstallLocation"
}

function Assert-BackgroundExecutablesUseGuiSubsystem {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  foreach ($executableName in @("launcher.exe", "azookey-server.exe", "ui.exe")) {
    $executablePath = Join-Path $InstallLocation $executableName
    $bytes = [System.IO.File]::ReadAllBytes($executablePath)
    if ($bytes.Length -lt 256) {
      throw "$executableName is too small to contain a valid PE header."
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    $subsystemOffset = $peOffset + 24 + 68
    if (($subsystemOffset + 2) -gt $bytes.Length) {
      throw "$executableName has an invalid PE optional header."
    }
    $subsystem = [BitConverter]::ToUInt16($bytes, $subsystemOffset)
    if ($subsystem -ne 2) {
      throw "release background executable is not built as Windows GUI subsystem: name=$executableName subsystem=$subsystem"
    }
  }

  Write-Host "release launcher/server/ui use Windows GUI subsystem"
}

function Assert-SecureStartupTask {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  $task = Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction Stop
  if ($task.Principal.RunLevel -ne "Highest") {
    throw "Startup task is not HighestAvailable: $($task.Principal.RunLevel)"
  }
  try {
    if ($task.Principal.GroupId -match '^S-\d-') {
      $principalSid = $task.Principal.GroupId
    } else {
      $principalSid = ([System.Security.Principal.NTAccount]$task.Principal.GroupId).
        Translate([System.Security.Principal.SecurityIdentifier]).Value
    }
  } catch {
    throw "Startup task principal could not be resolved: $($task.Principal.GroupId): $($_.Exception.Message)"
  }
  if ($principalSid -ne "S-1-5-32-544") {
    throw "Startup task principal is not Administrators: $($task.Principal.GroupId) ($principalSid)"
  }
  if ($task.Actions.Count -ne 1) {
    throw "Startup task must contain exactly one action: $($task.Actions.Count)"
  }

  $action = $task.Actions[0]
  $expectedLauncher = Join-Path $InstallLocation "launcher.exe"
  if (![string]::Equals($action.Execute.Trim('"'), $expectedLauncher, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Startup task does not execute launcher.exe directly: $($action.Execute)"
  }
  if (![string]::Equals($action.WorkingDirectory, $InstallLocation, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Startup task working directory is unexpected: $($action.WorkingDirectory)"
  }
  $serializedAction = "$($action.Execute) $($action.Arguments)".ToLowerInvariant()
  if ($serializedAction.Contains("wscript") -or $serializedAction.Contains(".vbs")) {
    throw "Startup task still uses wscript/VBS: $serializedAction"
  }

  Write-Host "startup task directly executes protected launcher.exe"
}

function Assert-ProcessRedirectionGuard {
  param([Parameter(Mandatory = $true)][string]$ProcessName)

  if ($null -eq ("AzookeyVm.ProcessMitigation" -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace AzookeyVm {
  public static class ProcessMitigation {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetProcessMitigationPolicy(
      IntPtr process,
      int mitigationPolicy,
      out uint buffer,
      UIntPtr length);
  }
}
"@
  }

  $process = Get-Process -Name $ProcessName -ErrorAction Stop | Select-Object -First 1
  $flags = [uint32]0
  $ok = [AzookeyVm.ProcessMitigation]::GetProcessMitigationPolicy(
    $process.Handle,
    16,
    [ref]$flags,
    [UIntPtr]::new([uint64]4)
  )
  if (!$ok -or (($flags -band 1) -eq 0)) {
    $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    throw "RedirectionGuard is not enforced for $ProcessName (flags=$flags win32=$errorCode)."
  }
}

function Assert-AzookeyProcessesStarted {
  $requiredProcesses = @("launcher", "azookey-server", "ui")
  $deadline = (Get-Date).AddSeconds(45)
  do {
    $missing = @($requiredProcesses | Where-Object {
      $null -eq (Get-Process -Name $_ -ErrorAction SilentlyContinue)
    })
    if ($missing.Count -eq 0) {
      foreach ($processName in $requiredProcesses) {
        Assert-ProcessRedirectionGuard -ProcessName $processName
      }
      Write-Host "launcher/server/ui processes are running with RedirectionGuard enforced"
      return
    }
    Start-Sleep -Seconds 1
  } while ((Get-Date) -lt $deadline)

  throw "Azookey processes did not start: $($missing -join ', ')"
}

function Assert-MutableDataOutsideInstallLocation {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  foreach ($relativePath in @("EngineRuntime", "logs", "ui-webview", "frontend.exe.WebView2", "ui.exe.WebView2")) {
    if (Test-Path -LiteralPath (Join-Path $InstallLocation $relativePath)) {
      throw "Mutable runtime data was created under Program Files: $relativePath"
    }
  }

  $runtimePath = Join-Path $env:APPDATA "Azookey\EngineRuntime"
  $uiWebViewPath = Join-Path $env:LOCALAPPDATA "Azookey\ui-webview"
  if (Test-Path -LiteralPath $runtimePath) {
    Write-Host "EngineRuntime is outside Program Files: $runtimePath"
  } else {
    # EngineRuntime is initialized lazily when the IME engine first handles
    # input, so launcher/server/UI startup alone does not require it to exist.
    Write-Host "EngineRuntime has not been initialized before IME input (expected lazy behavior)"
  }
  if (Test-Path -LiteralPath $uiWebViewPath) {
    Write-Host "UI WebView data is outside Program Files: $uiWebViewPath"
  } else {
    # The candidate UI creates its WebView lazily when it is first shown.
    Write-Host "UI WebView data has not been initialized before candidate UI display (expected lazy behavior)"
  }

  Write-Host "no mutable runtime or UI WebView data was created under Program Files"
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

function Assert-InstallPayloadRemovedAfterUninstall {
  param([Parameter(Mandatory = $true)][string]$InstallLocation)

  if (!(Test-Path -LiteralPath $InstallLocation)) {
    Write-Host "install directory removed after uninstall"
    return
  }

  $remaining = @(
    Get-ChildItem -LiteralPath $InstallLocation -Force -Recurse -ErrorAction SilentlyContinue
  )

  if ($remaining.Count -gt 0) {
    $sample = @($remaining | Select-Object -First 20 | ForEach-Object {
      $_.FullName.Substring($InstallLocation.Length).TrimStart("\")
    })
    throw "Unexpected files remain after uninstall: $($sample -join ', ')"
  }

  Write-Host "Program Files payload was removed after uninstall"
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

function Ensure-PreservedUserDataSentinels {
  $appDataRoot = Join-Path $env:APPDATA "Azookey"
  $learningRoot = Join-Path $appDataRoot "LearningMemory"
  New-Item -ItemType Directory -Force -Path $learningRoot | Out-Null
  $settingsPath = Join-Path $appDataRoot "settings.json"
  $settingsSentinel = '{"version":"0.1.2","zenzai":{"enable":false,"profile":"","backend":"cpu"},"general":{"punctuation_commit":true}}'
  [System.IO.File]::WriteAllText($settingsPath, $settingsSentinel, [System.Text.UTF8Encoding]::new($false))
  Set-Content -LiteralPath (Join-Path $appDataRoot "settings.json.broken-vm-sentinel") -Encoding UTF8 -Value "settings backup sentinel"
  Set-Content -LiteralPath (Join-Path $learningRoot "learning-sentinel.txt") -Encoding UTF8 -Value "learning sentinel"
  Write-Host "created settings, backup, and learning sentinels"
}

function Ensure-GeneratedAppDataForUninstallVerification {
  foreach ($dir in @(
    (Join-Path $env:APPDATA "Azookey\EngineRuntime"),
    (Join-Path $env:APPDATA "Azookey\logs"),
    (Join-Path $env:APPDATA "Azookey\frontend.exe.WebView2"),
    (Join-Path $env:APPDATA "Azookey\ui.exe.WebView2"),
    (Join-Path $env:LOCALAPPDATA "Azookey\ui-webview"),
    (Join-Path $env:LOCALAPPDATA "com.azookey.app")
  )) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-Content -LiteralPath (Join-Path $dir "uninstall-sentinel.txt") -Encoding UTF8 -Value "generated app data cleanup sentinel"
  }

  $appDataRoot = Join-Path $env:APPDATA "Azookey"
  Set-Content -LiteralPath (Join-Path $appDataRoot "legacy-payload.exe") -Encoding UTF8 -Value "legacy executable sentinel"
  $legacyDictionary = Join-Path $appDataRoot "Dictionary"
  New-Item -ItemType Directory -Force -Path $legacyDictionary | Out-Null
  Set-Content -LiteralPath (Join-Path $legacyDictionary "legacy-dictionary.txt") -Encoding UTF8 -Value "legacy dictionary sentinel"

  Write-Host "created generated app data sentinels for uninstall verification"
}

function Assert-UninstallDataPolicy {
  $appDataRoot = Join-Path $env:APPDATA "Azookey"
  foreach ($path in @(
    (Join-Path $appDataRoot "settings.json"),
    (Join-Path $appDataRoot "settings.json.broken-vm-sentinel"),
    (Join-Path $appDataRoot "LearningMemory\learning-sentinel.txt")
  )) {
    if (!(Test-Path -LiteralPath $path)) {
      throw "Preserved user data was removed during uninstall: $path"
    }
  }

  foreach ($path in @(
    (Join-Path $appDataRoot "EngineRuntime"),
    (Join-Path $appDataRoot "logs"),
    (Join-Path $appDataRoot "frontend.exe.WebView2"),
    (Join-Path $appDataRoot "ui.exe.WebView2"),
    (Join-Path $appDataRoot "legacy-payload.exe"),
    (Join-Path $appDataRoot "Dictionary"),
    (Join-Path $env:LOCALAPPDATA "Azookey\ui-webview"),
    (Join-Path $env:LOCALAPPDATA "com.azookey.app")
  )) {
    if (Test-Path -LiteralPath $path) {
      throw "Generated cache/log remains after uninstall: $path"
    }
  }

  if (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue) {
    throw "Startup task remains after uninstall."
  }

  Write-Host "uninstall preserved settings/learning and removed generated cache/log/task"
}

function Start-ExclusiveFileLock {
  param([Parameter(Mandatory = $true)][string]$Path)

  $readyPath = Join-Path $env:TEMP "azookey-file-lock-ready.txt"
  $releasePath = Join-Path $env:TEMP "azookey-file-lock-release.txt"
  $lockScriptPath = Join-Path $env:TEMP "azookey-file-lock.ps1"

  Remove-Item -LiteralPath $readyPath, $releasePath, $lockScriptPath -Force -ErrorAction SilentlyContinue

  @'
param(
  [Parameter(Mandatory = $true)][string]$Path,
  [Parameter(Mandatory = $true)][string]$ReadyPath,
  [Parameter(Mandatory = $true)][string]$ReleasePath
)

$ErrorActionPreference = "Stop"
$stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
try {
  Set-Content -LiteralPath $ReadyPath -Encoding UTF8 -Value "ready"
  while (!(Test-Path -LiteralPath $ReleasePath)) {
    Start-Sleep -Milliseconds 200
  }
} finally {
  $stream.Dispose()
}
'@ | Set-Content -LiteralPath $lockScriptPath -Encoding UTF8

  $proc = Start-Process -FilePath "powershell" -ArgumentList @(
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    ('"{0}"' -f $lockScriptPath),
    "-Path",
    ('"{0}"' -f $Path),
    "-ReadyPath",
    ('"{0}"' -f $readyPath),
    "-ReleasePath",
    ('"{0}"' -f $releasePath)
  ) -PassThru

  $deadline = (Get-Date).AddSeconds(30)
  while (!(Test-Path -LiteralPath $readyPath)) {
    if ($proc.HasExited) {
      throw "File lock helper exited before acquiring the lock. ExitCode=$($proc.ExitCode)"
    }
    if ((Get-Date) -ge $deadline) {
      Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
      throw "Timed out waiting for file lock helper: $Path"
    }
    Start-Sleep -Milliseconds 200
  }

  [PSCustomObject]@{
    Process = $proc
    ReleasePath = $releasePath
  }
}

function Stop-ExclusiveFileLock {
  param([Parameter(Mandatory = $true)]$Lock)

  Set-Content -LiteralPath $Lock.ReleasePath -Encoding UTF8 -Value "release"
  if (-not $Lock.Process.WaitForExit(10000)) {
    Stop-Process -Id $Lock.Process.Id -Force -ErrorAction SilentlyContinue
    throw "File lock helper did not exit after release."
  }
}

function Invoke-LockedFileUpgradeVerification {
  param(
    [Parameter(Mandatory = $true)][string]$InstallLocation,
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$InstallLogPath,
    [Parameter(Mandatory = $true)][int]$InstallerTimeoutSec
  )

  $lockedDllPath = Join-Path $InstallLocation "azookey.dll"
  $upgradeLogPath = [System.IO.Path]::ChangeExtension($InstallLogPath, ".upgrade.log")
  $lock = Start-ExclusiveFileLock -Path $lockedDllPath

  try {
    $upgradeArgs = @(
      "/SP-",
      "/VERYSILENT",
      "/SUPPRESSMSGBOXES",
      "/NOCLOSEAPPLICATIONS",
      "/NORESTART",
      "/RESTARTEXITCODE=3010",
      "/LOG=$upgradeLogPath"
    )

    Write-Host "reinstalling with locked file: $lockedDllPath"
    $upgradeProc = Start-Process -FilePath $InstallerPath -ArgumentList $upgradeArgs -PassThru
    if (-not $upgradeProc.WaitForExit($InstallerTimeoutSec * 1000)) {
      Stop-ProcessTree -RootPid $upgradeProc.Id
      throw "Locked-file upgrade installer timed out."
    }
    Write-Host "locked-file upgrade installer exit code: $($upgradeProc.ExitCode)"

    if ($upgradeProc.ExitCode -ne 3010) {
      throw "Expected locked-file upgrade to request restart with exit code 3010, got $($upgradeProc.ExitCode)."
    }
  } finally {
    Stop-ExclusiveFileLock -Lock $lock
  }

  Write-Host "locked-file upgrade requested restart without blocking"
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
if ($VerifyMissingLegacyNsisUninstaller) {
  Install-BrokenLegacyNsisInstall
}
$previousInnoLock = $null
if ($VerifyPreviousInnoMigration) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
}
if ($VerifyPreviousInnoRegistryDeleteFailure) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
}
if ($VerifyLockedPreviousInnoMigration) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
  $previousInnoLock = Start-ExclusiveFileLock -Path (Join-Path $env:APPDATA "Azookey\azookey.dll")
  Write-Host "locked previous AppData TSF DLL before migration"
}
if ($VerifyMissingPreviousInnoUninstaller) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
  $previousEntry = @(Get-AzookeyUninstallEntries)[0]
  $previousUninstaller = Normalize-RegistryPath -Path $previousEntry.UninstallString
  $previousUninstallerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $previousUninstaller).Hash.ToLowerInvariant()
  Write-Host "previous Inno uninstaller SHA-256: $previousUninstallerHash"
  $previousUninstallerData = [System.IO.Path]::ChangeExtension($previousUninstaller, ".dat")
  $previousUninstallerDataHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $previousUninstallerData).Hash.ToLowerInvariant()
  Write-Host "previous Inno uninstaller data SHA-256: $previousUninstallerDataHash"
  Move-Item -LiteralPath $previousUninstaller -Destination "$previousUninstaller.missing" -Force
  Write-Host "removed previous Inno uninstaller to inject migration failure"
}
if ($VerifyTamperedPreviousInnoData) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
  $previousEntry = @(Get-AzookeyUninstallEntries)[0]
  $previousUninstaller = Normalize-RegistryPath -Path $previousEntry.UninstallString
  $previousUninstallerData = [System.IO.Path]::ChangeExtension($previousUninstaller, ".dat")
  $stream = [System.IO.File]::Open($previousUninstallerData, [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
  try {
    $stream.WriteByte(0)
  } finally {
    $stream.Dispose()
  }
  Write-Host "tampered machine-specific previous Inno data; controlled migration must not execute it"
}
if ($VerifyTamperedPreviousInnoUninstaller) {
  Install-PreviousInnoVersion -PreviousInstallerPath $PreviousInnoInstallerPath -TimeoutSec $InstallerTimeoutSec
  $previousEntry = @(Get-AzookeyUninstallEntries)[0]
  $previousUninstaller = Normalize-RegistryPath -Path $previousEntry.UninstallString
  $stream = [System.IO.File]::Open($previousUninstaller, [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
  try {
    $stream.WriteByte(0)
  } finally {
    $stream.Dispose()
  }
  Write-Host "tampered previous Inno uninstaller executable to inject trust-validation failure"
}

$args = @(
  "/SP-",
  "/VERYSILENT",
  "/SUPPRESSMSGBOXES",
  "/NORESTART",
  "/RESTARTEXITCODE=3010",
  "/DIR=$(Join-Path $env:LOCALAPPDATA 'Azookey-unsafe-install-override')",
  "/LOG=$InstallLogPath"
)
if ($VerifyTaskCreationFailure) {
  $args += "/AZOOKEY_TEST_FAIL_TASK_CREATE"
}
if ($VerifyTaskRunFailure) {
  $args += "/AZOOKEY_TEST_FAIL_TASK_RUN"
}
if ($VerifyPreviousInnoRegistryDeleteFailure) {
  $args += "/AZOOKEY_TEST_FAIL_LEGACY_REGISTRY_DELETE"
}

Write-Host "installing: $InstallerPath"
Write-Host "timeout(sec): $InstallerTimeoutSec"
$proc = Start-Process -FilePath $InstallerPath -ArgumentList $args -PassThru
if (-not $proc.WaitForExit($InstallerTimeoutSec * 1000)) {
  if ($null -ne $previousInnoLock) {
    Stop-ExclusiveFileLock -Lock $previousInnoLock
  }
  Stop-ProcessTree -RootPid $proc.Id
  throw "Installer timed out."
}
Write-Host "installer exit code: $($proc.ExitCode)"
if ($null -ne $previousInnoLock) {
  Stop-ExclusiveFileLock -Lock $previousInnoLock
}

$setupLogText = Get-Content -LiteralPath $InstallLogPath -Raw
if ($setupLogText.IndexOf("RedirectionGuard status for current process: Enabled in enforcing mode") -lt 0) {
  throw "Installer did not run with RedirectionGuard enabled in enforcing mode."
}
Write-Host "installer RedirectionGuard is enabled in enforcing mode"

if ($VerifyMissingPreviousInnoUninstaller -or $VerifyTamperedPreviousInnoUninstaller) {
  if ($proc.ExitCode -eq 0) {
    throw "Installer succeeded even though the previous Inno uninstaller is missing or untrusted."
  }
  $previousEntries = @(Get-AzookeyUninstallEntries)
  if ($previousEntries.Count -ne 1) {
    throw "Previous Inno uninstall metadata was removed after failed migration."
  }
  $previousLocation = Normalize-RegistryPath -Path $previousEntries[0].InstallLocation
  foreach ($path in @(
    (Join-Path $previousLocation "launcher.exe"),
    (Join-Path $previousLocation "settings.json"),
    (Join-Path $previousLocation "LearningMemory\migration-sentinel.txt")
  )) {
    if (!(Test-Path -LiteralPath $path)) {
      throw "Previous Inno payload/data was manually removed after failed migration: $path"
    }
  }
  if (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue) {
    throw "Unsafe previous startup task remains after failed migration."
  }
  Write-Host "missing or untrusted previous Inno uninstaller aborted migration without payload deletion"
  exit 0
}

if ($VerifyPreviousInnoRegistryDeleteFailure) {
  if (($proc.ExitCode -eq 0) -or ($proc.ExitCode -eq 3010)) {
    throw "Installer reported success even though previous uninstall metadata deletion failure was injected."
  }
  $previousEntries = @(Get-AzookeyUninstallEntries)
  if ($previousEntries.Count -ne 1) {
    throw "Previous Inno uninstall metadata was removed after the injected preflight failure."
  }
  $previousLocation = Normalize-RegistryPath -Path $previousEntries[0].InstallLocation
  foreach ($path in @(
    (Join-Path $previousLocation "launcher.exe"),
    (Join-Path $previousLocation "settings.json"),
    (Join-Path $previousLocation "LearningMemory\migration-sentinel.txt")
  )) {
    if (!(Test-Path -LiteralPath $path)) {
      throw "Previous Inno payload/data was removed after the injected registry preflight failure: $path"
    }
  }
  if (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue) {
    throw "Unsafe previous startup task remains after the injected registry preflight failure."
  }
  $failureLog = Get-Content -LiteralPath $InstallLogPath -Raw
  if ($failureLog.IndexOf("Injecting previous Inno uninstall registry deletion failure for installer verification.") -lt 0) {
    throw "Installer log does not prove that uninstall metadata deletion failed before migration."
  }
  if ($failureLog.IndexOf("Migrating previous Inno install without executing its user-writable uninstall data") -ge 0) {
    throw "Installer began destructive payload migration after uninstall metadata deletion failed."
  }
  Write-Host "previous Inno registry deletion failure aborted before payload migration and preserved retry state"
  exit 0
}

if ($VerifyTaskCreationFailure) {
  if (($proc.ExitCode -eq 0) -or ($proc.ExitCode -eq 3010)) {
    throw "Installer reported success/restart even though task creation failure was injected: $($proc.ExitCode)"
  }
  if (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue) {
    throw "Partial startup task remains after injected creation failure."
  }
  $failureLog = Get-Content -LiteralPath $InstallLogPath -Raw
  if ($failureLog.IndexOf("Injecting startup task creation failure for installer verification.") -lt 0) {
    throw "Installer log does not prove that task creation reached the injected failure path."
  }
  Write-Host "task creation failure aborted install without leaving a partial task"
  exit 0
}

if ($VerifyTaskRunFailure) {
  if (($proc.ExitCode -eq 0) -or ($proc.ExitCode -eq 3010)) {
    throw "Installer reported success/restart even though task run failure was injected: $($proc.ExitCode)"
  }
  if (Get-ScheduledTask -TaskName "Azookey Startup" -ErrorAction SilentlyContinue) {
    throw "Created startup task remains after injected run failure."
  }
  $failureLog = Get-Content -LiteralPath $InstallLogPath -Raw
  $injectionIndex = $failureLog.IndexOf("Injecting startup task run failure after successful creation for installer verification.")
  $runFailureIndex = $failureLog.IndexOf("Startup task run failed.", $injectionIndex + 1)
  $cleanupIndex = $failureLog.IndexOf("Startup task was deleted and its absence was verified.", $runFailureIndex + 1)
  if (($injectionIndex -lt 0) -or ($runFailureIndex -le $injectionIndex) -or ($cleanupIndex -le $runFailureIndex)) {
    throw "Installer log does not prove create-success, run-failure, and verified cleanup ordering."
  }
  Write-Host "task run failure removed the already-created partial task"
  exit 0
}

if ($VerifyMissingLegacyNsisUninstaller) {
  if ($proc.ExitCode -eq 0) {
    throw "Installer succeeded even though the legacy NSIS uninstaller is missing."
  }
  Assert-BrokenLegacyNsisInstallPreserved
  Write-Host "missing legacy NSIS uninstaller aborted install as expected"
  exit 0
}

if ($VerifyLegacyNsisMigration) {
  if ($proc.ExitCode -eq 0) {
    throw "Installer executed or ignored an untrusted legacy NSIS install."
  }
  Assert-FakeLegacyNsisInstallRejectedSafely
  exit 0
}

if (($proc.ExitCode -ne 0) -and ($proc.ExitCode -ne 3010)) {
  throw "Installer failed. ExitCode=$($proc.ExitCode)"
}

if (($VerifyPreviousInnoMigration -or $VerifyLockedPreviousInnoMigration -or $VerifyTamperedPreviousInnoData) -and ($proc.ExitCode -ne 3010)) {
  throw "Previous Inno migration did not propagate restart as exit code 3010: $($proc.ExitCode)"
}

if ($VerifyPreviousInnoMigration -or $VerifyLockedPreviousInnoMigration -or $VerifyTamperedPreviousInnoData) {
  Assert-PreviousInnoMigrated -InstallLogPath $InstallLogPath -AllowScheduledLegacyDll:$VerifyLockedPreviousInnoMigration
}

if ($VerifyLockedPreviousInnoMigration) {
  $migrationLog = Get-Content -LiteralPath $InstallLogPath -Raw
  if ($migrationLog.IndexOf("Locked previous payload was queued for deletion in place with RedirectionGuard enforced.") -lt 0) {
    throw "Locked previous DLL was not queued by the RedirectionGuard-protected fallback."
  }
  $lockedLegacyDll = Join-Path $env:APPDATA "Azookey\azookey.dll"
  if (!(Test-Path -LiteralPath $lockedLegacyDll)) {
    throw "Locked legacy DLL unexpectedly disappeared before its locking handle was released."
  }
  $pendingRename = @(
    (Get-ItemPropertyValue `
      -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager" `
      -Name "PendingFileRenameOperations" `
      -ErrorAction SilentlyContinue)
  )
  if (-not ($pendingRename | Where-Object { $_ -like "*$lockedLegacyDll*" })) {
    throw "Locked legacy DLL is not present in PendingFileRenameOperations: $lockedLegacyDll"
  }
  Write-Host "locked previous AppData DLL is the only remaining payload and is queued for restart deletion"
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
Assert-ProtectedInstallLocation -InstallLocation $installLocation
Assert-BackgroundExecutablesUseGuiSubsystem -InstallLocation $installLocation
Assert-SecureStartupTask -InstallLocation $installLocation
Assert-AzookeyProcessesStarted
Assert-MutableDataOutsideInstallLocation -InstallLocation $installLocation
if (-not (Test-WebView2RuntimeInstalled)) {
  throw "WebView2 Runtime is missing after install."
}

Write-Host "install complete. entry found: $($entry.PSChildName)"
Write-Host "install location: $installLocation"
Write-Host "WebView2 Runtime installed"

if ($VerifySafeReinstall) {
  Ensure-PreservedUserDataSentinels
  $learningHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $env:APPDATA "Azookey\LearningMemory\learning-sentinel.txt")).Hash
  $reinstallLogPath = [System.IO.Path]::ChangeExtension($InstallLogPath, ".safe-reinstall.log")
  $reinstallProc = Start-Process -FilePath $InstallerPath -ArgumentList @(
    "/SP-",
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/RESTARTEXITCODE=3010",
    "/LOG=$reinstallLogPath"
  ) -PassThru
  if (-not $reinstallProc.WaitForExit($InstallerTimeoutSec * 1000)) {
    Stop-ProcessTree -RootPid $reinstallProc.Id
    throw "Safe-layout reinstall timed out."
  }
  if (($reinstallProc.ExitCode -ne 0) -and ($reinstallProc.ExitCode -ne 3010)) {
    throw "Safe-layout reinstall failed. ExitCode=$($reinstallProc.ExitCode)"
  }
  $settingsAfterReinstall = Get-Content -LiteralPath (Join-Path $env:APPDATA "Azookey\settings.json") -Raw | ConvertFrom-Json
  if ($settingsAfterReinstall.general.punctuation_commit -ne $true) {
    throw "Safe-layout reinstall did not preserve the settings semantic sentinel."
  }
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $env:APPDATA "Azookey\LearningMemory\learning-sentinel.txt")).Hash -ne $learningHash) {
    throw "Safe-layout reinstall changed learning data."
  }
  Assert-SecureStartupTask -InstallLocation $installLocation
  Assert-AzookeyProcessesStarted
  Assert-MutableDataOutsideInstallLocation -InstallLocation $installLocation
  Write-Host "safe-layout to safe-layout update preserved data and recreated the protected task"
}

if ($VerifyLockedFileUpgrade) {
  if ($UninstallAfterInstall) {
    throw "VerifyLockedFileUpgrade cannot be combined with UninstallAfterInstall because the upgrade intentionally leaves pending reboot operations."
  }
  Invoke-LockedFileUpgradeVerification -InstallLocation $installLocation -InstallerPath $InstallerPath -InstallLogPath $InstallLogPath -InstallerTimeoutSec $InstallerTimeoutSec
}

if ($UninstallAfterInstall) {
  Ensure-PreservedUserDataSentinels
  Ensure-GeneratedAppDataForUninstallVerification
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
  Assert-UninstallDataPolicy
  foreach ($relativePath in @("frontend.exe", "azookey.dll", "azookey32.dll", "launcher.exe")) {
    $path = Join-Path $installLocation $relativePath
    if (Test-Path $path) {
      throw "Installed file still exists after uninstall: $path"
    }
  }

  Assert-InstallPayloadRemovedAfterUninstall -InstallLocation $installLocation
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
  if [[ "${VERIFY_PREVIOUS_INNO_MIGRATION,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_LOCKED_PREVIOUS_INNO_MIGRATION,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_MISSING_PREVIOUS_INNO_UNINSTALLER,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_TAMPERED_PREVIOUS_INNO_UNINSTALLER,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_TAMPERED_PREVIOUS_INNO_DATA,,}" =~ ^(1|true|yes|on)$ ]] || [[ "${VERIFY_PREVIOUS_INNO_REGISTRY_DELETE_FAILURE,,}" =~ ^(1|true|yes|on)$ ]]; then
    log "旧 Inno installer を VM に転送します: $(basename "$PREVIOUS_INNO_INSTALLER")"
    scp_to_vm "$PREVIOUS_INNO_INSTALLER" "$REMOTE_PREVIOUS_INSTALLER_SCP"
  fi
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"

  log "VM でインストーラーを実行します（サイレント）"
  local install_rc=0
  local uninstall_switch=""
  local legacy_nsis_switch=""
  local missing_legacy_nsis_uninstaller_switch=""
  local locked_file_upgrade_switch=""
  local previous_inno_migration_switch=""
  local locked_previous_inno_migration_switch=""
  local missing_previous_inno_uninstaller_switch=""
  local tampered_previous_inno_uninstaller_switch=""
  local tampered_previous_inno_data_switch=""
  local previous_inno_registry_delete_failure_switch=""
  local safe_reinstall_switch=""
  local task_creation_failure_switch=""
  local task_run_failure_switch=""
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
  case "${VERIFY_MISSING_LEGACY_NSIS_UNINSTALLER,,}" in
    1|true|yes|on)
      missing_legacy_nsis_uninstaller_switch="-VerifyMissingLegacyNsisUninstaller"
      ;;
  esac
  case "${VERIFY_LOCKED_FILE_UPGRADE,,}" in
    1|true|yes|on)
      locked_file_upgrade_switch="-VerifyLockedFileUpgrade"
      ;;
  esac
  case "${VERIFY_PREVIOUS_INNO_MIGRATION,,}" in
    1|true|yes|on)
      previous_inno_migration_switch="-VerifyPreviousInnoMigration -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_LOCKED_PREVIOUS_INNO_MIGRATION,,}" in
    1|true|yes|on)
      locked_previous_inno_migration_switch="-VerifyLockedPreviousInnoMigration -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_MISSING_PREVIOUS_INNO_UNINSTALLER,,}" in
    1|true|yes|on)
      missing_previous_inno_uninstaller_switch="-VerifyMissingPreviousInnoUninstaller -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_TAMPERED_PREVIOUS_INNO_UNINSTALLER,,}" in
    1|true|yes|on)
      tampered_previous_inno_uninstaller_switch="-VerifyTamperedPreviousInnoUninstaller -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_TAMPERED_PREVIOUS_INNO_DATA,,}" in
    1|true|yes|on)
      tampered_previous_inno_data_switch="-VerifyTamperedPreviousInnoData -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_PREVIOUS_INNO_REGISTRY_DELETE_FAILURE,,}" in
    1|true|yes|on)
      previous_inno_registry_delete_failure_switch="-VerifyPreviousInnoRegistryDeleteFailure -PreviousInnoInstallerPath \"$REMOTE_PREVIOUS_INSTALLER_WIN\""
      ;;
  esac
  case "${VERIFY_SAFE_REINSTALL,,}" in
    1|true|yes|on)
      safe_reinstall_switch="-VerifySafeReinstall"
      ;;
  esac
  case "${VERIFY_TASK_CREATION_FAILURE,,}" in
    1|true|yes|on)
      task_creation_failure_switch="-VerifyTaskCreationFailure"
      ;;
  esac
  case "${VERIFY_TASK_RUN_FAILURE,,}" in
    1|true|yes|on)
      task_run_failure_switch="-VerifyTaskRunFailure"
      ;;
  esac
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -InstallerPath \"$REMOTE_INSTALLER_WIN\" -InstallLogPath \"$REMOTE_INSTALL_LOG_WIN\" -UninstallLogPath \"$REMOTE_UNINSTALL_LOG_WIN\" -InstallerTimeoutSec $INSTALL_TIMEOUT_SEC $legacy_nsis_switch $missing_legacy_nsis_uninstaller_switch $locked_file_upgrade_switch $previous_inno_migration_switch $locked_previous_inno_migration_switch $missing_previous_inno_uninstaller_switch $tampered_previous_inno_uninstaller_switch $tampered_previous_inno_data_switch $previous_inno_registry_delete_failure_switch $safe_reinstall_switch $task_creation_failure_switch $task_run_failure_switch $uninstall_switch" || install_rc=$?
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
