#!/bin/bash
# Camera Raw render of one RAW with edited develop settings (oracle for real frames).
# Usage: variant.sh <raw> <variant-name> [exiftool tag assignments...]
# Sources are read in place (never written). Work dir: $P2 (default test-data/p2):
#   dng/<stem>.dng                 DNG copy with the sidecar's settings (created once, -p0)
#   var/<variant>/<stem>.ref.jpg   Camera Raw full-size render (DNG Converter preview)
#   var/<variant>/<stem>.xmp       the exact settings rendered (for SIEVE_XMP_DIR)
set -e
P2="${P2:-/Users/gurjotsingh/Documents/GitHub/Sieve/test-data/p2}"
conv="/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
raw="$1"; name="$2"; shift 2
stem=$(basename "${raw%.*}")
out="$P2/var/$name"
mkdir -p "$P2/dng" "$out"
[ -f "$P2/dng/$stem.dng" ] || "$conv" -c -p0 -d "$P2/dng" "$raw" >/dev/null 2>&1
[ -f "$out/$stem.ref.jpg" ] && [ -f "$out/$stem.xmp" ] && exit 0
tmp=$(mktemp -d "$P2/tmp.XXXXXX")
mkdir -p "$tmp/in" "$tmp/out"
cp "$P2/dng/$stem.dng" "$tmp/in/$stem.dng"
if [ $# -gt 0 ]; then exiftool -m -q -overwrite_original "$@" "$tmp/in/$stem.dng"; fi
"$conv" -c -p2 -d "$tmp/out" "$tmp/in/$stem.dng" >/dev/null 2>&1
exiftool -m -b -JpgFromRaw "$tmp/out/$stem.dng" > "$out/$stem.ref.jpg"
exiftool -m -xmp -b "$tmp/in/$stem.dng" > "$out/$stem.xmp"
rm -rf "$tmp"
echo "$name/$stem $(stat -f %z "$out/$stem.ref.jpg")"
