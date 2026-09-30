//! `.cube` LUT library and evaluation (Phase 5). Owned by rust-engine-dev.
//!
//! Fixed surface: [`LutLibrary`] (`new`, `dir`, `list`, `import`, `delete`, `load`),
//! [`references`], [`Lut`] (`parse`, `apply`). Phase 9 (reference-match grading) writes
//! generated `.cube` files into the same directory, so the directory is the source of truth
//! (no catalog table): LUTs are shared by every catalog, like Lightroom profiles.
//!
//! - Library dir: `<app_data_dir>/luts/` (`$SIEVE_LUTS` overrides), created at startup.
//!   Files are `<id>.cube`; `id` = slug of the source name + `-` + 8 hex digits of a content
//!   hash (`[a-z0-9-]{1,64}`, see `ipc::types::is_valid_lut_id`), so re-importing the same
//!   file is idempotent (returns the existing entry).
//! - `.cube` (Adobe/Resolve spec): `TITLE`, `LUT_1D_SIZE` (2..=65536) or `LUT_3D_SIZE`
//!   (2..=65), `DOMAIN_MIN`/`DOMAIN_MAX` (and Resolve's `LUT_1D_INPUT_RANGE` /
//!   `LUT_3D_INPUT_RANGE`), `#` comments, red-fastest ordering. Invalid files ->
//!   `invalid_argument` on import (the file is not copied).
//! - Evaluation: 3D tetrahedral interpolation (trilinear acceptable for tests' reference
//!   values), 1D linear per channel; input is display-referred sRGB-encoded 0..=1, clamped
//!   to the domain; `amount` 0..=100 blends linearly with the input.
//! - `list` scans the dir (headers only), skips unparsable files, sorts by name.
//! - Parsed LUTs are cached in memory by id (`load`); `delete` evicts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;

use crate::ipc::error::AppResult;
use crate::ipc::types::{LutId, LutInfo, LutKind};

/// A parsed LUT ready for evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut {
    pub kind: LutKind,
    pub size: u32,
    pub title: Option<String>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// RGB triples, red fastest (`size^3` for 3D, `size` for 1D).
    pub table: Vec<[f32; 3]>,
}

impl Lut {
    /// Parses `.cube` text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let _ = text;
        todo!("rust-engine-dev: Lut::parse")
    }

    /// Maps one sRGB-encoded pixel (0..=1) and blends by `amount` (0..=100).
    pub fn apply(&self, rgb: [f32; 3], amount: f32) -> [f32; 3] {
        let _ = (rgb, amount);
        todo!("rust-engine-dev: Lut::apply")
    }
}

/// Managed Tauri state: the LUT directory and a parsed-LUT cache. Cheap to clone.
#[derive(Debug, Clone)]
pub struct LutLibrary {
    dir: PathBuf,
}

impl LutLibrary {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> AppResult<Vec<LutInfo>> {
        todo!("rust-engine-dev: LutLibrary::list")
    }

    /// Validates `path` as a `.cube`, copies it into the library (atomic temp + rename).
    pub fn import(&self, path: &Path) -> AppResult<LutInfo> {
        let _ = path;
        todo!("rust-engine-dev: LutLibrary::import")
    }

    /// Removes `<id>.cube` (`not_found` if absent). Reference checks are the caller's job.
    pub fn delete(&self, id: &str) -> AppResult<()> {
        let _ = id;
        todo!("rust-engine-dev: LutLibrary::delete")
    }

    /// Parsed LUT, `Ok(None)` if the id is not in the library.
    pub fn load(&self, id: &str) -> AppResult<Option<Arc<Lut>>> {
        let _ = id;
        todo!("rust-engine-dev: LutLibrary::load")
    }
}

/// Number of images whose stored adjustments reference `id`
/// (`json_extract(params_json, '$.lut.id') = ?`).
pub fn references(conn: &Connection, id: &LutId) -> AppResult<u32> {
    let _ = (conn, id);
    todo!("rust-engine-dev: lut::references")
}
