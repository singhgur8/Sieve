# ONNX models

The model files are **not committed**. Fetch them with:

```sh
scripts/fetch-models.sh          # idempotent; verifies SHA-256 against checksums.sha256
scripts/fetch-models.sh --force  # re-download everything
```

The script exits non-zero (and deletes the partial download) on any checksum mismatch.

The table below covers the culling models. The AI-mask models (`birefnet_lite.onnx`, `skyseg.onnx`,
`yolox_m.onnx`, `efficientsam_ti_*.onnx`, `selfie_multiclass_256x256.onnx`; all MIT / Apache-2.0) are
documented in "Segmentation models" at the end of this file.

| File | Purpose | Size | SHA-256 |
|---|---|---|---|
| `det_10g.onnx` | SCRFD-10GF face detector with 5-point keypoints (insightface `buffalo_l`) | 16,923,827 B | `5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91` |
| `2d106det.onnx` | 106-point 2D face landmarks (insightface `buffalo_l`), used for EAR blink detection | 5,030,888 B | `f001b856447c413801ef5c42091ed0cd516fcd21f2d6b79635b1e733a7109dbf` |
| `open_closed_eye.onnx` | Eye-state CNN (OpenVINO OMZ `open-closed-eye-0001`, Apache-2.0) | 46,164 B | `4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593` |
| `face_landmarks_detector_1x3x256x256.onnx` | MediaPipe FaceMesh V2, 478 3D landmarks (Apache-2.0; PINTO ONNX export) for head pose | 4,955,225 B | `70fe4e14169ca084b03b8103077a4051296e07939a19c1fdfd1f18b3792b4048` |
| `w600k_r50.onnx` | ArcFace R50 face embedding, 512-d (insightface `buffalo_l`, WebFace600K), face identity for target culling | 174,383,860 B | `4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43` |

Source: HuggingFace mirror `public-data/insightface`, pinned to revision
`33c1063c49c785b7652d3fd529f86fa4f149392b`:

- `https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l/det_10g.onnx`
- `https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l/2d106det.onnx`
- `https://huggingface.co/public-data/insightface/resolve/33c1063c49c785b7652d3fd529f86fa4f149392b/models/buffalo_l/w600k_r50.onnx`

These are the files of the official insightface v0.7 `buffalo_l` pack
(upstream zip: `https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip`, 288,621,354 B,
SHA-256 `80ffe37d8a5940d59a7384c201a2a38d4741f2f3c51eef46ebb28218a7b0ca2f`, which also contains the 3D-landmark and
gender/age models we do not need). Byte-compared 2026-10-10: `det_10g.onnx` and `2d106det.onnx` in the zip have the
checksums above (= the mirror's); `w600k_r50.onnx` was taken from the zip (the mirror was unreachable from the cloud
container that day), so its checksum is the zip member's.

## License

The insightface pretrained models are released **for non-commercial research purposes only**
(see https://github.com/deepinsight/insightface#license). Accepted for personal use; see
`docs/decisions.md` (2026-09-29). They must be replaced with permissively licensed models before any
commercial distribution. The insightface *code* is MIT; the restriction is on the weights. This covers
`w600k_r50.onnx` too (same pack, same terms; decisions.md 2026-10-10).

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

## `w600k_r50.onnx` — ArcFace R50 face embedding (face identity, Phase 9)

insightface `buffalo_l` recognition model: IResNet-50 trained with ArcFace loss on WebFace600K; ONNX opset 11.
Used by `ml::identity` (people clustering for target culling), not by the per-image analysis.

| | Name | Shape | Type |
|---|---|---|---|
| Input | `input.1` | `[N, 3, 112, 112]` (batch dim named `"None"`, pinned to 1) | float32 |
| Output | `683` | `[N, 512]` | float32 (not normalised; we L2-normalise) |

**Preprocessing** (insightface `ArcFaceONNX.get` + `face_align.norm_crop`):

1. Similarity transform (rotation + uniform scale + translation, least squares / Umeyama) from the 5 SCRFD keypoints
   to the ArcFace template in the 112x112 crop:
   `(38.2946, 51.6963) (73.5318, 51.5014) (56.0252, 71.7366) (41.5493, 92.3655) (70.7299, 92.2041)`
   (left eye, right eye, nose, left / right mouth corner; image left / right). Bilinear warp, outside = 0.
   Faces larger than ~2x the crop are box-downsampled first (no aliasing from the bilinear warp).
2. **RGB**, `x = (pixel - 127.5) / 127.5`, NCHW. Cosine similarity of L2-normalised outputs.

The analysis stores only the eye centres, so `ml::identity` re-runs SCRFD (640 px, same as the analysis) on the
2048 px preview for the keypoints and matches faces by box IoU >= 0.45.

**Accuracy** (`cargo test --lib ml::identity::tests::real -- --ignored --nocapture`, 10 LFW identities x 10 photos,
downloaded for the test and deleted afterwards): same-person cosine p1 0.43 / p5 0.58 / median 0.70; different people
median 0.01 / p99 0.16 / max 0.22. Clustering purity 1.000 for assignment thresholds 0.25-0.55 (0 identities split,
0 clusters merged); at 0.55 one face is left unassigned. With the same 100 photos added at 1/3 size (~33 px faces,
weak faces that can only join): purity 1.000, none unassigned. Near-duplicates (JPEG q40, +25 % brightness,
12 px shift, mirror, half size) stay at cosine >= 0.95.

**Speed**: Linux cloud VM (4 vCPU Xeon 2.1 GHz, CPU EP, 1 intra-op thread per session): 265-270 ms per face,
SCRFD at 640 ~500 ms per 2048 px preview (debug build; ORT itself is optimised). On Apple Silicon both run on
CoreML (MLProgram, `"None"` pinned to 1); expected a few ms each, not measured yet (no Mac in this container).
Shoot-size backfill (`tests::real::end_to_end_main_pair_and_timing`, `SIEVE_E2E_PHOTOS=2500`): 2,500 synthetic
2048 px previews (LFW composites, 5,449 faces, 5,394 embedded) took 1,082 s on that VM with 4 worker threads
(433 ms/photo, 201 ms/face wall); single-thread per photo: decode 46 ms, SCRFD + resize 523 ms, ~265 ms per face.
Purity 1.000 (4,166 labelled faces), the couple found as the main pair, the parent asked about; a re-run with nothing
to embed takes 1.9 s (clustering 5,394 faces, debug build). Clustering alone at 6,000 faces / 300 people + 1,500
one-off guests (512-d synthetic): 107 s in a debug build (leader clustering is O(faces x clusters); release is
several times faster - re-measure on the Mac).

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

---

# Segmentation models (AI masks, Phase 7c prep)

Engine: `src-tauri/src/ml/segment.rs` (`SegmentEngine`), wrapped as the v10 `SegmentModel`s in
`ml/segment_models.rs` and served by `ml::masking::Segmenter` (`compute_ai_mask`, `detect_people`,
`get_mask_capabilities`). Sieve stores each model's *unrefined* output (+ region) and refines edges at render time
(`ml/refine.rs`, below). Evaluated by `cargo run --release --example segment_eval` (overlays in
`test-data/segment-check/`) and by the ignored test `ml::masking::tests::real::real_masks_over_samples` (overlays
in `test-data/segment-check/v2/`). All models are fetched by `scripts/fetch-models.sh` (pinned revisions,
SHA-256 in `checksums.sha256`); about 560 MB in total.

| Mask | File | Model, license | Size | Input | EP | ms (M3 Max, 2048 px preview) |
|---|---|---|---|---|---|---|
| Subject / Background | `birefnet_lite.onnx` | BiRefNet_lite (Swin-T), **MIT** (onnx-community export) | 224 MB | `input_image` `[1,3,1024,1024]` | CPU (CoreML fails to compile) | 2,840 infer + 110 refine = **2,950** |
| Sky | `skyseg.onnx` | U2-Net sky segmentation (xiongzhu666), **MIT** | 176 MB | `input.1` `[1,3,320,320]` | CPU (CoreML runs but is slower: 218 vs 188 ms) | 200 infer + 120 refine = **320** |
| Person boxes | `yolox_m.onnx` | YOLOX-m COCO (Megvii official release), **Apache-2.0** | 101 MB | `images` `[1,3,640,640]` | **CoreML** (model 9.5 ms; CPU 117) | 26 (CPU 127) |
| Person instances | `efficientsam_ti_encoder.onnx`, `efficientsam_ti_decoder.onnx` | EfficientSAM-Ti (official HF repo), **Apache-2.0** | 25 + 17 MB | encoder `[1,3,1024,1024]`; decoder prompts | encoder **CoreML** (CPU 377), decoder CPU | encode 108 + decode ~30 for all people |
| Hair / face skin / body skin / clothes | `selfie_multiclass_256x256.onnx` | MediaPipe selfie multiclass, **Apache-2.0** (Google weights; ONNX conversion by senty-au, outputs identical to our own tf2onnx conversion of the TFLite file) | 16 MB | `input_29` `[1,256,256,3]` NHWC | CPU (CoreML: MLProgram parse error) | ~20 per crop; all parts incl. refinement ~750 |
| Eyes (sclera) / iris / brows / lips / teeth | existing `face_landmarks_detector_1x3x256x256.onnx` + `det_10g.onnx` | MediaPipe FaceMesh V2 (Apache-2.0) polygons; SCRFD faces (insightface, **non-commercial**, already used by culling) | – | – | FaceMesh CPU, SCRFD CoreML | ~16 face detection (2 scales) + a few ms per face |

Per 2048 px image (CoreML where supported, medians over 15 images after warm-up): **subject 2.95 s, sky 0.32 s,
people with all parts 1.08 s** (0.6–2.2 s depending on the number of people; parts are ~70% of it). CPU only:
subject 2.91 s, sky 0.31 s, people 1.43 s. First use: BiRefNet load 1.0 s; SAM encoder CoreML session 2–9 s even
with `coreml_cache` set (the ORT CoreML cache directory is written, but the first session still takes seconds).
Masks are meant to be computed on demand or in the background and cached per image, never per slider frame.
In the app, parts run only for the selected person and only the model groups a request needs (parser for
hair/skin/clothes, FaceMesh for features), and the guide statistics of the render-time refinement are computed
once per render and shared by every matte (all kinds except sky use the same radius).

## Refinement (all masks)

Network outputs are low resolution (256–1024 px). `segment.rs` upsamples every mask with a **fast colour guided
filter** (He et al.): coefficients `a` (3) / `b` are solved at <= 1024 px over the mask's ROI and applied with the
full-resolution RGB as guide, then an S-curve (`gamma`) counters the filter's softening. Radius / eps per kind
(radius as a fraction of the working long edge): subject 0.4% / 1e-4 (BiRefNet is already sharp), sky 1.2% / 1e-4,
person 0.6% / 1e-4, parts 0.8% / 5e-4. Masks are `Mask { x0, y0, width, height, data: Vec<f32> }` ROIs in image
pixels, so per-person part masks cost only their bounding box. For resolution-independent storage the natural
unit is the low-res network output plus ROI, refined against whatever resolution is being rendered.

## Preprocessing / decoding

- **BiRefNet_lite**: stretch to 1024x1024, RGB `/255`, ImageNet mean/std, NCHW. Output `output_image`
  `[1,1,1024,1024]` = logits (about -27..15) -> sigmoid.
- **skyseg**: stretch to 320x320, RGB `/255`, ImageNet mean/std. 7 outputs (U2-Net side outputs); output 0 is the
  fused map, already a sigmoid. The upstream demo min-max normalises per image; we do not, so frames without sky
  stay at 0 (max 0.0007–0.05 on our indoor / close-up frames).
- **YOLOX-m**: letterbox top-left into 640x640 filled with 114, **BGR**, raw 0..255. Output `[1,8400,85]`: per
  anchor over strides 8/16/32 (row-major grids) `cx = (o0 + gx) * s`, `cy = (o1 + gy) * s`, `w = exp(o2) * s`,
  `h = exp(o3) * s`, score = `o4 * o5` (class 0 = person; sigmoids are in the graph). Score >= 0.35, NMS 0.5.
- **EfficientSAM-Ti**: encoder input RGB in [0,1] (normalisation is inside the graph); we stretch to 1024x1024 so
  CoreML gets static shapes (free dims `batch` / `height` / `width` pinned). Output `image_embeddings`
  `[1,256,64,64]`. Decoder inputs, in this order: `image_embeddings`, `batched_point_coords` `[1,1,N,2]`,
  `batched_point_labels` `[1,1,N]` (1 positive, 0 negative, 2 / 3 box top-left / bottom-right), `orig_im_size`
  int64 `[h, w]`. Point coordinates are in the `orig_im_size` frame (the decoder rescales them by `1024 / size`) and
  masks come out at that size: `output_masks` `[1,1,3,h,w]` logits, `iou_predictions` `[1,1,3]` (take the best).
- **Selfie multiclass**: stretch the crop to 256x256, NHWC RGB in [0,1]. Output `Identity` `[1,256,256,6]` logits
  (apply softmax): 0 background, 1 hair, 2 body skin, 3 face skin, 4 clothes, 5 others (accessories).
- **FaceMesh** contours (MediaPipe `face_mesh_connections`): lips outer / inner rings, eye rings (33.. / 263..),
  brows (70,63,105,66,107 + 55,65,52,53,46 and the mirrored 300..276), iris centre 468 / 473 + 4 ring points each.

## People pipeline

1. YOLOX person boxes + SCRFD faces (two scales: at 640 SCRFD misses faces that fill most of the frame).
2. Faces matched to boxes greedily by distance to the expected head position, so a partner's box that also
   contains the face does not steal it; unmatched faces seed a synthetic box.
3. EfficientSAM per person: box prompt + own face centre (positive) + other people's face centres inside the box
   (negative). Pixels claimed by several instances go to the **smaller** mask (a box around an embrace otherwise
   swallows the partner; the smaller one is usually also in front). Slivers (< 12% of their box) are dropped.
4. Parts: selfie multiclass over near-square, 25%-overlapping tiles of each person (the model expects selfie
   framing; a stretched full-body crop comes back mostly as background) plus a head crop with 4x weight. Face skin
   further than ~1 face size from the face becomes body skin (arms were read as faces); accessories and
   unclassified pixels inside the person count as clothes. Features from FaceMesh polygons: sclera = eye opening
   minus iris disc, lips = outer minus inner ring, teeth = inner-mouth pixels that are bright and unsaturated;
   face skin excludes all features.

## Quality on the samples (overlays viewed)

- **Subject** (BiRefNet_lite): best candidate. Clean hair edges on the bride's updo (MON04829) and on loose hair
  (AZA06603; DSCF5929 at night against fairy lights), whole groups (6 on stage in MON05151, 5 dancers in
  MON05322), white dresses kept. It selects the *salient* subject: blurred foreground shoulders are excluded, as in
  Lightroom; in wide landscapes with small people it returns just the people (2–3% of the frame).
- **Sky**: good on clear / dusk skies with bridge towers, lamp posts and palm fronds (IMG_5595, AZA06579);
  suspension cables are partially included; blurred building edges are slightly stepped (320 px model); near a hazy
  horizon the mask fades to partial. ~0 on indoor, close-up and night frames (the small real sky patch in DSCF5929
  is found).
- **People instances**: separates all 6 in the stage group, the dancers, both women in MON04849, and couples in
  embraces (IMG_5697 including her hand on his cheek; DSCF5929 her hand on his chest). Errors: an arm across the
  partner can go to the partner (AZA06693 pointing arm, parts of the forearm in IMG_5697), a person-shaped picture
  on the wall is detected as a person (MON04849), and extreme close-ups rely on YOLOX alone because SCRFD misses
  the faces (AZA06793, no feature masks there).
- **Parts**: face skin, hair, clothes, lips, brows, eyes and teeth look right on frontal and 3/4 faces. Gaps vs
  Lightroom: beards are classified as face skin, turbans are hair or clothes depending on the crop, profile faces
  get partial face skin, feature polygons need FaceMesh (faces >= 40 px), and internal part boundaries are soft
  (256 px model).

## Candidates rejected

| Candidate | Why |
|---|---|
| RMBG-1.4 / RMBG-2.0 (BRIA) | Non-commercial license; not tested |
| BEN2 Base (MIT) | Hair quality close to BiRefNet, but **missed a whole person** (white-shirt dancer, MON05322); CoreML 1.36 s after a 2 min first compile |
| IS-Net / DIS general-use (Apache-2.0) | Drops white wedding dresses (semi-transparent); CoreML build fails; 585 ms CPU |
| MODNet (Apache-2.0) | Portrait-only, visible halos around hair, no CoreML; 100 ms (possible fast fallback) |
| U2-Net (Apache-2.0) | Predecessor of IS-Net; not better |
| BiRefNet_lite fp16 | Slower on CPU (3.6 s) |
| BiRefNet_lite on CoreML | "Required param 'pad' is missing" (Conv nodes without `pads`); with `pads` patched into the graph: 2.36 s vs 2.85 s CPU, 82 s compile; not worth shipping a patched model |
| SegFormer ADE20K (sky class) | NVIDIA source code license, non-commercial |
| Mask2Former ADE20K (Meta HF) | Weights license "other"; heavy export |
| MaskRCNN-12 (ONNX zoo, Apache) | 28x28 masks; missed the partner in close-ups (DSCF5929, IMG_5697); 0.6–3 s CPU |
| torchvision Mask R-CNN v2 (ONNX zoo, opset 18) | Exported with a fixed 224x224 input; mask pasting baked for that size |
| YOLOv8 / YOLO11-seg | AGPL-3.0 |
| BiSeNet face parsing (zllrunning, yakhyo), jonathandinu/face-parsing (SegFormer-B5) | Code MIT, but trained on CelebAMask-HQ (non-commercial research dataset); the SegFormer one also carries NVIDIA's backbone license |

License notes: BiRefNet, skyseg, EfficientSAM, YOLOX and MediaPipe weights are published under the licenses above by
their authors (GitHub repository licenses checked via the API: ZhengPeng7/BiRefNet MIT,
xiongzhu666/Sky-Segmentation-and-Post-processing MIT, yformer/EfficientSAM Apache-2.0, Megvii YOLOX Apache-2.0,
google-ai-edge/mediapipe Apache-2.0; HF model cards agree). They were trained on public datasets (DIS5K etc.,
COCO, SA-1B, Google's own data) whose terms were not audited beyond that. Re-check Google's selfie-multiclass model
card before commercial distribution. The SCRFD face detector reused here is the insightface **non-commercial**
model (see above) and would need replacing (e.g. the MediaPipe face detector, Apache-2.0) for commercial use.
