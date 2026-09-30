#!/bin/bash
# eval.sh <list> <variant|-> [parity_eval args...]
# Sieve vs Camera Raw for a settings variant rendered by variants.py (`-` = the original
# references in $P2/ref with the untouched sidecars). Prints per-image dE and the summary.
P2="${P2:-/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/p2}"
B="$(cd "$(dirname "$0")/../.." && pwd)/target/release/examples/parity_eval"
list="$1"; v="$2"; shift 2
if [ "$v" = "-" ]; then
  out="$P2/out/orig"; ref="$P2/ref"
  SIEVE_LR_REFERENCE="$ref" "$B" --list "$list" --out "$out" "$@"
else
  out="$P2/out/$v"; ref="$P2/var/$v"
  SIEVE_XMP_DIR="$ref" SIEVE_LR_REFERENCE="$ref" "$B" --list "$list" --out "$out" "$@"
fi 2>&1 | grep -E "dE2000|==|images, mean" | sed -E 's/: [0-9]+x[0-9]+ base.*\| dE2000/ dE/'
