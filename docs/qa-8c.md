# Phase 8c QA gate — PASS (2026-10-03)

Branch `phase-8c-feedback` @ 33c41fd (code) / ab56f46 (docs). Linux cloud container (no user RAWs, no CoreML, no macOS bundle),
run by qa-engineer; written up by the orchestrator from its report.

## Baseline gate
| Check | Result |
|---|---|
| `cargo test` | 479 passed, 0 failed, 25 ignored |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo fmt --check` | clean |
| `pnpm build` | OK |
| Bindings in sync (`cargo test bindings`, clean `git status`) | OK |
| Playwright (mock backend) | 318 / 319; the one failure `fixes8b.spec.ts:174` (mask overlay fade, Phase 8b test) passes 3/3 + 8/8 on an idle machine and fails under CPU contention; product timing verified correct (overlay hides at 555–715 ms in 20/20 probes under load) → test hardening routed to frontend-dev |

Pre-8c baseline on this machine: 275 / 279 (masks gradient drags + slider timing, all green in the gate run).

## Acceptance per task
| Task | Result | Evidence |
|---|---|---|
| Contract v18 | PASS | migration 0016 (`pick_origin`, `reasons_json`, keeper `mode`, facet indexes); `metadata_filters_one_by_one`, `filter_counts_honour_metadata`, `metadata_facets_cascade_and_count`, `pick_origin_filter`, `ipc::activity::tests::*`; ipc-changelog v18 / v18.1 |
| Culling shortcuts | PASS | `lr-keys.spec.ts` 5/5 (Grid, Loupe, Compare, Develop, cheat sheet); Space-before-decode zoom bug fixed (8cded95), 40/40 repeats |
| Arrow-key glitch | PASS | `nav-glitch.spec.ts`: previous-photo frames 0 (was 43 / 126 in Loupe), animation frames 0, list / count re-queries 0; Loupe main-thread ≈ 131 → ≈ 80–95 ms per key, Develop ≈ 306 → ≈ 205–297 ms (headless software rendering, loaded machine) |
| XMP Lightroom | PASS | `xmp::tests::lr_flags::*` 7/7; exiftool on Sieve-written sidecars: pick → `XMP-xmpDM:Pick 1, Good true`; reject → `Pick -1, Good false, Rating 3` (stars kept); unflagged → `Pick 0`, label `Blue` kept |
| "database is locked" | PASS | `xmp::tests::lock::explicit_save_during_auto_sync_and_rating_writes_is_not_locked`: 2,400 written / 0 failed (before the fix: 287 written, 2,113 "database is locked"); UI writes 1,449 ok / 0 failed; `catalog_lock_beyond_timeout_is_one_error` |
| Suggestion reasons | PASS | 13 reason tests in `ml::scoring` / `ml::bursts` incl. `every_non_pick_and_every_tag_has_a_reason` (~23k synthetic frames) |
| Culling clarity | PASS | `culling-clarity.spec.ts` 10/10, `cull-origin.spec.ts` 2/2 |
| Metadata filters | PASS | `phase8c-ui.spec.ts` (extension filter, facets, ranges) + Rust filter tests |
| Activity indicator | PASS | `phase8c-ui.spec.ts`, `ux8c-fixes.spec.ts` (one stack, no overlap), `ux8c-recheck.spec.ts` (behind modals) |
| Help & FAQ | PASS | `phase8c-ui.spec.ts`, copy checks in `ux8c-fixes` / `ux8c-recheck` |
| UX review | PASS | `docs/ux-review-8c.md` Re-check 2: open P0 0 / P1 0 (P2 R2-1..3 optional) |

## Verify on the Mac
1. Lightroom Classic 13.2+: pick / reject / stars / colour label / keywords appear after Metadata → Read Metadata from Files (photos already in a catalog) and on a fresh import.
2. Lightroom write-back (Cmd+S or "Automatically write changes into XMP") is picked up by Sieve on project open / window focus.
3. Arrow-key feel in Loupe and Develop with two filters (no blank or previous-photo flash with real previews).
4. CoreML analysis reasons on real ARW / RAF / CR3 (EV estimates, burst wording).
5. Cmd+S over ~2,400 real photos with auto-sync on: no "database is locked".
6. `pnpm tauri build` + `scripts/bundle-smoke.sh` (the Cargo.toml now enables CoreML via a macOS-only target dependency — confirm the bundle still uses CoreML).
7. Z / X / P / U / Space with the macOS keyboard, Caps Lock auto-advance; F1 (Fn) and Cmd+? for Help.
