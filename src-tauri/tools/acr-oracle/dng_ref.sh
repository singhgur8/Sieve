#!/bin/bash
# Converts RAWs (read in place, read-only) to DNG with a full-size ACR-rendered preview.
# Usage: dng_ref.sh <outdir> <raw>...
out="$1"; shift
mkdir -p "$out"
exec "/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter" -c -p2 -d "$out" "$@"
