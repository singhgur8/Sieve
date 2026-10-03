//! Catalog SQL of the analysis worker (reads the queue, writes measurements, scores,
//! auto tags and burst groups). Command-side reads live in `db::repo`.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::{AutoTag, ImageMetrics, Scored, MODEL_VERSION};
use crate::db::now_ms;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{AnalysisScope, AnalysisStatus, BurstGroupId, CullTag, ImageId};

/// SQL predicate (over `t` = thumbnails, `a` = image_analysis, `?1` = model version) for
/// "has a usable preview and needs (re)analysis".
const NEEDS: &str = "(a.image_id IS NULL OR a.status = 'queued' OR a.model_version IS NOT ?1
                      OR COALESCE(a.analyzed_at, 0) < COALESCE(t.extracted_at, 0))";
const READY: &str = "(t.status = 'ready' AND t.preview_path IS NOT NULL)";

/// Next images to measure: `(id, preview_path)`, oldest id first.
pub fn needs_analysis(conn: &Connection, limit: u32) -> AppResult<Vec<(ImageId, String)>> {
    let sql = format!(
        "SELECT t.image_id, t.preview_path FROM thumbnails t
         LEFT JOIN image_analysis a ON a.image_id = t.image_id
         WHERE {READY} AND {NEEDS} ORDER BY t.image_id LIMIT ?2"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![MODEL_VERSION, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn count_needs(conn: &Connection) -> AppResult<u32> {
    let sql = format!(
        "SELECT COUNT(*) FROM thumbnails t LEFT JOIN image_analysis a ON a.image_id = t.image_id
         WHERE {READY} AND {NEEDS}"
    );
    Ok(conn.query_row(&sql, [MODEL_VERSION], |r| r.get(0))?)
}

pub fn analysis_status(conn: &Connection, running: bool) -> AppResult<AnalysisStatus> {
    let sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM(NOT COALESCE({READY}, 0)), 0),
                COALESCE(SUM(COALESCE({READY}, 0) AND {NEEDS}), 0),
                COALESCE(SUM(COALESCE({READY}, 0) AND NOT {NEEDS} AND a.status = 'done'), 0),
                COALESCE(SUM(COALESCE({READY}, 0) AND NOT {NEEDS} AND a.status = 'failed'), 0)
         FROM images i
         LEFT JOIN thumbnails t ON t.image_id = i.id
         LEFT JOIN image_analysis a ON a.image_id = i.id"
    );
    let (total, waiting, pending, analyzed, failed) = conn.query_row(&sql, [MODEL_VERSION], |r| {
        Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?, r.get::<_, u32>(3)?, r.get::<_, u32>(4)?))
    })?;
    Ok(AnalysisStatus { total, analyzed, failed, pending, waiting, running })
}

/// Marks rows for re-measurement per `scope` (`Pending`/`Rescore` are no-ops here).
/// Atomic; unknown image / folder ids fail with `not_found`.
pub fn queue_scope(conn: &mut Connection, scope: &AnalysisScope) -> AppResult<()> {
    const UPSERT: &str = "INSERT INTO image_analysis (image_id, status) SELECT id, 'queued' FROM images WHERE {W}
                          ON CONFLICT(image_id) DO UPDATE SET status = 'queued'";
    let tx = conn.transaction()?;
    match scope {
        AnalysisScope::Pending | AnalysisScope::Rescore => {}
        AnalysisScope::Images { ids } => {
            let sql = UPSERT.replace("{W}", "id = ?1");
            let mut stmt = tx.prepare(&sql)?;
            for &id in ids {
                if stmt.execute([id])? == 0 {
                    return Err(AppError::not_found(format!("image {id}")));
                }
            }
        }
        AnalysisScope::Folder { folder_id } => {
            let exists: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM folders WHERE id = ?1)", [folder_id], |r| r.get(0))?;
            if !exists {
                return Err(AppError::not_found(format!("folder {folder_id}")));
            }
            tx.execute(&UPSERT.replace("{W}", "folder_id = ?1"), [folder_id])?;
        }
        AnalysisScope::Project { project_id } => {
            crate::db::projects::require_project(&tx, *project_id)?;
            tx.execute(
                &UPSERT.replace("{W}", "folder_id IN (SELECT id FROM folders WHERE project_id = ?1)"),
                [project_id],
            )?;
        }
        AnalysisScope::All => {
            tx.execute(&UPSERT.replace("{W}", "1"), [])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// `analyzed_at` never earlier than the preview's extraction time (clock skew would
/// otherwise re-queue the image forever).
fn stamp(tx: &Transaction, id: ImageId) -> AppResult<i64> {
    let extracted: Option<i64> = tx
        .query_row("SELECT extracted_at FROM thumbnails WHERE image_id = ?1", [id], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(now_ms().max(extracted.unwrap_or(0)))
}

/// Opens an immediate (write-locked) transaction for the results of image `id`, or `None`
/// when its `images` row is gone (removed mid-pass, e.g. by `remove_project`). The write
/// lock makes the existence check and the writes atomic against concurrent removals.
fn begin_for_image(conn: &mut Connection, id: ImageId) -> AppResult<Option<Transaction<'_>>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM images WHERE id = ?1)", [id], |r| r.get(0))?;
    Ok(exists.then_some(tx))
}

/// One transaction: measurements + scores + faces + auto tags of a measured image.
/// Returns `false` (nothing written) when the image was removed from the catalog.
pub fn record_measured(conn: &mut Connection, id: ImageId, metrics: &ImageMetrics, scored: &Scored) -> AppResult<bool> {
    let Some(tx) = begin_for_image(conn, id)? else { return Ok(false) };
    let now = stamp(&tx, id)?;
    tx.prepare_cached(
        "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, error, phash, faces_json, metrics_json)
         VALUES (?1, 'done', ?2, ?3, NULL, ?4, ?5, ?6)
         ON CONFLICT(image_id) DO UPDATE SET status = 'done', model_version = ?2, analyzed_at = ?3, error = NULL,
             phash = ?4, faces_json = ?5, metrics_json = ?6",
    )?
    .execute(params![
        id,
        MODEL_VERSION,
        now,
        metrics.phash as i64,
        serde_json::to_string(&scored.faces)?,
        serde_json::to_string(metrics)?
    ])?;
    write_scored(&tx, id, scored, now)?;
    tx.commit()?;
    Ok(true)
}

/// Records a failed measurement; stale scores and auto tags of the image are removed.
/// Returns `false` (nothing written) when the image was removed from the catalog.
pub fn record_failed(conn: &mut Connection, id: ImageId, reason: &str) -> AppResult<bool> {
    let Some(tx) = begin_for_image(conn, id)? else { return Ok(false) };
    let now = stamp(&tx, id)?;
    tx.prepare_cached(
        "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at, error)
         VALUES (?1, 'failed', ?2, ?3, ?4)
         ON CONFLICT(image_id) DO UPDATE SET status = 'failed', model_version = ?2, analyzed_at = ?3, error = ?4,
             phash = NULL, faces_json = NULL, metrics_json = NULL",
    )?
    .execute(params![id, MODEL_VERSION, now, reason])?;
    tx.execute("DELETE FROM quality_scores WHERE image_id = ?1", [id])?;
    set_auto_tags(&tx, id, &[], &[])?;
    tx.commit()?;
    Ok(true)
}

/// Writes `quality_scores`, `faces_json` and the per-image auto tags (everything except
/// `duplicate_burst`, which [`write_bursts`] owns).
pub fn write_scored(tx: &Transaction, id: ImageId, s: &Scored, now: i64) -> AppResult<()> {
    let q = &s.quality;
    tx.prepare_cached(
        "INSERT OR REPLACE INTO quality_scores (image_id, overall, face_sharpness, global_sharpness, eyes_open,
             composition, face_count, clipped_highlights_pct, clipped_shadows_pct, mean_luma, model_version,
             analyzed_at, suggested_rating, suggested_pick, reasons_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?
    .execute(params![
        id,
        q.overall,
        q.face_sharpness,
        q.global_sharpness,
        q.eyes_open,
        q.composition,
        q.face_count,
        q.exposure.clipped_highlights_pct,
        q.exposure.clipped_shadows_pct,
        q.exposure.mean_luma,
        q.model_version,
        now,
        q.suggested_rating,
        q.suggested_pick.as_str(),
        serde_json::to_string(&q.reasons)?,
    ])?;
    tx.prepare_cached("UPDATE image_analysis SET faces_json = ?2 WHERE image_id = ?1")?
        .execute(params![id, serde_json::to_string(&s.faces)?])?;
    set_auto_tags(tx, id, &s.tags, &[CullTag::DuplicateBurst])?;
    Ok(())
}

/// Makes the image's auto tags equal `tags`, leaving tags in `keep` alone:
/// upserts emitted tags as `auto` (new or confidence update), deletes unsuppressed auto
/// tags that are no longer emitted, never touches suppressed rows or `user` rows.
pub fn set_auto_tags(tx: &Transaction, id: ImageId, tags: &[AutoTag], keep: &[CullTag]) -> AppResult<()> {
    let mut upsert = tx.prepare_cached(
        "INSERT INTO image_tags (image_id, tag, source, confidence, suppressed) VALUES (?1, ?2, 'auto', ?3, 0)
         ON CONFLICT(image_id, tag) DO UPDATE SET confidence = excluded.confidence
         WHERE image_tags.source = 'auto' AND image_tags.suppressed = 0",
    )?;
    for t in tags {
        upsert.execute(params![id, t.tag.as_str(), t.confidence.clamp(0.0, 1.0)])?;
    }
    let existing: Vec<String> = tx
        .prepare_cached("SELECT tag FROM image_tags WHERE image_id = ?1 AND source = 'auto' AND suppressed = 0")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut delete = tx.prepare_cached("DELETE FROM image_tags WHERE image_id = ?1 AND tag = ?2")?;
    for tag in existing {
        let emitted = tags.iter().any(|t| t.tag.as_str() == tag);
        let kept = keep.iter().any(|k| k.as_str() == tag);
        if !emitted && !kept {
            delete.execute(params![id, tag])?;
        }
    }
    Ok(())
}

/// One analyzed image for the rescore pass.
pub struct StoredAnalysis {
    pub id: ImageId,
    pub folder_id: i64,
    pub captured_at_ms: Option<i64>,
    pub metrics: ImageMetrics,
}

/// Every image analyzed with the current model version (metrics parse errors skipped).
pub fn load_analyzed(conn: &Connection) -> AppResult<Vec<StoredAnalysis>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.folder_id, i.captured_at_ms, a.metrics_json FROM image_analysis a
         JOIN images i ON i.id = a.image_id
         WHERE a.status = 'done' AND a.model_version = ?1 AND a.metrics_json IS NOT NULL
         ORDER BY i.id",
    )?;
    let rows = stmt.query_map([MODEL_VERSION], |r| {
        Ok((r.get::<_, ImageId>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, String>(3)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, folder_id, captured_at_ms, json) = row?;
        match serde_json::from_str(&json) {
            Ok(metrics) => out.push(StoredAnalysis { id, folder_id, captured_at_ms, metrics }),
            Err(e) => eprintln!("[analysis] image {id}: unreadable metrics ({e}); skipped in rescore"),
        }
    }
    Ok(out)
}

/// A burst to persist: members in capture order, keeper, time span, and per-member
/// confidence for `duplicate_burst`.
pub struct BurstRow {
    pub members: Vec<ImageId>,
    pub keeper: ImageId,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
}

/// Replaces all burst groups; sets `duplicate_burst` on non-keepers and removes it
/// (auto, unsuppressed) from every other image. Returns the number of groups.
pub fn write_bursts(tx: &Transaction, bursts: &[BurstRow]) -> AppResult<u32> {
    tx.execute("UPDATE images SET burst_group_id = NULL WHERE burst_group_id IS NOT NULL", [])?;
    tx.execute("DELETE FROM burst_groups", [])?;
    let mut dup: HashMap<ImageId, bool> = HashMap::new();
    {
        let mut insert =
            tx.prepare("INSERT INTO burst_groups (started_at_ms, ended_at_ms, keeper_image_id) VALUES (?1, ?2, ?3)")?;
        let mut member = tx.prepare("UPDATE images SET burst_group_id = ?2 WHERE id = ?1")?;
        for b in bursts {
            insert.execute(params![b.started_at_ms, b.ended_at_ms, b.keeper])?;
            let gid = tx.last_insert_rowid();
            for &m in &b.members {
                member.execute(params![m, gid])?;
                dup.insert(m, m != b.keeper);
            }
        }
    }
    for (&id, &is_dup) in &dup {
        if is_dup {
            set_duplicate_tag(tx, id, true)?;
        }
    }
    let stale: Vec<ImageId> = tx
        .prepare(
            "SELECT image_id FROM image_tags WHERE tag = 'duplicate_burst' AND source = 'auto' AND suppressed = 0",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in stale {
        if !dup.get(&id).copied().unwrap_or(false) {
            set_duplicate_tag(tx, id, false)?;
        }
    }
    Ok(bursts.len() as u32)
}

/// The one rule for the auto `duplicate_burst` tag (burst non-keepers carry it):
/// `true` adds it as `auto` unless a row exists (a suppressed or `user` row is left alone);
/// `false` removes an unsuppressed `auto` row (suppressed and `user` rows are left alone).
pub fn set_duplicate_tag(conn: &Connection, id: ImageId, duplicate: bool) -> AppResult<()> {
    if duplicate {
        conn.prepare_cached(
            "INSERT INTO image_tags (image_id, tag, source, confidence, suppressed)
             VALUES (?1, 'duplicate_burst', 'auto', 1.0, 0)
             ON CONFLICT(image_id, tag) DO NOTHING",
        )?
        .execute([id])?;
    } else {
        conn.prepare_cached(
            "DELETE FROM image_tags
             WHERE image_id = ?1 AND tag = 'duplicate_burst' AND source = 'auto' AND suppressed = 0",
        )?
        .execute([id])?;
    }
    Ok(())
}

/// Images the user chose as burst keepers (`set_burst_keeper`); [`crate::ml::bursts::apply_pins`]
/// honours them when bursts are regrouped.
pub fn pinned_keepers(conn: &Connection) -> AppResult<HashSet<ImageId>> {
    let mut stmt = conn.prepare_cached("SELECT image_id FROM burst_keeper_pins")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `set_burst_keeper`: makes `keeper` the keeper of burst `group` now (keeper column,
/// `duplicate_burst` on the other members, removed from the keeper) and pins it so later
/// regrouping keeps the choice (other members of the group are unpinned). Atomic.
/// Unknown group or image -> `not_found`; image not a member -> `invalid_argument`.
/// Returns the members whose tags may have changed. Suggested rating/pick are refreshed by
/// the next rescore (the command kicks one).
pub fn set_burst_keeper(conn: &mut Connection, group: BurstGroupId, keeper: ImageId) -> AppResult<Vec<ImageId>> {
    let tx = conn.transaction()?;
    let exists: bool =
        tx.query_row("SELECT EXISTS (SELECT 1 FROM burst_groups WHERE id = ?1)", [group], |r| r.get(0))?;
    if !exists {
        return Err(AppError::not_found(format!("burst group {group}")));
    }
    let member_of: Option<Option<BurstGroupId>> =
        tx.query_row("SELECT burst_group_id FROM images WHERE id = ?1", [keeper], |r| r.get(0)).optional()?;
    match member_of {
        None => return Err(AppError::not_found(format!("image {keeper}"))),
        Some(g) if g != Some(group) => {
            return Err(AppError::invalid(format!("image {keeper} is not a member of burst group {group}")))
        }
        Some(_) => {}
    }
    let members: Vec<ImageId> = tx
        .prepare("SELECT id FROM images WHERE burst_group_id = ?1 ORDER BY id")?
        .query_map([group], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    tx.execute("UPDATE burst_groups SET keeper_image_id = ?2 WHERE id = ?1", params![group, keeper])?;
    tx.execute(
        "DELETE FROM burst_keeper_pins WHERE image_id IN (SELECT id FROM images WHERE burst_group_id = ?1)",
        [group],
    )?;
    tx.execute("INSERT INTO burst_keeper_pins (image_id, pinned_at) VALUES (?1, ?2)", params![keeper, now_ms()])?;
    for &m in &members {
        set_duplicate_tag(&tx, m, m != keeper)?;
    }
    tx.commit()?;
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 1, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 1, 0, 0),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 1, 0, 0);
             INSERT INTO thumbnails (image_id, status, path, preview_path, extracted_at)
             VALUES (1, 'ready', '/c/1_512.jpg', '/c/1_2048.jpg', 100),
                    (2, 'ready', '/c/2_512.jpg', '/c/2_2048.jpg', 100);
             INSERT INTO thumbnails (image_id) VALUES (3);",
        )
        .unwrap();
    }

    fn tags(conn: &Connection, id: ImageId) -> Vec<(String, String, bool)> {
        conn.prepare("SELECT tag, source, suppressed FROM image_tags WHERE image_id = ?1 ORDER BY tag")
            .unwrap()
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn queue_and_status_buckets() {
        let mut conn = open_in_memory();
        seed(&conn);
        let s = analysis_status(&conn, false).unwrap();
        assert_eq!(s, AnalysisStatus { total: 3, analyzed: 0, failed: 0, pending: 2, waiting: 1, running: false });
        assert_eq!(needs_analysis(&conn, 10).unwrap().len(), 2);

        conn.execute_batch(&format!(
            "INSERT INTO image_analysis (image_id, status, model_version, analyzed_at) VALUES
             (1, 'done', '{MODEL_VERSION}', 200), (2, 'failed', '{MODEL_VERSION}', 200)"
        ))
        .unwrap();
        let s = analysis_status(&conn, true).unwrap();
        assert_eq!((s.analyzed, s.failed, s.pending, s.waiting, s.running), (1, 1, 0, 1, true));
        assert!(needs_analysis(&conn, 10).unwrap().is_empty());

        // Re-extracted preview and old model version are pending again.
        conn.execute("UPDATE thumbnails SET extracted_at = 300 WHERE image_id = 1", []).unwrap();
        conn.execute("UPDATE image_analysis SET model_version = 'old' WHERE image_id = 2", []).unwrap();
        assert_eq!(count_needs(&conn).unwrap(), 2);
        conn.execute("UPDATE thumbnails SET extracted_at = 100 WHERE image_id = 1", []).unwrap();
        conn.execute(&format!("UPDATE image_analysis SET model_version = '{MODEL_VERSION}'"), []).unwrap();

        // Forced scopes.
        assert_eq!(
            queue_scope(&mut conn, &AnalysisScope::Images { ids: vec![1, 99] }).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(count_needs(&conn).unwrap(), 0, "failed batch rolled back");
        queue_scope(&mut conn, &AnalysisScope::Images { ids: vec![2] }).unwrap();
        assert_eq!(needs_analysis(&conn, 10).unwrap(), vec![(2, "/c/2_2048.jpg".to_string())]);
        assert_eq!(
            queue_scope(&mut conn, &AnalysisScope::Folder { folder_id: 9 }).unwrap_err().kind,
            ErrorKind::NotFound
        );
        queue_scope(&mut conn, &AnalysisScope::Folder { folder_id: 1 }).unwrap();
        assert_eq!(count_needs(&conn).unwrap(), 2);
        let s = analysis_status(&conn, false).unwrap();
        assert_eq!(s.total, s.analyzed + s.failed + s.pending + s.waiting);
        queue_scope(&mut conn, &AnalysisScope::All).unwrap();
    }

    #[test]
    fn auto_tag_upsert_respects_user_and_suppressed_rows() {
        let conn = open_in_memory();
        seed(&conn);
        conn.execute_batch(
            "INSERT INTO image_tags (image_id, tag, source, confidence, suppressed) VALUES
             (1, 'blink', 'auto', 0.9, 1),
             (1, 'underexposed', 'user', 1.0, 0),
             (1, 'motion_blur', 'auto', 0.6, 0),
             (1, 'duplicate_burst', 'auto', 1.0, 0);",
        )
        .unwrap();
        let mut c2 = conn;
        let tx = c2.transaction().unwrap();
        let emitted = [
            AutoTag { tag: CullTag::Blink, confidence: 0.8 },
            AutoTag { tag: CullTag::Underexposed, confidence: 0.7 },
            AutoTag { tag: CullTag::MissedFocus, confidence: 0.7 },
        ];
        set_auto_tags(&tx, 1, &emitted, &[CullTag::DuplicateBurst]).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            tags(&c2, 1),
            vec![
                ("blink".into(), "auto".into(), true),            // suppressed: untouched
                ("duplicate_burst".into(), "auto".into(), false), // kept (owned by bursts)
                ("missed_focus".into(), "auto".into(), false),    // new
                ("underexposed".into(), "user".into(), false),    // user row untouched
            ]
        );
        let conf: f64 = c2
            .query_row("SELECT confidence FROM image_tags WHERE image_id = 1 AND tag = 'blink'", [], |r| r.get(0))
            .unwrap();
        assert!((conf - 0.9).abs() < 1e-6, "suppressed row not updated");
        let src: String = c2
            .query_row("SELECT source FROM image_tags WHERE image_id = 1 AND tag = 'underexposed'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(src, "user");

        // Nothing emitted: unsuppressed auto tags go, others stay.
        let tx = c2.transaction().unwrap();
        set_auto_tags(&tx, 1, &[], &[]).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            tags(&c2, 1),
            vec![("blink".into(), "auto".into(), true), ("underexposed".into(), "user".into(), false)]
        );
    }

    #[test]
    fn bursts_replace_groups_and_duplicate_tags() {
        let mut conn = open_in_memory();
        seed(&conn);
        conn.execute(
            "INSERT INTO image_tags (image_id, tag, source, suppressed) VALUES (3, 'duplicate_burst', 'auto', 0)",
            [],
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        let n = write_bursts(&tx, &[BurstRow { members: vec![1, 2], keeper: 2, started_at_ms: 10, ended_at_ms: 20 }])
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(n, 1);
        let groups = crate::db::repo::list_burst_groups(&conn, None).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].keeper_image_id, groups[0].image_ids.clone()), (Some(2), vec![1, 2]));
        assert_eq!(tags(&conn, 1), vec![("duplicate_burst".into(), "auto".into(), false)]);
        assert!(tags(&conn, 2).is_empty());
        assert!(tags(&conn, 3).is_empty(), "stale duplicate removed");

        // Regroup to nothing clears everything.
        let tx = conn.transaction().unwrap();
        write_bursts(&tx, &[]).unwrap();
        tx.commit().unwrap();
        assert!(crate::db::repo::list_burst_groups(&conn, None).unwrap().is_empty());
        assert!(tags(&conn, 1).is_empty());
    }

    #[test]
    fn set_burst_keeper_moves_duplicate_tag_and_pins() {
        let mut conn = open_in_memory();
        seed(&conn);
        let tx = conn.transaction().unwrap();
        write_bursts(&tx, &[BurstRow { members: vec![1, 2, 3], keeper: 2, started_at_ms: 10, ended_at_ms: 30 }])
            .unwrap();
        tx.commit().unwrap();
        let gid = crate::db::repo::list_burst_groups(&conn, None).unwrap()[0].id;
        // 3's duplicate tag was suppressed by the user: stays suppressed whatever happens.
        conn.execute("UPDATE image_tags SET suppressed = 1 WHERE image_id = 3", []).unwrap();
        conn.execute("UPDATE images SET xmp_dirty = 0", []).unwrap();

        assert_eq!(set_burst_keeper(&mut conn, 99, 1).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(set_burst_keeper(&mut conn, gid, 99).unwrap_err().kind, ErrorKind::NotFound);
        conn.execute("UPDATE images SET burst_group_id = NULL WHERE id = 3", []).unwrap();
        assert_eq!(set_burst_keeper(&mut conn, gid, 3).unwrap_err().kind, ErrorKind::InvalidArgument);
        conn.execute("UPDATE images SET burst_group_id = ?1 WHERE id = 3", [gid]).unwrap();

        assert_eq!(set_burst_keeper(&mut conn, gid, 1).unwrap(), vec![1, 2, 3]);
        let g = &crate::db::repo::list_burst_groups(&conn, None).unwrap()[0];
        assert_eq!(g.keeper_image_id, Some(1));
        assert!(tags(&conn, 1).is_empty(), "new keeper loses duplicate_burst");
        assert_eq!(tags(&conn, 2), vec![("duplicate_burst".into(), "auto".into(), false)], "old keeper gains it");
        assert_eq!(tags(&conn, 3), vec![("duplicate_burst".into(), "auto".into(), true)], "suppressed kept");
        let dirty: Vec<ImageId> = conn
            .prepare("SELECT id FROM images WHERE xmp_dirty = 1 ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(dirty, vec![1, 2]);
        assert_eq!(pinned_keepers(&conn).unwrap(), HashSet::from([1]));

        // Re-choosing moves the pin; a user tag on the new keeper is left alone.
        conn.execute("UPDATE image_tags SET source = 'user' WHERE image_id = 2", []).unwrap();
        set_burst_keeper(&mut conn, gid, 2).unwrap();
        assert_eq!(pinned_keepers(&conn).unwrap(), HashSet::from([2]));
        assert_eq!(tags(&conn, 2), vec![("duplicate_burst".into(), "user".into(), false)]);
        assert_eq!(tags(&conn, 1), vec![("duplicate_burst".into(), "auto".into(), false)]);
    }
}
