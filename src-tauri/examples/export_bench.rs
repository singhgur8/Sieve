//! Export acceptance bench: runs the real `Exporter` (job queue, memory semaphore, encoders)
//! over a folder of RAWs for each format preset and samples the process memory.
//!
//! ```text
//! cargo run --release --example export_bench -- [src_dir] [out_dir] [--only <name>]
//! ```
//! Defaults: `<repo>/test-data/export-src` and `<repo>/test-data/export-out`. The source
//! folder must be a *copy* (never `~/Pictures`): the bench only reads it, but sidecars there
//! are used for metadata. A scratch catalog is built in `<out_dir>/../export-bench/` (import +
//! the real ingest pipeline for EXIF/orientation), a few images get edits (exposure/tone,
//! custom WB + HSL, a `.cube` LUT), then every preset exports all images into
//! `<out_dir>/<preset>/`. Memory is sampled every 250 ms (`SIEVE_BENCH_SAMPLE_MS`): live Rust
//! heap (a counting global allocator: every pixel buffer of the export path) and resident
//! size (also covers LibRaw's C allocations, but keeps freed pages malloc has not returned);
//! min/max are reported after warmup (the first `MAX_PARALLEL` images), plus the per-preset
//! heap peak and the process's lifetime peak physical footprint. (The *current* physical
//! footprint is not reported: macOS stops charging malloc regions that are freed and reused,
//! so it under-reports steady-state batches.)
//! `SIEVE_BENCH_REPEAT=n` runs the preset list n times (memory drift check).
//! `preview/` gets editor-style renders (half-size decode + preview pipeline) of the edited
//! images for a colour comparison.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use sieve_lib::db::{self, repo};
use sieve_lib::develop::{pipeline, source};
use sieve_lib::export::{ExportConfig, ExportSink, Exporter, MAX_PARALLEL};
use sieve_lib::ingest::{run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{ExportFinished, ExportProgress, ImportProgress, ThumbnailFailed, ThumbnailReady};
use sieve_lib::ipc::types::{
    BitDepth, ChromaSubsampling, CollisionPolicy, ExportColorSpace, ExportDestination, ExportFormat, ExportSettings,
    FileNaming, HslAdjustments, HslChannels, ImportOptions, LutRef, MetadataInclude, MetadataOptions, OutputSharpening,
    ParametricAdjustments, ResizeMode, ResizeOptions, SharpenAmount, SharpenMedia, TiffCompression, WhiteBalance,
};
use sieve_lib::lut::LutLibrary;

/// Counts live Rust heap bytes (LibRaw's own C allocations are not included; RSS covers
/// them). `HEAP_PEAK` is reset per preset.
struct Counting;
static HEAP: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator; only adds atomic bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = HEAP.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            HEAP_PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        HEAP.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            let now = HEAP.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            HEAP_PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                let now = HEAP.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size - layout.size();
                HEAP_PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                HEAP.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[derive(Clone, Copy)]
struct Sample {
    done: u32,
    heap: f64,
    rss: f64,
    #[allow(dead_code)]
    footprint: f64,
}

struct Quiet;
impl IngestSink for Quiet {
    fn ready(&self, _: ThumbnailReady) {}
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("ingest failed: image {}: {}", e.image_id, e.reason);
    }
    fn progress(&self, _: ImportProgress) {}
}

struct Sink {
    done: AtomicU32,
    finished: Mutex<Option<mpsc::Sender<ExportFinished>>>,
}

impl ExportSink for Sink {
    fn progress(&self, e: ExportProgress) {
        self.done.store(e.done, Ordering::SeqCst);
    }
    fn finished(&self, e: ExportFinished) {
        if let Some(tx) = self.finished.lock().unwrap().as_ref() {
            let _ = tx.send(e);
        }
    }
}

/// (current physical footprint, current resident size, lifetime max footprint), MiB.
fn memory_mb() -> (f64, f64, f64) {
    // SAFETY: proc_pid_rusage fills a zeroed rusage_info_v4 we own.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    if rc != 0 {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let mb = |v: u64| v as f64 / (1024.0 * 1024.0);
    (mb(info.ri_phys_footprint), mb(info.ri_resident_size), mb(info.ri_lifetime_max_phys_footprint))
}

fn film_cube() -> String {
    let n = 17;
    let mut s = String::from("TITLE \"Bench warm film\"\nLUT_3D_SIZE 17\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let c = [r, g, b].map(|v| v as f32 / (n - 1) as f32);
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let curve = |x: f32| x + 0.12 * (x - 0.5) * (1.0 - (2.0 * x - 1.0).abs());
                let warm = (l - 0.5) * 0.08;
                let o = [curve(c[0]) + warm, curve(c[1]) + warm * 0.3, curve(c[2]) - warm];
                s.push_str(&format!(
                    "{:.6} {:.6} {:.6}\n",
                    o[0].clamp(0.0, 1.0),
                    o[1].clamp(0.0, 1.0),
                    o[2].clamp(0.0, 1.0)
                ));
            }
        }
    }
    s
}

fn base(out: &Path, format: ExportFormat, cs: ExportColorSpace, mode: ResizeMode) -> ExportSettings {
    ExportSettings {
        format,
        color_space: cs,
        resize: ResizeOptions { mode, dont_enlarge: true, resolution_ppi: 300 },
        sharpening: None,
        naming: FileNaming { template: "{filename}".into(), start_number: 1, collision: CollisionPolicy::Overwrite },
        destination: ExportDestination::Folder { path: out.to_string_lossy().into_owned() },
        subfolder: None,
        metadata: MetadataOptions {
            include: MetadataInclude::All,
            remove_location: false,
            include_keywords: true,
            copyright: None,
            creator: None,
        },
    }
}

fn presets(out: &Path, heic: bool) -> Vec<(&'static str, ExportSettings)> {
    let mut v = Vec::new();
    // 1. Client JPEG: full res, sRGB, q90 4:4:4, all metadata (+ keywords, location kept).
    v.push((
        "jpeg_full_srgb_q90",
        base(
            &out.join("jpeg_full_srgb_q90"),
            ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
            ExportColorSpace::Srgb,
            ResizeMode::None,
        ),
    ));
    // 2. Web: 2048 long edge, Display P3, q80 4:2:0, screen/standard sharpening, copyright only.
    let mut s = base(
        &out.join("jpeg_2048_p3_q80"),
        ExportFormat::Jpeg { quality: 80, chroma_subsampling: ChromaSubsampling::Yuv420 },
        ExportColorSpace::DisplayP3,
        ResizeMode::LongEdge { px: 2048 },
    );
    s.resize.resolution_ppi = 72;
    s.sharpening = Some(OutputSharpening { media: SharpenMedia::Screen, amount: SharpenAmount::Standard });
    s.metadata = MetadataOptions {
        include: MetadataInclude::CopyrightOnly,
        remove_location: true,
        include_keywords: false,
        copyright: Some("(c) 2026 Sieve Bench Studio".into()),
        creator: None,
    };
    v.push(("jpeg_2048_p3_q80", s));
    // 3. Print: TIFF 16-bit Adobe RGB LZW, glossy sharpening, all metadata but location removed.
    let mut s = base(
        &out.join("tiff16_adobe_lzw"),
        ExportFormat::Tiff { bit_depth: BitDepth::Sixteen, compression: TiffCompression::Lzw },
        ExportColorSpace::AdobeRgb,
        ResizeMode::None,
    );
    s.sharpening = Some(OutputSharpening { media: SharpenMedia::Glossy, amount: SharpenAmount::Standard });
    s.metadata.remove_location = true;
    v.push(("tiff16_adobe_lzw", s));
    // 4. PNG 8-bit sRGB, 1600 short edge, no metadata.
    let mut s = base(
        &out.join("png8_srgb_1600"),
        ExportFormat::Png { bit_depth: BitDepth::Eight },
        ExportColorSpace::Srgb,
        ResizeMode::ShortEdge { px: 1600 },
    );
    s.metadata.include = MetadataInclude::None;
    v.push(("png8_srgb_1600", s));
    // 5. WebP q85 2048 sRGB, copyright + contact with creator override.
    let mut s = base(
        &out.join("webp_2048_srgb_q85"),
        ExportFormat::Webp { quality: 85, lossless: false },
        ExportColorSpace::Srgb,
        ResizeMode::LongEdge { px: 2048 },
    );
    s.resize.resolution_ppi = 72;
    s.metadata.include = MetadataInclude::CopyrightAndContact;
    s.metadata.creator = Some("Override Creator".into());
    v.push(("webp_2048_srgb_q85", s));
    if heic {
        // 6. HEIC q80 2048 Display P3.
        let mut s = base(
            &out.join("heic_2048_p3_q80"),
            ExportFormat::Heic { quality: 80 },
            ExportColorSpace::DisplayP3,
            ResizeMode::LongEdge { px: 2048 },
        );
        s.resize.resolution_ppi = 72;
        v.push(("heic_2048_p3_q80", s));
    }
    v
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|r| r.filter_map(Result::ok).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let default_root =
        std::env::var_os("SIEVE_TEST_DATA").map(PathBuf::from).unwrap_or_else(|| repo_root.join("test-data"));
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let only = args.iter().position(|a| a == "--only").map(|i| {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        args.drain(i..(i + 2).min(args.len()));
        v
    });
    let src = args.first().map(PathBuf::from).unwrap_or_else(|| default_root.join("export-src"));
    let out = args.get(1).map(PathBuf::from).unwrap_or_else(|| default_root.join("export-out"));
    let src = src.canonicalize()?;
    if src.starts_with("/Users/gurjotsingh/Pictures") {
        return Err("refusing to use ~/Pictures directly; copy the RAWs into test-data first".into());
    }
    std::fs::create_dir_all(&out)?;
    let out = out.canonicalize()?;
    let work = out.parent().unwrap_or(&out).join("export-bench");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;

    // Catalog: import + ingest (EXIF, orientation, sizes).
    let config = IngestConfig { catalog_path: work.join("catalog.sqlite"), cache_dir: work.join("cache") };
    let mut conn = db::open(&config.catalog_path)?;
    let summary = repo::import_folder(&mut conn, &src, &ImportOptions { recursive: false })?;
    let t = Instant::now();
    let running = AtomicBool::new(true);
    run_until_idle(&config, &Quiet, &running)?;
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM images ORDER BY file_name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    println!(
        "source: {} ({} RAWs, ingest {:.1}s) | catalog {}",
        src.display(),
        summary.added,
        t.elapsed().as_secs_f64(),
        config.catalog_path.display()
    );

    // Edits on a few images.
    let luts = LutLibrary::new(work.join("luts"));
    let cube = work.join("bench-warm-film.cube");
    std::fs::write(&cube, film_cube())?;
    let lut = luts.import(&cube)?;
    let edits: Vec<(usize, ParametricAdjustments)> = vec![
        (
            0,
            ParametricAdjustments {
                exposure: 0.5,
                contrast: 20.0,
                shadows: 40.0,
                highlights: -30.0,
                vibrance: 25.0,
                ..Default::default()
            },
        ),
        (
            1,
            ParametricAdjustments {
                white_balance: WhiteBalance::Custom { temperature_k: 4300.0, tint: 8.0 },
                hsl: HslAdjustments {
                    saturation: HslChannels { orange: -20.0, blue: 30.0, ..Default::default() },
                    luminance: HslChannels { orange: 15.0, ..Default::default() },
                    ..Default::default()
                },
                clarity: 25.0,
                ..Default::default()
            },
        ),
        (
            2,
            ParametricAdjustments {
                exposure: 0.2,
                lut: Some(LutRef { id: lut.id.clone(), amount: 80.0 }),
                ..Default::default()
            },
        ),
        (3, ParametricAdjustments { dehaze: 20.0, texture: 30.0, saturation: -15.0, ..Default::default() }),
        (4, ParametricAdjustments { exposure: -0.7, blacks: -20.0, whites: 25.0, ..Default::default() }),
    ];
    for (i, adj) in &edits {
        if let Some(id) = ids.get(*i) {
            repo::save_adjustments(&conn, *id, adj)?;
        }
    }
    println!("edited images: {:?} (LUT {})", edits.iter().filter_map(|(i, _)| ids.get(*i)).collect::<Vec<_>>(), lut.id);

    // Editor-style previews of the edited images (colour reference).
    let pdir = out.join("preview");
    std::fs::create_dir_all(&pdir)?;
    for (i, adj) in &edits {
        let Some(id) = ids.get(*i) else { continue };
        let e = repo::get_image(&conn, *id)?;
        let img = source::decode_half_size(Path::new(&e.path))?;
        let prep = source::prepare(&img, e.orientation.unwrap_or(1), None, 2048);
        let input = pipeline::RenderInput {
            width: prep.width,
            height: prep.height,
            pixels: &prep.pixels,
            color: &img.color,
            frame_long_edge: prep.frame_long_edge,
        };
        let lut_obj = match &adj.lut {
            Some(l) => luts.load(&l.id)?,
            None => None,
        };
        let r = pipeline::render(&input, adj, lut_obj.as_deref());
        let jpeg = sieve_lib::raw::turbo::encode_rgb_444(&r.rgb, r.width, r.height, 92)?;
        let stem = Path::new(&e.file_name).file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(pdir.join(format!("{stem}.preview.jpg")), jpeg)?;
    }
    drop(conn);

    let exporter =
        Exporter::new(ExportConfig { catalog_path: config.catalog_path.clone(), memory_budget_mb: None }, luts);
    let caps = exporter.capabilities();
    let heic = caps.formats.iter().any(|f| f.kind.as_str() == "heic" && f.available);
    println!(
        "exporter: max_parallel {} | memory budget {} MiB | formats: {}",
        caps.max_parallel,
        caps.memory_budget_mb,
        caps.formats
            .iter()
            .map(|f| format!(
                "{}={}",
                f.kind.as_str(),
                if f.available { "yes".into() } else { format!("no ({})", f.reason.clone().unwrap_or_default()) }
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut rows = Vec::new();
    let repeat: usize = std::env::var("SIEVE_BENCH_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let runs: Vec<(&str, ExportSettings)> = (0..repeat.max(1)).flat_map(|_| presets(&out, heic)).collect();
    for (name, settings) in runs {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let target = match &settings.destination {
            ExportDestination::Folder { path } => PathBuf::from(path),
            _ => unreachable!(),
        };
        let _ = std::fs::remove_dir_all(&target);
        let (tx, rx) = mpsc::channel();
        let sink = Arc::new(Sink { done: AtomicU32::new(0), finished: Mutex::new(Some(tx)) });
        let samples: Arc<Mutex<Vec<Sample>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let sample_ms: u64 = std::env::var("SIEVE_BENCH_SAMPLE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(250);
        HEAP_PEAK.store(HEAP.load(Ordering::SeqCst), Ordering::SeqCst);
        let sampler = {
            let (samples, stop, sink) = (samples.clone(), stop.clone(), sink.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let (footprint, rss, _) = memory_mb();
                    let heap = HEAP.load(Ordering::SeqCst) as f64 / (1024.0 * 1024.0);
                    samples.lock().unwrap().push(Sample {
                        done: sink.done.load(Ordering::SeqCst),
                        heap,
                        rss,
                        footprint,
                    });
                    std::thread::sleep(Duration::from_millis(sample_ms));
                }
            })
        };
        let t = Instant::now();
        exporter.enqueue_with(sink.clone(), ids.clone(), settings.clone(), Some(name.into()))?;
        let fin = rx.recv_timeout(Duration::from_secs(3600))?;
        let secs = t.elapsed().as_secs_f64();
        stop.store(true, Ordering::SeqCst);
        sampler.join().ok();
        let s = samples.lock().unwrap().clone();
        let warm: Vec<&Sample> = s.iter().filter(|x| x.done as usize >= MAX_PARALLEL).collect();
        let range =
            |f: &dyn Fn(&Sample) -> f64| warm.iter().fold((f64::MAX, 0.0f64), |(a, b), x| (a.min(f(x)), b.max(f(x))));
        let (hmin, hmax) = range(&|x| x.heap);
        let (rmin, rmax) = range(&|x| x.rss);
        let heap_peak = HEAP_PEAK.load(Ordering::SeqCst) as f64 / (1024.0 * 1024.0);
        let (_, _, lifetime) = memory_mb();
        for f in &fin.failed {
            eprintln!("  {name}: FAILED {}: {}", f.file_name, f.reason);
        }
        let row = format!(
            "| {name} | {} | {} | {:.1} | {:.2} | {:.0}-{:.0} | {:.0} | {:.0}-{:.0} | {:.0} | {} | {:.1} |",
            fin.succeeded,
            fin.failed.len(),
            secs,
            fin.succeeded as f64 / secs,
            hmin,
            hmax,
            heap_peak,
            rmin,
            rmax,
            lifetime,
            warm.len(),
            dir_bytes(&target) as f64 / (1024.0 * 1024.0),
        );
        println!("{row}");
        // Live-heap trace (every 4th sample) for the log.
        let trace: Vec<String> = s.iter().step_by(4).map(|x| format!("{}:{:.0}", x.done, x.heap)).collect();
        println!("  live heap trace (done:MiB): {}", trace.join(" "));
        rows.push(row);
    }
    println!(
        "\n| preset | ok | failed | wall s | files/s | live heap MiB (min-max after warmup) | heap peak MiB | RSS MiB (min-max after warmup) | lifetime peak footprint MiB | samples | output MiB |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        println!("{r}");
    }
    println!("output: {}", out.display());
    Ok(())
}
