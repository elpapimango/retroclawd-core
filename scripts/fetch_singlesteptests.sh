#!/usr/bin/env bash
# Fetches a SingleStepTests suite (never committed) into a folder of your own.
#   scripts/fetch_singlesteptests.sh 65816 /path/to/sst65816     (then RC_SST_65816=/path/to/sst65816 cargo test --release ...)
#   scripts/fetch_singlesteptests.sh spc700 /path/to/sstspc700
# 65816 is about 3.3 GB: 512 files (an opcode in each of emulation and native mode).
set -euo pipefail
suite=${1:?suite: 65816 or spc700}
dir=${2:?folder}
mkdir -p "$dir"
cd "$dir"
case "$suite" in
  65816) names=$(for i in $(seq 0 255); do h=$(printf '%02x' "$i"); echo "$h.e.json"; echo "$h.n.json"; done); path=v1 ;;
  spc700) names=$(for i in $(seq 0 255); do printf '%02x.json\n' "$i"; done); path=v1 ;;
  *) echo "unknown suite $suite" >&2; exit 1 ;;
esac
echo "$names" | xargs -P 8 -I{} sh -c '[ -s {} ] || curl -sf --retry 3 -o {} "https://raw.githubusercontent.com/SingleStepTests/'"$suite"'/main/'"$path"'/{}"'
echo "done: $(ls | wc -l) files in $dir"
