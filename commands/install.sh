#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install.sh [--prefix <dir>] [--copy]

  --prefix <dir>  Install under <dir>/bin (default: ~/.local).
  --copy          Copy the command instead of symlinking.
EOF
}

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
prefix="${PREFIX:-$HOME/.local}"
copy_mode=0

while (( $# )); do
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
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

source_command="$script_dir/bin/mbx"
target_command="$prefix/bin/mbx"

[[ -f "$source_command" ]] || die "missing $source_command"
mkdir -p "$prefix/bin"

if (( copy_mode )); then
  if [[ -L "$target_command" ]]; then
    rm -f "$target_command"
  fi
  install -m 0755 "$source_command" "$target_command"
else
  ln -sfn "$source_command" "$target_command"
fi

printf 'Installed %s\n' "$target_command"
