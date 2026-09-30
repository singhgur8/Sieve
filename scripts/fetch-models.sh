#!/usr/bin/env bash
# Download the ONNX models used by the culling engine and AI masks into src-tauri/models/.
#
# Idempotent: files already present with the expected SHA-256 are skipped.
# Every file is verified against src-tauri/models/checksums.sha256; a mismatch
# aborts with a non-zero exit code and the bad file is removed.
#
# Usage: scripts/fetch-models.sh [--force] [--culling] [--dest DIR]
#   --culling   only the culling models bundled into the app (~27 MB; `pnpm tauri build` runs this)
#   --dest DIR  install into DIR instead of src-tauri/models (e.g. the app's data dir,
#               ~/Library/Application Support/com.sieve.app/models, to pre-seed AI-mask models)
#
# Licenses: the insightface model zoo weights (det_10g, 2d106det) are released for NON-COMMERCIAL
# research use only (see src-tauri/models/README.md and docs/decisions.md).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="$ROOT/src-tauri/models"
CHECKSUMS="$MODELS_DIR/checksums.sha256"
FORCE=0
CULLING_ONLY=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --force) FORCE=1 ;;
    --culling) CULLING_ONLY=1 ;;
    --dest) shift; MODELS_DIR="${1:?--dest needs a directory}" ;;
    *) echo "fetch-models: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
# Bundled with the app (keep in sync with BUNDLED_MODELS in src-tauri/build.rs).
CULLING_MODELS=" det_10g.onnx 2d106det.onnx open_closed_eye.onnx face_landmarks_detector_1x3x256x256.onnx "

# Pinned HuggingFace revision of public-data/insightface (mirror of the
# insightface buffalo_l model pack, v0.7 release).
HF_REPO="https://huggingface.co/public-data/insightface/resolve"
HF_REV="33c1063c49c785b7652d3fd529f86fa4f149392b"

# OpenVINO Open Model Zoo `open-closed-eye-0001` (Apache-2.0, 46 KB eye-state CNN).
# The OMZ model.yml pins SHA-384 2615bce5...cba27 for this exact file.
OMZ_EYE="https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx"

# MediaPipe FaceMesh V2 landmarks (Apache-2.0), ONNX export from PINTO_model_zoo #410
# (shipped only inside an 18 MB tarball; the extracted file is checksum-verified).
PINTO_MESH="https://s3.ap-northeast-2.wasabisys.com/pinto-model-zoo/410_FaceMeshV2/resources.tar.gz"

# --- Segmentation / AI masks (Phase 7c; all permissively licensed, see models/README.md) ---
# Pinned HuggingFace revisions; the YOLOX file is an immutable GitHub release asset.
HF="https://huggingface.co"
# BiRefNet_lite (MIT), onnx-community export: Select Subject / Background, 1024x1024.
SEG_SUBJECT="$HF/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx"
# Sky segmentation (MIT, xiongzhu666/Sky-Segmentation-and-Post-processing), U2-Net 320x320.
SEG_SKY="$HF/JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx"
# YOLOX-m COCO detector (Apache-2.0, Megvii official release): person boxes.
SEG_PERSON="https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_m.onnx"
# EfficientSAM-Ti (Apache-2.0, official HF repo): per-person instance masks from box prompts.
SEG_SAM="$HF/yunyangx/EfficientSAM/resolve/1cf49585c39567bfc49e991ab8eb31f491ad4877"
# MediaPipe selfie multiclass 256x256 (Apache-2.0, Google), ONNX conversion of the unchanged
# TFLite weights (verified identical to a local tf2onnx conversion): hair / face skin / body skin / clothes.
SEG_PARTS="$HF/senty-au/selfie_multiclass_256x256-ONNX/resolve/6db8421a7150ac20558f2c24675078eb3a1a04d0/onnx/model.onnx"

# name|url[|member inside a .tar.gz at url]
MODELS=(
  "det_10g.onnx|$HF_REPO/$HF_REV/models/buffalo_l/det_10g.onnx"
  "2d106det.onnx|$HF_REPO/$HF_REV/models/buffalo_l/2d106det.onnx"
  "open_closed_eye.onnx|$OMZ_EYE"
  "face_landmarks_detector_1x3x256x256.onnx|$PINTO_MESH|face_landmarks_detector_1x3x256x256.onnx"
  "birefnet_lite.onnx|$SEG_SUBJECT"
  "skyseg.onnx|$SEG_SKY"
  "yolox_m.onnx|$SEG_PERSON"
  "efficientsam_ti_encoder.onnx|$SEG_SAM/efficientsam_ti_encoder.onnx"
  "efficientsam_ti_decoder.onnx|$SEG_SAM/efficientsam_ti_decoder.onnx"
  "selfie_multiclass_256x256.onnx|$SEG_PARTS"
)

die() { echo "fetch-models: ERROR: $*" >&2; exit 1; }

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    die "neither shasum nor sha256sum found"
  fi
}

expected_sha() {
  awk -v f="$1" '$2 == f || $2 == "*"f {print $1}' "$CHECKSUMS"
}

command -v curl >/dev/null 2>&1 || die "curl is required"
[[ -f "$CHECKSUMS" ]] || die "missing $CHECKSUMS"
mkdir -p "$MODELS_DIR"

for entry in "${MODELS[@]}"; do
  IFS='|' read -r name url member <<< "$entry"
  if [[ $CULLING_ONLY -eq 1 && "$CULLING_MODELS" != *" $name "* ]]; then
    continue
  fi
  dest="$MODELS_DIR/$name"
  want="$(expected_sha "$name")"
  [[ -n "$want" ]] || die "no checksum for $name in $CHECKSUMS"

  if [[ -f "$dest" && $FORCE -eq 0 ]]; then
    have="$(sha256_of "$dest")"
    if [[ "$have" == "$want" ]]; then
      echo "ok       $name (already present, checksum matches)"
      continue
    fi
    echo "stale    $name (checksum mismatch, re-downloading)"
  fi

  echo "download $name"
  tmp="$dest.part"
  rm -f "$tmp"
  if [[ -n "${member:-}" ]]; then
    work="$(mktemp -d)"
    curl --fail --location --retry 3 --retry-delay 2 --show-error --progress-bar \
      -o "$work/archive.tar.gz" "$url" || { rm -rf "$work"; die "download failed: $url"; }
    tar -xzf "$work/archive.tar.gz" -C "$work" "$member" || { rm -rf "$work"; die "$member not found in $url"; }
    mv -f "$work/$member" "$tmp"
    rm -rf "$work"
  else
    curl --fail --location --retry 3 --retry-delay 2 --show-error --progress-bar \
      -o "$tmp" "$url" || { rm -f "$tmp"; die "download failed: $url"; }
  fi

  have="$(sha256_of "$tmp")"
  if [[ "$have" != "$want" ]]; then
    rm -f "$tmp"
    die "SHA-256 mismatch for $name
  expected: $want
  actual:   $have
  url:      $url"
  fi
  mv -f "$tmp" "$dest"
  echo "ok       $name (verified)"
done

echo "All models present and verified in $MODELS_DIR"
