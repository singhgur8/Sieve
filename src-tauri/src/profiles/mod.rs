//! Adobe camera profiles (DCP) and looks, read at runtime from the user's own Adobe
//! installation (Phase 7b parity). Contract seam by the architect; bodies belong to
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
//!   `Camera/<model>/`; `Index.dat` is optional and may be ignored). A DCP is identified by its
//!   IFD0 tags `UniqueCameraModel` (0xC614) and `ProfileName` (0xC6F8), not its file name.
//!   Installed by Adobe DNG Converter (free), Lightroom or Photoshop.
//! - Looks: `/Library/Application Support/Adobe/CameraRaw/Settings` and
//!   `~/Library/Application Support/Adobe/CameraRaw/Settings` (recursive `*.xmp` with
//!   `crs:PresetType="Look"`; keyed by `crs:UUID`). E.g. `Adobe/Profiles/Adobe Raw/Adobe
//!   Color.xmp` (UUID B952C231111CD8E0ECCF14B86BAA7077, LookTable E1095149...).
//!
//! Camera matching: DCP `UniqueCameraModel` is Adobe's normalized "<Make> <Model>" ("Sony
//! ILCE-7M4", "Canon EOS M6 Mark II", "Fujifilm X-M5"). Match the catalog's make/model
//! (EXIF) case-insensitively against, in order: "<Make> <Model>" with Adobe/LibRaw make
//! normalization (SONY -> Sony, FUJIFILM -> Fujifilm, Canon model already prefixed), then the
//! bare model. `list_profiles` reports the matched model.
//!
//! Rendering (ACR/DNG order; the pipeline stages live in `develop::pipeline` +
//! `develop::parity`, the maths in `dcp.rs` / `look.rs`):
//! 1. Camera RGB -> white balance -> `ForwardMatrix` (interpolated between illuminant 1 and 2
//!    by the white balance's correlated temperature, inverse-mired weighting, DNG spec
//!    "Mapping Camera Color Space to CIE XYZ Space") -> XYZ D50 -> linear ProPhoto; without
//!    forward matrices, invert the interpolated `ColorMatrix`.
//! 2. `ProfileHueSatMap` (interpolated with the same weight; HSV in linear ProPhoto; 2.5D or
//!    3D with value encoding per `ProfileHueSatMapEncoding`).
//! 3. Exposure incl. the raw file's `BaselineExposure` + DCP `BaselineExposureOffset`.
//! 4. `ProfileLookTable` (HSV, `ProfileLookTableEncoding`).
//! 5. `ProfileToneCurve`, or Adobe's default ACR3 tone curve when absent.
//! 6. The look (`ProfileSettings.look`), blended by `amount`: its `LookTable` (HSV) or
//!    `RGBTable` (3D RGB, own colour space/gamma per table header), plus its `crs:` parameters
//!    (e.g. Adobe Color's point curve, Adobe Monochrome's grayscale + clarity) applied under
//!    the user's settings.
//! 7. The user's PV2012 settings and Phase 7b groups; output transform.
//!
//! Non-RAW sources skip 1-5 (display-referred); looks still apply.

pub mod dcp;
pub mod look;
pub mod table;

use std::path::PathBuf;
use std::sync::Arc;

use crate::ipc::types::{ImageFormat, ProfileCatalog};

pub use dcp::Dcp;
pub use look::LookProfile;

/// Resolved at startup by `lib.rs` (env overrides, else the macOS defaults above).
#[derive(Debug, Clone)]
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

/// Managed state: lazily scanned index of installed DCPs and looks, plus an LRU of parsed
/// profiles (parsed DCPs are ~100-500 KB of floats; keep ~16).
pub struct ProfileLibrary {
    config: ProfileConfig,
}

impl ProfileLibrary {
    pub fn new(config: ProfileConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ProfileConfig {
        &self.config
    }

    /// Profile browser contents for an image (scans on first use; cheap afterwards).
    pub fn catalog(&self, image_id: i64, camera: &CameraKey) -> ProfileCatalog {
        let _ = (image_id, camera);
        todo!("rust-engine-dev: profiles::ProfileLibrary::catalog")
    }

    /// The parsed DCP named `profile_name` (`crs:CameraProfile`) for `camera`, if installed.
    pub fn dcp(&self, camera: &CameraKey, profile_name: &str) -> Option<Arc<Dcp>> {
        let _ = (camera, profile_name);
        todo!("rust-engine-dev: profiles::ProfileLibrary::dcp")
    }

    /// The installed look with `uuid` (`LookSettings.uuid`), tables decoded.
    pub fn look(&self, uuid: &str) -> Option<Arc<LookProfile>> {
        let _ = uuid;
        todo!("rust-engine-dev: profiles::ProfileLibrary::look")
    }
}
