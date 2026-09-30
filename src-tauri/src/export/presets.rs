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

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::now_ms;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{ExportPreset, ExportPresetId, ExportSettings};

fn user_presets(conn: &Connection) -> AppResult<Vec<ExportPreset>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, settings_json, created_at, updated_at FROM export_presets ORDER BY name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, json, created_at_ms, updated_at_ms) = row?;
        let settings = match serde_json::from_str::<ExportSettings>(&json) {
            Ok(s) => match s.validate() {
                Ok(()) => s,
                Err(e) => {
                    eprintln!("export preset {id} ({name}) skipped: {e}");
                    continue;
                }
            },
            Err(e) => {
                eprintln!("export preset {id} ({name}) skipped: {e}");
                continue;
            }
        };
        out.push(ExportPreset { id, name, built_in: false, settings, created_at_ms, updated_at_ms });
    }
    Ok(out)
}

pub fn list(conn: &Connection) -> AppResult<Vec<ExportPreset>> {
    let mut out = ExportPreset::builtins();
    out.extend(user_presets(conn)?);
    Ok(out)
}

fn clean_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    let len = n.chars().count();
    if !(1..=100).contains(&len) {
        return Err(AppError::invalid("preset name must be 1..=100 characters"));
    }
    Ok(n.to_owned())
}

pub fn save(
    conn: &Connection,
    id: Option<ExportPresetId>,
    name: &str,
    settings: &ExportSettings,
) -> AppResult<ExportPreset> {
    if id.is_some_and(|i| i <= 0) {
        return Err(AppError::invalid("built-in presets are read-only; save a copy under a new name"));
    }
    let name = clean_name(name)?;
    settings.validate().map_err(AppError::invalid)?;
    let lower = name.to_lowercase();
    if ExportPreset::builtins().iter().any(|b| b.name.to_lowercase() == lower) {
        return Err(AppError::invalid(format!("a built-in preset is already named {name:?}")));
    }
    let clash: Option<i64> = conn
        .query_row("SELECT id FROM export_presets WHERE name = ?1 COLLATE NOCASE", [&name], |r| r.get(0))
        .optional()?;
    if clash.is_some_and(|c| Some(c) != id) {
        return Err(AppError::invalid(format!("a preset is already named {name:?}")));
    }
    let json = serde_json::to_string(settings)?;
    let now = now_ms();
    let id = match id {
        None => {
            conn.execute(
                "INSERT INTO export_presets (name, settings_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
                params![name, json, now],
            )?;
            conn.last_insert_rowid()
        }
        Some(id) => {
            let n = conn.execute(
                "UPDATE export_presets SET name = ?2, settings_json = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, name, json, now],
            )?;
            if n == 0 {
                return Err(AppError::not_found(format!("export preset {id}")));
            }
            id
        }
    };
    let (created_at_ms, updated_at_ms): (i64, i64) =
        conn.query_row("SELECT created_at, updated_at FROM export_presets WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(ExportPreset { id, name, built_in: false, settings: settings.clone(), created_at_ms, updated_at_ms })
}

pub fn delete(conn: &Connection, id: ExportPresetId) -> AppResult<()> {
    if id <= 0 {
        return Err(AppError::invalid("built-in presets cannot be deleted"));
    }
    if conn.execute("DELETE FROM export_presets WHERE id = ?1", [id])? == 0 {
        return Err(AppError::not_found(format!("export preset {id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    fn settings() -> ExportSettings {
        ExportPreset::builtins()[1].settings.clone()
    }

    #[test]
    fn crud_and_rules() {
        let conn = open_in_memory();
        let b = list(&conn).unwrap();
        assert_eq!(b.len(), 3);
        assert!(b.iter().all(|p| p.built_in && p.id < 0));

        let p = save(&conn, None, "  Web small ", &settings()).unwrap();
        assert_eq!(p.name, "Web small");
        assert!(p.id > 0 && !p.built_in && p.created_at_ms > 0);
        let a = save(&conn, None, "album", &settings()).unwrap();
        let names: Vec<String> = list(&conn).unwrap().into_iter().skip(3).map(|p| p.name).collect();
        assert_eq!(names, vec!["album", "Web small"]);

        // Clashes (case-insensitive), incl. built-ins.
        assert_eq!(save(&conn, None, "WEB SMALL", &settings()).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, None, "web 2048 srgb", &settings()).unwrap_err().kind, ErrorKind::InvalidArgument);
        // Renaming itself to a different case is fine.
        let r = save(&conn, Some(p.id), "WEB SMALL", &settings()).unwrap();
        assert_eq!((r.id, r.name.as_str()), (p.id, "WEB SMALL"));
        assert_eq!(save(&conn, Some(p.id), "album", &settings()).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, Some(999), "x", &settings()).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(save(&conn, Some(-1), "x", &settings()).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, None, "   ", &settings()).unwrap_err().kind, ErrorKind::InvalidArgument);
        let mut bad = settings();
        bad.resize.resolution_ppi = 0;
        assert_eq!(save(&conn, None, "bad", &bad).unwrap_err().kind, ErrorKind::InvalidArgument);

        // A corrupt stored preset is skipped.
        conn.execute("UPDATE export_presets SET settings_json = '{}' WHERE id = ?1", [a.id]).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 4);

        assert_eq!(delete(&conn, -2).unwrap_err().kind, ErrorKind::InvalidArgument);
        delete(&conn, p.id).unwrap();
        assert_eq!(delete(&conn, p.id).unwrap_err().kind, ErrorKind::NotFound);
    }
}
