//! Style library (IPC v14): catalog-wide presets + profiles, grouped by source folder.
//!
//! Ownership: rust-engine-dev (bodies). The architect wrote the catalog SQL (list, remove,
//! LUT-library sync, profile-browser rows) and the Sieve-preset path; [`import_folder`] and
//! the key-wise application of imported presets ([`resolve_preset`]) are rust-engine-dev's.
//!
//! Model (see `docs/architecture.md`, "Style library"):
//! - `style_groups`: one row per imported source folder (`source_path` unique; re-import
//!   replaces its items) + built-ins "User Presets" (`USER_PRESETS_GROUP_ID`) and "LUTs"
//!   (`LUT_LIBRARY_GROUP_ID`).
//! - Presets live in `presets` (same ids as `list_presets` / `apply_preset`). Imported presets
//!   keep the file's `crs:` settings in `settings_json` and apply **exactly those keys**
//!   (Lightroom semantics); Sieve presets apply `fields` groups.
//! - Profiles live in `style_profiles`: looks (`.xmp`, `crs:PresetType="Look"`) and DCPs are
//!   read in place (licensing rule, `profiles` module docs; `ProfileLibrary` must resolve them:
//!   see [`imported_profile_paths`]); `.cube` files are copied into the LUT library and
//!   referenced by `lut_id` (a LUT is a profile in the UI; no separate LUT concept).
//!
//! `import_folder` contract (rust-engine-dev):
//! - Walk `root` recursively (depth <= 8, sorted, symlinks not followed). Every directory
//!   containing at least one importable file becomes one group named after the directory
//!   (the root itself included), `source_path` = that directory. Existing groups with the same
//!   `source_path` are replaced (their items deleted, group id kept).
//! - `.xmp` with `crs:PresetType="Normal"` (or no PresetType but `crs:HasSettings="True"`):
//!   preset. Name = `crs:Name` (x-default), else file stem. Settings = every top-level `crs:`
//!   develop property + curves (`rdf:Seq`) + `crs:Look` struct; bookkeeping properties
//!   (`PresetType`, `Cluster`, `UUID`, `Supports*`, `CameraModelRestriction`, `Copyright`,
//!   `ContactInfo`, `Version`, `HasSettings`, `Name`, `ShortName`, `SortName`, `Group`,
//!   `Description`) are not settings. `fields` = groups the keys touch; `adjustments` =
//!   RAW defaults with the settings applied. Unsupported keys -> `warnings`.
//! - `.lrtemplate` with `type = "Develop"`: same, from `value.settings` (Lua table; keys are
//!   `crs:` names without prefix). Other template types (External Editor, export...) ->
//!   skipped with a reason.
//! - `.xmp` with `crs:PresetType="Look"`: profile `look` (uuid, name, `SupportsAmount`,
//!   monochrome, `CameraProfile`, `CameraModelRestriction`). A look without a UUID is skipped.
//! - `.dcp`: profile `camera_profile` (`ProfileName`, `UniqueCameraModel`).
//! - `.cube`: `LutLibrary::import` (copy, idempotent) + profile `lut`.
//! - Duplicate names within a group get " (2)", " (3)"...
//! - One transaction for all catalog writes; files that fail parse are listed in `skipped`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::now_ms;
use crate::develop::presets;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;
use crate::lut::LutLibrary;
use crate::profiles::CameraKey;

/// Imports every preset / profile under `root` (see the module docs). Contract:
/// rust-engine-dev.
pub fn import_folder(conn: &mut Connection, luts: &LutLibrary, root: &Path) -> AppResult<ImportStyleReport> {
    let _ = (conn, luts, root);
    Err(AppError::internal("Importing presets and profiles is not implemented yet (IPC v14 stub, rust-engine-dev)."))
}

/// What applying preset `preset_id` to an image whose adjustments are `base` gives.
/// Sieve presets: `base` with the preset's `fields` copied. Imported presets: exactly the
/// preset's `crs:` settings applied onto `base` (rust-engine-dev: `xmp::crs`; until then the
/// `fields` approximation below).
pub fn resolve_preset(
    conn: &Connection,
    preset_id: PresetId,
    base: &ParametricAdjustments,
) -> AppResult<ParametricAdjustments> {
    let preset = presets::get(conn, preset_id)?;
    let mut out = base.clone();
    // TODO(rust-engine-dev): imported presets (settings_json NOT NULL) apply key by key.
    out.copy_fields(&preset.adjustments, &preset.fields);
    Ok(out)
}

/// Applies preset `preset_id` to `ids` ("Preset: <name>" history entry per changed image).
/// Atomic. Used by the `apply_preset` command.
pub fn apply_preset(conn: &mut Connection, ids: &[ImageId], preset_id: PresetId) -> AppResult<Vec<ImageId>> {
    let preset = presets::get(conn, preset_id)?;
    let label = format!("{}{}", crate::develop::history::LABEL_PRESET_PREFIX, preset.name);
    let mut items = Vec::with_capacity(ids.len());
    for &id in ids {
        let base = crate::db::repo::get_adjustments(conn, id)?;
        items.push((id, resolve_preset(conn, preset_id, &base)?));
    }
    crate::develop::history::commit_batch(conn, &items, &label)
}

/// Registers LUT-library files (`<app_data>/luts`, pre-v14 `import_lut`) that no style profile
/// references as profiles of the "LUTs" group. Idempotent; cheap (one directory listing).
pub fn sync_lut_library(conn: &Connection, luts: &LutLibrary) -> AppResult<()> {
    let known: Vec<String> = conn
        .prepare("SELECT lut_id FROM style_profiles WHERE lut_id IS NOT NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let now = now_ms();
    for info in luts.list()? {
        if known.contains(&info.id) {
            continue;
        }
        conn.execute(
            "INSERT INTO style_profiles (group_id, kind, name, source_format, source_path, lut_id, supports_amount, created_at)
             VALUES (?1, 'lut', ?2, 'sieve', ?3, ?4, 1, ?5)",
            params![LUT_LIBRARY_GROUP_ID, info.name, info.path, info.id, now],
        )?;
    }
    Ok(())
}

fn string_list(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn profile_rows(conn: &Connection) -> AppResult<Vec<StyleProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, group_id, kind, name, source_format, source_path, supports_amount, monochrome, camera_profile,
                camera_model, look_uuid, lut_id
         FROM style_profiles ORDER BY name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        let path: String = r.get(5)?;
        Ok(StyleProfile {
            id: r.get(0)?,
            group_id: r.get(1)?,
            kind: StyleProfileKind::parse(&r.get::<_, String>(2)?).unwrap_or(StyleProfileKind::Look),
            name: r.get(3)?,
            source_format: StyleSourceFormat::parse(&r.get::<_, String>(4)?).unwrap_or(StyleSourceFormat::Sieve),
            available: Path::new(&path).is_file(),
            source_path: path,
            supports_amount: r.get(6)?,
            monochrome: r.get(7)?,
            camera_profile: r.get(8)?,
            camera_model: r.get(9)?,
            look_uuid: r.get(10)?,
            lut_id: r.get(11)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `list_styles()`: every group with its presets and profiles (see [`StyleLibrary`]).
pub fn list(conn: &Connection) -> AppResult<StyleLibrary> {
    let mut groups: Vec<StyleGroup> = conn
        .prepare("SELECT id, name, kind, source_path, imported_at FROM style_groups")?
        .query_map([], |r| {
            Ok(StyleGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: StyleGroupKind::parse(&r.get::<_, String>(2)?).unwrap_or(StyleGroupKind::Imported),
                source_path: r.get(3)?,
                imported_at_ms: r.get(4)?,
                presets: Vec::new(),
                profiles: Vec::new(),
            })
        })?
        .collect::<Result<_, _>>()?;
    let order = |k: StyleGroupKind| match k {
        StyleGroupKind::User => 0,
        StyleGroupKind::Imported => 1,
        StyleGroupKind::Luts => 2,
    };
    groups.sort_by(|a, b| {
        order(a.kind).cmp(&order(b.kind)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id))
    });
    let index: HashMap<StyleGroupId, usize> = groups.iter().enumerate().map(|(i, g)| (g.id, i)).collect();

    let mut stmt = conn.prepare(
        "SELECT id, group_id, name, source_format, source_path, fields_json, setting_keys_json, supports_amount,
                warnings_json
         FROM presets ORDER BY name COLLATE NOCASE, id",
    )?;
    let presets = stmt.query_map([], |r| {
        let fields_json: String = r.get(5)?;
        let names: Vec<String> = serde_json::from_str(&fields_json).unwrap_or_default();
        Ok(StylePreset {
            id: r.get(0)?,
            group_id: r.get(1)?,
            name: r.get(2)?,
            source_format: StyleSourceFormat::parse(&r.get::<_, String>(3)?).unwrap_or(StyleSourceFormat::Sieve),
            source_path: r.get(4)?,
            fields: names.iter().filter_map(|n| AdjustmentField::parse(n)).collect(),
            setting_keys: string_list(&r.get::<_, String>(6)?),
            supports_amount: r.get(7)?,
            warnings: string_list(&r.get::<_, String>(8)?),
        })
    })?;
    for p in presets {
        let p = p?;
        if let Some(&i) = index.get(&p.group_id) {
            groups[i].presets.push(p);
        }
    }
    for p in profile_rows(conn)? {
        if let Some(&i) = index.get(&p.group_id) {
            groups[i].profiles.push(p);
        }
    }
    Ok(StyleLibrary { groups })
}

/// Removes an imported group with its presets and profiles (the source files and LUT-library
/// copies are left alone; images using them keep their settings and render without a missing
/// profile, reporting it). Built-in groups -> `invalid_argument`; unknown -> `not_found`.
pub fn remove_group(conn: &mut Connection, id: StyleGroupId) -> AppResult<()> {
    let kind: Option<String> =
        conn.query_row("SELECT kind FROM style_groups WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    match kind.as_deref().and_then(StyleGroupKind::parse) {
        None => Err(AppError::not_found(format!("style group {id}"))),
        Some(StyleGroupKind::Imported) => {
            conn.execute("DELETE FROM style_groups WHERE id = ?1", [id])?;
            Ok(())
        }
        Some(_) => Err(AppError::invalid("built-in style groups cannot be removed")),
    }
}

/// Files of imported looks and DCPs (read in place). Contract seam for rust-engine-dev: the
/// `ProfileLibrary` must resolve these (look by UUID, DCP by camera + name) in addition to the
/// Adobe directories, and pick up changes after `import_style_folder` / `remove_style_group`.
pub fn imported_profile_paths(conn: &Connection) -> AppResult<Vec<(StyleProfileKind, PathBuf)>> {
    let mut stmt =
        conn.prepare("SELECT kind, source_path FROM style_profiles WHERE kind IN ('look', 'camera_profile')")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (kind, path) = row?;
        if let Some(kind) = StyleProfileKind::parse(&kind) {
            out.push((kind, PathBuf::from(path)));
        }
    }
    Ok(out)
}

/// Adds the library's imported looks, DCPs (for `camera`) and LUTs to a profile browser
/// listing (`list_profiles`). Imported looks shadow nothing: an Adobe-installed look with the
/// same UUID is listed once (the installed one).
pub fn extend_profile_catalog(conn: &Connection, catalog: &mut ProfileCatalog, camera: &CameraKey) -> AppResult<()> {
    let group_names: HashMap<StyleGroupId, String> = conn
        .prepare("SELECT id, name FROM style_groups")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let cands = camera.candidates();
    let camera_ok = |model: &Option<String>| match model {
        None => true,
        Some(m) => m.is_empty() || cands.contains(&m.to_ascii_lowercase()),
    };
    for p in profile_rows(conn)? {
        let group = group_names.get(&p.group_id).cloned().unwrap_or_default();
        match p.kind {
            StyleProfileKind::Look => {
                let Some(uuid) = p.look_uuid.clone() else { continue };
                if catalog.looks.iter().any(|l| l.uuid == uuid) {
                    continue;
                }
                catalog.looks.push(LookProfileInfo {
                    uuid,
                    name: p.name.clone(),
                    group,
                    supports_amount: p.supports_amount,
                    monochrome: p.monochrome,
                    camera_profile: p.camera_profile.clone(),
                    available: p.available && camera_ok(&p.camera_model),
                    style_id: Some(p.id),
                });
            }
            StyleProfileKind::CameraProfile => {
                if !camera.format.is_raw() || p.camera_model.is_none() || !camera_ok(&p.camera_model) {
                    continue;
                }
                let name = p.camera_profile.clone().unwrap_or_else(|| p.name.clone());
                if catalog.camera_profiles.iter().any(|c| c.name == name) {
                    continue;
                }
                catalog.camera_profiles.push(CameraProfileInfo { name, group, style_id: Some(p.id) });
            }
            StyleProfileKind::Lut => {
                let Some(lut_id) = p.lut_id.clone() else { continue };
                catalog.luts.push(LutProfileInfo {
                    style_id: p.id,
                    lut_id,
                    name: p.name.clone(),
                    group,
                    available: p.available,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    fn insert_group(conn: &Connection, name: &str, path: &str) -> StyleGroupId {
        conn.execute(
            "INSERT INTO style_groups (name, kind, source_path, imported_at) VALUES (?1, 'imported', ?2, 1)",
            params![name, path],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn list_orders_groups_and_attaches_items() {
        let mut conn = open_in_memory();
        let user = presets::save(&conn, None, "Warm", &ParametricAdjustments::default(), &[AdjustmentField::Exposure])
            .unwrap();
        assert_eq!(user.group_id, USER_PRESETS_GROUP_ID);
        assert_eq!(user.source_format, StyleSourceFormat::Sieve);
        let g = insert_group(&conn, "Cinematic", "/presets/Cinematic");
        // Imported presets may reuse a user preset's name (unique per group only).
        conn.execute(
            "INSERT INTO presets (group_id, name, params_json, fields_json, source_format, source_path, settings_json,
                                  setting_keys_json, created_at, updated_at)
             VALUES (?1, 'Warm', '{}', '[\"exposure\"]', 'xmp_preset', '/presets/Cinematic/Warm.xmp',
                     '{\"Exposure2012\":\"+0.50\"}', '[\"Exposure2012\"]', 1, 1)",
            [g],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO style_profiles (group_id, kind, name, source_format, source_path, look_uuid, supports_amount,
                                         created_at)
             VALUES (?1, 'look', 'CN Look', 'xmp_profile', '/nonexistent/look.xmp', ?2, 1, 1)",
            params![g, format!("{:032X}", 7)],
        )
        .unwrap();
        let lib = list(&conn).unwrap();
        let names: Vec<_> = lib.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["User Presets", "Cinematic", "LUTs"]);
        assert_eq!(lib.groups[0].presets.len(), 1);
        assert_eq!(lib.groups[1].presets[0].setting_keys, ["Exposure2012"]);
        assert_eq!(lib.groups[1].presets[0].source_format, StyleSourceFormat::XmpPreset);
        assert!(!lib.groups[1].profiles[0].available);

        // Built-ins cannot be removed; removing a group cascades to its items.
        assert_eq!(remove_group(&mut conn, USER_PRESETS_GROUP_ID).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(remove_group(&mut conn, 999).unwrap_err().kind, ErrorKind::NotFound);
        remove_group(&mut conn, g).unwrap();
        let lib = list(&conn).unwrap();
        assert_eq!(lib.groups.len(), 2);
        assert_eq!(presets::list(&conn).unwrap().len(), 1);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM style_profiles", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn lut_library_sync_is_idempotent() {
        let conn = open_in_memory();
        let dir = tempfile::tempdir().unwrap();
        let luts = LutLibrary::new(dir.path().join("luts"));
        let src = dir.path().join("src.cube");
        std::fs::write(&src, "TITLE \"Warm\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n")
            .unwrap();
        let info = luts.import(&src).unwrap();
        sync_lut_library(&conn, &luts).unwrap();
        sync_lut_library(&conn, &luts).unwrap();
        let lib = list(&conn).unwrap();
        let lut_group = lib.groups.iter().find(|g| g.id == LUT_LIBRARY_GROUP_ID).unwrap();
        assert_eq!(lut_group.profiles.len(), 1);
        let p = &lut_group.profiles[0];
        assert_eq!((p.kind, p.lut_id.as_deref(), p.available), (StyleProfileKind::Lut, Some(info.id.as_str()), true));

        let mut cat = ProfileCatalog {
            image_id: 1,
            camera_model: None,
            camera_profiles: Vec::new(),
            looks: Vec::new(),
            luts: Vec::new(),
            search_dirs: Vec::new(),
        };
        let camera = CameraKey { format: ImageFormat::Arw, make: Some("Sony".into()), model: Some("ILCE-7M4".into()) };
        extend_profile_catalog(&conn, &mut cat, &camera).unwrap();
        assert_eq!(cat.luts.len(), 1);
        assert_eq!(cat.luts[0].group, "LUTs");
    }

    #[test]
    fn profile_selection_semantics() {
        let base = ParametricAdjustments {
            lut: Some(LutRef { id: "old".into(), amount: 100.0 }),
            ..ParametricAdjustments::default()
        };
        let look = StyleProfile {
            id: 1,
            group_id: 3,
            kind: StyleProfileKind::Look,
            name: "CN".into(),
            source_format: StyleSourceFormat::XmpProfile,
            source_path: "/x.xmp".into(),
            available: true,
            supports_amount: true,
            monochrome: false,
            camera_profile: Some("Adobe Standard".into()),
            camera_model: None,
            look_uuid: Some(format!("{:032X}", 9)),
            lut_id: None,
        };
        let a = look.apply_to(&base, 150.0);
        assert_eq!(a.profile.look.as_ref().unwrap().amount, 1.5);
        assert!(a.lut.is_none());
        a.validate().unwrap();
        let lut =
            StyleProfile { kind: StyleProfileKind::Lut, lut_id: Some("warm".into()), look_uuid: None, ..look.clone() };
        let b = lut.apply_to(&base, 250.0);
        assert_eq!(b.lut, Some(LutRef { id: "warm".into(), amount: 200.0 }));
        assert_eq!(b.profile, base.profile);
        b.validate().unwrap();
        let dcp =
            StyleProfile { kind: StyleProfileKind::CameraProfile, camera_profile: Some("Camera ST".into()), ..look };
        let c = dcp.apply_to(&base, 100.0);
        assert_eq!(c.profile.camera_profile.as_deref(), Some("Camera ST"));
        assert!(c.profile.look.is_none() && c.lut.is_none());
    }
}
