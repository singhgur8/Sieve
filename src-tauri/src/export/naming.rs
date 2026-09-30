//! File-name template expansion (suggested seam; owned by rust-engine-dev).
//! Grammar and validation: `ipc::types::parse_filename_template` / `TemplatePart`.

use crate::ipc::types::{RawImageEntry, TemplatePart};

/// Expands parsed `parts` for `entry` at `seq` (= position in `ids` + `startNumber`) into a
/// file stem (no extension): token values sanitized (`/ \ :` and control chars -> `_`),
/// leading `.`/spaces stripped, truncated to 240 bytes on a char boundary, never empty
/// (falls back to the RAW's stem). `{date}` uses `capture.capturedAtMs` as naive wall-clock
/// time (UTC fields), else `fileMtimeMs` in local time.
pub fn expand(parts: &[TemplatePart], entry: &RawImageEntry, seq: u64) -> String {
    let _ = (parts, entry, seq);
    todo!("rust-engine-dev: export::naming::expand")
}
