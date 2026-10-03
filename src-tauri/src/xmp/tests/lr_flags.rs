//! Lightroom Classic flags (`xmpDM:pick` / `xmpDM:good`, decisions 2026-10-03).

use super::*;

const LR_CLASSIC: &str = include_str!("../fixtures/lightroom_classic_flags.xmp");
const LR_CLASSIC_DATE: &str = "xmp:MetadataDate=\"2026-10-02T09:14:03+02:00\"";
const DATE_ATTR: &str = "xmp:MetadataDate=\"2026-09-29T12:00:00Z\"";

#[test]
fn lightroom_classic_flags_are_read() {
    let v = parse(LR_CLASSIC).unwrap();
    assert_eq!((v.rating, v.label.as_deref(), v.dm_pick, v.dm_good), (Some(4), Some("Green"), Some(1), Some(true)));
    assert_eq!(catalog_values(&v, 0), (4, PickFlag::Pick, Some(ColorLabel::Green)));
    let dev = v.develop.as_ref().expect("crs: settings decoded");
    assert_eq!((dev.exposure, dev.highlights), (0.6, -55.0));

    let rejected =
        LR_CLASSIC.replace("xmpDM:pick=\"1\"", "xmpDM:pick=\"-1\"").replace("\"True\"\n   aux", "\"False\"\n   aux");
    let v = parse(&rejected).unwrap();
    assert_eq!((v.dm_pick, v.dm_good), (Some(-1), Some(false)));
    assert_eq!(catalog_values(&v, 0), (4, PickFlag::Reject, Some(ColorLabel::Green)), "stars kept on reject");

    // Unflagged in Lightroom: pick 0, good removed.
    let unflagged = LR_CLASSIC.replace("xmpDM:pick=\"1\"", "xmpDM:pick=\"0\"").replace("\n   xmpDM:good=\"True\"", "");
    assert_eq!(catalog_values(&parse(&unflagged).unwrap(), 0), (4, PickFlag::Unflagged, Some(ColorLabel::Green)));

    // Only xmpDM:good (older Bridge / Lightroom builds).
    let good_only = LR_CLASSIC.replace("\n   xmpDM:pick=\"1\"", "").replace("\"True\"\n   aux", "\"False\"\n   aux");
    assert_eq!(catalog_values(&parse(&good_only).unwrap(), 0).1, PickFlag::Reject);
}

#[test]
fn lightroom_classic_element_form_flags() {
    let src = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:xmpDM="http://ns.adobe.com/xmp/1.0/DynamicMedia/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
   <xmp:Rating>2</xmp:Rating>
   <xmpDM:pick>1</xmpDM:pick>
   <xmpDM:good>True</xmpDM:good>
   <photoshop:City>Calgary</photoshop:City>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
    let v = parse(src).unwrap();
    assert_eq!(catalog_values(&v, 0), (2, PickFlag::Pick, None));
    let mut w = want(2, None, &[]);
    w.pick = PickFlag::Reject;
    let out = merge(Some(src), &w).unwrap();
    assert!(out.contains("   <xmpDM:pick>-1</xmpDM:pick>\n   <xmpDM:good>False</xmpDM:good>\n"), "{out}");
    assert!(out.contains("<photoshop:City>Calgary</photoshop:City>"));
    assert_eq!(catalog_values(&parse(&out).unwrap(), 0), (2, PickFlag::Reject, None));
    w.pick = PickFlag::Unflagged;
    let out = merge(Some(&out), &w).unwrap();
    assert!(out.contains("<xmpDM:pick>0</xmpDM:pick>\n   <photoshop:City>") && !out.contains("xmpDM:good"), "{out}");
    assert_eq!(catalog_values(&parse(&out).unwrap(), 0), (2, PickFlag::Unflagged, None));
}

/// Writing over a Lightroom Classic sidecar changes only the flag values (and the metadata
/// date): every other byte, `crs:` settings and the colour label included, is kept.
#[test]
fn lightroom_classic_sidecar_write_round_trip() {
    let mut w = want(4, Some("Green"), &[]);
    w.pick = PickFlag::Pick;
    let same = merge(Some(LR_CLASSIC), &w).unwrap();
    assert_eq!(same, LR_CLASSIC.replace(LR_CLASSIC_DATE, DATE_ATTR), "same state: only the date moves");

    w.pick = PickFlag::Reject;
    let rejected = merge(Some(LR_CLASSIC), &w).unwrap();
    assert_eq!(
        rejected,
        LR_CLASSIC
            .replace(LR_CLASSIC_DATE, DATE_ATTR)
            .replace("xmpDM:pick=\"1\"", "xmpDM:pick=\"-1\"")
            .replace("xmpDM:good=\"True\"", "xmpDM:good=\"False\"")
    );
    assert_eq!(catalog_values(&parse(&rejected).unwrap(), 0), (4, PickFlag::Reject, Some(ColorLabel::Green)));

    w.pick = PickFlag::Unflagged;
    let unflagged = merge(Some(&rejected), &w).unwrap();
    assert_eq!(
        unflagged,
        LR_CLASSIC
            .replace(LR_CLASSIC_DATE, DATE_ATTR)
            .replace("xmpDM:pick=\"1\"", "xmpDM:pick=\"0\"")
            .replace("\n   xmpDM:good=\"True\"", "")
    );
    assert_eq!(catalog_values(&parse(&unflagged).unwrap(), 0), (4, PickFlag::Unflagged, Some(ColorLabel::Green)));

    w.pick = PickFlag::Pick;
    let picked = merge(Some(&unflagged), &w).unwrap();
    let v = parse(&picked).unwrap();
    assert_eq!((v.dm_pick, v.dm_good), (Some(1), Some(true)));
    assert_eq!(catalog_values(&v, 0), (4, PickFlag::Pick, Some(ColorLabel::Green)));
    assert_unrelated_preserved(LR_CLASSIC, &picked, &["MetadataDate", "xmpDM:pick", "xmpDM:good"]);
}

/// Catalog -> sidecar -> catalog through the sync layer, starting from a Lightroom sidecar.
#[test]
fn lightroom_classic_flags_sync_round_trip() {
    let f = Fixture::new(3);
    fs::write(f.sidecar(0), LR_CLASSIC).unwrap();
    f.sync.read_images(&f.ids[..1]).unwrap();
    assert_eq!(f.values(f.ids[0]), (4, PickFlag::Pick, Some(ColorLabel::Green)));

    let mut conn = f.conn();
    repo::set_pick(&mut conn, &f.ids[..1], PickFlag::Reject).unwrap();
    repo::set_rating(&mut conn, &f.ids[1..2], 5).unwrap();
    repo::set_pick(&mut conn, &f.ids[1..2], PickFlag::Pick).unwrap();
    repo::set_color_label(&mut conn, &f.ids[1..2], Some(ColorLabel::Purple)).unwrap();
    repo::set_rating(&mut conn, &f.ids[2..3], 2).unwrap();
    repo::set_pick(&mut conn, &f.ids[2..3], PickFlag::Reject).unwrap();
    let want_values: Vec<_> = f.ids.iter().map(|&id| f.values(id)).collect();
    assert_eq!(f.sync.write_images(&f.ids).unwrap().succeeded, 3);

    let s0 = fs::read_to_string(f.sidecar(0)).unwrap();
    assert!(s0.contains("xmpDM:pick=\"-1\"") && s0.contains("xmpDM:good=\"False\"") && s0.contains("xmp:Rating=\"4\""));
    assert!(s0.contains("crs:Exposure2012=\"+0.6") && s0.contains("xmp:Label=\"Green\""), "{s0}");
    let s2 = parse(&fs::read_to_string(f.sidecar(2)).unwrap()).unwrap();
    assert_eq!((s2.rating, s2.dm_pick, s2.dm_good), (Some(2), Some(-1), Some(false)), "stars kept on reject");

    // Wipe the catalog's culling state, read the sidecars back.
    repo::set_rating(&mut conn, &f.ids, 0).unwrap();
    repo::set_pick(&mut conn, &f.ids, PickFlag::Unflagged).unwrap();
    repo::set_color_label(&mut conn, &f.ids, None).unwrap();
    f.sync.read_images(&f.ids).unwrap();
    let got: Vec<_> = f.ids.iter().map(|&id| f.values(id)).collect();
    assert_eq!(got, want_values);
}

/// Window focus / project open: a flag changed in Lightroom is picked up by
/// `refresh_folders`, which reports the changed images and is a no-op the second time.
#[test]
fn refresh_folders_picks_up_lightroom_flag_changes() {
    let f = Fixture::new(2);
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids, 2).unwrap();
    assert_eq!(f.sync.write_images(&f.ids).unwrap().succeeded, 2);
    assert!(f.sync.refresh_folders(&[1]).unwrap().is_empty(), "nothing changed");
    tick();
    // Lightroom rejects image 1 (attribute form on rdf:Description).
    let s1 = fs::read_to_string(f.sidecar(1)).unwrap();
    let edited = s1.replace(
        "xmp:Rating=\"2\"",
        "xmp:Rating=\"2\"\n    xmlns:xmpDM=\"http://ns.adobe.com/xmp/1.0/DynamicMedia/\" xmpDM:pick=\"-1\" xmpDM:good=\"False\"",
    );
    assert_ne!(edited, s1);
    fs::write(f.sidecar(1), edited).unwrap();
    assert_eq!(f.sync.refresh_folders(&[1]).unwrap(), vec![f.ids[1]]);
    assert_eq!(f.values(f.ids[1]), (2, PickFlag::Reject, None));
    assert!(!f.state(f.ids[1]).0, "read from the sidecar: nothing to write back");
    assert!(f.sync.refresh_folders(&[1]).unwrap().is_empty(), "second refresh is a no-op");
}

/// Sidecars written by older Sieve versions (`xmp:Rating -1` = reject, `xmp:Label "Pick"` =
/// pick) are read, and rewritten in the Lightroom Classic encoding on the next write.
#[test]
fn legacy_sieve_sidecars_are_read_and_migrated_on_write() {
    let f = Fixture::new(3);
    let legacy = |rating: &str, label: &str| {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Sieve\">\n \
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\"\n   \
xmp:Rating=\"{rating}\"{label}\n   photoshop:City=\"Calgary\">\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n"
        )
    };
    fs::write(f.sidecar(0), legacy("-1", "")).unwrap();
    fs::write(f.sidecar(1), legacy("4", "\n   xmp:Label=\"Pick\"")).unwrap();
    fs::write(f.sidecar(2), legacy("2", "\n   xmp:Label=\"Pick\"")).unwrap();
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids[..1], 3).unwrap();
    f.sync.read_images(&f.ids).unwrap();
    assert_eq!(f.values(f.ids[0]), (3, PickFlag::Reject, None), "legacy reject; catalog stars kept");
    assert_eq!(f.values(f.ids[1]), (4, PickFlag::Pick, None), "legacy pick");
    assert_eq!(f.values(f.ids[2]), (2, PickFlag::Pick, None));

    repo::set_color_label(&mut conn, &f.ids[1..2], Some(ColorLabel::Red)).unwrap();
    repo::set_pick(&mut conn, &f.ids[2..3], PickFlag::Unflagged).unwrap();
    assert_eq!(f.sync.write_images(&f.ids).unwrap().succeeded, 3);
    let text: Vec<String> = (0..3).map(|i| fs::read_to_string(f.sidecar(i)).unwrap()).collect();
    for t in &text {
        assert!(t.contains("photoshop:City=\"Calgary\""), "{t}");
        assert!(!t.contains("Rating=\"-1\"") && !t.contains("\"Pick\""), "legacy encodings removed: {t}");
    }
    let has = |i: usize, parts: &[&str]| parts.iter().all(|p| text[i].contains(p));
    assert!(has(0, &["xmp:Rating=\"3\"", "xmpDM:pick=\"-1\"", "xmpDM:good=\"False\""]), "{}", text[0]);
    assert!(has(1, &["xmp:Label=\"Red\"", "xmpDM:pick=\"1\"", "xmpDM:good=\"True\""]), "{}", text[1]);
    assert!(
        has(2, &["xmp:Rating=\"2\""]) && !text[2].contains("xmpDM") && !text[2].contains("xmp:Label"),
        "{}",
        text[2]
    );
    // And they read back as the catalog has them.
    let before: Vec<_> = f.ids.iter().map(|&id| f.values(id)).collect();
    assert!(f.sync.read_images(&f.ids).unwrap().changed.is_empty());
    assert_eq!(f.ids.iter().map(|&id| f.values(id)).collect::<Vec<_>>(), before);
}

/// exiftool (an independent XMP reader) sees the flag as `XMP-xmpDM:Pick` / `XMP-xmpDM:Good`.
/// Skipped (with a note) when exiftool is not on PATH.
#[test]
fn exiftool_sees_lightroom_classic_flags() {
    if std::process::Command::new("exiftool").arg("-ver").output().is_err() {
        eprintln!("note: exiftool not on PATH; skipped");
        return;
    }
    let f = Fixture::new(3);
    fs::write(f.sidecar(2), LR_CLASSIC).unwrap();
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids[..1], 5).unwrap();
    repo::set_pick(&mut conn, &f.ids[..1], PickFlag::Pick).unwrap();
    repo::set_rating(&mut conn, &f.ids[1..2], 3).unwrap();
    repo::set_pick(&mut conn, &f.ids[1..2], PickFlag::Reject).unwrap();
    repo::set_color_label(&mut conn, &f.ids[2..3], Some(ColorLabel::Blue)).unwrap();
    repo::set_rating(&mut conn, &f.ids[2..3], 1).unwrap();
    assert_eq!(f.sync.write_images(&f.ids).unwrap().succeeded, 3);
    let out = std::process::Command::new("exiftool")
        .args(["-j", "-G1", "-XMP-xmpDM:Pick", "-XMP-xmpDM:Good", "-XMP-xmp:Rating", "-XMP-xmp:Label"])
        .arg("-XMP-crs:Exposure2012")
        .args((0..3).map(|i| f.sidecar(i)))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    println!("{}", serde_json::to_string_pretty(&j).unwrap());
    let j = j.as_array().unwrap();
    use serde_json::json;
    let flag =
        |i: usize| (j[i]["XMP-xmpDM:Pick"].clone(), j[i]["XMP-xmpDM:Good"].clone(), j[i]["XMP-xmp:Rating"].clone());
    assert_eq!(flag(0), (json!(1), json!(true), json!(5)));
    assert!(j[0].get("XMP-xmp:Label").is_none());
    assert_eq!(flag(1), (json!(-1), json!(false), json!(3)));
    // The Lightroom sidecar was never read into this catalog, so the catalog (unflagged) is
    // authoritative on write: pick 0, good removed; colour, stars, crs: as the catalog has them.
    assert_eq!(flag(2), (json!(0), json!(null), json!(1)));
    assert!(j[2].get("XMP-xmpDM:Good").is_none());
    assert_eq!(j[2]["XMP-xmp:Label"], json!("Blue"));
    assert_eq!(j[2]["XMP-crs:Exposure2012"], json!("+0.60"));
}
