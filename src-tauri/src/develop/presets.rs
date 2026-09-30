//! Develop presets (catalog table `presets`, migration 0005).
//!
//! - `params_json` is overlaid on neutral defaults when read (like `adjustments`).
//! - `fields_json` is a non-empty, de-duplicated `AdjustmentField[]`.
//! - Names are trimmed, 1..=100 chars, unique case-insensitively within their style group
//!   (`invalid_argument` on a clash with another preset). Unknown id -> `not_found`.
//! - `save` writes Sieve presets (group `USER_PRESETS_GROUP_ID`); imported presets (IPC v14,
//!   `styles`) carry `settings_json` and are applied key by key (`styles::resolve_preset`).
//! - Listed by name (case-insensitive).

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::db::now_ms;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    AdjustmentField, ParametricAdjustments, Preset, PresetId, StyleGroupId, StyleSourceFormat, USER_PRESETS_GROUP_ID,
};

const SELECT: &str = "SELECT id, name, params_json, fields_json, created_at, updated_at, group_id, source_format, \
                      setting_keys_json FROM presets";

pub(crate) fn overlay(json: &str) -> AppResult<ParametricAdjustments> {
    fn merge(base: &mut serde_json::Value, overlay: serde_json::Value) {
        match (base, overlay) {
            (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
                for (k, v) in o {
                    match b.get_mut(&k) {
                        Some(slot) => merge(slot, v),
                        None => {
                            b.insert(k, v);
                        }
                    }
                }
            }
            (slot, v) => *slot = v,
        }
    }
    let mut value = serde_json::to_value(ParametricAdjustments::default())?;
    merge(&mut value, serde_json::from_str(json)?);
    Ok(serde_json::from_value(value)?)
}

type RawRow = (PresetId, String, String, String, i64, i64, StyleGroupId, String, String);

fn raw(r: &Row) -> rusqlite::Result<RawRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
}

fn to_preset(
    (id, name, params_json, fields_json, created_at_ms, updated_at_ms, group_id, source_format, keys_json): RawRow,
) -> AppResult<Preset> {
    let names: Vec<String> = serde_json::from_str(&fields_json)?;
    // Unknown (future) field names are skipped rather than failing the whole list.
    let fields = names.iter().filter_map(|n| AdjustmentField::parse(n)).collect();
    Ok(Preset {
        id,
        name,
        adjustments: overlay(&params_json)?,
        fields,
        created_at_ms,
        updated_at_ms,
        group_id,
        source_format: StyleSourceFormat::parse(&source_format).unwrap_or(StyleSourceFormat::Sieve),
        setting_keys: serde_json::from_str(&keys_json).unwrap_or_default(),
    })
}

pub fn list(conn: &Connection) -> AppResult<Vec<Preset>> {
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY name COLLATE NOCASE, id"))?;
    let rows = stmt.query_map([], raw)?.collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(to_preset).collect()
}

pub fn get(conn: &Connection, id: PresetId) -> AppResult<Preset> {
    let row = conn
        .query_row(&format!("{SELECT} WHERE id = ?1"), [id], raw)
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("preset {id}")))?;
    to_preset(row)
}

/// Creates (`id = None`) or overwrites (`Some`) a preset; validates `adjustments`.
pub fn save(
    conn: &Connection,
    id: Option<PresetId>,
    name: &str,
    adjustments: &ParametricAdjustments,
    fields: &[AdjustmentField],
) -> AppResult<Preset> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > 100 {
        return Err(AppError::invalid("preset name must be 1..=100 characters"));
    }
    adjustments.validate().map_err(AppError::invalid)?;
    let mut uniq: Vec<AdjustmentField> = Vec::new();
    for f in fields {
        if !uniq.contains(f) {
            uniq.push(*f);
        }
    }
    if uniq.is_empty() {
        return Err(AppError::invalid("fields must not be empty"));
    }
    let clash: Option<PresetId> = conn
        .query_row(
            "SELECT id FROM presets WHERE group_id = ?3 AND name = ?1 COLLATE NOCASE AND id IS NOT ?2",
            params![name, id, USER_PRESETS_GROUP_ID],
            |r| r.get(0),
        )
        .optional()?;
    if clash.is_some() {
        return Err(AppError::invalid(format!("a preset named {name:?} already exists")));
    }
    let params_json = serde_json::to_string(adjustments)?;
    let fields_json = serde_json::to_string(&uniq.iter().map(|f| f.as_str()).collect::<Vec<_>>())?;
    let now = now_ms();
    let id = match id {
        Some(id) => {
            // Only Sieve presets are editable; imported ones are replaced by re-importing.
            let n = conn.execute(
                "UPDATE presets SET name = ?2, params_json = ?3, fields_json = ?4, updated_at = ?5
                 WHERE id = ?1 AND group_id = ?6",
                params![id, name, params_json, fields_json, now, USER_PRESETS_GROUP_ID],
            )?;
            if n == 0 {
                return Err(AppError::not_found(format!("preset {id}")));
            }
            id
        }
        None => {
            conn.execute(
                "INSERT INTO presets (name, params_json, fields_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![name, params_json, fields_json, now],
            )?;
            conn.last_insert_rowid()
        }
    };
    get(conn, id)
}

pub fn delete(conn: &Connection, id: PresetId) -> AppResult<()> {
    if conn.execute("DELETE FROM presets WHERE id = ?1", [id])? == 0 {
        return Err(AppError::not_found(format!("preset {id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    #[test]
    fn crud_and_validation() {
        let conn = open_in_memory();
        assert!(list(&conn).unwrap().is_empty());
        let warm = ParametricAdjustments { exposure: 0.3, vibrance: 20.0, ..Default::default() };
        let fields = [AdjustmentField::Exposure, AdjustmentField::Vibrance, AdjustmentField::Exposure];
        let p = save(&conn, None, "  Warm  ", &warm, &fields).unwrap();
        assert_eq!(p.name, "Warm");
        assert_eq!(p.fields, vec![AdjustmentField::Exposure, AdjustmentField::Vibrance]);
        assert_eq!(p.adjustments, warm);
        assert_eq!(get(&conn, p.id).unwrap(), p);

        let b = save(&conn, None, "b&w", &ParametricAdjustments::default(), AdjustmentField::ALL).unwrap();
        assert_eq!(list(&conn).unwrap().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["b&w", "Warm"]);

        // Name clash (case-insensitive) with another preset; renaming itself is fine.
        assert_eq!(save(&conn, None, "WARM", &warm, &fields).unwrap_err().kind, ErrorKind::InvalidArgument);
        let p2 = save(&conn, Some(p.id), "warm", &warm, &[AdjustmentField::Exposure]).unwrap();
        assert_eq!((p2.id, p2.name.as_str(), p2.fields.len()), (p.id, "warm", 1));
        assert_eq!(p2.created_at_ms, p.created_at_ms);
        assert_eq!(save(&conn, Some(b.id), "Warm", &warm, &fields).unwrap_err().kind, ErrorKind::InvalidArgument);

        assert_eq!(save(&conn, None, " ", &warm, &fields).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, None, &"x".repeat(101), &warm, &fields).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, None, "x", &warm, &[]).unwrap_err().kind, ErrorKind::InvalidArgument);
        let bad = ParametricAdjustments { exposure: 7.0, ..Default::default() };
        assert_eq!(save(&conn, None, "x", &bad, &fields).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(save(&conn, Some(999), "x", &warm, &fields).unwrap_err().kind, ErrorKind::NotFound);

        delete(&conn, b.id).unwrap();
        assert_eq!(delete(&conn, b.id).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(get(&conn, b.id).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(list(&conn).unwrap().len(), 1);

        // Stored JSON missing newer fields loads as neutral for them.
        conn.execute(
            "INSERT INTO presets (name, params_json, fields_json, created_at, updated_at)
             VALUES ('old', '{\"exposure\":1.5}', '[\"exposure\",\"future_field\"]', 0, 0)",
            [],
        )
        .unwrap();
        let old = list(&conn).unwrap().into_iter().find(|p| p.name == "old").unwrap();
        assert_eq!(old.adjustments, ParametricAdjustments { exposure: 1.5, ..Default::default() });
        assert_eq!(old.fields, vec![AdjustmentField::Exposure]);
    }
}
