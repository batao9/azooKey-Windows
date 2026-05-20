#!/usr/bin/env bash
# Windows 上の VirtualBox VM 内でビルドを実行するためのスクリプトです。
# 出力が長くなりがちなので、サブエージェントから呼び出すことを推奨します。

set -euo pipefail

VM_NAME="${VM_NAME:-}"
SNAPSHOT_NAME="${SNAPSHOT_NAME:-}"
RESTORE_BEFORE_BUILD="${RESTORE_BEFORE_BUILD:-1}"
RESTORE_AFTER_BUILD="${RESTORE_AFTER_BUILD:-1}"
ALLOW_DIRTY_WORKTREE="${ALLOW_DIRTY_WORKTREE:-1}"
DISCARD_SAVED_STATE_BEFORE_BUILD="${DISCARD_SAVED_STATE_BEFORE_BUILD:-0}"
SSH_USER="${SSH_USER:-}"
SSH_PORT="${SSH_PORT:-}"
SSH_KEY="${SSH_KEY:-}"
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
  echo "Usage: VM_NAME=... SNAPSHOT_NAME=... SSH_USER=... SSH_PORT=... SSH_KEY=... $0 <branch>"
  echo "Example: VM_NAME=<vm-name> SNAPSHOT_NAME=<snapshot-name> SSH_USER=<user> SSH_PORT=<port> SSH_KEY=<key-path> $0 feature/example"
  exit 1
fi

TARGET_BRANCH="$1"
TARGET_BRANCH_SLUG="$(printf '%s' "$TARGET_BRANCH" | tr '/ ' '__')"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="$REPO_ROOT/.local/artifacts"
mkdir -p "$ARTIFACT_DIR"

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
LOCAL_ARTIFACT="$ARTIFACT_DIR/azookey-setup-${TARGET_BRANCH_SLUG}-$TIMESTAMP.exe"

REMOTE_TMP_WIN="C:\\Users\\$SSH_USER\\AppData\\Local\\Temp"
REMOTE_TAR_WIN="$REMOTE_TMP_WIN\\azookey-src.tar.gz"
REMOTE_PS_WIN="$REMOTE_TMP_WIN\\azookey-vm-build.ps1"
REMOTE_SRC_WIN="C:\\work\\azookey-src-$TIMESTAMP"
REMOTE_ART_WIN="C:\\work\\artifacts"

REMOTE_TAR_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-src.tar.gz"
REMOTE_PS_SCP="/C:/Users/$SSH_USER/AppData/Local/Temp/azookey-vm-build.ps1"
REMOTE_ART_SCP="/C:/work/artifacts/azookey-setup.exe"

SSH_OPTS=(-i "$SSH_KEY" -p "$SSH_PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)
SCP_OPTS=(-i "$SSH_KEY" -P "$SSH_PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8)

TMP_SRC_ARCHIVE=""
TMP_REMOTE_PS=""
VM_TOUCHED=0
FINAL_RESTORE_DONE=0

log() {
  printf '[vm-build] %s\n' "$*"
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

ssh_run() {
  ssh "${SSH_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST" "$@"
}

scp_to_vm() {
  scp "${SCP_OPTS[@]}" "$1" "$SSH_USER@$ACTIVE_HOST:$2"
}

scp_from_vm() {
  scp "${SCP_OPTS[@]}" "$SSH_USER@$ACTIVE_HOST:$1" "$2"
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
      # sshd 起動直後の banner 待ちでハングしないよう 1 試行ごとに上限時間を設ける。
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

stop_staging_vm_if_running() {
  if [[ -z "$STAGING_VM_NAME" || "$STAGING_VM_NAME" == "$VM_NAME" ]]; then
    return 0
  fi

  if ! is_named_vm_running "$STAGING_VM_NAME"; then
    return 0
  fi

  log "ビルド前に staging VM を停止します: $STAGING_VM_NAME"
  vbox controlvm "$STAGING_VM_NAME" acpipowerbutton >/dev/null || true
  if ! wait_for_named_vm_poweroff "$STAGING_VM_NAME"; then
    log "staging VM が停止しないため強制停止します: $STAGING_VM_NAME"
    vbox controlvm "$STAGING_VM_NAME" poweroff >/dev/null || true
  fi
}

cleanup() {
  local rc=$?
  set +e

  rm -f "${TMP_SRC_ARCHIVE:-}" "${TMP_REMOTE_PS:-}"

  if [[ "$rc" -ne 0 && "$RESTORE_AFTER_BUILD" == "1" && "$VM_TOUCHED" == "1" && "$FINAL_RESTORE_DONE" != "1" ]]; then
    log "エラー終了のためクリーン状態へ復元します: $SNAPSHOT_NAME"
    if is_vm_running; then
      vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
      if ! wait_for_vm_poweroff; then
        vbox controlvm "$VM_NAME" poweroff >/dev/null || true
      fi
    fi

    if snapshot_exists; then
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null || true
      FINAL_RESTORE_DONE=1
    else
      log "復元対象スナップショットが見つからないためスキップします: $SNAPSHOT_NAME"
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
      log "未コミット差分または未追跡ファイルを含む作業ツリーをそのままビルドします"
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

create_remote_ps1() {
  local ps1="$1"
  cat > "$ps1" <<'PS1'
param(
  [Parameter(Mandatory = $true)][string]$SourceTarPath,
  [Parameter(Mandatory = $true)][string]$SourceDir,
  [Parameter(Mandatory = $true)][string]$ArtifactDir,
  [Parameter(Mandatory = $true)][string]$HostTimestampUtc
)

$ErrorActionPreference = "Stop"
$env:Path += ";$env:USERPROFILE\\.cargo\\bin"

$LLAMA_CPU_URL = "https://github.com/fkunn1326/llama.cpp/releases/download/b4846/llama-b4846-bin-win-avx-x64.zip"
$LLAMA_CUDA_URL = "https://github.com/fkunn1326/llama.cpp/releases/download/b4846/llama-b4846-bin-win-cuda-cu12.4-x64.zip"
$LLAMA_VULKAN_URL = "https://github.com/fkunn1326/llama.cpp/releases/download/b4846/llama-b4846-bin-win-vulkan-x64.zip"
$ZENZ_MODEL_URL = "https://huggingface.co/Miwa-Keita/zenz-v3-small-gguf/resolve/main/ggml-model-Q5_K_M.gguf"
$downloadAllAssets = $env:DOWNLOAD_ALL_ASSETS -eq "1"
$cacheRoot = "C:\work\azooKey-Windows"

function Sync-GuestClock {
  param([string]$TimestampUtc)

  try {
    # GitHub/Tauri tool downloads can fail with NotValidYet if the restored VM clock
    # is behind the host or the certificate validity window has just rolled.
    # Keep a wide safety margin for local VM builds so snapshot restore time skew does not block bundling.
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

function Download-Extract {
  param(
    [string]$Url,
    [string]$DestFolder
  )
  $tempZip = Join-Path $env:TEMP ([System.IO.Path]::GetRandomFileName() + ".zip")
  Invoke-WebRequest -Uri $Url -OutFile $tempZip
  New-Item -Path $DestFolder -ItemType Directory -Force | Out-Null
  Expand-Archive -Path $tempZip -DestinationPath $DestFolder -Force
  Remove-Item $tempZip -Force
}

function Copy-TreeIfExists {
  param(
    [string]$SourceDir,
    [string]$DestDir
  )
  if (!(Test-Path $SourceDir)) {
    return $false
  }
  New-Item -Path $DestDir -ItemType Directory -Force | Out-Null
  Copy-Item -Path (Join-Path $SourceDir "*") -Destination $DestDir -Recurse -Force
  return $true
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

if (Test-Path $SourceDir) {
  Remove-Item -Recurse -Force $SourceDir
}
New-Item -Path $SourceDir -ItemType Directory -Force | Out-Null

tar -xzf $SourceTarPath -C $SourceDir
Set-Location $SourceDir
Write-Host "source extracted: $SourceDir"
Sync-GuestClock -TimestampUtc $HostTimestampUtc

$llamaCpuDir = Join-Path $SourceDir "llama_cpu"
$llamaCudaDir = Join-Path $SourceDir "llama_cuda"
$llamaVulkanDir = Join-Path $SourceDir "llama_vulkan"
$zenzPath = Join-Path $SourceDir "zenz.gguf"
$emojiDictDir = Join-Path $SourceDir "server-swift\\azooKey_emoji_dictionary_storage\\EmojiDictionary"
$mainDictDir = Join-Path $SourceDir "server-swift\\azooKey_dictionary_storage\\Dictionary"

if (!(Test-Path (Join-Path $llamaCpuDir "llama.lib"))) {
  if (Copy-TreeIfExists -SourceDir (Join-Path $cacheRoot "llama_cpu") -DestDir $llamaCpuDir) {
    Write-Host "reused llama cpu assets from cache"
  } else {
    Write-Host "downloading llama cpu assets"
    Download-Extract -Url $LLAMA_CPU_URL -DestFolder $llamaCpuDir
  }
}

if ($downloadAllAssets) {
  if (!(Test-Path (Join-Path $llamaCudaDir "llama.dll"))) {
    if (Copy-TreeIfExists -SourceDir (Join-Path $cacheRoot "llama_cuda") -DestDir $llamaCudaDir) {
      Write-Host "reused llama cuda assets from cache"
    } else {
      Write-Host "downloading llama cuda assets"
      Download-Extract -Url $LLAMA_CUDA_URL -DestFolder $llamaCudaDir
    }
  }
  if (!(Test-Path (Join-Path $llamaVulkanDir "llama.dll"))) {
    if (Copy-TreeIfExists -SourceDir (Join-Path $cacheRoot "llama_vulkan") -DestDir $llamaVulkanDir) {
      Write-Host "reused llama vulkan assets from cache"
    } else {
      Write-Host "downloading llama vulkan assets"
      Download-Extract -Url $LLAMA_VULKAN_URL -DestFolder $llamaVulkanDir
    }
  }
  if (!(Test-Path $zenzPath)) {
    $cachedZenzPath = Join-Path $cacheRoot "zenz.gguf"
    if (Test-Path $cachedZenzPath) {
      Copy-Item $cachedZenzPath -Destination $zenzPath -Force
      Write-Host "reused zenz model from cache"
    } else {
      Write-Host "downloading zenz model"
      Invoke-WebRequest -Uri $ZENZ_MODEL_URL -OutFile $zenzPath
    }
  }
} else {
  # Fast local build mode: create minimum assets required by post_build copy steps.
  New-Item -Path $llamaCudaDir -ItemType Directory -Force | Out-Null
  New-Item -Path $llamaVulkanDir -ItemType Directory -Force | Out-Null

  if (!(Test-Path (Join-Path $llamaCudaDir "llama.dll"))) {
    if (!(Copy-TreeIfExists -SourceDir (Join-Path $cacheRoot "llama_cuda") -DestDir $llamaCudaDir)) {
      Copy-Item (Join-Path $llamaCpuDir "llama.dll") -Destination (Join-Path $llamaCudaDir "llama.dll") -Force
    }
  }
  if (!(Test-Path (Join-Path $llamaVulkanDir "llama.dll"))) {
    if (!(Copy-TreeIfExists -SourceDir (Join-Path $cacheRoot "llama_vulkan") -DestDir $llamaVulkanDir)) {
      Copy-Item (Join-Path $llamaCpuDir "llama.dll") -Destination (Join-Path $llamaVulkanDir "llama.dll") -Force
    }
  }
  if (!(Test-Path $zenzPath)) {
    $cachedZenzPath = Join-Path $cacheRoot "zenz.gguf"
    if (Test-Path $cachedZenzPath) {
      Copy-Item $cachedZenzPath -Destination $zenzPath -Force
    } else {
      New-Item -Path $zenzPath -ItemType File -Force | Out-Null
    }
  }
}

Copy-Item (Join-Path $llamaCpuDir "llama.lib") -Destination (Join-Path $SourceDir "server-swift") -Force

$cachedEmojiDictDir = Join-Path $cacheRoot "server-swift\\azooKey_emoji_dictionary_storage\\EmojiDictionary"
$cachedMainDictDir = Join-Path $cacheRoot "server-swift\\azooKey_dictionary_storage\\Dictionary"

# NOTE:
# WSL(tar) -> Windows(tar -xzf) で日本語ファイル名が壊れる環境があるため、
# 辞書は Windows 側 clone のキャッシュを優先して上書きする。
if (!(Replace-TreeFromCache -CacheDir $cachedEmojiDictDir -DestDir $emojiDictDir -Label "emoji dictionary")) {
  Write-Host "emoji dictionary cache not found; using extracted source files"
}
if (!(Replace-TreeFromCache -CacheDir $cachedMainDictDir -DestDir $mainDictDir -Label "main dictionary")) {
  Write-Host "main dictionary cache not found; using extracted source files"
}

if (!(Test-Path $emojiDictDir)) {
  throw "EmojiDictionary not found in source/cache"
}
if (!(Test-Path $mainDictDir)) {
  throw "Dictionary not found in source/cache"
}
if ((Get-ChildItem -Path $emojiDictDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object).Count -eq 0) {
  throw "EmojiDictionary is empty"
}
if ((Get-ChildItem -Path $mainDictDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object).Count -eq 0) {
  throw "Dictionary is empty"
}
$katakanaProbe = Join-Path $mainDictDir ("p\\" + "p_" + [char]0x30A2 + ".csv")
if (!(Test-Path $katakanaProbe)) {
  throw "Dictionary filename encoding appears broken (missing p_ア.csv)."
}

$swiftUsrDir = $null
if ($env:RESOLVED_SWIFT_BUILD) {
  $swiftVersionDir = $env:RESOLVED_SWIFT_BUILD -replace "-RELEASE$", ""
  $candidate = Join-Path $env:LOCALAPPDATA ("Programs\\Swift\\Platforms\\" + $swiftVersionDir + "\\Windows.platform\\Developer\\SDKs\\Windows.sdk\\usr")
  if (Test-Path $candidate) {
    $swiftUsrDir = $candidate
  }
}
if (-not $swiftUsrDir) {
  $swiftPlatformsRoot = Join-Path $env:LOCALAPPDATA "Programs\\Swift\\Platforms"
  $swiftPlatformDir = Get-ChildItem -Path $swiftPlatformsRoot -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    Select-Object -First 1
  if ($swiftPlatformDir) {
    $candidate = Join-Path $swiftPlatformDir.FullName "Windows.platform\\Developer\\SDKs\\Windows.sdk\\usr"
    if (Test-Path $candidate) {
      $swiftUsrDir = $candidate
    }
  }
}
if ($swiftUsrDir) {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $ucrtModulemapUrl = "https://gist.githubusercontent.com/fkunn1326/ef8be2217082302b291f2b8d4178194a/raw/c424968c250afcd5afa1131aea1329dc0744a7f9/ucrt.modulemap"
  $ucrtModulemapDest = Join-Path $swiftUsrDir "share\\ucrt.modulemap"
  try {
    Invoke-WebRequest -Uri $ucrtModulemapUrl -OutFile $ucrtModulemapDest
    Write-Host "updated swift ucrt.modulemap: $ucrtModulemapDest"
  } catch {
    if (!(Test-Path $ucrtModulemapDest)) {
      throw
    }
    Write-Host "failed to refresh swift ucrt.modulemap; using existing file: $ucrtModulemapDest"
  }
} else {
  throw "Swift Windows SDK usr directory not found"
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\\Installer\\vswhere.exe"
if (Test-Path $vswhere) {
  $vsInstallPath = & $vswhere -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if (![string]::IsNullOrWhiteSpace($vsInstallPath)) {
    $vsDevCmd = Join-Path $vsInstallPath "Common7\\Tools\\VsDevCmd.bat"
    if (Test-Path $vsDevCmd) {
      cmd.exe /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set" | ForEach-Object {
        if ($_ -match "^(.*?)=(.*)$") {
          Set-Item -Path ("Env:" + $matches[1]) -Value $matches[2]
        }
      }
      Write-Host "loaded Visual Studio build environment"
    } else {
      Write-Host "VsDevCmd.bat not found; continuing without Visual Studio build environment"
    }
  } else {
    Write-Host "Visual Studio VC tools not found; continuing without Visual Studio build environment"
  }
} else {
  Write-Host "vswhere.exe not found; continuing without Visual Studio build environment"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "cargo command not found"
}
if (-not (Get-Command cargo-make -ErrorAction SilentlyContinue)) {
  Write-Host "installing cargo-make"
  cargo install --locked cargo-make
}

Set-Location (Join-Path $SourceDir "frontend")
Write-Host "running npm ci"
npm.cmd ci --prefer-offline --no-audit

Set-Location $SourceDir
Write-Host "forcing tauri bundle target to nsis for local VM build"
$tauriConfigPath = Join-Path $SourceDir "frontend\\src-tauri\\tauri.conf.json"
$tauriConfigRaw = Get-Content $tauriConfigPath -Raw
if ($tauriConfigRaw -match '"targets"\s*:\s*"all"') {
  $tauriConfigRaw = $tauriConfigRaw -replace '"targets"\s*:\s*"all"', '"targets": "nsis"'
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($tauriConfigPath, $tauriConfigRaw, $utf8NoBom)
}
Write-Host "running cargo make build --release"
cargo make build --release

New-Item -Path $ArtifactDir -ItemType Directory -Force | Out-Null
Copy-Item (Join-Path $SourceDir "build\\azookey-setup.exe") -Destination (Join-Path $ArtifactDir "azookey-setup.exe") -Force
Write-Host "build finished: $ArtifactDir\\azookey-setup.exe"
PS1
}

main() {
  ensure_preconditions
  stop_staging_vm_if_running
  ensure_submodules
  ensure_required_dictionary_paths

  TMP_SRC_ARCHIVE="$(mktemp "/tmp/azookey-${TARGET_BRANCH_SLUG}-src.XXXXXX.tar.gz")"
  TMP_REMOTE_PS="$(mktemp /tmp/azookey-vm-build.XXXXXX.ps1)"

  create_archive "$TMP_SRC_ARCHIVE"
  create_remote_ps1 "$TMP_REMOTE_PS"

  if [[ "$RESTORE_BEFORE_BUILD" == "1" ]]; then
    if snapshot_exists; then
      VM_TOUCHED=1
      if is_vm_running; then
        log "スナップショット復元のため VM を停止します"
        vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
        if ! wait_for_vm_poweroff; then
          vbox controlvm "$VM_NAME" poweroff >/dev/null
        fi
      fi
      log "ビルド前にスナップショットを復元します: $SNAPSHOT_NAME"
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null
      if [[ "$DISCARD_SAVED_STATE_BEFORE_BUILD" == "1" ]]; then
        vbox discardstate "$VM_NAME" >/dev/null 2>&1 || true
      fi
    else
      log "復元対象スナップショットが見つからないためスキップします: $SNAPSHOT_NAME"
    fi
  fi

  if ! is_vm_running; then
    VM_TOUCHED=1
    log "VM を起動します: $VM_NAME"
    vbox startvm "$VM_NAME" --type headless >/dev/null
  else
    VM_TOUCHED=1
    log "VM は既に起動済みです: $VM_NAME"
  fi

  if ! wait_for_ssh; then
    log "VM への SSH 接続に失敗しました"
    exit 1
  fi

  log "アーカイブとビルドスクリプトを VM に転送します"
  scp_to_vm "$TMP_SRC_ARCHIVE" "$REMOTE_TAR_SCP"
  scp_to_vm "$TMP_REMOTE_PS" "$REMOTE_PS_SCP"

  log "VM 上でビルドを実行します（時間がかかる場合があります）"
  ssh_run "powershell -NoProfile -ExecutionPolicy Bypass -File \"$REMOTE_PS_WIN\" -SourceTarPath \"$REMOTE_TAR_WIN\" -SourceDir \"$REMOTE_SRC_WIN\" -ArtifactDir \"$REMOTE_ART_WIN\" -HostTimestampUtc \"$HOST_TIMESTAMP_UTC\""

  log "成果物を WSL 側へ回収します"
  scp_from_vm "$REMOTE_ART_SCP" "$LOCAL_ARTIFACT"

  log "VM を停止します"
  vbox controlvm "$VM_NAME" acpipowerbutton >/dev/null || true
  if ! wait_for_vm_poweroff; then
    log "通常停止できなかったため poweroff します"
    vbox controlvm "$VM_NAME" poweroff >/dev/null
  fi

  if [[ "$RESTORE_AFTER_BUILD" == "1" ]]; then
    if snapshot_exists; then
      log "ビルド後にクリーン状態へ戻すため復元します: $SNAPSHOT_NAME"
      vbox snapshot "$VM_NAME" restore "$SNAPSHOT_NAME" >/dev/null
      FINAL_RESTORE_DONE=1
    else
      log "復元対象スナップショットが見つからないためスキップします: $SNAPSHOT_NAME"
    fi
  fi

  log "完了: $LOCAL_ARTIFACT"
}

trap cleanup EXIT
main "$@"
