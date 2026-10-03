//! Packaged-app smoke test: exercises the native stack (LibRaw, libjpeg-turbo, lcms2, libomp,
//! ONNX Runtime + CoreML, SQLite migrations) the way the app does, from inside a built bundle.
//!
//! ```text
//! cargo build --release --example bundle_smoke
//! scripts/bundle-smoke.sh            # copies the .app, drops this binary into Contents/MacOS
//! ```
//! Run from `<copy>.app/Contents/MacOS/`, `@executable_path/../Frameworks` resolves to the
//! bundle's Frameworks and the models come from `Contents/Resources/models`, exactly like the
//! `sieve` binary. Steps: open catalog (migrations) -> import + ingest thumbnails/previews ->
//! culling analysis (bundled models) -> one develop render -> one full-res JPEG export. Prints
//! the loaded non-system images (proof of where each dylib was loaded from).
//!
//! Usage: `bundle_smoke <src_dir> <work_dir>`. `src_dir` must be a copy (never `~/Pictures`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use sieve_lib::db::{self, repo};
use sieve_lib::develop::{pipeline, source};
use sieve_lib::export::{ExportConfig, ExportSink, Exporter};
use sieve_lib::ingest::{run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{
    AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady, ExportFinished, ExportProgress, ImportProgress,
    ThumbnailFailed, ThumbnailReady,
};
use sieve_lib::ipc::types::{
    ChromaSubsampling, CollisionPolicy, CropSettings, ExportColorSpace, ExportDestination, ExportFormat,
    ExportSettings, FileNaming, ImportOptions, MetadataInclude, MetadataOptions, ParametricAdjustments, ResizeMode,
    ResizeOptions,
};
use sieve_lib::lut::LutLibrary;
use sieve_lib::ml::worker::{self, AnalysisSink};
use sieve_lib::ml::{AnalysisConfig, Analyzer};

#[derive(Default)]
struct Sink {
    thumbs: AtomicU32,
    analyzed: AtomicU32,
    failures: AtomicU32,
    finished: Mutex<Option<mpsc::Sender<ExportFinished>>>,
}

impl IngestSink for Sink {
    fn ready(&self, _: ThumbnailReady) {
        self.thumbs.fetch_add(1, Ordering::Relaxed);
    }
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("thumbnail failed: image {}: {}", e.image_id, e.reason);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }
    fn progress(&self, _: ImportProgress) {}
}

impl AnalysisSink for Sink {
    fn ready(&self, _: AnalysisReady) {
        self.analyzed.fetch_add(1, Ordering::Relaxed);
    }
    fn failed(&self, e: AnalysisFailed) {
        eprintln!("analysis failed: image {}: {}", e.image_id, e.reason);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }
    fn progress(&self, _: AnalysisProgress) {}
    fn finished(&self, _: AnalysisFinished) {}
    fn ingest_running(&self) -> bool {
        false
    }
}

impl ExportSink for Sink {
    fn progress(&self, _: ExportProgress) {}
    fn finished(&self, e: ExportFinished) {
        if let Some(tx) = self.finished.lock().unwrap().as_ref() {
            let _ = tx.send(e);
        }
    }
}

/// Non-system Mach-O images loaded into this process (dyld's list).
#[cfg(target_os = "macos")]
fn loaded_images() -> Vec<String> {
    extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const std::ffi::c_char;
    }
    // SAFETY: dyld returns valid C strings for indices below the image count.
    (0..unsafe { _dyld_image_count() })
        .filter_map(|i| {
            let p = unsafe { _dyld_get_image_name(i) };
            (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
        })
        .filter(|n| !n.starts_with("/System/") && !n.starts_with("/usr/lib/"))
        .collect()
}

/// macOS-only check (dyld); other platforms report nothing.
#[cfg(not(target_os = "macos"))]
fn loaded_images() -> Vec<String> {
    Vec::new()
}

fn step(name: &str, t: Instant) {
    println!("ok   {name:<34} {:>8.0} ms", t.elapsed().as_secs_f64() * 1000.0);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [src, work] = [0, 1].map(|i| args.get(i).map(PathBuf::from));
    let (Some(src), Some(work)) = (src, work) else {
        return Err("usage: bundle_smoke <src_dir> <work_dir>".into());
    };
    let src = src.canonicalize()?;
    if src.starts_with("/Users/gurjotsingh/Pictures") {
        return Err("refusing to use ~/Pictures directly; copy the files into test-data first".into());
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    let contents = exe.parent().and_then(Path::parent).ok_or("exe has no Contents dir")?;
    let models_dir =
        std::env::var_os("SIEVE_MODELS").map(PathBuf::from).unwrap_or_else(|| contents.join("Resources/models"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    println!("exe     {}", exe.display());
    println!("models  {}", models_dir.display());
    let total = Instant::now();

    // 1. Catalog + migrations.
    let t = Instant::now();
    let config = IngestConfig { catalog_path: work.join("catalog.sqlite"), cache_dir: work.join("cache") };
    std::fs::create_dir_all(config.thumbs_dir())?;
    let mut conn = db::open(&config.catalog_path)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    step(&format!("catalog + migrations (v{version})"), t);

    // 2. Import + ingest (LibRaw / TurboJPEG thumbnails and 2048 px previews).
    let t = Instant::now();
    let summary = repo::import_folder(&mut conn, &src, &ImportOptions::raw_only(false))?;
    let sink = Arc::new(Sink::default());
    run_until_idle(&config, sink.as_ref(), &AtomicBool::new(true))?;
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM images ORDER BY file_name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    step(&format!("import + ingest ({} files, {} thumbs)", summary.added, sink.thumbs.load(Ordering::Relaxed)), t);
    if ids.is_empty() || sink.thumbs.load(Ordering::Relaxed) as usize != ids.len() {
        return Err("ingest did not produce a thumbnail for every image".into());
    }

    // 3. Culling analysis with the bundled models (CoreML EP when available).
    let t = Instant::now();
    let analyzer = Analyzer::load(&models_dir)?;
    let (det, lmk) = analyzer.providers();
    drop(analyzer);
    let stats = worker::run_blocking(
        &AnalysisConfig { catalog_path: config.catalog_path.clone(), models_dir: models_dir.clone() },
        sink.as_ref(),
    )?;
    step(&format!("analysis ({} ok, {} failed; det {det:?}, lmk {lmk:?})", stats.analyzed, stats.failed), t);
    if stats.failed > 0 || stats.analyzed as usize != ids.len() {
        return Err("analysis failed".into());
    }

    // 4. One develop render (half-size LibRaw decode + preview pipeline + TurboJPEG).
    let t = Instant::now();
    let entry = repo::get_image(&conn, ids[0])?;
    let adj = ParametricAdjustments { exposure: 0.3, contrast: 15.0, vibrance: 20.0, ..Default::default() };
    let (img, meta) = source::decode_half_size_meta(Path::new(&entry.path))?;
    let profile =
        sieve_lib::develop::camera::resolve(&meta, &adj.profile, &sieve_lib::profiles::ProfileLibrary::shared(), None);
    let prep = source::prepare(&img, entry.orientation.unwrap_or(1), &CropSettings::default(), None, 2048);
    let input = pipeline::RenderInput {
        width: prep.width,
        height: prep.height,
        pixels: &prep.pixels,
        color: &img.color,
        frame_long_edge: prep.frame_long_edge,
        view: prep.view,
        profile: &profile,
        seed: 1,
        quality: pipeline::Quality::Preview,
        tone: None,
    };
    let r = pipeline::render(&input, &adj, None);
    let jpeg = sieve_lib::raw::turbo::encode_rgb_444(&r.rgb, r.width, r.height, 92)?;
    let preview = work.join("render.jpg");
    std::fs::write(&preview, &jpeg)?;
    step(&format!("render {}x{} ({})", r.width, r.height, entry.file_name), t);
    repo::save_adjustments(&conn, ids[0], &adj)?;
    drop(conn);

    // 5. One full-resolution JPEG export through the real Exporter.
    let t = Instant::now();
    let out = work.join("export");
    let exporter = Exporter::new(
        ExportConfig { catalog_path: config.catalog_path.clone(), memory_budget_mb: None },
        LutLibrary::new(work.join("luts")),
    );
    let settings = ExportSettings {
        format: ExportFormat::Jpeg { quality: 90, chroma_subsampling: ChromaSubsampling::Yuv444 },
        color_space: ExportColorSpace::Srgb,
        resize: ResizeOptions { mode: ResizeMode::None, dont_enlarge: true, resolution_ppi: 300 },
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
    };
    let (tx, rx) = mpsc::channel();
    *sink.finished.lock().unwrap() = Some(tx);
    exporter.enqueue_with(sink.clone(), vec![ids[0]], settings, Some("smoke".into()))?;
    let fin = rx.recv_timeout(Duration::from_secs(600))?;
    for f in &fin.failed {
        eprintln!("export failed: {}: {}", f.file_name, f.reason);
    }
    let files: Vec<_> = std::fs::read_dir(&out)?.filter_map(Result::ok).map(|e| e.path()).collect();
    let bytes: u64 = files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
    step(&format!("export JPEG ({} ok, {:.1} MB)", fin.succeeded, bytes as f64 / 1e6), t);
    if fin.succeeded != 1 {
        return Err("export failed".into());
    }

    println!("total   {:.1} s", total.elapsed().as_secs_f64());
    println!("outputs {} | {}", preview.display(), files.first().map(|p| p.display().to_string()).unwrap_or_default());
    println!("loaded non-system images:");
    for name in loaded_images() {
        println!("  {name}");
    }
    Ok(())
}
