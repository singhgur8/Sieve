use std::sync::mpsc;

use rusqlite::params;

use super::packet::{self, merge, parse, Desired};
use super::*;
use crate::ipc::types::{CullTag, ImportOptions};

const LIGHTROOM: &str = include_str!("fixtures/lightroom.xmp");
const BRIDGE: &str = include_str!("fixtures/bridge.xmp");
const CAPTURE_ONE: &str = include_str!("fixtures/captureone.xmp");
const DARKTABLE: &str = include_str!("fixtures/darktable.xmp");
const DATE: &str = "2026-09-29T12:00:00Z";

fn want(rating: i32, label: Option<&'static str>, tags: &[&str]) -> Desired {
    Desired {
        rating,
        label,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        metadata_date: DATE.into(),
        develop: Vec::new(),
        seqs: Vec::new(),
        profile: None,
    }
}

/// Every line of `orig` not mentioning a field we own must appear in `out`, in order.
fn assert_unrelated_preserved(orig: &str, out: &str, owned: &[&str]) {
    let mut out_lines = out.lines();
    for line in orig.lines() {
        if owned.iter().any(|o| line.contains(o)) {
            continue;
        }
        let stem = line.trim_end_matches("/>").trim_end_matches('>');
        assert!(
            out_lines.any(|l| l == line || (stem.len() > 1 && l.starts_with(stem))),
            "line lost or reordered: {line:?}\n--- output ---\n{out}"
        );
    }
}

const OWNED: &[&str] = &["Rating", "Label", "MetadataDate", "Sieve|", "<rdf:li>blink<", "<rdf:li>motion_blur<"];

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_lightroom_attribute_sidecar() {
    let v = parse(LIGHTROOM).unwrap();
    assert_eq!(v.rating, Some(3));
    assert_eq!(v.label.as_deref(), Some("Red"));
    assert_eq!(v.subjects, ["wedding", "Smith & Jones", "blink", "motion_blur"]);
    assert_eq!(
        v.hierarchical_subjects,
        ["Events|wedding", "Clients|Smith & Jones", "Sieve|blink", "Sieve|motion_blur"]
    );
}

#[test]
fn parses_element_style_and_custom_prefixes() {
    let b = parse(BRIDGE).unwrap();
    assert_eq!((b.rating, b.label.as_deref()), (Some(1), Some("Approved")));
    let c = parse(CAPTURE_ONE).unwrap();
    assert_eq!((c.rating, c.label.as_deref()), (Some(2), Some("Purple")));
    assert_eq!(c.subjects, ["travel", "portrait"]);
    let d = parse(DARKTABLE).unwrap();
    assert_eq!(d.rating, Some(4), "xap: prefix bound to the xmp namespace");
    assert_eq!(d.label, None);
    assert_eq!(d.hierarchical_subjects, ["darktable|format|ARW", "Sieve|underexposed"]);
}

#[test]
fn rejects_malformed_xml() {
    assert!(parse("<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF>").is_err());
    assert!(merge(Some("<a><b></a>"), &want(1, None, &[])).is_err());
    assert!(merge(Some("not xml at all"), &want(1, None, &[])).is_err());
}

// ---------------------------------------------------------------------------
// Merging
// ---------------------------------------------------------------------------

#[test]
fn lightroom_merge_updates_owned_fields_only() {
    let out = merge(Some(LIGHTROOM), &want(5, Some("Pick"), &["blink", "missed_focus"])).unwrap();
    assert_unrelated_preserved(LIGHTROOM, &out, OWNED);
    // Attribute form kept in place.
    assert!(out.contains("   xmp:Rating=\"5\"\n   xmp:Label=\"Pick\"\n   xmp:MetadataDate=\"2026-09-29T12:00:00Z\"\n"));
    assert!(out.contains("crs:Exposure2012=\"+0.35\""));
    assert!(out.contains("<rdf:li>Smith &amp; Jones</rdf:li>"));
    let v = parse(&out).unwrap();
    assert_eq!(v.rating, Some(5));
    assert_eq!(v.label.as_deref(), Some("Pick"));
    assert_eq!(
        v.hierarchical_subjects,
        ["Events|wedding", "Clients|Smith & Jones", "Sieve|blink", "Sieve|missed_focus"]
    );
    // motion_blur leaf removed with its Sieve item; blink kept (still wanted).
    assert_eq!(v.subjects, ["wedding", "Smith & Jones", "blink", "missed_focus"]);
    // New items use the existing indentation.
    assert!(out.contains("     <rdf:li>Sieve|missed_focus</rdf:li>\n    </rdf:Bag>"));
}

#[test]
fn merge_is_idempotent_and_minimal() {
    let w = want(3, Some("Red"), &["blink", "motion_blur"]);
    let once = merge(Some(LIGHTROOM), &w).unwrap();
    // Only the metadata date differs from the Lightroom original.
    assert_eq!(once, LIGHTROOM.replace("2026-06-20T10:01:44+02:00\"\n   aux", "2026-09-29T12:00:00Z\"\n   aux"));
    assert_eq!(merge(Some(&once), &w).unwrap(), once);
    for fixture in [BRIDGE, CAPTURE_ONE, DARKTABLE] {
        let w = want(-1, Some("Blue"), &["underexposed"]);
        let a = merge(Some(fixture), &w).unwrap();
        assert_eq!(merge(Some(&a), &w).unwrap(), a);
    }
}

#[test]
fn removes_only_our_keywords_and_labels() {
    let out = merge(Some(LIGHTROOM), &want(0, None, &[])).unwrap();
    assert_unrelated_preserved(LIGHTROOM, &out, OWNED);
    let v = parse(&out).unwrap();
    assert_eq!(v.rating, Some(0));
    assert_eq!(v.label, None, "Red is one of our labels");
    assert_eq!(v.hierarchical_subjects, ["Events|wedding", "Clients|Smith & Jones"]);
    assert_eq!(v.subjects, ["wedding", "Smith & Jones"]);
    assert!(!out.contains("xmp:Label"));

    // A custom label (Bridge "Approved") is not ours: kept.
    let out = merge(Some(BRIDGE), &want(2, None, &[])).unwrap();
    assert_eq!(parse(&out).unwrap().label.as_deref(), Some("Approved"));
}

#[test]
fn leaf_shared_with_foreign_hierarchy_is_kept() {
    let src = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:lr="http://ns.adobe.com/lightroom/1.0/">
<dc:subject><rdf:Bag><rdf:li>blink</rdf:li><rdf:li>x</rdf:li></rdf:Bag></dc:subject>
<lr:hierarchicalSubject><rdf:Bag><rdf:li>Mine|blink</rdf:li><rdf:li>Sieve|blink</rdf:li></rdf:Bag></lr:hierarchicalSubject>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let v = parse(&merge(Some(src), &want(0, None, &[])).unwrap()).unwrap();
    assert_eq!(v.hierarchical_subjects, ["Mine|blink"]);
    assert_eq!(v.subjects, ["blink", "x"]);
}

#[test]
fn element_style_sidecars_are_edited_in_place() {
    let out = merge(Some(CAPTURE_ONE), &want(-1, None, &["blink"])).unwrap();
    assert_unrelated_preserved(CAPTURE_ONE, &out, &["Rating", "Label"]);
    assert!(out.contains("<xmp:Rating>-1</xmp:Rating>"));
    assert!(!out.contains("Purple"), "Purple is ours and was cleared");
    let v = parse(&out).unwrap();
    assert_eq!(v.subjects, ["travel", "portrait", "blink"]);
    assert_eq!(v.hierarchical_subjects, ["Sieve|blink"]);
    // lr namespace was not declared: declared on the description.
    assert!(out.contains("xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\""));

    let out = merge(Some(BRIDGE), &want(4, Some("Green"), &[])).unwrap();
    assert_unrelated_preserved(BRIDGE, &out, &["Rating", "Label", "MetadataDate"]);
    assert!(out.contains("         <xmp:Rating>4</xmp:Rating>\n         <xmp:Label>Green</xmp:Label>\n"));
    assert!(out.contains("<xmp:MetadataDate>2026-09-29T12:00:00Z</xmp:MetadataDate>"));
    assert!(out.ends_with("<?xpacket end=\"w\"?>\n"), "packet trailer and padding kept");
}

#[test]
fn multiple_descriptions_and_foreign_prefixes() {
    let out = merge(Some(DARKTABLE), &want(2, Some("Yellow"), &["blink"])).unwrap();
    assert_unrelated_preserved(DARKTABLE, &out, &["Rating", "Sieve|"]);
    assert!(out.contains("xap:Rating='2'"), "existing prefix and quote style kept");
    assert!(out.contains("xap:Label=\"Yellow\""));
    let v = parse(&out).unwrap();
    assert_eq!(v.hierarchical_subjects, ["darktable|format|ARW", "Sieve|blink"]);
    assert_eq!(v.subjects, ["blink"]);
    assert_eq!(v.label.as_deref(), Some("Yellow"));
}

#[test]
fn creates_minimal_packet() {
    let out = merge(None, &want(4, Some("Pick"), &["duplicate_burst"])).unwrap();
    assert!(out.starts_with("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\""));
    let v = parse(&out).unwrap();
    assert_eq!(v.rating, Some(4));
    assert_eq!(v.label.as_deref(), Some("Pick"));
    assert_eq!(v.hierarchical_subjects, ["Sieve|duplicate_burst"]);
    assert_eq!(v.subjects, ["duplicate_burst"]);
    // No tags: no keyword properties at all.
    let bare = merge(None, &want(0, None, &[])).unwrap();
    assert!(!bare.contains("subject") && !bare.contains("Subject"));
    assert!(bare.contains("xmp:Rating=\"0\""));
}

#[test]
fn self_closing_description_and_missing_description() {
    let empty_desc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"1\" xmp:Nickname=\"n\"/></rdf:RDF></x:xmpmeta>";
    let out = merge(Some(empty_desc), &want(3, None, &["blink"])).unwrap();
    assert!(out.contains("xmp:Nickname=\"n\""));
    let v = parse(&out).unwrap();
    assert_eq!((v.rating, v.subjects.clone()), (Some(3), vec!["blink".to_string()]));

    let no_desc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n </rdf:RDF>\n</x:xmpmeta>\n";
    let v = parse(&merge(Some(no_desc), &want(2, Some("Red"), &[])).unwrap()).unwrap();
    assert_eq!((v.rating, v.label.as_deref()), (Some(2), Some("Red")));
}

#[test]
fn keeps_bom_and_escapes_values() {
    let src = format!("\u{feff}{}", CAPTURE_ONE);
    let out = merge(Some(&src), &want(1, None, &[])).unwrap();
    assert!(out.starts_with('\u{feff}'));
    assert_eq!(parse(&out).unwrap().rating, Some(1));
}

#[test]
fn owned_label_detection() {
    for l in ["Pick", "red", "Purple "] {
        assert!(packet::is_owned_label(l));
    }
    for l in ["Approved", "Second", "To Do", ""] {
        assert!(!packet::is_owned_label(l));
    }
}

// ---------------------------------------------------------------------------
// Mapping / policy helpers
// ---------------------------------------------------------------------------

fn row(rating: u8, pick: PickFlag, label: Option<ColorLabel>) -> ImageRow {
    ImageRow {
        id: 1,
        path: PathBuf::from("/x/a.ARW"),
        rating,
        pick,
        color_label: label,
        meta_updated_at: None,
        xmp_mtime_ms: None,
    }
}

#[test]
fn catalog_to_sidecar_mapping() {
    let d = desired(&row(4, PickFlag::Reject, Some(ColorLabel::Blue)), &[], None);
    assert_eq!((d.rating, d.label), (-1, Some("Blue")));
    let d = desired(&row(2, PickFlag::Pick, Some(ColorLabel::Blue)), &[], None);
    assert_eq!((d.rating, d.label), (2, Some("Pick")));
    let d = desired(&row(0, PickFlag::Unflagged, None), &["blink".into()], None);
    assert_eq!((d.rating, d.label, d.tags), (0, None, vec!["blink".to_string()]));
}

#[test]
fn sidecar_to_catalog_mapping() {
    let v = |rating: Option<i32>, label: Option<&str>| SidecarValues {
        rating,
        label: label.map(str::to_owned),
        ..Default::default()
    };
    assert_eq!(catalog_values(&v(Some(-1), Some("Red")), 3), (3, PickFlag::Reject, Some(ColorLabel::Red)));
    assert_eq!(catalog_values(&v(Some(4), Some("Pick")), 0), (4, PickFlag::Pick, None));
    assert_eq!(catalog_values(&v(Some(9), Some("purple")), 0), (5, PickFlag::Unflagged, Some(ColorLabel::Purple)));
    assert_eq!(catalog_values(&v(None, Some("Approved")), 2), (0, PickFlag::Unflagged, None));
}

#[test]
fn newer_wins_decisions() {
    assert_eq!(decide(None, Some(5), Some(10)), Action::Write, "no sidecar");
    assert_eq!(decide(Some(5), Some(5), Some(10)), Action::Write, "not modified externally");
    assert_eq!(decide(Some(20), Some(5), Some(10)), Action::Read(None), "sidecar newer");
    assert_eq!(decide(Some(7), Some(5), Some(10)), Action::Write, "catalog newer");
    assert_eq!(decide(Some(7), None, None), Action::Read(None));
}

#[test]
fn iso_dates() {
    assert_eq!(iso8601_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(UNIX_EPOCH + Duration::from_secs(1_790_000_000)), "2026-09-21T14:13:20Z");
    assert_eq!(iso8601_utc(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
}

/// Prints merged fixtures for manual review (`cargo test dump_merges -- --ignored --nocapture`).
#[test]
#[ignore]
fn dump_merges() {
    for f in [LIGHTROOM, BRIDGE, CAPTURE_ONE, DARKTABLE] {
        println!("=====\n{}", merge(Some(f), &want(-1, Some("Blue"), &["blink", "underexposed"])).unwrap());
    }
    println!("=====\n{}", merge(None, &want(2, None, &["blink"])).unwrap());
}

#[test]
fn sidecar_next_to_raw() {
    assert_eq!(sidecar_path(Path::new("/s/DSC0001.ARW")), PathBuf::from("/s/DSC0001.xmp"));
    assert_eq!(sidecar_path(Path::new("/s/a.b/IMG_1.CR3")), PathBuf::from("/s/a.b/IMG_1.xmp"));
}

// ---------------------------------------------------------------------------
// Catalog + files
// ---------------------------------------------------------------------------

struct Fixture {
    dir: tempfile::TempDir,
    sync: XmpSync,
    ids: Vec<ImageId>,
}

impl Fixture {
    /// Catalog with `n` fake RAW rows in one folder (file contents are never read).
    fn new(n: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        let conn = db::open(&catalog).unwrap();
        let shoot = dir.path().join("shoot");
        fs::create_dir(&shoot).unwrap();
        conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, ?1, 0)", [shoot.to_str().unwrap()]).unwrap();
        let mut ids = Vec::new();
        for i in 0..n {
            let raw = shoot.join(format!("DSC{i:04}.ARW"));
            fs::write(&raw, b"raw").unwrap();
            conn.execute(
                "INSERT INTO images (folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
                 VALUES (1, ?1, ?2, 'arw', 'sony', 3, 0, 0)",
                params![raw.to_str().unwrap(), format!("DSC{i:04}.ARW")],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog }).with_debounce(Duration::from_millis(50));
        Fixture { dir, sync, ids }
    }

    fn conn(&self) -> Connection {
        db::open(&self.sync.config().catalog_path).unwrap()
    }

    fn sidecar(&self, i: usize) -> PathBuf {
        self.dir.path().join("shoot").join(format!("DSC{i:04}.xmp"))
    }

    /// `(dirty, synced_at, mtime, error)`.
    fn state(&self, id: ImageId) -> (bool, Option<i64>, Option<i64>, Option<String>) {
        self.conn()
            .query_row(
                "SELECT xmp_dirty, xmp_synced_at, xmp_mtime_ms, xmp_error FROM images WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
    }

    fn values(&self, id: ImageId) -> (u8, PickFlag, Option<ColorLabel>) {
        let r = store::load(&self.conn(), id).unwrap().unwrap();
        (r.rating, r.pick, r.color_label)
    }
}

/// Sleeps long enough for file mtimes / trigger timestamps to move on.
fn tick() {
    std::thread::sleep(Duration::from_millis(15));
}

#[test]
fn write_images_creates_sidecars_and_clears_dirty() {
    let f = Fixture::new(3);
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids[..1], 4).unwrap();
    repo::set_pick(&mut conn, &f.ids[1..2], PickFlag::Reject).unwrap();
    repo::set_color_label(&mut conn, &f.ids[2..3], Some(ColorLabel::Green)).unwrap();
    repo::set_user_tag(&mut conn, &f.ids[..1], CullTag::Blink, true).unwrap();
    assert!(f.state(f.ids[0]).0);

    let report = f.sync.write_images(&f.ids).unwrap();
    assert_eq!((report.succeeded, report.skipped, report.failed.len()), (3, 0, 0));
    let v0 = parse(&fs::read_to_string(f.sidecar(0)).unwrap()).unwrap();
    assert_eq!((v0.rating, v0.label.as_deref()), (Some(4), None));
    assert_eq!(v0.hierarchical_subjects, ["Sieve|blink"]);
    assert_eq!(parse(&fs::read_to_string(f.sidecar(1)).unwrap()).unwrap().rating, Some(-1));
    assert_eq!(parse(&fs::read_to_string(f.sidecar(2)).unwrap()).unwrap().label.as_deref(), Some("Green"));
    for (i, &id) in f.ids.iter().enumerate() {
        let (dirty, synced, mtime, err) = f.state(id);
        assert!(!dirty && synced.is_some() && err.is_none());
        assert_eq!(mtime, file_mtime_ms(&f.sidecar(i)));
    }
    assert!(!f.dir.path().join("shoot/DSC0000.xmp.tmp").exists());
}

#[test]
fn write_dirty_writes_only_dirty_images_of_the_folder() {
    let f = Fixture::new(3);
    let mut conn = f.conn();
    assert_eq!(f.sync.write_dirty(None).unwrap(), XmpSyncReport::default());
    repo::set_rating(&mut conn, &[f.ids[0], f.ids[2]], 3).unwrap();
    assert_eq!(f.sync.write_dirty(Some(2)).unwrap().succeeded, 0, "other folder: nothing");
    let report = f.sync.write_dirty(Some(1)).unwrap();
    assert_eq!((report.succeeded, report.failed.len()), (2, 0));
    assert!(f.sidecar(0).exists() && !f.sidecar(1).exists() && f.sidecar(2).exists());
    assert!(f.ids.iter().all(|&id| !f.state(id).0));
    assert_eq!(f.sync.write_dirty(None).unwrap().succeeded, 0, "clean now");
}

#[test]
fn unknown_ids_fail_before_touching_files() {
    let f = Fixture::new(2);
    let err = f.sync.write_images(&[f.ids[0], 999]).unwrap_err();
    assert_eq!(err.kind, crate::ipc::error::ErrorKind::NotFound);
    assert!(!f.sidecar(0).exists());
    assert_eq!(f.sync.read_images(&[999]).unwrap_err().kind, crate::ipc::error::ErrorKind::NotFound);
}

#[test]
fn read_images_applies_sidecar_values() {
    let f = Fixture::new(3);
    fs::write(f.sidecar(0), LIGHTROOM).unwrap(); // Rating 3, Red
    fs::write(f.sidecar(1), CAPTURE_ONE.replace(">2</xmp:Rating>", ">-1</xmp:Rating>")).unwrap();
    let report = f.sync.read_images(&f.ids).unwrap();
    assert_eq!((report.succeeded, report.skipped), (2, 1));
    assert_eq!(report.changed, vec![f.ids[0], f.ids[1]]);
    assert_eq!(f.values(f.ids[0]), (3, PickFlag::Unflagged, Some(ColorLabel::Red)));
    assert_eq!(f.values(f.ids[1]), (0, PickFlag::Reject, Some(ColorLabel::Purple)));
    let (dirty, synced, mtime, _) = f.state(f.ids[0]);
    assert!(!dirty, "read clears the flag its own update re-set");
    assert!(synced.is_some());
    assert_eq!(mtime, file_mtime_ms(&f.sidecar(0)));
    // Tags are not read back.
    assert_eq!(store::visible_tags(&f.conn(), f.ids[0]).unwrap(), Vec::<String>::new());
    // Re-reading unchanged values reports no change.
    assert!(f.sync.read_images(&f.ids[..1]).unwrap().changed.is_empty());
}

#[test]
fn unparsable_sidecar_is_reported_and_not_clobbered() {
    let f = Fixture::new(1);
    fs::write(f.sidecar(0), "<x:xmpmeta><broken").unwrap();
    let report = f.sync.write_images(&f.ids).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read_to_string(f.sidecar(0)).unwrap(), "<x:xmpmeta><broken");
    assert!(f.state(f.ids[0]).3.is_some());
}

#[test]
fn refresh_folder_reads_new_and_externally_changed_sidecars() {
    let f = Fixture::new(3);
    fs::write(f.sidecar(0), LIGHTROOM).unwrap();
    fs::write(f.sidecar(2), BRIDGE).unwrap();
    assert_eq!(f.sync.refresh_folder(1).unwrap(), 2);
    assert_eq!(f.sync.refresh_folder(1).unwrap(), 0, "nothing changed on disk");
    assert_eq!(f.values(f.ids[2]).0, 1);

    tick();
    fs::write(f.sidecar(2), BRIDGE.replace(">1</xmp:Rating>", ">5</xmp:Rating>")).unwrap();
    // Dirty images are left alone (their catalog change is pending).
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids[..1], 1).unwrap();
    fs::write(f.sidecar(0), LIGHTROOM.replace("xmp:Rating=\"3\"", "xmp:Rating=\"2\"")).unwrap();
    assert_eq!(f.sync.refresh_folder(1).unwrap(), 1);
    assert_eq!(f.values(f.ids[2]).0, 5);
    assert_eq!(f.values(f.ids[0]).0, 1);
}

struct ChannelSink(Mutex<mpsc::Sender<Result<XmpSynced, XmpWriteFailed>>>);

impl XmpSink for ChannelSink {
    fn synced(&self, e: XmpSynced) {
        let _ = self.0.lock().unwrap().send(Ok(e));
    }
    fn failed(&self, e: XmpWriteFailed) {
        let _ = self.0.lock().unwrap().send(Err(e));
    }
}

fn sink() -> (ChannelSink, mpsc::Receiver<Result<XmpSynced, XmpWriteFailed>>) {
    let (tx, rx) = mpsc::channel();
    (ChannelSink(Mutex::new(tx)), rx)
}

fn wait_idle(sync: &XmpSync) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(10));
        let st = lock_ignore_poison(&sync.state.0);
        if !st.alive {
            return;
        }
    }
    panic!("auto-sync worker did not finish");
}

#[test]
fn auto_sync_respects_the_setting() {
    let f = Fixture::new(1);
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids, 2).unwrap();
    let (s, rx) = sink();
    f.sync.notify_with(s);
    wait_idle(&f.sync);
    assert!(!f.sidecar(0).exists(), "auto-sync is off by default");
    assert!(rx.try_recv().is_err());
    assert!(f.state(f.ids[0]).0);
}

#[test]
fn auto_sync_debounces_and_writes_dirty_images() {
    let f = Fixture::new(2);
    let mut conn = f.conn();
    repo::set_xmp_auto_sync(&conn, true).unwrap();
    let (s, rx) = sink();
    for r in 1..=5 {
        repo::set_rating(&mut conn, &f.ids, r).unwrap();
        f.sync.notify_with(ChannelSink(Mutex::new(s.0.lock().unwrap().clone())));
    }
    let event = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(event.written, f.ids);
    assert!(event.read.is_empty());
    wait_idle(&f.sync);
    assert!(rx.try_recv().is_err(), "one pass for the burst of notifications");
    assert_eq!(parse(&fs::read_to_string(f.sidecar(1)).unwrap()).unwrap().rating, Some(5));
    assert!(!f.state(f.ids[0]).0);
    assert!(!f.sync.is_running());
}

#[test]
fn auto_sync_newer_wins() {
    let f = Fixture::new(2);
    let mut conn = f.conn();
    repo::set_xmp_auto_sync(&conn, true).unwrap();
    repo::set_rating(&mut conn, &f.ids, 1).unwrap();
    f.sync.write_images(&f.ids).unwrap();

    // Image 0: catalog changed, then the sidecar was edited externally (newer) -> read.
    // Image 1: sidecar edited externally, then the catalog changed (newer) -> write.
    tick();
    repo::set_rating(&mut conn, &f.ids[..1], 2).unwrap();
    repo::set_user_tag(&mut conn, &f.ids[..1], CullTag::Blink, true).unwrap();
    tick();
    let s0 = fs::read_to_string(f.sidecar(0)).unwrap().replace("xmp:Rating=\"1\"", "xmp:Rating=\"4\"");
    fs::write(f.sidecar(0), s0).unwrap();
    let s1 = fs::read_to_string(f.sidecar(1)).unwrap().replace("xmp:Rating=\"1\"", "xmp:Rating=\"4\"");
    fs::write(f.sidecar(1), s1).unwrap();
    tick();
    repo::set_rating(&mut conn, &f.ids[1..], 3).unwrap();

    let (s, rx) = sink();
    f.sync.notify_with(s);
    let event = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(event.read, vec![f.ids[0]]);
    assert_eq!(event.written, vec![f.ids[1]]);
    assert_eq!(f.values(f.ids[0]).0, 4);
    let v0 = parse(&fs::read_to_string(f.sidecar(0)).unwrap()).unwrap();
    assert_eq!(v0.rating, Some(4));
    assert_eq!(v0.hierarchical_subjects, ["Sieve|blink"], "pending tag change still written");
    assert_eq!(parse(&fs::read_to_string(f.sidecar(1)).unwrap()).unwrap().rating, Some(3));
    assert!(!f.state(f.ids[0]).0 && !f.state(f.ids[1]).0);
}

#[test]
fn auto_sync_reports_failures() {
    let f = Fixture::new(1);
    let mut conn = f.conn();
    repo::set_xmp_auto_sync(&conn, true).unwrap();
    repo::set_rating(&mut conn, &f.ids, 2).unwrap();
    fs::write(f.sidecar(0), "garbage <<<").unwrap();
    let (s, rx) = sink();
    f.sync.notify_with(s);
    let failed = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
    assert_eq!(failed.image_id, f.ids[0]);
    let (dirty, _, _, err) = f.state(f.ids[0]);
    assert!(dirty && err.is_some());
}

// ---------------------------------------------------------------------------
// Real files + exiftool (run with `cargo test -- --ignored xmp_exiftool`)
// ---------------------------------------------------------------------------

fn exiftool_json(files: &[PathBuf]) -> serde_json::Value {
    let out = std::process::Command::new("exiftool")
        .args(["-j", "-XMP:Rating", "-XMP:Label", "-XMP-lr:HierarchicalSubject", "-XMP-dc:Subject", "-XMP-crs:all"])
        .args(files)
        .output()
        .expect("exiftool on PATH");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Copies 4 ARWs from `$SIEVE_XMP_SAMPLES` (default `~/Pictures/test RAWS`, read only)
/// into a temp dir, imports them, culls, writes sidecars and checks them with exiftool,
/// then edits a sidecar with exiftool and reads it back.
#[test]
#[ignore]
fn xmp_exiftool_roundtrip_on_real_raws() {
    let samples = std::env::var("SIEVE_XMP_SAMPLES")
        .unwrap_or_else(|_| format!("{}/Pictures/test RAWS", std::env::var("HOME").unwrap()));
    let mut arws: Vec<PathBuf> = fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("arw")))
        .collect();
    arws.sort();
    arws.truncate(4);
    let samples_xmps_before = fs::read_dir(&samples)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
        .count();
    assert_eq!(arws.len(), 4);

    let dir = tempfile::tempdir().unwrap();
    let shoot = dir.path().join("shoot");
    fs::create_dir(&shoot).unwrap();
    for a in &arws {
        fs::copy(a, shoot.join(a.file_name().unwrap())).unwrap();
    }
    // A pre-existing Lightroom sidecar (with develop settings) on the 4th file.
    let lr_sidecar = shoot.join(arws[3].file_name().unwrap()).with_extension("xmp");
    fs::write(&lr_sidecar, LIGHTROOM).unwrap();

    let catalog = dir.path().join("cat.sqlite");
    let mut conn = db::open(&catalog).unwrap();
    let summary = repo::import_folder(&mut conn, &shoot, &ImportOptions::raw_only(false)).unwrap();
    assert_eq!(summary.added, 4);
    let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog.clone() });
    assert_eq!(sync.refresh_folder(summary.folder_id).unwrap(), 1, "existing sidecar read on import");
    let ids: Vec<ImageId> = {
        let mut stmt = conn.prepare("SELECT id FROM images ORDER BY file_name").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(store::load(&conn, ids[3]).unwrap().unwrap().rating, 3, "Lightroom rating imported");

    repo::set_rating(&mut conn, &ids[..1], 5).unwrap();
    repo::set_pick(&mut conn, &ids[..1], PickFlag::Pick).unwrap();
    repo::set_pick(&mut conn, &ids[1..2], PickFlag::Reject).unwrap();
    repo::set_user_tag(&mut conn, &ids[1..2], CullTag::Blink, true).unwrap();
    repo::set_color_label(&mut conn, &ids[2..3], Some(ColorLabel::Yellow)).unwrap();
    repo::set_rating(&mut conn, &ids[2..3], 2).unwrap();
    repo::set_user_tag(&mut conn, &ids[2..3], CullTag::MotionBlur, true).unwrap();
    repo::set_user_tag(&mut conn, &ids[2..3], CullTag::Underexposed, true).unwrap();
    repo::set_user_tag(&mut conn, &ids[3..4], CullTag::MissedFocus, true).unwrap();

    let report = sync.write_images(&ids).unwrap();
    assert_eq!(report.succeeded, 4, "{report:?}");
    let sidecars: Vec<PathBuf> =
        ids.iter().map(|&id| resolve_sidecar(&store::load(&conn, id).unwrap().unwrap().path)).collect();
    let json = exiftool_json(&sidecars);
    println!("exiftool after write_xmp:\n{}", serde_json::to_string_pretty(&json).unwrap());
    let j = json.as_array().unwrap();
    assert_eq!(j[0]["Rating"], 5);
    assert_eq!(j[0]["Label"], "Pick");
    assert_eq!(j[1]["Rating"], -1);
    assert_eq!(j[1]["HierarchicalSubject"], "Sieve|blink");
    assert_eq!(j[1]["Subject"], "blink");
    assert_eq!(j[2]["Rating"], 2);
    assert_eq!(j[2]["Label"], "Yellow");
    assert_eq!(j[2]["HierarchicalSubject"], serde_json::json!(["Sieve|motion_blur", "Sieve|underexposed"]));
    assert_eq!(j[3]["Rating"], 3);
    assert_eq!(j[3]["Label"], "Red");
    assert_eq!(
        j[3]["HierarchicalSubject"],
        serde_json::json!(["Events|wedding", "Clients|Smith & Jones", "Sieve|missed_focus"])
    );
    assert_eq!(j[3]["Subject"], serde_json::json!(["wedding", "Smith & Jones", "missed_focus"]));
    assert_eq!(j[3]["Exposure2012"], "+0.35", "crs: develop settings preserved");
    assert_eq!(j[3]["ToneCurvePV2012"], serde_json::json!(["0, 0", "255, 255"]));

    // External edit (Bridge / Lightroom stand-in): exiftool sets Rating 4 on image 0.
    std::thread::sleep(Duration::from_millis(20));
    let st = std::process::Command::new("exiftool")
        .args(["-overwrite_original", "-XMP:Rating=4", "-XMP:Label=Blue"])
        .arg(&sidecars[0])
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(sync.refresh_folder(summary.folder_id).unwrap(), 1);
    let r = store::load(&conn, ids[0]).unwrap().unwrap();
    println!("after exiftool edit + refresh_folder: rating={} pick={:?} label={:?}", r.rating, r.pick, r.color_label);
    assert_eq!((r.rating, r.pick, r.color_label), (4, PickFlag::Unflagged, Some(ColorLabel::Blue)));

    std::thread::sleep(Duration::from_millis(20));
    let st = std::process::Command::new("exiftool")
        .args(["-overwrite_original", "-XMP:Rating=-1"])
        .arg(&sidecars[2])
        .status()
        .unwrap();
    assert!(st.success());
    let report = sync.read_images(&ids[2..3]).unwrap();
    assert_eq!(report.changed, vec![ids[2]]);
    let r = store::load(&conn, ids[2]).unwrap().unwrap();
    println!("after exiftool reject + read_xmp: rating={} pick={:?} label={:?}", r.rating, r.pick, r.color_label);
    assert_eq!((r.rating, r.pick, r.color_label), (2, PickFlag::Reject, Some(ColorLabel::Yellow)));
    // Writing over an exiftool-serialized sidecar (element style) works too.
    repo::set_rating(&mut conn, &ids[..1], 1).unwrap();
    repo::set_user_tag(&mut conn, &ids[..1], CullTag::CreativeBlur, true).unwrap();
    assert_eq!(sync.write_images(&ids[..1]).unwrap().succeeded, 1);
    let j = exiftool_json(&sidecars[..1]);
    println!("exiftool after re-write over exiftool output:\n{}", serde_json::to_string_pretty(&j).unwrap());
    assert_eq!(j[0]["Rating"], 1);
    assert_eq!(j[0]["Label"], "Blue");
    assert_eq!(j[0]["HierarchicalSubject"], "Sieve|creative_blur");
    // The originals were only read.
    let xmps_in = |d: &str| {
        fs::read_dir(d)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
            .count()
    };
    assert_eq!(xmps_in(&samples), samples_xmps_before);
    // Keep a copy for inspection when requested.
    if let Ok(keep) = std::env::var("SIEVE_XMP_KEEP") {
        let keep = PathBuf::from(keep);
        fs::create_dir_all(&keep).unwrap();
        for s in &sidecars {
            fs::copy(s, keep.join(s.file_name().unwrap())).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Develop settings (crs:)
// ---------------------------------------------------------------------------

fn edited() -> ParametricAdjustments {
    use crate::ipc::types::{HslAdjustments, HslChannels, LutRef, WhiteBalance};
    ParametricAdjustments {
        white_balance: WhiteBalance::Custom { temperature_k: 4350.0, tint: -7.5 },
        exposure: -0.65,
        contrast: 18.0,
        highlights: -55.0,
        shadows: 32.0,
        whites: 5.0,
        blacks: -12.0,
        texture: 10.0,
        clarity: 15.0,
        dehaze: 7.0,
        vibrance: 22.0,
        saturation: -4.0,
        hsl: HslAdjustments {
            hue: HslChannels { orange: -6.0, blue: 12.0, ..Default::default() },
            saturation: HslChannels { aqua: -30.0, ..Default::default() },
            luminance: HslChannels { orange: 8.5, ..Default::default() },
        },
        lut: Some(LutRef { id: "film-a1b2c3d4".into(), amount: 65.0 }),
        ..Default::default()
    }
}

#[test]
fn develop_settings_written_only_for_edited_images_and_merged() {
    let f = Fixture::new(2);
    fs::write(f.sidecar(0), LIGHTROOM).unwrap();
    fs::write(f.sidecar(1), LIGHTROOM).unwrap();
    let mut conn = f.conn();
    develop::history::commit(&mut conn, f.ids[0], &edited(), "Exposure").unwrap();
    // Image 1 is only rated: its Lightroom crs: settings must not be touched.
    repo::set_rating(&mut conn, &f.ids[1..], 5).unwrap();
    let report = f.sync.write_images(&f.ids).unwrap();
    assert_eq!(report.succeeded, 2);

    let out1 = fs::read_to_string(f.sidecar(1)).unwrap();
    for line in LIGHTROOM.lines().filter(|l| l.contains("crs:")) {
        assert!(out1.contains(line), "untouched crs: line lost: {line}");
    }

    let out0 = fs::read_to_string(f.sidecar(0)).unwrap();
    assert!(out0.contains("crs:Exposure2012=\"-0.65\""), "{out0}");
    assert!(out0.contains("crs:WhiteBalance=\"Custom\""));
    assert!(out0.contains("crs:Temperature=\"4350\"") && out0.contains("crs:Tint=\"-7.5\""));
    // A newer process version is never downgraded (Lightroom would re-render with PV 11).
    assert!(out0.contains("crs:ProcessVersion=\"15.4\""));
    assert!(out0.contains("crs:LuminanceAdjustmentOrange=\"+8.5\""));
    assert!(
        out0.contains("sieve:LutId=\"film-a1b2c3d4\"") && out0.contains("xmlns:sieve=\"http://sieve.app/ns/1.0/\"")
    );
    // Unowned crs: properties survive byte-for-byte.
    for keep in [
        "crs:Version=\"16.4\"",
        "crs:CameraProfile=\"Adobe Color\"",
        "<crs:ToneCurvePV2012>",
        "<rdf:li>255, 255</rdf:li>",
    ] {
        assert!(out0.contains(keep), "{keep}");
    }
    let owned: Vec<&str> = OWNED
        .iter()
        .copied()
        .chain([
            "crs:ProcessVersion",
            "crs:Exposure2012",
            "crs:Contrast2012",
            "crs:Highlights2012",
            "crs:Shadows2012",
            "crs:WhiteBalance",
        ])
        .collect();
    assert_unrelated_preserved(LIGHTROOM, &out0, &owned);
    // Reads back identically, and the catalog -> XMP -> catalog trip is lossless.
    assert_eq!(parse(&out0).unwrap().develop, Some(edited()));

    // As-shot removes Temperature/Tint; LUT removal removes the sieve: fields.
    let as_shot =
        ParametricAdjustments { lut: None, white_balance: crate::ipc::types::WhiteBalance::AsShot, ..edited() };
    develop::history::commit(&mut conn, f.ids[0], &as_shot, "White Balance").unwrap();
    f.sync.write_images(&f.ids[..1]).unwrap();
    let out0 = fs::read_to_string(f.sidecar(0)).unwrap();
    assert!(out0.contains("crs:WhiteBalance=\"As Shot\""));
    assert!(!out0.contains("crs:Temperature") && !out0.contains("crs:Tint") && !out0.contains("sieve:Lut"), "{out0}");
    assert_eq!(parse(&out0).unwrap().develop, Some(as_shot));
}

#[test]
fn develop_settings_read_from_lightroom_sidecar() {
    let f = Fixture::new(3);
    fs::write(f.sidecar(0), LIGHTROOM).unwrap();
    // PV2010-only sidecar: rating read, develop not imported.
    fs::write(
        f.sidecar(1),
        LIGHTROOM
            .replace("crs:ProcessVersion=\"15.4\"", "crs:ProcessVersion=\"5.7\"")
            .replace("xmp:Rating=\"3\"", "xmp:Rating=\"2\""),
    )
    .unwrap();
    // Malformed develop value: ratings still sync, develop untouched.
    fs::write(f.sidecar(2), LIGHTROOM.replace("crs:Contrast2012=\"+12\"", "crs:Contrast2012=\"lots\"")).unwrap();
    let report = f.sync.read_images(&f.ids).unwrap();
    assert_eq!((report.succeeded, report.failed.len()), (3, 0));
    assert_eq!(report.changed, f.ids);
    let conn = f.conn();
    let a = repo::get_adjustments(&conn, f.ids[0]).unwrap();
    let want = ParametricAdjustments {
        exposure: 0.35,
        contrast: 12.0,
        highlights: -40.0,
        shadows: 25.0,
        ..Default::default()
    };
    assert_eq!(a, want);
    let h = develop::history::history(&conn, f.ids[0]).unwrap();
    assert_eq!(h.entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Original", "Read from XMP"]);
    assert!(!f.state(f.ids[0]).0, "reading develop settings leaves the image clean");
    assert!(repo::get_adjustments(&conn, f.ids[1]).unwrap().is_neutral());
    assert_eq!(f.values(f.ids[1]).0, 2);
    assert!(develop::history::history(&conn, f.ids[1]).unwrap().entries.is_empty());
    assert!(repo::get_adjustments(&conn, f.ids[2]).unwrap().is_neutral());
    // Unchanged on re-read: no new history entry, no change reported.
    assert!(f.sync.read_images(&f.ids[..1]).unwrap().changed.is_empty());
    assert_eq!(develop::history::history(&conn, f.ids[0]).unwrap().entries.len(), 2);
}

#[test]
fn develop_catalog_xmp_catalog_round_trip() {
    let f = Fixture::new(1);
    let mut conn = f.conn();
    develop::history::commit(&mut conn, f.ids[0], &edited(), "Edit").unwrap();
    f.sync.write_images(&f.ids).unwrap();
    // A second catalog importing the same sidecar gets identical settings.
    let g = Fixture::new(1);
    fs::copy(f.sidecar(0), g.sidecar(0)).unwrap();
    assert_eq!(g.sync.read_images(&g.ids).unwrap().changed, g.ids);
    assert_eq!(repo::get_adjustments(&g.conn(), g.ids[0]).unwrap(), edited());
}

/// Writes develop settings into the sidecar of a copy of a real RAW under
/// `test-data/xmp-crs/` (the original is only read) and checks it with exiftool.
#[test]
#[ignore = "needs exiftool and sample RAWs"]
fn develop_exiftool_check() {
    let samples = std::env::var("SIEVE_XMP_SAMPLES")
        .unwrap_or_else(|_| format!("{}/Pictures/test RAWS", std::env::var("HOME").unwrap()));
    let raw = fs::read_dir(&samples)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| crate::raw::format_from_extension(p).is_some())
        .min()
        .unwrap();
    let out_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data/xmp-crs"));
    let _ = fs::remove_dir_all(&out_dir);
    fs::create_dir_all(&out_dir).unwrap();
    let copy = out_dir.join(raw.file_name().unwrap());
    fs::copy(&raw, &copy).unwrap();
    let catalog = out_dir.join("cat.sqlite");
    let mut conn = db::open(&catalog).unwrap();
    repo::import_folder(&mut conn, &out_dir, &ImportOptions::raw_only(false)).unwrap();
    let id: ImageId = conn.query_row("SELECT id FROM images", [], |r| r.get(0)).unwrap();
    develop::history::commit(&mut conn, id, &edited(), "Edit").unwrap();
    let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog });
    assert_eq!(sync.write_images(&[id]).unwrap().succeeded, 1);
    let sidecar = sidecar_path(&copy);
    let out = std::process::Command::new("exiftool")
        .args(["-XMP-crs:all", "-XMP-sieve:all", "-j"])
        .arg(&sidecar)
        .output()
        .expect("exiftool on PATH");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("{text}");
    let j: serde_json::Value = serde_json::from_str(&text).unwrap();
    // exiftool prints signed positives as strings ("+18"), negatives as numbers.
    assert_eq!(j[0]["Exposure2012"], -0.65);
    assert_eq!(j[0]["ColorTemperature"], 4350);
    assert_eq!(j[0]["Tint"], -7.5);
    assert_eq!(j[0]["WhiteBalance"], "Custom");
    assert_eq!(j[0]["Contrast2012"], "+18");
    assert_eq!(j[0]["Highlights2012"], -55);
    assert_eq!(j[0]["LuminanceAdjustmentOrange"], "+8.5");
    assert_eq!(j[0]["ProcessVersion"], 11.0);
    assert_eq!(j[0]["HasSettings"], true);
    assert_eq!(j[0]["LutId"], "film-a1b2c3d4");
}

/// Read-only parity check over real Lightroom sidecars:
/// `SIEVE_SAMPLE_XMP_DIR=/path/to/dir cargo test --lib real_lightroom_sidecars_decode -- --ignored --nocapture`.
/// Never writes anything.
#[test]
#[ignore = "needs SIEVE_SAMPLE_XMP_DIR"]
fn real_lightroom_sidecars_decode() {
    use crate::ipc::types::{DevelopWarningCode, PointCurves};
    let dir = PathBuf::from(std::env::var("SIEVE_SAMPLE_XMP_DIR").expect("SIEVE_SAMPLE_XMP_DIR"));
    let (mut total, mut developed, mut curves, mut cropped, mut masks) = (0, 0, 0, 0, 0);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("xmp") {
            continue;
        }
        total += 1;
        let v = parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v.develop_error, None, "{}", path.display());
        if let Some(adj) = v.develop {
            adj.validate().unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            developed += 1;
            curves += usize::from(!PointCurves::is_identity(&adj.tone_curve.point.master));
            cropped += usize::from(adj.crop.enabled);
        }
        masks += usize::from(v.warnings.iter().any(|w| w.code == DevelopWarningCode::MasksUnsupported));
    }
    println!(
        "sidecars {total}, with develop {developed}, custom master curve {curves}, cropped {cropped}, masks {masks}"
    );
    assert!(total > 0);
}

const MASKED_SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="1"
   crs:ProcessVersion="15.4"
   crs:Exposure2012="+0.50">
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description
       crs:What="Correction"
       crs:CorrectionAmount="1"
       crs:CorrectionActive="true"
       crs:CorrectionName="Sky"
       crs:CorrectionSyncID="11111111111111111111111111111111"
       crs:LocalExposure2012="-0.25">
      <crs:CorrectionMasks>
       <rdf:Seq>
        <rdf:li
         crs:What="Mask/Gradient"
         crs:MaskActive="true"
         crs:MaskName="Linear Gradient 1"
         crs:MaskBlendMode="0"
         crs:MaskInverted="false"
         crs:MaskSyncID="22222222222222222222222222222222"
         crs:MaskValue="1"
         crs:ZeroX="0.5"
         crs:ZeroY="0.6"
         crs:FullX="0.5"
         crs:FullY="0.2"/>
       </rdf:Seq>
      </crs:CorrectionMasks>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

#[test]
fn masks_import_on_read_and_are_protected_until_imported() {
    let f = Fixture::new(1);
    let id = f.ids[0];
    fs::write(f.sidecar(0), MASKED_SIDECAR).unwrap();
    // Pre-v10 read: masks only reported. A develop edit must not delete them on write.
    {
        let mut conn = f.conn();
        conn.execute("UPDATE images SET masks_pending_import = 1 WHERE id = ?1", [id]).unwrap();
        let adj = ParametricAdjustments { exposure: 1.0, ..Default::default() };
        crate::develop::history::commit(&mut conn, id, &adj, "Exposure").unwrap();
    }
    assert_eq!(f.sync.write_images(&[id]).unwrap().succeeded, 1);
    let written = fs::read_to_string(f.sidecar(0)).unwrap();
    assert!(!written.contains("crs:Exposure2012=\"+0.50\""), "develop written: {written}");
    assert!(written.contains("<crs:MaskGroupBasedCorrections>"), "pending masks kept");
    // Read imports them, clears the flag and reports no unsupported masks.
    assert_eq!(f.sync.read_images(&[id]).unwrap().succeeded, 1);
    let mut conn = f.conn();
    let adj = repo::get_adjustments(&conn, id).unwrap();
    assert_eq!(adj.masks.len(), 1);
    assert_eq!(adj.masks[0].name, "Sky");
    assert_eq!(adj.masks[0].adjustments.exposure, -1.0);
    assert!(!store::masks_pending(&conn, id).unwrap());
    let warnings: Option<String> =
        conn.query_row("SELECT develop_warnings FROM images WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(!warnings.unwrap_or_default().contains("masks_unsupported"));
    // Now edits to masks reach the sidecar; untouched components keep their bytes.
    let mut edited = adj.clone();
    edited.masks[0].amount = 0.5;
    crate::develop::history::commit(&mut conn, id, &edited, "Mask").unwrap();
    assert_eq!(f.sync.write_images(&[id]).unwrap().succeeded, 1);
    let written = fs::read_to_string(f.sidecar(0)).unwrap();
    assert!(written.contains("crs:CorrectionAmount=\"0.5\""), "{written}");
    assert!(written.contains("crs:ZeroY=\"0.6\""));
    assert_eq!(masks::read(&written).unwrap().unwrap().groups, edited.masks);
    // Deleting all masks removes the element.
    let mut none = edited;
    none.masks.clear();
    crate::develop::history::commit(&mut conn, id, &none, "Mask").unwrap();
    f.sync.write_images(&[id]).unwrap();
    assert!(!fs::read_to_string(f.sidecar(0)).unwrap().contains("MaskGroupBasedCorrections"));
}
