//! Style library import + key-exact preset application (rust-engine-dev).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::preset_file::PresetSettings;
use super::*;
use crate::db::open_in_memory;
use crate::ipc::error::ErrorKind;
use crate::xmp::crs::{self, CRS_NS};

const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";

fn preset_xmp(name: &str, attrs: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="{CRS}" crs:PresetType="Normal" crs:UUID="0FB70F2CF30749A981E3FE67A48128D2"
 crs:SupportsAmount="False" crs:Version="13.2" crs:ProcessVersion="11.0" {attrs} crs:HasSettings="True">
 <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name>{body}
</rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}

fn look_xmp(name: &str, uuid: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:crs="{CRS}" crs:PresetType="Look" crs:UUID="{uuid}" crs:SupportsAmount="True"
 crs:SupportsOutputReferred="True" crs:CameraProfile="Adobe Standard" crs:Clarity2012="+10">
 <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name>
</rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}

const CUBE: &str = "TITLE \"Warm\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

#[test]
fn import_folder_groups_skips_and_replaces() {
    let _registry = test_registry_guard();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("MyStyles");
    let film = root.join("Film");
    let deep = root.join("A/B/C");
    for d in [&root, &film, &deep] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(root.join("warm.xmp"), preset_xmp("Warm", r#"crs:Exposure2012="+0.50" crs:Vibrance="+20""#, ""))
        .unwrap();
    // Same crs:Name twice in one folder -> "Warm (2)".
    std::fs::write(root.join("warm copy.xmp"), preset_xmp("warm", r#"crs:Contrast2012="+5""#, "")).unwrap();
    std::fs::write(
        film.join("old.lrtemplate"),
        r#"s = { title = "Old Film", type = "Develop", value = { settings = { Saturation = -20, GrainAmount = 30 } } }"#,
    )
    .unwrap();
    std::fs::write(
        film.join("editor.lrtemplate"),
        r#"s = { title = "Photoshop", type = "External Editor", value = { editorPath = "/x" } }"#,
    )
    .unwrap();
    let uuid = "ABCDEF0123456789ABCDEF0123456789";
    std::fs::write(film.join("look.xmp"), look_xmp("Film Look", uuid)).unwrap();
    std::fs::write(film.join("warm.cube"), CUBE).unwrap();
    std::fs::write(film.join("bad.cube"), "LUT_3D_SIZE 3\n0 0 0\n").unwrap();
    std::fs::write(deep.join("camera.dcp"), crate::profiles::dcp::tests_support::sample()).unwrap();
    // A photo sidecar and an unrelated file.
    std::fs::write(deep.join("IMG_0001.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
    std::fs::write(deep.join("readme.txt"), "hi").unwrap();

    let mut conn = open_in_memory();
    let luts = LutLibrary::new(dir.path().join("luts"));
    let report = import_folder(&mut conn, &luts, &root).unwrap();
    assert_eq!((report.presets, report.profiles), (3, 3), "{report:?}");
    assert_eq!(report.group_ids.len(), 3);
    let reasons: Vec<&str> = report.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(report.skipped.len(), 3, "{reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("External Editor")));
    assert!(reasons.iter().any(|r| r.starts_with("invalid .cube")));
    assert!(reasons.iter().any(|r| r.contains("photo sidecar")));

    let lib = list(&conn).unwrap();
    let names: Vec<&str> = lib.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["User Presets", "B / C", "MyStyles", "MyStyles / Film", "LUTs"]);
    let top = lib.groups.iter().find(|g| g.name == "MyStyles").unwrap();
    let pn: Vec<&str> = top.presets.iter().map(|p| p.name.as_str()).collect();
    // Files are read in name order: "warm copy.xmp" (crs:Name "warm") first.
    assert_eq!(pn, ["warm", "Warm (2)"]);
    let warm = &top.presets[1];
    assert_eq!(warm.setting_keys, ["Exposure2012", "Vibrance"]);
    assert_eq!(warm.fields, [AdjustmentField::Exposure, AdjustmentField::Vibrance]);
    assert_eq!(warm.source_format, StyleSourceFormat::XmpPreset);
    let film_g = lib.groups.iter().find(|g| g.name == "MyStyles / Film").unwrap();
    assert_eq!(film_g.presets[0].name, "Old Film");
    assert_eq!(film_g.presets[0].source_format, StyleSourceFormat::Lrtemplate);
    let kinds: Vec<StyleProfileKind> = film_g.profiles.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [StyleProfileKind::Look, StyleProfileKind::Lut]);
    let look = &film_g.profiles[0];
    assert_eq!(look.look_uuid.as_deref(), Some(uuid));
    assert!(look.supports_amount && look.available);
    assert_eq!(look.camera_profile.as_deref(), Some("Adobe Standard"));
    let lut = &film_g.profiles[1];
    assert!(lut.lut_id.as_deref().unwrap().starts_with("warm-"));
    assert!(Path::new(&lut.source_path).starts_with(dir.path().join("luts")));
    let dcp_g = lib.groups.iter().find(|g| g.name == "B / C").unwrap();
    assert_eq!(dcp_g.profiles[0].kind, StyleProfileKind::CameraProfile);
    assert_eq!(dcp_g.profiles[0].camera_model.as_deref(), Some("Sony ILCE-7M4"));

    // The render engine and XMP writer resolve the imported look.
    let lp = crate::profiles::ProfileLibrary::shared().look(uuid).expect("imported look resolvable");
    assert_eq!(lp.parameters.clarity, 10.0);
    assert!(crate::xmp::looks::installed(uuid).is_some());

    // Apply: exactly the preset's keys; Sieve presets keep the fields path.
    let base = ParametricAdjustments { exposure: -1.0, contrast: 30.0, saturation: 7.0, ..Default::default() };
    let out = resolve_preset(&conn, warm.id, &base).unwrap();
    assert_eq!((out.exposure, out.vibrance, out.contrast, out.saturation), (0.5, 20.0, 30.0, 7.0));
    let old = resolve_preset(&conn, film_g.presets[0].id, &base).unwrap();
    assert_eq!((old.saturation, old.effects.grain.amount, old.exposure), (-20.0, 30.0, -1.0));

    // Profiles listed for a Sony RAW (DCP camera match) incl. the LUT.
    let camera = CameraKey { format: ImageFormat::Arw, make: Some("Sony".into()), model: Some("ILCE-7M4".into()) };
    let mut cat = ProfileCatalog {
        image_id: 1,
        camera_model: None,
        camera_profiles: Vec::new(),
        looks: Vec::new(),
        luts: Vec::new(),
        search_dirs: Vec::new(),
    };
    extend_profile_catalog(&conn, &mut cat, &camera).unwrap();
    assert!(cat.looks.iter().any(|l| l.uuid == uuid && l.style_id.is_some()));
    assert_eq!(cat.luts.len(), 1);

    // Re-import replaces the groups' items (same group ids, no duplicates).
    std::fs::remove_file(root.join("warm copy.xmp")).unwrap();
    let again = import_folder(&mut conn, &luts, &root).unwrap();
    let mut a = again.group_ids.clone();
    let mut b = report.group_ids.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b);
    assert_eq!(again.presets, 2);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM presets", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);

    // Removing the look's group unregisters it.
    let fg = list(&conn).unwrap().groups.into_iter().find(|g| g.name == "MyStyles / Film").unwrap();
    remove_group(&mut conn, fg.id).unwrap();
    assert!(crate::profiles::imported_look_path(uuid).is_none());

    // Errors.
    assert_eq!(import_folder(&mut conn, &luts, &dir.path().join("nope")).unwrap_err().kind, ErrorKind::NotFound);
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("x.txt"), "").unwrap();
    assert_eq!(import_folder(&mut conn, &luts, &empty).unwrap_err().kind, ErrorKind::InvalidArgument);
}

// ---------------------------------------------------------------------------
// Key-exact application check (also run on the user's real preset folders below).
// ---------------------------------------------------------------------------

/// Every owned `crs:` property of `adj` as written to a sidecar (scalars, curves, profile,
/// mask ids): the vocabulary presets are expressed in.
fn properties(adj: &ParametricAdjustments) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in crs::encode(adj) {
        m.insert(e.name, e.value.unwrap_or_default());
    }
    let (seqs, name) = crs::encode_curves(adj);
    for s in seqs {
        m.insert(s.name.to_owned(), s.items.unwrap_or_default().join(" | "));
    }
    m.insert(name.name, name.value.unwrap_or_default());
    m.insert(crs::CAMERA_PROFILE.into(), adj.profile.camera_profile.clone().unwrap_or_default());
    m.insert(
        crs::LOOK.into(),
        adj.profile.look.as_ref().map(|l| format!("{} {}", l.uuid, l.amount)).unwrap_or_default(),
    );
    m.insert(preset_file::MASKS.into(), adj.masks.iter().map(|g| g.id.clone()).collect::<Vec<_>>().join(","));
    m
}

/// Properties a key may change besides itself (derived values of the same Lightroom setting).
fn allowed(keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = keys.to_vec();
    let has = |k: &str| keys.iter().any(|x| x == k);
    let any = |p: &str| keys.iter().any(|x| x.starts_with(p));
    if has("WhiteBalance") || has("Temperature") || has("Tint") {
        out.extend(["WhiteBalance", "Temperature", "Tint"].map(String::from));
    }
    if any("SplitToning") && !has("ColorGradeBlending") {
        out.push("ColorGradeBlending".into());
    }
    if any("Parametric") && any("Parametric") {
        out.extend(["ParametricShadowSplit", "ParametricMidtoneSplit", "ParametricHighlightSplit"].map(String::from));
    }
    if any("ToneCurvePV2012") || has(crs::CURVE_NAME) {
        out.extend(["ToneCurvePV2012", crs::CURVE_NAME].map(String::from));
    }
    if any("Crop") || has("HasCrop") {
        out.extend(["CropTop", "CropLeft", "CropBottom", "CropRight", "CropAngle"].map(String::from));
    }
    if has(crs::CAMERA_PROFILE) {
        out.push(crs::LOOK.into());
    }
    out
}

fn num(v: &str) -> Option<f32> {
    v.trim().trim_start_matches('+').parse::<f32>().ok()
}

/// Checks that applying `settings` to `base` changes only the preset's keys and sets each
/// numeric key to the file's value (clamped to the slider range). Returns the problems.
fn check_exact(settings: &PresetSettings, base: &ParametricAdjustments) -> Vec<String> {
    let mut problems = Vec::new();
    let out = match settings.apply(base) {
        Ok(o) => o,
        Err(e) => return vec![format!("apply failed: {e}")],
    };
    if let Err(e) = out.validate() {
        problems.push(format!("invalid result: {e}"));
    }
    let keys = settings.keys();
    let ok = allowed(&keys);
    let (before, after) = (properties(base), properties(&out));
    for (k, v) in &after {
        if before.get(k) != Some(v) && !ok.contains(k) {
            problems.push(format!("{k} changed ({:?} -> {v:?}) but is not in the preset", before.get(k)));
        }
    }
    let named_wb = settings.scalars.get("WhiteBalance").is_some_and(|w| w.trim() != "Custom");
    for (k, v) in &settings.scalars {
        if named_wb && (k == "Temperature" || k == "Tint") {
            continue;
        }
        let (Some(want), Some(got)) = (num(v), after.get(k).and_then(|g| num(g))) else { continue };
        let range =
            crs::PARITY_SCALARS.iter().find(|f| f.name == k).map(|f| (f.lo, f.hi)).unwrap_or(match k.as_str() {
                "Exposure2012" => (-5.0, 5.0),
                "Temperature" => (2000.0, 50000.0),
                "Tint" => (-150.0, 150.0),
                _ => (-100.0, 100.0),
            });
        let want = want.clamp(range.0, range.1);
        let splits = k.starts_with("Parametric") && k.ends_with("Split");
        if (want - got).abs() > 1e-3 && !splits {
            problems.push(format!("{k}: file {v}, applied {got}"));
        }
    }
    for (k, items) in &settings.seqs {
        let want = crs::parse_curve(k, items).map(|c| crs::format_curve(&c).join(" | ")).unwrap_or_default();
        if after.get(k) != Some(&want) {
            problems.push(format!("{k}: curve not applied"));
        }
    }
    if let Some(l) = &settings.look {
        if out.profile.look.as_ref().map(|x| &x.uuid) != Some(&l.uuid) {
            problems.push("look not applied".into());
        }
    }
    problems
}

/// Deterministic non-neutral bases (so "unchanged" is observable).
fn bases() -> Vec<ParametricAdjustments> {
    let mut a = ParametricAdjustments {
        exposure: 0.35,
        contrast: 12.0,
        highlights: -40.0,
        shadows: 33.0,
        whites: 7.0,
        blacks: -9.0,
        texture: 4.0,
        clarity: 6.0,
        dehaze: 3.0,
        vibrance: 11.0,
        saturation: -3.0,
        white_balance: WhiteBalance::Custom { temperature_k: 5123.0, tint: 7.0 },
        ..Default::default()
    };
    a.hsl.hue.orange = -4.0;
    a.hsl.saturation.blue = -13.0;
    a.hsl.luminance.orange = 6.0;
    a.tone_curve.parametric.darks = 5.0;
    a.tone_curve.point.master = vec![[0.0, 8.0], [128.0, 131.0], [255.0, 250.0]];
    a.color_grading.shadows.hue = 210.0;
    a.color_grading.shadows.saturation = 8.0;
    a.calibration.blue.saturation = 15.0;
    a.detail.sharpening.amount = 55.0;
    a.detail.noise_reduction.luminance = 12.0;
    a.effects.vignette.amount = -11.0;
    a.effects.grain.amount = 9.0;
    a.lut = Some(LutRef { id: "keep-00000000".into(), amount: 60.0 });
    let b = ParametricAdjustments::default();
    vec![a, b]
}

#[test]
fn synthetic_presets_apply_exactly_their_keys() {
    let mut s = PresetSettings::default();
    for (k, v) in [("Exposure2012", "+9.0"), ("Shadows2012", "-150"), ("SplitToningHighlightHue", "40")] {
        s.scalars.insert(k.into(), v.into());
    }
    s.seqs.insert("ToneCurvePV2012Red".into(), vec!["0, 0".into(), "128, 140".into(), "255, 255".into()]);
    for b in bases() {
        assert_eq!(check_exact(&s, &b), Vec::<String>::new());
    }
    // The checker catches a key that leaks.
    let mut bad = s.clone();
    bad.scalars.insert("Vibrance".into(), "+50".into());
    let mut out_keys = bad.clone();
    out_keys.scalars.remove("Vibrance");
    let leaked = bad.apply(&bases()[0]).unwrap();
    let before = properties(&bases()[0]);
    let after = properties(&leaked);
    assert!(allowed(&out_keys.keys()).iter().all(|k| k != "Vibrance"));
    assert_ne!(before.get("Vibrance"), after.get("Vibrance"));
}

/// Imports real preset/profile folders and checks every preset: `SIEVE_STYLE_DIRS`
/// (`:`-separated; default: the read-only copies in the main checkout's `test-data/styles`
/// plus Adobe's installed Camera Raw Settings, read in place).
/// `cargo test --lib real_preset_folders -- --ignored --nocapture`
#[test]
#[ignore = "needs the user's preset folders (test-data/styles) / Adobe Camera Raw Settings"]
fn real_preset_folders_apply_exactly_their_keys() {
    let _registry = test_registry_guard();
    let dirs: Vec<PathBuf> = match std::env::var_os("SIEVE_STYLE_DIRS") {
        Some(v) => std::env::split_paths(&v).collect(),
        None => vec![
            PathBuf::from("/Users/gurjotsingh/Developer/Sieve/test-data/styles"),
            PathBuf::from(crate::profiles::ProfileConfig::SYSTEM_LOOK_DIR),
        ],
    };
    let tmp = tempfile::tempdir().unwrap();
    let luts = LutLibrary::new(tmp.path().join("luts"));
    let mut conn = open_in_memory();
    let t = std::time::Instant::now();
    let (mut presets, mut profiles, mut skipped) = (0, 0, 0);
    for d in &dirs {
        if !d.is_dir() {
            eprintln!("skip missing {}", d.display());
            continue;
        }
        let r = import_folder(&mut conn, &luts, d).unwrap();
        eprintln!(
            "{}: {} groups, {} presets, {} profiles, {} skipped",
            d.display(),
            r.group_ids.len(),
            r.presets,
            r.profiles,
            r.skipped.len()
        );
        let mut reasons: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for s in &r.skipped {
            reasons.entry(s.reason.chars().take(70).collect()).or_default().push(&s.path);
        }
        for (why, paths) in reasons {
            eprintln!("   skipped {:>4} x {why}", paths.len());
            for p in paths.iter().take(20) {
                eprintln!("      {p}");
            }
        }
        presets += r.presets;
        profiles += r.profiles;
        skipped += r.skipped.len();
    }
    eprintln!("import: {presets} presets, {profiles} profiles, {skipped} skipped in {:?}", t.elapsed());
    assert!(presets > 0);

    let ids: Vec<(PresetId, String)> = conn
        .prepare("SELECT id, name FROM presets WHERE settings_json IS NOT NULL")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut failures = Vec::new();
    let (mut keys_total, mut with_warnings, mut with_masks, mut with_look) = (0usize, 0usize, 0usize, 0usize);
    let mut warning_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (id, name) in &ids {
        let s = preset_settings(&conn, *id).unwrap().unwrap();
        keys_total += s.keys().len();
        with_masks += usize::from(s.masks.is_some());
        with_look += usize::from(s.look.is_some());
        let warnings: String =
            conn.query_row("SELECT warnings_json FROM presets WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        let w: Vec<String> = serde_json::from_str(&warnings).unwrap();
        with_warnings += usize::from(!w.is_empty());
        for x in w {
            *warning_counts.entry(x).or_default() += 1;
        }
        for (i, b) in bases().iter().enumerate() {
            // Through the catalog path (`resolve_preset`) as the app does.
            let via_catalog = resolve_preset(&conn, *id, b).unwrap();
            assert_eq!(via_catalog, s.apply(b).unwrap());
            for p in check_exact(&s, b) {
                failures.push(format!("{name} (base {i}): {p}"));
            }
        }
    }
    eprintln!(
        "checked {} presets x {} bases: {} keys total (mean {:.1}/preset), {} with a look, {} with masks, {} with ignored settings",
        ids.len(),
        bases().len(),
        keys_total,
        keys_total as f64 / ids.len().max(1) as f64,
        with_look,
        with_masks,
        with_warnings
    );
    for (w, n) in &warning_counts {
        eprintln!("   warning {n:>4} x {w}");
    }
    for f in failures.iter().take(40) {
        eprintln!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} key-exactness failures", failures.len());
    let _ = CRS_NS;
}
