//! Phase 8: develop writes only add `crs:` keys that differ from the format defaults (or
//! already exist), so an edit on a Lightroom sidecar changes only the edited key(s) plus
//! `xmp:MetadataDate`.

use std::path::PathBuf;

use super::packet::{self, merge, parse, parse_for, Desired};
use super::{catalog_values, crs, masks, store::ImageRow, write_sidecar};
use crate::ipc::types::{ImageFormat, ParametricAdjustments};

const LOOK_SIDECAR: &str = include_str!("fixtures/lightroom_look.xmp");
const DATE: &str = "2026-09-30T12:00:00Z";

/// Lines added to / removed from `a` to get `b` (multiset difference, whitespace-trimmed;
/// a trailing `>` / `/>` closing an attribute list is ignored so an appended attribute only
/// counts once).
fn line_diff(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
    let norm = |l: &str| l.trim().trim_end_matches("/>").trim_end_matches('>').to_owned();
    let mut left: Vec<String> = a.lines().map(norm).collect();
    let mut added = Vec::new();
    for l in b.lines().map(norm) {
        match left.iter().position(|x| *x == l) {
            Some(i) => {
                left.swap_remove(i);
            }
            None => added.push(l),
        }
    }
    (added, left)
}

fn key_of(line: &str) -> String {
    line.split('=').next().unwrap_or("").trim().trim_start_matches('<').to_owned()
}

fn assert_only_keys(orig: &str, out: &str, keys: &[&str], ctx: &str) {
    let (added, removed) = line_diff(orig, out);
    for l in added.iter().chain(&removed) {
        assert!(
            keys.contains(&key_of(l).as_str()),
            "{ctx}: unexpected change {l:?}\nadded {added:?}\nremoved {removed:?}"
        );
    }
}

fn write(adj: &ParametricAdjustments, format: Option<ImageFormat>) -> Desired {
    let mut edits = crs::encode(adj);
    let (seqs, name) = crs::encode_curves(adj);
    edits.push(name);
    Desired {
        develop: edits,
        rating: 2,
        pick: crate::ipc::types::PickFlag::Unflagged,
        label: None,
        tags: Vec::new(),
        metadata_date: DATE.into(),
        seqs,
        profile: Some(packet::ProfileWrite { settings: adj.profile.clone(), look_source: None }),
        format,
        capture_time: None,
    }
}

#[test]
fn edit_adds_only_the_edited_key() {
    let adj = parse(LOOK_SIDECAR).unwrap().develop.unwrap();
    // Unchanged: only the metadata date.
    let out = merge(Some(LOOK_SIDECAR), &write(&adj, None)).unwrap();
    assert_only_keys(LOOK_SIDECAR, &out, &["xmp:MetadataDate"], "unchanged");
    for k in
        ["GrayMixerRed", "PostCropVignetteMidpoint", "GrainSize", "CropTop", "HasCrop", "ColorNoiseReductionDetail"]
    {
        assert!(!out.contains(&format!("crs:{k}=")), "{k} added:\n{out}");
    }
    // A default-valued key the sidecar lacks is added once it differs from the default.
    let mut m = adj.clone();
    m.effects.vignette.amount = -12.0;
    m.exposure += 0.25;
    let out = merge(Some(LOOK_SIDECAR), &write(&m, None)).unwrap();
    assert_only_keys(
        LOOK_SIDECAR,
        &out,
        &["xmp:MetadataDate", "crs:Exposure2012", "crs:PostCropVignetteAmount"],
        "edit",
    );
    assert_eq!(parse(&out).unwrap().develop.unwrap(), m);
    // ... and kept (at its default) once present, so Lightroom sees the reset.
    m.effects.vignette.amount = 0.0;
    let out2 = merge(Some(&out), &write(&m, None)).unwrap();
    assert!(out2.contains("crs:PostCropVignetteAmount=\"0\""), "{out2}");
    assert_eq!(parse(&out2).unwrap().develop.unwrap(), m);
}

#[test]
fn fresh_packets_omit_defaults_and_round_trip() {
    for format in [ImageFormat::Arw, ImageFormat::Jpeg] {
        let d = ParametricAdjustments::defaults_for(format);
        let out = merge(None, &write(&d, Some(format))).unwrap();
        for k in ["crs:Exposure2012", "crs:GrayMixerRed", "crs:HasCrop", "crs:Sharpness", "ToneCurve"] {
            assert!(!out.contains(k), "{format:?} {k}: {out}");
        }
        for k in ["crs:ProcessVersion", "crs:WhiteBalance", "crs:HasSettings"] {
            assert!(out.contains(k), "{format:?} {k}: {out}");
        }
        assert_eq!(parse_for(&out, format).unwrap().develop.unwrap(), d, "{format:?}");
        // A RAW default that is not a JPEG default (sharpening 40) is written for a JPEG.
        let mut m = d.clone();
        m.detail.sharpening.amount = 40.0;
        m.contrast = 10.0;
        m.tone_curve.point.green = vec![[0.0, 0.0], [120.0, 128.0], [255.0, 255.0]];
        let out = merge(None, &write(&m, Some(format))).unwrap();
        assert_eq!(out.contains("crs:Sharpness="), format == ImageFormat::Jpeg, "{out}");
        assert!(out.contains("<crs:ToneCurvePV2012Green>") && !out.contains("<crs:ToneCurvePV2012Red>"), "{out}");
        assert_eq!(parse_for(&out, format).unwrap().develop.unwrap(), m, "{format:?}");
    }
}

#[test]
fn legacy_split_toning_keeps_default_blending() {
    // Absent blending reads as 100 next to legacy split toning: a default 50 must be written.
    let src = NO_BLENDING;
    let mut adj = parse(src).unwrap().develop.unwrap();
    assert_eq!(adj.color_grading.blending, 100.0);
    adj.color_grading.blending = 50.0;
    let out = merge(Some(src), &write(&adj, None)).unwrap();
    assert!(out.contains("crs:ColorGradeBlending=\"50\""), "{out}");
    assert_eq!(parse(&out).unwrap().develop.unwrap(), adj);
    // Unchanged (100): nothing added.
    adj.color_grading.blending = 100.0;
    let out = merge(Some(src), &write(&adj, None)).unwrap();
    assert!(!out.contains("ColorGradeBlending"), "{out}");
}

const NO_BLENDING: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n   xmp:Rating=\"2\"\n   \
crs:ProcessVersion=\"11.0\"\n   crs:WhiteBalance=\"As Shot\"\n   crs:SplitToningShadowHue=\"30\"\n   \
crs:SplitToningShadowSaturation=\"20\"\n   crs:HasSettings=\"True\">\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";

/// Real sidecars (read-only source, copies under `test-data/`): the production write path
/// (`write_sidecar`, masks included) on 5 masked Lightroom sidecars changes only the edited
/// keys + `xmp:MetadataDate`.
#[test]
#[ignore = "reads the user's Lightroom sidecars (writes copies under test-data)"]
fn real_masked_sidecars_minimal_diff() {
    let src_dir = PathBuf::from(
        std::env::var("SIEVE_SAMPLE_XMP_DIR")
            .unwrap_or_else(|_| "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal".into()),
    );
    let out_dir = PathBuf::from(
        std::env::var("SIEVE_TEST_OUT")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../test-data/xmp-minimal").into()),
    );
    assert!(!out_dir.starts_with(&src_dir), "never write next to the originals");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&src_dir)
        .into_iter()
        .flatten()
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
        .collect();
    files.sort();
    let mut done = 0;
    for f in files {
        let orig = std::fs::read_to_string(&f).unwrap();
        let Some(read) = masks::read(&orig).unwrap() else { continue };
        if read.groups.is_empty() {
            continue;
        }
        let v = parse_for(&orig, ImageFormat::Arw).unwrap();
        let mut adj = v.develop.clone().unwrap();
        adj.masks = read.groups.clone();
        let (rating, pick, color_label) = catalog_values(&v, 0);
        let copy = out_dir.join(f.file_name().unwrap());
        // write_sidecar refuses to create a sidecar for a missing original; a placeholder stands in.
        let raw = copy.with_extension("ARW");
        std::fs::write(&raw, b"").unwrap();
        let row = ImageRow {
            id: 1,
            path: copy.with_extension("ARW"),
            rating,
            pick,
            color_label,
            meta_updated_at: None,
            xmp_mtime_ms: None,
            captured_at_ms: None,
            exif_captured_at_ms: None,
            capture_time_source: crate::ipc::types::CaptureTimeSource::Exif,
        };
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        let run = |adj: &ParametricAdjustments, keys: &[&str], what: &str| {
            std::fs::write(&copy, &orig).unwrap();
            write_sidecar(&copy, &row, &[], Some(adj), false).unwrap();
            let out = std::fs::read_to_string(&copy).unwrap();
            assert_only_keys(&orig, &out, keys, &format!("{stem} {what}"));
            let back = parse_for(&out, ImageFormat::Arw).unwrap().develop.unwrap();
            let back_masks = masks::read(&out).unwrap().unwrap().groups;
            let mut want = adj.clone();
            want.masks = Vec::new();
            assert_eq!(back, want, "{stem} {what}");
            assert_eq!(back_masks, adj.masks, "{stem} {what}");
            let (added, removed) = line_diff(&orig, &out);
            eprintln!("{stem} {what}: +{added:?} -{removed:?}");
            std::fs::write(out_dir.join(format!("{stem}.{what}.xmp")), &out).unwrap();
        };
        run(&adj, &["xmp:MetadataDate"], "unchanged");
        let mut m = adj.clone();
        m.exposure = (m.exposure + 0.3).min(5.0);
        run(&m, &["xmp:MetadataDate", "crs:Exposure2012"], "exposure");
        let mut m = adj.clone();
        m.masks[0].adjustments.clarity = (m.masks[0].adjustments.clarity + 10.0).min(100.0);
        run(&m, &["xmp:MetadataDate", "crs:LocalClarity2012"], "local-clarity");
        let mut m = adj.clone();
        m.effects.grain.amount = 15.0;
        run(&m, &["xmp:MetadataDate", "crs:GrainAmount"], "grain");
        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_file(&raw);
        done += 1;
        if done == 5 {
            break;
        }
    }
    assert_eq!(done, 5);
}
