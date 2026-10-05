//! Culling-engine evaluation: runs the real analysis worker over a *copy* of an ingested
//! catalog and scores it against hand labels.
//!
//! ```text
//! cargo run --release --example cull_eval -- [options]
//!   --catalog PATH     ingested catalog (default test-data/qa-phase2/bench.sqlite; only read)
//!   --labels PATH      labels JSON (default test-data/labels.json)
//!   --work DIR         where the catalog copy lives (default: temp dir, deleted)
//!   --rescore          reuse the copy in --work: rescore only (no ML), for threshold tuning
//!   --shoot TYPE       shoot type (default wedding)
//!   --thresholds JSON  partial CullThresholds override, e.g. '{"blinkEar":0.15}'
//!   --strictness LEVEL reject strictness: conservative | balanced (default) | aggressive
//! ```
//! Prints ms/image (throughput with the worker pool, and single-thread latency),
//! precision/recall for blink / missed_focus / motion_blur / underexposed on the labeled
//! set (null labels excluded), per-tag counts, suggestion counts and a burst summary.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use sieve_lib::db::{self, projects, repo};
use sieve_lib::ipc::events::{AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady};
use sieve_lib::ipc::types::{CullThresholds, ShootType};
use sieve_lib::ml::scoring::RejectStrictness;
use sieve_lib::ml::store;
use sieve_lib::ml::worker::{self, AnalysisSink};
use sieve_lib::ml::{AnalysisConfig, Analyzer};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data");

#[derive(Default)]
struct Counter {
    ready: AtomicU32,
    failed: AtomicU32,
}

impl AnalysisSink for Counter {
    fn ready(&self, _: AnalysisReady) {
        self.ready.fetch_add(1, Ordering::Relaxed);
    }
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = arg(&args, "--catalog").unwrap_or_else(|| format!("{ROOT}/qa-phase2/bench.sqlite"));
    let labels_path = arg(&args, "--labels").unwrap_or_else(|| format!("{ROOT}/labels.json"));
    let rescore_only = args.iter().any(|a| a == "--rescore");
    let shoot = ShootType::parse(&arg(&args, "--shoot").unwrap_or_else(|| "wedding".into())).expect("shoot type");
    let models_dir = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));

    let tmp = tempfile::tempdir().unwrap();
    let work = arg(&args, "--work").map(PathBuf::from).unwrap_or_else(|| tmp.path().to_path_buf());
    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join("eval.sqlite");
    if !rescore_only {
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
        conn.execute(
            "INSERT INTO catalog_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [store::REJECT_STRICTNESS_KEY, level.as_str()],
        )
        .unwrap();
    }
    println!("reject strictness: {}", store::reject_strictness(&conn).unwrap().as_str());
    if let Some(json) = arg(&args, "--thresholds") {
        let mut v = serde_json::to_value(repo::cull_thresholds(&conn, shoot).unwrap()).unwrap();
        let patch: serde_json::Value = serde_json::from_str(&json).expect("thresholds JSON");
        for (k, val) in patch.as_object().expect("object") {
            v[k] = val.clone();
        }
        let t: CullThresholds = serde_json::from_value(v).unwrap();
        repo::set_cull_thresholds(&conn, shoot, Some(&t)).unwrap();
    }
    println!(
        "thresholds ({shoot:?}): {}",
        serde_json::to_string(&repo::cull_thresholds(&conn, shoot).unwrap()).unwrap()
    );

    let n_ready: u32 = conn
        .query_row("SELECT COUNT(*) FROM thumbnails WHERE status = 'ready' AND preview_path IS NOT NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    if rescore_only {
        let t = Instant::now();
        let groups = worker::rescore_all(&mut conn).unwrap();
        println!("rescore: {n_ready} images, {groups} bursts in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    } else {
        let config = AnalysisConfig { catalog_path: copy.clone(), models_dir: models_dir.clone() };
        let sink = Counter::default();
        let t = Instant::now();
        let stats = worker::run_blocking(&config, &sink).unwrap();
        let wall = t.elapsed().as_secs_f64() * 1000.0;
        let n = (stats.analyzed + stats.failed).max(1);
        println!(
            "worker: {} analyzed, {} failed, {} bursts, threads={}, wall {:.0} ms => {:.1} ms/image (throughput, incl. model load)",
            stats.analyzed,
            stats.failed,
            stats.burst_groups,
            std::env::var("SIEVE_ANALYSIS_THREADS").unwrap_or_else(|_| worker::default_threads().to_string()),
            wall,
            wall / n as f64
        );
        // Single-thread latency on the first 100 previews (after one warm-up image).
        let paths: Vec<String> = conn
            .prepare("SELECT preview_path FROM thumbnails WHERE preview_path IS NOT NULL ORDER BY image_id LIMIT 101")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let t = Instant::now();
        let mut a = Analyzer::load(&models_dir).unwrap();
        let load = t.elapsed().as_secs_f64() * 1000.0;
        a.measure(Path::new(&paths[0])).unwrap();
        let t = Instant::now();
        for p in &paths[1..] {
            a.measure(Path::new(p)).unwrap();
        }
        println!(
            "latency: {:.1} ms/image single-thread over {} previews (model load {:.0} ms, providers {:?})",
            t.elapsed().as_secs_f64() * 1000.0 / (paths.len() - 1) as f64,
            paths.len() - 1,
            load,
            a.providers()
        );
    }

    // Tags per image stem.
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT i.file_name, t.tag FROM images i JOIN image_tags t ON t.image_id = i.id
             WHERE t.source = 'auto' AND t.suppressed = 0",
        )
        .unwrap();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (name, tag) = row.unwrap();
        let stem = name.rsplit_once('.').map_or(name.clone(), |(s, _)| s.to_string());
        tags.entry(stem).or_default().push(tag);
    }
    let picks: HashMap<String, (String, i64)> = conn
        .prepare("SELECT i.file_name, q.suggested_pick, q.suggested_rating FROM images i JOIN quality_scores q ON q.image_id = i.id")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
        .unwrap()
        .map(|r| {
            let (n, p, s) = r.unwrap();
            (n.rsplit_once('.').map_or(n.clone(), |(s, _)| s.to_string()), (p, s))
        })
        .collect();

    let labels: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&labels_path).unwrap()).unwrap();
    let images = labels["images"].as_object().unwrap();
    // Label sets: entries without "set" are the original 50; others name their set.
    let set_of = |l: &serde_json::Value| l["set"].as_str().unwrap_or("original").to_string();
    let mut sets: Vec<String> = images.values().map(set_of).collect();
    sets.sort();
    sets.dedup();
    sets.push("all".into());
    for set in &sets {
        let n = images.values().filter(|l| set == "all" || &set_of(l) == set).count();
        println!("\nlabel set '{set}': {n} images");
        for (label, tag) in [
            ("blink", "blink"),
            ("missed_focus", "missed_focus"),
            ("motion_blur", "motion_blur"),
            ("underexposed", "underexposed"),
        ] {
            let (mut tp, mut fp, mut fn_) = (0, 0, 0);
            let mut errors = Vec::new();
            for (stem, l) in images {
                if set != "all" && &set_of(l) != set {
                    continue;
                }
                let Some(truth) = l[label].as_bool() else { continue };
                let pred = tags.get(stem).is_some_and(|t| t.iter().any(|x| x == tag));
                match (pred, truth) {
                    (true, true) => tp += 1,
                    (true, false) => {
                        fp += 1;
                        errors.push(format!("FP {stem}"));
                    }
                    (false, true) => {
                        fn_ += 1;
                        errors.push(format!("FN {stem}"));
                    }
                    _ => {}
                }
            }
            let p = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { f64::NAN };
            let r = if tp + fn_ > 0 { tp as f64 / (tp + fn_) as f64 } else { f64::NAN };
            if tp + fp + fn_ > 0 {
                println!(
                    "{label:>13}: precision {p:.2} recall {r:.2}  (TP {tp}, FP {fp}, FN {fn_})  {}",
                    errors.join(" ")
                );
            }
        }
    }
    // Tagged frames nobody has labeled for that tag (for review).
    for tag in ["blink", "missed_focus"] {
        let mut unlabeled: Vec<&String> = tags
            .iter()
            .filter(|(stem, t)| t.iter().any(|x| x == tag) && images.get(*stem).is_none_or(|l| l[tag].is_null()))
            .map(|(stem, _)| stem)
            .collect();
        unlabeled.sort();
        println!(
            "\n{tag} tagged, not labeled for it ({}): {}",
            unlabeled.len(),
            unlabeled.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
        );
    }
    // Keeper agreement.
    let mut keeper = BTreeMap::new();
    for (stem, l) in images {
        let Some(k) = l["keeper"].as_bool() else { continue };
        let pick = picks.get(stem).map_or("none".to_string(), |p| p.0.clone());
        *keeper.entry((k, pick)).or_insert(0) += 1;
    }
    println!("keeper label x suggested pick: {keeper:?}");

    // Whole-catalog counts.
    println!("\nall {n_ready} images, auto tags:");
    let counts: Vec<(String, u32)> = conn
        .prepare(
            "SELECT tag, COUNT(*) FROM image_tags WHERE source = 'auto' AND suppressed = 0 GROUP BY tag ORDER BY tag",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (t, c) in counts {
        println!("  {t:>16}: {c}");
    }
    let sugg: Vec<(String, u32)> = conn
        .prepare("SELECT suggested_pick, COUNT(*) FROM quality_scores GROUP BY suggested_pick")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let stars: Vec<(i64, u32)> = conn
        .prepare("SELECT suggested_rating, COUNT(*) FROM quality_scores GROUP BY suggested_rating")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    println!("  suggested pick: {sugg:?}; stars: {stars:?}");

    let groups = repo::list_burst_groups(&conn, None).unwrap();
    let members: usize = groups.iter().map(|g| g.image_ids.len()).sum();
    let mut sizes: BTreeMap<usize, u32> = BTreeMap::new();
    for g in &groups {
        *sizes.entry(g.image_ids.len()).or_insert(0) += 1;
    }
    println!("\nbursts: {} groups, {members} member images, sizes {sizes:?}", groups.len());
    let name: HashMap<i64, String> = conn
        .prepare("SELECT id, file_name FROM images")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for g in groups.iter().take(12) {
        let names: Vec<String> = g
            .image_ids
            .iter()
            .map(|id| {
                let n = name[id].trim_end_matches(".ARW").to_string();
                if Some(*id) == g.keeper_image_id {
                    format!("*{n}")
                } else {
                    n
                }
            })
            .collect();
        println!("  {:>5} ms: {}", g.ended_at_ms - g.started_at_ms, names.join(" "));
    }
    if arg(&args, "--work").is_some() {
        println!("\ncatalog copy kept at {}", copy.display());
    }
}
