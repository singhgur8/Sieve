#!/bin/bash
# Converts every RAW listed in $2 (one path per line, read in place) into DNGs in $1 and
# extracts each DNG's full-size ACR-rendered preview as <stem>.ref.jpg.
out="$1"; list="$2"
mkdir -p "$out"
conv="/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter"
while IFS= read -r raw; do
  [ -z "$raw" ] && continue
  stem=$(basename "${raw%.*}")
  if [ ! -f "$out/$stem.ref.jpg" ]; then
    "$conv" -c -p2 -d "$out" "$raw" >/dev/null 2>&1
    exiftool -b -JpgFromRaw "$out/$stem.dng" > "$out/$stem.ref.jpg" 2>/dev/null
  fi
  echo "$stem $(stat -f %z "$out/$stem.ref.jpg")"
done < "$list"
