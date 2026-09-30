//! Phase 7b XMP parity: nested structs (`<crs:Look>`), `rdf:Seq` curve writes, profile
//! writes, and a lossless round trip over the user's real Lightroom sidecars (ignored).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::crs::{self, CrsSource, CRS_NS};
use super::packet::{self, merge, parse, Desired, Packet, ProfileWrite};
use crate::ipc::types::{ImageFormat, LookSettings, ParametricAdjustments, ProfileSettings};

const LOOK_SIDECAR: &str = include_str!("fixtures/lightroom_look.xmp");
const DATE: &str = "2026-09-29T12:00:00Z";

/// A develop write of `adj` (as `xmp::desired` builds it) keeping rating 2 / no label.
fn develop_write(adj: &ParametricAdjustments, look_source: Option<&str>) -> Desired {
    let mut edits = crs::encode(adj);
    let (seqs, name) = crs::encode_curves(adj);
    edits.push(name);
    Desired {
        develop: edits,
        rating: 2,
        label: None,
        tags: Vec::new(),
        metadata_date: DATE.into(),
        seqs,
        profile: Some(ProfileWrite { settings: adj.profile.clone(), look_source: look_source.map(Arc::from) }),
    }
}

fn owned_names(adj: &ParametricAdjustments) -> HashSet<(String, String)> {
    let mut set: HashSet<(String, String)> = crs::encode(adj).into_iter().map(|e| (e.ns.to_owned(), e.name)).collect();
    for (name, _) in crs::CRS_CURVES {
        set.insert((CRS_NS.into(), (*name).into()));
    }
    for n in [crs::CURVE_NAME, crs::CAMERA_PROFILE, crs::CAMERA_PROFILE_DIGEST, crs::LOOK] {
        set.insert((CRS_NS.into(), n.into()));
    }
    for n in ["Rating", "Label", "MetadataDate"] {
        set.insert((packet::NS_XMP.into(), n.into()));
    }
    set.insert((packet::NS_DC.into(), "subject".into()));
    set.insert((packet::NS_LR.into(), "hierarchicalSubject".into()));
    set
}

/// Every top-level property Sieve does not own is byte-identical in `out`.
fn assert_unowned_identical(orig: &str, out: &str, owned: &HashSet<(String, String)>) {
    let before = packet::top_level_properties(orig).unwrap();
    let after: HashSet<(String, String, String)> = packet::top_level_properties(out).unwrap().into_iter().collect();
    for (ns, local, text) in before {
        if owned.contains(&(ns.clone(), local.clone())) {
            continue;
        }
        assert!(after.contains(&(ns.clone(), local.clone(), text.clone())), "unowned {ns} {local} changed:\n{text}");
    }
}

#[test]
fn reads_look_struct_and_parameters() {
    let v = parse(LOOK_SIDECAR).unwrap();
    let adj = v.develop.unwrap();
    assert_eq!(adj.profile, ProfileSettings::default(), "Adobe Standard + Adobe Color");
    assert_eq!(adj.tone_curve.point.master, vec![[0.0, 14.0], [44.0, 46.0], [106.0, 110.0], [255.0, 252.0]]);
    assert_eq!(adj.tone_curve.point.red.len(), 3);

    let p = Packet::parse(LOOK_SIDECAR).unwrap();
    let look = p.look().unwrap();
    assert_eq!(look.settings, LookSettings::adobe_color());
    assert_eq!((look.supports_amount, look.group.as_deref()), (Some(false), Some("Profiles")));
    let params = look.parameters.unwrap();
    assert_eq!(params.scalar(CRS_NS, "LookTable").as_deref(), Some("E1095149FDB39D7A057BAB208837E2E1"));
    assert_eq!(params.seq(CRS_NS, "ToneCurvePV2012").map(|v| v.len()), Some(7));
    // Nested properties never leak into the top level.
    let top = p.top();
    assert_eq!(top.scalar(CRS_NS, "LookTable"), None);
    assert_eq!(top.seq(CRS_NS, "ToneCurvePV2012").map(|v| v.len()), Some(4));
    assert!(top.scalars(CRS_NS).iter().any(|(k, _)| k.starts_with("Table_")));
}

#[test]
fn parse_type_resource_struct_and_legacy_profiles() {
    let body = |inner: &str| {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" \
             crs:ProcessVersion=\"11.0\" crs:Exposure2012=\"0\" {inner}</rdf:Description></rdf:RDF></x:xmpmeta>"
        )
    };
    let x = body(
        "crs:CameraProfile=\"Camera ST\"><crs:Look rdf:parseType=\"Resource\"><crs:Name>Adobe Vivid</crs:Name>\
         <crs:Amount>0.5</crs:Amount><crs:UUID>ea1de074f188405965ef399c72c221d9</crs:UUID></crs:Look>",
    );
    let adj = parse(&x).unwrap().develop.unwrap();
    assert_eq!(adj.profile.camera_profile.as_deref(), Some("Camera ST"));
    assert_eq!(
        adj.profile.look,
        Some(LookSettings { name: "Adobe Vivid".into(), uuid: "EA1DE074F188405965EF399C72C221D9".into(), amount: 0.5 })
    );
    // Legacy DCP-era name without a look.
    let adj = parse(&body("crs:CameraProfile=\"Adobe Landscape\">")).unwrap().develop.unwrap();
    assert_eq!(adj.profile.camera_profile.as_deref(), Some("Adobe Standard"));
    assert_eq!(adj.profile.look.as_ref().map(|l| l.uuid.as_str()), Some("6F9C877E84273F4E8271E6B91BEB36A1"));
    // Bare DCP choice: no look.
    let adj = parse(&body("crs:CameraProfile=\"Camera Neutral\">")).unwrap().develop.unwrap();
    assert_eq!((adj.profile.camera_profile.as_deref(), adj.profile.look), (Some("Camera Neutral"), None));
    // Nothing recorded: format default.
    let adj = parse(&body(">")).unwrap().develop.unwrap();
    assert_eq!(adj.profile, ProfileSettings::default());
    let adj = packet::parse_for(&body(">"), ImageFormat::Jpeg).unwrap().develop.unwrap();
    assert_eq!(adj, ParametricAdjustments::defaults_for(ImageFormat::Jpeg));
    // Non-RAW "Embedded".
    let adj = parse(&body("crs:CameraProfile=\"Embedded\">")).unwrap().develop.unwrap();
    assert_eq!((adj.profile.camera_profile, adj.profile.look), (None, None));
}

#[test]
fn unchanged_write_keeps_look_and_curves_byte_for_byte() {
    let adj = parse(LOOK_SIDECAR).unwrap().develop.unwrap();
    let out = merge(Some(LOOK_SIDECAR), &develop_write(&adj, None)).unwrap();
    assert_eq!(parse(&out).unwrap().develop, Some(adj.clone()));
    // Owned-but-unchanged structs/seqs are untouched too.
    for keep in ["<crs:Look>", "crs:CameraProfileDigest=\"A4DC20C59DE4E2DF67946B80BFDA0801\"", "<rdf:li>0, 14</rdf:li>"]
    {
        assert!(out.contains(keep), "{keep}");
    }
    let look_span = |s: &str| {
        let a = s.find("<crs:Look>").unwrap();
        let b = s.find("</crs:Look>").unwrap();
        s[a..b].to_owned()
    };
    assert_eq!(look_span(&out), look_span(LOOK_SIDECAR));
    assert_unowned_identical(LOOK_SIDECAR, &out, &owned_names(&adj));
    assert!(out.contains("crs:ProcessVersion=\"15.4\""), "never downgraded");
    // Idempotent.
    assert_eq!(merge(Some(&out), &develop_write(&adj, None)).unwrap(), out);
}

#[test]
fn curves_are_replaced_created_and_named() {
    let mut adj = parse(LOOK_SIDECAR).unwrap().develop.unwrap();
    adj.tone_curve.point.master = vec![[0.0, 0.0], [128.0, 140.0], [255.0, 255.0]];
    adj.tone_curve.point.blue = vec![[0.0, 10.0], [255.0, 245.0]];
    let out = merge(Some(LOOK_SIDECAR), &develop_write(&adj, None)).unwrap();
    let back = parse(&out).unwrap().develop.unwrap();
    assert_eq!(back.tone_curve, adj.tone_curve);
    assert!(
        out.contains(
            "   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>\n     <rdf:li>128, 140</rdf:li>"
        ),
        "{out}"
    );
    assert!(out.contains("crs:ToneCurveName2012=\"Custom\""));
    // Identity master -> "Linear".
    adj.tone_curve.point.master = vec![[0.0, 0.0], [255.0, 255.0]];
    let out = merge(Some(&out), &develop_write(&adj, None)).unwrap();
    assert!(out.contains("crs:ToneCurveName2012=\"Linear\""));
    assert_eq!(parse(&out).unwrap().develop.unwrap().tone_curve, adj.tone_curve);
    // A new packet gets all four Seqs (Lightroom's layout).
    let fresh = merge(None, &develop_write(&adj, None)).unwrap();
    for (name, _) in crs::CRS_CURVES {
        assert!(fresh.contains(&format!("<crs:{name}>\n")), "{name}: {fresh}");
    }
    assert_eq!(parse(&fresh).unwrap().develop.unwrap().tone_curve, adj.tone_curve);
}

const FAKE_VIVID: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n   crs:PresetType=\"Look\"\n   \
crs:UUID=\"EA1DE074F188405965EF399C72C221D9\"\n   crs:SupportsAmount=\"False\"\n   crs:SupportsMonochrome=\"False\"\n   \
crs:SupportsOutputReferred=\"False\"\n   crs:Copyright=\"(c) test\"\n   crs:Version=\"10.3\"\n   crs:ProcessVersion=\"10.0\"\n   \
crs:ConvertToGrayscale=\"False\"\n   crs:CameraProfile=\"Adobe Standard\"\n   crs:LookTable=\"0123456789ABCDEF0123456789ABCDEF\"\n   \
crs:Table_0123456789ABCDEF0123456789ABCDEF=\"TABLEDATA\"\n   crs:HasSettings=\"True\">\n   <crs:Name>\n    <rdf:Alt>\n     \
<rdf:li xml:lang=\"x-default\">Adobe Vivid</rdf:li>\n    </rdf:Alt>\n   </crs:Name>\n   <crs:Group>\n    <rdf:Alt>\n     \
<rdf:li xml:lang=\"x-default\">Profiles</rdf:li>\n    </rdf:Alt>\n   </crs:Group>\n   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     \
<rdf:li>0, 0</rdf:li>\n     <rdf:li>128, 136</rdf:li>\n     <rdf:li>255, 255</rdf:li>\n    </rdf:Seq>\n   </crs:ToneCurvePV2012>\n  \
</rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";

fn vivid(amount: f32) -> LookSettings {
    LookSettings { name: "Adobe Vivid".into(), uuid: "EA1DE074F188405965EF399C72C221D9".into(), amount }
}

#[test]
fn profile_and_look_writes() {
    let base = parse(LOOK_SIDECAR).unwrap().develop.unwrap();

    // New look (installed source): struct replaced, tables never copied, digest kept
    // (same camera profile).
    let mut adj = base.clone();
    adj.profile.look = Some(vivid(1.0));
    let out = merge(Some(LOOK_SIDECAR), &develop_write(&adj, Some(FAKE_VIVID))).unwrap();
    assert_eq!(parse(&out).unwrap().develop.unwrap().profile, adj.profile);
    let p = Packet::parse(&out).unwrap();
    let look = p.look().unwrap();
    assert_eq!(
        (look.supports_amount, look.group.as_deref(), look.copyright.as_deref()),
        (Some(false), Some("Profiles"), Some("(c) test"))
    );
    let params = look.parameters.unwrap();
    assert_eq!(params.scalar(CRS_NS, "LookTable").as_deref(), Some("0123456789ABCDEF0123456789ABCDEF"));
    assert_eq!(params.seq(CRS_NS, "ToneCurvePV2012").map(|v| v.len()), Some(3));
    assert_eq!(params.scalar(CRS_NS, "PresetType"), None);
    assert!(!out.contains("TABLEDATA"), "installed look tables are not copied");
    assert!(out.contains("CameraProfileDigest"));
    assert!(out.contains("crs:Table_892DAC8D01F7CC3EC28616FEA58F9763"), "unrelated tables preserved");
    assert_eq!(out.matches("<crs:Look>").count(), 1);
    assert_unowned_identical(LOOK_SIDECAR, &out, &owned_names(&adj));

    // Amount only: one attribute changes inside the struct.
    let mut amt = adj.clone();
    amt.profile.look = Some(vivid(0.6));
    let out2 = merge(Some(&out), &develop_write(&amt, Some(FAKE_VIVID))).unwrap();
    assert_eq!(parse(&out2).unwrap().develop.unwrap().profile, amt.profile);
    assert_eq!(out2.replace("crs:Amount=\"0.6\"", "crs:Amount=\"1\""), out);

    // Look not installed: minimal struct.
    let out3 = merge(Some(LOOK_SIDECAR), &develop_write(&adj, None)).unwrap();
    assert_eq!(parse(&out3).unwrap().develop.unwrap().profile, adj.profile);
    assert!(!out3.contains("<crs:Parameters>"));

    // Different camera profile: digest removed.
    let mut cam = base.clone();
    cam.profile.camera_profile = Some("Camera ST".into());
    let out4 = merge(Some(LOOK_SIDECAR), &develop_write(&cam, None)).unwrap();
    assert!(out4.contains("crs:CameraProfile=\"Camera ST\"") && !out4.contains("CameraProfileDigest"));
    assert_eq!(parse(&out4).unwrap().develop.unwrap().profile, cam.profile);
    // The look's nested crs:CameraProfile is not a top-level property.
    assert!(out4.contains("      crs:CameraProfile=\"Adobe Standard\""));

    // No look: struct removed.
    let mut none = base.clone();
    none.profile.look = None;
    let out5 = merge(Some(LOOK_SIDECAR), &develop_write(&none, None)).unwrap();
    assert!(!out5.contains("crs:Look"), "{out5}");
    assert_eq!(parse(&out5).unwrap().develop.unwrap().profile, none.profile);

    // Non-RAW "no profile" over a packet without profile data writes nothing.
    let fresh = merge(None, &develop_write(&ParametricAdjustments::defaults_for(ImageFormat::Jpeg), None)).unwrap();
    assert!(!fresh.contains("CameraProfile") && !fresh.contains("crs:Look"));
    assert_eq!(
        packet::parse_for(&fresh, ImageFormat::Jpeg).unwrap().develop,
        Some(ParametricAdjustments::defaults_for(ImageFormat::Jpeg))
    );
    // RAW default over nothing recorded: nothing written either (Lightroom's default).
    let fresh = merge(None, &develop_write(&ParametricAdjustments::default(), None)).unwrap();
    assert!(!fresh.contains("CameraProfile"));
}

/// The user's real sidecars (read-only): decode -> encode into a copy -> decode equal, and
/// every unowned property byte-identical. Also writes changed curves/looks into copies and
/// checks exiftool parses them (when installed).
#[test]
#[ignore = "needs $SIEVE_SAMPLE_XMP_DIR (read-only Lightroom sidecars)"]
fn real_sidecars_round_trip() {
    let dir = PathBuf::from(
        std::env::var("SIEVE_SAMPLE_XMP_DIR")
            .unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal".into()),
    );
    let files: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xmp")))
        .collect();
    assert!(!files.is_empty(), "no sidecars under {}", dir.display());
    let out_dir = tempfile::tempdir().unwrap();
    let (mut with_develop, mut looks, mut curves) = (0, 0, 0);
    let mut modified: Vec<PathBuf> = Vec::new();
    for f in &files {
        let orig = std::fs::read_to_string(f).unwrap();
        let v = parse(&orig).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert!(v.develop_error.is_none(), "{}: {:?}", f.display(), v.develop_error);
        let Some(adj) = v.develop else { continue };
        with_develop += 1;
        looks += usize::from(adj.profile.look.is_some());
        curves += usize::from(!crate::ipc::types::PointCurves::is_identity(&adj.tone_curve.point.master));
        let mut want = develop_write(&adj, None);
        want.rating = v.rating.unwrap_or(0).clamp(-1, 5);
        want.label = v.label.as_deref().and_then(|l| packet::OWNED_LABELS.iter().copied().find(|o| *o == l));
        let out = merge(Some(&orig), &want).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let back = parse(&out).unwrap().develop.unwrap();
        assert_eq!(back, adj, "{}", f.display());
        assert_unowned_identical(&orig, &out, &owned_names(&adj));
        // Unchanged profile: the Look struct is byte-identical.
        if let (Some(a), Some(b)) = (orig.find("<crs:Look>"), orig.find("</crs:Look>")) {
            assert!(out.contains(&orig[a..b]), "{}: Look changed", f.display());
        }
        // A variant with an edited curve + look amount for exiftool.
        if modified.len() < 5 && adj.profile.look.is_some() {
            let mut m = adj.clone();
            m.tone_curve.point.master = vec![[0.0, 5.0], [100.0, 110.0], [255.0, 250.0]];
            m.tone_curve.point.red = vec![[0.0, 0.0], [128.0, 135.0], [255.0, 255.0]];
            if let Some(l) = m.profile.look.as_mut() {
                l.amount = 0.75;
            }
            // The first variant switches to an installed look (full struct from the look file).
            let source = if modified.is_empty() {
                m.profile.look = Some(vivid(1.0));
                super::looks::installed(&vivid(1.0).uuid)
            } else {
                None
            };
            let out = merge(Some(&orig), &develop_write(&m, source.as_deref())).unwrap();
            assert_eq!(parse(&out).unwrap().develop.unwrap(), m);
            let p = out_dir.path().join(f.file_name().unwrap());
            std::fs::write(&p, out).unwrap();
            modified.push(p);
        }
    }
    eprintln!(
        "{} sidecars, {with_develop} with develop settings, {looks} with looks, {curves} custom master curves",
        files.len()
    );
    if let Ok(o) = std::process::Command::new("exiftool")
        .args(["-struct", "-XMP-crs:ToneCurvePV2012", "-XMP-crs:ToneCurvePV2012Red", "-XMP-crs:Look", "-j"])
        .args(&modified)
        .output()
    {
        let json = String::from_utf8_lossy(&o.stdout);
        eprintln!("{json}");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(
            json.matches("\"ToneCurvePV2012\": [\"0, 5\",\"100, 110\",\"255, 250\"]").count(),
            modified.len(),
            "{json}"
        );
        assert_eq!(json.matches("\"Amount\": 0.75").count(), modified.len() - 1, "{json}");
        if super::looks::installed(&vivid(1.0).uuid).is_some() {
            assert!(json.contains("\"Name\": \"Adobe Vivid\"") && json.contains("\"Parameters\""), "{json}");
        }
    }
    let _ = Path::new("");
}

/// Non-RAW: embedded XMP is imported read-only; edits go to `<file name>.xmp`; the JPEG is
/// never modified; the RAW sibling's `<stem>.xmp` is untouched.
#[test]
fn non_raw_sidecar_and_embedded_fallback() {
    use crate::db::{self, repo};
    use crate::ipc::types::ImportOptions;
    use crate::raw::raster::tests::fixtures;
    use crate::xmp::{XmpSync, XmpSyncConfig};

    let dir = tempfile::tempdir().unwrap();
    let shoot = dir.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    let xmp = fixtures::packet(
        "xmp:Rating=\"4\" crs:ProcessVersion=\"15.4\" crs:Exposure2012=\"+0.40\" crs:CameraProfile=\"Embedded\"",
    );
    let jpeg_bytes = fixtures::camera_jpeg(32, 24, 1, Some(&xmp));
    let jpeg = fixtures::write(&shoot, "IMG_0001.JPG", &jpeg_bytes);
    let raw_sidecar = shoot.join("IMG_0001.xmp");
    std::fs::write(&raw_sidecar, "not ours").unwrap();
    let mtime = std::fs::metadata(&jpeg).unwrap().modified().unwrap();

    let catalog = dir.path().join("cat.sqlite");
    let mut conn = db::open(&catalog).unwrap();
    let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: true };
    let summary = repo::import_folder(&mut conn, &shoot, &opts).unwrap();
    assert_eq!(summary.added, 1);
    let id: i64 = conn.query_row("SELECT id FROM images", [], |r| r.get(0)).unwrap();
    let sync = XmpSync::new(XmpSyncConfig { catalog_path: catalog.clone() });

    assert_eq!(sync.refresh_folder(summary.folder_id).unwrap(), 1, "embedded XMP read");
    let rating: u8 = conn.query_row("SELECT rating FROM images WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(rating, 4);
    let adj = repo::get_adjustments(&conn, id).unwrap();
    assert_eq!(adj.exposure, 0.4);
    assert_eq!(adj.profile, ProfileSettings::none());
    assert_eq!(adj.detail, ParametricAdjustments::defaults_for(ImageFormat::Jpeg).detail, "non-RAW defaults");

    // Write: `IMG_0001.JPG.xmp`, JPEG and the RAW-style sidecar untouched.
    repo::set_rating(&mut conn, &[id], 2).unwrap();
    let report = sync.write_images(&[id]).unwrap();
    assert_eq!(report.succeeded, 1, "{:?}", report.failed);
    let side = dir.path().join("shoot").join("IMG_0001.JPG.xmp");
    let written = std::fs::read_to_string(&side).unwrap();
    assert!(written.contains("xmp:Rating=\"2\""));
    assert!(!written.contains("CameraProfile"), "no profile for non-RAW: {written}");
    assert_eq!(std::fs::read(&jpeg).unwrap(), jpeg_bytes);
    assert_eq!(std::fs::metadata(&jpeg).unwrap().modified().unwrap(), mtime);
    assert_eq!(std::fs::read_to_string(&raw_sidecar).unwrap(), "not ours");
    // From now on the sidecar wins over the embedded packet.
    let v = super::packet::parse_for(&written, ImageFormat::Jpeg).unwrap();
    assert_eq!(v.develop.map(|d| d.exposure), Some(0.4));
    assert_eq!(sync.read_images(&[id]).unwrap().succeeded, 1);
    let rating: u8 = conn.query_row("SELECT rating FROM images WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(rating, 2);
}

#[test]
#[ignore = "debug dump"]
fn dump_look_write() {
    let mut adj = parse(LOOK_SIDECAR).unwrap().develop.unwrap();
    adj.profile.look = Some(vivid(1.0));
    adj.tone_curve.point.master = vec![[0.0, 0.0], [128.0, 140.0], [255.0, 255.0]];
    println!("{}", merge(Some(LOOK_SIDECAR), &develop_write(&adj, Some(FAKE_VIVID))).unwrap());
}
