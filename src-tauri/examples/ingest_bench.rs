//! Runs the real ingest pipeline over a folder with a scratch catalog + cache.
//!
//! ```text
//! cargo run --release --example ingest_bench -- [folder] [limit]
//! ```
//! `folder` defaults to `$SIEVE_SAMPLES`, then "/Users/gurjotsingh/Pictures/test RAWS".
//! The source folder is only read. Set `SIEVE_BENCH_KEEP=/dir` to keep the
//! generated catalog + thumbnails there instead of a temp dir.
//! `SIEVE_BENCH_NON_RAW=1`: also import JPEG/HEIC/TIFF/PNG (camera JPEG siblings pair with
//! their RAW unless `SIEVE_BENCH_PAIR=0`). `SIEVE_BENCH_XMP=1`: then read the folder's
//! sidecars into the catalog (import hook, sidecar-wins; never writes files) and report
//! ratings / develop settings imported. Per-format ready/failed and capture-time nulls
//! are always reported.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use sieve_lib::db::{self, repo};
use sieve_lib::ingest::{default_threads, run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
use sieve_lib::ipc::types::ImportOptions;
use sieve_lib::xmp::{XmpSync, XmpSyncConfig};

const DEFAULT_SAMPLES: &str = "/Users/gurjotsingh/Pictures/test RAWS";

#[derive(Default)]
struct Counter {
    ready: AtomicU32,
    failed: AtomicU32,
    progress: AtomicU32,
}

impl IngestSink for Counter {
    fn ready(&self, _: ThumbnailReady) {
        self.ready.fetch_add(1, Ordering::Relaxed);
    }
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("failed: image {}: {}", e.image_id, e.reason);
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
    fn progress(&self, _: ImportProgress) {
        self.progress.fetch_add(1, Ordering::Relaxed);
    }
}

fn peak_rss_mb() -> f64 {
    // SAFETY: getrusage fills a zeroed struct we own.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // ru_maxrss is bytes on macOS, KiB on Linux.
    let bytes = if cfg!(target_os = "macos") { usage.ru_maxrss as f64 } else { usage.ru_maxrss as f64 * 1024.0 };
    bytes / (1024.0 * 1024.0)
}

/// Peak physical footprint (what Activity Monitor calls "Memory"). Unlike ru_maxrss it
/// excludes pages malloc has freed but the kernel has not reclaimed yet.
#[cfg(target_os = "macos")]
fn peak_footprint_mb() -> Option<f64> {
    // SAFETY: proc_pid_rusage fills a zeroed rusage_info_v4 we own.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    (rc == 0).then(|| info.ri_lifetime_max_phys_footprint as f64 / (1024.0 * 1024.0))
}

#[cfg(not(target_os = "macos"))]
fn peak_footprint_mb() -> Option<f64> {
    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let folder =
        args.next().or_else(|| std::env::var("SIEVE_SAMPLES").ok()).unwrap_or_else(|| DEFAULT_SAMPLES.to_owned());
    let limit: Option<u32> = args.next().map(|s| s.parse()).transpose()?;

    let tmp = tempfile::tempdir()?;
    let work = std::env::var_os("SIEVE_BENCH_KEEP").map(PathBuf::from).unwrap_or_else(|| tmp.path().to_owned());
    std::fs::create_dir_all(&work)?;
    let folder_path = PathBuf::from(&folder).canonicalize()?;
    if work.canonicalize()?.starts_with(&folder_path) {
        return Err("output directory must not be inside the source folder".into());
    }
    let config = IngestConfig { catalog_path: work.join("bench.sqlite"), cache_dir: work.join("cache") };

    let mut conn = db::open(&config.catalog_path)?;
    let t_import = Instant::now();
    let flag = |k: &str| std::env::var(k).is_ok_and(|v| v == "1");
    let opts = ImportOptions {
        recursive: true,
        include_non_raw: flag("SIEVE_BENCH_NON_RAW"),
        pair_jpeg_with_raw: std::env::var("SIEVE_BENCH_PAIR").map_or(true, |v| v != "0"),
    };
    let summary = repo::import_folder(&mut conn, &folder_path, &opts)?;
    if let Some(n) = limit {
        conn.execute("DELETE FROM images WHERE id NOT IN (SELECT id FROM images ORDER BY file_name LIMIT ?1)", [n])?;
    }
    let import_s = t_import.elapsed().as_secs_f64();
    let queued = repo::count_pending(&conn)?;
    println!(
        "folder: {folder}\nregistered: {} added, {} invalid, {} companions; queued {queued} (limit {limit:?}) in {import_s:.2}s",
        summary.added, summary.invalid, summary.companions
    );

    let threads =
        std::env::var("SIEVE_INGEST_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(default_threads);
    println!("threads: {threads}");
    let sink = Counter::default();
    let running = AtomicBool::new(true);
    let t = Instant::now();
    let stats = run_until_idle(&config, &sink, &running)?;
    let secs = t.elapsed().as_secs_f64();

    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, u32>(0));
    let ready = count("SELECT COUNT(*) FROM thumbnails WHERE status = 'ready'")?;
    let failed = count("SELECT COUNT(*) FROM thumbnails WHERE status = 'failed'")?;
    let no_time = count("SELECT COUNT(*) FROM images WHERE captured_at_ms IS NULL")?;
    let no_subsec = count("SELECT COUNT(*) FROM images WHERE captured_at_ms % 1000 = 0")?;
    let no_iso = count("SELECT COUNT(*) FROM images WHERE iso IS NULL OR shutter_s IS NULL OR aperture IS NULL")?;
    let portrait = count("SELECT COUNT(*) FROM thumbnails WHERE height > width")?;

    println!("processed: {} ({} failed) in {secs:.2}s", stats.done, stats.failed);
    println!("throughput: {:.1} files/s", stats.done as f64 / secs.max(1e-9));
    println!("catalog: ready {ready}, failed {failed}, portrait thumbs {portrait}");
    println!(
        "capture time null: {no_time}; whole-second (no subsec): {no_subsec}; missing iso/shutter/aperture: {no_iso}"
    );
    println!(
        "events: ready {}, failed {}, progress {}",
        sink.ready.load(Ordering::Relaxed),
        sink.failed.load(Ordering::Relaxed),
        sink.progress.load(Ordering::Relaxed)
    );
    let mut stmt = conn.prepare(
        "SELECT i.format, COUNT(*), SUM(t.status = 'ready'), SUM(t.status = 'failed'),
                SUM(i.captured_at_ms IS NULL), SUM(i.camera_model IS NULL), SUM(i.width IS NULL),
                GROUP_CONCAT(DISTINCT i.sensor_layout), GROUP_CONCAT(DISTINCT i.camera_model)
         FROM images i JOIN thumbnails t ON t.image_id = i.id GROUP BY i.format ORDER BY i.format",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(format!(
            "  {:<5} n={:<4} ready={:<4} failed={:<3} no_time={:<3} no_model={:<3} no_dims={:<3} layouts=[{}] models=[{}]",
            r.get::<_, String>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, u32>(3)?,
            r.get::<_, u32>(4)?,
            r.get::<_, u32>(5)?,
            r.get::<_, u32>(6)?,
            r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        ))
    })?;
    println!("per format:");
    for row in rows {
        println!("{}", row?);
    }
    let mut stmt = conn.prepare(
        "SELECT i.file_name, t.error FROM images i JOIN thumbnails t ON t.image_id = i.id WHERE t.status = 'failed' LIMIT 20",
    )?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (name, err) = row?;
        println!("  FAILED {name}: {}", err.unwrap_or_default());
    }
    drop(stmt);

    if flag("SIEVE_BENCH_XMP") {
        let t = Instant::now();
        let sync = XmpSync::new(XmpSyncConfig { catalog_path: config.catalog_path.clone() });
        let read = sync.refresh_folder(summary.folder_id)?;
        let secs = t.elapsed().as_secs_f64();
        let rated =
            count("SELECT COUNT(*) FROM images WHERE rating > 0 OR pick != 'unflagged' OR color_label IS NOT NULL")?;
        let edited = count("SELECT COUNT(*) FROM adjustments")?;
        let not_neutral = count("SELECT COUNT(*) FROM adjustments WHERE neutral = 0")?;
        let warned =
            count("SELECT COUNT(*) FROM images WHERE develop_warnings IS NOT NULL AND develop_warnings != '[]'")?;
        let errors = count("SELECT COUNT(*) FROM images WHERE xmp_error IS NOT NULL")?;
        let dirty = count("SELECT COUNT(*) FROM images WHERE xmp_dirty = 1")?;
        println!(
            "sidecars read: {read} in {secs:.2}s; images with rating/pick/label: {rated}; develop rows: {edited} \
             (non-neutral {not_neutral}); with develop warnings: {warned}; xmp errors: {errors}; dirty after read: {dirty}"
        );
        let mut stmt = conn.prepare("SELECT rating, COUNT(*) FROM images GROUP BY rating ORDER BY rating")?;
        let dist: Vec<String> = stmt
            .query_map([], |r| Ok(format!("{}*:{}", r.get::<_, u8>(0)?, r.get::<_, u32>(1)?)))?
            .collect::<Result<_, _>>()?;
        println!("rating distribution: {}", dist.join(" "));
        let mut stmt =
            conn.prepare("SELECT i.file_name, i.xmp_error FROM images i WHERE xmp_error IS NOT NULL LIMIT 10")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (n, e) = row?;
            println!("  XMP ERROR {n}: {e}");
        }
    }
    println!("peak RSS (ru_maxrss): {:.1} MB", peak_rss_mb());
    if let Some(mb) = peak_footprint_mb() {
        println!("peak physical footprint: {mb:.1} MB");
    }
    if std::env::var_os("SIEVE_BENCH_KEEP").is_some() {
        println!("kept output in {}", work.display());
    }
    Ok(())
}
