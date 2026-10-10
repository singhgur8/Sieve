//! Target-selection calibration: runs the Phase 9 engine (people -> moments -> selection) on a
//! folder of JPEG previews and compares the delivery set with the photographer's own flags,
//! read (never written) from the XMP sidecars next to them.
//!
//! ```text
//! cargo run --release --example target_eval -- <folder> [options]
//!   <folder>          JPEG previews named like the RAWs (DSC0412.JPG) + the RAWs' sidecars
//!                     (DSC0412.xmp; DSC0412.JPG.xmp also accepted). Only read.
//!   --target N        target count (default: the number of keepers in the sidecars)
//!   --shoot TYPE      shoot type (default wedding)
//!   --strictness L    reject strictness: conservative | balanced (default) | aggressive
//!   --work DIR        scratch catalog + preview cache (default: temp dir, deleted)
//!   --reuse           reuse the catalog in --work (skip import / ingest / analysis)
//!   --min-rating N    keepers = picks; when no sidecar has a pick flag, rating >= N
//!                     (default 1)
//!   --csv PATH        per-photo table (name, label, choice, shot type, moment, reasons)
//!   --list N          print the N worst moments and keepers Sieve set aside (default 15)
//! ```
//! Models: `SIEVE_MODELS` or `src-tauri/models` (face detection; face identity when installed).
//!
//! Labels: *keeper* = `xmpDM:pick="1"` (Lightroom 13.2+ pick) or Sieve's legacy
//! `xmp:Label="Pick"`; when no sidecar carries a pick, `xmp:Rating >= --min-rating`.
//! *Rejected* = `xmpDM:pick="-1"` or `xmp:Rating="-1"`. Everything else = not delivered.
//! Capture time: EXIF of the JPEG; when missing, the sidecar's `exif:DateTimeOriginal` /
//! `photoshop:DateCreated` / `xmp:CreateDate` (wall clock, as the catalog stores it).
//! The engine never sees the labels: catalog flags stay unflagged.
//!
//! Prints overall agreement (precision / recall / F1 of "delivered" vs "keeper", accuracy,
//! keepers within one key = delivered or alternative), per shot type, per moment (count error,
//! moments with keepers covered by a delivery, delivered moments without keepers), the choice
//! and first reason of keepers that were not delivered and of delivered non-keepers, and timings.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use sieve_lib::db::{self, projects, repo, target};
use sieve_lib::ingest::{run_until_idle, IngestConfig, IngestSink};
use sieve_lib::ipc::events::{
    AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady, ImportProgress, ThumbnailFailed, ThumbnailReady,
};
use sieve_lib::ipc::types::{ImageId, ImageSelection, ImportOptions, RejectStrictness, ShootType, TargetChoice};
use sieve_lib::ml::selection::{run_pipeline, TargetJob};
use sieve_lib::ml::worker::{self, AnalysisSink};
use sieve_lib::ml::AnalysisConfig;

#[derive(Default)]
struct Counter {
    failed: AtomicU32,
}

impl IngestSink for Counter {
    fn ready(&self, _: ThumbnailReady) {}
    fn failed(&self, e: ThumbnailFailed) {
        eprintln!("preview failed: image {}: {}", e.image_id, e.reason);
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
    fn progress(&self, _: ImportProgress) {}
}

impl AnalysisSink for Counter {
    fn ready(&self, _: AnalysisReady) {}
    fn failed(&self, e: AnalysisFailed) {
        eprintln!("analysis failed: image {}: {}", e.image_id, e.reason);
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

// ---------------------------------------------------------------------------
// Sidecars
// ---------------------------------------------------------------------------

/// Value of an XMP property in attribute (`ns:Name="v"`) or element (`<ns:Name>v</ns:Name>`) form.
fn xmp_value(xmp: &str, prop: &str) -> Option<String> {
    let attr = format!("{prop}=\"");
    if let Some(i) = xmp.find(&attr) {
        let rest = &xmp[i + attr.len()..];
        return rest.find('"').map(|j| rest[..j].trim().to_owned());
    }
    let open = format!("<{prop}>");
    let i = xmp.find(&open)?;
    let rest = &xmp[i + open.len()..];
    rest.find('<').map(|j| rest[..j].trim().to_owned())
}

#[derive(Debug, Clone, Default)]
struct Sidecar {
    pick: Option<i32>,
    rating: Option<i32>,
    label: Option<String>,
    captured_ms: Option<i64>,
}

/// Days from 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2024-06-01T14:03:22.45+02:00" -> wall-clock ms as if UTC (timezone ignored).
fn parse_xmp_time(v: &str) -> Option<i64> {
    let num = |s: &str| s.parse::<i64>().ok();
    let (date, time) = v.split_once('T').unwrap_or((v, "00:00:00"));
    let mut d = date.split('-');
    let (y, mo, da) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    let time: String = time.chars().take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.').collect();
    let mut t = time.split(':');
    let h = t.next().and_then(num).unwrap_or(0);
    let mi = t.next().and_then(num).unwrap_or(0);
    let sec: f64 = t.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some(((days_from_civil(y, mo, da) * 24 + h) * 60 + mi) * 60_000 + (sec * 1000.0).round() as i64)
}

fn read_sidecar(dir: &Path, file_name: &str) -> Option<Sidecar> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let candidates =
        [format!("{stem}.xmp"), format!("{stem}.XMP"), format!("{file_name}.xmp"), format!("{file_name}.XMP")];
    let path = candidates.iter().map(|c| dir.join(c)).find(|p| p.exists())?;
    let xmp = std::fs::read_to_string(path).ok()?;
    let int = |p: &str| xmp_value(&xmp, p).and_then(|v| v.parse::<i32>().ok());
    let captured_ms = ["exif:DateTimeOriginal", "photoshop:DateCreated", "xmp:CreateDate"]
        .iter()
        .find_map(|p| xmp_value(&xmp, p).and_then(|v| parse_xmp_time(&v)));
    Some(Sidecar {
        pick: int("xmpDM:pick"),
        rating: int("xmp:Rating"),
        label: xmp_value(&xmp, "xmp:Label"),
        captured_ms,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Label {
    Keeper,
    Rejected,
    Other,
}

// ---------------------------------------------------------------------------
// Report helpers
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Agree {
    n: u32,
    keepers: u32,
    delivered: u32,
    both: u32,
    in_reach: u32,
}

impl Agree {
    fn add(&mut self, keeper: bool, choice: TargetChoice) {
        let delivered = choice == TargetChoice::Deliver;
        self.n += 1;
        self.keepers += u32::from(keeper);
        self.delivered += u32::from(delivered);
        self.both += u32::from(keeper && delivered);
        self.in_reach += u32::from(keeper && matches!(choice, TargetChoice::Deliver | TargetChoice::Alternative));
    }

    fn line(&self, name: &str) -> String {
        let p = self.both as f64 / self.delivered.max(1) as f64;
        let r = self.both as f64 / self.keepers.max(1) as f64;
        let f1 = if p + r > 0.0 { 2.0 * p * r / (p + r) } else { 0.0 };
        let agree = self.n - (self.keepers - self.both) - (self.delivered - self.both);
        format!(
            "{name:<10} n={:<5} keepers={:<5} delivered={:<5} both={:<5} precision={:>5.1}% recall={:>5.1}% F1={:.3} \
             agreement={:>5.1}% keepers within one key={:>5.1}%",
            self.n,
            self.keepers,
            self.delivered,
            self.both,
            p * 100.0,
            r * 100.0,
            f1,
            agree as f64 / self.n.max(1) as f64 * 100.0,
            self.in_reach as f64 / self.keepers.max(1) as f64 * 100.0
        )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(folder) = args.get(1).filter(|a| !a.starts_with("--")).map(PathBuf::from) else {
        eprintln!("usage: target_eval <folder of JPEG previews + .xmp> [--target N] [--shoot TYPE] [--work DIR] ...");
        std::process::exit(2);
    };
    let folder = folder.canonicalize().expect("folder");
    let shoot = ShootType::parse(&arg(&args, "--shoot").unwrap_or_else(|| "wedding".into())).expect("shoot type");
    let reuse = args.iter().any(|a| a == "--reuse");
    let min_rating: i32 = arg(&args, "--min-rating").map_or(1, |v| v.parse().expect("--min-rating"));
    let list_n: usize = arg(&args, "--list").map_or(15, |v| v.parse().expect("--list"));
    let models_dir = std::env::var("SIEVE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));

    let tmp = tempfile::tempdir().unwrap();
    let work = arg(&args, "--work").map(PathBuf::from).unwrap_or_else(|| tmp.path().to_path_buf());
    std::fs::create_dir_all(&work).unwrap();
    assert!(!work.canonicalize().unwrap().starts_with(&folder), "--work must not be inside the photo folder");
    let catalog = work.join("target_eval.sqlite");
    let ingest = IngestConfig { catalog_path: catalog.clone(), cache_dir: work.join("cache") };

    let t_all = Instant::now();
    if !reuse {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", catalog.display()));
        }
    }
    let mut conn = db::open(&catalog).unwrap();
    if !reuse {
        let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: false };
        let summary = repo::import_folder(&mut conn, &folder, &opts).unwrap();
        println!("imported {} photos from {}", summary.added, folder.display());
        // The engine must not see the photographer's decisions.
        conn.execute("UPDATE images SET pick = 'unflagged', rating = 0, color_label = NULL", []).unwrap();
        let t = Instant::now();
        let stats = run_until_idle(&ingest, &Counter::default(), &AtomicBool::new(true)).unwrap();
        println!("previews: {} ({} failed) in {:.1}s", stats.done, stats.failed, t.elapsed().as_secs_f64());
    }
    let project = projects::list_projects(&conn).unwrap().into_iter().next().expect("a project");
    projects::set_project_shoot_type(&conn, project.id, shoot).unwrap();
    if let Some(level) = arg(&args, "--strictness") {
        let level = RejectStrictness::parse(&level).expect("conservative | balanced | aggressive");
        projects::set_project_reject_strictness(&conn, project.id, level).unwrap();
    }

    // Labels + capture times from the sidecars.
    let mut stmt = conn.prepare("SELECT id, file_name, captured_at_ms FROM images ORDER BY id").unwrap();
    let rows: Vec<(ImageId, String, Option<i64>)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    drop(stmt);
    let sidecars: HashMap<ImageId, Sidecar> =
        rows.iter().filter_map(|(id, name, _)| read_sidecar(&folder, name).map(|s| (*id, s))).collect();
    let mut times_from_xmp = 0;
    for (id, _, captured) in &rows {
        if captured.is_none() {
            if let Some(ms) = sidecars.get(id).and_then(|s| s.captured_ms) {
                conn.execute("UPDATE images SET captured_at_ms = ?2 WHERE id = ?1", rusqlite::params![id, ms]).unwrap();
                times_from_xmp += 1;
            }
        }
    }
    let any_pick = sidecars.values().any(|s| s.pick == Some(1) || s.label.as_deref() == Some("Pick"));
    let label_of = |id: ImageId| -> Label {
        let Some(s) = sidecars.get(&id) else { return Label::Other };
        if s.pick == Some(-1) || s.rating == Some(-1) {
            Label::Rejected
        } else if s.pick == Some(1)
            || s.label.as_deref() == Some("Pick")
            || (!any_pick && s.rating.is_some_and(|r| r >= min_rating))
        {
            Label::Keeper
        } else {
            Label::Other
        }
    };
    let labels: HashMap<ImageId, Label> = rows.iter().map(|(id, _, _)| (*id, label_of(*id))).collect();
    let keepers = labels.values().filter(|&&l| l == Label::Keeper).count();
    let rejected = labels.values().filter(|&&l| l == Label::Rejected).count();
    let no_time = rows.iter().filter(|r| r.2.is_none()).count() - times_from_xmp;
    println!(
        "photos {}: sidecars {}, keepers {keepers} ({}), rejected {rejected}; capture time from sidecar {times_from_xmp}, \
         none {no_time}",
        rows.len(),
        sidecars.len(),
        if any_pick { "pick flags".to_owned() } else { format!("rating >= {min_rating}") }
    );

    if !reuse {
        let t = Instant::now();
        let config = AnalysisConfig { catalog_path: catalog.clone(), models_dir: models_dir.clone() };
        let stats = worker::run_blocking(&config, &Counter::default()).unwrap();
        let secs = t.elapsed().as_secs_f64();
        println!(
            "analysis: {} ({} failed) in {secs:.1}s ({:.0} ms/photo)",
            stats.analyzed,
            stats.failed,
            secs * 1000.0 / stats.analyzed.max(1) as f64
        );
        if stats.analyzed == 0 {
            eprintln!(
                "no photo was analysed: install the culling models (scripts/fetch-models.sh) or set SIEVE_MODELS \
                 (now {}); every photo will read 'not analysed'",
                models_dir.display()
            );
        }
    } else {
        worker::rescore_all(&mut conn).unwrap();
    }

    let target_count: u32 = arg(&args, "--target").map_or((keepers as u32).max(1), |v| v.parse().expect("--target"));
    let job = TargetJob { project_id: project.id, target_count, shoot_type: shoot };
    let t = Instant::now();
    let out = run_pipeline(&mut conn, &models_dir, &job, &AtomicBool::new(false), &mut |label, done, total| {
        if done == 0 || total.is_some_and(|t| done == t) {
            println!("  {label} {done}/{}", total.map_or("?".into(), |t| t.to_string()));
        }
    })
    .unwrap();
    println!(
        "selection (target {target_count}): {} in {:.2}s [{}]",
        out.message.unwrap_or_default(),
        t.elapsed().as_secs_f64(),
        out.model_version
    );

    // Compare.
    let ids: Vec<ImageId> = rows.iter().map(|r| r.0).collect();
    let selections: HashMap<ImageId, ImageSelection> =
        target::selections(&conn, &ids).unwrap().into_iter().map(|s| (s.image_id, s)).collect();
    let names: HashMap<ImageId, &str> = rows.iter().map(|r| (r.0, r.1.as_str())).collect();
    let mut overall = Agree::default();
    let mut per_type: BTreeMap<String, Agree> = BTreeMap::new();
    let mut keeper_choice: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut extra_reason: BTreeMap<String, u32> = BTreeMap::new();
    let mut rejected_delivered = 0;
    let mut missed: Vec<(ImageId, String)> = Vec::new();
    for &id in &ids {
        let Some(s) = selections.get(&id) else { continue };
        let keeper = labels[&id] == Label::Keeper;
        overall.add(keeper, s.choice);
        per_type.entry(s.shot_type.map_or("none", |t| t.as_str()).to_owned()).or_default().add(keeper, s.choice);
        let first = s.reasons.first().map_or_else(|| "-".to_owned(), |r| format!("{}: {}", r.kind.as_str(), r.text));
        if keeper && s.choice != TargetChoice::Deliver {
            let kind = s.reasons.first().map_or("-", |r| r.kind.as_str()).to_owned();
            *keeper_choice.entry((s.choice.as_str().to_owned(), kind)).or_default() += 1;
            if s.choice == TargetChoice::SetAside {
                missed.push((id, first.clone()));
            }
        }
        if !keeper && s.choice == TargetChoice::Deliver {
            *extra_reason.entry(s.reasons.first().map_or("-", |r| r.kind.as_str()).to_owned()).or_default() += 1;
        }
        rejected_delivered += u32::from(labels[&id] == Label::Rejected && s.choice == TargetChoice::Deliver);
    }
    println!("\n== Agreement (delivered vs keeper) ==");
    println!("{}", overall.line("overall"));
    for (t, a) in &per_type {
        println!("{}", a.line(t));
    }
    println!("photographer's rejects delivered: {rejected_delivered}");
    println!("\nkeepers not delivered, by choice / first reason:");
    for ((choice, kind), n) in &keeper_choice {
        println!("  {choice:<12} {kind:<20} {n}");
    }
    println!("delivered non-keepers, by first reason:");
    for (kind, n) in &extra_reason {
        println!("  {kind:<20} {n}");
    }

    // Per moment.
    let moments = target::list_moments(&conn, project.id).unwrap();
    let mut with_keepers = 0;
    let mut covered = 0;
    let mut empty_delivered = 0;
    let mut abs_err = 0u32;
    let mut per_moment_type: BTreeMap<&str, (u32, u32, u32)> = BTreeMap::new();
    let mut worst: Vec<(u32, String)> = Vec::new();
    for m in &moments {
        let k = m.image_ids.iter().filter(|id| labels.get(id) == Some(&Label::Keeper)).count() as u32;
        let d = m.delivered_ids.len() as u32;
        with_keepers += u32::from(k > 0);
        covered += u32::from(k > 0 && d > 0);
        empty_delivered += u32::from(k == 0 && d > 0);
        abs_err += k.abs_diff(d);
        let e = per_moment_type.entry(m.shot_type.as_str()).or_default();
        e.0 += 1;
        e.1 += k;
        e.2 += d;
        let first = m.image_ids.first().and_then(|id| names.get(id)).copied().unwrap_or("?");
        let last = m.image_ids.last().and_then(|id| names.get(id)).copied().unwrap_or("?");
        worst.push((
            k.abs_diff(d),
            format!(
                "{} {first}..{last} ({} frames): keepers {k}, delivered {d}",
                m.shot_type.as_str(),
                m.image_ids.len()
            ),
        ));
    }
    println!("\n== Moments ==");
    println!(
        "moments {}: with keepers {with_keepers}, covered by a delivery {covered} ({:.1}%), delivered without keepers \
         {empty_delivered}, mean |keepers - delivered| {:.2}",
        moments.len(),
        covered as f64 / with_keepers.max(1) as f64 * 100.0,
        abs_err as f64 / moments.len().max(1) as f64
    );
    for (t, (n, k, d)) in &per_moment_type {
        println!("  {t:<8} moments {n:<5} keepers {k:<5} delivered {d}");
    }
    worst.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    println!("worst moments:");
    for (_, w) in worst.iter().take(list_n) {
        println!("  {w}");
    }
    println!("keepers set aside:");
    for (id, r) in missed.iter().take(list_n) {
        println!("  {} {r}", names[id]);
    }

    if let Some(path) = arg(&args, "--csv") {
        let mut csv = String::from("file,label,choice,shot_type,moment,rank,covered_by,score,reasons\n");
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        for &id in &ids {
            let Some(s) = selections.get(&id) else { continue };
            let reasons: Vec<String> = s.reasons.iter().map(|r| r.text.clone()).collect();
            csv.push_str(&format!(
                "{},{:?},{},{},{},{},{},{:.3},{}\n",
                quote(names[&id]),
                labels[&id],
                s.choice.as_str(),
                s.shot_type.map_or("", |t| t.as_str()),
                s.moment_id.map_or(String::new(), |m| m.to_string()),
                s.rank.map_or(String::new(), |r| r.to_string()),
                s.covered_by.and_then(|c| names.get(&c)).copied().unwrap_or(""),
                s.score,
                quote(&reasons.join("; "))
            ));
        }
        std::fs::write(&path, csv).expect("write csv");
        println!("\nwrote {path}");
    }
    println!("\ntotal {:.1}s", t_all.elapsed().as_secs_f64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_values_and_times() {
        let x = r#"<rdf:Description xmpDM:pick="1" xmp:Rating="3"><exif:DateTimeOriginal>2024-06-01T14:03:22.45+02:00</exif:DateTimeOriginal>"#;
        assert_eq!(xmp_value(x, "xmpDM:pick").as_deref(), Some("1"));
        assert_eq!(xmp_value(x, "xmp:Rating").as_deref(), Some("3"));
        let ms = parse_xmp_time(&xmp_value(x, "exif:DateTimeOriginal").unwrap()).unwrap();
        assert_eq!(ms, 1_717_250_602_450);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }
}
