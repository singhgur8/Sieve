# Phase 9 QA gate — PASS except real-shoot calibration (2026-10-10)

Branch `phase-9-target-cull`; Rust at df3201f (later commits are frontend + docs only), frontend at be09933.
Linux cloud container, no user photos, no CoreML. Run by qa-engineer; written up by the orchestrator.

## Baseline
| Check | Result |
|---|---|
| `cargo test` | 596 passed, 0 failed, 32 ignored |
| clippy `-D warnings` / fmt | clean / clean |
| Bindings in sync | OK |
| `pnpm build` | OK |
| Playwright (mock) | 478 / 480; the 2 failures are `slider-smooth` p95 (40 / 80 ms renders, ~35 ms vs 20 ms limit) — known container-load failure, branch touches no slider / develop code |

## Acceptance per task
| Task | Result | Evidence |
|---|---|---|
| Contract v20 + v20.1 | PASS | `db::target` 11 tests (apply plan, covered-by after edits, user flags win, swap/add/undo, people ids survive re-runs, piles); `ipc-v20-mock.spec.ts` |
| Face identity | PASS (synthetic); real-model numbers cited | 8 identity tests; LFW subset purity 1.000, same-person cosine median 0.70 vs different-person max 0.22 (decisions 2026-10-10); `cluster_scale` 6,000 faces in 111 s (debug build) |
| Moments and shot types | PASS | 13 `ml::moments` tests incl. labelled shot-type fixtures, back-of-head, detail focus |
| Target selection engine | PASS | 15 rule tests (one+ per user rule), ±10 % target, locked choices, never delivers defects / user rejects; 2,556 frames select in 0.5 s (debug) |
| Target cull UI | PASS | `target-cull.spec.ts` + `ipc-v20-mock.spec.ts` 43/43 on HEAD |
| UX review | PASS (after N3-1 fix 57c7af4) | `docs/ux-review-9.md`: review P0 3 / P1 11 → re-check 1 P1 3 → re-check 2 P0 1 (scroll crash) / P1 1 → re-check 3 P1 1 (N3-1: undo of Apply left "applied" stamp) → fixed in IPC v20.2 `restore_target_apply`; Rust `restore_apply_resets_applied_at`, Playwright N3-1 (both undo paths); orchestrator re-ran target-cull + ipc-v20-mock 48/48, build, clippy |
| Calibrate on the user's shoot | OPEN — waiting for the user's previews + XMP | `target_eval` example builds and prints usage |

## Verify on the Mac
1. CoreML speed of face embeddings (ArcFace R50) and SCRFD re-detection; time for ~2,500 photos.
2. Clustering time on a real wedding in a release build (cloud debug: 6,000 faces in 111 s).
3. App bundle size: `w600k_r50.onnx` adds ~174 MB — `pnpm tauri build` + `scripts/bundle-smoke.sh`.
4. Identity on real wedding faces (veils, profiles, small faces): couple found, sensible "Is this person important?" questions.
5. Real-shoot calibration with `target_eval` at target 800; tune `NEAR_DUP_COUPLE`, `CANDID_MIN_VISIBLE`, `COVER_MIN`, clustering cuts.
6. End-to-end on real photos: Pick the best N → people → pass 1 (moment grid) → pass 2 piles → Apply / Undo → XMP flags in Lightroom.
7. Licence: insightface weights are non-commercial (personal use accepted); replace before any commercial release.
