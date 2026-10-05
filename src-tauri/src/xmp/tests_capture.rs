//! Capture time in sidecars (IPC v19): Lightroom-corrected times are read; Sieve corrections
//! are written in Lightroom's form and reverted without touching anything else.

use std::fs;
use std::path::PathBuf;

use rusqlite::{params, Connection};

use super::packet::{format_xmp_datetime, parse, parse_xmp_datetime};
use super::{XmpSync, XmpSyncConfig};
use crate::db;
use crate::ipc::types::{CaptureTimeEdit, CaptureTimeSource, ImageId};

const UPRIGHT: &str = include_str!("fixtures/lightroom_upright.xmp");
const H: i64 = 3_600_000;

fn naive(s: &str) -> i64 {
    crate::raw::meta::parse_exif_datetime(s, None).unwrap()
}

#[test]
fn xmp_dates_parse_as_naive_wall_clock() {
    let base = naive("2026:09:18 17:45:46");
    assert_eq!(parse_xmp_datetime("2026-09-18T17:45:46.500-07:00"), Some(base + 500));
    assert_eq!(parse_xmp_datetime("2026-09-18T17:45:46.05+05:30"), Some(base + 50));
    assert_eq!(parse_xmp_datetime("2026-09-18T17:45:46Z"), Some(base));
    assert_eq!(parse_xmp_datetime("2026-09-18T17:45:46"), Some(base));
    assert_eq!(parse_xmp_datetime("2026-09-18T17:45-07:00"), Some(base - 46_000));
    assert_eq!(parse_xmp_datetime("2026-09-18"), None, "date only: no time to use");
    assert_eq!(parse_xmp_datetime("garbage"), None);
    assert_eq!(format_xmp_datetime(base + 500, "-07:00"), "2026-09-18T17:45:46.500-07:00");
    assert_eq!(format_xmp_datetime(base, ""), "2026-09-18T17:45:46");
    assert_eq!(format_xmp_datetime(base + 7, "Z"), "2026-09-18T17:45:46.007Z");
    // Sidecar values: exif:DateTimeOriginal first, else photoshop:DateCreated.
    assert_eq!(parse(UPRIGHT).unwrap().capture_time_ms, Some(naive("2026:09:18 18:45:46") + 500));
    let only_ps = UPRIGHT.replace("   exif:DateTimeOriginal=\"2026-09-18T18:45:46.500-07:00\"\n", "");
    let only_ps = only_ps
        .replace("photoshop:DateCreated=\"2026-09-18T18:45:46.500", "photoshop:DateCreated=\"2026-09-18T18:40:00.000");
    assert_eq!(parse(&only_ps).unwrap().capture_time_ms, Some(naive("2026:09:18 18:40:00")));
}

struct Shoot {
    dir: tempfile::TempDir,
    sync: XmpSync,
    ids: Vec<ImageId>,
}

impl Shoot {
    /// Fake RAW rows with these EXIF times (naive ms).
    fn new(times: &[i64]) -> Shoot {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        let conn = db::open(&catalog).unwrap();
        let shoot = dir.path().join("shoot");
        fs::create_dir(&shoot).unwrap();
        conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, ?1, 0)", [shoot.to_str().unwrap()]).unwrap();
        let mut ids = Vec::new();
        for (i, t) in times.iter().enumerate() {
            let raw = shoot.join(format!("IMG{i}.RAF"));
            fs::write(&raw, b"raw").unwrap();
            conn.execute(
                "INSERT INTO images (folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                     imported_at, captured_at_ms, exif_captured_at_ms)
                 VALUES (1, ?1, ?2, 'raf', 'fujifilm', 3, 0, 0, ?3, ?3)",
                params![raw.to_str().unwrap(), format!("IMG{i}.RAF"), t],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();
        let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog });
        Shoot { dir, sync, ids }
    }

    fn conn(&self) -> Connection {
        db::open(&self.sync.config().catalog_path).unwrap()
    }

    fn sidecar(&self, i: usize) -> PathBuf {
        self.dir.path().join("shoot").join(format!("IMG{i}.xmp"))
    }

    fn time(&self, id: ImageId) -> (Option<i64>, Option<i64>, CaptureTimeSource) {
        let e = db::repo::get_image(&self.conn(), id).unwrap();
        (e.capture.captured_at_ms, e.capture.original_captured_at_ms, e.capture.capture_time_source)
    }

    /// Ids by corrected capture time.
    fn order(&self) -> Vec<ImageId> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM images ORDER BY captured_at_ms, id").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    }
}

/// A sidecar Lightroom shifted by +1 h imports with the corrected time (source `sidecar`) and
/// re-sorts the shoot; the file's EXIF time is kept as the original.
#[test]
fn lightroom_shifted_time_is_read() {
    let exif = naive("2026:09:18 17:45:46") + 500;
    let other = naive("2026:09:18 18:00:00");
    let s = Shoot::new(&[exif, other]);
    assert_eq!(s.order(), vec![s.ids[0], s.ids[1]]);
    fs::write(s.sidecar(0), UPRIGHT).unwrap();
    let report = s.sync.read_images(&s.ids).unwrap();
    assert_eq!(report.changed, vec![s.ids[0]]);
    assert!(s.sync.take_capture_time_changes() && !s.sync.take_capture_time_changes());
    assert_eq!(s.time(s.ids[0]), (Some(exif + H), Some(exif), CaptureTimeSource::Sidecar));
    assert_eq!(s.order(), vec![s.ids[1], s.ids[0]], "the corrected time sorts after the other camera");
    // Reading it again changes nothing; a sidecar equal to EXIF goes back to `exif`.
    assert!(s.sync.read_images(&s.ids[..1]).unwrap().changed.is_empty());
    fs::write(s.sidecar(0), UPRIGHT.replace("T18:45:46.500", "T17:45:46.500")).unwrap();
    s.sync.read_images(&s.ids[..1]).unwrap();
    assert_eq!(s.time(s.ids[0]), (Some(exif), Some(exif), CaptureTimeSource::Exif));
}

/// Shift -1 h -> both dates written (Lightroom form, zone kept), nothing else changes;
/// revert -> back to the EXIF time; a sidecar Sieve created loses exactly what it added.
#[test]
fn corrections_are_written_and_reverted() {
    let exif = naive("2026:09:18 18:45:46") + 500;
    let s = Shoot::new(&[exif, exif + 1000]);
    // Image 0: Lightroom sidecar with its own (equal) dates. Image 1: no sidecar.
    fs::write(s.sidecar(0), UPRIGHT).unwrap();
    s.sync.read_images(&s.ids[..1]).unwrap();
    assert_eq!(s.time(s.ids[0]).2, CaptureTimeSource::Exif);
    // An unrelated write leaves the dates byte-for-byte.
    s.sync.write_images(&s.ids[..1]).unwrap();
    let unchanged = fs::read_to_string(s.sidecar(0)).unwrap();
    for line in UPRIGHT.lines().filter(|l| l.contains("Date") && !l.contains("MetadataDate")) {
        assert!(unchanged.contains(line), "{line}");
    }
    assert!(!unchanged.contains("CaptureTimeAdded"));

    let r = db::capture_time::edit(&mut s.conn(), &s.ids, &CaptureTimeEdit::Shift { offset_ms: -H }).unwrap();
    assert_eq!(r.changed_ids, s.ids);
    s.sync.write_dirty(None).unwrap();
    let lr = fs::read_to_string(s.sidecar(0)).unwrap();
    assert!(lr.contains("exif:DateTimeOriginal=\"2026-09-18T17:45:46.500-07:00\""), "{lr}");
    assert!(lr.contains("photoshop:DateCreated=\"2026-09-18T17:45:46.500-07:00\""));
    assert!(lr.contains("xmp:CreateDate=\"2026-09-18T18:45:46.500-07:00\""), "unrelated date untouched");
    assert!(!lr.contains("CaptureTimeAdded"), "Lightroom's own properties were edited, not added");
    let fresh = fs::read_to_string(s.sidecar(1)).unwrap();
    assert!(fresh.contains("exif:DateTimeOriginal=\"2026-09-18T17:45:47.500\""), "{fresh}");
    assert!(fresh.contains("sieve:CaptureTimeAdded=\"DateTimeOriginal,DateCreated\""));
    // Reading the written sidecars back keeps the corrected times.
    s.sync.read_images(&s.ids).unwrap();
    assert_eq!(s.time(s.ids[0]), (Some(exif - H), Some(exif), CaptureTimeSource::Sidecar));
    assert_eq!(s.time(s.ids[1]).0, Some(exif + 1000 - H));

    // Revert: Lightroom's values go back to the EXIF time, Sieve's additions are removed.
    db::capture_time::edit(&mut s.conn(), &s.ids, &CaptureTimeEdit::Revert).unwrap();
    s.sync.write_dirty(None).unwrap();
    let lr = fs::read_to_string(s.sidecar(0)).unwrap();
    let base = |t: &str| t.lines().filter(|l| !l.contains("MetadataDate")).collect::<Vec<_>>().join("\n");
    assert_eq!(base(&lr), base(&unchanged), "restored byte-for-byte (but the metadata date)");
    let fresh = fs::read_to_string(s.sidecar(1)).unwrap();
    assert!(
        !fresh.contains("DateTimeOriginal") && !fresh.contains("DateCreated") && !fresh.contains("CaptureTimeAdded")
    );
    s.sync.read_images(&s.ids).unwrap();
    assert_eq!(s.time(s.ids[0]), (Some(exif), Some(exif), CaptureTimeSource::Exif));
    assert_eq!(s.time(s.ids[1]), (Some(exif + 1000), Some(exif + 1000), CaptureTimeSource::Exif));
}

/// Two-camera acceptance on real copies (`$SIEVE_CAPTURE_SET`: a folder of RAW copies + their
/// Lightroom sidecars, e.g. under `test-data/`; `$SIEVE_CAPTURE_ORIGINALS`: `:`-separated
/// folders holding the originals, compared byte-for-byte). Import, extract EXIF, read
/// sidecars; shift the Canon frames -1 h (exiftool shows the written time, zone kept); sync the
/// Canon to the Fuji timeline (sort order changes); undo both (times, sidecars restored).
/// `SIEVE_CAPTURE_SET=... cargo test --lib two_camera_capture_time -- --ignored --nocapture`
#[test]
#[ignore = "needs $SIEVE_CAPTURE_SET (copies of real RAWs + sidecars)"]
fn two_camera_capture_time_on_real_raws() {
    use crate::db::repo;
    use crate::ipc::types::ImportOptions;
    let set = PathBuf::from(std::env::var("SIEVE_CAPTURE_SET").expect("SIEVE_CAPTURE_SET"));
    let originals: Vec<PathBuf> =
        std::env::var("SIEVE_CAPTURE_ORIGINALS").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    let sidecars_before: Vec<(PathBuf, String)> = fs::read_dir(&set)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
        .map(|p| (p.clone(), fs::read_to_string(&p).unwrap()))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let catalog = dir.path().join("cat.sqlite");
    let mut conn = db::open(&catalog).unwrap();
    let summary = repo::import_folder(&mut conn, &set, &ImportOptions::raw_only(false)).unwrap();
    let rows: Vec<(ImageId, PathBuf)> = {
        let mut stmt = conn.prepare("SELECT id, path FROM images ORDER BY file_name").unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?))))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    // Import reads sidecars first, then ingest extracts the EXIF (as the app does).
    let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog.clone() });
    sync.refresh_folder(summary.folder_id).unwrap();
    for (id, path) in &rows {
        let format = crate::raw::format_from_extension(path).unwrap();
        let ex = crate::raw::extract(path, format, &mut Vec::new()).unwrap();
        repo::record_extraction(&mut conn, *id, Some(&ex.meta), Err("test")).unwrap();
    }
    let ext = |p: &PathBuf| p.extension().unwrap().to_string_lossy().to_ascii_uppercase();
    let ids_of = |e: &str| rows.iter().filter(|(_, p)| ext(p) == e).map(|(id, _)| *id).collect::<Vec<_>>();
    let (canon, fuji) = (ids_of("CR3"), ids_of("RAF"));
    assert!(!canon.is_empty() && !fuji.is_empty());
    let order = |conn: &Connection| -> Vec<(String, Option<i64>, String)> {
        let mut stmt = conn
            .prepare("SELECT file_name, captured_at_ms, capture_time_source FROM images ORDER BY captured_at_ms, id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    let show = |label: &str, o: &[(String, Option<i64>, String)]| {
        println!("{label}:");
        for (n, t, s) in o {
            println!("  {n:16} {:28} {s}", t.map(|t| format_xmp_datetime(t, "")).unwrap_or_default());
        }
    };
    let before = order(&conn);
    show("imported (sort by capture time)", &before);
    assert!(before.iter().all(|(_, _, s)| s == "exif"), "Lightroom sidecars equal to EXIF are no correction");

    // Shift the Canon -1 h.
    let shift = db::capture_time::edit(&mut conn, &canon, &CaptureTimeEdit::Shift { offset_ms: -H }).unwrap();
    assert_eq!(shift.changed_ids, canon);
    sync.write_dirty(None).unwrap();
    let after_shift = order(&conn);
    show("Canon shifted -1 h", &after_shift);
    let exif_json = |paths: Vec<PathBuf>| {
        let out = std::process::Command::new("exiftool")
            .args(["-j", "-XMP-exif:DateTimeOriginal", "-XMP-photoshop:DateCreated", "-EXIF:DateTimeOriginal"])
            .args(&paths)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let canon_xmp: Vec<PathBuf> =
        rows.iter().filter(|(id, _)| canon.contains(id)).map(|(_, p)| super::resolve_sidecar(p)).collect();
    let canon_raws: Vec<PathBuf> = rows.iter().filter(|(id, _)| canon.contains(id)).map(|(_, p)| p.clone()).collect();
    println!("exiftool, Canon sidecars after the shift:\n{}", exif_json(canon_xmp.clone()));
    println!("exiftool, Canon originals (EXIF):\n{}", exif_json(canon_raws));
    for (id, p) in rows.iter().filter(|(id, _)| canon.contains(id)) {
        let (cur, exif, _) = (|| {
            let e = repo::get_image(&conn, *id).unwrap();
            (e.capture.captured_at_ms, e.capture.original_captured_at_ms, e.capture.capture_time_source)
        })();
        assert_eq!(cur.unwrap(), exif.unwrap() - H);
        let v = parse(&fs::read_to_string(super::resolve_sidecar(p)).unwrap()).unwrap();
        assert_eq!(v.capture_time_ms, cur, "{}", p.display());
    }

    // Sync the Canon to the Fuji timeline: first Canon frame = first Fuji frame.
    let sync_r = db::capture_time::edit(
        &mut conn,
        &canon,
        &CaptureTimeEdit::SyncCameras { reference_id: fuji[0], target_id: canon[0] },
    )
    .unwrap();
    sync.write_dirty(None).unwrap();
    let after_sync = order(&conn);
    show("Canon synced to the Fuji clock", &after_sync);
    let names = |o: &[(String, Option<i64>, String)]| o.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    assert_ne!(names(&after_sync), names(&before), "sort order changed");

    // Undo both (as the UI does with `previous`).
    db::capture_time::restore(&mut conn, &sync_r.previous).unwrap();
    db::capture_time::restore(&mut conn, &shift.previous).unwrap();
    sync.write_dirty(None).unwrap();
    let restored = order(&conn);
    show("undone", &restored);
    assert_eq!(names(&restored), names(&before));
    assert_eq!(restored.iter().map(|x| x.1).collect::<Vec<_>>(), before.iter().map(|x| x.1).collect::<Vec<_>>());
    println!("exiftool, Canon sidecars after undo:\n{}", exif_json(canon_xmp));
    // Lightroom sidecars: back to their original text (but xmp:MetadataDate); originals intact.
    let strip = |t: &str| t.lines().filter(|l| !l.contains("MetadataDate")).collect::<Vec<_>>().join("\n");
    for (p, text) in &sidecars_before {
        assert_eq!(strip(&fs::read_to_string(p).unwrap()), strip(text), "{}", p.display());
    }
    for (_, p) in &rows {
        if let Some(orig) = originals.iter().map(|d| d.join(p.file_name().unwrap())).find(|o| o.exists()) {
            assert!(fs::read(&orig).unwrap() == fs::read(p).unwrap(), "{} differs from the original", p.display());
            println!("byte-identical to the original: {}", p.file_name().unwrap().to_string_lossy());
        }
    }
}
