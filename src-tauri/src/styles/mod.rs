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

#[cfg(test)]
mod import_tests;
pub mod lua;
pub mod preset_file;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::now_ms;
use crate::develop::presets;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;
use crate::lut::LutLibrary;
use crate::profiles::{CameraKey, LookProfile};

use preset_file::{ParsedPreset, PresetSettings, Template};

/// Maximum directory depth below the import root.
const MAX_DEPTH: usize = 8;
/// Files larger than this are skipped (Adobe's biggest look profiles are a few MB; 65^3 .cube
/// files are ~8 MB).
const MAX_FILE_BYTES: u64 = 64 << 20;

/// One importable item found in a directory.
pub(crate) enum Item {
    Preset { path: PathBuf, format: StyleSourceFormat, parsed: ParsedPreset },
    Profile(ProfileRow),
}

pub(crate) struct ProfileRow {
    kind: StyleProfileKind,
    name: String,
    format: StyleSourceFormat,
    path: String,
    look_uuid: Option<String>,
    lut_id: Option<String>,
    camera_profile: Option<String>,
    camera_model: Option<String>,
    supports_amount: bool,
    monochrome: bool,
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn ext_of(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Directories under `root` (itself first), sorted, depth-limited, symlinks not followed.
fn directories(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        out.push(dir.to_path_buf());
        if depth >= MAX_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        subs.sort();
        for d in subs {
            walk(&d, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

/// Group name of `dir` (UX spec 7.1): the path relative to the import root's parent, at most
/// the last two components (`VSCO / Film 01`).
fn group_name(root: &Path, dir: &Path) -> String {
    let base = root.parent().unwrap_or(root);
    let rel = dir.strip_prefix(base).unwrap_or(dir);
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let parts = if parts.len() > 2 { &parts[parts.len() - 2..] } else { &parts[..] };
    let name = parts.join(" / ");
    if name.is_empty() {
        dir.display().to_string()
    } else {
        name
    }
}

/// Reads one file: `Ok(None)` for unsupported extensions (ignored silently), `Err(reason)`
/// for files that could not be imported.
fn read_item(path: &Path, luts: &LutLibrary) -> Result<Option<Item>, String> {
    let Some(ext) = ext_of(path) else { return Ok(None) };
    if !matches!(ext.as_str(), "xmp" | "lrtemplate" | "dcp" | "cube") {
        return Ok(None);
    }
    let size = std::fs::metadata(path).map_err(|e| format!("unreadable: {e}"))?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("file too large ({} MB)", size >> 20));
    }
    match ext.as_str() {
        "xmp" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
            if let Some(parsed) = preset_file::parse_xmp(&text).map_err(|e| format!("invalid XMP: {e}"))? {
                return Ok(Some(Item::Preset { path: path.to_owned(), format: StyleSourceFormat::XmpPreset, parsed }));
            }
            match LookProfile::parse_file_opts(&text, false) {
                Ok(Some(look)) => Ok(Some(Item::Profile(ProfileRow {
                    kind: StyleProfileKind::Look,
                    name: if look.name.trim().is_empty() { stem(path) } else { look.name.trim().to_owned() },
                    format: StyleSourceFormat::XmpProfile,
                    path: path.display().to_string(),
                    look_uuid: Some(look.uuid),
                    lut_id: None,
                    camera_profile: look.camera_profile,
                    camera_model: look.camera_model_restriction,
                    supports_amount: look.supports_amount,
                    monochrome: look.monochrome,
                }))),
                Ok(None) => {
                    let kind = crate::xmp::packet::Packet::parse(&text).ok().and_then(|p| {
                        use crate::xmp::crs::CrsSource;
                        p.top().scalar(crate::xmp::crs::CRS_NS, "PresetType")
                    });
                    Err(match kind {
                        Some(k) => format!("not a develop preset or profile (crs:PresetType \"{}\")", k.trim()),
                        None => "not a develop preset or profile (photo sidecar or other XMP)".into(),
                    })
                }
                Err(e) => Err(format!("invalid look profile: {e}")),
            }
        }
        "lrtemplate" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
            match preset_file::parse_lrtemplate(&text).map_err(|e| format!("invalid .lrtemplate: {e}"))? {
                Template::Develop(parsed) => {
                    Ok(Some(Item::Preset { path: path.to_owned(), format: StyleSourceFormat::Lrtemplate, parsed }))
                }
                Template::Other(kind) => Err(format!("not a develop preset ({kind} preset)")),
            }
        }
        "dcp" => {
            let (model, name) = crate::profiles::read_dcp_names(path).ok_or("unreadable DCP")?;
            if name.trim().is_empty() || model.trim().is_empty() {
                return Err("DCP without ProfileName / UniqueCameraModel".into());
            }
            Ok(Some(Item::Profile(ProfileRow {
                kind: StyleProfileKind::CameraProfile,
                name: name.clone(),
                format: StyleSourceFormat::Dcp,
                path: path.display().to_string(),
                look_uuid: None,
                lut_id: None,
                camera_profile: Some(name),
                camera_model: Some(model),
                supports_amount: false,
                monochrome: false,
            })))
        }
        _ => {
            let info = luts.import(path).map_err(|e| {
                let prefix = format!("{}: ", path.display());
                format!("invalid .cube: {}", e.message.strip_prefix(&prefix).unwrap_or(&e.message))
            })?;
            Ok(Some(Item::Profile(ProfileRow {
                kind: StyleProfileKind::Lut,
                name: if info.name.trim().is_empty() { stem(path) } else { info.name.clone() },
                format: StyleSourceFormat::Cube,
                path: info.path.clone(),
                look_uuid: None,
                lut_id: Some(info.id),
                camera_profile: None,
                camera_model: None,
                supports_amount: true,
                monochrome: false,
            })))
        }
    }
}

/// `name`, or `name (2)`, `name (3)`... not yet in `taken` (case-insensitive); records it.
fn unique_name(name: &str, taken: &mut Vec<String>) -> String {
    let base: String = name.trim().chars().take(100).collect();
    let base = if base.is_empty() { "Untitled".to_owned() } else { base };
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
        let suffix = format!(" ({n})");
        let keep = 100 - suffix.chars().count();
        candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
        n += 1;
    }
    taken.push(candidate.clone());
    candidate
}

/// Imports every preset / profile under `root` (see the module docs). Contract:
/// rust-engine-dev. [`scan_folder`] + [`store_scan`] (the command runs the scan off the
/// catalog thread).
pub fn import_folder(conn: &mut Connection, luts: &LutLibrary, root: &Path) -> AppResult<ImportStyleReport> {
    let scan = scan_folder(luts, root)?;
    store_scan(conn, scan)
}

/// Result of [`scan_folder`]: parsed items per directory, not yet in the catalog.
pub struct FolderScan {
    root: PathBuf,
    found: Vec<(PathBuf, Vec<Item>)>,
    skipped: Vec<StyleImportSkip>,
}

/// Walks and parses `root` (no catalog access; `.cube` files are copied into the LUT
/// library). Errors: `not_found`, `invalid_argument` (nothing importable).
pub fn scan_folder(luts: &LutLibrary, root: &Path) -> AppResult<FolderScan> {
    if !root.is_dir() {
        return Err(AppError::not_found(format!("{}: no such folder", root.display())));
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut report = ImportStyleReport { root: root.display().to_string(), ..Default::default() };
    let mut found: Vec<(PathBuf, Vec<Item>)> = Vec::new();

    for dir in directories(&root) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        files.sort();
        let mut items = Vec::new();
        for f in files {
            match read_item(&f, luts) {
                Ok(Some(Item::Preset { parsed, .. })) if parsed.settings.is_empty() => {
                    let why = if parsed.warnings.is_empty() {
                        "preset has no develop settings".to_owned()
                    } else {
                        format!("no settings Sieve can apply ({})", parsed.warnings.join("; "))
                    };
                    report.skipped.push(StyleImportSkip { path: f.display().to_string(), reason: why });
                }
                Ok(Some(item)) => items.push(item),
                Ok(None) => {}
                Err(reason) => report.skipped.push(StyleImportSkip { path: f.display().to_string(), reason }),
            }
        }
        if !items.is_empty() {
            found.push((dir, items));
        }
    }
    if found.is_empty() && report.skipped.is_empty() {
        return Err(AppError::invalid(format!(
            "{}: no presets or profiles found (.xmp, .lrtemplate, .dcp, .cube)",
            root.display()
        )));
    }

    Ok(FolderScan { root, found, skipped: report.skipped })
}

/// Writes a [`FolderScan`] in one transaction (groups replaced by source folder) and
/// registers the imported looks / DCPs with the render engine.
pub fn store_scan(conn: &mut Connection, scan: FolderScan) -> AppResult<ImportStyleReport> {
    let FolderScan { root, found, skipped } = scan;
    let mut report = ImportStyleReport { root: root.display().to_string(), skipped, ..Default::default() };
    let now = now_ms();
    let tx = conn.transaction()?;
    for (dir, items) in found {
        let source = dir.display().to_string();
        let name = group_name(&root, &dir);
        let existing: Option<StyleGroupId> =
            tx.query_row("SELECT id FROM style_groups WHERE source_path = ?1", [&source], |r| r.get(0)).optional()?;
        let gid = match existing {
            Some(id) => {
                tx.execute("DELETE FROM presets WHERE group_id = ?1", [id])?;
                tx.execute("DELETE FROM style_profiles WHERE group_id = ?1", [id])?;
                tx.execute(
                    "UPDATE style_groups SET name = ?2, imported_at = ?3 WHERE id = ?1",
                    params![id, name, now],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO style_groups (name, kind, source_path, imported_at) VALUES (?1, 'imported', ?2, ?3)",
                    params![name, source, now],
                )?;
                tx.last_insert_rowid()
            }
        };
        report.group_ids.push(gid);
        let mut preset_names = Vec::new();
        let mut profile_names = Vec::new();
        for item in items {
            match item {
                Item::Preset { path, format, parsed } => {
                    let name = unique_name(&parsed.name.clone().unwrap_or_else(|| stem(&path)), &mut preset_names);
                    let adjustments =
                        parsed.settings.apply(&ParametricAdjustments::default()).map_err(AppError::internal)?;
                    let fields: Vec<&str> = parsed.settings.fields().iter().map(|f| f.as_str()).collect();
                    tx.execute(
                        "INSERT INTO presets (group_id, name, params_json, fields_json, source_format, source_path,
                                              settings_json, setting_keys_json, supports_amount, warnings_json,
                                              created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
                        params![
                            gid,
                            name,
                            serde_json::to_string(&adjustments)?,
                            serde_json::to_string(&fields)?,
                            format.as_str(),
                            path.display().to_string(),
                            serde_json::to_string(&parsed.settings)?,
                            serde_json::to_string(&parsed.settings.keys())?,
                            parsed.supports_amount,
                            serde_json::to_string(&parsed.warnings)?,
                            now
                        ],
                    )?;
                    report.presets += 1;
                }
                Item::Profile(p) => {
                    let name = unique_name(&p.name, &mut profile_names);
                    tx.execute(
                        "INSERT INTO style_profiles (group_id, kind, name, source_format, source_path, look_uuid, lut_id,
                                                     camera_profile, camera_model, supports_amount, monochrome, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        params![
                            gid,
                            p.kind.as_str(),
                            name,
                            p.format.as_str(),
                            p.path,
                            p.look_uuid,
                            p.lut_id,
                            p.camera_profile,
                            p.camera_model,
                            p.supports_amount,
                            p.monochrome,
                            now
                        ],
                    )?;
                    report.profiles += 1;
                }
            }
        }
    }
    tx.commit()?;
    register_imported_profiles(conn)?;
    Ok(report)
}

/// Makes the library's imported looks / DCPs resolvable by the render engine and the XMP
/// writer (`profiles::set_imported`). Called after imports / removals and at startup.
pub fn register_imported_profiles(conn: &Connection) -> AppResult<()> {
    crate::profiles::set_imported(&imported_profile_paths(conn)?);
    Ok(())
}

/// Test-only serialisation of the process-wide imported-profile registry
/// (`profiles::set_imported`). Every test that replaces the registry (style imports,
/// group removals, `DevelopCache` startup registration) holds this guard for its whole
/// replace-then-assert sequence so parallel tests cannot clobber each other's set.
/// Not reentrant: take it in the test (or the spawned thread), never inside
/// [`register_imported_profiles`].
#[cfg(test)]
pub(crate) fn test_registry_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The stored `crs:` settings of an imported preset (`None` for Sieve presets).
pub fn preset_settings(conn: &Connection, preset_id: PresetId) -> AppResult<Option<PresetSettings>> {
    let json: Option<Option<String>> =
        conn.query_row("SELECT settings_json FROM presets WHERE id = ?1", [preset_id], |r| r.get(0)).optional()?;
    match json {
        None => Err(AppError::not_found(format!("preset {preset_id}"))),
        Some(None) => Ok(None),
        Some(Some(j)) => Ok(Some(serde_json::from_str(&j)?)),
    }
}

/// What applying preset `preset_id` to an image whose adjustments are `base` gives.
/// Sieve presets: `base` with the preset's `fields` copied. Imported presets: exactly the
/// preset's `crs:` settings applied onto `base` (Lightroom semantics; masks are appended).
pub fn resolve_preset(
    conn: &Connection,
    preset_id: PresetId,
    base: &ParametricAdjustments,
) -> AppResult<ParametricAdjustments> {
    if let Some(settings) = preset_settings(conn, preset_id)? {
        let out = settings.apply(base).map_err(AppError::internal)?;
        out.validate().map_err(AppError::internal)?;
        return Ok(out);
    }
    let preset = presets::get(conn, preset_id)?;
    let mut out = base.clone();
    out.copy_fields(&preset.adjustments, &preset.fields);
    Ok(out)
}

/// The settings `render_preview_variant` renders for image `id` (IPC v19): `base` with the
/// preset applied (as [`resolve_preset`]) or with `fields` back at the image's format
/// defaults. Nothing is saved. Unknown preset / image -> `not_found`.
pub fn resolve_preview_variant(
    conn: &Connection,
    id: ImageId,
    base: &ParametricAdjustments,
    variant: &PreviewVariant,
) -> AppResult<ParametricAdjustments> {
    match variant {
        PreviewVariant::Preset { preset_id } => {
            crate::db::repo::image_format(conn, id)?;
            resolve_preset(conn, *preset_id, base)
        }
        PreviewVariant::WithoutFields { fields } => {
            let defaults = ParametricAdjustments::defaults_for(crate::db::repo::image_format(conn, id)?);
            let mut out = base.clone();
            out.copy_fields(&defaults, fields);
            Ok(out)
        }
    }
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
    let tx = conn.savepoint()?;
    let changed = crate::develop::history::commit_batch_in(&tx, &items, &label)?;
    crate::develop::history::record_applied_preset(&tx, ids, preset_id)?;
    tx.commit()?;
    Ok(changed)
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
            register_imported_profiles(conn)
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
        let _registry = test_registry_guard();
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
