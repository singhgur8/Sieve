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
| `open_closed_eye.onnx` | Eye-state CNN (OpenVINO OMZ `open-closed-eye-0001`, Apache-2.0) | 46,164 B | `4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593` |
| `face_landmarks_detector_1x3x256x256.onnx` | MediaPipe FaceMesh V2, 478 3D landmarks (Apache-2.0; PINTO ONNX export) for head pose | 4,955,225 B | `70fe4e14169ca084b03b8103077a4051296e07939a19c1fdfd1f18b3792b4048` |

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

The engine (`ml/metrics.rs`) uses the 3-vertical variant per eye; blink = the *more-open* eye below
`blinkEar` (calibrated 0.15). Open eyes measure 0.20-0.35, squints/laughs 0.15-0.20, downcast gaze
0.10-0.16, closed < 0.12.

**Mouth** (verified on a frontal sample crop): corners 52 (image-left) and 61 (image-right); inner upper
lip 62, inner lower lip 60. `mouth_open = |62-60| / |52-61|` (laughing >= ~0.25). Points 52-71 are the
mouth, 72-86 the nose, 0-32 the jaw contour.

## `open_closed_eye.onnx` — eye-state CNN (blink round 2)

OpenVINO Open Model Zoo `open-closed-eye-0001` (Apache-2.0, 46 KB, MRL eye dataset). Source:
`https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx`
(OMZ `model.yml` SHA-384 `2615bce5...cba27` matches). Input `input.1` `[1,3,32,32]`, `(x - 127) / 255`;
we feed grey (MRL is IR) replicated to 3 channels, crop = 2.2 x eye width around the eye corners in the
upright landmark crop. Output `[1,2,1,1]` softmax. **The OMZ docs say `[open, closed]`, but on our
previews index 1 is ~1.0 for clearly open eyes**, so index 1 = open. Caveats: it flips between 0 and 1 as
the crop size changes and calls a lowered gaze "closed"; used only as one of several agreeing signals.

## `face_landmarks_detector_1x3x256x256.onnx` — MediaPipe FaceMesh V2 (blink round 2)

478 3D landmarks incl. iris (Apache-2.0, MediaPipe Face Landmarker v2), ONNX export from PINTO_model_zoo
#410 (`resources.tar.gz`, file extracted and checksum-verified by the fetch script). Input `input_12`
`[1,3,256,256]` RGB in `[0,1]`, crop = upright eye-aligned face box x1.5 (same as the 106-pt model).
Output 0 `Identity` `[1,1,1,1434]` = (x, y, z) in crop pixels. Used for **3D head pose**: rigid fit
(Horn's quaternion method) of the 468 vertices to MediaPipe's canonical face model
(`ml/canonical_face.rs`, Apache-2.0) — pitch > 15° (face turned down) makes closed-looking eyes
"undetermined". CoreML (MLProgram) fails to compile this export ("Required param 'pad' is missing"), so
it runs on CPU, and only for frontal faces whose EAR < 0.3.

Tried and rejected for the closed-vs-downcast problem (no separation on the labeled faces): lid-arc /
brow / lower-lid geometry from the 106-pt model, FaceMesh iris position, MediaPipe Blendshape V2
(`eyeBlink*` vs `eyeLookDown*`, driven by the same landmarks), a 5-keypoint pitch proxy.

## Rust integration (measured)

`ml/models.rs` builds the detector and 106-pt sessions with `ort` 2.0.0-rc.13 (ORT 1.28 static),
CoreML MLProgram + `with_static_input_shapes(true)` + the dimension overrides above (both run on
CoreML); the eye CNN and FaceMesh run on CPU. Full per-image analysis of a 2048 px preview (M3 Max,
release, 396 sample previews): 43 ms single-thread, 13.7 ms/image throughput with the 4-thread pool.
(Round 1 without eye CNN / FaceMesh: 27.7 / 9.9 ms; CPU EP fallback for everything ~86 ms/image.)

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
