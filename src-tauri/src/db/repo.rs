//! Catalog queries. Pure functions over a `Connection` so they are unit-testable
//! without a running Tauri app.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use walkdir::WalkDir;

use super::now_ms;
use super::projects::{self, FolderScope, ImportTarget};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;
use crate::raw;

// ---------------------------------------------------------------------------
// Catalog settings
// ---------------------------------------------------------------------------

fn get_meta(conn: &Connection, key: &str) -> AppResult<String> {
    Ok(conn.query_row("SELECT value FROM catalog_meta WHERE key = ?1", [key], |r| r.get(0))?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO catalog_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn set_shoot_type(conn: &Connection, shoot_type: ShootType) -> AppResult<()> {
    set_meta(conn, "shoot_type", shoot_type.as_str())
}

pub fn set_burst_window(conn: &Connection, ms: u32) -> AppResult<()> {
    if !(100..=60_000).contains(&ms) {
        return Err(AppError::invalid(format!("burst window {ms}ms is outside 100..=60000")));
    }
    set_meta(conn, "burst_window_ms", &ms.to_string())
}

pub fn catalog_state(conn: &Connection, catalog_path: &str, cache_dir: &str) -> AppResult<CatalogState> {
    let shoot_type = ShootType::parse(&get_meta(conn, "shoot_type")?).unwrap_or(ShootType::General);
    let burst_window_ms = get_meta(conn, "burst_window_ms")?.parse().unwrap_or(1500);
    let image_count: u32 = conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))?;

    let folders = conn
        .prepare(
            "SELECT f.id, f.path, COUNT(i.id), f.project_id FROM folders f
             LEFT JOIN images i ON i.folder_id = f.id
             GROUP BY f.id ORDER BY f.path",
        )?
        .query_map([], |r| {
            Ok(FolderEntry {
                id: r.get(0)?,
                path: r.get(1)?,
                image_count: r.get(2)?,
                project_id: r.get::<_, Option<ProjectId>>(3)?.unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let tag_counts = conn
        .prepare("SELECT tag, COUNT(*) FROM image_tags WHERE suppressed = 0 GROUP BY tag ORDER BY tag")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
        .filter_map(|r| match r {
            Ok((tag, count)) => CullTag::parse(&tag).map(|tag| Ok(TagCount { tag, count })),
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CatalogState {
        catalog_path: catalog_path.to_owned(),
        image_count,
        shoot_type,
        burst_window_ms,
        folders,
        tag_counts,
        cache_dir: cache_dir.to_owned(),
        auto_analyze: auto_analyze(conn)?,
        xmp_auto_sync: xmp_auto_sync(conn)?,
        health: super::health_state(Path::new(catalog_path)),
        keeper_rule: keeper_rule(conn)?,
    })
}

pub fn auto_analyze(conn: &Connection) -> AppResult<bool> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM catalog_meta WHERE key = 'auto_analyze'", [], |r| r.get(0)).optional()?;
    Ok(v.as_deref() != Some("0"))
}

pub fn set_auto_analyze(conn: &Connection, enabled: bool) -> AppResult<()> {
    set_meta(conn, "auto_analyze", if enabled { "1" } else { "0" })
}

/// `catalog_meta.xmp_auto_sync`; default on since schema v12 (IPC v14).
pub fn xmp_auto_sync(conn: &Connection) -> AppResult<bool> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM catalog_meta WHERE key = 'xmp_auto_sync'", [], |r| r.get(0)).optional()?;
    Ok(v.as_deref() != Some("0"))
}

/// Also records that the user chose explicitly (`xmp_auto_sync_user_set`, see migration 0012).
pub fn set_xmp_auto_sync(conn: &Connection, enabled: bool) -> AppResult<()> {
    set_meta(conn, "xmp_auto_sync", if enabled { "1" } else { "0" })?;
    set_meta(conn, "xmp_auto_sync_user_set", "1")
}

/// `catalog_meta.keeper_rule` (IPC v14); default [`KeeperRule::default`].
pub fn keeper_rule(conn: &Connection) -> AppResult<KeeperRule> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM catalog_meta WHERE key = 'keeper_rule'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|j| serde_json::from_str::<KeeperRule>(&j).ok()).filter(|r| r.validate().is_ok()).unwrap_or_default())
}

pub fn set_keeper_rule(conn: &Connection, rule: &KeeperRule) -> AppResult<()> {
    rule.validate().map_err(AppError::invalid)?;
    set_meta(conn, "keeper_rule", &serde_json::to_string(rule)?)
}

pub fn xmp_status(conn: &Connection, running: bool) -> AppResult<XmpStatus> {
    let (dirty, failed) = conn.query_row(
        "SELECT COALESCE(SUM(xmp_dirty = 1), 0), COALESCE(SUM(xmp_error IS NOT NULL), 0) FROM images",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(XmpStatus { dirty, failed, running, auto_sync: xmp_auto_sync(conn)? })
}

pub fn shoot_type(conn: &Connection) -> AppResult<ShootType> {
    Ok(ShootType::parse(&get_meta(conn, "shoot_type")?).unwrap_or(ShootType::General))
}

fn thresholds_key(shoot_type: ShootType) -> String {
    format!("cull_thresholds.{}", shoot_type.as_str())
}

/// Effective thresholds for `shoot_type`: stored overrides overlaid on
/// `ml::thresholds::default_thresholds`, so fields added later load as defaults.
pub fn cull_thresholds(conn: &Connection, shoot_type: ShootType) -> AppResult<CullThresholds> {
    let defaults = crate::ml::thresholds::default_thresholds(shoot_type);
    let json: Option<String> = conn
        .query_row("SELECT value FROM catalog_meta WHERE key = ?1", [thresholds_key(shoot_type)], |r| r.get(0))
        .optional()?;
    let Some(json) = json else { return Ok(defaults) };
    let mut merged = serde_json::to_value(&defaults)?;
    merge_json(&mut merged, serde_json::from_str(&json)?);
    let t: CullThresholds = serde_json::from_value(merged)?;
    // Stored values that no longer validate (e.g. after a range change) fall back.
    Ok(if t.validate().is_ok() { t } else { defaults })
}

/// Stores `thresholds` for `shoot_type`, or resets to defaults when `None`.
pub fn set_cull_thresholds(
    conn: &Connection,
    shoot_type: ShootType,
    thresholds: Option<&CullThresholds>,
) -> AppResult<()> {
    match thresholds {
        Some(t) => {
            t.validate().map_err(AppError::invalid)?;
            set_meta(conn, &thresholds_key(shoot_type), &serde_json::to_string(t)?)
        }
        None => {
            conn.execute("DELETE FROM catalog_meta WHERE key = ?1", [thresholds_key(shoot_type)])?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Registers every supported RAW under `folder` with a `pending` thumbnail.
/// No decoding happens here; metadata is filled in by the Phase 2 extractor.
/// Project: [`ImportTarget::Auto`] (see [`import_folder_to`]).
pub fn import_folder(conn: &mut Connection, folder: &Path, opts: &ImportOptions) -> AppResult<ImportSummary> {
    Ok(import_folder_to(conn, folder, opts, &ImportTarget::Auto)?.0)
}

/// [`import_folder`] into the project chosen by `target` (IPC v14). Returns the summary and
/// whether the folder (or a folder containing it, whose row and project are reused) was
/// already in the catalog.
pub fn import_folder_to(
    conn: &mut Connection,
    folder: &Path,
    opts: &ImportOptions,
    target: &ImportTarget,
) -> AppResult<(ImportSummary, bool)> {
    let folder = folder.canonicalize().map_err(|e| AppError::invalid(format!("{}: {e}", folder.display())))?;
    if !folder.is_dir() {
        return Err(AppError::invalid(format!("{} is not a directory", folder.display())));
    }
    let folder_str = path_str(&folder)?;
    let now = now_ms();

    let tx = conn.savepoint()?;
    let row = projects::import_folder_row(&tx, &folder_str, target)?;
    let folder_id = row.folder_id;

    let mut summary = ImportSummary { folder_id, project_id: row.project_id, ..Default::default() };
    {
        let mut insert_image = tx.prepare(
            "INSERT INTO images (folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(path) DO NOTHING",
        )?;
        let mut insert_thumb = tx.prepare("INSERT INTO thumbnails (image_id, status) VALUES (?1, 'pending')")?;
        let mut exists = tx.prepare("SELECT 1 FROM images WHERE path = ?1")?;
        let mut set_companion = tx.prepare(
            "UPDATE images SET companion_path = ?2
             WHERE path = ?1 AND companion_path IS NOT ?2",
        )?;

        // Pass 1: candidate files (sorted, so pairing and ids are deterministic).
        let walker = WalkDir::new(&folder).max_depth(if opts.recursive { usize::MAX } else { 1 }).sort_by_file_name();
        let mut files: Vec<(walkdir::DirEntry, ImageFormat)> = Vec::new();
        for entry in walker.into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(format) = raw::format_from_extension(entry.path()) else { continue };
            if !format.is_raw() && !opts.include_non_raw {
                continue;
            }
            files.push((entry, format));
        }
        // RAWs by (directory, lower-case stem) -> path, for companion pairing.
        let pair_key = |p: &Path| -> Option<(std::path::PathBuf, String)> {
            Some((p.parent()?.to_path_buf(), p.file_stem()?.to_str()?.to_lowercase()))
        };
        let pairing = opts.include_non_raw && opts.pair_jpeg_with_raw;
        let mut raw_by_stem: HashMap<(std::path::PathBuf, String), std::path::PathBuf> = HashMap::new();
        if pairing {
            for (entry, format) in &files {
                if format.is_raw() {
                    if let Some(k) = pair_key(entry.path()) {
                        raw_by_stem.entry(k).or_insert_with(|| entry.path().to_path_buf());
                    }
                }
            }
        }
        // JPEG before HEIC when a RAW has several siblings (first companion wins).
        files.sort_by_key(|(e, f)| (!f.is_raw(), *f != ImageFormat::Jpeg, e.path().to_path_buf()));

        // Pass 2: register.
        let mut paired: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
        for (entry, format) in files {
            let path = entry.path();
            match raw::identify(path) {
                Ok(Some(f)) if f == format => {}
                Ok(_) => continue,
                Err(_) => {
                    summary.invalid += 1;
                    continue;
                }
            }
            if pairing && format.pairs_with_raw() {
                let raw_path = pair_key(path).and_then(|k| raw_by_stem.get(&k).cloned());
                if let Some(raw_path) = raw_path {
                    let path_s = path_str(path)?;
                    // A sibling already registered as its own image (earlier import without
                    // pairing) stays an image; never pair twice for one RAW.
                    if !exists.exists([&path_s])? && !paired.contains(&raw_path) {
                        let changed = set_companion.execute(params![path_str(&raw_path)?, path_s])?;
                        paired.insert(raw_path);
                        if changed > 0 {
                            summary.companions += 1;
                        } else {
                            // Already recorded by an earlier import.
                            summary.skipped += 1;
                        }
                    } else {
                        summary.skipped += 1;
                    }
                    continue;
                }
            }
            let meta = entry.metadata().map_err(|e| AppError::internal(e.to_string()))?;
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let inserted = insert_image.execute(params![
                folder_id,
                path_str(path)?,
                entry.file_name().to_string_lossy(),
                format.as_str(),
                raw::default_make(format).as_str(),
                raw::default_sensor_layout(format).as_str(),
                meta.len() as i64,
                mtime_ms,
                now,
            ])?;
            if inserted == 0 {
                summary.skipped += 1;
            } else {
                insert_thumb.execute([tx.last_insert_rowid()])?;
                summary.added += 1;
            }
        }
    }
    // Re-import refreshes the missing flags of the folder's images (IPC v13): a file found
    // again is cleared, a catalogued file that is gone is flagged.
    let known: Vec<(ImageId, String)> = tx
        .prepare("SELECT id, path FROM images WHERE folder_id = ?1")?
        .query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, path) in known {
        match std::fs::metadata(&path) {
            Ok(_) => set_original_missing(&tx, id, false)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => set_original_missing(&tx, id, true)?,
            Err(_) => false,
        };
    }
    tx.commit()?;
    Ok((summary, row.existing))
}

fn path_str(p: &Path) -> AppResult<String> {
    p.to_str().map(str::to_owned).ok_or_else(|| AppError::invalid(format!("non-UTF-8 path: {}", p.display())))
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

const ENTRY_SELECT: &str = "
    SELECT i.id, i.folder_id, i.path, i.file_name, i.format,
           i.camera_make, i.camera_model, i.sensor_layout,
           i.captured_at_ms, i.iso, i.shutter_s, i.aperture, i.focal_length_mm, i.lens,
           i.width, i.height, i.orientation, i.file_size, i.file_mtime_ms,
           t.status, t.path, t.width, t.height, t.error,
           i.rating, i.pick, i.color_label, i.burst_group_id,
           q.overall, q.face_sharpness, q.global_sharpness, q.eyes_open, q.composition,
           q.face_count, q.clipped_highlights_pct, q.clipped_shadows_pct, q.mean_luma,
           q.model_version,
           EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = i.id AND a.neutral = 0),
           t.preview_path,
           q.suggested_rating, q.suggested_pick,
           EXISTS (SELECT 1 FROM burst_groups b WHERE b.id = i.burst_group_id AND b.keeper_image_id = i.id),
           i.xmp_dirty, i.xmp_synced_at, i.xmp_error,
           i.scene_id, i.scene_anchor,
           i.companion_path, i.develop_warnings,
           i.missing_since_ms,
           i.pick_origin, i.xmp_mtime_ms IS NOT NULL, q.reasons_json,
           i.exif_captured_at_ms, i.capture_time_source, i.camera_serial
    FROM images i
    LEFT JOIN thumbnails t ON t.image_id = i.id
    LEFT JOIN quality_scores q ON q.image_id = i.id";

fn bad_enum(col: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        col,
        rusqlite::types::Type::Text,
        format!("unexpected enum value {value:?}").into(),
    )
}

fn enum_col<T>(row: &Row, idx: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    parse(&s).ok_or_else(|| bad_enum(idx, &s))
}

fn opt_enum_col<T>(row: &Row, idx: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<Option<T>> {
    row.get::<_, Option<String>>(idx)?.map(|s| parse(&s).ok_or_else(|| bad_enum(idx, &s))).transpose()
}

/// `quality_scores.reasons_json` (v16); missing or unreadable JSON reads as no reasons (never
/// blocks listing).
pub fn parse_reasons(json: Option<&str>) -> Vec<SuggestionReason> {
    json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
}

/// Maps an `ENTRY_SELECT` row; `tags` are filled in afterwards.
fn entry_from_row(r: &Row) -> rusqlite::Result<RawImageEntry> {
    let thumbnail = match r.get::<_, Option<String>>(19)?.as_deref() {
        Some("ready") => {
            ThumbnailState::Ready { path: r.get(20)?, preview_path: r.get(39)?, width: r.get(21)?, height: r.get(22)? }
        }
        Some("failed") => ThumbnailState::Failed { reason: r.get::<_, Option<String>>(23)?.unwrap_or_default() },
        _ => ThumbnailState::Pending,
    };
    let quality = match r.get::<_, Option<f32>>(28)? {
        Some(overall) => Some(QualityScore {
            overall,
            face_sharpness: r.get(29)?,
            global_sharpness: r.get(30)?,
            eyes_open: r.get(31)?,
            composition: r.get(32)?,
            face_count: r.get(33)?,
            exposure: ExposureStats {
                clipped_highlights_pct: r.get(34)?,
                clipped_shadows_pct: r.get(35)?,
                mean_luma: r.get(36)?,
            },
            model_version: r.get(37)?,
            suggested_rating: r.get(40)?,
            suggested_pick: enum_col(r, 41, PickFlag::parse)?,
            reasons: parse_reasons(r.get::<_, Option<String>>(53)?.as_deref()),
        }),
        None => None,
    };
    let pick = enum_col(r, 25, PickFlag::parse)?;
    let pick_origin = match pick {
        PickFlag::Unflagged => None,
        _ => Some(PickOrigin::parse(&r.get::<_, String>(51)?).unwrap_or(PickOrigin::User)),
    };
    Ok(RawImageEntry {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        path: r.get(2)?,
        file_name: r.get(3)?,
        format: enum_col(r, 4, RawFormat::parse)?,
        camera: CameraInfo {
            make: enum_col(r, 5, CameraMake::parse)?,
            model: r.get(6)?,
            sensor_layout: enum_col(r, 7, SensorLayout::parse)?,
            serial: r.get(56)?,
        },
        capture: CaptureMeta {
            captured_at_ms: r.get(8)?,
            original_captured_at_ms: r.get(54)?,
            capture_time_source: CaptureTimeSource::parse(&r.get::<_, String>(55)?).unwrap_or_default(),
            iso: r.get(9)?,
            shutter_seconds: r.get(10)?,
            aperture: r.get(11)?,
            focal_length_mm: r.get(12)?,
            lens: r.get(13)?,
        },
        width: r.get(14)?,
        height: r.get(15)?,
        orientation: r.get(16)?,
        file_size: r.get(17)?,
        file_mtime_ms: r.get(18)?,
        thumbnail,
        rating: r.get(24)?,
        pick,
        pick_origin,
        color_label: opt_enum_col(r, 26, ColorLabel::parse)?,
        burst_group_id: r.get(27)?,
        is_burst_keeper: r.get(42)?,
        tags: Vec::new(),
        quality,
        has_edits: r.get(38)?,
        xmp: XmpSyncState { dirty: r.get(43)?, synced_at_ms: r.get(44)?, error: r.get(45)?, has_sidecar: r.get(52)? },
        scene_id: r.get(46)?,
        is_scene_anchor: r.get(47)?,
        companion_path: r.get(48)?,
        // Unreadable JSON reads as no warnings (never blocks listing).
        develop_warnings: r
            .get::<_, Option<String>>(49)?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
        missing_since_ms: r.get(50)?,
        edited_preview: None,
    })
}

/// Loads tags for `entries` in one query and attaches them.
fn attach_tags(conn: &Connection, entries: &mut [RawImageEntry]) -> AppResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; entries.len()].join(",");
    let sql = format!(
        "SELECT image_id, tag, source, confidence, suppressed FROM image_tags
         WHERE image_id IN ({placeholders}) ORDER BY image_id, tag"
    );
    let mut by_id: HashMap<ImageId, Vec<CullTagEntry>> = HashMap::new();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(entries.iter().map(|e| e.id)), |r| {
        Ok((
            r.get::<_, ImageId>(0)?,
            CullTagEntry {
                tag: enum_col(r, 1, CullTag::parse)?,
                source: enum_col(r, 2, TagSource::parse)?,
                confidence: r.get(3)?,
                suppressed: r.get(4)?,
            },
        ))
    })?;
    for row in rows {
        let (id, tag) = row?;
        by_id.entry(id).or_default().push(tag);
    }
    for e in entries {
        e.tags = by_id.remove(&e.id).unwrap_or_default();
    }
    Ok(())
}

pub fn get_image(conn: &Connection, id: ImageId) -> AppResult<RawImageEntry> {
    let entry = conn
        .query_row(&format!("{ENTRY_SELECT} WHERE i.id = ?1"), [id], entry_from_row)
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("image {id}")))?;
    let mut entries = [entry];
    attach_tags(conn, &mut entries)?;
    crate::develop::edited::attach(conn, &mut entries)?;
    let [entry] = entries;
    Ok(entry)
}

/// Entries for `ids` in the given order. Atomic: an unknown id fails with `not_found`.
pub fn get_images(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<RawImageEntry>> {
    load_entries(conn, ids, true)
}

/// Entries for `ids` in the given order; unknown ids fail with `not_found` when `strict`,
/// else are skipped (a page whose rows were deleted meanwhile).
fn load_entries(conn: &Connection, ids: &[ImageId], strict: bool) -> AppResult<Vec<RawImageEntry>> {
    let mut by_id: HashMap<ImageId, RawImageEntry> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!("{ENTRY_SELECT} WHERE i.id IN ({ph})"))?;
        for e in stmt.query_map(params_from_iter(chunk.iter()), entry_from_row)? {
            let e = e?;
            by_id.insert(e.id, e);
        }
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match by_id.get(id) {
            Some(e) => out.push(e.clone()),
            None if strict => return Err(AppError::not_found(format!("image {id}"))),
            None => {}
        }
    }
    attach_tags(conn, &mut out)?;
    crate::develop::edited::attach(conn, &mut out)?;
    Ok(out)
}

/// Binds `items` as text values and returns the matching `?,?,..` placeholder list.
fn text_list<T: Copy>(items: &[T], args: &mut Vec<Value>, as_str: fn(T) -> &'static str) -> String {
    args.extend(items.iter().map(|&t| Value::Text(as_str(t).to_owned())));
    vec!["?"; items.len()].join(",")
}

/// SQL predicate "is a keeper under `rule`" over the `images` columns prefixed with `prefix`
/// (`""` or `"i."`); mirror of `KeeperRule::is_keeper_values` (unknown pick values count as
/// unflagged). Values are integers, so inlining them is injection-safe.
pub fn keeper_predicate(rule: &KeeperRule, prefix: &str) -> String {
    if rule.mode == KeeperMode::NotRejected {
        return format!("({prefix}pick <> 'reject')");
    }
    format!(
        "({prefix}pick = 'pick' OR ({prefix}pick <> 'reject' AND ({prefix}rating >= {min} OR \
         ({sugg} AND {prefix}rating = 0 AND {prefix}id IN \
         (SELECT image_id FROM quality_scores WHERE suggested_pick = 'pick')))))",
        min = rule.min_rating,
        sugg = i32::from(rule.use_suggestions),
    )
}

/// A facet of the Library Filter metadata row (`MetadataFilterOptions`, v18): counted with
/// every constraint of the query except its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Facet {
    Format,
    Extension,
    Camera,
    Body,
    Lens,
    Iso,
    FocalLength,
    Aperture,
    Shutter,
    Captured,
    Edited,
    Sidecar,
}

/// Lower-case file extension of `{p}file_name` without the dot (`''` when none): the text
/// after the last dot (`rtrim` strips every non-dot character from the right).
fn ext_sql(p: &str) -> String {
    format!(
        "lower(CASE WHEN instr({p}file_name, '.') = 0 THEN '' \
         ELSE substr({p}file_name, length(rtrim({p}file_name, replace({p}file_name, '.', ''))) + 1) END)"
    )
}

/// Lens / camera model with blank values read as unknown (`NULL`).
fn lens_sql(p: &str) -> String {
    format!("NULLIF(TRIM({p}lens), '')")
}

fn model_sql(p: &str) -> String {
    format!("NULLIF(TRIM({p}camera_model), '')")
}

/// Body serial with blank values read as unknown (v19.2).
fn serial_sql(p: &str) -> String {
    format!("NULLIF(TRIM({p}camera_serial), '')")
}

/// `{p}`-prefixed predicate: the photo has a pending suggestion of `kind` (v19.2
/// `ImageQuery.suggested`): analysed, unflagged, 0 stars, and the suggestion matches. The
/// `CullSummary.suggested*Pending` / `apply_suggestions(onlyUnset)` rule.
fn suggested_sql(kind: PendingSuggestion, p: &str) -> String {
    let q = match kind {
        PendingSuggestion::Reject => "suggested_pick = 'reject'",
        PendingSuggestion::Pick => "suggested_pick = 'pick'",
        PendingSuggestion::Rating => "suggested_pick NOT IN ('pick', 'reject') AND suggested_rating > 0",
    };
    format!("({p}pick = 'unflagged' AND {p}rating = 0 AND {p}id IN (SELECT image_id FROM quality_scores WHERE {q}))")
}

/// `{p}`-prefixed "has develop edits" (same as `RawImageEntry.hasEdits`).
fn edited_sql(p: &str) -> String {
    format!("EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = {p}id AND a.neutral = 0)")
}

/// Day start (naive ms, floor) of `{p}captured_at_ms`; `NULL` without a capture time.
fn day_sql(p: &str) -> String {
    format!("({p}captured_at_ms - ((({p}captured_at_ms % 86400000) + 86400000) % 86400000))")
}

/// Appends `{expr} >= lo AND {expr} <= hi` for `range` (bounds widened by 1e-6 relative so a
/// facet value used as both bounds matches itself). Non-finite bounds -> `invalid_argument`.
fn range_clause(
    expr: &str,
    name: &str,
    range: &NumberRange,
    clauses: &mut Vec<String>,
    args: &mut Vec<Value>,
) -> AppResult<()> {
    for (bound, op, sign) in [(range.min, ">=", -1.0), (range.max, "<=", 1.0)] {
        if let Some(v) = bound {
            if !v.is_finite() {
                return Err(AppError::invalid(format!("{name} bound {v} is not a finite number")));
            }
            clauses.push(format!("{expr} {op} ?"));
            args.push(Value::Real(v + sign * v.abs() * 1e-6));
        }
    }
    Ok(())
}

/// `ImageQuery.metadata` constraints over columns prefixed with `p` (`"i."`, or `"images."`
/// for statements over the bare table), skipping `skip`'s own (facet counts).
fn metadata_clauses(
    m: &MetadataFilter,
    p: &str,
    skip: Option<Facet>,
    clauses: &mut Vec<String>,
    args: &mut Vec<Value>,
) -> AppResult<()> {
    let on = |f: Facet| skip != Some(f);
    if on(Facet::Format) && !m.formats.is_empty() {
        let ph = text_list(&m.formats, args, ImageFormat::as_str);
        clauses.push(format!("{p}format IN ({ph})"));
    }
    if on(Facet::Extension) && !m.extensions.is_empty() {
        for e in &m.extensions {
            if e.is_empty() || e.len() > 10 || !e.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(AppError::invalid(format!("file extension {e:?} must be 1..=10 letters or digits")));
            }
            args.push(Value::Text(e.to_ascii_lowercase()));
        }
        clauses.push(format!("{} IN ({})", ext_sql(p), vec!["?"; m.extensions.len()].join(",")));
    }
    if on(Facet::Camera) && !m.cameras.is_empty() {
        let mut ors = Vec::with_capacity(m.cameras.len());
        for c in &m.cameras {
            ors.push(format!("({p}camera_make = ? AND {} IS ?)", model_sql(p)));
            args.push(Value::Text(c.make.as_str().to_owned()));
            args.push(c.model.as_ref().map_or(Value::Null, |s| Value::Text(s.trim().to_owned())));
        }
        clauses.push(format!("({})", ors.join(" OR ")));
    }
    if on(Facet::Body) && !m.bodies.is_empty() {
        let mut ors = Vec::with_capacity(m.bodies.len());
        for b in &m.bodies {
            ors.push(format!("({p}camera_make = ? AND {} IS ? AND {} IS ?)", model_sql(p), serial_sql(p)));
            args.push(Value::Text(b.make.as_str().to_owned()));
            let blank = |v: &Option<String>| match v.as_deref().map(str::trim) {
                Some(t) if !t.is_empty() => Value::Text(t.to_owned()),
                _ => Value::Null,
            };
            args.push(blank(&b.model));
            args.push(blank(&b.serial));
        }
        clauses.push(format!("({})", ors.join(" OR ")));
    }
    if on(Facet::Lens) && !m.lenses.is_empty() {
        let known: Vec<&String> = m.lenses.iter().flatten().collect();
        let mut ors = Vec::new();
        if !known.is_empty() {
            args.extend(known.iter().map(|s| Value::Text(s.trim().to_owned())));
            ors.push(format!("{} IN ({})", lens_sql(p), vec!["?"; known.len()].join(",")));
        }
        if m.lenses.iter().any(Option::is_none) {
            ors.push(format!("{} IS NULL", lens_sql(p)));
        }
        clauses.push(format!("({})", ors.join(" OR ")));
    }
    if let (true, Some(r)) = (on(Facet::Iso), &m.iso) {
        range_clause(&format!("{p}iso"), "iso", r, clauses, args)?;
    }
    if let (true, Some(r)) = (on(Facet::FocalLength), &m.focal_length_mm) {
        range_clause(&format!("ROUND({p}focal_length_mm, 1)"), "focalLengthMm", r, clauses, args)?;
    }
    if let (true, Some(r)) = (on(Facet::Aperture), &m.aperture) {
        range_clause(&format!("ROUND({p}aperture, 1)"), "aperture", r, clauses, args)?;
    }
    if let (true, Some(r)) = (on(Facet::Shutter), &m.shutter_seconds) {
        range_clause(&format!("{p}shutter_s"), "shutterSeconds", r, clauses, args)?;
    }
    if let (true, Some(r)) = (on(Facet::Captured), &m.captured) {
        if let Some(from) = r.from_ms {
            clauses.push(format!("{p}captured_at_ms >= ?"));
            args.push(Value::Integer(from));
        }
        if let Some(to) = r.to_ms {
            clauses.push(format!("{p}captured_at_ms < ?"));
            args.push(Value::Integer(to));
        }
    }
    if let (true, Some(edited)) = (on(Facet::Edited), m.edited) {
        clauses.push(if edited { edited_sql(p) } else { format!("NOT {}", edited_sql(p)) });
    }
    if let (true, Some(sidecar)) = (on(Facet::Sidecar), m.has_sidecar) {
        clauses.push(format!("{p}xmp_mtime_ms IS {}NULL", if sidecar { "NOT " } else { "" }));
    }
    Ok(())
}

/// `ImageQuery.pickOrigin` as a predicate on the `images` row aliased `p` (v18.1). Only
/// flagged images match; a flag without an origin (pre-v18) counts as the user's, like
/// `CullSummary.rejectedByUser`.
fn pick_origin_sql(origin: PickOrigin, p: &str) -> String {
    match origin {
        PickOrigin::Auto => format!("({p}pick IN ('pick', 'reject') AND {p}pick_origin = 'auto')"),
        PickOrigin::User => format!("({p}pick IN ('pick', 'reject') AND {p}pick_origin IS NOT 'auto')"),
    }
}

/// `WHERE` clause (with leading space, or empty) and its bound values for `q`'s filters.
/// `keepers` = the catalog's keeper rule, required when `q.keepersOnly`.
fn query_filter(q: &ImageQuery, keepers: Option<&KeeperRule>) -> AppResult<(String, Vec<Value>)> {
    query_filter_skip(q, keepers, None)
}

/// [`query_filter`] without the metadata constraint of facet `skip`.
fn query_filter_skip(
    q: &ImageQuery,
    keepers: Option<&KeeperRule>,
    skip: Option<Facet>,
) -> AppResult<(String, Vec<Value>)> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();

    // Tag filters as set membership (the subquery is evaluated once into an ephemeral
    // index) rather than a correlated probe per image row: ~5x faster on 50k images.
    if !q.include_tags.is_empty() {
        let ph = text_list(&q.include_tags, &mut args, CullTag::as_str);
        clauses.push(match q.tag_match {
            TagMatch::Any => {
                format!("i.id IN (SELECT image_id FROM image_tags WHERE suppressed = 0 AND tag IN ({ph}))")
            }
            TagMatch::All => {
                let distinct = q.include_tags.iter().collect::<std::collections::HashSet<_>>().len();
                format!(
                    "i.id IN (SELECT image_id FROM image_tags WHERE suppressed = 0 AND tag IN ({ph})
                              GROUP BY image_id HAVING COUNT(DISTINCT tag) = {distinct})"
                )
            }
        });
    }
    if !q.exclude_tags.is_empty() {
        let ph = text_list(&q.exclude_tags, &mut args, CullTag::as_str);
        clauses.push(format!("i.id NOT IN (SELECT image_id FROM image_tags WHERE suppressed = 0 AND tag IN ({ph}))"));
    }
    if !q.picks.is_empty() {
        let ph = text_list(&q.picks, &mut args, PickFlag::as_str);
        clauses.push(format!("i.pick IN ({ph})"));
    }
    if let Some(origin) = q.pick_origin {
        clauses.push(pick_origin_sql(origin, "i."));
    }
    for (bound, op) in [(q.min_rating, ">="), (q.max_rating, "<=")] {
        if let Some(v) = bound {
            if v > 5 {
                return Err(AppError::invalid(format!("rating bound {v} is outside 0..=5")));
            }
            clauses.push(format!("i.rating {op} ?"));
            args.push(Value::Integer(v.into()));
        }
    }
    if !q.color_labels.is_empty() {
        let ph = text_list(&q.color_labels, &mut args, ColorLabel::as_str);
        clauses.push(format!("i.color_label IN ({ph})"));
    }
    if let Some(burst) = q.burst_group_id {
        clauses.push("i.burst_group_id = ?".into());
        args.push(Value::Integer(burst));
    }
    if let Some(scene) = q.scene_id {
        clauses.push("i.scene_id = ?".into());
        args.push(Value::Integer(scene));
    }
    if q.collapse_bursts {
        // Burst members other than a chosen keeper (driven from `burst_groups`, which is small).
        let in_folder = match q.folder_id {
            Some(f) => {
                args.push(Value::Integer(f));
                " AND m.folder_id = ?"
            }
            None => "",
        };
        clauses.push(format!(
            "i.id NOT IN (SELECT m.id FROM burst_groups b JOIN images m ON m.burst_group_id = b.id
                          WHERE b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> m.id{in_folder})"
        ));
    }
    if q.missing_only {
        clauses.push("i.missing_since_ms IS NOT NULL".into());
    }
    if q.keepers_only {
        let rule = keepers.ok_or_else(|| AppError::internal("keepersOnly without a keeper rule"))?;
        clauses.push(keeper_predicate(rule, "i."));
    }
    if let Some(kind) = q.suggested {
        clauses.push(suggested_sql(kind, "i."));
    }
    if !q.target_choices.is_empty() {
        clauses.push(crate::db::target::choices_clause(&q.target_choices, "i."));
    }
    if !q.target_reason_kinds.is_empty() {
        let ph = text_list(&q.target_reason_kinds, &mut args, TargetReasonKind::as_str);
        clauses.push(format!(
            "i.id IN (SELECT image_id FROM target_selection WHERE json_extract(reasons_json, '$[0].kind') IN ({ph}))"
        ));
    }
    if !q.target_piles.is_empty() {
        let ph = text_list(&q.target_piles, &mut args, TargetPile::as_str);
        clauses.push(format!(
            "i.id IN (SELECT s.image_id FROM target_selection s WHERE ({}) IN ({ph}))",
            crate::db::target::PILE_SQL
        ));
    }
    metadata_clauses(&q.metadata, "i.", skip, &mut clauses, &mut args)?;
    if let Some(folder) = q.folder_id {
        clauses.push("i.folder_id = ?".into());
        args.push(Value::Integer(folder));
    }
    if let Some(project) = q.project_id {
        clauses.push("i.folder_id IN (SELECT id FROM folders WHERE project_id = ?)".into());
        args.push(Value::Integer(project));
    }

    let where_sql = if clauses.is_empty() { String::new() } else { format!(" WHERE {}", clauses.join(" AND ")) };
    Ok((where_sql, args))
}

/// Sort keys of one image for [`list_image_ids`].
struct SortRow {
    id: ImageId,
    captured: Option<i64>,
    file_name: String,
    rating: i64,
    overall: Option<f64>,
    /// `target_moment` sort only: (moment start, moment id) and the selection score.
    moment: Option<(Option<i64>, i64)>,
    score: f64,
}

/// Orders `rows` like the SQL `ORDER BY` of `q`'s sort (always ending with a unique key):
/// - capture time: `captured IS NULL, captured, file_name, id` (desc: every key but the
///   null flag reversed);
/// - file name: `file_name, id` (desc reversed);
/// - quality: `overall IS NULL, overall DESC, id` (desc: `overall ASC, id DESC`);
/// - rating: `rating DESC` (desc: `ASC`), then capture-time ascending order.
///
/// Byte-wise string order equals SQLite's default `BINARY` collation. Sorting in Rust
/// instead of SQLite keeps the sorter from carrying wide rows (50k images: ~5x faster).
fn sort_rows(rows: &mut [SortRow], sort: ImageSort, descending: bool) {
    use std::cmp::Ordering;
    let capture = |a: &SortRow, b: &SortRow| {
        a.captured
            .is_none()
            .cmp(&b.captured.is_none())
            .then(a.captured.cmp(&b.captured))
            .then_with(|| a.file_name.cmp(&b.file_name))
            .then(a.id.cmp(&b.id))
    };
    let real = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    };
    match (sort, descending) {
        (ImageSort::CaptureTime, false) => rows.sort_unstable_by(capture),
        (ImageSort::CaptureTime, true) => rows.sort_unstable_by(|a, b| {
            a.captured
                .is_none()
                .cmp(&b.captured.is_none())
                .then(b.captured.cmp(&a.captured))
                .then_with(|| b.file_name.cmp(&a.file_name))
                .then(b.id.cmp(&a.id))
        }),
        (ImageSort::FileName, false) => {
            rows.sort_unstable_by(|a, b| a.file_name.cmp(&b.file_name).then(a.id.cmp(&b.id)))
        }
        (ImageSort::FileName, true) => {
            rows.sort_unstable_by(|a, b| b.file_name.cmp(&a.file_name).then(b.id.cmp(&a.id)))
        }
        (ImageSort::Quality, false) => rows.sort_unstable_by(|a, b| {
            a.overall.is_none().cmp(&b.overall.is_none()).then(real(b.overall, a.overall)).then(a.id.cmp(&b.id))
        }),
        (ImageSort::Quality, true) => rows.sort_unstable_by(|a, b| {
            a.overall.is_none().cmp(&b.overall.is_none()).then(real(a.overall, b.overall)).then(b.id.cmp(&a.id))
        }),
        (ImageSort::Rating, false) => rows.sort_unstable_by(|a, b| b.rating.cmp(&a.rating).then_with(|| capture(a, b))),
        (ImageSort::Rating, true) => rows.sort_unstable_by(|a, b| a.rating.cmp(&b.rating).then_with(|| capture(a, b))),
        (ImageSort::TargetMoment, desc) => rows.sort_unstable_by(|a, b| {
            let moment = |x: &(Option<i64>, i64), y: &(Option<i64>, i64)| {
                x.0.is_none().cmp(&y.0.is_none()).then(x.0.cmp(&y.0)).then(x.1.cmp(&y.1))
            };
            let by_moment = match (&a.moment, &b.moment) {
                (Some(x), Some(y)) if desc => moment(y, x),
                (Some(x), Some(y)) => moment(x, y),
                (x, y) => x.is_none().cmp(&y.is_none()),
            };
            by_moment.then(b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal)).then_with(|| capture(a, b))
        }),
    }
}

/// One page of `q` (`offset`/`limit`, capped at [`ImageQuery::MAX_LIMIT`]) + the total.
/// The page is cut from the sorted id list, then loaded by id, so deep pages cost the
/// same as the first one.
pub fn list_images(conn: &Connection, q: &ImageQuery) -> AppResult<ImagePage> {
    let ids = list_image_ids(conn, q)?;
    let total = ids.len() as u32;
    let limit = q.limit.min(ImageQuery::MAX_LIMIT) as usize;
    let page: Vec<ImageId> = ids.into_iter().skip(q.offset as usize).take(limit).collect();
    let items = load_entries(conn, &page, false)?;
    Ok(ImagePage { items, total })
}

/// Every id matching `q` in sort order; `offset`/`limit` are ignored.
pub fn list_image_ids(conn: &Connection, q: &ImageQuery) -> AppResult<Vec<ImageId>> {
    let rule = if q.keepers_only { Some(keeper_rule(conn)?) } else { None };
    let (where_sql, args) = query_filter(q, rule.as_ref())?;
    let quality = q.sort == ImageSort::Quality;
    let (name_col, overall_col, join) = if quality {
        ("''", "q.overall", " LEFT JOIN quality_scores q ON q.image_id = i.id")
    } else {
        ("i.file_name", "NULL", "")
    };
    let (moment_cols, moment_join) = if q.sort == ImageSort::TargetMoment {
        (
            "mo.started_at_ms, mo.id, COALESCE(ts.score, 0)",
            " LEFT JOIN target_selection ts ON ts.image_id = i.id LEFT JOIN moments mo ON mo.id = ts.moment_id",
        )
    } else {
        ("NULL, NULL, 0", "")
    };
    let sql = format!(
        "SELECT i.id, i.captured_at_ms, {name_col}, i.rating, {overall_col}, {moment_cols}
         FROM images i{join}{moment_join}{where_sql}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            Ok(SortRow {
                id: r.get(0)?,
                captured: r.get(1)?,
                file_name: r.get(2)?,
                rating: r.get(3)?,
                overall: r.get(4)?,
                moment: match r.get::<_, Option<i64>>(6)? {
                    Some(id) => Some((r.get(5)?, id)),
                    None => None,
                },
                score: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    sort_rows(&mut rows, q.sort, q.sort_descending);
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Facet counts over `scope` (a folder, a project's folders, or the whole catalog). Separate
/// statements per case so SQLite can use the folder index (an `?1 IS NULL OR folder_id = ?1`
/// predicate cannot).
pub fn filter_counts(conn: &Connection, scope: impl Into<FolderScope>) -> AppResult<FilterCounts> {
    filter_counts_impl(conn, scope.into(), None, &MetadataFilter::default(), None)
}

/// [`filter_counts`] over the keepers of `scope` only (`get_filter_counts(.., keepersOnly)`,
/// IPC v15; keeper rule = the catalog's).
pub fn filter_counts_keepers(conn: &Connection, scope: impl Into<FolderScope>) -> AppResult<FilterCounts> {
    filter_counts_with(conn, scope, true, &MetadataFilter::default(), None)
}

/// `get_filter_counts(folderId, projectId, keepersOnly, metadata, pickOrigin)` (v18, v18.1):
/// [`filter_counts`] over the images of `scope` that pass `metadata` and `pick_origin` (and
/// are keepers with `keepers_only`).
pub fn filter_counts_with(
    conn: &Connection,
    scope: impl Into<FolderScope>,
    keepers_only: bool,
    metadata: &MetadataFilter,
    pick_origin: Option<PickOrigin>,
) -> AppResult<FilterCounts> {
    let rule = if keepers_only { Some(keeper_rule(conn)?) } else { None };
    filter_counts_impl(conn, scope.into(), rule.as_ref(), metadata, pick_origin)
}

fn filter_counts_impl(
    conn: &Connection,
    scope: FolderScope,
    keepers: Option<&KeeperRule>,
    metadata: &MetadataFilter,
    pick_origin: Option<PickOrigin>,
) -> AppResult<FilterCounts> {
    let mut c = FilterCounts { ratings: vec![0; 6], ..Default::default() };
    let scoped;
    // Metadata constraints, once over the bare table and once over `i.` (same bound values,
    // each statement below contains the clause exactly once).
    let mut meta_args: Vec<Value> = Vec::new();
    let (meta_f, meta_fi) = if metadata.is_empty() {
        (String::new(), String::new())
    } else {
        let mut bare = Vec::new();
        metadata_clauses(metadata, "images.", None, &mut bare, &mut meta_args)?;
        let mut prefixed = Vec::new();
        metadata_clauses(metadata, "i.", None, &mut prefixed, &mut Vec::new())?;
        (format!(" AND {}", bare.join(" AND ")), format!(" AND {}", prefixed.join(" AND ")))
    };
    // Literal SQL (no bound values), so it can follow the metadata clause in both forms.
    let (meta_f, meta_fi) = match pick_origin {
        None => (meta_f, meta_fi),
        Some(o) => (
            format!("{meta_f} AND {}", pick_origin_sql(o, "images.")),
            format!("{meta_fi} AND {}", pick_origin_sql(o, "i.")),
        ),
    };
    let fast = scope.is_all() && keepers.is_none() && metadata.is_empty() && pick_origin.is_none();
    let mut suggest_scope = String::new();
    let (pick_sql, tag_sql, burst_sql, missing_sql) = match fast {
        true => (
            // Grouping by folder first follows `idx_images_folder_pick_rating` (0011)
            // without a temp B-tree; the per-folder rows are summed below.
            "SELECT pick, rating, COUNT(*) FROM images GROUP BY folder_id, pick, rating",
            // Covered by the partial `idx_image_tags_live` (0011).
            "SELECT tag, COUNT(*) FROM image_tags WHERE suppressed = 0 GROUP BY tag ORDER BY tag",
            "SELECT COUNT(DISTINCT i.burst_group_id),
                    COALESCE(SUM(b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id), 0)
             FROM images i JOIN burst_groups b ON b.id = i.burst_group_id",
            // Partial `idx_images_missing` (0011): only missing rows are visited.
            "SELECT COUNT(*) FROM images WHERE missing_since_ms IS NOT NULL",
        ),
        false => {
            let (f, fi) = match keepers {
                None => (scope.predicate("folder_id"), scope.predicate("i.folder_id")),
                Some(rule) => (
                    format!("{} AND {}", scope.predicate("folder_id"), keeper_predicate(rule, "")),
                    format!("{} AND {}", scope.predicate("i.folder_id"), keeper_predicate(rule, "i.")),
                ),
            };
            let (f, fi) = (format!("{f}{meta_f}"), format!("{fi}{meta_fi}"));
            suggest_scope.clone_from(&fi);
            scoped = [
                format!("SELECT pick, rating, COUNT(*) FROM images WHERE {f} GROUP BY pick, rating"),
                format!(
                    "SELECT tag, COUNT(*) FROM image_tags
                     WHERE suppressed = 0 AND image_id IN (SELECT id FROM images WHERE {f})
                     GROUP BY tag ORDER BY tag"
                ),
                format!(
                    "SELECT COUNT(DISTINCT i.burst_group_id),
                            COALESCE(SUM(b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id), 0)
                     FROM images i JOIN burst_groups b ON b.id = i.burst_group_id WHERE {fi}"
                ),
                format!("SELECT COUNT(*) FROM images WHERE missing_since_ms IS NOT NULL AND {f}"),
            ];
            (scoped[0].as_str(), scoped[1].as_str(), scoped[2].as_str(), scoped[3].as_str())
        }
    };
    let args = meta_args;
    {
        let mut stmt = conn.prepare_cached(pick_sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?, r.get::<_, u32>(2)?))
        })?;
        for row in rows {
            let (pick, rating, n) = row?;
            c.total += n;
            if let Some(slot) = c.ratings.get_mut(usize::from(rating)) {
                *slot += n;
            }
            match PickFlag::parse(&pick) {
                Some(PickFlag::Pick) => c.picked += n,
                Some(PickFlag::Reject) => c.rejected += n,
                _ => c.unflagged += n,
            }
        }
    }
    c.tags = conn
        .prepare_cached(tag_sql)?
        .query_map(params_from_iter(args.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
        .filter_map(|r| match r {
            Ok((tag, count)) => CullTag::parse(&tag).map(|tag| Ok(TagCount { tag, count })),
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    (c.burst_groups, c.burst_non_keepers) =
        conn.prepare_cached(burst_sql)?.query_row(params_from_iter(args.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
    c.missing = conn.prepare_cached(missing_sql)?.query_row(params_from_iter(args.iter()), |r| r.get(0))?;
    // v19.2 pending suggestions (`suggested_sql`); only untouched rows are visited.
    let suggest_where = if fast { String::new() } else { format!(" AND {}", suggest_scope) };
    let suggest_sql = format!(
        "SELECT COALESCE(SUM(q.suggested_pick = 'reject'), 0), COALESCE(SUM(q.suggested_pick = 'pick'), 0),
                COALESCE(SUM(q.suggested_pick NOT IN ('pick', 'reject') AND q.suggested_rating > 0), 0)
         FROM images i JOIN quality_scores q ON q.image_id = i.id
         WHERE i.pick = 'unflagged' AND i.rating = 0{suggest_where}"
    );
    (c.suggested_reject, c.suggested_pick, c.suggested_rating) = conn
        .prepare_cached(&suggest_sql)?
        .query_row(params_from_iter(args.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(c)
}

/// Images of `scope` whose last sidecar write / read failed, capture order (IPC v15
/// `list_xmp_failures`).
pub fn xmp_failures(conn: &Connection, scope: &FolderScope) -> AppResult<Vec<XmpFailure>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, xmp_error FROM images
         WHERE xmp_error IS NOT NULL AND {}
         ORDER BY captured_at_ms IS NULL, captured_at_ms, file_name, id",
        scope.predicate("folder_id")
    ))?;
    let rows = stmt.query_map([], |r| Ok(XmpFailure { image_id: r.get(0)?, reason: r.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---------------------------------------------------------------------------
// Cull summary + metadata facets (IPC v18)
// ---------------------------------------------------------------------------

/// `get_cull_summary(projectId)`: flag counts, keepers under the catalog's rule and their
/// breakdown, over `scope`. `keepers` uses [`keeper_predicate`], so it equals a `keepersOnly`
/// query over the same scope.
pub fn cull_summary(conn: &Connection, scope: &FolderScope) -> AppResult<CullSummary> {
    let rule = keeper_rule(conn)?;
    let sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM(i.pick = 'pick'), 0),
                COALESCE(SUM(i.pick = 'pick' AND i.pick_origin = 'auto'), 0),
                COALESCE(SUM(i.pick = 'reject'), 0),
                COALESCE(SUM(i.pick = 'reject' AND i.pick_origin = 'auto'), 0),
                COALESCE(SUM(i.rating > 0), 0),
                COALESCE(SUM(i.pick NOT IN ('pick', 'reject') AND i.rating >= ?1), 0),
                COALESCE(SUM(i.pick NOT IN ('pick', 'reject') AND i.rating = 0 AND q.suggested_pick = 'pick'), 0),
                COALESCE(SUM({untouched} AND q.suggested_pick = 'reject'), 0),
                COALESCE(SUM({untouched} AND q.suggested_pick = 'pick'), 0),
                COALESCE(SUM({untouched} AND q.suggested_pick NOT IN ('pick', 'reject') AND q.suggested_rating > 0), 0),
                COALESCE(SUM(q.image_id IS NULL), 0),
                COALESCE(SUM({keeper}), 0)
           FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id
          WHERE {scope}",
        keeper = keeper_predicate(&rule, "i."),
        scope = scope.predicate("i.folder_id"),
        // `apply_suggestions(onlyUnset = true)`'s own condition (analysed = `q` joined).
        untouched = "(q.image_id IS NOT NULL AND i.pick = 'unflagged' AND i.rating = 0)",
    );
    let row: [u32; 13] = conn.query_row(&sql, [rule.min_rating], |r| {
        let mut out = [0u32; 13];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = r.get(i)?;
        }
        Ok(out)
    })?;
    let [total, picked, picked_auto, rejected, rejected_auto, starred, starred_unflagged, suggested_unflagged, sugg_reject, sugg_pick, sugg_rating, unanalyzed, keepers] =
        row;
    let unflagged = total - picked - rejected;
    let keeper_breakdown = match rule.mode {
        KeeperMode::NotRejected => KeeperBreakdown { picked, unflagged, ..Default::default() },
        KeeperMode::PicksAndRatings => KeeperBreakdown {
            picked,
            starred: starred_unflagged,
            suggested: if rule.use_suggestions { suggested_unflagged } else { 0 },
            ..Default::default()
        },
    };
    Ok(CullSummary {
        total,
        picked,
        picked_auto,
        unflagged,
        rejected,
        rejected_by_user: rejected - rejected_auto,
        rejected_auto,
        starred,
        keepers,
        keeper_breakdown,
        keeper_rule: rule,
        suggested_reject_pending: sugg_reject,
        suggested_pick_pending: sugg_pick,
        suggested_rating_pending: sugg_rating,
        unanalyzed,
    })
}

/// `get_metadata_filter_options(query)`: distinct values with counts per metadata facet over
/// `q`'s images, each facet ignoring its own constraint (see `MetadataFilterOptions`).
/// `offset` / `limit` / sort are ignored.
pub fn metadata_filter_options(conn: &Connection, q: &ImageQuery) -> AppResult<MetadataFilterOptions> {
    let rule = if q.keepers_only { Some(keeper_rule(conn)?) } else { None };
    // `SELECT {cols}, COUNT(*) FROM images i WHERE <q without facet's constraint> GROUP BY ..`.
    let grouped = |facet: Facet, cols: &str, n_keys: usize, f: &mut dyn FnMut(&Row, u32) -> rusqlite::Result<()>| {
        let (where_sql, args) = query_filter_skip(q, rule.as_ref(), Some(facet))?;
        let group: Vec<String> = (1..=n_keys).map(|i| i.to_string()).collect();
        let sql = format!("SELECT {cols}, COUNT(*) FROM images i{where_sql} GROUP BY {}", group.join(", "));
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        while let Some(r) = rows.next()? {
            let count: u32 = r.get(n_keys)?;
            f(r, count)?;
        }
        Ok::<(), AppError>(())
    };
    let mut o = MetadataFilterOptions::default();
    {
        let (where_sql, args) = query_filter(q, rule.as_ref())?;
        o.total =
            conn.query_row(&format!("SELECT COUNT(*) FROM images i{where_sql}"), params_from_iter(args.iter()), |r| {
                r.get(0)
            })?;
    }
    grouped(Facet::Format, "i.format", 1, &mut |r, count| {
        if let Some(format) = ImageFormat::parse(&r.get::<_, String>(0)?) {
            o.formats.push(FormatCount { format, count });
        }
        Ok(())
    })?;
    o.formats.sort_by_key(|c| ImageFormat::ALL.iter().position(|&f| f == c.format));
    grouped(Facet::Extension, &ext_sql("i."), 1, &mut |r, count| {
        o.extensions.push(ExtensionCount { extension: r.get(0)?, count });
        Ok(())
    })?;
    o.extensions.sort_by(|a, b| a.extension.cmp(&b.extension));
    grouped(Facet::Camera, &format!("i.camera_make, {}", model_sql("i.")), 2, &mut |r, count| {
        let make = CameraMake::parse(&r.get::<_, String>(0)?).unwrap_or(CameraMake::Other);
        o.cameras.push(CameraCount { camera: CameraFilter { make, model: r.get(1)? }, count });
        Ok(())
    })?;
    o.cameras.sort_by(|a, b| {
        (a.camera.model.is_none(), a.camera.make.as_str(), &a.camera.model).cmp(&(
            b.camera.model.is_none(),
            b.camera.make.as_str(),
            &b.camera.model,
        ))
    });
    grouped(Facet::Body, &format!("i.camera_make, {}, {}", model_sql("i."), serial_sql("i.")), 3, &mut |r, count| {
        let make = CameraMake::parse(&r.get::<_, String>(0)?).unwrap_or(CameraMake::Other);
        o.bodies.push(CameraBodyCount { body: CameraBody { make, model: r.get(1)?, serial: r.get(2)? }, count });
        Ok(())
    })?;
    o.bodies.sort_by(|a, b| {
        let key = |c: &CameraBody| {
            (c.model.is_none(), c.make.as_str(), c.model.clone(), c.serial.is_none(), c.serial.clone())
        };
        key(&a.body).cmp(&key(&b.body))
    });
    grouped(Facet::Lens, &lens_sql("i."), 1, &mut |r, count| {
        o.lenses.push(LensCount { lens: r.get(0)?, count });
        Ok(())
    })?;
    o.lenses.sort_by(|a, b| (a.lens.is_none(), &a.lens).cmp(&(b.lens.is_none(), &b.lens)));
    let numbers = |facet: Facet, expr: String, out: &mut Vec<NumberCount>| -> AppResult<()> {
        grouped(facet, &expr, 1, &mut |r, count| {
            out.push(NumberCount { value: r.get(0)?, count });
            Ok(())
        })?;
        out.sort_by(|a, b| match (a.value, b.value) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (x, y) => x.is_none().cmp(&y.is_none()),
        });
        Ok(())
    };
    numbers(Facet::Iso, "i.iso".into(), &mut o.isos)?;
    numbers(Facet::FocalLength, "ROUND(i.focal_length_mm, 1)".into(), &mut o.focal_lengths)?;
    numbers(Facet::Aperture, "ROUND(i.aperture, 1)".into(), &mut o.apertures)?;
    numbers(Facet::Shutter, "i.shutter_s".into(), &mut o.shutter_speeds)?;
    grouped(Facet::Captured, &day_sql("i."), 1, &mut |r, count| {
        o.capture_days.push(DayCount { day_start_ms: r.get(0)?, count });
        Ok(())
    })?;
    o.capture_days.sort_by_key(|c| (c.day_start_ms.is_none(), c.day_start_ms));
    grouped(Facet::Edited, &edited_sql("i."), 1, &mut |r, count| {
        if r.get::<_, bool>(0)? {
            o.edited.yes += count;
        } else {
            o.edited.no += count;
        }
        Ok(())
    })?;
    grouped(Facet::Sidecar, "i.xmp_mtime_ms IS NOT NULL", 1, &mut |r, count| {
        if r.get::<_, bool>(0)? {
            o.has_sidecar.yes += count;
        } else {
            o.has_sidecar.no += count;
        }
        Ok(())
    })?;
    Ok(o)
}

// ---------------------------------------------------------------------------
// Missing originals (IPC v13)
// ---------------------------------------------------------------------------

/// Records the outcome of accessing image `id`'s original: `missing` sets
/// `missing_since_ms` (keeping the first time it was found missing), `!missing` clears it.
/// A no-op write when nothing changes; returns whether the row changed. Unknown ids are
/// ignored (the image may have been removed meanwhile).
pub fn set_original_missing(conn: &Connection, id: ImageId, missing: bool) -> AppResult<bool> {
    let n = if missing {
        conn.prepare_cached("UPDATE images SET missing_since_ms = ?2 WHERE id = ?1 AND missing_since_ms IS NULL")?
            .execute(params![id, now_ms()])?
    } else {
        conn.prepare_cached("UPDATE images SET missing_since_ms = NULL WHERE id = ?1 AND missing_since_ms IS NOT NULL")?
            .execute([id])?
    };
    Ok(n > 0)
}

/// [`set_original_missing`] from a per-file failure `reason` (plain string): a missing
/// original (`raw::access::MISSING_PREFIX`) flags the image; any other failure says nothing
/// about the file's presence and changes nothing.
pub fn note_access_failure(conn: &Connection, id: ImageId, reason: &str) -> AppResult<()> {
    if raw::access::is_missing_message(reason) {
        set_original_missing(conn, id, true)?;
    }
    Ok(())
}

/// Folder path and its images' (id, path, file name, companion path).
type FolderImages = (String, Vec<(ImageId, String, String, Option<String>)>);

fn folder_images(conn: &Connection, folder_id: FolderId) -> AppResult<FolderImages> {
    let folder: String = conn
        .query_row("SELECT path FROM folders WHERE id = ?1", [folder_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("folder {folder_id}")))?;
    let images = conn
        .prepare("SELECT id, path, file_name, companion_path FROM images WHERE folder_id = ?1 ORDER BY id")?
        .query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((folder, images))
}

/// Outcome of [`relocate_folder`] for the command layer.
#[derive(Debug, Default)]
pub struct Relocated {
    pub result: RelocateResult,
    /// Images whose path changed (their develop sources must be forgotten).
    pub moved: Vec<ImageId>,
    /// Moved images whose thumbnail extraction had failed because the file was missing
    /// (to re-extract).
    pub retry_thumbnails: Vec<ImageId>,
}

/// `relocate_folder`: points folder `folder_id` (and its images) at `new_path`, e.g. after
/// the shoot was moved or its drive remounted under another name. Each image is looked up
/// at its path relative to the old folder, then by file name anywhere under `new_path`
/// (unique names only). Images found are repointed and their missing flag cleared; the
/// rest keep their path and are flagged missing. Companion JPEG/HEIC paths are repointed
/// the same way. Fails with `invalid_argument` (changing nothing) when `new_path` is not a
/// directory, is already another catalog folder, or holds none of the folder's images.
/// Atomic.
pub fn relocate_folder(conn: &mut Connection, folder_id: FolderId, new_path: &Path) -> AppResult<Relocated> {
    let new_root = new_path.canonicalize().map_err(|e| AppError::invalid(format!("{}: {e}", new_path.display())))?;
    if !new_root.is_dir() {
        return Err(AppError::invalid(format!("{} is not a folder", new_root.display())));
    }
    let new_root_s = path_str(&new_root)?;
    let (old_root, images) = folder_images(conn, folder_id)?;
    let other: Option<FolderId> = conn
        .query_row("SELECT id FROM folders WHERE path = ?1 AND id <> ?2", params![new_root_s, folder_id], |r| r.get(0))
        .optional()?;
    if other.is_some() {
        return Err(AppError::invalid(format!(
            "{new_root_s} is already in the catalog as another folder; choose the folder this shoot was moved to"
        )));
    }

    // File name -> path under the new root (names found more than once are ambiguous).
    let mut by_name: HashMap<String, Option<std::path::PathBuf>> = HashMap::new();
    let mut indexed = false;
    let mut index = |by_name: &mut HashMap<String, Option<std::path::PathBuf>>| {
        if indexed {
            return;
        }
        indexed = true;
        for e in WalkDir::new(&new_root).into_iter().filter_map(Result::ok).filter(|e| e.file_type().is_file()) {
            let name = e.file_name().to_string_lossy().into_owned();
            by_name.entry(name).and_modify(|v| *v = None).or_insert_with(|| Some(e.path().to_path_buf()));
        }
    };
    let old = Path::new(&old_root);
    let mut locate = |path: &str, name: &str, by_name: &mut HashMap<String, Option<std::path::PathBuf>>| {
        if let Ok(rel) = Path::new(path).strip_prefix(old) {
            let candidate = new_root.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        index(by_name);
        by_name.get(name).cloned().flatten()
    };

    let mut plan: Vec<(ImageId, Option<String>, Option<String>)> = Vec::with_capacity(images.len());
    for (id, path, name, companion) in &images {
        let found = locate(path, name, &mut by_name).map(|p| path_str(&p)).transpose()?;
        let companion = match companion {
            Some(c) => {
                let cname = Path::new(c).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                locate(c, &cname, &mut by_name).map(|p| path_str(&p)).transpose()?
            }
            None => None,
        };
        plan.push((*id, found, companion));
    }
    let matched = plan.iter().filter(|(_, f, _)| f.is_some()).count();
    if matched == 0 && !images.is_empty() {
        return Err(AppError::invalid(format!(
            "None of the {} photos of {old_root} were found in {new_root_s}. Choose the folder the shoot was moved to.",
            images.len()
        )));
    }

    let mut out = Relocated::default();
    super::atomic(conn, |tx| {
        tx.execute("UPDATE folders SET path = ?2 WHERE id = ?1", params![folder_id, new_root_s])?;
        let mut repoint = tx.prepare(
            "UPDATE images SET path = ?2, missing_since_ms = NULL,
                               companion_path = COALESCE(?3, companion_path)
             WHERE id = ?1",
        )?;
        let mut taken = tx.prepare("SELECT EXISTS (SELECT 1 FROM images WHERE path = ?1 AND id <> ?2)")?;
        let mut failed_missing =
            tx.prepare("SELECT error FROM thumbnails WHERE image_id = ?1 AND status = 'failed'")?;
        for (id, found, companion) in &plan {
            let target = match found {
                // Another catalog image already has that path: leave this one missing.
                Some(p) if !taken.query_row(params![p, id], |r| r.get::<_, bool>(0))? => p,
                _ => {
                    set_original_missing(tx, *id, true)?;
                    out.result.still_missing += 1;
                    continue;
                }
            };
            repoint.execute(params![id, target, companion])?;
            out.result.matched += 1;
            out.moved.push(*id);
            let error: Option<Option<String>> = failed_missing.query_row([id], |r| r.get(0)).optional()?;
            if error.flatten().is_some_and(|e| raw::access::is_missing_message(&e)) {
                out.retry_thumbnails.push(*id);
            }
        }
        Ok(())
    })?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Culling writes
// ---------------------------------------------------------------------------

/// Runs `sql` with `(value, id)` for every id in one transaction; errors if any id is unknown.
fn update_each(conn: &mut Connection, ids: &[ImageId], sql: &str, value: Value) -> AppResult<()> {
    let tx = conn.savepoint()?;
    {
        let mut stmt = tx.prepare(sql)?;
        for &id in ids {
            if stmt.execute(params![value, id])? == 0 {
                return Err(AppError::not_found(format!("image {id}")));
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn set_rating(conn: &mut Connection, ids: &[ImageId], rating: u8) -> AppResult<()> {
    if rating > 5 {
        return Err(AppError::invalid(format!("rating {rating} is outside 0..=5")));
    }
    update_each(conn, ids, "UPDATE images SET rating = ?1 WHERE id = ?2", Value::Integer(rating.into()))
}

pub fn set_pick(conn: &mut Connection, ids: &[ImageId], pick: PickFlag) -> AppResult<()> {
    // A user flag (v16 `pick_origin`), also when re-setting the value "Auto" chose.
    update_each(
        conn,
        ids,
        "UPDATE images SET pick = ?1, pick_origin = 'user' WHERE id = ?2",
        Value::Text(pick.as_str().into()),
    )
}

pub fn set_color_label(conn: &mut Connection, ids: &[ImageId], label: Option<ColorLabel>) -> AppResult<()> {
    let value = label.map_or(Value::Null, |l| Value::Text(l.as_str().into()));
    update_each(conn, ids, "UPDATE images SET color_label = ?1 WHERE id = ?2", value)
}

/// Adds or removes a tag as the user. Removing an auto tag suppresses it rather than
/// deleting it, so re-running analysis does not bring it back.
pub fn set_user_tag(conn: &mut Connection, ids: &[ImageId], tag: CullTag, present: bool) -> AppResult<()> {
    let tx = conn.savepoint()?;
    for &id in ids {
        let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)", [id], |r| r.get(0))?;
        if !exists {
            return Err(AppError::not_found(format!("image {id}")));
        }
        if present {
            tx.execute(
                "INSERT INTO image_tags (image_id, tag, source, confidence, suppressed)
                 VALUES (?1, ?2, 'user', 1.0, 0)
                 ON CONFLICT(image_id, tag) DO UPDATE SET source = 'user', confidence = 1.0, suppressed = 0",
                params![id, tag.as_str()],
            )?;
        } else {
            tx.execute(
                "DELETE FROM image_tags WHERE image_id = ?1 AND tag = ?2 AND source = 'user'",
                params![id, tag.as_str()],
            )?;
            tx.execute(
                "UPDATE image_tags SET suppressed = 1 WHERE image_id = ?1 AND tag = ?2 AND source = 'auto'",
                params![id, tag.as_str()],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Adjustments
// ---------------------------------------------------------------------------

/// Source format of image `id` (`not_found` if absent).
pub fn image_format(conn: &Connection, id: ImageId) -> AppResult<ImageFormat> {
    let s: Option<String> = conn.query_row("SELECT format FROM images WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let s = s.ok_or_else(|| AppError::not_found(format!("image {id}")))?;
    ImageFormat::parse(&s).ok_or_else(|| AppError::internal(format!("image {id}: unknown format {s:?}")))
}

/// Stored adjustments, or the format's neutral defaults
/// (`ParametricAdjustments::defaults_for`) if the image has never been edited.
pub fn get_adjustments(conn: &Connection, id: ImageId) -> AppResult<ParametricAdjustments> {
    // Also distinguishes "unedited" from "no such image".
    let format = image_format(conn, id)?;
    let json: Option<String> =
        conn.query_row("SELECT params_json FROM adjustments WHERE image_id = ?1", [id], |r| r.get(0)).optional()?;
    let defaults = ParametricAdjustments::defaults_for(format);
    match json {
        Some(json) => {
            // Overlay stored values on the defaults so JSON written before a slider or
            // group existed (e.g. pre-v9 rows without `toneCurve`) still loads.
            let mut merged = serde_json::to_value(defaults)?;
            merge_json(&mut merged, serde_json::from_str(&json)?);
            Ok(serde_json::from_value(merged)?)
        }
        None => Ok(defaults),
    }
}

fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (k, v) in overlay {
                match base.get_mut(&k) {
                    Some(slot) => merge_json(slot, v),
                    None => {
                        base.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

/// Upserts the adjustments row only (no history; see `develop::history::commit` for the
/// command path). Keeps `neutral` in step with the values.
pub fn save_adjustments(conn: &Connection, id: ImageId, adj: &ParametricAdjustments) -> AppResult<()> {
    adj.validate().map_err(AppError::invalid)?;
    let neutral = adj.is_neutral_for(image_format(conn, id)?);
    let json = serde_json::to_string(adj)?;
    let updated_at = now_ms();
    let changed = conn.execute(
        "INSERT INTO adjustments (image_id, params_json, process_version, updated_at, neutral)
         SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM images WHERE id = ?1)
         ON CONFLICT(image_id) DO UPDATE SET
             params_json = excluded.params_json,
             process_version = excluded.process_version,
             updated_at = excluded.updated_at,
             neutral = excluded.neutral",
        params![id, json, adj.process_version, updated_at, neutral],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("image {id}")));
    }
    // Regenerates the edited preview once this write is committed (IPC v19.1).
    crate::develop::edited::notify_saved(conn, id, updated_at);
    Ok(())
}

// ---------------------------------------------------------------------------
// Ingest (thumbnail + EXIF extraction)
// ---------------------------------------------------------------------------

/// An image waiting for extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingImage {
    pub id: ImageId,
    pub path: String,
    pub format: RawFormat,
}

/// Up to `limit` images with `thumbnails.status = 'pending'`, lowest id first.
pub fn pending_thumbnails(conn: &Connection, limit: u32) -> AppResult<Vec<PendingImage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.id, i.path, i.format FROM thumbnails t JOIN images i ON i.id = t.image_id
         WHERE t.status = 'pending' ORDER BY t.image_id LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok(PendingImage { id: r.get(0)?, path: r.get(1)?, format: enum_col(r, 2, RawFormat::parse)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn count_pending(conn: &Connection) -> AppResult<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM thumbnails WHERE status = 'pending'", [], |r| r.get(0))?)
}

/// Generated cache files for one image.
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbFiles {
    pub thumb_path: String,
    pub preview_path: Option<String>,
    /// Thumbnail pixel size after orientation.
    pub width: u32,
    pub height: u32,
}

/// f32 EXIF values widened to f64 as-is store e.g. 1.4 as 1.399999976; keep 2 decimals.
fn round_f32(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

/// Records one extraction result atomically: EXIF columns (if any were read) and the
/// thumbnail row (`ready` with files, or `failed` with `reason`).
pub fn record_extraction(
    conn: &mut Connection,
    id: ImageId,
    meta: Option<&raw::meta::ImageMeta>,
    outcome: Result<&ThumbFiles, &str>,
) -> AppResult<()> {
    let tx = conn.savepoint()?;
    if let Some(m) = meta {
        tx.prepare_cached(
            "UPDATE images SET
                 camera_make = COALESCE(?2, camera_make),
                 camera_model = COALESCE(?3, camera_model),
                 sensor_layout = COALESCE(?4, sensor_layout),
                 lens = ?5, exif_captured_at_ms = ?6,
                 captured_at_ms = CASE WHEN capture_time_source = 'exif' THEN ?6 ELSE captured_at_ms END,
                 -- A sidecar time read before the EXIF was known (import reads sidecars first)
                 -- that equals it is no correction.
                 capture_time_source = CASE WHEN capture_time_source = 'sidecar' AND captured_at_ms IS ?6
                                            THEN 'exif' ELSE capture_time_source END,
                 iso = ?7, shutter_s = ?8, aperture = ?9,
                 focal_length_mm = ?10, width = ?11, height = ?12, orientation = ?13,
                 camera_serial = COALESCE(?14, camera_serial), camera_serial_read = 1
             WHERE id = ?1",
        )?
        .execute(params![
            id,
            m.make.map(|v| v.as_str()),
            m.model,
            m.sensor_layout.map(|v| v.as_str()),
            m.lens,
            m.captured_at_ms,
            m.iso,
            m.shutter_seconds,
            m.aperture.map(round_f32),
            m.focal_length_mm.map(round_f32),
            m.width,
            m.height,
            m.orientation,
            m.serial,
        ])?;
    }
    let now = now_ms();
    match outcome {
        Ok(f) => tx
            .prepare_cached(
                "UPDATE thumbnails SET status = 'ready', path = ?2, preview_path = ?3, width = ?4, height = ?5,
                 error = NULL, extracted_at = ?6
             WHERE image_id = ?1",
            )?
            .execute(params![id, f.thumb_path, f.preview_path, f.width, f.height, now])?,
        Err(reason) => tx
            .prepare_cached(
                "UPDATE thumbnails SET status = 'failed', path = NULL, preview_path = NULL, width = NULL,
                 height = NULL, error = ?2, extracted_at = ?3
             WHERE image_id = ?1",
            )?
            .execute(params![id, reason, now])?,
    };
    // The original was read (success) or found missing (IPC v13 missing flag).
    match outcome {
        Ok(_) => set_original_missing(&tx, id, false)?,
        Err(reason) if raw::access::is_missing_message(reason) => set_original_missing(&tx, id, true)?,
        Err(_) => false,
    };
    tx.commit()?;
    Ok(())
}

/// Resets `ids` to `pending` (clearing paths/errors) so the pipeline redoes them.
/// Atomic; unknown ids fail the batch with `not_found`. Returns the old cache paths
/// so the caller can delete them.
pub fn reset_thumbnails(conn: &mut Connection, ids: &[ImageId]) -> AppResult<Vec<String>> {
    let tx = conn.savepoint()?;
    let mut old = Vec::new();
    {
        let mut select = tx.prepare(
            "SELECT t.path, t.preview_path FROM images i
                                     LEFT JOIN thumbnails t ON t.image_id = i.id WHERE i.id = ?1",
        )?;
        let mut upsert = tx.prepare(
            "INSERT INTO thumbnails (image_id, status) VALUES (?1, 'pending')
             ON CONFLICT(image_id) DO UPDATE SET status = 'pending', path = NULL, preview_path = NULL,
                 width = NULL, height = NULL, error = NULL, extracted_at = NULL",
        )?;
        for &id in ids {
            let paths = select
                .query_row([id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
                .optional()?
                .ok_or_else(|| AppError::not_found(format!("image {id}")))?;
            old.extend(paths.0);
            old.extend(paths.1);
            upsert.execute([id])?;
        }
    }
    tx.commit()?;
    Ok(old)
}

// ---------------------------------------------------------------------------
// Analysis reads + user-driven writes (Phase 3 contract plumbing). The worker's writes
// (measurements, scores, auto tags, burst groups) live in `ml/` (vision-ml-dev).
// ---------------------------------------------------------------------------

fn ensure_image(conn: &Connection, id: ImageId) -> AppResult<()> {
    let exists: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)", [id], |r| r.get(0))?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found(format!("image {id}")))
    }
}

/// Detected faces from the last analysis; empty if unanalyzed or no faces.
pub fn get_faces(conn: &Connection, id: ImageId) -> AppResult<Vec<FaceInfo>> {
    ensure_image(conn, id)?;
    let json: Option<Option<String>> =
        conn.query_row("SELECT faces_json FROM image_analysis WHERE image_id = ?1", [id], |r| r.get(0)).optional()?;
    match json.flatten() {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(Vec::new()),
    }
}

/// Burst groups (optionally only those with a member in `folder`), in capture order.
/// Burst groups with a member in `scope` (all members listed), start order.
pub fn list_burst_groups(conn: &Connection, scope: impl Into<FolderScope>) -> AppResult<Vec<BurstGroup>> {
    let scope: FolderScope = scope.into();
    let filter = match scope.is_all() {
        true => String::new(),
        false => format!(
            "WHERE b.id IN (SELECT burst_group_id FROM images WHERE {} AND burst_group_id IS NOT NULL)",
            scope.predicate("folder_id")
        ),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT b.id, b.started_at_ms, b.ended_at_ms, b.keeper_image_id, i.id
         FROM burst_groups b JOIN images i ON i.burst_group_id = b.id
         {filter}
         ORDER BY b.started_at_ms, b.id, i.captured_at_ms, i.file_name, i.id"
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            BurstGroup {
                id: r.get(0)?,
                started_at_ms: r.get(1)?,
                ended_at_ms: r.get(2)?,
                keeper_image_id: r.get(3)?,
                image_ids: Vec::new(),
            },
            r.get::<_, ImageId>(4)?,
        ))
    })?;
    let mut groups: Vec<BurstGroup> = Vec::new();
    for row in rows {
        let (group, member) = row?;
        match groups.last_mut() {
            Some(last) if last.id == group.id => last.image_ids.push(member),
            _ => groups.push(BurstGroup { image_ids: vec![member], ..group }),
        }
    }
    Ok(groups)
}

/// Copies `suggested_rating` / `suggested_pick` into the user's rating/pick for `ids`.
/// Unanalyzed images are skipped, and so are images already matching their suggestion (v18.1,
/// so `applied` counts real changes); with `only_unset`, so are images already flagged
/// (`pick != unflagged`) or rated (`rating != 0`). With `only_unset` over a scope it changes
/// exactly the `CullSummary.suggested*Pending` images. Atomic; unknown ids fail with `not_found`.
/// A flag it changes gets `pick_origin = 'auto'` (v16); an unchanged flag keeps its origin.
pub fn apply_suggestions(
    conn: &mut Connection,
    ids: &[ImageId],
    only_unset: bool,
) -> AppResult<ApplySuggestionsResult> {
    apply_suggestions_kinds(conn, ids, only_unset, SuggestionKinds::default())
}

/// [`apply_suggestions`] copying only the suggestion kinds in `kinds` (v19.2, see
/// [`SuggestionKinds`]): a suggested pick flag with `picks`, a suggested reject with `rejects`,
/// a suggested "no flag" only with both, the suggested stars with `stars`. An image counts as
/// applied only when its flag or stars change.
pub fn apply_suggestions_kinds(
    conn: &mut Connection,
    ids: &[ImageId],
    only_unset: bool,
    kinds: SuggestionKinds,
) -> AppResult<ApplySuggestionsResult> {
    let tx = conn.savepoint()?;
    let mut applied = 0;
    {
        let mut read = tx.prepare_cached(
            "SELECT i.pick, i.rating, q.suggested_pick, q.suggested_rating
             FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id WHERE i.id = ?1",
        )?;
        let mut write = tx.prepare_cached(
            "UPDATE images SET rating = ?2, pick = ?3,
                 pick_origin = CASE WHEN pick = ?3 THEN pick_origin ELSE 'auto' END
             WHERE id = ?1",
        )?;
        for &id in ids {
            let row: Option<(String, u8, Option<String>, Option<u8>)> =
                read.query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
            let (pick, rating, s_pick, s_rating) = row.ok_or_else(|| AppError::not_found(format!("image {id}")))?;
            let (Some(s_pick), Some(s_rating)) = (s_pick, s_rating) else { continue };
            if only_unset && (pick != PickFlag::Unflagged.as_str() || rating != 0) {
                continue;
            }
            let take_flag = match PickFlag::parse(&s_pick) {
                Some(PickFlag::Pick) => kinds.picks,
                Some(PickFlag::Reject) => kinds.rejects,
                _ => kinds.picks && kinds.rejects,
            };
            let next_pick = if take_flag { s_pick } else { pick.clone() };
            let next_rating = if kinds.stars { s_rating } else { rating };
            if next_pick == pick && next_rating == rating {
                continue;
            }
            applied += write.execute(params![id, next_rating, next_pick])? as u32;
        }
    }
    tx.commit()?;
    Ok(ApplySuggestionsResult { applied, skipped: ids.len() as u32 - applied })
}

// ---------------------------------------------------------------------------
// Culling snapshots (undo) and UI preferences (IPC v8)
// ---------------------------------------------------------------------------

/// Current rating/pick/label of `ids`, in the given order. Unknown ids -> `not_found`.
pub fn cull_snapshot(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<CullSnapshot>> {
    let mut stmt = conn.prepare_cached("SELECT rating, pick, color_label, pick_origin FROM images WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        let snap = stmt
            .query_row([id], |r| {
                let pick = enum_col(r, 1, PickFlag::parse)?;
                Ok(CullSnapshot {
                    image_id: id,
                    rating: r.get(0)?,
                    pick,
                    color_label: opt_enum_col(r, 2, ColorLabel::parse)?,
                    pick_origin: match pick {
                        PickFlag::Unflagged => None,
                        _ => Some(PickOrigin::parse(&r.get::<_, String>(3)?).unwrap_or(PickOrigin::User)),
                    },
                })
            })
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("image {id}")))?;
        out.push(snap);
    }
    Ok(out)
}

/// Writes every snapshot back in one transaction (all or nothing). Rating > 5 ->
/// `invalid_argument`; unknown ids -> `not_found`. Changed images become `xmp_dirty`
/// through the usual triggers. Returns the ids whose values actually changed.
pub fn restore_cull_snapshot(conn: &mut Connection, snapshots: &[CullSnapshot]) -> AppResult<Vec<ImageId>> {
    let tx = conn.savepoint()?;
    let changed = write_cull_snapshot(&tx, snapshots)?;
    tx.commit()?;
    Ok(changed)
}

/// Body of [`restore_cull_snapshot`] for callers that already hold a transaction (the caller
/// commits, or rolls back on error).
pub fn write_cull_snapshot(conn: &Connection, snapshots: &[CullSnapshot]) -> AppResult<Vec<ImageId>> {
    if let Some(s) = snapshots.iter().find(|s| s.rating > 5) {
        return Err(AppError::invalid(format!("rating {} is outside 0..=5", s.rating)));
    }
    let mut changed = Vec::new();
    // `pick_origin` is restored with the flag (v16); it only matters for flagged images.
    let mut stmt = conn.prepare_cached(
        "UPDATE images SET rating = ?2, pick = ?3, color_label = ?4, pick_origin = ?5
         WHERE id = ?1 AND (rating IS NOT ?2 OR pick IS NOT ?3 OR color_label IS NOT ?4
                            OR (?3 <> 'unflagged' AND pick_origin IS NOT ?5))",
    )?;
    for s in snapshots {
        ensure_image(conn, s.image_id)?;
        let label = s.color_label.map(|l| l.as_str());
        let origin = s.pick_origin.unwrap_or(PickOrigin::User).as_str();
        if stmt.execute(params![s.image_id, s.rating, s.pick.as_str(), label, origin])? > 0
            && !changed.contains(&s.image_id)
        {
            changed.push(s.image_id);
        }
    }
    Ok(changed)
}

const UI_PREFS_KEY: &str = "ui_prefs";

/// Stored UI preferences; defaults when unset or unreadable (prefs never block the app).
pub fn ui_prefs(conn: &Connection) -> AppResult<UiPrefs> {
    let json: Option<String> =
        conn.query_row("SELECT value FROM catalog_meta WHERE key = ?1", [UI_PREFS_KEY], |r| r.get(0)).optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
}

pub fn set_ui_prefs(conn: &Connection, prefs: &UiPrefs) -> AppResult<()> {
    if prefs.last_export_folder.as_deref().is_some_and(|f| f.is_empty() || f.len() > 4096) {
        return Err(AppError::invalid("lastExportFolder must be 1..=4096 bytes"));
    }
    set_meta(conn, UI_PREFS_KEY, &serde_json::to_string(prefs)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;
    use crate::raw::test_fixtures::stub_header;

    /// Folder with one valid file per format, a nested ARW, a corrupt ARW and a sidecar.
    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("DSC0001.ARW"), stub_header(RawFormat::Arw)).unwrap();
        std::fs::write(p.join("DSCF0002.RAF"), stub_header(RawFormat::Raf)).unwrap();
        std::fs::write(p.join("IMG_0003.CR3"), stub_header(RawFormat::Cr3)).unwrap();
        std::fs::write(p.join("broken.arw"), b"garbage").unwrap();
        std::fs::write(p.join("DSC0001.xmp"), b"<x/>").unwrap();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/DSC0004.ARW"), stub_header(RawFormat::Arw)).unwrap();
        dir
    }

    fn import(conn: &mut Connection, dir: &Path, recursive: bool) -> ImportSummary {
        import_folder(conn, dir, &ImportOptions::raw_only(recursive)).unwrap()
    }

    fn all_ids(conn: &Connection) -> Vec<ImageId> {
        list_images(conn, &ImageQuery::default()).unwrap().items.iter().map(|e| e.id).collect()
    }

    /// The Rust-side sort + set-membership filters return exactly what the SQL
    /// `ORDER BY` / correlated `EXISTS` formulation did (ties, NULLs, both directions).
    #[test]
    fn rust_sort_matches_sql_order_by() {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/a', 0), (2, '/b', 0);
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < 300)
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, captured_at_ms, rating, pick,
                                 file_size, file_mtime_ms, imported_at)
             SELECT x, 1 + x % 2, '/p/' || x, 'F' || (x * 37 % 23) || CASE WHEN x % 5 = 0 THEN 'b' ELSE '' END,
                    'arw', 'sony', CASE WHEN x % 7 = 0 THEN NULL ELSE (x * 13) % 40 END, x % 6,
                    CASE x % 4 WHEN 0 THEN 'pick' WHEN 1 THEN 'reject' ELSE 'unflagged' END, 1, 0, 0 FROM s;
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < 300)
             INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                                         clipped_shadows_pct, mean_luma, model_version, analyzed_at)
             SELECT x, (x * 11 % 9) / 10.0, 0, 0, 0, 0, 'v', 0 FROM s WHERE x % 3 <> 0;
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < 300)
             INSERT INTO image_tags (image_id, tag, source, suppressed)
             SELECT x, CASE x % 3 WHEN 0 THEN 'blink' WHEN 1 THEN 'missed_focus' ELSE 'motion_blur' END, 'auto',
                    CASE WHEN x % 10 = 0 THEN 1 ELSE 0 END FROM s WHERE x % 4 <> 0;
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < 300)
             INSERT INTO image_tags (image_id, tag, source) SELECT x, 'duplicate_burst', 'auto' FROM s WHERE x % 5 < 2;
             WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < 30)
             INSERT INTO burst_groups (id, started_at_ms, ended_at_ms, keeper_image_id)
             SELECT x, 0, 0, CASE WHEN x % 3 = 0 THEN NULL ELSE x * 10 + 1 END FROM s;
             UPDATE images SET burst_group_id = id / 10 WHERE id % 10 < 4 AND id >= 10;",
        )
        .unwrap();
        let order = |sort: ImageSort, desc: bool| match (sort, desc) {
            (ImageSort::CaptureTime, false) => "i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
            (ImageSort::CaptureTime, true) => {
                "i.captured_at_ms IS NULL, i.captured_at_ms DESC, i.file_name DESC, i.id DESC"
            }
            (ImageSort::FileName, false) => "i.file_name, i.id",
            (ImageSort::FileName, true) => "i.file_name DESC, i.id DESC",
            (ImageSort::Quality, false) => "q.overall IS NULL, q.overall DESC, i.id",
            (ImageSort::Quality, true) => "q.overall IS NULL, q.overall, i.id DESC",
            (ImageSort::Rating, false) => {
                "i.rating DESC, i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id"
            }
            (ImageSort::Rating, true) => "i.rating, i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
            (ImageSort::TargetMoment, _) => unreachable!("not compared here"),
        };
        // The pre-Phase-8 filter formulation.
        let legacy_where = |q: &ImageQuery| {
            let tags = |t: &[CullTag]| t.iter().map(|t| format!("'{}'", t.as_str())).collect::<Vec<_>>().join(",");
            let mut c = Vec::new();
            if !q.include_tags.is_empty() {
                c.push(match q.tag_match {
                    TagMatch::Any => format!(
                        "EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id AND it.suppressed = 0
                                 AND it.tag IN ({}))",
                        tags(&q.include_tags)
                    ),
                    TagMatch::All => format!(
                        "(SELECT COUNT(DISTINCT it.tag) FROM image_tags it WHERE it.image_id = i.id
                          AND it.suppressed = 0 AND it.tag IN ({})) = {}",
                        tags(&q.include_tags),
                        q.include_tags.len()
                    ),
                });
            }
            if !q.exclude_tags.is_empty() {
                c.push(format!(
                    "NOT EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id AND it.suppressed = 0
                                 AND it.tag IN ({}))",
                    tags(&q.exclude_tags)
                ));
            }
            if q.collapse_bursts {
                c.push(
                    "(i.burst_group_id IS NULL OR NOT EXISTS (SELECT 1 FROM burst_groups b WHERE b.id = i.burst_group_id
                      AND b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id))"
                        .to_owned(),
                );
            }
            if let Some(f) = q.folder_id {
                c.push(format!("i.folder_id = {f}"));
            }
            if c.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", c.join(" AND "))
            }
        };
        let filters = [
            ImageQuery::default(),
            ImageQuery { include_tags: vec![CullTag::Blink, CullTag::MotionBlur], ..Default::default() },
            ImageQuery {
                include_tags: vec![CullTag::Blink, CullTag::DuplicateBurst],
                tag_match: TagMatch::All,
                ..Default::default()
            },
            ImageQuery { exclude_tags: vec![CullTag::MissedFocus], folder_id: Some(2), ..Default::default() },
            ImageQuery { collapse_bursts: true, ..Default::default() },
        ];
        for f in &filters {
            for sort in [ImageSort::CaptureTime, ImageSort::FileName, ImageSort::Quality, ImageSort::Rating] {
                for desc in [false, true] {
                    let q = ImageQuery { sort, sort_descending: desc, ..f.clone() };
                    let sql = format!(
                        "SELECT i.id FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id{} ORDER BY {}",
                        legacy_where(&q),
                        order(sort, desc)
                    );
                    let want: Vec<ImageId> =
                        conn.prepare(&sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
                    assert!(!want.is_empty());
                    assert_eq!(list_image_ids(&conn, &q).unwrap(), want, "{sort:?} desc={desc} {f:?}");
                    let page = list_images(&conn, &ImageQuery { offset: 7, limit: 50, ..q.clone() }).unwrap();
                    assert_eq!(page.total as usize, want.len());
                    let got: Vec<ImageId> = page.items.iter().map(|e| e.id).collect();
                    assert_eq!(got, want.iter().skip(7).take(50).copied().collect::<Vec<_>>());
                }
            }
        }
        // Facet counts: whole catalog = sum over folders.
        let (all, a, b) = (
            filter_counts(&conn, None).unwrap(),
            filter_counts(&conn, Some(1)).unwrap(),
            filter_counts(&conn, Some(2)).unwrap(),
        );
        assert_eq!(all.total, 300);
        assert_eq!(a.total + b.total, 300);
        assert_eq!(a.picked + b.picked, all.picked);
        let tag_sum = |c: &FilterCounts| c.tags.iter().map(|t| t.count).sum::<u32>();
        assert_eq!(tag_sum(&a) + tag_sum(&b), tag_sum(&all));
        assert_eq!(a.burst_non_keepers + b.burst_non_keepers, all.burst_non_keepers);
    }

    #[test]
    fn import_then_list_round_trip() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();

        let s = import(&mut conn, dir.path(), false);
        assert_eq!((s.added, s.skipped, s.invalid), (3, 0, 1));

        let s = import(&mut conn, dir.path(), true);
        assert_eq!((s.added, s.skipped, s.invalid), (1, 3, 1));

        let page = list_images(&conn, &ImageQuery { sort: ImageSort::FileName, ..Default::default() }).unwrap();
        assert_eq!(page.total, 4);
        let names: Vec<_> = page.items.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, ["DSC0001.ARW", "DSC0004.ARW", "DSCF0002.RAF", "IMG_0003.CR3"]);

        let raf = &page.items[2];
        assert_eq!(raf.format, RawFormat::Raf);
        assert_eq!(raf.camera.make, CameraMake::Fujifilm);
        assert_eq!(raf.camera.sensor_layout, SensorLayout::Unknown);
        assert_eq!(raf.thumbnail, ThumbnailState::Pending);
        assert_eq!(raf.pick, PickFlag::Unflagged);
        assert!(!raf.has_edits);

        assert_eq!(get_image(&conn, raf.id).unwrap(), *raf);

        let state = catalog_state(&conn, ":memory:", "").unwrap();
        assert_eq!(state.image_count, 4);
        assert_eq!(state.folders.len(), 1);
        assert_eq!(state.folders[0].image_count, 4);
        assert_eq!(state.shoot_type, ShootType::General);
        assert_eq!(state.burst_window_ms, 1500);
    }

    /// IPC v13: missing flags from re-import / access failures / extraction, the `missing`
    /// facet and filter, and `relocate_folder`.
    #[test]
    fn missing_originals_are_flagged_filtered_and_relocated() {
        let mut conn = open_in_memory();
        let root = tempfile::tempdir().unwrap();
        let shoot = root.path().join("shoot");
        std::fs::create_dir(&shoot).unwrap();
        for (name, f) in [("DSC0001.ARW", RawFormat::Arw), ("DSCF0002.RAF", RawFormat::Raf)] {
            std::fs::write(shoot.join(name), stub_header(f)).unwrap();
        }
        std::fs::create_dir(shoot.join("sub")).unwrap();
        std::fs::write(shoot.join("sub/DSC0004.ARW"), stub_header(RawFormat::Arw)).unwrap();
        let folder = import(&mut conn, &shoot, true).folder_id;
        let by_name = |conn: &Connection, name: &str| -> RawImageEntry {
            let id: ImageId =
                conn.query_row("SELECT id FROM images WHERE file_name = ?1", [name], |r| r.get(0)).unwrap();
            get_image(conn, id).unwrap()
        };
        let missing_q = ImageQuery { missing_only: true, ..Default::default() };
        assert!(by_name(&conn, "DSC0001.ARW").missing_since_ms.is_none());
        assert_eq!(filter_counts(&conn, None).unwrap().missing, 0);

        // Re-import flags a deleted file and clears it once it is back.
        let a = shoot.join("DSC0001.ARW");
        std::fs::rename(&a, root.path().join("aside.ARW")).unwrap();
        import(&mut conn, &shoot, true);
        let gone = by_name(&conn, "DSC0001.ARW");
        let since = gone.missing_since_ms.expect("flagged by re-import");
        assert_eq!(list_image_ids(&conn, &missing_q).unwrap(), [gone.id]);
        assert_eq!(filter_counts(&conn, None).unwrap().missing, 1);
        assert_eq!(filter_counts(&conn, Some(folder)).unwrap().missing, 1);
        assert_eq!(filter_counts(&conn, Some(folder + 1)).unwrap().missing, 0);
        // A later failure keeps the first time; other failures change nothing.
        note_access_failure(&conn, gone.id, &raw::access::missing_message(&a)).unwrap();
        note_access_failure(&conn, gone.id, "Could not decode x").unwrap();
        assert_eq!(by_name(&conn, "DSC0001.ARW").missing_since_ms, Some(since));
        std::fs::rename(root.path().join("aside.ARW"), &a).unwrap();
        import(&mut conn, &shoot, true);
        assert!(list_image_ids(&conn, &missing_q).unwrap().is_empty());

        // Extraction failures flag it, success clears it.
        let raf = by_name(&conn, "DSCF0002.RAF").id;
        let reason = raw::access::missing_message(&shoot.join("DSCF0002.RAF"));
        record_extraction(&mut conn, raf, None, Err(&reason)).unwrap();
        assert!(get_image(&conn, raf).unwrap().missing_since_ms.is_some());

        // The shoot moves; sub/DSC0004.ARW lands flat in the new folder (found by name).
        let moved = root.path().join("moved");
        std::fs::rename(&shoot, &moved).unwrap();
        std::fs::rename(moved.join("sub/DSC0004.ARW"), moved.join("DSC0004.ARW")).unwrap();
        std::fs::remove_file(moved.join("DSCF0002.RAF")).unwrap();
        let empty = root.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let e = relocate_folder(&mut conn, folder, &empty).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidArgument);
        assert!(e.message.starts_with("None of the 3 photos"), "{}", e.message);
        assert_eq!(relocate_folder(&mut conn, 999, &moved).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(
            relocate_folder(&mut conn, folder, &root.path().join("nope")).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );

        let r = relocate_folder(&mut conn, folder, &moved).unwrap();
        assert_eq!(r.result, RelocateResult { matched: 2, still_missing: 1 });
        assert_eq!(r.moved.len(), 2);
        assert!(r.retry_thumbnails.is_empty());
        let moved_c = moved.canonicalize().unwrap();
        assert_eq!(catalog_state(&conn, ":memory:", "").unwrap().folders[0].path, moved_c.to_str().unwrap());
        assert_eq!(by_name(&conn, "DSC0004.ARW").path, moved_c.join("DSC0004.ARW").to_str().unwrap());
        assert_eq!(by_name(&conn, "DSC0001.ARW").path, moved_c.join("DSC0001.ARW").to_str().unwrap());
        assert_eq!(list_image_ids(&conn, &missing_q).unwrap(), [raf]);

        // The RAF turns up: relocating again finds it and queues its failed thumbnail.
        std::fs::write(moved.join("DSCF0002.RAF"), stub_header(RawFormat::Raf)).unwrap();
        let r = relocate_folder(&mut conn, folder, &moved).unwrap();
        assert_eq!(r.result, RelocateResult { matched: 3, still_missing: 0 });
        assert_eq!(r.retry_thumbnails, [raf]);
        assert!(list_image_ids(&conn, &missing_q).unwrap().is_empty());
        assert_eq!(filter_counts(&conn, None).unwrap().missing, 0);
    }

    /// RAW + camera JPEG siblings, lone non-RAW files, a bad JPEG.
    fn mixed_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("DSCF0010.RAF"), stub_header(RawFormat::Raf)).unwrap();
        std::fs::write(p.join("DSCF0010.JPG"), stub_header(RawFormat::Jpeg)).unwrap();
        std::fs::write(p.join("dscf0010.hif"), stub_header(RawFormat::Heic)).unwrap();
        std::fs::write(p.join("IMG_0020.CR3"), stub_header(RawFormat::Cr3)).unwrap();
        std::fs::write(p.join("IMG_0021.jpeg"), stub_header(RawFormat::Jpeg)).unwrap();
        std::fs::write(p.join("scan.tif"), stub_header(RawFormat::Tiff)).unwrap();
        std::fs::write(p.join("IMG_0020.png"), stub_header(RawFormat::Png)).unwrap();
        std::fs::write(p.join("broken.jpg"), b"not a jpeg").unwrap();
        dir
    }

    #[test]
    fn non_raw_import_and_companion_pairing() {
        let dir = mixed_dir();
        let opts = |include: bool, pair: bool| ImportOptions {
            recursive: false,
            include_non_raw: include,
            pair_jpeg_with_raw: pair,
        };
        let names = |conn: &Connection| -> Vec<(String, ImageFormat, Option<String>)> {
            let q = ImageQuery { sort: ImageSort::FileName, ..Default::default() };
            list_images(conn, &q)
                .unwrap()
                .items
                .into_iter()
                .map(|e| {
                    let companion =
                        e.companion_path.map(|c| Path::new(&c).file_name().unwrap().to_string_lossy().into_owned());
                    (e.file_name, e.format, companion)
                })
                .collect()
        };

        // Default (RAW only): the pre-v9 behaviour.
        let mut conn = open_in_memory();
        let s = import_folder(&mut conn, dir.path(), &ImportOptions::raw_only(false)).unwrap();
        assert_eq!((s.added, s.companions, s.invalid), (2, 0, 0));

        // Non-RAW + pairing: the JPEG (preferred over HEIC) becomes the RAF's companion; the
        // HEIC sibling is skipped; TIFF/PNG never pair; the bad JPEG is invalid.
        let mut conn = open_in_memory();
        let s = import_folder(&mut conn, dir.path(), &opts(true, true)).unwrap();
        assert_eq!((s.added, s.companions, s.skipped, s.invalid), (5, 1, 1, 1), "{s:?}");
        assert_eq!(
            names(&conn),
            [
                ("DSCF0010.RAF".to_owned(), ImageFormat::Raf, Some("DSCF0010.JPG".to_owned())),
                ("IMG_0020.CR3".to_owned(), ImageFormat::Cr3, None),
                ("IMG_0020.png".to_owned(), ImageFormat::Png, None),
                ("IMG_0021.jpeg".to_owned(), ImageFormat::Jpeg, None),
                ("scan.tif".to_owned(), ImageFormat::Tiff, None),
            ]
        );
        let jpeg = list_images(&conn, &ImageQuery::default())
            .unwrap()
            .items
            .into_iter()
            .find(|e| e.format == ImageFormat::Jpeg)
            .unwrap();
        assert_eq!((jpeg.camera.make, jpeg.camera.sensor_layout), (CameraMake::Other, SensorLayout::Unknown));
        // Neutral defaults depend on the format.
        assert_eq!(get_adjustments(&conn, jpeg.id).unwrap(), ParametricAdjustments::defaults_for(ImageFormat::Jpeg));
        save_adjustments(&conn, jpeg.id, &ParametricAdjustments::defaults_for(ImageFormat::Jpeg)).unwrap();
        assert!(!get_image(&conn, jpeg.id).unwrap().has_edits, "non-RAW defaults are neutral for a JPEG");
        save_adjustments(&conn, jpeg.id, &ParametricAdjustments::default()).unwrap();
        assert!(get_image(&conn, jpeg.id).unwrap().has_edits, "RAW sharpening on a JPEG is an edit");
        // Re-import is idempotent.
        let s = import_folder(&mut conn, dir.path(), &opts(true, true)).unwrap();
        assert_eq!((s.added, s.companions), (0, 0));

        // Pairing off: every sibling is its own image.
        let mut conn = open_in_memory();
        let s = import_folder(&mut conn, dir.path(), &opts(true, false)).unwrap();
        assert_eq!((s.added, s.companions, s.invalid), (7, 0, 1));
        assert!(names(&conn).iter().all(|(_, _, c)| c.is_none()));
    }

    #[test]
    fn develop_warnings_column_round_trips() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), false);
        let id = all_ids(&conn)[0];
        assert!(get_image(&conn, id).unwrap().develop_warnings.is_empty());
        let w = vec![DevelopWarning { code: DevelopWarningCode::MasksUnsupported, detail: Some("2".into()) }];
        // What `xmp::store::set_develop_warnings` writes.
        conn.execute(
            "UPDATE images SET develop_warnings = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(&w).unwrap()],
        )
        .unwrap();
        let e = get_image(&conn, id).unwrap();
        assert_eq!(e.develop_warnings, w);
        assert!(!e.xmp.dirty, "warnings are not XMP-mapped");
        conn.execute("UPDATE images SET develop_warnings = NULL WHERE id = ?1", [id]).unwrap();
        assert!(get_image(&conn, id).unwrap().develop_warnings.is_empty());
    }

    #[test]
    fn pagination() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), true);
        let page = list_images(&conn, &ImageQuery { offset: 1, limit: 2, ..Default::default() }).unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(page.items.len(), 2);
    }

    #[test]
    fn tag_filters() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), true);
        // Ascending ids: `query` returns sorted ids (import order is by path since v9).
        let mut ids = all_ids(&conn);
        ids.sort();
        let (a, b, c) = (ids[0], ids[1], ids[2]);

        // a: blink + motion_blur, b: blink, c: auto motion_blur later suppressed.
        set_user_tag(&mut conn, &[a, b], CullTag::Blink, true).unwrap();
        set_user_tag(&mut conn, &[a], CullTag::MotionBlur, true).unwrap();
        conn.execute(
            "INSERT INTO image_tags (image_id, tag, source, confidence) VALUES (?1, 'motion_blur', 'auto', 0.8)",
            [c],
        )
        .unwrap();

        let query = |conn: &Connection, include: &[CullTag], exclude: &[CullTag], m: TagMatch| {
            let mut ids: Vec<_> = list_images(
                conn,
                &ImageQuery {
                    include_tags: include.to_vec(),
                    exclude_tags: exclude.to_vec(),
                    tag_match: m,
                    ..Default::default()
                },
            )
            .unwrap()
            .items
            .iter()
            .map(|e| e.id)
            .collect();
            ids.sort();
            ids
        };

        use CullTag::*;
        assert_eq!(query(&conn, &[MotionBlur], &[], TagMatch::Any), [a, c]);
        assert_eq!(query(&conn, &[Blink, MotionBlur], &[], TagMatch::All), [a]);
        assert_eq!(query(&conn, &[Blink, MotionBlur], &[], TagMatch::Any), [a, b, c]);
        assert_eq!(query(&conn, &[], &[Blink], TagMatch::Any).len(), 2);

        // User dismisses the auto tag on c: suppressed, no longer matches, row kept.
        set_user_tag(&mut conn, &[c], MotionBlur, false).unwrap();
        assert_eq!(query(&conn, &[MotionBlur], &[], TagMatch::Any), [a]);
        let tags = get_image(&conn, c).unwrap().tags;
        assert_eq!(tags.len(), 1);
        assert!(tags[0].suppressed && tags[0].source == TagSource::Auto);

        // Removing a user tag deletes it.
        set_user_tag(&mut conn, &[b], Blink, false).unwrap();
        assert!(get_image(&conn, b).unwrap().tags.is_empty());

        let counts = catalog_state(&conn, "", "").unwrap().tag_counts;
        assert_eq!(counts, [TagCount { tag: Blink, count: 1 }, TagCount { tag: MotionBlur, count: 1 }]);
    }

    #[test]
    fn rating_pick_label() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), true);
        let ids = all_ids(&conn);

        set_rating(&mut conn, &ids[..2], 4).unwrap();
        set_pick(&mut conn, &ids[..1], PickFlag::Reject).unwrap();
        set_color_label(&mut conn, &ids[..1], Some(ColorLabel::Red)).unwrap();

        let e = get_image(&conn, ids[0]).unwrap();
        assert_eq!((e.rating, e.pick, e.color_label), (4, PickFlag::Reject, Some(ColorLabel::Red)));

        let q = ImageQuery { min_rating: Some(4), picks: vec![PickFlag::Reject], ..Default::default() };
        assert_eq!(list_images(&conn, &q).unwrap().total, 1);

        assert_eq!(set_rating(&mut conn, &ids, 6).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(set_rating(&mut conn, &[9999], 1).unwrap_err().kind, ErrorKind::NotFound);
        // Failed batch is rolled back.
        assert!(set_pick(&mut conn, &[ids[1], 9999], PickFlag::Pick).is_err());
        assert_eq!(get_image(&conn, ids[1]).unwrap().pick, PickFlag::Unflagged);
    }

    #[test]
    fn adjustments_round_trip() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), false);
        let id = all_ids(&conn)[0];

        assert_eq!(get_adjustments(&conn, id).unwrap(), ParametricAdjustments::default());

        let mut adj = ParametricAdjustments {
            exposure: 0.7,
            shadows: 35.0,
            white_balance: WhiteBalance::Custom { temperature_k: 5600.0, tint: 8.0 },
            lut: Some(LutRef { id: "film".into(), amount: 60.0 }),
            ..Default::default()
        };
        adj.hsl.saturation.orange = -20.0;
        save_adjustments(&conn, id, &adj).unwrap();
        assert_eq!(get_adjustments(&conn, id).unwrap(), adj);
        assert!(get_image(&conn, id).unwrap().has_edits);

        let bad = ParametricAdjustments { exposure: 9.0, ..Default::default() };
        assert_eq!(save_adjustments(&conn, id, &bad).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(get_adjustments(&conn, 9999).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(save_adjustments(&conn, 9999, &adj).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn adjustments_dirty_xmp_and_neutral_rows_are_not_edits() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), false);
        let id = all_ids(&conn)[0];
        let dirty = |conn: &Connection| -> bool {
            conn.query_row("SELECT xmp_dirty FROM images WHERE id = ?1", [id], |r| r.get(0)).unwrap()
        };
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();

        let adj = ParametricAdjustments { exposure: 0.3, ..Default::default() };
        save_adjustments(&conn, id, &adj).unwrap();
        assert!(dirty(&conn), "insert dirties the sidecar");
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();
        save_adjustments(&conn, id, &adj).unwrap();
        assert!(!dirty(&conn), "unchanged values do not");

        save_adjustments(&conn, id, &ParametricAdjustments::default()).unwrap();
        assert!(dirty(&conn), "reset to neutral does");
        assert!(!get_image(&conn, id).unwrap().has_edits, "neutral row is not an edit");
    }

    #[test]
    fn adjustments_json_is_forward_compatible() {
        // Older stored JSON missing newer sliders must load with neutral values.
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), false);
        let id = all_ids(&conn)[0];
        conn.execute(
            "INSERT INTO adjustments (image_id, params_json, process_version, updated_at)
             VALUES (?1, '{\"exposure\": 1.5, \"hsl\": {\"hue\": {\"red\": 10}}}', 1, 0)",
            [id],
        )
        .unwrap();
        let mut expected = ParametricAdjustments { exposure: 1.5, ..Default::default() };
        expected.hsl.hue.red = 10.0;
        assert_eq!(get_adjustments(&conn, id).unwrap(), expected);
    }

    #[test]
    fn ingest_records_and_resets() {
        let mut conn = open_in_memory();
        let dir = fixture_dir();
        import(&mut conn, dir.path(), true);
        let pending = pending_thumbnails(&conn, 10).unwrap();
        assert_eq!(pending.len(), 4);
        assert_eq!(count_pending(&conn).unwrap(), 4);
        assert_eq!(pending_thumbnails(&conn, 2).unwrap(), pending[..2]);
        let (a, b) = (pending[0].id, pending[1].id);

        let meta = raw::meta::ImageMeta {
            make: Some(CameraMake::Fujifilm),
            model: Some("X-T5".into()),
            sensor_layout: Some(SensorLayout::XTrans),
            captured_at_ms: Some(1_790_447_764_106),
            iso: Some(125),
            shutter_seconds: Some(0.005),
            aperture: Some(1.4),
            focal_length_mm: Some(85.0),
            lens: Some("85mm".into()),
            width: Some(4608),
            height: Some(3072),
            orientation: Some(8),
            serial: Some("61000657".into()),
        };
        let files = ThumbFiles {
            thumb_path: "/c/thumbs/1_512.jpg".into(),
            preview_path: Some("/c/thumbs/1_2048.jpg".into()),
            width: 341,
            height: 512,
        };
        record_extraction(&mut conn, a, Some(&meta), Ok(&files)).unwrap();
        record_extraction(&mut conn, b, None, Err("no embedded JPEG")).unwrap();
        let stored: f64 = conn.query_row("SELECT aperture FROM images WHERE id = ?1", [a], |r| r.get(0)).unwrap();
        assert_eq!(stored, 1.4, "f32 widening noise is rounded away");

        let e = get_image(&conn, a).unwrap();
        assert_eq!(
            e.camera,
            CameraInfo {
                make: CameraMake::Fujifilm,
                model: Some("X-T5".into()),
                sensor_layout: SensorLayout::XTrans,
                serial: Some("61000657".into()),
            }
        );
        assert_eq!(e.capture.captured_at_ms, Some(1_790_447_764_106));
        assert_eq!((e.capture.iso, e.capture.shutter_seconds, e.capture.aperture), (Some(125), Some(0.005), Some(1.4)));
        assert_eq!((e.width, e.height, e.orientation), (Some(4608), Some(3072), Some(8)));
        assert_eq!(
            e.thumbnail,
            ThumbnailState::Ready {
                path: files.thumb_path.clone(),
                preview_path: files.preview_path.clone(),
                width: 341,
                height: 512
            }
        );
        let fb = get_image(&conn, b).unwrap();
        assert_eq!(fb.thumbnail, ThumbnailState::Failed { reason: "no embedded JPEG".into() });
        assert_eq!(fb.camera.make, raw::default_make(pending[1].format), "no meta: defaults kept");
        assert_eq!(count_pending(&conn).unwrap(), 2);

        // Reset returns old files, is atomic, and puts rows back in the queue.
        assert_eq!(reset_thumbnails(&mut conn, &[a, 9999]).unwrap_err().kind, ErrorKind::NotFound);
        assert!(matches!(get_image(&conn, a).unwrap().thumbnail, ThumbnailState::Ready { .. }));
        let old = reset_thumbnails(&mut conn, &[a, b]).unwrap();
        assert_eq!(old, ["/c/thumbs/1_512.jpg", "/c/thumbs/1_2048.jpg"]);
        assert_eq!(get_image(&conn, b).unwrap().thumbnail, ThumbnailState::Pending);
        assert_eq!(count_pending(&conn).unwrap(), 4);
    }

    #[test]
    fn catalog_settings() {
        let conn = open_in_memory();
        set_shoot_type(&conn, ShootType::Wedding).unwrap();
        set_burst_window(&conn, 800).unwrap();
        assert!(set_burst_window(&conn, 5).is_err());
        let s = catalog_state(&conn, "", "").unwrap();
        assert_eq!((s.shoot_type, s.burst_window_ms), (ShootType::Wedding, 800));
    }

    #[test]
    fn analysis_settings_and_thresholds() {
        let conn = open_in_memory();
        assert!(catalog_state(&conn, "", "").unwrap().auto_analyze);
        set_auto_analyze(&conn, false).unwrap();
        assert!(!catalog_state(&conn, "", "").unwrap().auto_analyze);

        let defaults = crate::ml::thresholds::default_thresholds(ShootType::Wedding);
        assert_eq!(cull_thresholds(&conn, ShootType::Wedding).unwrap(), defaults);
        let custom = CullThresholds { blink_ear: 0.2, ..defaults.clone() };
        set_cull_thresholds(&conn, ShootType::Wedding, Some(&custom)).unwrap();
        assert_eq!(cull_thresholds(&conn, ShootType::Wedding).unwrap(), custom);
        assert_ne!(cull_thresholds(&conn, ShootType::Sports).unwrap(), custom);
        let bad = CullThresholds { reject_max_overall: 0.9, pick_min_overall: 0.5, ..defaults.clone() };
        assert_eq!(
            set_cull_thresholds(&conn, ShootType::Wedding, Some(&bad)).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        set_cull_thresholds(&conn, ShootType::Wedding, None).unwrap();
        assert_eq!(cull_thresholds(&conn, ShootType::Wedding).unwrap(), defaults);
    }

    #[test]
    fn analysis_reads_and_suggestions() {
        let mut conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0), (2, '/g', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                 imported_at, captured_at_ms, rating, pick)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0, 1000, 2, 'unflagged'),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0, 1500, 0, 'unflagged'),
                    (3, 2, '/g/c.arw', 'c.arw', 'arw', 'sony', 1, 0, 0, 9000, 1, 'pick');
             INSERT INTO burst_groups (id, started_at_ms, ended_at_ms, keeper_image_id) VALUES (7, 1000, 1500, 2);
             UPDATE images SET burst_group_id = 7 WHERE id IN (1, 2);
             INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                                         clipped_shadows_pct, mean_luma, model_version, analyzed_at,
                                         suggested_rating, suggested_pick)
             VALUES (1, 0.2, 0.3, 0, 0, 0.5, 'm', 0, 1, 'reject'),
                    (2, 0.9, 0.8, 0, 0, 0.5, 'm', 0, 4, 'pick');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, phash, faces_json)
             VALUES (2, 'done', 'm', 0, ?1, ?2)",
            params![
                u64::MAX as i64,
                r#"[{"bbox":{"x":0.1,"y":0.2,"width":0.3,"height":0.4},"leftEye":{"x":0.2,"y":0.3},
                    "rightEye":{"x":0.3,"y":0.3},"detectionScore":0.9,"ear":0.25,"eyesOpen":1.0,
                    "sharpness":0.8,"blink":false,"inFocus":true,"primary":true,"considered":true}]"#
            ],
        )
        .unwrap();

        let b = get_image(&conn, 2).unwrap();
        assert!(b.is_burst_keeper);
        let q = b.quality.unwrap();
        assert_eq!((q.suggested_rating, q.suggested_pick), (4, PickFlag::Pick));
        assert!(!get_image(&conn, 1).unwrap().is_burst_keeper);

        let faces = get_faces(&conn, 2).unwrap();
        assert_eq!(faces.len(), 1);
        assert!(faces[0].primary && faces[0].ear == Some(0.25));
        assert!(get_faces(&conn, 1).unwrap().is_empty());
        assert_eq!(get_faces(&conn, 99).unwrap_err().kind, ErrorKind::NotFound);

        let groups = list_burst_groups(&conn, None).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].keeper_image_id, groups[0].image_ids.clone()), (Some(2), vec![1, 2]));
        assert_eq!(list_burst_groups(&conn, Some(1)).unwrap().len(), 1);
        assert!(list_burst_groups(&conn, Some(2)).unwrap().is_empty());

        assert_eq!(apply_suggestions(&mut conn, &[1, 99], false).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(get_image(&conn, 1).unwrap().rating, 2);
        // onlyUnset: 1 is rated (2), 3 is unanalyzed; only 2 (unflagged, 0 stars) is applied.
        let r = apply_suggestions(&mut conn, &[1, 2, 3], true).unwrap();
        assert_eq!(r, ApplySuggestionsResult { applied: 1, skipped: 2 });
        assert_eq!(get_image(&conn, 1).unwrap().rating, 2);
        assert_eq!(get_image(&conn, 2).unwrap().pick, PickFlag::Pick);
        // 2 already matches its suggestion and 3 is unanalyzed (v18.1: only changes count).
        let r = apply_suggestions(&mut conn, &[1, 2, 3], false).unwrap();
        assert_eq!(r, ApplySuggestionsResult { applied: 1, skipped: 2 });
        let (a, b, c) = (get_image(&conn, 1).unwrap(), get_image(&conn, 2).unwrap(), get_image(&conn, 3).unwrap());
        assert_eq!((a.rating, a.pick), (1, PickFlag::Reject));
        assert_eq!((b.rating, b.pick), (4, PickFlag::Pick));
        assert_eq!((c.rating, c.pick), (1, PickFlag::Pick));
    }

    /// Three images in folder 1 (burst 7 = {1, 2}, keeper 2) and one in folder 2.
    fn phase4_fixture() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0), (2, '/g', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms,
                                 imported_at, captured_at_ms, rating, pick, color_label)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0, 1000, 2, 'unflagged', 'red'),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0, 1500, 5, 'pick', NULL),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 1, 0, 0, 9000, 0, 'reject', 'blue'),
                    (4, 2, '/g/d.arw', 'd.arw', 'arw', 'sony', 1, 0, 0, NULL, 2, 'unflagged', NULL);
             INSERT INTO burst_groups (id, started_at_ms, ended_at_ms, keeper_image_id) VALUES (7, 1000, 1500, 2);
             UPDATE images SET burst_group_id = 7 WHERE id IN (1, 2);
             INSERT INTO image_tags (image_id, tag, source, confidence, suppressed) VALUES
                 (1, 'duplicate_burst', 'auto', 1.0, 0), (3, 'blink', 'auto', 0.9, 0), (3, 'motion_blur', 'auto', 0.5, 1);
             UPDATE images SET xmp_dirty = 0, meta_updated_at = NULL;",
        )
        .unwrap();
        conn
    }

    #[test]
    fn keepers_only_query_counts_and_xmp_failures() {
        let conn = phase4_fixture();
        let keepers = ImageQuery { keepers_only: true, ..Default::default() };
        // Default rule (1 star keeps): 1 (2 stars), 2 (picked), 4 (2 stars); 3 is rejected.
        assert_eq!(ids_for(&conn, keepers.clone()), [1, 2, 4]);
        assert_eq!(ids_for(&conn, ImageQuery { folder_id: Some(1), ..keepers.clone() }), [1, 2]);
        let c = filter_counts_keepers(&conn, FolderScope::from(Some(1))).unwrap();
        assert_eq!((c.total, c.picked, c.rejected, c.unflagged), (2, 1, 0, 1));
        assert_eq!(c.ratings, vec![0, 0, 1, 0, 0, 1]);
        assert_eq!(c.burst_groups, 1);
        assert!(c.tags.iter().all(|t| t.tag != CullTag::Blink), "the rejected frame's tags are not counted");
        assert_eq!(filter_counts_keepers(&conn, FolderScope::all()).unwrap().total, 3);
        // Suggestions keep untouched frames; a stricter rule drops the 2-star ones.
        conn.execute("UPDATE images SET rating = 0 WHERE id = 4", []).unwrap();
        conn.execute(
            "INSERT INTO quality_scores (image_id, overall, global_sharpness, face_count, clipped_highlights_pct,
                                         clipped_shadows_pct, mean_luma, model_version, analyzed_at,
                                         suggested_rating, suggested_pick)
             VALUES (4, 0.9, 0.5, 0, 0, 0, 0.5, 'test', 1, 3, 'pick')",
            [],
        )
        .unwrap();
        assert_eq!(ids_for(&conn, keepers.clone()), [1, 2, 4]);
        set_keeper_rule(&conn, &KeeperRule::picks_and_ratings(3, false)).unwrap();
        assert_eq!(ids_for(&conn, keepers.clone()), [2]);
        assert_eq!(filter_counts_keepers(&conn, FolderScope::all()).unwrap().total, 1);
        for id in 1..=4 {
            let e = get_image(&conn, id).unwrap();
            let rule = keeper_rule(&conn).unwrap();
            assert_eq!(rule.is_keeper(&e), ids_for(&conn, keepers.clone()).contains(&id), "SQL mirrors the rule");
        }

        conn.execute("UPDATE images SET xmp_error = 'read-only volume' WHERE id IN (4, 2)", []).unwrap();
        let f = xmp_failures(&conn, &FolderScope::all()).unwrap();
        assert_eq!(f.iter().map(|x| x.image_id).collect::<Vec<_>>(), vec![2, 4], "capture order, undated last");
        assert_eq!(f[0].reason, "read-only volume");
        assert_eq!(xmp_failures(&conn, &FolderScope::from(Some(1))).unwrap().len(), 1);
    }

    fn ids_for(conn: &Connection, q: ImageQuery) -> Vec<ImageId> {
        let ids = list_image_ids(conn, &q).unwrap();
        let page: Vec<_> = list_images(conn, &q).unwrap().items.iter().map(|e| e.id).collect();
        assert_eq!(ids, page, "list_image_ids and list_images agree on order");
        ids
    }

    #[test]
    fn phase4_filters_and_sorts() {
        let conn = phase4_fixture();
        let q = ImageQuery::default;
        assert_eq!(ids_for(&conn, q()), [1, 2, 3, 4], "capture time, missing last");
        assert_eq!(ids_for(&conn, ImageQuery { sort_descending: true, ..q() }), [3, 2, 1, 4]);
        assert_eq!(ids_for(&conn, ImageQuery { sort: ImageSort::Rating, ..q() }), [2, 1, 4, 3]);
        assert_eq!(ids_for(&conn, ImageQuery { sort: ImageSort::Rating, sort_descending: true, ..q() }), [3, 1, 4, 2]);
        assert_eq!(
            ids_for(&conn, ImageQuery { sort: ImageSort::FileName, sort_descending: true, ..q() }),
            [4, 3, 2, 1]
        );
        assert_eq!(ids_for(&conn, ImageQuery { picks: vec![PickFlag::Pick, PickFlag::Unflagged], ..q() }), [1, 2, 4]);
        assert_eq!(ids_for(&conn, ImageQuery { min_rating: Some(1), max_rating: Some(2), ..q() }), [1, 4]);
        assert_eq!(ids_for(&conn, ImageQuery { max_rating: Some(0), ..q() }), [3]);
        assert_eq!(ids_for(&conn, ImageQuery { color_labels: vec![ColorLabel::Red, ColorLabel::Blue], ..q() }), [1, 3]);
        assert_eq!(ids_for(&conn, ImageQuery { collapse_bursts: true, ..q() }), [2, 3, 4]);
        assert_eq!(ids_for(&conn, ImageQuery { folder_id: Some(1), collapse_bursts: true, ..q() }), [2, 3]);
        assert_eq!(ids_for(&conn, ImageQuery { include_tags: vec![CullTag::MotionBlur], ..q() }), Vec::<i64>::new());
        assert_eq!(
            list_images(&conn, &ImageQuery { min_rating: Some(6), ..q() }).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        // limit/offset do not apply to ids.
        assert_eq!(list_image_ids(&conn, &ImageQuery { offset: 3, limit: 1, ..q() }).unwrap().len(), 4);
    }

    #[test]
    fn phase4_get_images_and_counts() {
        let conn = phase4_fixture();
        let got: Vec<_> = get_images(&conn, &[3, 1]).unwrap();
        assert_eq!(got.iter().map(|e| e.id).collect::<Vec<_>>(), [3, 1]);
        assert_eq!(got[0].tags.len(), 2);
        assert_eq!(got[0], get_image(&conn, 3).unwrap());
        assert!(get_images(&conn, &[]).unwrap().is_empty());
        assert_eq!(get_images(&conn, &[1, 99]).unwrap_err().kind, ErrorKind::NotFound);

        let all = filter_counts(&conn, None).unwrap();
        assert_eq!(
            all,
            FilterCounts {
                total: 4,
                tags: vec![
                    TagCount { tag: CullTag::Blink, count: 1 },
                    TagCount { tag: CullTag::DuplicateBurst, count: 1 }
                ],
                picked: 1,
                rejected: 1,
                unflagged: 2,
                ratings: vec![1, 0, 2, 0, 0, 1],
                burst_groups: 1,
                burst_non_keepers: 1,
                missing: 0,
                suggested_reject: 0,
                suggested_pick: 0,
                suggested_rating: 0,
            }
        );
        let g = filter_counts(&conn, Some(2)).unwrap();
        assert_eq!((g.total, g.tags.len(), g.burst_groups, g.ratings[2]), (1, 0, 0, 1));
    }

    #[test]
    fn phase4_xmp_dirty_triggers_and_status() {
        let mut conn = phase4_fixture();
        let dirty = |conn: &Connection| -> Vec<ImageId> {
            conn.prepare("SELECT id FROM images WHERE xmp_dirty = 1 ORDER BY id")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert!(dirty(&conn).is_empty());
        assert_eq!(get_image(&conn, 1).unwrap().xmp, XmpSyncState::default());

        // No-op writes and non-visible tag changes do not dirty.
        set_rating(&mut conn, &[2], 5).unwrap();
        conn.execute("UPDATE image_tags SET confidence = 0.3 WHERE image_id = 3 AND tag = 'blink'", []).unwrap();
        conn.execute("UPDATE image_tags SET source = 'user' WHERE image_id = 1", []).unwrap();
        conn.execute("DELETE FROM image_tags WHERE image_id = 3 AND tag = 'motion_blur'", []).unwrap();
        conn.execute("INSERT INTO image_tags (image_id, tag, source, suppressed) VALUES (4, 'blink', 'auto', 1)", [])
            .unwrap();
        assert!(dirty(&conn).is_empty());

        set_rating(&mut conn, &[1], 3).unwrap();
        set_pick(&mut conn, &[2], PickFlag::Reject).unwrap();
        set_color_label(&mut conn, &[3], None).unwrap();
        assert_eq!(dirty(&conn), [1, 2, 3]);
        let e = get_image(&conn, 1).unwrap();
        assert!(e.xmp.dirty && e.xmp.synced_at_ms.is_none());
        let updated: i64 = conn.query_row("SELECT meta_updated_at FROM images WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert!((now_ms() - updated).abs() < 5_000, "meta_updated_at is unix ms");

        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();
        set_user_tag(&mut conn, &[4], CullTag::Blink, true).unwrap(); // un-suppress via user tag
        set_user_tag(&mut conn, &[3], CullTag::Blink, false).unwrap(); // suppress auto
        conn.execute("INSERT INTO image_tags (image_id, tag, source) VALUES (2, 'underexposed', 'auto')", []).unwrap();
        assert_eq!(dirty(&conn), [2, 3, 4]);
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();
        conn.execute("DELETE FROM image_tags WHERE image_id = 2", []).unwrap();
        assert_eq!(dirty(&conn), [2]);

        // The sync bookkeeping update itself does not re-dirty.
        conn.execute("UPDATE images SET xmp_dirty = 0, xmp_synced_at = 5, xmp_error = 'x' WHERE id = 2", []).unwrap();
        assert!(dirty(&conn).is_empty());
        assert_eq!(
            get_image(&conn, 2).unwrap().xmp,
            XmpSyncState { dirty: false, synced_at_ms: Some(5), error: Some("x".into()), has_sidecar: false }
        );

        // On by default since v12 (IPC v14).
        assert!(catalog_state(&conn, "", "").unwrap().xmp_auto_sync);
        set_xmp_auto_sync(&conn, false).unwrap();
        assert!(!catalog_state(&conn, "", "").unwrap().xmp_auto_sync);
        set_xmp_auto_sync(&conn, true).unwrap();
        assert!(catalog_state(&conn, "", "").unwrap().xmp_auto_sync);
        set_rating(&mut conn, &[4], 1).unwrap();
        assert_eq!(xmp_status(&conn, true).unwrap(), XmpStatus { dirty: 1, failed: 1, running: true, auto_sync: true });
    }

    #[test]
    fn cull_snapshot_round_trip_is_atomic_and_marks_dirty() {
        let mut conn = phase4_fixture();
        let before = cull_snapshot(&conn, &[2, 1]).unwrap();
        assert_eq!(
            before,
            vec![
                CullSnapshot {
                    image_id: 2,
                    rating: 5,
                    pick: PickFlag::Pick,
                    color_label: None,
                    pick_origin: Some(PickOrigin::User)
                },
                CullSnapshot {
                    image_id: 1,
                    rating: 2,
                    pick: PickFlag::Unflagged,
                    color_label: Some(ColorLabel::Red),
                    pick_origin: None
                },
            ]
        );
        assert_eq!(cull_snapshot(&conn, &[1, 99]).unwrap_err().kind, ErrorKind::NotFound);

        set_rating(&mut conn, &[1, 2], 0).unwrap();
        set_pick(&mut conn, &[1], PickFlag::Reject).unwrap();
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();

        // Unknown id: nothing restored.
        let mut bad = before.clone();
        bad.push(CullSnapshot {
            image_id: 99,
            rating: 0,
            pick: PickFlag::Unflagged,
            color_label: None,
            pick_origin: None,
        });
        assert_eq!(restore_cull_snapshot(&mut conn, &bad).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(get_image(&conn, 1).unwrap().pick, PickFlag::Reject);
        let mut bad = before.clone();
        bad[0].rating = 6;
        assert_eq!(restore_cull_snapshot(&mut conn, &bad).unwrap_err().kind, ErrorKind::InvalidArgument);

        assert_eq!(restore_cull_snapshot(&mut conn, &before).unwrap(), vec![2, 1]);
        assert_eq!(cull_snapshot(&conn, &[2, 1]).unwrap(), before);
        assert!(get_image(&conn, 1).unwrap().xmp.dirty && get_image(&conn, 2).unwrap().xmp.dirty);
        // Restoring unchanged values is a no-op.
        assert!(restore_cull_snapshot(&mut conn, &before).unwrap().is_empty());
    }

    #[test]
    fn ui_prefs_round_trip() {
        let conn = open_in_memory();
        assert_eq!(ui_prefs(&conn).unwrap(), UiPrefs::default());
        let prefs = UiPrefs {
            last_export_folder: Some("/Users/me/Exports".into()),
            copy_fields: Some(vec![AdjustmentField::Exposure, AdjustmentField::ProcessVersion]),
            xmp_explainer_seen: Some(true),
            scene_strip_visible: Some(false),
        };
        set_ui_prefs(&conn, &prefs).unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), prefs);
        let empty = UiPrefs { last_export_folder: Some(String::new()), ..Default::default() };
        assert_eq!(set_ui_prefs(&conn, &empty).unwrap_err().kind, ErrorKind::InvalidArgument);
        // Unknown / missing fields from other versions are tolerated.
        set_meta(&conn, UI_PREFS_KEY, r#"{"futureThing":1}"#).unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), UiPrefs::default());
        set_meta(&conn, UI_PREFS_KEY, "not json").unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), UiPrefs::default());
    }

    // -----------------------------------------------------------------------
    // IPC v18: keeper modes, pick origin, cull summary, metadata filters
    // -----------------------------------------------------------------------

    /// Every (pick, rating, suggestion) combination, one image each, in folder 1.
    /// (id, pick, rating, suggested pick) of a [`keeper_grid`] image.
    type GridRow = (ImageId, PickFlag, u8, Option<PickFlag>);

    fn keeper_grid() -> (Connection, Vec<GridRow>) {
        let conn = open_in_memory();
        conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0)", []).unwrap();
        let mut rows = Vec::new();
        let mut id = 0;
        for pick in PickFlag::ALL {
            for rating in 0..=5u8 {
                for sugg in [None, Some(PickFlag::Pick), Some(PickFlag::Unflagged), Some(PickFlag::Reject)] {
                    id += 1;
                    conn.execute(
                        "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size,
                                             file_mtime_ms, imported_at, rating, pick)
                         VALUES (?1, 1, ?2, ?2, 'arw', 'sony', 1, 0, 0, ?3, ?4)",
                        params![id, format!("/f/{id}.arw"), rating, pick.as_str()],
                    )
                    .unwrap();
                    if let Some(s) = sugg {
                        conn.execute(
                            "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                                     clipped_shadows_pct, mean_luma, model_version, analyzed_at, suggested_pick)
                             VALUES (?1, 0.5, 0.5, 0, 0, 0.5, 'v', 1, ?2)",
                            params![id, s.as_str()],
                        )
                        .unwrap();
                    }
                    rows.push((id, *pick, rating, sugg));
                }
            }
        }
        (conn, rows)
    }

    /// v18.1 (UX P1-1): the summary's pending suggestions are exactly what one Apply
    /// suggestions with its defaults (`onlyUnset`) over the scope changes, split by outcome;
    /// afterwards nothing is pending and a second Apply changes nothing.
    #[test]
    fn suggestions_pending_equals_default_apply() {
        let (mut conn, rows) = keeper_grid();
        // Suggested stars everywhere, plus untouched images whose suggestion changes nothing
        // (unflagged, 0 stars; 1000) or only the stars (1001, 1002).
        conn.execute("UPDATE quality_scores SET suggested_rating = 3", []).unwrap();
        for (id, stars) in [(1000, 0), (1001, 2), (1002, 5)] {
            conn.execute(
                "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size,
                                     file_mtime_ms, imported_at, rating, pick)
                 VALUES (?1, 1, ?2, ?2, 'arw', 'sony', 1, 0, 0, 0, 'unflagged')",
                params![id, format!("/f/{id}.arw")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                         clipped_shadows_pct, mean_luma, model_version, analyzed_at, suggested_pick, suggested_rating)
                 VALUES (?1, 0.5, 0.5, 0, 0, 0.5, 'v', 1, 'unflagged', ?2)",
                params![id, stars],
            )
            .unwrap();
        }
        let ids: Vec<ImageId> = rows.iter().map(|r| r.0).chain([1000, 1001, 1002]).collect();
        let s = cull_summary(&conn, &FolderScope::all()).unwrap();
        let pending = s.suggested_pick_pending + s.suggested_reject_pending + s.suggested_rating_pending;
        // Grid: one untouched image per suggestion (the `unflagged` one now has 3 stars).
        assert_eq!((s.suggested_pick_pending, s.suggested_reject_pending, s.suggested_rating_pending), (1, 1, 3));

        let before: Vec<RawImageEntry> = ids.iter().map(|&id| get_image(&conn, id).unwrap()).collect();
        let r = apply_suggestions(&mut conn, &ids, true).unwrap();
        assert_eq!(r.applied, pending, "Apply changes exactly the pending images");
        assert_eq!(r.applied + r.skipped, ids.len() as u32);
        let (mut picks, mut rejects, mut stars) = (0, 0, 0);
        for b in &before {
            let a = get_image(&conn, b.id).unwrap();
            if (a.pick, a.rating) == (b.pick, b.rating) {
                continue;
            }
            assert_eq!((b.pick, b.rating), (PickFlag::Unflagged, 0), "only untouched images change");
            match a.pick {
                PickFlag::Pick => picks += 1,
                PickFlag::Reject => rejects += 1,
                PickFlag::Unflagged => stars += 1,
            }
        }
        assert_eq!(
            (picks, rejects, stars),
            (s.suggested_pick_pending, s.suggested_reject_pending, s.suggested_rating_pending)
        );
        let after = cull_summary(&conn, &FolderScope::all()).unwrap();
        assert_eq!(
            (after.suggested_pick_pending, after.suggested_reject_pending, after.suggested_rating_pending),
            (0, 0, 0)
        );
        assert_eq!(apply_suggestions(&mut conn, &ids, true).unwrap().applied, 0);
        // Without onlyUnset, images already matching their suggestion are not counted either.
        assert!(apply_suggestions(&mut conn, &ids, false).unwrap().applied > 0);
        assert_eq!(apply_suggestions(&mut conn, &ids, false).unwrap().applied, 0);
    }

    /// v18.1 (UX P1-6): `ImageQuery.pickOrigin` in queries, facet counts and metadata options.
    #[test]
    fn pick_origin_filter() {
        let (conn, rows) = keeper_grid();
        // Every other flagged image was flagged by Auto.
        conn.execute("UPDATE images SET pick_origin = 'auto' WHERE pick <> 'unflagged' AND id % 2 = 0", []).unwrap();
        let s = cull_summary(&conn, &FolderScope::all()).unwrap();
        let q = |picks: Vec<PickFlag>, o: Option<PickOrigin>| ImageQuery {
            picks,
            pick_origin: o,
            limit: 1000,
            ..Default::default()
        };
        let rejects = |o| ids_for(&conn, q(vec![PickFlag::Reject], o)).len() as u32;
        assert!(s.rejected_auto > 0 && s.rejected_by_user > 0);
        assert_eq!(rejects(Some(PickOrigin::Auto)), s.rejected_auto);
        assert_eq!(rejects(Some(PickOrigin::User)), s.rejected_by_user);
        assert_eq!(rejects(None), s.rejected);
        assert_eq!(ids_for(&conn, q(vec![PickFlag::Pick], Some(PickOrigin::Auto))).len() as u32, s.picked_auto);
        // Unflagged images never match an origin; alone, an origin means "flagged by".
        assert!(ids_for(&conn, q(vec![PickFlag::Unflagged], Some(PickOrigin::User))).is_empty());
        let auto = sorted(ids_for(&conn, q(vec![], Some(PickOrigin::Auto))));
        let expect: Vec<ImageId> =
            rows.iter().filter(|r| r.1 != PickFlag::Unflagged && r.0 % 2 == 0).map(|r| r.0).collect();
        assert_eq!(auto, expect);

        for o in [PickOrigin::Auto, PickOrigin::User] {
            let none = MetadataFilter::default();
            let c = filter_counts_with(&conn, FolderScope::all(), false, &none, Some(o)).unwrap();
            let (p, r) = match o {
                PickOrigin::Auto => (s.picked_auto, s.rejected_auto),
                PickOrigin::User => (s.picked - s.picked_auto, s.rejected_by_user),
            };
            assert_eq!((c.picked, c.rejected, c.unflagged, c.total), (p, r, 0, p + r), "{o:?}");
            assert_eq!(c.ratings.iter().sum::<u32>(), c.total);
            let scoped = filter_counts_with(&conn, FolderScope::from(Some(1)), false, &none, Some(o)).unwrap();
            assert_eq!(scoped, c, "folder-scoped path agrees ({o:?})");
            let opts = metadata_filter_options(&conn, &q(vec![PickFlag::Reject], Some(o))).unwrap();
            assert_eq!(opts.total, r, "{o:?}");
        }
    }

    #[test]
    fn keeper_modes_rust_and_sql_agree_and_summary_matches_keepers_only() {
        let (conn, rows) = keeper_grid();
        let rules = [
            KeeperRule::default(),
            KeeperRule::picks_and_ratings(1, true),
            KeeperRule::picks_and_ratings(1, false),
            KeeperRule::picks_and_ratings(3, true),
            KeeperRule::picks_and_ratings(5, false),
        ];
        for rule in rules {
            set_keeper_rule(&conn, &rule).unwrap();
            let sql: Vec<ImageId> =
                ids_for(&conn, ImageQuery { keepers_only: true, limit: 1000, ..Default::default() });
            let rust: Vec<ImageId> =
                rows.iter().filter(|(_, p, r, s)| rule.is_keeper_values(*p, *r, *s)).map(|r| r.0).collect();
            let mut sql_sorted = sql.clone();
            sql_sorted.sort();
            assert_eq!(sql_sorted, rust, "SQL mirrors {rule:?}");

            let s = cull_summary(&conn, &FolderScope::all()).unwrap();
            assert_eq!(s.keepers as usize, sql.len(), "{rule:?}");
            let b = &s.keeper_breakdown;
            assert_eq!(b.picked + b.unflagged + b.starred + b.suggested, s.keepers, "breakdown adds up for {rule:?}");
            assert_eq!(s.total, s.picked + s.unflagged + s.rejected);
            assert_eq!(s.keeper_rule, rule);
            assert_eq!(filter_counts_keepers(&conn, FolderScope::all()).unwrap().total, s.keepers);
            let projects_keepers: u32 = conn
                .query_row(&format!("SELECT COUNT(*) FROM images WHERE {}", keeper_predicate(&rule, "")), [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(projects_keepers, s.keepers);
        }
        // The default keeps everything not rejected.
        set_keeper_rule(&conn, &KeeperRule::default()).unwrap();
        let s = cull_summary(&conn, &FolderScope::all()).unwrap();
        assert_eq!(s.keepers, s.picked + s.unflagged);
        assert_eq!((s.keeper_breakdown.starred, s.keeper_breakdown.suggested), (0, 0));
        // 24 combinations per flag; suggestions pending = unflagged, 0 stars, that suggestion
        // (v18.1; suggested ratings are all 0 here).
        assert_eq!((s.picked, s.unflagged, s.rejected), (24, 24, 24));
        assert_eq!((s.suggested_reject_pending, s.suggested_pick_pending, s.unanalyzed), (1, 1, 18));
        assert_eq!(s.suggested_rating_pending, 0);
        assert_eq!(s.starred, 60);
        // Validation is mode-independent.
        let bad = KeeperRule { mode: KeeperMode::NotRejected, min_rating: 0, use_suggestions: true };
        assert_eq!(set_keeper_rule(&conn, &bad).unwrap_err().kind, ErrorKind::InvalidArgument);
    }

    #[test]
    fn pick_origin_follows_writers_and_feeds_the_summary() {
        let mut conn = phase4_fixture();
        conn.execute_batch(
            "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                     clipped_shadows_pct, mean_luma, model_version, analyzed_at, suggested_rating, suggested_pick,
                     reasons_json)
             VALUES (1, 0.1, 0.1, 0, 0, 0.5, 'v', 1, 0, 'reject',
                     '[{\"kind\":\"blink\",\"text\":\"Eyes closed\"},{\"kind\":\"duplicate_burst\",\"text\":\"Duplicate in burst (keeper b.arw)\",\"relatedImageId\":2}]'),
                    (2, 0.9, 0.9, 0, 0, 0.5, 'v', 1, 5, 'pick', 'not json');",
        )
        .unwrap();
        let reasons = get_image(&conn, 1).unwrap().quality.unwrap().reasons;
        assert_eq!(reasons.len(), 2);
        assert_eq!(reasons[1].kind, SuggestionReasonKind::DuplicateBurst);
        assert_eq!(reasons[1].related_image_id, Some(2));
        assert!(get_image(&conn, 2).unwrap().quality.unwrap().reasons.is_empty(), "bad JSON reads as none");
        assert_eq!(get_image(&conn, 1).unwrap().pick_origin, None, "unflagged has no origin");

        let before = cull_snapshot(&conn, &[1, 2]).unwrap();
        apply_suggestions(&mut conn, &[1, 2], false).unwrap();
        let e1 = get_image(&conn, 1).unwrap();
        assert_eq!((e1.pick, e1.pick_origin), (PickFlag::Reject, Some(PickOrigin::Auto)));
        assert_eq!(get_image(&conn, 2).unwrap().pick_origin, Some(PickOrigin::User), "unchanged flag keeps origin");
        let s = cull_summary(&conn, &FolderScope::all()).unwrap();
        assert_eq!((s.rejected, s.rejected_auto, s.rejected_by_user), (2, 1, 1));
        assert_eq!(s.picked_auto, 0);

        // Undo restores the user state; redo (snapshot of the auto state) restores `auto`.
        let after = cull_snapshot(&conn, &[1]).unwrap();
        assert_eq!(after[0].pick_origin, Some(PickOrigin::Auto));
        restore_cull_snapshot(&mut conn, &before).unwrap();
        assert_eq!(get_image(&conn, 1).unwrap().pick, PickFlag::Unflagged);
        restore_cull_snapshot(&mut conn, &after).unwrap();
        assert_eq!(get_image(&conn, 1).unwrap().pick_origin, Some(PickOrigin::Auto));
        // Only the origin differs: still restored (and reported changed).
        let as_user = vec![CullSnapshot { pick_origin: None, ..after[0].clone() }];
        assert_eq!(restore_cull_snapshot(&mut conn, &as_user).unwrap(), vec![1]);
        assert_eq!(get_image(&conn, 1).unwrap().pick_origin, Some(PickOrigin::User));

        // A user flag over an auto flag becomes `user`.
        apply_suggestions(&mut conn, &[1], false).unwrap();
        set_pick(&mut conn, &[1], PickFlag::Reject).unwrap();
        assert_eq!(get_image(&conn, 1).unwrap().pick_origin, Some(PickOrigin::User));
    }

    /// Six images with varied metadata (folder 1: 1-5, folder 2: 6).
    fn metadata_fixture() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0), (2, '/g', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, camera_model, lens, iso,
                                 shutter_s, aperture, focal_length_mm, captured_at_ms, xmp_mtime_ms,
                                 file_size, file_mtime_ms, imported_at, pick)
             VALUES
               (1, 1, '/f/A.ARW',  'A.ARW',  'arw',  'sony',     'ILCE-7M4', 'FE 35mm F1.4 GM', 100,  0.004,   1.4, 35.0,
                86400000 + 1000, 5, 1, 0, 0, 'pick'),
               (2, 1, '/f/b.arw',  'b.arw',  'arw',  'sony',     'ILCE-7M4', 'FE 85mm F1.4 GM', 800,  0.008,   1.4, 85.0,
                86400000 + 5000, NULL, 1, 0, 0, 'unflagged'),
               (3, 1, '/f/c.RAF',  'c.RAF',  'raf',  'fujifilm', 'X-T5',     NULL,              3200, 0.000125, 2.8, 23.04,
                2 * 86400000, NULL, 1, 0, 0, 'reject'),
               (4, 1, '/f/d.JPG',  'd.JPG',  'jpeg', 'canon',    'EOS R5',   '  ',              6400, 0.5,     2.8, 50.0,
                NULL, 7, 1, 0, 0, 'unflagged'),
               (5, 1, '/f/e.jpeg', 'e.jpeg', 'jpeg', 'other',    NULL,       NULL,              NULL, NULL,    NULL, NULL,
                3 * 86400000, NULL, 1, 0, 0, 'unflagged'),
               (6, 2, '/g/f.arw',  'f.arw',  'arw',  'sony',     'ILCE-7M4', 'FE 35mm F1.4 GM', 100,  0.004,   1.4, 35.0,
                86400000, NULL, 1, 0, 0, 'unflagged');
             INSERT INTO adjustments (image_id, params_json, process_version, updated_at, neutral)
             VALUES (1, '{}', 1, 0, 0), (3, '{}', 1, 0, 1);",
        )
        .unwrap();
        conn
    }

    fn meta(m: MetadataFilter) -> ImageQuery {
        ImageQuery { metadata: m, ..Default::default() }
    }

    fn sorted(mut v: Vec<ImageId>) -> Vec<ImageId> {
        v.sort();
        v
    }

    #[test]
    fn metadata_filters_one_by_one() {
        let conn = metadata_fixture();
        let ids = |m: MetadataFilter| sorted(ids_for(&conn, meta(m)));
        let d = MetadataFilter::default;
        assert_eq!(ids(d()), [1, 2, 3, 4, 5, 6]);
        assert_eq!(ids(MetadataFilter { formats: vec![ImageFormat::Jpeg], ..d() }), [4, 5]);
        assert_eq!(ids(MetadataFilter { formats: vec![ImageFormat::Raf, ImageFormat::Arw], ..d() }), [1, 2, 3, 6]);
        assert_eq!(ids(MetadataFilter { extensions: vec!["JPG".into()], ..d() }), [4], "case-insensitive");
        assert_eq!(ids(MetadataFilter { extensions: vec!["jpeg".into(), "raf".into()], ..d() }), [3, 5]);
        assert_eq!(ids(MetadataFilter { extensions: vec!["arw".into()], ..d() }), [1, 2, 6]);
        let cam = |make, model: Option<&str>| CameraFilter { make, model: model.map(str::to_owned) };
        assert_eq!(ids(MetadataFilter { cameras: vec![cam(CameraMake::Sony, Some("ILCE-7M4"))], ..d() }), [1, 2, 6]);
        assert_eq!(
            ids(MetadataFilter {
                cameras: vec![cam(CameraMake::Fujifilm, Some("X-T5")), cam(CameraMake::Other, None)],
                ..d()
            }),
            [3, 5]
        );
        assert_eq!(ids(MetadataFilter { lenses: vec![Some("FE 35mm F1.4 GM".into())], ..d() }), [1, 6]);
        assert_eq!(ids(MetadataFilter { lenses: vec![None], ..d() }), [3, 4, 5], "blank lens is unknown");
        let range = |min: Option<f64>, max: Option<f64>| Some(NumberRange { min, max });
        assert_eq!(ids(MetadataFilter { iso: range(Some(800.0), Some(3200.0)), ..d() }), [2, 3]);
        assert_eq!(ids(MetadataFilter { iso: range(None, Some(100.0)), ..d() }), [1, 6]);
        assert_eq!(ids(MetadataFilter { focal_length_mm: range(Some(23.0), Some(23.0)), ..d() }), [3], "0.1 mm");
        assert_eq!(ids(MetadataFilter { focal_length_mm: range(Some(50.0), None), ..d() }), [2, 4]);
        assert_eq!(ids(MetadataFilter { aperture: range(Some(2.8), Some(2.8)), ..d() }), [3, 4]);
        assert_eq!(ids(MetadataFilter { shutter_seconds: range(Some(0.000125), Some(0.000125)), ..d() }), [3]);
        assert_eq!(ids(MetadataFilter { shutter_seconds: range(Some(0.004), Some(0.008)), ..d() }), [1, 2, 6]);
        let day = 86_400_000;
        let dates = |from, to| Some(DateRange { from_ms: from, to_ms: to });
        assert_eq!(ids(MetadataFilter { captured: dates(Some(day), Some(2 * day)), ..d() }), [1, 2, 6], "to exclusive");
        assert_eq!(ids(MetadataFilter { captured: dates(Some(2 * day), None), ..d() }), [3, 5]);
        assert_eq!(ids(MetadataFilter { edited: Some(true), ..d() }), [1], "neutral adjustments are unedited");
        assert_eq!(ids(MetadataFilter { edited: Some(false), ..d() }), [2, 3, 4, 5, 6]);
        assert_eq!(ids(MetadataFilter { has_sidecar: Some(true), ..d() }), [1, 4]);
        assert_eq!(ids(MetadataFilter { has_sidecar: Some(false), ..d() }), [2, 3, 5, 6]);
        assert!(get_image(&conn, 1).unwrap().xmp.has_sidecar && !get_image(&conn, 2).unwrap().xmp.has_sidecar);
        // Combined (AND across fields) and with the other filters.
        let m = MetadataFilter { formats: vec![ImageFormat::Arw], iso: range(Some(100.0), Some(100.0)), ..d() };
        assert_eq!(ids(m.clone()), [1, 6]);
        assert_eq!(sorted(ids_for(&conn, ImageQuery { folder_id: Some(1), ..meta(m) })), [1]);
        // Invalid constraints.
        for bad in [
            MetadataFilter { extensions: vec!["a.b".into()], ..d() },
            MetadataFilter { extensions: vec![String::new()], ..d() },
            MetadataFilter { iso: range(Some(f64::NAN), None), ..d() },
        ] {
            assert_eq!(list_image_ids(&conn, &meta(bad)).unwrap_err().kind, ErrorKind::InvalidArgument);
        }
        // Wire shape: every field optional.
        let q: ImageQuery = serde_json::from_value(serde_json::json!({
            "includeTags": [], "excludeTags": [], "tagMatch": "any", "picks": [], "minRating": null,
            "maxRating": null, "colorLabels": [], "burstGroupId": null, "collapseBursts": false, "folderId": null,
            "sort": "capture_time", "sortDescending": false, "offset": 0, "limit": 10,
            "metadata": {"extensions": ["raf"]}
        }))
        .unwrap();
        assert_eq!(sorted(ids_for(&conn, q)), [3]);
    }

    #[test]
    fn filter_counts_honour_metadata() {
        let conn = metadata_fixture();
        let m = MetadataFilter { formats: vec![ImageFormat::Arw, ImageFormat::Raf], ..Default::default() };
        let c = filter_counts_with(&conn, FolderScope::all(), false, &m, None).unwrap();
        assert_eq!((c.total, c.picked, c.rejected, c.unflagged), (4, 1, 1, 2));
        let c = filter_counts_with(&conn, FolderScope::from(Some(1)), false, &m, None).unwrap();
        assert_eq!(c.total, 3);
        let c = filter_counts_with(&conn, FolderScope::from(Some(1)), true, &m, None).unwrap();
        assert_eq!(c.total, 2, "keepers (not rejected) among folder 1's RAWs");
        let edited = MetadataFilter { edited: Some(true), ..Default::default() };
        assert_eq!(filter_counts_with(&conn, FolderScope::all(), false, &edited, None).unwrap().total, 1);
        assert_eq!(
            filter_counts_with(&conn, FolderScope::all(), false, &MetadataFilter::default(), None).unwrap(),
            filter_counts(&conn, FolderScope::all()).unwrap()
        );
    }

    #[test]
    fn metadata_facets_cascade_and_count() {
        let conn = metadata_fixture();
        let o = metadata_filter_options(&conn, &ImageQuery::default()).unwrap();
        assert_eq!(o.total, 6);
        assert_eq!(
            o.formats.iter().map(|c| (c.format, c.count)).collect::<Vec<_>>(),
            [(ImageFormat::Arw, 3), (ImageFormat::Raf, 1), (ImageFormat::Jpeg, 2)]
        );
        assert_eq!(
            o.extensions.iter().map(|c| (c.extension.as_str(), c.count)).collect::<Vec<_>>(),
            [("arw", 3), ("jpeg", 1), ("jpg", 1), ("raf", 1)]
        );
        assert_eq!(o.cameras.len(), 4);
        assert_eq!(o.cameras.last().unwrap().camera, CameraFilter { make: CameraMake::Other, model: None });
        assert_eq!(o.cameras.iter().find(|c| c.camera.make == CameraMake::Sony).unwrap().count, 3);
        assert_eq!(
            o.lenses.iter().map(|c| (c.lens.as_deref(), c.count)).collect::<Vec<_>>(),
            [(Some("FE 35mm F1.4 GM"), 2), (Some("FE 85mm F1.4 GM"), 1), (None, 3)]
        );
        assert_eq!(
            o.isos.iter().map(|c| (c.value, c.count)).collect::<Vec<_>>(),
            [(Some(100.0), 2), (Some(800.0), 1), (Some(3200.0), 1), (Some(6400.0), 1), (None, 1)]
        );
        assert_eq!(o.focal_lengths.first().unwrap().value, Some(23.0), "rounded to 0.1 mm");
        assert_eq!(o.apertures.iter().map(|c| c.count).collect::<Vec<_>>(), [3, 2, 1]);
        assert_eq!(o.shutter_speeds.first().unwrap().value, Some(0.000125));
        assert_eq!(
            o.capture_days.iter().map(|c| (c.day_start_ms, c.count)).collect::<Vec<_>>(),
            [(Some(86_400_000), 3), (Some(2 * 86_400_000), 1), (Some(3 * 86_400_000), 1), (None, 1)]
        );
        assert_eq!(o.edited, YesNoCount { yes: 1, no: 5 });
        assert_eq!(o.has_sidecar, YesNoCount { yes: 2, no: 4 });

        // A facet ignores its own constraint but follows the others.
        let q = meta(MetadataFilter { formats: vec![ImageFormat::Jpeg], ..Default::default() });
        let o = metadata_filter_options(&conn, &q).unwrap();
        assert_eq!(o.total, 2);
        assert_eq!(o.formats.len(), 3, "own facet: every format still offered");
        assert_eq!(o.extensions.iter().map(|c| c.extension.as_str()).collect::<Vec<_>>(), ["jpeg", "jpg"]);
        assert_eq!(o.edited, YesNoCount { yes: 0, no: 2 });
        // Every facet value's count equals the total of selecting it.
        let o = metadata_filter_options(&conn, &ImageQuery { folder_id: Some(1), ..Default::default() }).unwrap();
        for c in &o.isos {
            let m = MetadataFilter { iso: Some(NumberRange { min: c.value, max: c.value }), ..Default::default() };
            if c.value.is_some() {
                let n = list_image_ids(&conn, &ImageQuery { folder_id: Some(1), ..meta(m) }).unwrap().len();
                assert_eq!(n as u32, c.count, "iso {:?}", c.value);
            }
        }
        for c in &o.apertures {
            let m = MetadataFilter { aperture: Some(NumberRange { min: c.value, max: c.value }), ..Default::default() };
            if c.value.is_some() {
                let n = list_image_ids(&conn, &ImageQuery { folder_id: Some(1), ..meta(m) }).unwrap().len();
                assert_eq!(n as u32, c.count, "aperture {:?}", c.value);
            }
        }
        // Keepers only: the rejected RAF disappears.
        let o = metadata_filter_options(&conn, &ImageQuery { keepers_only: true, ..Default::default() }).unwrap();
        assert!(o.formats.iter().all(|c| c.format != ImageFormat::Raf));
    }

    /// v19.2 (UX P1-4): camera bodies (make + model + serial) as a filter and a facet.
    #[test]
    fn camera_body_filter_and_facet() {
        let conn = metadata_fixture();
        conn.execute_batch(
            "UPDATE images SET camera_serial = 'A1' WHERE id IN (1, 6);
             UPDATE images SET camera_serial = ' B2 ' WHERE id = 2;",
        )
        .unwrap();
        let body = |make, model: Option<&str>, serial: Option<&str>| CameraBody {
            make,
            model: model.map(str::to_owned),
            serial: serial.map(str::to_owned),
        };
        let a1 = body(CameraMake::Sony, Some("ILCE-7M4"), Some("A1"));
        let ids =
            |bodies: Vec<CameraBody>| sorted(ids_for(&conn, meta(MetadataFilter { bodies, ..Default::default() })));
        assert_eq!(ids(vec![a1.clone()]), [1, 6]);
        assert_eq!(ids(vec![body(CameraMake::Sony, Some("ILCE-7M4"), Some("B2"))]), [2], "trimmed");
        assert!(ids(vec![body(CameraMake::Sony, Some("ILCE-7M4"), None)]).is_empty(), "null serial = unknown only");
        assert_eq!(ids(vec![body(CameraMake::Canon, Some("EOS R5"), None)]), [4]);
        assert_eq!(ids(vec![a1.clone(), body(CameraMake::Other, None, None)]), [1, 5, 6]);

        let o = metadata_filter_options(&conn, &ImageQuery::default()).unwrap();
        assert_eq!(
            o.bodies.iter().map(|c| (c.body.clone(), c.count)).collect::<Vec<_>>(),
            [
                (body(CameraMake::Canon, Some("EOS R5"), None), 1),
                (body(CameraMake::Fujifilm, Some("X-T5"), None), 1),
                (a1.clone(), 2),
                (body(CameraMake::Sony, Some("ILCE-7M4"), Some("B2")), 1),
                (body(CameraMake::Other, None, None), 1),
            ]
        );
        // Cascade: the body facet ignores its own constraint, the camera facet follows it.
        let q = meta(MetadataFilter { bodies: vec![a1], ..Default::default() });
        let o = metadata_filter_options(&conn, &q).unwrap();
        assert_eq!((o.total, o.bodies.len()), (2, 5));
        assert_eq!(o.cameras.iter().map(|c| c.count).collect::<Vec<_>>(), [2]);
        let counts = filter_counts_with(&conn, FolderScope::all(), false, &q.metadata, None).unwrap();
        assert_eq!(counts.total, 2);
        // The entry carries the serial.
        assert_eq!(get_image(&conn, 1).unwrap().camera.serial.as_deref(), Some("A1"));
    }

    /// v19.2 (UX P1-5): `ImageQuery.suggested`, `FilterCounts.suggested*`, and
    /// `apply_suggestions` per kind.
    #[test]
    fn suggested_filter_counts_and_apply_kinds() {
        let mut conn = metadata_fixture();
        // 1: picked by the user, suggested reject (not pending); 2: pending reject (1 star
        // suggested); 4: pending pick (4 stars); 5: pending stars only; 6: rated, not pending.
        for (id, pick, stars) in
            [(1, "reject", 0), (2, "reject", 1), (4, "pick", 4), (5, "unflagged", 3), (6, "reject", 0)]
        {
            conn.execute(
                "INSERT INTO quality_scores (image_id, overall, global_sharpness, clipped_highlights_pct,
                         clipped_shadows_pct, mean_luma, model_version, analyzed_at, suggested_pick, suggested_rating)
                 VALUES (?1, 0.5, 0.5, 0, 0, 0.5, 'v', 1, ?2, ?3)",
                params![id, pick, stars],
            )
            .unwrap();
        }
        conn.execute("UPDATE images SET rating = 2 WHERE id = 6", []).unwrap();
        let q = |kind| ImageQuery { suggested: Some(kind), ..Default::default() };
        assert_eq!(ids_for(&conn, q(PendingSuggestion::Reject)), [2]);
        assert_eq!(ids_for(&conn, q(PendingSuggestion::Pick)), [4]);
        assert_eq!(ids_for(&conn, q(PendingSuggestion::Rating)), [5]);
        let c = filter_counts(&conn, FolderScope::all()).unwrap();
        assert_eq!((c.suggested_reject, c.suggested_pick, c.suggested_rating), (1, 1, 1));
        let s = cull_summary(&conn, &FolderScope::all()).unwrap();
        assert_eq!(
            (s.suggested_reject_pending, s.suggested_pick_pending, s.suggested_rating_pending),
            (c.suggested_reject, c.suggested_pick, c.suggested_rating)
        );
        // Scoped / constrained counts take the slow path.
        let jpeg = MetadataFilter { formats: vec![ImageFormat::Jpeg], ..Default::default() };
        let c = filter_counts_with(&conn, FolderScope::all(), false, &jpeg, None).unwrap();
        assert_eq!((c.suggested_reject, c.suggested_pick, c.suggested_rating), (0, 1, 1));
        let facets = metadata_filter_options(&conn, &q(PendingSuggestion::Reject)).unwrap();
        assert_eq!(facets.total, 1);

        let all: Vec<ImageId> = (1..=6).collect();
        let pick_rating = |conn: &Connection, id| {
            let e = get_image(conn, id).unwrap();
            (e.pick, e.rating, e.pick_origin)
        };
        // Rejects only: the pending reject is flagged, its suggested star is not copied.
        let only = |picks, rejects, stars| SuggestionKinds { picks, rejects, stars };
        let r = apply_suggestions_kinds(&mut conn, &all, true, only(false, true, false)).unwrap();
        assert_eq!((r.applied, r.skipped), (1, 5));
        assert_eq!(pick_rating(&conn, 2), (PickFlag::Reject, 0, Some(PickOrigin::Auto)));
        assert_eq!(pick_rating(&conn, 4), (PickFlag::Unflagged, 0, None));
        assert!(ids_for(&conn, q(PendingSuggestion::Reject)).is_empty());
        // Stars only (2 is no longer untouched).
        let r = apply_suggestions_kinds(&mut conn, &all, true, only(false, false, true)).unwrap();
        assert_eq!(r.applied, 2);
        assert_eq!(pick_rating(&conn, 4), (PickFlag::Unflagged, 4, None));
        assert_eq!(pick_rating(&conn, 5).1, 3);
        // Picks only without onlyUnset: the user's own flags with other suggestions stay.
        let r = apply_suggestions_kinds(&mut conn, &all, false, only(true, false, false)).unwrap();
        assert_eq!(r.applied, 1);
        assert_eq!(pick_rating(&conn, 4), (PickFlag::Pick, 4, Some(PickOrigin::Auto)));
        assert_eq!(pick_rating(&conn, 1).0, PickFlag::Pick, "suggested reject of a pick needs `rejects`");
        // A suggested "no flag" clears a flag only with both flag kinds.
        conn.execute("UPDATE images SET pick = 'pick' WHERE id = 5", []).unwrap();
        assert_eq!(apply_suggestions_kinds(&mut conn, &[5], false, only(true, false, false)).unwrap().applied, 0);
        assert_eq!(apply_suggestions_kinds(&mut conn, &[5], false, only(true, true, false)).unwrap().applied, 1);
        assert_eq!(pick_rating(&conn, 5).0, PickFlag::Unflagged);
        assert_eq!(
            apply_suggestions_kinds(&mut conn, &[1, 99], false, SuggestionKinds::default()).unwrap_err().kind,
            crate::ipc::error::ErrorKind::NotFound
        );
    }

    /// v19.2: extraction records the body serial (and keeps a known one when a later read has none).
    #[test]
    fn extraction_records_camera_serial() {
        let mut conn = metadata_fixture();
        let serial = |c: &Connection| -> (Option<String>, bool) {
            c.query_row("SELECT camera_serial, camera_serial_read FROM images WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
        };
        assert_eq!(serial(&conn), (None, false));
        let meta = raw::meta::ImageMeta { serial: Some("06258214".into()), ..Default::default() };
        record_extraction(&mut conn, 1, Some(&meta), Err("x")).unwrap();
        assert_eq!(serial(&conn), (Some("06258214".into()), true));
        record_extraction(&mut conn, 1, Some(&raw::meta::ImageMeta::default()), Err("x")).unwrap();
        assert_eq!(serial(&conn).0.as_deref(), Some("06258214"));
    }
}
