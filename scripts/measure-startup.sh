#!/usr/bin/env bash
# Measure `gset version` startup latency (median + p90, milliseconds).
# This is the single highest-value benchmark for a per-invocation CLI.
set -euo pipefail

BINARY="${1:-./target/release/gset}"
ITERS="${ITERS:-200}"
PROFILE="${PROFILE:-release}"

if [[ ! -x "$BINARY" ]]; then
    cargo build "--$PROFILE" -p gset-cli >/dev/null
    BINARY="./target/release/gset"
    if [[ "$PROFILE" == "debug" ]]; then BINARY="./target/debug/gset"; fi
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
for ((i=0;i<ITERS;i++)); do
    t0=$(date +%s%N)
    "$BINARY" version >/dev/null
    t1=$(date +%s%N)
    echo $(( (t1-t0)/1000000 ))
done > "$tmp"

sort -n "$tmp" | awk -v iters="$ITERS" '
{ a[NR]=$1 }
END {
    mid=int((iters+1)/2);
    p90=int(0.9*iters);
    if (p90<1) p90=1;
    printf "iters=%d median_ms=%d p90_ms=%d min_ms=%d max_ms=%d\n", iters, a[mid], a[p90], a[1], a[iters];
}'
