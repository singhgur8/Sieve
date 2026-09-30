//! Develop presets (catalog table `presets`, migration 0005).
//!
//! - `params_json` is overlaid on neutral defaults when read (like `adjustments`).
//! - `fields_json` is a non-empty, de-duplicated `AdjustmentField[]`.
//! - Names are trimmed, 1..=100 chars, unique case-insensitively (`invalid_argument` on
//!   a clash with another preset). Unknown id -> `not_found`.
//! - Listed by name (case-insensitive).

use rusqlite::Connection;

use crate::ipc::error::AppResult;
use crate::ipc::types::{AdjustmentField, ParametricAdjustments, Preset, PresetId};

pub fn list(conn: &Connection) -> AppResult<Vec<Preset>> {
    let _ = conn;
    todo!("rust-engine-dev: develop::presets::list")
}

pub fn get(conn: &Connection, id: PresetId) -> AppResult<Preset> {
    let _ = (conn, id);
    todo!("rust-engine-dev: develop::presets::get")
}

/// Creates (`id = None`) or overwrites (`Some`) a preset; validates `adjustments`.
pub fn save(
    conn: &Connection,
    id: Option<PresetId>,
    name: &str,
    adjustments: &ParametricAdjustments,
    fields: &[AdjustmentField],
) -> AppResult<Preset> {
    let _ = (conn, id, name, adjustments, fields);
    todo!("rust-engine-dev: develop::presets::save")
}

pub fn delete(conn: &Connection, id: PresetId) -> AppResult<()> {
    let _ = (conn, id);
    todo!("rust-engine-dev: develop::presets::delete")
}
