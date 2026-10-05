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
