//! Export presets: built-ins from code (`ExportPreset::builtins()`, negative ids, read-only)
//! plus user presets in the catalog table `export_presets` (migration 0006). Owned by
//! rust-engine-dev.
//!
//! - `settings_json` is `ExportSettings` JSON; a stored preset that no longer deserializes or
//!   validates is skipped by `list` (logged), not an error.
//! - Names are trimmed, 1..=100 chars, unique case-insensitively across built-in and user
//!   presets (`invalid_argument` on a clash with another preset).
//! - `list`: built-ins (in `builtins()` order) then user presets by name (case-insensitive).
//! - `save`: `id = None` creates, `Some(id > 0)` overwrites (unknown -> `not_found`),
//!   `Some(id < 0)` (built-in) -> `invalid_argument`. Validates `settings`
//!   (`ExportSettings::validate`; `choose` destinations are allowed).
//! - `delete`: built-in -> `invalid_argument`; unknown -> `not_found`.

use rusqlite::Connection;

use crate::ipc::error::AppResult;
use crate::ipc::types::{ExportPreset, ExportPresetId, ExportSettings};

pub fn list(conn: &Connection) -> AppResult<Vec<ExportPreset>> {
    let _ = conn;
    todo!("rust-engine-dev: export::presets::list")
}

pub fn save(
    conn: &Connection,
    id: Option<ExportPresetId>,
    name: &str,
    settings: &ExportSettings,
) -> AppResult<ExportPreset> {
    let _ = (conn, id, name, settings);
    todo!("rust-engine-dev: export::presets::save")
}

pub fn delete(conn: &Connection, id: ExportPresetId) -> AppResult<()> {
    let _ = (conn, id);
    todo!("rust-engine-dev: export::presets::delete")
}
