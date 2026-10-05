//! Keeper evaluation: scores Sieve's suggestions against a photographer's own cull,
//! read (never written) from the Lightroom/ACR sidecars next to the RAWs.
//!
//! ```text
//! cargo run --release --example keeper_eval -- [options]
//!   --catalog PATH     analyzed/ingested catalog (default test-data/qa-phase7b/analysis/eval.sqlite; only read)
//!   --work DIR         where the catalog copy lives (default: temp dir, deleted)
//!   --reuse            reuse the copy already in --work (no copy)
//!   --measure          run the analysis worker first (images whose stored metrics are
//!                      from another MODEL_VERSION are re-measured); default: rescore only
//!   --shoot TYPE       shoot type (default wedding)
//!   --thresholds JSON  partial CullThresholds override
//!   --strictness LEVEL reject strictness: conservative | balanced (default) | aggressive
//!   --fit-share F      share of the shoot (by capture time) used for fitting (default 0.6)
//!   --list TAG|reject-keeper   print held-out frames with that tag / keepers suggested reject
//! ```
//!
//! Labels: *keeper* = sidecar with develop settings (`crs:HasSettings="True"`) or
//! `xmp:Rating >= 2`; *non-keeper* = no sidecar at all (the photographer skipped it: a
//! noisy negative, some are simply redundant); sidecar with rating 0/1 and no develop
//! settings = ambiguous, excluded from keeper metrics. User rating (for Spearman) = the
//! sidecar rating, 0 without sidecar.
//!
//! The shoot is split by capture time: the first `fit-share` is the fit set (tune on it),
//! the rest is held out.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sieve_lib::db::{self, projects, repo};
use sieve_lib::ipc::events::{AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady};
use sieve_lib::ipc::types::RejectStrictness;
use sieve_lib::ipc::types::{CullThresholds, ShootType};
use sieve_lib::ml::worker::{self, AnalysisSink};
use sieve_lib::ml::AnalysisConfig;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data");

#[derive(Default)]
struct Counter {
    failed: AtomicU32,
}

impl AnalysisSink for Counter {
    fn ready(&self, _: AnalysisReady) {}
    fn failed(&self, e: AnalysisFailed) {
        eprintln!("failed: image {}: {}", e.image_id, e.reason);
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
    fn progress(&self, _: AnalysisProgress) {}
    fn finished(&self, _: AnalysisFinished) {}
    fn ingest_running(&self) -> bool {
        false
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Label {
    Keeper,
    NonKeeper,
    Ambiguous,
}

struct Frame {
    name: String,
    preview: String,
    captured: i64,
    label: Label,
    user_rating: i32,
    has_sidecar: bool,
    pick: String,
    stars: i64,
    overall: f64,
    tags: Vec<String>,
}

/// `(has develop settings, rating)` from the sidecar next to `raw`, if any.
fn sidecar(raw: &Path) -> Option<(bool, i32)> {
    let text = std::fs::read_to_string(raw.with_extension("xmp")).ok()?;
    let settings = text.contains("crs:HasSettings=\"True\"") || text.contains("<crs:HasSettings>True");
    let rating = text
        .split_once("xmp:Rating=\"")
        .and_then(|(_, r)| r.split('"').next())
        .or_else(|| text.split_once("<xmp:Rating>").and_then(|(_, r)| r.split('<').next()))
        .and_then(|r| r.trim().parse().ok())
        .unwrap_or(0);
    Some((settings, rating))
}

/// Average ranks (ties share the mean rank).
fn ranks(v: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| v[a].total_cmp(&v[b]));
    let mut r = vec![0.0; v.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && v[idx[j + 1]] == v[idx[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for &k in &idx[i..=j] {
            r[k] = avg;
        }
        i = j + 1;
    }
    r
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    cov / (va * vb).sqrt()
}

fn spearman(a: &[f64], b: &[f64]) -> f64 {
    pearson(&ranks(a), &ranks(b))
}

/// Probability that a random keeper has a higher `overall` than a random non-keeper.
fn auc(frames: &[&Frame]) -> f64 {
    let pos: Vec<f64> = frames.iter().filter(|f| f.label == Label::Keeper).map(|f| f.overall).collect();
    let neg: Vec<f64> = frames.iter().filter(|f| f.label == Label::NonKeeper).map(|f| f.overall).collect();
    let mut s = 0.0;
    for p in &pos {
        for n in &neg {
            s += if p > n {
                1.0
            } else if p == n {
                0.5
            } else {
                0.0
            };
        }
    }
    s / (pos.len() * neg.len()).max(1) as f64
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        "  n/a".into()
    } else {
        format!("{:5.1}%", 100.0 * a as f64 / b as f64)
    }
}

fn report(name: &str, frames: &[&Frame]) {
    let keepers = frames.iter().filter(|f| f.label == Label::Keeper).count();
    let non = frames.iter().filter(|f| f.label == Label::NonKeeper).count();
    let labeled = keepers + non;
    println!(
        "\n== {name}: {} frames ({keepers} keepers, {non} non-keepers, {} ambiguous)",
        frames.len(),
        frames.len() - labeled
    );
    println!("   keeper base rate {}", pct(keepers, labeled));
    let count = |pick: &str, label: Label| frames.iter().filter(|f| f.pick == pick && f.label == label).count();
    let (rk, rn) = (count("reject", Label::Keeper), count("reject", Label::NonKeeper));
    let (pk, pn) = (count("pick", Label::Keeper), count("pick", Label::NonKeeper));
    let rej = frames.iter().filter(|f| f.pick == "reject").count();
    let picks = frames.iter().filter(|f| f.pick == "pick").count();
    println!(
        "   suggested reject {rej:4} | false-reject rate on keepers {} ({rk}/{keepers}) | reject precision vs non-keepers {} ({rn}/{})",
        pct(rk, keepers),
        pct(rn, rk + rn),
        rk + rn
    );
    println!(
        "   suggested pick   {picks:4} | pick precision {} ({pk}/{}) | pick recall {} ({pk}/{keepers})",
        pct(pk, pk + pn),
        pk + pn,
        pct(pk, keepers)
    );
    let rated: Vec<&&Frame> = frames.iter().filter(|f| f.has_sidecar).collect();
    let rho_rated = spearman(
        &rated.iter().map(|f| f.stars as f64).collect::<Vec<_>>(),
        &rated.iter().map(|f| f.user_rating as f64).collect::<Vec<_>>(),
    );
    let lab: Vec<&&Frame> = frames.iter().filter(|f| f.label != Label::Ambiguous).collect();
    let rho_keep = spearman(
        &lab.iter().map(|f| f.stars as f64).collect::<Vec<_>>(),
        &lab.iter().map(|f| if f.label == Label::Keeper { 1.0 } else { 0.0 }).collect::<Vec<_>>(),
    );
    println!(
        "   Spearman(stars, user rating | rated n={}) {rho_rated:.3} | Spearman(stars, keeper) {rho_keep:.3} | AUC(overall, keeper) {:.3}",
        rated.len(),
        auc(frames)
    );
    // Mean suggested stars per user bucket.
    let mut buckets: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for f in frames {
        let key = match (f.has_sidecar, f.label) {
            (false, _) => "no sidecar".to_string(),
            (true, _) => format!("user {}*", f.user_rating),
        };
        let e = buckets.entry(key).or_default();
        e.0 += f.stars as f64;
        e.1 += 1;
    }
    let b: Vec<String> = buckets.iter().map(|(k, (s, n))| format!("{k}: {:.2} (n={n})", s / *n as f64)).collect();
    println!("   mean suggested stars: {}", b.join(" | "));
    let mut stars: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for f in frames.iter().filter(|f| f.label != Label::Ambiguous) {
        let e = stars.entry(f.stars).or_default();
        e.1 += 1;
        if f.label == Label::Keeper {
            e.0 += 1;
        }
    }
    let s: Vec<String> = stars.iter().map(|(k, (a, n))| format!("{k}*: {} of {n}", pct(*a, *n).trim())).collect();
    println!("   keeper share by suggested stars: {}", s.join(" | "));
    // Tags.
    let mut tags: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for f in frames {
        for t in &f.tags {
            let e = tags.entry(t.as_str()).or_default();
            e.0 += 1;
            match f.label {
                Label::Keeper => e.1 += 1,
                Label::NonKeeper => e.2 += 1,
                Label::Ambiguous => {}
            }
        }
    }
    for (t, (n, k, nk)) in &tags {
        println!(
            "   tag {t:>16}: {n:4} ({} of frames) keeper share {} ({k}/{})",
            pct(*n, frames.len()),
            pct(*k, k + nk),
            k + nk
        );
    }
    // What drives rejects of keepers.
    let mut drivers: BTreeMap<String, usize> = BTreeMap::new();
    for f in frames.iter().filter(|f| f.pick == "reject" && f.label == Label::Keeper) {
        let mut t = f.tags.clone();
        t.sort();
        *drivers.entry(if t.is_empty() { "(low overall)".into() } else { t.join("+") }).or_default() += 1;
    }
    if !drivers.is_empty() {
        println!("   keeper rejects by tags: {drivers:?}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = arg(&args, "--catalog").unwrap_or_else(|| format!("{ROOT}/qa-phase7b/analysis/eval.sqlite"));
    let shoot = ShootType::parse(&arg(&args, "--shoot").unwrap_or_else(|| "wedding".into())).expect("shoot type");
    let fit_share: f64 = arg(&args, "--fit-share").map_or(0.6, |s| s.parse().expect("fit share"));
    let models_dir = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));

    let tmp = tempfile::tempdir().unwrap();
    let work = arg(&args, "--work").map(PathBuf::from).unwrap_or_else(|| tmp.path().to_path_buf());
    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join("keeper_eval.sqlite");
    if !args.iter().any(|a| a == "--reuse") {
        for suffix in ["", "-wal", "-shm"] {
            let src = format!("{catalog}{suffix}");
            let dst = format!("{}{suffix}", copy.display());
            let _ = std::fs::remove_file(&dst);
            if Path::new(&src).exists() {
                std::fs::copy(&src, &dst).unwrap();
            }
        }
    }
    let mut conn = db::open(&copy).unwrap();
    repo::set_shoot_type(&conn, shoot).unwrap();
    // Ingest puts each folder in a project (default shoot type `general`) and images are
    // scored with their project's shoot type, so set it there too.
    for p in projects::list_projects(&conn).unwrap() {
        projects::set_project_shoot_type(&conn, p.id, shoot).unwrap();
    }
    if let Some(level) = arg(&args, "--strictness") {
        let level = RejectStrictness::parse(&level).expect("conservative | balanced | aggressive");
        for p in projects::list_projects(&conn).unwrap() {
            projects::set_project_reject_strictness(&conn, p.id, level).unwrap();
        }
    }
    let levels: Vec<&str> =
        projects::list_projects(&conn).unwrap().iter().map(|p| p.reject_strictness.as_str()).collect();
    println!("reject strictness (per project): {levels:?}");
    if let Some(json) = arg(&args, "--thresholds") {
        let mut v = serde_json::to_value(repo::cull_thresholds(&conn, shoot).unwrap()).unwrap();
        let patch: serde_json::Value = serde_json::from_str(&json).expect("thresholds JSON");
        for (k, val) in patch.as_object().expect("object") {
            v[k] = val.clone();
        }
        let t: CullThresholds = serde_json::from_value(v).unwrap();
        repo::set_cull_thresholds(&conn, shoot, Some(&t)).unwrap();
    }
    if args.iter().any(|a| a == "--measure") {
        let config = AnalysisConfig { catalog_path: copy.clone(), models_dir };
        let t = std::time::Instant::now();
        let stats = worker::run_blocking(&config, &Counter::default()).unwrap();
        println!("measured {} ({} failed) in {:.1} s", stats.analyzed, stats.failed, t.elapsed().as_secs_f64());
    }
    let groups = worker::rescore_all(&mut conn).unwrap();
    println!("rescored; {groups} bursts");

    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    let mut stmt =
        conn.prepare("SELECT image_id, tag FROM image_tags WHERE source = 'auto' AND suppressed = 0").unwrap();
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (id, tag) = row.unwrap();
        tags.entry(id).or_default().push(tag);
    }
    let mut frames: Vec<Frame> = conn
        .prepare(
            "SELECT i.id, i.path, i.file_name, i.captured_at_ms, q.suggested_pick, q.suggested_rating, q.overall,
                    t.preview_path
             FROM images i JOIN quality_scores q ON q.image_id = i.id JOIN thumbnails t ON t.image_id = i.id
             WHERE i.captured_at_ms IS NOT NULL",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, f64>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .unwrap()
        .map(|r| {
            let (id, path, name, captured, pick, stars, overall, preview) = r.unwrap();
            let sc = sidecar(Path::new(&path));
            let label = match sc {
                None => Label::NonKeeper,
                Some((settings, rating)) if settings || rating >= 2 => Label::Keeper,
                Some(_) => Label::Ambiguous,
            };
            Frame {
                name,
                preview: preview.unwrap_or_default(),
                captured,
                label,
                user_rating: sc.map_or(0, |s| s.1),
                has_sidecar: sc.is_some(),
                pick,
                stars,
                overall,
                tags: tags.remove(&id).unwrap_or_default(),
            }
        })
        .collect();
    // The same file imported twice (e.g. a card copy in another folder): the copy without
    // a sidecar is not a "skipped" frame, so it is excluded from the keeper metrics.
    let mut with_sidecar: HashMap<(String, i64), bool> = HashMap::new();
    for f in &frames {
        *with_sidecar.entry((f.name.clone(), f.captured)).or_default() |= f.has_sidecar;
    }
    let mut copies = 0;
    for f in &mut frames {
        if !f.has_sidecar && with_sidecar[&(f.name.clone(), f.captured)] {
            f.label = Label::Ambiguous;
            copies += 1;
        }
    }
    println!("{copies} duplicate copies of sidecar'd frames excluded from keeper metrics");
    frames.sort_by_key(|f| (f.captured, f.name.clone()));
    let cut = (frames.len() as f64 * fit_share).round() as usize;
    let fit: Vec<&Frame> = frames[..cut].iter().collect();
    let held: Vec<&Frame> = frames[cut..].iter().collect();
    let all: Vec<&Frame> = frames.iter().collect();
    report(&format!("fit (first {:.0}%)", fit_share * 100.0), &fit);
    report(&format!("held-out (last {:.0}%)", (1.0 - fit_share) * 100.0), &held);
    report("all", &all);

    if let Some(what) = arg(&args, "--list") {
        println!("\nheld-out frames: {what}");
        for f in held.iter().filter(|f| {
            if what == "reject-keeper" {
                f.pick == "reject" && f.label == Label::Keeper
            } else {
                f.tags.iter().any(|t| t == &what)
            }
        }) {
            println!(
                "  {:14} {:?} user {} stars {} {} {:?} {}",
                f.name, f.label, f.user_rating, f.stars, f.pick, f.tags, f.preview
            );
        }
    }
    if arg(&args, "--work").is_some() {
        println!("\ncatalog copy kept at {}", copy.display());
    }
}
