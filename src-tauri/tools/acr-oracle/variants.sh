#!/bin/bash
# ACR renders of one real frame with tone-slider variants (settings edited inside a DNG copy).
# Usage: variants.sh <raw>   (read in place; outputs under test-data/variants/<stem>/)
set -e
H="$(cd "$(dirname "$0")" && pwd)"
raw="$1"
stem=$(basename "${raw%.*}")
out="$H/variants/$stem"
mkdir -p "$out/base"
conv="/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
[ -f "$out/base/$stem.dng" ] || "$conv" -c -p0 -d "$out/base" "$raw" >/dev/null 2>&1
render() {
  name="$1"; shift
  [ -f "$out/$name.jpg" ] && return
  mkdir -p "$out/in_$name" "$out/out_$name"
  cp "$out/base/$stem.dng" "$out/in_$name/$stem.dng"
  if [ $# -gt 0 ]; then exiftool -m -q -overwrite_original "$@" "$out/in_$name/$stem.dng"; fi
  "$conv" -c -p2 -d "$out/out_$name" "$out/in_$name/$stem.dng" >/dev/null 2>&1
  exiftool -b -JpgFromRaw "$out/out_$name/$stem.dng" > "$out/$name.jpg"
  rm -rf "$out/in_$name" "$out/out_$name"
}
render asis
render S0 -XMP-crs:Shadows2012=0
render B0 -XMP-crs:Blacks2012=0
render C0 -XMP-crs:Contrast2012=0
render H0 -XMP-crs:Highlights2012=0
render W0 -XMP-crs:Whites2012=0
render tone0 -XMP-crs:Shadows2012=0 -XMP-crs:Blacks2012=0 -XMP-crs:Contrast2012=0 -XMP-crs:Highlights2012=0 -XMP-crs:Whites2012=0
render tone0curve0 -XMP-crs:Shadows2012=0 -XMP-crs:Blacks2012=0 -XMP-crs:Contrast2012=0 -XMP-crs:Highlights2012=0 -XMP-crs:Whites2012=0 -XMP-crs:ParametricShadows=0 -XMP-crs:ParametricDarks=0 -XMP-crs:ParametricLights=0 -XMP-crs:ParametricHighlights=0 "-XMP-crs:ToneCurvePV2012=0, 0" "-XMP-crs:ToneCurvePV2012+=255, 255"
ls "$out"
