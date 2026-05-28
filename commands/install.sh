#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  install.sh [--prefix <dir>] [--copy] [--remove-legacy]

Options:
  --prefix <dir>     Install under <dir>/bin. Default: ~/.local
  --copy             Copy mbx instead of symlinking to this checkout.
  --remove-legacy    Move old ai/aione/codex-slot helpers out of PATH.
  -h, --help         Show this help.
EOF
}

die() {
  echo "install.sh: $*" >&2
  exit 1
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
prefix="${PREFIX:-$HOME/.local}"
copy_mode=0
remove_legacy=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || die "--prefix requires a directory"
      prefix="$2"
      shift 2
      ;;
    --copy)
      copy_mode=1
      shift
      ;;
    --remove-legacy)
      remove_legacy=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

bin_dir="$prefix/bin"
mkdir -p "$bin_dir"

src="$script_dir/bin/mbx"
target="$bin_dir/mbx"
[[ -f "$src" ]] || die "missing $src"

if (( copy_mode )); then
  install -m 0755 "$src" "$target"
else
  ln -sfn "$src" "$target"
fi

if (( remove_legacy )); then
  backup_dir="${XDG_DATA_HOME:-$HOME/.local/share}/mobailmux/legacy-bin-backup-$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$backup_dir"

  legacy_names=(
    ai aiupdate aitrustupdate
    aione aitwo aithree aifour aifive aisix aiseven aieight ainine
    ailist aicheck aistopall
    ainewone ainewtwo ainewthree ainewfour ainewfive ainewsix ainewseven aineweight ainewnine
    airesumeone airesumetwo airesumethree airesumefour airesumefive airesumesix airesumeseven airesumeeight airesumenine
    aistopone aistoptwo aistopthree aistopfour aistopfive aistopsix aistopseven aistopeight aistopnine
    codex-slot codex-ls codex-stop
  )

  moved=0
  for name in "${legacy_names[@]}"; do
    path="$bin_dir/$name"
    if [[ -e "$path" || -L "$path" ]]; then
      mv -f "$path" "$backup_dir/$name"
      moved=1
    fi
  done

  if (( moved )); then
    echo "Moved legacy helpers to $backup_dir"
  else
    rmdir "$backup_dir"
  fi
fi

echo "Installed $target"
