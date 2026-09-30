//! Runs the real ingest pipeline over a folder with a scratch catalog + cache.
//!
//! ```text
//! cargo run --release --example ingest_bench -- [folder] [limit]
//! ```
//! `folder` defaults to `$SIEVE_SAMPLES`, then "/Users/gurjotsingh/Pictures/test RAWS".
//! The source folder is only read. Set `SIEVE_BENCH_KEEP=/dir` to keep the
//! generated catalog + thumbnails there instead of a temp dir.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use sieve_lib::db::{self, repo};
use sieve_lib::ingest::{default_threads, run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
use sieve_lib::ipc::types::ImportOptions;

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
    let summary = repo::import_folder(&mut conn, &folder_path, &ImportOptions::raw_only(false))?;
    if let Some(n) = limit {
        conn.execute("DELETE FROM images WHERE id NOT IN (SELECT id FROM images ORDER BY file_name LIMIT ?1)", [n])?;
    }
    let import_s = t_import.elapsed().as_secs_f64();
    let queued = repo::count_pending(&conn)?;
    println!(
        "folder: {folder}\nregistered: {} added, {} invalid; queued {queued} (limit {limit:?}) in {import_s:.2}s",
        summary.added, summary.invalid
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
    println!("peak RSS (ru_maxrss): {:.1} MB", peak_rss_mb());
    if let Some(mb) = peak_footprint_mb() {
        println!("peak physical footprint: {mb:.1} MB");
    }
    if std::env::var_os("SIEVE_BENCH_KEEP").is_some() {
        println!("kept output in {}", work.display());
    }
    Ok(())
}
