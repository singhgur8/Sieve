#!/bin/bash
# Sieve vs ACR per tone variant for one frame (after variants.sh).
H="$(cd "$(dirname "$0")" && pwd)"
raw="$1"
stem=$(basename "${raw%.*}")
V="$H/variants/$stem"
echo "$raw" > "$H/one.txt"
cd "$H/../.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_INCREMENTAL=0 cargo build -q --release --example parity_eval || exit 1
while read -r name ov; do
  mkdir -p "$V/ref_$name"
  cp "$V/$name.jpg" "$V/ref_$name/$stem.ref.jpg"
  line=$(PARITY_OVERRIDE="$ov" SIEVE_LR_REFERENCE="$V/ref_$name" ./target/release/examples/parity_eval --list "$H/one.txt" --out "$V/sieve_$name" | grep -o "dE2000 mean [0-9.]*")
  echo "$name [$ov]: $line"
done <<EOF
asis
S0 shadows=0
B0 blacks=0
C0 contrast=0
H0 highlights=0
W0 whites=0
tone0 shadows=0,blacks=0,contrast=0,highlights=0,whites=0
tone0curve0 shadows=0,blacks=0,contrast=0,highlights=0,whites=0,curve0=1
EOF
