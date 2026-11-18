#!/usr/bin/env bash
# Reproduces the README benchmarks. Requires hyperfine:
# https://github.com/sharkdp/hyperfine
set -euo pipefail

FFIND=${FFIND:-ffind}
FD=${FD:-fd}
FIND=${FIND:-find}

for tool in hyperfine "$FFIND" "$FD" "$FIND"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: '$tool' not found (set FFIND/FD/FIND to override)" >&2
    exit 1
  fi
done

echo "ffind: $("$FFIND" --version)"
echo "fd:    $("$FD" --version | head -n 1)"
echo "find:  $("$FIND" --version | head -n 1)"

if [ -d /usr/share ]; then
  echo
  echo "== substring search in /usr/share =="
  hyperfine --warmup 3 --shell bash \
    "$FFIND python /usr/share" \
    "$FD python /usr/share" \
    "$FIND /usr/share -iname '*python*'"
fi

if [ -d /usr/include ]; then
  echo
  echo "== extension search in /usr/include =="
  hyperfine --warmup 3 --shell bash \
    "$FFIND -e h /usr/include" \
    "$FD -e h '' /usr/include" \
    "$FIND /usr/include -name '*.h'"
fi
