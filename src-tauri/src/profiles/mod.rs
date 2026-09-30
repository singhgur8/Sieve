//! Adobe camera profiles (DCP) and looks, read at runtime from the user's own Adobe
//! installation (Phase 7b parity). Contract seam by the architect; bodies by
//! rust-engine-dev.
//!
//! Licensing rule: profiles and look tables are **read in place, never copied into the app,
//! the catalog, the cache or exports, and never redistributed**. Only the parsed, in-memory
//! form is cached. When nothing is installed Sieve falls back to its LibRaw-matrix base and
//! reports `DevelopWarningCode::ProfileUnavailable` / `LookUnavailable`.
//!
//! Search paths (`SIEVE_CAMERA_PROFILES` / `SIEVE_LOOK_PROFILES` override, `:`-separated):
//! - DCPs: `/Library/Application Support/Adobe/CameraRaw/CameraProfiles` and
//!   `~/Library/Application Support/Adobe/CameraRaw/CameraProfiles` (recursive: `Adobe Standard/`,
//!   `Camera/<model>/`; `Index.dat` is ignored). A DCP is identified by its IFD0 tags
//!   `UniqueCameraModel` (0xC614) and `ProfileName` (0xC6F8), not its file name.
//!   Installed by Adobe DNG Converter (free), Lightroom or Photoshop.
//! - Looks: `/Library/Application Support/Adobe/CameraRaw/Settings` and
//!   `~/Library/Application Support/Adobe/CameraRaw/Settings` (recursive `*.xmp` with
//!   `crs:PresetType="Look"`; keyed by `crs:UUID`).
//!
//! Camera matching: DCP `UniqueCameraModel` is Adobe's normalized "<Make> <Model>" ("Sony
//! ILCE-7M4", "Canon EOS M6 Mark II", "Fujifilm X-M5"). The catalog's make/model (EXIF) is
//! matched case-insensitively against "<Make> <Model>" (make normalized: SONY -> Sony,
//! FUJIFILM -> Fujifilm, NIKON CORPORATION -> Nikon, ...; a model that already starts with
//! the make is used as is), then the bare model.
//!
//! Rendering (ACR/DNG order; the pipeline lives in `develop::pipeline` + `develop::parity`,
//! the maths in `dcp.rs` / `table.rs`):
//! 1. Camera RGB -> white balance -> `ForwardMatrix` (interpolated between illuminant 1 and 2
//!    by the white balance's correlated temperature, inverse-mired weighting) -> XYZ D50 ->
//!    linear ProPhoto; without forward matrices, the interpolated `ColorMatrix` inverted.
//! 2. `ProfileHueSatMap` (same weight; HSV in linear ProPhoto; 2.5D tables commute with
//!    exposure, so the pipeline evaluates them after the exposure/tone gains).
//! 3. Exposure incl. the raw file's baseline exposure + DCP `BaselineExposureOffset`.
//! 4. `ProfileLookTable`, then the look's HSV `LookTable` (at the look amount).
//! 5. `ProfileToneCurve`, or Adobe's default tone curve when absent, composed with the look's
//!    point curve and the user's curves (hue-preserving "RGB tone" application).
//! 6. The look's `RGBTable` (own primaries/gamma, `RGBTableAmount` x amount).
//! 7. The user's remaining settings; output transform.
//!
//! Non-RAW sources skip 1-5 (display-referred); output-referred looks still apply.

pub mod dcp;
pub mod look;
pub mod table;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::ipc::types::{CameraProfileInfo, ImageFormat, LookProfileInfo, ProfileCatalog};

pub use dcp::Dcp;
pub use look::LookProfile;

/// Resolved at startup by `lib.rs` (env overrides, else the macOS defaults above).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProfileConfig {
    pub dcp_dirs: Vec<PathBuf>,
    pub look_dirs: Vec<PathBuf>,
}

impl ProfileConfig {
    pub const SYSTEM_DCP_DIR: &'static str = "/Library/Application Support/Adobe/CameraRaw/CameraProfiles";
    pub const SYSTEM_LOOK_DIR: &'static str = "/Library/Application Support/Adobe/CameraRaw/Settings";

    /// Env overrides, else the system + per-user Adobe directories (missing ones are fine).
    pub fn from_env() -> Self {
        let split = |v: std::ffi::OsString| std::env::split_paths(&v).collect::<Vec<_>>();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let user = |rel: &str| home.as_ref().map(|h| h.join("Library/Application Support/Adobe/CameraRaw").join(rel));
        let dcp_dirs = std::env::var_os("SIEVE_CAMERA_PROFILES").map(split).unwrap_or_else(|| {
            [Some(PathBuf::from(Self::SYSTEM_DCP_DIR)), user("CameraProfiles")].into_iter().flatten().collect()
        });
        let look_dirs = std::env::var_os("SIEVE_LOOK_PROFILES").map(split).unwrap_or_else(|| {
            [Some(PathBuf::from(Self::SYSTEM_LOOK_DIR)), user("Settings")].into_iter().flatten().collect()
        });
        Self { dcp_dirs, look_dirs }
    }
}

/// What the library needs to know about an image to pick its profiles.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraKey {
    pub format: ImageFormat,
    /// EXIF make as stored in the catalog (`CameraInfo.make` + raw EXIF make if available).
    pub make: Option<String>,
    pub model: Option<String>,
}

impl CameraKey {
    /// Candidate Adobe unique camera model names, best first (lower-case).
    pub fn candidates(&self) -> Vec<String> {
        let Some(model) = self.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(make) = self.make.as_deref().map(normalize_make).filter(|m| !m.is_empty()) {
            if model.to_ascii_lowercase().starts_with(&make.to_ascii_lowercase()) {
                out.push(model.to_owned());
            } else {
                out.push(format!("{make} {model}"));
            }
        }
        out.push(model.to_owned());
        out.into_iter().map(|s| s.to_ascii_lowercase()).collect()
    }
}

/// Adobe's spelling of an EXIF make.
pub fn normalize_make(make: &str) -> String {
    let m = make.trim();
    let lower = m.to_ascii_lowercase();
    let known = [
        ("sony", "Sony"),
        ("fujifilm", "Fujifilm"),
        ("canon", "Canon"),
        ("nikon", "Nikon"),
        ("olympus", "Olympus"),
        ("om digital", "OM Digital Solutions"),
        ("panasonic", "Panasonic"),
        ("leica", "Leica"),
        ("pentax", "Pentax"),
        ("ricoh", "Ricoh"),
        ("hasselblad", "Hasselblad"),
        ("sigma", "Sigma"),
        ("apple", "Apple"),
    ];
    for (k, v) in known {
        if lower.starts_with(k) {
            return v.to_owned();
        }
    }
    m.to_owned()
}

#[derive(Debug, Clone)]
struct DcpEntry {
    model_lower: String,
    model: String,
    name: String,
    group: String,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct LookEntry {
    info: LookProfile,
    path: PathBuf,
}

#[derive(Default)]
struct Index {
    dcps: Vec<DcpEntry>,
    looks: Vec<LookEntry>,
}

/// Parsed-profile caches (bounded).
#[derive(Default)]
struct Caches {
    dcps: Vec<((PathBuf, String), Arc<Dcp>)>,
    looks: Vec<(String, Arc<LookProfile>)>,
}

const MAX_CACHED_DCPS: usize = 16;
const MAX_CACHED_LOOKS: usize = 16;

struct Inner {
    config: ProfileConfig,
    index: OnceLock<Index>,
    caches: Mutex<Caches>,
}

/// Managed state: lazily scanned index of installed DCPs and looks, plus an LRU of parsed
/// profiles (parsed DCPs are ~100-500 KB of floats; keep ~16). Instances created with the
/// same config share one index and cache.
#[derive(Clone)]
pub struct ProfileLibrary {
    inner: Arc<Inner>,
}

fn registry() -> &'static Mutex<Vec<Arc<Inner>>> {
    static R: OnceLock<Mutex<Vec<Arc<Inner>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, ext, out, depth + 1);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

/// Reads the first `n` bytes of a file.
fn head(path: &Path, n: usize) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn dcp_group(path: &Path, name: &str) -> String {
    let in_dir = |d: &str| path.components().any(|c| c.as_os_str().eq_ignore_ascii_case(d));
    if in_dir("Adobe Standard") || name.starts_with("Adobe") {
        "Adobe Raw".into()
    } else if in_dir("Camera") {
        "Camera Matching".into()
    } else {
        "Other".into()
    }
}

impl Index {
    fn scan(config: &ProfileConfig) -> Index {
        let mut dcps = Vec::new();
        for dir in &config.dcp_dirs {
            let mut files = Vec::new();
            walk(dir, "dcp", &mut files, 0);
            for path in files {
                // The names sit near the start of Adobe's DCPs; fall back to the whole file.
                let names = head(&path, 8192)
                    .and_then(|b| Dcp::peek_names(&b).ok())
                    .or_else(|| std::fs::read(&path).ok().and_then(|b| Dcp::peek_names(&b).ok()));
                if let Some((model, name)) = names {
                    let group = dcp_group(&path, &name);
                    dcps.push(DcpEntry { model_lower: model.to_ascii_lowercase(), model, name, group, path });
                }
            }
        }
        // UUID -> file from the shared look scan (`xmp::looks`), then the header of each look.
        let mut found: Vec<(String, PathBuf)> = crate::xmp::looks::scan(&config.look_dirs).into_iter().collect();
        found.sort_by(|a, b| a.1.cmp(&b.1));
        let mut looks: Vec<LookEntry> = Vec::new();
        for (_, path) in found {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            match LookProfile::parse_file_opts(&text, false) {
                Ok(Some(mut info)) => {
                    if info.group.is_empty() {
                        info.group = path
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                    }
                    if !looks.iter().any(|l| l.info.uuid == info.uuid) {
                        looks.push(LookEntry { info, path });
                    }
                }
                Ok(None) => {}
                Err(e) => eprintln!("look profile {}: {e}", path.display()),
            }
        }
        Index { dcps, looks }
    }

    /// Adobe model name matched for `camera` and its DCP entries.
    fn camera_dcps(&self, camera: &CameraKey) -> (Option<String>, Vec<&DcpEntry>) {
        if !camera.format.is_raw() {
            return (None, Vec::new());
        }
        for cand in camera.candidates() {
            let found: Vec<&DcpEntry> = self.dcps.iter().filter(|d| d.model_lower == cand).collect();
            if let Some(first) = found.first() {
                return (Some(first.model.clone()), found);
            }
        }
        (None, Vec::new())
    }
}

impl ProfileLibrary {
    pub fn new(config: ProfileConfig) -> Self {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inner) = reg.iter().find(|i| i.config == config) {
            return Self { inner: inner.clone() };
        }
        let inner = Arc::new(Inner { config, index: OnceLock::new(), caches: Mutex::new(Caches::default()) });
        reg.push(inner.clone());
        Self { inner }
    }

    /// The library for [`ProfileConfig::from_env`] (the develop/export engines use this; it
    /// shares its index with the managed state built from the same config).
    pub fn shared() -> ProfileLibrary {
        static S: OnceLock<ProfileLibrary> = OnceLock::new();
        S.get_or_init(|| ProfileLibrary::new(ProfileConfig::from_env())).clone()
    }

    pub fn config(&self) -> &ProfileConfig {
        &self.inner.config
    }

    fn index(&self) -> &Index {
        self.inner.index.get_or_init(|| Index::scan(&self.inner.config))
    }

    /// Scans the installed profiles now (call from a background thread at startup).
    pub fn warm(&self) {
        let _ = self.index();
    }

    /// Adobe unique camera model matched for `camera` (if any DCP is installed for it).
    pub fn camera_model(&self, camera: &CameraKey) -> Option<String> {
        self.index().camera_dcps(camera).0
    }

    /// Profile browser contents for an image (scans on first use; cheap afterwards).
    pub fn catalog(&self, image_id: i64, camera: &CameraKey) -> ProfileCatalog {
        let index = self.index();
        let (camera_model, dcps) = index.camera_dcps(camera);
        let mut camera_profiles: Vec<CameraProfileInfo> = Vec::new();
        for d in dcps {
            if !camera_profiles.iter().any(|p| p.name == d.name) {
                camera_profiles.push(CameraProfileInfo {
                    name: d.name.clone(),
                    group: d.group.clone(),
                    style_id: None,
                });
            }
        }
        let order = |g: &str| match g {
            "Adobe Raw" => 0,
            "Camera Matching" => 1,
            _ => 2,
        };
        camera_profiles.sort_by(|a, b| order(&a.group).cmp(&order(&b.group)).then(a.name.cmp(&b.name)));
        let cands = camera.candidates();
        let looks = index
            .looks
            .iter()
            .map(|l| {
                let i = &l.info;
                let restriction_ok = match &i.camera_model_restriction {
                    None => true,
                    Some(r) => {
                        let r = r.to_ascii_lowercase();
                        cands.contains(&r)
                    }
                };
                let source_ok = camera.format.is_raw() || i.supports_output_referred;
                LookProfileInfo {
                    uuid: i.uuid.clone(),
                    name: i.name.clone(),
                    group: i.group.clone(),
                    supports_amount: i.supports_amount,
                    monochrome: i.monochrome,
                    camera_profile: i.camera_profile.clone(),
                    available: restriction_ok && source_ok,
                    style_id: None,
                }
            })
            .collect();
        let search_dirs = self
            .inner
            .config
            .dcp_dirs
            .iter()
            .chain(&self.inner.config.look_dirs)
            .map(|p| p.display().to_string())
            .collect();
        ProfileCatalog { image_id, camera_model, camera_profiles, looks, luts: Vec::new(), search_dirs }
    }

    /// The parsed DCP named `profile_name` (`crs:CameraProfile`) for `camera`, if installed.
    pub fn dcp(&self, camera: &CameraKey, profile_name: &str) -> Option<Arc<Dcp>> {
        let (_, dcps) = self.index().camera_dcps(camera);
        let entry = dcps.into_iter().find(|d| d.name.eq_ignore_ascii_case(profile_name.trim()))?;
        let key = (entry.path.clone(), entry.name.clone());
        {
            let mut c = self.inner.caches.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = c.dcps.iter().position(|(k, _)| *k == key) {
                let hit = c.dcps.remove(i);
                let d = hit.1.clone();
                c.dcps.push(hit);
                return Some(d);
            }
        }
        let bytes = std::fs::read(&entry.path).ok()?;
        let dcp = match Dcp::parse(&bytes) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                eprintln!("DCP {}: {e}", entry.path.display());
                return None;
            }
        };
        let mut c = self.inner.caches.lock().unwrap_or_else(|e| e.into_inner());
        if c.dcps.len() >= MAX_CACHED_DCPS {
            c.dcps.remove(0);
        }
        c.dcps.push((key, dcp.clone()));
        Some(dcp)
    }

    /// The installed look with `uuid` (`LookSettings.uuid`), tables decoded.
    pub fn look(&self, uuid: &str) -> Option<Arc<LookProfile>> {
        let uuid = uuid.trim().to_ascii_uppercase();
        {
            let mut c = self.inner.caches.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = c.looks.iter().position(|(k, _)| *k == uuid) {
                let hit = c.looks.remove(i);
                let l = hit.1.clone();
                c.looks.push(hit);
                return Some(l);
            }
        }
        let entry = self.index().looks.iter().find(|l| l.info.uuid == uuid)?;
        let text = std::fs::read_to_string(&entry.path).ok()?;
        let mut look = match LookProfile::parse_file(&text) {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => {
                eprintln!("look {}: {e}", entry.path.display());
                return None;
            }
        };
        look.group = entry.info.group.clone();
        let look = Arc::new(look);
        let mut c = self.inner.caches.lock().unwrap_or_else(|e| e.into_inner());
        if c.looks.len() >= MAX_CACHED_LOOKS {
            c.looks.remove(0);
        }
        c.looks.push((uuid, look.clone()));
        Some(look)
    }
}

/// Default raw baseline exposure (EV) Camera Raw applies for a camera (Adobe's per-model
/// `BaselineExposure`, as written by Adobe DNG Converter 17.5 into converted DNGs of the
/// sample cameras). DNG files carry their own. Unknown cameras use Sony's typical value.
pub fn raw_baseline_exposure(make: Option<&str>, model: Option<&str>) -> f32 {
    let key = CameraKey { format: ImageFormat::Arw, make: make.map(str::to_owned), model: model.map(str::to_owned) };
    let known: [(&str, f32); 3] = [("sony ilce-7m4", 0.35), ("canon eos m6 mark ii", 0.2), ("fujifilm x-m5", 1.07)];
    for cand in key.candidates() {
        if let Some((_, v)) = known.iter().find(|(m, _)| *m == cand) {
            return *v;
        }
    }
    let make = make.map(normalize_make).unwrap_or_default();
    match make.as_str() {
        "Canon" => 0.25,
        "Fujifilm" => 0.72,
        _ => 0.35,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_candidates() {
        let k = CameraKey { format: ImageFormat::Arw, make: Some("SONY".into()), model: Some("ILCE-7M4".into()) };
        assert_eq!(k.candidates(), vec!["sony ilce-7m4", "ilce-7m4"]);
        let k = CameraKey {
            format: ImageFormat::Cr3,
            make: Some("Canon".into()),
            model: Some("Canon EOS M6 Mark II".into()),
        };
        assert_eq!(k.candidates()[0], "canon eos m6 mark ii");
        let k = CameraKey { format: ImageFormat::Raf, make: Some("FUJIFILM".into()), model: Some("X-M5".into()) };
        assert_eq!(k.candidates()[0], "fujifilm x-m5");
        assert!(CameraKey { format: ImageFormat::Raf, make: None, model: None }.candidates().is_empty());
        assert_eq!(raw_baseline_exposure(Some("SONY"), Some("ILCE-7M4")), 0.35);
        assert_eq!(raw_baseline_exposure(Some("FUJIFILM"), Some("X-M5")), 1.07);
    }

    #[test]
    fn library_scans_synthetic_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let dcp_dir = dir.path().join("CameraProfiles/Adobe Standard");
        std::fs::create_dir_all(&dcp_dir).unwrap();
        std::fs::write(dcp_dir.join("whatever.dcp"), dcp::tests_support::sample()).unwrap();
        let look_dir = dir.path().join("Settings/Mine");
        std::fs::create_dir_all(&look_dir).unwrap();
        std::fs::write(
            look_dir.join("l.xmp"),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Look"
 crs:UUID="AAAABBBBCCCCDDDDEEEEFFFF00001111" crs:SupportsAmount="True" crs:SupportsOutputReferred="True"
 crs:Clarity2012="+10"><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Mine</rdf:li></rdf:Alt></crs:Name>
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();
        let lib = ProfileLibrary::new(ProfileConfig {
            dcp_dirs: vec![dir.path().join("CameraProfiles")],
            look_dirs: vec![dir.path().join("Settings")],
        });
        let sony = CameraKey { format: ImageFormat::Arw, make: Some("SONY".into()), model: Some("ILCE-7M4".into()) };
        let cat = lib.catalog(7, &sony);
        assert_eq!(cat.image_id, 7);
        assert_eq!(cat.camera_model.as_deref(), Some("Sony ILCE-7M4"));
        assert_eq!(
            cat.camera_profiles,
            vec![CameraProfileInfo { name: "Adobe Standard".into(), group: "Adobe Raw".into(), style_id: None }]
        );
        assert_eq!(cat.looks.len(), 1);
        assert_eq!(cat.looks[0].group, "Mine", "directory name when crs:Group is absent");
        assert!(cat.looks[0].available && cat.looks[0].supports_amount);
        assert!(lib.dcp(&sony, "adobe standard").is_some());
        assert!(lib.dcp(&sony, "Camera ST").is_none());
        let canon = CameraKey { format: ImageFormat::Cr3, make: Some("Canon".into()), model: Some("EOS R5".into()) };
        assert!(lib.catalog(1, &canon).camera_profiles.is_empty());
        let jpeg = CameraKey { format: ImageFormat::Jpeg, make: Some("SONY".into()), model: Some("ILCE-7M4".into()) };
        assert!(lib.catalog(1, &jpeg).camera_profiles.is_empty());
        let look = lib.look("aaaabbbbccccddddeeeeffff00001111").unwrap();
        assert_eq!(look.parameters.clarity, 10.0);
        assert!(lib.look("00000000000000000000000000000000").is_none());
    }

    #[test]
    #[ignore = "needs Adobe DNG Converter / Camera Raw profiles installed"]
    fn installed_library_finds_sample_cameras() {
        let lib = ProfileLibrary::new(ProfileConfig::from_env());
        let t = std::time::Instant::now();
        for (make, model, fmt) in [
            ("SONY", "ILCE-7M4", ImageFormat::Arw),
            ("Canon", "Canon EOS M6 Mark II", ImageFormat::Cr3),
            ("FUJIFILM", "X-M5", ImageFormat::Raf),
        ] {
            let k = CameraKey { format: fmt, make: Some(make.into()), model: Some(model.into()) };
            let cat = lib.catalog(1, &k);
            assert!(cat.camera_profiles.iter().any(|p| p.name == "Adobe Standard"), "{model}: {cat:?}");
            assert!(lib.dcp(&k, "Adobe Standard").is_some());
        }
        eprintln!("scan + lookups: {:?}", t.elapsed());
        let look = lib.look("B952C231111CD8E0ECCF14B86BAA7077").expect("Adobe Color installed");
        assert_eq!(look.name, "Adobe Color");
        assert_eq!(look.tables.len(), 1);
    }
}
