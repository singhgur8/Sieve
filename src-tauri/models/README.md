# ONNX models

The model files are **not committed**. Fetch them with:

```sh
scripts/fetch-models.sh          # idempotent; verifies SHA-256 against checksums.sha256
scripts/fetch-models.sh --force  # re-download everything
```

The script exits non-zero (and deletes the partial download) on any checksum mismatch.

| File | Purpose | Size | SHA-256 |
|---|---|---|---|
| `det_10g.onnx` | SCRFD-10GF face detector with 5-point keypoints (insightface `buffalo_l`) | 16,923,827 B | `5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91` |
| `2d106det.onnx` | 106-point 2D face landmarks (insightface `buffalo_l`), used for EAR blink detection | 5,030,888 B | `f001b856447c413801ef5c42091ed0cd516fcd21f2d6b79635b1e733a7109dbf` |

Source: HuggingFace mirror `public-data/insightface`, pinned to revision
`33c1063c49c785b7652d3fd529f86fa4f149392b`:

- `https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l/det_10g.onnx`
- `https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l/2d106det.onnx`

These are the files of the official insightface v0.7 `buffalo_l` pack (not byte-compared against the zip)
(upstream zip: `https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip`,
~280 MB, which also contains the recognition/gender models we do not need).

## License

The insightface pretrained models are released **for non-commercial research purposes only**
(see https://github.com/deepinsight/insightface#license). Accepted for personal use; see
`docs/decisions.md` (2026-09-29). They must be replaced with permissively licensed models before any
commercial distribution. The insightface *code* is MIT; the restriction is on the weights.

---

## `det_10g.onnx` — SCRFD face detector

ONNX opset 11, IR 6.

**Input**

| Name | Shape | Type |
|---|---|---|
| `input.1` | `[1, 3, H, W]` (H, W are dynamic, both named `"?"`) | float32 |

**Preprocessing** (matches insightface `SCRFD.detect`):

1. Letterbox: scale the image so the longer side equals the input size (e.g. 640), keep aspect ratio,
   paste at the **top-left** of a zero-filled `S x S` canvas. Remember `scale = S / max(w, h)`.
2. **RGB** channel order (insightface uses `cv2.dnn.blobFromImage(..., swapRB=True)` on BGR input).
3. `x = (pixel - 127.5) / 128.0`, layout NCHW.

H and W must be multiples of 32. The output metadata declares static 640x640 shapes, but the graph
computes them dynamically (CPU EP works at any size; see CoreML note below).

**Outputs** (9 tensors; order is score x3, bbox x3, kps x3, strides 8/16/32). Counts shown for 640x640;
in general `N_s = (H/s) * (W/s) * 2` (2 anchors per location):

| Index | Name | Shape @640 | Meaning |
|---|---|---|---|
| 0 | `448` | `[12800, 1]` | score, stride 8 (already sigmoid, 0..1) |
| 1 | `471` | `[3200, 1]` | score, stride 16 |
| 2 | `494` | `[800, 1]` | score, stride 32 |
| 3 | `451` | `[12800, 4]` | bbox distances, stride 8 |
| 4 | `474` | `[3200, 4]` | bbox distances, stride 16 |
| 5 | `497` | `[800, 4]` | bbox distances, stride 32 |
| 6 | `454` | `[12800, 10]` | 5 keypoints (x,y offsets), stride 8 |
| 7 | `477` | `[3200, 10]` | keypoints, stride 16 |
| 8 | `500` | `[800, 10]` | keypoints, stride 32 |

Note: there is no batch dimension on the outputs; run one image per call.

**Decoding** (per stride `s`, rows ordered row-major over the `(H/s) x (W/s)` grid, each anchor
center repeated twice):

```
anchor (cx, cy) = (col * s, row * s)
box   = [cx - d0*s, cy - d1*s, cx + d2*s, cy + d3*s]      // d = bbox row
kp_i  = (cx + k[2i]*s, cy + k[2i+1]*s), i = 0..4          // k = kps row
```

Keep `score >= 0.5`, then NMS with IoU 0.4, then divide coordinates by `scale` to map back to the
source image. Keypoint order: left eye, right eye, nose, left mouth corner, right mouth corner (image
left/right; i.e. subject's right eye is index 0 for a frontal face).

## `2d106det.onnx` — 106-point landmarks

ONNX opset 12, IR 7.

**Input**

| Name | Shape | Type |
|---|---|---|
| `data` | `[N, 3, 192, 192]` (batch dim named `"None"`) | float32 |

**Preprocessing** (matches insightface `Landmark.get`):

1. Crop from the source image with a similarity transform: center = face-box center,
   scale = `192 / (max(box_w, box_h) * 1.5)`, output 192x192.
2. **Roll alignment (important):** rotate the crop by the eye-line angle
   `atan2(kp1.y - kp0.y, kp1.x - kp0.x)` from the SCRFD keypoints so the eyes are horizontal.
   Many of our previews are un-rotated portrait frames (EXIF orientation 8), so faces arrive at 90°;
   SCRFD still finds them but the landmark model produces garbage unless the crop is upright.
   (Alternatively rotate the whole preview by EXIF orientation first.)
3. **RGB**, raw 0..255 float, NCHW. **No mean/std in preprocessing**: the graph starts with
   `Sub 127.5` and `Mul 0.0078125`, so normalization is built in.

**Output**

| Name | Shape | Meaning |
|---|---|---|
| `fc1` | `[N, 212]` | 106 (x, y) pairs in `[-1, 1]` crop coordinates |

Decode: `p = (out + 1) * 96` gives pixel coordinates in the 192x192 crop; apply the inverse of the
crop transform to map to the source image.

**Eye indices** (verified visually on sample previews). The two eyes have the same layout offset by 54:

| Role | Eye A (image-left) | Eye B (image-right) |
|---|---|---|
| outer/inner corners (p1, p4) | 35, 39 | 89, 93 |
| upper lid (p2, mid, p3) | 41, 40, 42 | 95, 94, 96 |
| lower lid (p6, mid, p5) | 36, 33, 37 | 90, 87, 91 |
| pupil center (duplicated) | 34, 38 | 88, 92 |

Eye Aspect Ratio (Soukupova & Cech, 6-point form):

```
EAR = (|p2 - p6| + |p3 - p5|) / (2 * |p1 - p4|)
    = (|41-36| + |42-37|) / (2 * |35-39|)    // eye A; add 54 to each index for eye B
```

A 3-vertical variant adding `|40-33|` (and dividing by 3) is slightly more stable. On an open eye in the
samples EAR is about 0.24; thresholds must be calibrated on `test-data/labels.json`.

---

## Execution providers and performance (Apple M3 Max, onnxruntime 1.30, Python)

| Model | EP | Input | Time |
|---|---|---|---|
| det_10g | CPU | 320 / 480 / 640 | 12.5 / 24 / 42 ms |
| det_10g | CoreML, dynamic input (no override) | 640 | 23 ms (fails at 320/480, see below) |
| det_10g | CoreML MLProgram + dimension override | 320 / 480 / 640 | **2.6 / 3.4 / 4.4 ms** |
| 2d106det | CPU / CoreML | 192 | 1.1 / 0.4 ms |

**CoreML needs static shapes.** With the dynamic `"?"` H/W, CoreML only takes 133 of 153 nodes in 7
partitions, and at any size other than 640 it errors with
`CoreML static output shape ... and inferred shape ... have different ranks`. Fix: pin the free
dimension when building the session. Both H and W share the name `"?"`, so one override sets both
(square inputs only):

- Python: `so.add_free_dimension_override_by_name("?", 640)`
- Rust `ort` 2.x: `Session::builder()?.with_dimension_override("?", 640)?`
  and for the landmark model `with_dimension_override("None", 1)`.

CoreML outputs matched CPU to <1e-4 at all tested sizes. Recommended: MLProgram model format,
detector at 640 (4.4 ms) for small faces in wide wedding shots, CPU EP as fallback.
