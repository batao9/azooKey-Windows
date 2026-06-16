#!/usr/bin/env bash
set -euo pipefail

VM_NAME="${VM_NAME:-}"
SNAPSHOT_NAME="${SNAPSHOT_NAME:-}"
RESTORE_BEFORE_TEST="${RESTORE_BEFORE_TEST:-1}"
RESTORE_AFTER_TEST="${RESTORE_AFTER_TEST:-1}"
ALLOW_DIRTY_WORKTREE="${ALLOW_DIRTY_WORKTREE:-1}"
DISCARD_SAVED_STATE_BEFORE_TEST="${DISCARD_SAVED_STATE_BEFORE_TEST:-0}"
PRUNE_ORPHAN_MEDIA_AFTER_RESTORE="${PRUNE_ORPHAN_MEDIA_AFTER_RESTORE:-1}"
SSH_USER="${SSH_USER:-}"
SSH_PORT="${SSH_PORT:-}"
SSH_KEY="${SSH_KEY:-}"
SSH_READY_TIMEOUT_SEC="${SSH_READY_TIMEOUT_SEC:-10}"
SSH_COMMAND_TIMEOUT_SEC="${SSH_COMMAND_TIMEOUT_SEC:-60}"
SSH_TEST_TIMEOUT_SEC="${SSH_TEST_TIMEOUT_SEC:-7200}"
SCP_COMMAND_TIMEOUT_SEC="${SCP_COMMAND_TIMEOUT_SEC:-300}"
VBOX_MANAGE="${VBOX_MANAGE:-}"
STAGING_VM_NAME="${STAGING_VM_NAME:-}"

if [[ -z "$VBOX_MANAGE" ]]; then
  if command -v VBoxManage >/dev/null 2>&1; then
    VBOX_MANAGE="$(command -v VBoxManage)"
  elif [[ -x "/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe" ]]; then
    VBOX_MANAGE="/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe"
  fi
fi

if [[ $# -lt 1 || -z "${1:-}" ]]; then
  echo "Usage: VM_NAME=... SNAPSHOT_NAME=... SSH_USER=... SSH_PORT=... SSH_KEY=... $0 <branch> [cargo-test-filter|skip] [swift-test-filter|all|skip]"
  echo "Example: VM_NAME=<vm-name> SNAPSHOT_NAME=<snapshot-name> SSH_USER=<user> SSH_PORT=<port> SSH_KEY=<key-path> $0 feature/clause-adjustment-stateful-tests composition all"
  exit 1
fi

TARGET_BRANCH="$1"
CARGO_TEST_FILTER="${2:-composition}"
SWIFT_TEST_FILTER="${3:-}"

if [[ "$CARGO_TEST_FILTER" == "-" ]]; then
  CARGO_TEST_FILTER="skip"
fi
if [[ "$SWIFT_TEST_FILTER" == "-" ]]; then
  SWIFT_TEST_FILTER="skip"
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_DIR="$REPO_ROOT/.local/logs"
SWIFT_VENDOR_CACHE_DIR="${SWIFT_VENDOR_CACHE_DIR:-$REPO_ROOT/.local/cache/AzooKeyKanaKanjiConverter}"
SWIFT_VENDOR_REPO_URL="${SWIFT_VENDOR_REPO_URL:-https://github.com/batao9/AzooKeyKanaKanjiConverter}"
SWIFT_VENDOR_REVISION="${SWIFT_VENDOR_REVISION:-56268957b81b004ca8231ffc3491a4af684d0e20}"
mkdir -p "$LOG_DIR"

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
HOST_TIMESTAMP_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
LOG_FILE="$LOG_DIR/vm-client-test-$TIMESTAMP.log"

REMOTE_TMP_WIN="C:\\Users\\$SSH_USER\\AppData\\Local\\Temp"
REMOTE_TAR_WIN="$REMOTE_TMP_WIN\\azookey-src.tar.gz"
REMOTE_PS_WIN="$REMOTE_TMP_WIN\\azookey-vm-client-test.ps1"
REMOTE_SWIFT_VENDOR_TAR_WIN="$REMOTE_TMP_WIN\\azookey-swift-vendor.tar.gz"
REMOTE_SRC_WIN="C:\\w\\azt-$TIMESTAMP"

REMOTE_TAR_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-src.tar.gz"
REMOTE_PS_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-vm-client-test.ps1"
REMOTE_SWIFT_VENDOR_TAR_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-swift-vendor.tar.gz"

SSH_CONTROL_PATH="/tmp/vm-client-test-$TIMESTAMP-%C"
SSH_OPTS=(
  -i "$SSH_KEY"
  -p "$SSH_PORT"
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=8
  -o ServerAliveInterval=5
  -o ServerAliveCountMax=4
  -o ControlMaster=auto
  -o ControlPersist=600
  -o "ControlPath=$SSH_CONTROL_PATH"
)
SCP_OPTS=(
  -i "$SSH_KEY"
  -P "$SSH_PORT"
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=8
  -o ServerAliveInterval=5
  -o ServerAliveCountMax=4
  -o ControlMaster=auto
  -o ControlPersist=600
  -o "ControlPath=$SSH_CONTROL_PATH"
)

TMP_SRC_ARCHIVE=""
TMP_REMOTE_PS=""
TMP_SWIFT_VENDOR_ARCHIVE=""
VM_TOUCHED=0
FINAL_RESTORE_DONE=0

exec > >(tee "$LOG_FILE") 2>&1

log() {
  printf '[vm-client-test] %s\n' "$*"
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

matches_regex() {
  if command -v rg >/dev/null 2>&1; then
    rg -q "$1"
  else
    grep -q -- "$1"
  fi
}

is_vm_running() {
  vbox list runningvms | matches_fixed "\"$VM_NAME\""
}

is_named_vm_running() {
  local vm_name="$1"
  vbox list runningvms | matches_fixed "\"$vm_name\""
}

snapshot_exists() {
  vbox snapshot "$VM_NAME" list --machinereadable | matches_fixed "=\"$SNAPSHOT_NAME\""
}

prune_orphan_media_after_restore() {
  if [[ "$PRUNE_ORPHAN_MEDIA_AFTER_RESTORE" != "1" ]]; then
    return 0
  fi

  if ! "$REPO_ROOT/scripts/vm_prune_orphan_media.sh" "$VM_NAME"; then
    log "orphan media prune に失敗しました。処理を続行します"
  fi
}

ssh_run() {
  timeout "${SSH_COMMAND_TIMEOUT_SEC}s" \
    ssh "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" "$@"
}

ssh_run_test() {
  timeout "${SSH_TEST_TIMEOUT_SEC}s" \
    ssh "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" "$@"
}

scp_to_vm() {
  timeout "${SCP_COMMAND_TIMEOUT_SEC}s" \
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
      if timeout "${SSH_READY_TIMEOUT_SEC}s" \
        ssh "${SSH_OPTS[@]}" "$SSH_USER@$host" "echo ready" >/dev/null 2>&1; then
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

wait_for_named_vm_poweroff() {
  local vm_name="$1"
  local tries=60
  for ((i=1; i<=tries; i++)); do
    if ! is_named_vm_running "$vm_name"; then
      return 0
    fi
    sleep 2
  done
  return 1
}

start_vm() {
  local tries=10
  local output=""
  for ((i=1; i<=tries; i++)); do
    if output="$(vbox startvm "$VM_NAME" --type headless 2>&1)"; then
      return 0
    fi
    if printf '%s' "$output" | matches_fixed "already locked by a session"; then
      log "VM 起動が一時ロック中です。再試行します ($i/$tries)"
      sleep 3
      continue
    fi
    printf '%s\n' "$output" >&2
    return 1
  done
  printf '%s\n' "$output" >&2
  return 1
}

stop_staging_vm_if_running() {
  if [[ -z "$STAGING_VM_NAME" || "$STAGING_VM_NAME" == "$VM_NAME" ]]; then
    return 0
  fi

  if ! is_named_vm_running "$STAGING_VM_NAME"; then
    return 0
  fi

  log "test 前に staging VM を停止します: $STAGING_VM_NAME"
  vbox controlvm "$STAGING_VM_NAME" acpipowerbutton >/dev/null || true
  if ! wait_for_named_vm_poweroff "$STAGING_VM_NAME"; then
    log "staging VM が停止しないため強制停止します: $STAGING_VM_NAME"
    vbox controlvm "$STAGING_VM_NAME" poweroff >/dev/null || true
  fi
}

cleanup() {
  local rc=$?
  set +e

  ssh -O exit "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" >/dev/null 2>&1 || true
  rm -f "${TMP_SRC_ARCHIVE:-}" "${TMP_REMOTE_PS:-}" "${TMP_SWIFT_VENDOR_ARCHIVE:-}"

  if [[ "$rc" -ne 0 && "$RESTORE_AFTER_TEST" == "1" && "$VM_TOUCHED" == "1" && "$FINAL_RESTORE_DONE" != "1" ]]; then
    log "エラー終了のためクリーン状態へ復元します: $SNAPSHOT_NAME"
    if is_vm_running; then
      vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
      if ! wait_for_vm_poweroff; then
        vbox controlvm "$VM_NAME" poweroff >/dev/null || true
      fi
    fi

    if snapshot_exists; then
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null || true
      if [[ "$DISCARD_SAVED_STATE_BEFORE_TEST" == "1" ]]; then
        vbox discardstate "$VM_NAME" >/dev/null 2>&1 || true
      fi
      prune_orphan_media_after_restore
      FINAL_RESTORE_DONE=1
    fi
  fi

  trap - EXIT
  exit "$rc"
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

  local branch
  branch="$(git -C "$REPO_ROOT" branch --show-current)"
  if [[ "$branch" != "$TARGET_BRANCH" ]]; then
    log "現在ブランチ($branch)と指定ブランチ($TARGET_BRANCH)が一致しません"
    exit 1
  fi

  local worktree_status
  worktree_status="$(git -C "$REPO_ROOT" status --porcelain --untracked-files=normal)"
  if [[ -n "$worktree_status" ]]; then
    if [[ "$ALLOW_DIRTY_WORKTREE" == "1" ]]; then
      log "未コミット差分または未追跡ファイルを含む作業ツリーをそのまま test します"
    else
      log "作業ツリーに未コミット差分または未追跡ファイルがあります。指定ブランチをクリーン状態にしてください。"
      exit 1
    fi
  fi
}

ensure_submodules() {
  log "サブモジュールを同期・初期化します"
  git -C "$REPO_ROOT" submodule sync --recursive
  git -C "$REPO_ROOT" submodule update --init --recursive

  local submodule_status
  submodule_status="$(git -C "$REPO_ROOT" submodule status --recursive || true)"
  if echo "$submodule_status" | matches_regex "^-"; then
    log "未初期化サブモジュールがあります。続行できません。"
    echo "$submodule_status"
    exit 1
  fi
}

ensure_required_dictionary_paths() {
  local dict_root="$REPO_ROOT/server-swift/azooKey_dictionary_storage"
  local emoji_root="$REPO_ROOT/server-swift/azooKey_emoji_dictionary_storage"
  local dict_dir="$dict_root/Dictionary"
  local emoji_dir="$emoji_root/EmojiDictionary"

  if [[ ! -d "$dict_root" || ! -d "$emoji_root" ]]; then
    log "辞書サブモジュールのディレクトリが見つかりません。"
    exit 1
  fi

  if [[ ! -d "$dict_dir" || ! -d "$emoji_dir" ]]; then
    log "辞書実体ディレクトリが見つかりません（Dictionary / EmojiDictionary）。"
    exit 1
  fi

  if [[ -z "$(find "$dict_dir" -type f -print -quit 2>/dev/null)" ]]; then
    log "Dictionary データが空です。サブモジュール取得状態を確認してください。"
    exit 1
  fi

  if [[ -z "$(find "$emoji_dir" -type f -print -quit 2>/dev/null)" ]]; then
    log "EmojiDictionary データが空です。サブモジュール取得状態を確認してください。"
    exit 1
  fi
}

ensure_swift_vendor_cache() {
  if [[ -z "$SWIFT_TEST_FILTER" || "$SWIFT_TEST_FILTER" == "skip" ]]; then
    return 0
  fi

  mkdir -p "$(dirname "$SWIFT_VENDOR_CACHE_DIR")"
  if [[ ! -d "$SWIFT_VENDOR_CACHE_DIR/.git" ]]; then
    log "Swift test 用依存を clone します: $SWIFT_VENDOR_REPO_URL"
    git clone "$SWIFT_VENDOR_REPO_URL" "$SWIFT_VENDOR_CACHE_DIR"
  fi

  log "Swift test 用依存を revision 固定します: $SWIFT_VENDOR_REVISION"
  git -C "$SWIFT_VENDOR_CACHE_DIR" fetch --tags --prune origin
  git -C "$SWIFT_VENDOR_CACHE_DIR" checkout --force "$SWIFT_VENDOR_REVISION"
  git -C "$SWIFT_VENDOR_CACHE_DIR" submodule sync --recursive
  git -C "$SWIFT_VENDOR_CACHE_DIR" submodule update --init --recursive
}

create_archive() {
  local archive="$1"
  log "WSL 側ソースをアーカイブします"
  tar -C "$REPO_ROOT" -czf "$archive" \
    --exclude-vcs \
    --exclude='./target' \
    --exclude='./build' \
    --exclude='./frontend/node_modules' \
    --exclude='./.local' \
    --exclude='./logs' \
    .
}

create_swift_vendor_archive() {
  local archive="$1"
  if [[ -z "$SWIFT_TEST_FILTER" || "$SWIFT_TEST_FILTER" == "skip" ]]; then
    return 0
  fi
  tar -C "$(dirname "$SWIFT_VENDOR_CACHE_DIR")" -czf "$archive" "$(basename "$SWIFT_VENDOR_CACHE_DIR")"
}

create_remote_ps1() {
  local ps1="$1"
  cat > "$ps1" <<'PS1'
param(
  [Parameter(Mandatory = $true)][string]$SourceTarPath,
  [Parameter(Mandatory = $true)][string]$SourceDir,
  [Parameter(Mandatory = $true)][string]$HostTimestampUtc,
  [Parameter(Mandatory = $false)][string]$CargoTestFilter = "composition",
  [Parameter(Mandatory = $false)][string]$SwiftTestFilter = "",
  [Parameter(Mandatory = $false)][string]$SwiftVendorTarPath = ""
)

$ErrorActionPreference = "Stop"
$env:Path += ";$env:USERPROFILE\.cargo\bin"
$env:RUST_BACKTRACE = "1"
$cacheRoot = "C:\work\azooKey-Windows"

function Sync-GuestClock {
  param([string]$TimestampUtc)

  try {
    $targetUtc = [DateTime]::Parse($TimestampUtc).ToUniversalTime().AddHours(12)
    $currentUtc = (Get-Date).ToUniversalTime()
    $deltaSeconds = [Math]::Abs(($targetUtc - $currentUtc).TotalSeconds)

    if ($deltaSeconds -gt 30) {
      Write-Host "syncing guest clock from $($currentUtc.ToString('o')) to $($targetUtc.ToString('o'))"
      Set-Date -Date $targetUtc.ToLocalTime() | Out-Null
      Write-Host "guest clock updated to $(((Get-Date).ToUniversalTime()).ToString('o'))"
    } else {
      Write-Host "guest clock already in sync"
    }
  } catch {
    Write-Host "warning: failed to sync guest clock: $($_.Exception.Message)"
  }
}

function Replace-TreeFromCache {
  param(
    [string]$CacheDir,
    [string]$DestDir,
    [string]$Label
  )
  if (!(Test-Path $CacheDir)) {
    return $false
  }
  if (Test-Path $DestDir) {
    Remove-Item -Path $DestDir -Recurse -Force
  }
  New-Item -Path (Split-Path $DestDir -Parent) -ItemType Directory -Force | Out-Null
  Copy-Item -Path $CacheDir -Destination $DestDir -Recurse -Force
  Write-Host "reused $Label from cache"
  return $true
}

function Copy-TreeIfExists {
  param(
    [string]$SourceDir,
    [string]$DestDir,
    [string]$Label
  )
  if (!(Test-Path $SourceDir)) {
    return $false
  }
  if (Test-Path $DestDir) {
    Remove-Item -Path $DestDir -Recurse -Force
  }
  New-Item -Path (Split-Path $DestDir -Parent) -ItemType Directory -Force | Out-Null
  Copy-Item -Path $SourceDir -Destination $DestDir -Recurse -Force
  Write-Host "reused $Label from cache"
  return $true
}

function Initialize-SwiftTestEnvironment {
  git config --global core.longpaths true

  $llamaLibCache = Join-Path $cacheRoot "llama_cpu\llama.lib"
  $llamaLibDest = Join-Path $SourceDir "server-swift\llama.lib"
  if ((Test-Path $llamaLibCache) -and !(Test-Path $llamaLibDest)) {
    Copy-Item $llamaLibCache -Destination $llamaLibDest -Force
    Write-Host "copied llama.lib from cache"
  }

  $emojiDictDir = Join-Path $SourceDir "server-swift\azooKey_emoji_dictionary_storage\EmojiDictionary"
  $mainDictDir = Join-Path $SourceDir "server-swift\azooKey_dictionary_storage\Dictionary"
  $cachedEmojiDictDir = Join-Path $cacheRoot "server-swift\azooKey_emoji_dictionary_storage\EmojiDictionary"
  $cachedMainDictDir = Join-Path $cacheRoot "server-swift\azooKey_dictionary_storage\Dictionary"

  if (!(Replace-TreeFromCache -CacheDir $cachedEmojiDictDir -DestDir $emojiDictDir -Label "emoji dictionary")) {
    Write-Host "emoji dictionary cache not found; using extracted source files"
  }
  if (!(Replace-TreeFromCache -CacheDir $cachedMainDictDir -DestDir $mainDictDir -Label "main dictionary")) {
    Write-Host "main dictionary cache not found; using extracted source files"
  }

  $cachedSwiftBuildDir = Join-Path $cacheRoot "server-swift\.build"
  $sourceSwiftBuildDir = Join-Path $SourceDir "server-swift\.build"
  Copy-TreeIfExists -SourceDir (Join-Path $cachedSwiftBuildDir "checkouts") -DestDir (Join-Path $sourceSwiftBuildDir "checkouts") -Label "swiftpm checkouts" | Out-Null
  Copy-TreeIfExists -SourceDir (Join-Path $cachedSwiftBuildDir "repositories") -DestDir (Join-Path $sourceSwiftBuildDir "repositories") -Label "swiftpm repositories" | Out-Null
  if (Test-Path (Join-Path $cachedSwiftBuildDir "workspace-state.json")) {
    New-Item -Path $sourceSwiftBuildDir -ItemType Directory -Force | Out-Null
    Copy-Item (Join-Path $cachedSwiftBuildDir "workspace-state.json") -Destination (Join-Path $sourceSwiftBuildDir "workspace-state.json") -Force
    Write-Host "reused swiftpm workspace state from cache"
  }

  $swiftUsrDir = $null
  if ($env:RESOLVED_SWIFT_BUILD) {
    $swiftVersionDir = $env:RESOLVED_SWIFT_BUILD -replace "-RELEASE$", ""
    $candidate = Join-Path $env:LOCALAPPDATA ("Programs\Swift\Platforms\" + $swiftVersionDir + "\Windows.platform\Developer\SDKs\Windows.sdk\usr")
    if (Test-Path $candidate) {
      $swiftUsrDir = $candidate
    }
  }
  if (-not $swiftUsrDir) {
    $swiftPlatformsRoot = Join-Path $env:LOCALAPPDATA "Programs\Swift\Platforms"
    $swiftPlatformDir = Get-ChildItem -Path $swiftPlatformsRoot -Directory -ErrorAction SilentlyContinue |
      Sort-Object Name -Descending |
      Select-Object -First 1
    if ($swiftPlatformDir) {
      $candidate = Join-Path $swiftPlatformDir.FullName "Windows.platform\Developer\SDKs\Windows.sdk\usr"
      if (Test-Path $candidate) {
        $swiftUsrDir = $candidate
      }
    }
  }
  if (-not $swiftUsrDir) {
    throw "Swift Windows SDK usr directory not found"
  }

  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $ucrtModulemapUrl = "https://gist.githubusercontent.com/fkunn1326/ef8be2217082302b291f2b8d4178194a/raw/c424968c250afcd5afa1131aea1329dc0744a7f9/ucrt.modulemap"
  $ucrtModulemapDest = Join-Path $swiftUsrDir "share\ucrt.modulemap"
  try {
    Invoke-WebRequest -Uri $ucrtModulemapUrl -OutFile $ucrtModulemapDest
    Write-Host "updated swift ucrt.modulemap: $ucrtModulemapDest"
  } catch {
    if (!(Test-Path $ucrtModulemapDest)) {
      throw
    }
    Write-Host "failed to refresh swift ucrt.modulemap; using existing file: $ucrtModulemapDest"
  }

  $vsInitScript = $null
  $vsInitArgs = "-arch=x64 -host_arch=x64"
  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path $vswhere) {
    $vsInstallPath = & $vswhere -latest -products * -property installationPath
    if (![string]::IsNullOrWhiteSpace($vsInstallPath)) {
      $candidate = Join-Path $vsInstallPath "Common7\Tools\VsDevCmd.bat"
      if (Test-Path $candidate) {
        $vsInitScript = $candidate
      }
    }
  }

  if (-not $vsInitScript) {
    $searchRoots = @(
      "C:\Program Files\Microsoft Visual Studio",
      "C:\Program Files (x86)\Microsoft Visual Studio"
    ) | Where-Object { Test-Path $_ }

    foreach ($root in $searchRoots) {
      $vsDevCmd = Get-ChildItem -Path $root -Filter VsDevCmd.bat -Recurse -ErrorAction SilentlyContinue |
        Sort-Object FullName |
        Select-Object -First 1 -ExpandProperty FullName
      if ($vsDevCmd) {
        $vsInitScript = $vsDevCmd
        break
      }
    }
  }

  if (-not $vsInitScript) {
    $searchRoots = @(
      "C:\Program Files\Microsoft Visual Studio",
      "C:\Program Files (x86)\Microsoft Visual Studio"
    ) | Where-Object { Test-Path $_ }

    foreach ($root in $searchRoots) {
      $vcVars = Get-ChildItem -Path $root -Filter vcvars64.bat -Recurse -ErrorAction SilentlyContinue |
        Sort-Object FullName |
        Select-Object -First 1 -ExpandProperty FullName
      if ($vcVars) {
        $vsInitScript = $vcVars
        $vsInitArgs = ""
        break
      }
    }
  }

  if ($vsInitScript) {
    $cmdLine = if ([string]::IsNullOrWhiteSpace($vsInitArgs)) {
      "`"$vsInitScript`" >nul && set"
    } else {
      "`"$vsInitScript`" $vsInitArgs >nul && set"
    }
    cmd.exe /s /c $cmdLine | ForEach-Object {
      if ($_ -match "^(.*?)=(.*)$") {
        Set-Item -Path ("Env:" + $matches[1]) -Value $matches[2]
      }
    }
    Write-Host "loaded Visual Studio build environment from $vsInitScript"
  } else {
    Write-Host "Visual Studio build environment not found; continuing without it"
  }

  $cppIncludePath = $null
  $cppSearchRoots = @()
  if ($vsInitScript) {
    $cppSearchRoots += (Split-Path (Split-Path (Split-Path $vsInitScript -Parent) -Parent) -Parent)
  }
  $cppSearchRoots += @(
    "C:\Program Files\Microsoft Visual Studio",
    "C:\Program Files (x86)\Microsoft Visual Studio"
  )

  foreach ($root in ($cppSearchRoots | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique)) {
    $ccomplex = Get-ChildItem -Path $root -Filter ccomplex -Recurse -ErrorAction SilentlyContinue |
      Sort-Object FullName |
      Select-Object -First 1 -ExpandProperty FullName
    if ($ccomplex) {
      $cppIncludePath = Split-Path $ccomplex -Parent
      break
    }
  }

  if ($cppIncludePath) {
    Write-Host "detected C++ include path at $cppIncludePath"
  } else {
    Write-Host "ccomplex header not found; relying on _CRT_USE_C_COMPLEX_H workaround"
  }

  if (-not (Get-Command swift -ErrorAction SilentlyContinue)) {
    throw "swift command not found"
  }
}

function Invoke-SwiftPackageTests {
  param(
    [Parameter(Mandatory = $true)][string]$SwiftSourceDir,
    [Parameter(Mandatory = $false)][string]$TestFilter = "all"
  )

  $releaseDir = Join-Path $SwiftSourceDir ".build\x86_64-unknown-windows-msvc\release"
  if (!(Test-Path $releaseDir)) {
    throw "swift release build directory not found: $releaseDir"
  }

  $testBundle = Get-ChildItem -Path $releaseDir -Filter "*.xctest" -File -ErrorAction SilentlyContinue |
    Sort-Object Name |
    Select-Object -First 1
  if (-not $testBundle) {
    throw "swift test bundle (*.xctest) not found under $releaseDir"
  }

  $runnerExe = [System.IO.Path]::ChangeExtension($testBundle.FullName, ".exe")
  Copy-Item $testBundle.FullName -Destination $runnerExe -Force

  $swiftRoot = Join-Path $env:LOCALAPPDATA "Programs\Swift"
  $runtimeDir = Get-ChildItem -Path (Join-Path $swiftRoot "Runtimes") -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    Select-Object -First 1
  if (-not $runtimeDir) {
    throw "Swift runtime directory not found under $swiftRoot"
  }

  $platformDir = Get-ChildItem -Path (Join-Path $swiftRoot "Platforms") -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    Select-Object -First 1
  if (-not $platformDir) {
    throw "Swift platform directory not found under $swiftRoot"
  }

  $swiftRuntimeBin = Join-Path $runtimeDir.FullName "usr\bin"
  $swiftToolchainBin = Split-Path (Get-Command swift).Source -Parent
  $xctestBin = Join-Path $platformDir.FullName "Windows.platform\Developer\Library\XCTest-development\usr\bin64"
  $testingBin = Join-Path $platformDir.FullName "Windows.platform\Developer\Library\Testing-development\usr\bin64"
  $llamaCpuDir = Join-Path $cacheRoot "llama_cpu"

  foreach ($requiredPath in @($swiftRuntimeBin, $swiftToolchainBin, $xctestBin, $testingBin, $llamaCpuDir)) {
    if (!(Test-Path $requiredPath)) {
      throw "required swift test runtime path not found: $requiredPath"
    }
  }

  $env:Path = @(
    $releaseDir,
    $llamaCpuDir,
    $swiftRuntimeBin,
    $swiftToolchainBin,
    $xctestBin,
    $testingBin,
    $env:Path
  ) -join ";"

  $runArgs = @("--testing-library", "swift-testing")
  if (![string]::IsNullOrWhiteSpace($TestFilter) -and $TestFilter -ne "all") {
    $runArgs += @("--filter", $TestFilter)
  }

  Write-Host "running swift test runner $runnerExe $($runArgs -join ' ')"
  Push-Location $releaseDir
  try {
    & $runnerExe @runArgs
  } finally {
    Pop-Location
  }

  if ($LASTEXITCODE -ne 0) {
    throw "swift test runner failed with exit code $LASTEXITCODE"
  }
}

if (Test-Path $SourceDir) {
  Remove-Item -Recurse -Force $SourceDir
}
New-Item -Path $SourceDir -ItemType Directory -Force | Out-Null

tar -xzf $SourceTarPath -C $SourceDir
Set-Location $SourceDir
Write-Host "source extracted: $SourceDir"
Sync-GuestClock -TimestampUtc $HostTimestampUtc

if (![string]::IsNullOrWhiteSpace($SwiftVendorTarPath) -and (Test-Path $SwiftVendorTarPath)) {
  $vendorRoot = Join-Path $SourceDir "vendor"
  if (Test-Path $vendorRoot) {
    Remove-Item -Path $vendorRoot -Recurse -Force
  }
  New-Item -Path $vendorRoot -ItemType Directory -Force | Out-Null
  tar -xzf $SwiftVendorTarPath -C $vendorRoot

  $packageSwift = Join-Path $SourceDir "server-swift\Package.swift"
  $packageRaw = Get-Content $packageSwift -Raw
  $packageRaw = [regex]::Replace(
    $packageRaw,
    '(?ms)\.package\(\s*url:\s*"https://github\.com/(?:azookey|batao9)/AzooKeyKanaKanjiConverter",\s*(?:revision|branch):\s*"[^"]+",\s*traits:\s*\["Zenzai"\]\s*\)',
    ".package(`n            path: `"../vendor/AzooKeyKanaKanjiConverter`",`n            traits: [`"Zenzai`"]`n        )"
  )
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($packageSwift, $packageRaw, $utf8NoBom)
  Write-Host "rewired Package.swift to use local AzooKeyKanaKanjiConverter vendor"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "cargo command not found"
}

if (![string]::IsNullOrWhiteSpace($CargoTestFilter) -and $CargoTestFilter -ne "skip") {
  Write-Host "running cargo test -p azookey-windows $CargoTestFilter -- --nocapture"
  cargo test -p azookey-windows $CargoTestFilter -- --nocapture
  if ($LASTEXITCODE -ne 0) {
    throw "cargo test failed with exit code $LASTEXITCODE"
  }
} else {
  Write-Host "skipping cargo test"
}

if (![string]::IsNullOrWhiteSpace($SwiftTestFilter) -and $SwiftTestFilter -ne "skip") {
  Initialize-SwiftTestEnvironment
  $swiftSourceDir = Join-Path $SourceDir "server-swift"
  Set-Location $swiftSourceDir
  if ($SwiftTestFilter -eq "all") {
    Write-Host "running swift test -c release --verbose -Xcc -D_CRT_USE_C_COMPLEX_H"
    swift test -c release --verbose -Xcc -D_CRT_USE_C_COMPLEX_H
  } else {
    Write-Host "running swift test -c release --verbose -Xcc -D_CRT_USE_C_COMPLEX_H --filter $SwiftTestFilter"
    swift test -c release --verbose -Xcc -D_CRT_USE_C_COMPLEX_H --filter $SwiftTestFilter
  }
  if ($LASTEXITCODE -eq 0) {
    Write-Host "swift test completed without Windows runner workaround"
  } else {
    Write-Host "swift test returned exit code $LASTEXITCODE; retrying with Windows runner workaround"
    Invoke-SwiftPackageTests -SwiftSourceDir $swiftSourceDir -TestFilter $SwiftTestFilter
  }
} else {
  Write-Host "skipping swift test"
}
PS1
}

main() {
  ensure_preconditions
  stop_staging_vm_if_running
  ensure_submodules
  ensure_required_dictionary_paths
  ensure_swift_vendor_cache

  TMP_SRC_ARCHIVE="$(mktemp "/tmp/azookey-client-test.XXXXXX.tar.gz")"
  TMP_REMOTE_PS="$(mktemp /tmp/azookey-vm-client-test.XXXXXX.ps1)"
  if [[ -n "$SWIFT_TEST_FILTER" && "$SWIFT_TEST_FILTER" != "skip" ]]; then
    TMP_SWIFT_VENDOR_ARCHIVE="$(mktemp /tmp/azookey-swift-vendor.XXXXXX.tar.gz)"
  fi

  create_archive "$TMP_SRC_ARCHIVE"
  create_remote_ps1 "$TMP_REMOTE_PS"
  if [[ -n "${TMP_SWIFT_VENDOR_ARCHIVE:-}" ]]; then
    create_swift_vendor_archive "$TMP_SWIFT_VENDOR_ARCHIVE"
  fi

  if [[ "$RESTORE_BEFORE_TEST" == "1" ]]; then
    if snapshot_exists; then
      VM_TOUCHED=1
      if is_vm_running; then
        log "スナップショット復元のため VM を停止します"
        vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
        if ! wait_for_vm_poweroff; then
          vbox controlvm "$VM_NAME" poweroff >/dev/null
        fi
      fi
      log "test 前にスナップショットを復元します: $SNAPSHOT_NAME"
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null
      if [[ "$DISCARD_SAVED_STATE_BEFORE_TEST" == "1" ]]; then
        vbox discardstate "$VM_NAME" >/dev/null 2>&1 || true
      fi
      prune_orphan_media_after_restore
    fi
  fi

  if ! is_vm_running; then
    VM_TOUCHED=1
    log "VM を起動します: $VM_NAME"
    start_vm
  else
    VM_TOUCHED=1
    log "VM は既に起動済みです: $VM_NAME"
  fi

  if ! wait_for_ssh; then
    log "VM への SSH 接続に失敗しました"
    exit 1
  fi

  log "アーカイブと test スクリプトを VM に転送します"
  scp_to_vm "$TMP_SRC_ARCHIVE" "$REMOTE_TAR_SCP"
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"
  if [[ -n "${TMP_SWIFT_VENDOR_ARCHIVE:-}" ]]; then
    scp_to_vm "$TMP_SWIFT_VENDOR_ARCHIVE" "$REMOTE_SWIFT_VENDOR_TAR_SCP"
  fi

  local swift_vendor_arg=""
  if [[ -n "${TMP_SWIFT_VENDOR_ARCHIVE:-}" ]]; then
    swift_vendor_arg=" -SwiftVendorTarPath \"$REMOTE_SWIFT_VENDOR_TAR_WIN\""
  fi

  log "VM 上で test を実行します"
  ssh_run_test "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -SourceTarPath \"$REMOTE_TAR_WIN\" -SourceDir \"$REMOTE_SRC_WIN\" -HostTimestampUtc \"$HOST_TIMESTAMP_UTC\" -CargoTestFilter \"$CARGO_TEST_FILTER\" -SwiftTestFilter \"$SWIFT_TEST_FILTER\"$swift_vendor_arg"

  log "VM を停止します"
  vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
  if ! wait_for_vm_poweroff; then
    log "通常停止できなかったため poweroff します"
    vbox controlvm "$VM_NAME" poweroff >/dev/null
  fi

  if [[ "$RESTORE_AFTER_TEST" == "1" ]]; then
    if snapshot_exists; then
      log "test 後にクリーン状態へ戻すため復元します: $SNAPSHOT_NAME"
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null
      if [[ "$DISCARD_SAVED_STATE_BEFORE_TEST" == "1" ]]; then
        vbox discardstate "$VM_NAME" >/dev/null 2>&1 || true
      fi
      prune_orphan_media_after_restore
      FINAL_RESTORE_DONE=1
    fi
  fi

  log "完了: $LOG_FILE"
}

trap cleanup EXIT
main "$@"
