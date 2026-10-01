//! End-to-end check of the style model through the command-layer code (`ml::style`) on a
//! scratch catalog (Phase 8b "Style learning" acceptance).
//!
//! ```text
//! cargo run --release --example style_eval -- --out ../test-data/style-eval   # writes heldout.json
//! cargo run --release --example style_e2e -- --work ../test-data/style-e2e \
//!     [--folder DIR] [--eval-rows ../test-data/style-eval/heldout.json] [--n 12]
//! ```
//! 1. APFS clones (`cp -c`) of every RAW of `--folder` into `<work>/raws` (no sidecars; the
//!    originals are only read), import + ingest (previews), scene detection (Phase 7).
//! 2. The user's Lightroom settings of the eval's *training* frames (every edited frame not in
//!    `heldout.json`) are stored as catalog edits (what "Read from XMP" does); held-out frames
//!    stay unedited.
//! 3. `ml::style::train_catalog` (features -> fit -> validate -> store) with the private
//!    training cache, timed.
//! 4. `StyleModel::predict` (the `predict_style` body) for `--n` held-out frames spread over
//!    the shoot, cold (decode) and warm, then ΔE2000 at 768 px vs the user's settings
//!    (`ml::style::validation_errors`, masks off, user crop) next to the eval's numbers for
//!    the same frames; `apply_style_prediction`'s commit + undo on them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;

use serde::Deserialize;

use sieve_lib::db::{self, projects::FolderScope, repo};
use sieve_lib::develop::{batches, DevelopCache, DevelopConfig};
use sieve_lib::ingest::{run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{ImportProgress, ThumbnailFailed, ThumbnailReady};
use sieve_lib::ipc::types::{ImageId, ImportOptions, ParametricAdjustments, SceneDetectOptions, StyleTrainPhase};
use sieve_lib::lut::LutLibrary;
use sieve_lib::ml::style::{self, CatalogFrame, StyleModel, StyleModelConfig, TrainControl};
use sieve_lib::ml::style_model;
use sieve_lib::{raw, scene, xmp};

const DEFAULT_FOLDER: &str = "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal/10060918";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvalRow {
    stem: String,
    /// Stage A, no edit, auto tone (the eval's 768 px renders).
    predicted: f64,
    no_edit: f64,
    auto_tone: f64,
}

struct Sink;
impl IngestSink for Sink {
    fn ready(&self, _: ThumbnailReady) {}
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("thumbnail failed: image {}: {}", e.image_id, e.reason);
    }
    fn progress(&self, _: ImportProgress) {}
}

struct Control {
    last: Mutex<Option<StyleTrainPhase>>,
    started: Instant,
}
impl TrainControl for Control {
    fn progress(&self, phase: StyleTrainPhase, done: u32, total: u32) {
        let mut last = self.last.lock().unwrap();
        if *last != Some(phase) || done == total {
            println!("  [{:>6.1}s] {:?} {done}/{total}", self.started.elapsed().as_secs_f64(), phase);
            *last = Some(phase);
        }
    }
    fn cancelled(&self) -> bool {
        false
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn step(name: &str, t: Instant) {
    println!("{name:<40} {:>8.1} s", t.elapsed().as_secs_f64());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let folder = PathBuf::from(arg("--folder").unwrap_or_else(|| DEFAULT_FOLDER.into()));
    let work = PathBuf::from(arg("--work").unwrap_or_else(|| "../test-data/style-e2e".into()));
    let rows_path = PathBuf::from(arg("--eval-rows").unwrap_or_else(|| "../test-data/style-eval/heldout.json".into()));
    let n: usize = arg("--n").and_then(|v| v.parse().ok()).unwrap_or(12);
    std::fs::create_dir_all(&work)?;
    let work = work.canonicalize()?;
    if work.starts_with("/Users/gurjotsingh/Pictures") {
        return Err("--work must not be under ~/Pictures".into());
    }
    let eval_rows: Vec<EvalRow> = serde_json::from_slice(&std::fs::read(&rows_path)?)?;
    let held: HashMap<String, &EvalRow> = eval_rows.iter().map(|r| (r.stem.clone(), r)).collect();

    // 1. Clones + catalog.
    let t = Instant::now();
    let raws_dir = work.join("raws");
    let catalog_path = work.join("catalog.sqlite");
    for p in [&raws_dir, &work.join("cache")] {
        let _ = std::fs::remove_dir_all(p);
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", catalog_path.display()));
    }
    std::fs::create_dir_all(&raws_dir)?;
    let mut originals: Vec<PathBuf> = std::fs::read_dir(&folder)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| raw::format_from_extension(p).is_some_and(|f| f.is_raw()))
        .collect();
    originals.sort();
    for p in &originals {
        let st =
            std::process::Command::new("cp").arg("-c").arg(p).arg(raws_dir.join(p.file_name().unwrap())).status()?;
        if !st.success() {
            return Err(format!("clone failed: {}", p.display()).into());
        }
    }
    step(&format!("clone {} RAWs (cp -c)", originals.len()), t);

    let t = Instant::now();
    let config = IngestConfig { catalog_path: catalog_path.clone(), cache_dir: work.join("cache") };
    std::fs::create_dir_all(config.thumbs_dir())?;
    let mut conn = db::open(&catalog_path)?;
    repo::import_folder(&mut conn, &raws_dir, &ImportOptions::raw_only(false))?;
    run_until_idle(&config, &Sink, &AtomicBool::new(true))?;
    step("import + ingest", t);

    let t = Instant::now();
    let mut frames = scene::store::detection_frames(&conn, FolderScope::all(), false)?;
    let computed = scene::features::compute_missing(&mut frames, &|_, _| {});
    scene::store::save_features(&mut conn, &computed)?;
    let groups = scene::detect::group(&frames, &SceneDetectOptions::default());
    let scenes = scene::store::replace_scenes(&mut conn, FolderScope::all(), &groups, false)?;
    step(&format!("scene detection ({} scenes)", scenes.len()), t);

    // 2. Training edits from the original sidecars (read only).
    let ids: Vec<(ImageId, String)> = conn
        .prepare("SELECT id, file_name FROM images ORDER BY captured_at_ms, file_name")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut train_n = 0;
    let mut held_frames: Vec<(ImageId, String, ParametricAdjustments)> = Vec::new();
    for (id, name) in &ids {
        let stem = Path::new(name).file_stem().unwrap().to_string_lossy().into_owned();
        let format = raw::format_from_extension(Path::new(name)).unwrap();
        let Ok(text) = std::fs::read_to_string(folder.join(format!("{stem}.xmp"))) else { continue };
        let Some(adj) = xmp::packet::parse_for(&text, format).ok().and_then(|v| v.develop) else { continue };
        if !style_model::is_style_sample(&adj, format) {
            continue;
        }
        if held.contains_key(&stem) {
            held_frames.push((*id, stem, adj));
        } else {
            repo::save_adjustments(&conn, *id, &adj)?;
            train_n += 1;
        }
    }
    println!("training edits stored: {train_n}; held-out (unedited in the catalog): {}", held_frames.len());

    // 3. Train through the command-layer code.
    let t = Instant::now();
    let luts = LutLibrary::new(work.join("luts"));
    let train_cache = StyleModel::training_cache(&DevelopConfig::default());
    let control = Control { last: Mutex::new(None), started: Instant::now() };
    let summary = style::train_catalog(&catalog_path, &train_cache, &luts, &control)?.expect("not cancelled");
    let train_s = t.elapsed().as_secs_f64();
    println!(
        "train_catalog: {:.1} s total (features {:.1} s for {} renders, fit {:.1} s, validate {:.1} s); {} samples, {} skipped",
        train_s,
        summary.features_ms / 1000.0,
        summary.features_computed,
        summary.fit_ms / 1000.0,
        summary.validate_ms / 1000.0,
        summary.samples,
        summary.skipped
    );
    println!("in-app validation (StyleValidation): {:?}", summary.validation);
    drop(train_cache);
    // Retrain with cached features (what a user's second "Train" costs).
    let t = Instant::now();
    let train_cache = StyleModel::training_cache(&DevelopConfig::default());
    let s2 = style::train_catalog(&catalog_path, &train_cache, &luts, &control)?.expect("not cancelled");
    println!(
        "retrain (cached features): {:.1} s (features {:.1} s, fit {:.1} s, validate {:.1} s)",
        t.elapsed().as_secs_f64(),
        s2.features_ms / 1000.0,
        s2.fit_ms / 1000.0,
        s2.validate_ms / 1000.0
    );
    drop(train_cache);

    // 4. Predict held-out frames through `StyleModel::predict` (predict_style body).
    let state = StyleModel::new(StyleModelConfig { catalog_path: catalog_path.clone() });
    let status = state.status(&conn)?;
    println!(
        "status: {:?}, trainingExamples {}, availableExamples {}, minExamples {}",
        status.state, status.training_examples, status.available_examples, status.min_examples
    );
    let app_cache = DevelopCache::new(DevelopConfig::default());
    let pick: Vec<&(ImageId, String, ParametricAdjustments)> =
        (0..n.min(held_frames.len())).map(|k| &held_frames[k * held_frames.len() / n.min(held_frames.len())]).collect();
    let mut rows: BTreeMap<String, ([f64; 3], [f64; 3])> = BTreeMap::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    let mut items = Vec::new();
    for (id, stem, user) in &pick {
        let inputs = scene::store::match_inputs(&conn, &[*id])?;
        let t = Instant::now();
        let p = state.predict(&app_cache, &luts, &inputs)?;
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let p2 = state.predict(&app_cache, &luts, &inputs)?;
        warm.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(p, p2, "prediction is deterministic");
        let frame = CatalogFrame::load(&conn, *id, None)?;
        let de = style::validation_errors(&app_cache, &luts, &frame, user, &p[0].adjustments, 768)?;
        let e = held[stem.as_str()];
        rows.insert(stem.clone(), (de, [e.predicted, e.auto_tone, e.no_edit]));
        println!(
            "  {stem:<12} catalog: pred {:5.2} auto {:5.2} none {:5.2} | eval: pred {:5.2} auto {:5.2} none {:5.2} | conf {:.2} {:?}",
            de[0], de[1], de[2], e.predicted, e.auto_tone, e.no_edit, p[0].confidence, p[0].notes
        );
        items.push(batches::BatchItem {
            image_id: *id,
            adjustments: p[0].adjustments.clone(),
            scene_id: None,
            review_reason: None,
        });
    }
    let col = |k: usize, eval: bool| -> f64 {
        mean(&rows.values().map(|(c, e)| if eval { e[k] } else { c[k] }).collect::<Vec<_>>())
    };
    println!(
        "\n== {} held-out frames, ΔE2000 vs the user's settings (768 px) ==\n  catalog path: predicted {:.2} | auto tone {:.2} | no edit {:.2}\n  style_eval  : predicted {:.2} | auto tone {:.2} | no edit {:.2}",
        rows.len(),
        col(0, false),
        col(1, false),
        col(2, false),
        col(0, true),
        col(1, true),
        col(2, true)
    );
    let med = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    println!(
        "predict per frame: cold {:.0} ms median (decode + 640 px neutral render + features), warm {:.1} ms median",
        med(&mut cold),
        med(&mut warm)
    );

    // apply_style_prediction's commit + undo.
    let r = batches::commit_recorded(&mut conn, &items, batches::LABEL_STYLE, batches::BatchKind::StylePrediction)?;
    let labels: i64 =
        conn.query_row("SELECT COUNT(*) FROM adjustment_history WHERE label = ?1", [batches::LABEL_STYLE], |r| {
            r.get(0)
        })?;
    let undo = batches::undo(&mut conn, r.batch_id.expect("batch"))?;
    println!(
        "apply: batch {:?} label {:?} changed {} ({} history entries); undo restored {} skipped {}",
        r.batch_id,
        r.label,
        r.changed_ids.len(),
        labels,
        undo.restored_ids.len(),
        undo.skipped_ids.len()
    );
    Ok(())
}
