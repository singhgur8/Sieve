//! Catalog queries. Pure functions over a `Connection` so they are unit-testable
//! without a running Tauri app.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use walkdir::WalkDir;

use super::now_ms;
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
            "SELECT f.id, f.path, COUNT(i.id) FROM folders f
             LEFT JOIN images i ON i.folder_id = f.id
             GROUP BY f.id ORDER BY f.path",
        )?
        .query_map([], |r| Ok(FolderEntry { id: r.get(0)?, path: r.get(1)?, image_count: r.get(2)? }))?
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

/// `catalog_meta.xmp_auto_sync`; default off.
pub fn xmp_auto_sync(conn: &Connection) -> AppResult<bool> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM catalog_meta WHERE key = 'xmp_auto_sync'", [], |r| r.get(0)).optional()?;
    Ok(v.as_deref() == Some("1"))
}

pub fn set_xmp_auto_sync(conn: &Connection, enabled: bool) -> AppResult<()> {
    set_meta(conn, "xmp_auto_sync", if enabled { "1" } else { "0" })
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
pub fn import_folder(conn: &mut Connection, folder: &Path, opts: &ImportOptions) -> AppResult<ImportSummary> {
    let folder = folder.canonicalize().map_err(|e| AppError::invalid(format!("{}: {e}", folder.display())))?;
    if !folder.is_dir() {
        return Err(AppError::invalid(format!("{} is not a directory", folder.display())));
    }
    let folder_str = path_str(&folder)?;
    let now = now_ms();

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO folders (path, added_at) VALUES (?1, ?2) ON CONFLICT(path) DO NOTHING",
        params![folder_str, now],
    )?;
    let folder_id: FolderId = tx.query_row("SELECT id FROM folders WHERE path = ?1", [&folder_str], |r| r.get(0))?;

    let mut summary = ImportSummary { folder_id, ..Default::default() };
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
    tx.commit()?;
    Ok(summary)
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
           i.companion_path, i.develop_warnings
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
        }),
        None => None,
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
        },
        capture: CaptureMeta {
            captured_at_ms: r.get(8)?,
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
        pick: enum_col(r, 25, PickFlag::parse)?,
        color_label: opt_enum_col(r, 26, ColorLabel::parse)?,
        burst_group_id: r.get(27)?,
        is_burst_keeper: r.get(42)?,
        tags: Vec::new(),
        quality,
        has_edits: r.get(38)?,
        xmp: XmpSyncState { dirty: r.get(43)?, synced_at_ms: r.get(44)?, error: r.get(45)? },
        scene_id: r.get(46)?,
        is_scene_anchor: r.get(47)?,
        companion_path: r.get(48)?,
        // Unreadable JSON reads as no warnings (never blocks listing).
        develop_warnings: r
            .get::<_, Option<String>>(49)?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
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
    let [entry] = entries;
    Ok(entry)
}

/// Entries for `ids` in the given order. Atomic: an unknown id fails with `not_found`.
pub fn get_images(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<RawImageEntry>> {
    let mut by_id: HashMap<ImageId, RawImageEntry> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!("{ENTRY_SELECT} WHERE i.id IN ({ph})"))?;
        for e in stmt.query_map(params_from_iter(chunk.iter()), entry_from_row)? {
            let e = e?;
            by_id.insert(e.id, e);
        }
    }
    let mut out = ids
        .iter()
        .map(|id| by_id.get(id).cloned().ok_or_else(|| AppError::not_found(format!("image {id}"))))
        .collect::<AppResult<Vec<_>>>()?;
    attach_tags(conn, &mut out)?;
    Ok(out)
}

/// Binds `items` as text values and returns the matching `?,?,..` placeholder list.
fn text_list<T: Copy>(items: &[T], args: &mut Vec<Value>, as_str: fn(T) -> &'static str) -> String {
    args.extend(items.iter().map(|&t| Value::Text(as_str(t).to_owned())));
    vec!["?"; items.len()].join(",")
}

/// `WHERE` clause (with leading space, or empty) and its bound values for `q`'s filters.
fn query_filter(q: &ImageQuery) -> AppResult<(String, Vec<Value>)> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();

    if !q.include_tags.is_empty() {
        let ph = text_list(&q.include_tags, &mut args, CullTag::as_str);
        clauses.push(match q.tag_match {
            TagMatch::Any => format!(
                "EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id
                         AND it.suppressed = 0 AND it.tag IN ({ph}))"
            ),
            TagMatch::All => {
                let distinct = q.include_tags.iter().collect::<std::collections::HashSet<_>>().len();
                format!(
                    "(SELECT COUNT(DISTINCT it.tag) FROM image_tags it WHERE it.image_id = i.id
                      AND it.suppressed = 0 AND it.tag IN ({ph})) = {distinct}"
                )
            }
        });
    }
    if !q.exclude_tags.is_empty() {
        let ph = text_list(&q.exclude_tags, &mut args, CullTag::as_str);
        clauses.push(format!(
            "NOT EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id
                         AND it.suppressed = 0 AND it.tag IN ({ph}))"
        ));
    }
    if !q.picks.is_empty() {
        let ph = text_list(&q.picks, &mut args, PickFlag::as_str);
        clauses.push(format!("i.pick IN ({ph})"));
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
        clauses.push(
            "(i.burst_group_id IS NULL OR NOT EXISTS (
                 SELECT 1 FROM burst_groups b WHERE b.id = i.burst_group_id
                 AND b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id))"
                .into(),
        );
    }
    if let Some(folder) = q.folder_id {
        clauses.push("i.folder_id = ?".into());
        args.push(Value::Integer(folder));
    }

    let where_sql = if clauses.is_empty() { String::new() } else { format!(" WHERE {}", clauses.join(" AND ")) };
    Ok((where_sql, args))
}

/// `ORDER BY` terms for `q` (always ends with a unique key for stable paging).
fn query_order(q: &ImageQuery) -> &'static str {
    match (q.sort, q.sort_descending) {
        (ImageSort::CaptureTime, false) => "i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        (ImageSort::CaptureTime, true) => {
            "i.captured_at_ms IS NULL, i.captured_at_ms DESC, i.file_name DESC, i.id DESC"
        }
        (ImageSort::FileName, false) => "i.file_name, i.id",
        (ImageSort::FileName, true) => "i.file_name DESC, i.id DESC",
        (ImageSort::Quality, false) => "q.overall IS NULL, q.overall DESC, i.id",
        (ImageSort::Quality, true) => "q.overall IS NULL, q.overall, i.id DESC",
        (ImageSort::Rating, false) => "i.rating DESC, i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        (ImageSort::Rating, true) => "i.rating, i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
    }
}

pub fn list_images(conn: &Connection, q: &ImageQuery) -> AppResult<ImagePage> {
    let (where_sql, args) = query_filter(q)?;
    let total: u32 =
        conn.query_row(&format!("SELECT COUNT(*) FROM images i{where_sql}"), params_from_iter(args.iter()), |r| {
            r.get(0)
        })?;

    let limit = q.limit.min(ImageQuery::MAX_LIMIT);
    let sql = format!("{ENTRY_SELECT}{where_sql} ORDER BY {} LIMIT {limit} OFFSET {}", query_order(q), q.offset);
    let mut items =
        conn.prepare(&sql)?.query_map(params_from_iter(args.iter()), entry_from_row)?.collect::<Result<Vec<_>, _>>()?;
    attach_tags(conn, &mut items)?;

    Ok(ImagePage { items, total })
}

/// Every id matching `q` in sort order; `offset`/`limit` are ignored.
pub fn list_image_ids(conn: &Connection, q: &ImageQuery) -> AppResult<Vec<ImageId>> {
    let (where_sql, args) = query_filter(q)?;
    let sql = format!(
        "SELECT i.id FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id{where_sql} ORDER BY {}",
        query_order(q)
    );
    let ids = conn.prepare(&sql)?.query_map(params_from_iter(args.iter()), |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Facet counts over `folder` (or the whole catalog).
pub fn filter_counts(conn: &Connection, folder: Option<FolderId>) -> AppResult<FilterCounts> {
    let mut c = FilterCounts { ratings: vec![0; 6], ..Default::default() };
    {
        let mut stmt = conn.prepare(
            "SELECT pick, rating, COUNT(*) FROM images WHERE ?1 IS NULL OR folder_id = ?1 GROUP BY pick, rating",
        )?;
        let rows =
            stmt.query_map([folder], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?, r.get::<_, u32>(2)?)))?;
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
        .prepare(
            "SELECT it.tag, COUNT(*) FROM image_tags it JOIN images i ON i.id = it.image_id
             WHERE it.suppressed = 0 AND (?1 IS NULL OR i.folder_id = ?1)
             GROUP BY it.tag ORDER BY it.tag",
        )?
        .query_map([folder], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
        .filter_map(|r| match r {
            Ok((tag, count)) => CullTag::parse(&tag).map(|tag| Ok(TagCount { tag, count })),
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    (c.burst_groups, c.burst_non_keepers) = conn.query_row(
        "SELECT COUNT(DISTINCT i.burst_group_id),
                COALESCE(SUM(b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id), 0)
         FROM images i JOIN burst_groups b ON b.id = i.burst_group_id
         WHERE ?1 IS NULL OR i.folder_id = ?1",
        [folder],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(c)
}

// ---------------------------------------------------------------------------
// Culling writes
// ---------------------------------------------------------------------------

/// Runs `sql` with `(value, id)` for every id in one transaction; errors if any id is unknown.
fn update_each(conn: &mut Connection, ids: &[ImageId], sql: &str, value: Value) -> AppResult<()> {
    let tx = conn.transaction()?;
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
    update_each(conn, ids, "UPDATE images SET pick = ?1 WHERE id = ?2", Value::Text(pick.as_str().into()))
}

pub fn set_color_label(conn: &mut Connection, ids: &[ImageId], label: Option<ColorLabel>) -> AppResult<()> {
    let value = label.map_or(Value::Null, |l| Value::Text(l.as_str().into()));
    update_each(conn, ids, "UPDATE images SET color_label = ?1 WHERE id = ?2", value)
}

/// Adds or removes a tag as the user. Removing an auto tag suppresses it rather than
/// deleting it, so re-running analysis does not bring it back.
pub fn set_user_tag(conn: &mut Connection, ids: &[ImageId], tag: CullTag, present: bool) -> AppResult<()> {
    let tx = conn.transaction()?;
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
    let changed = conn.execute(
        "INSERT INTO adjustments (image_id, params_json, process_version, updated_at, neutral)
         SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM images WHERE id = ?1)
         ON CONFLICT(image_id) DO UPDATE SET
             params_json = excluded.params_json,
             process_version = excluded.process_version,
             updated_at = excluded.updated_at,
             neutral = excluded.neutral",
        params![id, json, adj.process_version, now_ms(), neutral],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("image {id}")));
    }
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
    let tx = conn.transaction()?;
    if let Some(m) = meta {
        tx.prepare_cached(
            "UPDATE images SET
                 camera_make = COALESCE(?2, camera_make),
                 camera_model = COALESCE(?3, camera_model),
                 sensor_layout = COALESCE(?4, sensor_layout),
                 lens = ?5, captured_at_ms = ?6, iso = ?7, shutter_s = ?8, aperture = ?9,
                 focal_length_mm = ?10, width = ?11, height = ?12, orientation = ?13
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
    tx.commit()?;
    Ok(())
}

/// Resets `ids` to `pending` (clearing paths/errors) so the pipeline redoes them.
/// Atomic; unknown ids fail the batch with `not_found`. Returns the old cache paths
/// so the caller can delete them.
pub fn reset_thumbnails(conn: &mut Connection, ids: &[ImageId]) -> AppResult<Vec<String>> {
    let tx = conn.transaction()?;
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
pub fn list_burst_groups(conn: &Connection, folder: Option<FolderId>) -> AppResult<Vec<BurstGroup>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.started_at_ms, b.ended_at_ms, b.keeper_image_id, i.id
         FROM burst_groups b JOIN images i ON i.burst_group_id = b.id
         WHERE ?1 IS NULL OR b.id IN (SELECT burst_group_id FROM images WHERE folder_id = ?1)
         ORDER BY b.started_at_ms, b.id, i.captured_at_ms, i.file_name, i.id",
    )?;
    let rows = stmt.query_map([folder], |r| {
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
/// Unanalyzed images are skipped; with `only_unset`, so are images already flagged
/// (`pick != unflagged`) or rated (`rating != 0`). Atomic; unknown ids fail with `not_found`.
pub fn apply_suggestions(
    conn: &mut Connection,
    ids: &[ImageId],
    only_unset: bool,
) -> AppResult<ApplySuggestionsResult> {
    let tx = conn.transaction()?;
    let mut applied = 0;
    {
        let mut stmt = tx.prepare(
            "UPDATE images SET
                 rating = (SELECT suggested_rating FROM quality_scores WHERE image_id = ?1),
                 pick = (SELECT suggested_pick FROM quality_scores WHERE image_id = ?1)
             WHERE id = ?1 AND EXISTS (SELECT 1 FROM quality_scores WHERE image_id = ?1)
               AND (?2 = 0 OR (pick = 'unflagged' AND rating = 0))",
        )?;
        for &id in ids {
            ensure_image(&tx, id)?;
            applied += stmt.execute(params![id, only_unset])? as u32;
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
    let mut stmt = conn.prepare_cached("SELECT rating, pick, color_label FROM images WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        let snap = stmt
            .query_row([id], |r| {
                Ok(CullSnapshot {
                    image_id: id,
                    rating: r.get(0)?,
                    pick: enum_col(r, 1, PickFlag::parse)?,
                    color_label: opt_enum_col(r, 2, ColorLabel::parse)?,
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
    if let Some(s) = snapshots.iter().find(|s| s.rating > 5) {
        return Err(AppError::invalid(format!("rating {} is outside 0..=5", s.rating)));
    }
    let tx = conn.transaction()?;
    let mut changed = Vec::new();
    {
        let mut stmt = tx.prepare(
            "UPDATE images SET rating = ?2, pick = ?3, color_label = ?4
             WHERE id = ?1 AND (rating IS NOT ?2 OR pick IS NOT ?3 OR color_label IS NOT ?4)",
        )?;
        for s in snapshots {
            ensure_image(&tx, s.image_id)?;
            let label = s.color_label.map(|l| l.as_str());
            if stmt.execute(params![s.image_id, s.rating, s.pick.as_str(), label])? > 0
                && !changed.contains(&s.image_id)
            {
                changed.push(s.image_id);
            }
        }
    }
    tx.commit()?;
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
            CameraInfo { make: CameraMake::Fujifilm, model: Some("X-T5".into()), sensor_layout: SensorLayout::XTrans }
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
        let r = apply_suggestions(&mut conn, &[1, 2, 3], false).unwrap();
        assert_eq!(r, ApplySuggestionsResult { applied: 2, skipped: 1 });
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
            XmpSyncState { dirty: false, synced_at_ms: Some(5), error: Some("x".into()) }
        );

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
                CullSnapshot { image_id: 2, rating: 5, pick: PickFlag::Pick, color_label: None },
                CullSnapshot { image_id: 1, rating: 2, pick: PickFlag::Unflagged, color_label: Some(ColorLabel::Red) },
            ]
        );
        assert_eq!(cull_snapshot(&conn, &[1, 99]).unwrap_err().kind, ErrorKind::NotFound);

        set_rating(&mut conn, &[1, 2], 0).unwrap();
        set_pick(&mut conn, &[1], PickFlag::Reject).unwrap();
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();

        // Unknown id: nothing restored.
        let mut bad = before.clone();
        bad.push(CullSnapshot { image_id: 99, rating: 0, pick: PickFlag::Unflagged, color_label: None });
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
        let prefs = UiPrefs { last_export_folder: Some("/Users/me/Exports".into()) };
        set_ui_prefs(&conn, &prefs).unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), prefs);
        let empty = UiPrefs { last_export_folder: Some(String::new()) };
        assert_eq!(set_ui_prefs(&conn, &empty).unwrap_err().kind, ErrorKind::InvalidArgument);
        // Unknown / missing fields from other versions are tolerated.
        set_meta(&conn, UI_PREFS_KEY, r#"{"futureThing":1}"#).unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), UiPrefs::default());
        set_meta(&conn, UI_PREFS_KEY, "not json").unwrap();
        assert_eq!(ui_prefs(&conn).unwrap(), UiPrefs::default());
    }
}
