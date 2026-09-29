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

pub fn catalog_state(conn: &Connection, catalog_path: &str) -> AppResult<CatalogState> {
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
    })
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

        let walker = WalkDir::new(&folder).max_depth(if opts.recursive { usize::MAX } else { 1 });
        for entry in walker.into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let format = match raw::identify(path) {
                Ok(Some(f)) => f,
                Ok(None) => continue,
                Err(_) => {
                    summary.invalid += 1;
                    continue;
                }
            };
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
           EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = i.id)
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
        Some("ready") => ThumbnailState::Ready { path: r.get(20)?, width: r.get(21)?, height: r.get(22)? },
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
        tags: Vec::new(),
        quality,
        has_edits: r.get(38)?,
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

pub fn list_images(conn: &Connection, q: &ImageQuery) -> AppResult<ImagePage> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();

    let tag_list = |tags: &[CullTag], args: &mut Vec<Value>| {
        args.extend(tags.iter().map(|t| Value::Text(t.as_str().to_owned())));
        vec!["?"; tags.len()].join(",")
    };

    if !q.include_tags.is_empty() {
        let ph = tag_list(&q.include_tags, &mut args);
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
        let ph = tag_list(&q.exclude_tags, &mut args);
        clauses.push(format!(
            "NOT EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id
                         AND it.suppressed = 0 AND it.tag IN ({ph}))"
        ));
    }
    if let Some(pick) = q.pick {
        clauses.push("i.pick = ?".into());
        args.push(Value::Text(pick.as_str().into()));
    }
    if let Some(min) = q.min_rating {
        clauses.push("i.rating >= ?".into());
        args.push(Value::Integer(min.into()));
    }
    if let Some(burst) = q.burst_group_id {
        clauses.push("i.burst_group_id = ?".into());
        args.push(Value::Integer(burst));
    }
    if let Some(folder) = q.folder_id {
        clauses.push("i.folder_id = ?".into());
        args.push(Value::Integer(folder));
    }

    let where_sql = if clauses.is_empty() { String::new() } else { format!(" WHERE {}", clauses.join(" AND ")) };

    let total: u32 =
        conn.query_row(&format!("SELECT COUNT(*) FROM images i{where_sql}"), params_from_iter(args.iter()), |r| {
            r.get(0)
        })?;

    let order = match q.sort {
        ImageSort::CaptureTime => "i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        ImageSort::FileName => "i.file_name, i.id",
        ImageSort::Quality => "q.overall IS NULL, q.overall DESC, i.id",
    };
    let limit = q.limit.min(ImageQuery::MAX_LIMIT);
    let sql = format!("{ENTRY_SELECT}{where_sql} ORDER BY {order} LIMIT {limit} OFFSET {}", q.offset);
    let mut items =
        conn.prepare(&sql)?.query_map(params_from_iter(args.iter()), entry_from_row)?.collect::<Result<Vec<_>, _>>()?;
    attach_tags(conn, &mut items)?;

    Ok(ImagePage { items, total })
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

/// Stored adjustments, or neutral defaults if the image has never been edited.
pub fn get_adjustments(conn: &Connection, id: ImageId) -> AppResult<ParametricAdjustments> {
    let json: Option<String> =
        conn.query_row("SELECT params_json FROM adjustments WHERE image_id = ?1", [id], |r| r.get(0)).optional()?;
    match json {
        Some(json) => {
            // Overlay stored values on neutral defaults so JSON written before a
            // slider existed still loads.
            let mut merged = serde_json::to_value(ParametricAdjustments::default())?;
            merge_json(&mut merged, serde_json::from_str(&json)?);
            Ok(serde_json::from_value(merged)?)
        }
        None => {
            // Distinguish "unedited" from "no such image".
            get_image(conn, id)?;
            Ok(ParametricAdjustments::default())
        }
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

pub fn save_adjustments(conn: &Connection, id: ImageId, adj: &ParametricAdjustments) -> AppResult<()> {
    adj.validate().map_err(AppError::invalid)?;
    let json = serde_json::to_string(adj)?;
    let changed = conn.execute(
        "INSERT INTO adjustments (image_id, params_json, process_version, updated_at)
         SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM images WHERE id = ?1)
         ON CONFLICT(image_id) DO UPDATE SET
             params_json = excluded.params_json,
             process_version = excluded.process_version,
             updated_at = excluded.updated_at",
        params![id, json, adj.process_version, now_ms()],
    )?;
    if changed == 0 {
        return Err(AppError::not_found(format!("image {id}")));
    }
    Ok(())
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
        import_folder(conn, dir, &ImportOptions { recursive }).unwrap()
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

        let state = catalog_state(&conn, ":memory:").unwrap();
        assert_eq!(state.image_count, 4);
        assert_eq!(state.folders.len(), 1);
        assert_eq!(state.folders[0].image_count, 4);
        assert_eq!(state.shoot_type, ShootType::General);
        assert_eq!(state.burst_window_ms, 1500);
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
        let ids = all_ids(&conn);
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

        let counts = catalog_state(&conn, "").unwrap().tag_counts;
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

        let q = ImageQuery { min_rating: Some(4), pick: Some(PickFlag::Reject), ..Default::default() };
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
            lut: Some(LutRef { path: "/luts/film.cube".into(), amount: 60.0 }),
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
    fn catalog_settings() {
        let conn = open_in_memory();
        set_shoot_type(&conn, ShootType::Wedding).unwrap();
        set_burst_window(&conn, 800).unwrap();
        assert!(set_burst_window(&conn, 5).is_err());
        let s = catalog_state(&conn, "").unwrap();
        assert_eq!((s.shoot_type, s.burst_window_ms), (ShootType::Wedding, 800));
    }
}
