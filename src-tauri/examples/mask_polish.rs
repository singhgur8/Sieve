//! Mask polish check (Phase 8): Select Sky + luminance / colour range overlays, full frame
//! and at 100 % on a region, through the app's `render_mask_overlay` path.
//!
//! ```text
//! cargo run --release --example mask_polish -- --src DIR_WITH_COPIES --work DIR --out DIR
//!     [--region x,y,w,h] [--edge PX]
//! ```
//! `--src` holds *copies* of RAWs (+ sidecars); the catalog is scratch. `--region` is in the
//! displayed frame (default `0.3,0.35,0.3,0.2`). Writes per image `<stem>.photo.jpg`,
//! `<stem>.zoom.photo.jpg`, `<stem>.<mask>.{full,zoom}.jpg` (grayscale overlays) and prints
//! sizes, coverage and an edge-staircase measure (share of 2x2-constant blocks on mask edges).

use std::path::PathBuf;
use std::time::Instant;

use sieve_lib::db::{self, repo};
use sieve_lib::develop::masks::{render_overlay, MaskCache, MaskCacheConfig};
use sieve_lib::develop::{DevelopCache, DevelopConfig, SourceImage};
use sieve_lib::ipc::masks::{unorient_point, ColorRange, ColorSample, LuminanceRange};
use sieve_lib::ipc::types::{
    AiMask, AiMaskRequest, AiTarget, ImportOptions, LocalAdjustments, MaskBlendMode, MaskComponent, MaskGroup,
    MaskOverlayOptions, MaskOverlayTarget, MaskShape, NormPoint, NormRect, ParametricAdjustments, RenderSlot,
};
use sieve_lib::lut::LutLibrary;
use sieve_lib::ml::masking::{Segmenter, SegmenterConfig};
use sieve_lib::raw::turbo;

fn group(id: u32, shape: MaskShape) -> MaskGroup {
    MaskGroup {
        id: format!("{id:032X}"),
        name: format!("m{id}"),
        active: true,
        amount: 1.0,
        adjustments: LocalAdjustments { exposure: 1.0, ..LocalAdjustments::default() },
        components: vec![MaskComponent {
            id: format!("{:032X}", id + 100),
            name: String::new(),
            active: true,
            mode: MaskBlendMode::Add,
            inverted: false,
            opacity: 1.0,
            shape,
        }],
    }
}

/// Share of mask-edge pixels (0.1 < v < 0.9 neighbourhood change) sitting in 2x2 blocks of
/// identical values: ~0.25 for smooth edges, -> 1 for nearest-upsampled staircases.
fn staircase(px: &[u8], w: usize, h: usize) -> f64 {
    let (mut edge, mut blocky) = (0u64, 0u64);
    for y in (0..h.saturating_sub(1)).step_by(2) {
        for x in (0..w.saturating_sub(1)).step_by(2) {
            let q = [px[y * w + x], px[y * w + x + 1], px[(y + 1) * w + x], px[(y + 1) * w + x + 1]];
            // Edge: this block differs from its right or lower neighbour block.
            let nb = |dx: usize, dy: usize| {
                let (xx, yy) = (x + dx, y + dy);
                (xx < w && yy < h).then(|| px[yy * w + xx])
            };
            let diff = [nb(2, 0), nb(0, 2)].iter().flatten().any(|v| (i32::from(*v) - i32::from(q[0])).abs() > 20);
            if !diff {
                continue;
            }
            edge += 1;
            if q.iter().all(|v| *v == q[0]) {
                blocky += 1;
            }
        }
    }
    blocky as f64 / edge.max(1) as f64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let src_dir = PathBuf::from(arg("--src").expect("--src"));
    let work = PathBuf::from(arg("--work").expect("--work"));
    let out = PathBuf::from(arg("--out").expect("--out"));
    let edge: u32 = arg("--edge").and_then(|v| v.parse().ok()).unwrap_or(1600);
    let region: Vec<f32> =
        arg("--region").unwrap_or_else(|| "0.3,0.35,0.3,0.2".into()).split(',').map(|v| v.parse().unwrap()).collect();
    let region = NormRect { x: region[0], y: region[1], width: region[2], height: region[3] };
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let catalog = work.join("polish.sqlite");
    let ids: Vec<i64> = {
        let mut conn = db::open(&catalog).unwrap();
        let opts = ImportOptions { recursive: false, include_non_raw: false, pair_jpeg_with_raw: true };
        repo::import_folder(&mut conn, &src_dir, &opts).unwrap();
        let rows: Vec<(i64, String)> = {
            let mut st = conn.prepare("SELECT id, path FROM images ORDER BY id").unwrap();
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
            rows
        };
        // Orientation comes from ingest (not run here): read it from the RAW.
        for (id, path) in &rows {
            let p = PathBuf::from(path);
            let f = sieve_lib::raw::format_from_extension(&p).unwrap();
            let mut buf = Vec::new();
            let o = sieve_lib::raw::extract(&p, f, &mut buf).ok().and_then(|x| x.meta.orientation).unwrap_or(1);
            conn.execute("UPDATE images SET orientation = ?1 WHERE id = ?2", (i64::from(o), id)).unwrap();
        }
        rows.into_iter().map(|r| r.0).collect()
    };
    let cache = MaskCache::new(MaskCacheConfig { catalog_path: catalog.clone(), cache_dir: work.join("cache") });
    let models = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));
    let seg = Segmenter::new(SegmenterConfig { models_dir: models, catalog_path: catalog.clone() }, cache.clone());
    let dev = DevelopCache::new(DevelopConfig { cache_bytes: 1 << 30, mask_cache: Some(cache.clone()) });
    let luts = LutLibrary::new(work.join("luts"));
    let conn = db::open(&catalog).unwrap();
    for &id in &ids {
        let e = repo::get_image(&conn, id).unwrap();
        let stem = e.file_name.rsplit_once('.').map_or(e.file_name.clone(), |(s, _)| s.to_owned());
        let src = SourceImage { id, path: PathBuf::from(&e.path), orientation: e.orientation };
        let o = e.orientation.filter(|v| (1..=8).contains(v)).unwrap_or(1);
        let t = Instant::now();
        let sky = seg
            .compute(&src, None, &AiMaskRequest { target: AiTarget::Sky, reference_point: None, force: true })
            .unwrap();
        let sky_ms = t.elapsed().as_millis();
        // Colour sample: top centre of the displayed frame (sky in these frames).
        let sample = unorient_point(NormPoint { x: 0.5, y: 0.06 }, o);
        let groups = vec![
            group(1, MaskShape::Ai(AiMask { target: AiTarget::Sky, reference_point: None, digest: Some(sky.digest) })),
            group(
                2,
                MaskShape::Luminance(LuminanceRange {
                    feather_low: 0.45,
                    low: 0.55,
                    high: 1.0,
                    feather_high: 1.0,
                    smoothness: 50.0,
                }),
            ),
            group(
                3,
                MaskShape::Luminance(LuminanceRange {
                    feather_low: 0.6,
                    low: 0.6,
                    high: 1.0,
                    feather_high: 1.0,
                    smoothness: 0.0,
                }),
            ),
            group(
                4,
                MaskShape::Color(ColorRange {
                    samples: vec![ColorSample { point: sample, area: None, lightroom_model: None }],
                    amount: 50.0,
                }),
            ),
        ];
        let names = ["sky", "lum", "lumhard", "color"];
        let adj = ParametricAdjustments { masks: groups.clone(), ..ParametricAdjustments::defaults_for(e.format) };
        let mut plain = adj.clone();
        plain.masks.clear();
        for (tag, reg, max_edge) in [("full", None, edge), ("zoom", Some(region), 8000)] {
            let px = dev.render_image(&src, &plain, reg, max_edge, &luts).unwrap();
            let name = if tag == "full" { format!("{stem}.photo.jpg") } else { format!("{stem}.zoom.photo.jpg") };
            let jpg = turbo::encode_rgb(&px.image.rgb, px.image.width, px.image.height, 90).unwrap();
            std::fs::write(out.join(name), jpg).unwrap();
            for (g, n) in groups.iter().zip(names) {
                let ticket = dev.ticket(id, RenderSlot::Mask);
                let opts = MaskOverlayOptions { max_edge, region: reg };
                let target = MaskOverlayTarget { group_id: g.id.clone(), component_id: None };
                let t = Instant::now();
                let r = render_overlay(&dev, &cache, ticket, &src, &adj, &target, &opts).unwrap().unwrap();
                let ms = t.elapsed().as_millis();
                let jpeg = dev.encoded(id, RenderSlot::Mask, r.seq).unwrap();
                let gray = turbo::decode_rgb(&jpeg, r.width.max(r.height), 1 << 30)
                    .ok()
                    .map(|d| d.pixels.chunks(3).map(|p| p[0]).collect::<Vec<u8>>());
                let stair = gray.as_ref().map_or(f64::NAN, |g| staircase(g, r.width as usize, r.height as usize));
                std::fs::write(out.join(format!("{stem}.{n}.{tag}.jpg")), jpeg.as_slice()).unwrap();
                println!(
                    "{stem} o{o} {tag} {n}: {}x{} (photo {}x{}) cov {:.3} staircase {stair:.3} | {ms} ms{}",
                    r.width,
                    r.height,
                    px.image.width,
                    px.image.height,
                    r.coverage,
                    if n == "sky" { format!(" (sky compute {sky_ms} ms)") } else { String::new() }
                );
            }
        }
    }
}
