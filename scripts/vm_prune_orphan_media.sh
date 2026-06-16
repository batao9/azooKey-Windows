#!/usr/bin/env bash
set -euo pipefail

VM_NAME="${1:-}"
VBOX_MANAGE="${VBOX_MANAGE:-}"

if [[ -z "$VM_NAME" ]]; then
  echo "Usage: VBOX_MANAGE=... $0 <VM_NAME>" >&2
  exit 1
fi

if [[ -z "$VBOX_MANAGE" ]]; then
  if command -v VBoxManage >/dev/null 2>&1; then
    VBOX_MANAGE="$(command -v VBoxManage)"
  elif [[ -x "/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe" ]]; then
    VBOX_MANAGE="/mnt/c/Program Files/Oracle/VirtualBox/VBoxManage.exe"
  fi
fi

if [[ ! -x "$VBOX_MANAGE" ]]; then
  echo "[vm-prune-orphan-media] VBoxManage が見つかりません。VBOX_MANAGE を設定してください: ${VBOX_MANAGE:-<unset>}" >&2
  exit 1
fi

vbox() {
  "$VBOX_MANAGE" "$@"
}

machine_cfg_file() {
  vbox showvminfo "$VM_NAME" --machinereadable |
    awk -F= '$1 == "CfgFile" { value=$2 } END { gsub(/"/, "", value); print value }'
}

machine_snapshot_dir() {
  vbox showvminfo "$VM_NAME" --machinereadable |
    awk -F= '$1 == "SnapFldr" { value=$2 } END { gsub(/"/, "", value); print value }'
}

log() {
  printf '[vm-prune-orphan-media] %s\n' "$*"
}

cfg_file="$(machine_cfg_file)"
cfg_file="${cfg_file//\\\\/\\}"
if [[ -z "$cfg_file" ]]; then
  log "VM 設定ファイルが取得できないため orphan media prune をスキップします"
  exit 0
fi

vm_dir="${cfg_file%\\*}"
snapshot_dir="$(machine_snapshot_dir)"
snapshot_dir="${snapshot_dir//\\\\/\\}"
if [[ -z "$snapshot_dir" ]]; then
  snapshot_dir="$vm_dir\\Snapshots"
fi

candidates=()
mapfile -t candidates < <(
  vbox list hdds --long | tr -d '\r' |
    SNAPSHOT_DIR="$snapshot_dir" awk '
      BEGIN {
        RS="\n\n"; FS="\n";
        snapshot_dir = ENVIRON["SNAPSHOT_DIR"];
        gsub(/\\/, "/", snapshot_dir);
        gsub(/\/+$/, "", snapshot_dir);
        snapshot_dir = tolower(snapshot_dir "/");
      }
      {
        uuid=""; loc=""; size=""; use="no"; child="no";
        for (i=1; i<=NF; i++) {
          line=$i;
          if (line ~ /^UUID:/) { sub(/^UUID:[[:space:]]*/, "", line); uuid=line }
          if (line ~ /^Location:/) { sub(/^Location:[[:space:]]*/, "", line); loc=line }
          if (line ~ /^Size on disk:/) { sub(/^Size on disk:[[:space:]]*/, "", line); size=line }
          if (line ~ /^In use by VMs:/) { use="yes" }
          if (line ~ /^Child UUIDs:/) { child="yes" }
        }
        loc_norm = loc;
        gsub(/\\/, "/", loc_norm);
        loc_norm_lc = tolower(loc_norm);
        in_scope = (index(loc_norm_lc, snapshot_dir) == 1);
        if (uuid != "" && use == "no" && child == "no" && in_scope && loc_norm ~ /\.vdi$/) {
          print uuid "\t" size "\t" loc
        }
      }'
)

if (( ${#candidates[@]} == 0 )); then
  log "未接続 leaf VDI はありません"
  exit 0
fi

for entry in "${candidates[@]}"; do
  IFS=$'\t' read -r uuid size loc <<<"$entry"
  log "未接続 leaf VDI を削除します: $uuid ($size) $loc"
  vbox closemedium disk "$uuid" --delete </dev/null
done
