//! Catalog SQL for scenes (migration 0007). Every write of `scenes`, `images.scene_id`,
//! `images.scene_anchor` and `scene_features` goes through here, which keeps the invariants:
//! - an image is in at most one scene (`images.scene_id`); scenes are never empty (a scene
//!   whose last member leaves is deleted);
//! - anchors (`scene_anchor = 1`) are members, at most `Scene::MAX_ANCHORS` per scene; an image
//!   leaving its scene loses its anchor flag;
//! - `scenes.folder_id` / `started_at_ms` / `ended_at_ms` are derived from the members;
//! - user edits of membership make a scene `manual`; anchor edits keep the method.
//!
//! Batch functions are atomic: unknown ids -> `not_found`, bad input -> `invalid_argument`,
//! nothing written.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::{DetectFrame, MatchImage, SceneFeatures, FEATURES_VERSION};
use crate::db::projects::FolderScope;
use crate::db::{now_ms, repo};
use crate::develop::SourceImage;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{FolderId, ImageId, Scene, SceneId, SceneMethod};

/// Capture order of members (images without a capture time last, by file name).
const MEMBER_ORDER: &str = "captured_at_ms IS NULL, captured_at_ms, file_name, id";

fn dedup(ids: &[ImageId]) -> Vec<ImageId> {
    let mut out: Vec<ImageId> = Vec::with_capacity(ids.len());
    for &id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn require_images(conn: &Connection, ids: &[ImageId]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached("SELECT 1 FROM images WHERE id = ?1")?;
    for &id in ids {
        if !stmt.exists([id])? {
            return Err(AppError::not_found(format!("image {id}")));
        }
    }
    Ok(())
}

fn require_scene(conn: &Connection, id: SceneId) -> AppResult<()> {
    if conn.prepare_cached("SELECT 1 FROM scenes WHERE id = ?1")?.exists([id])? {
        Ok(())
    } else {
        Err(AppError::not_found(format!("scene {id}")))
    }
}

/// Members of `id` in capture order, with their anchor flags.
fn members(conn: &Connection, id: SceneId) -> AppResult<Vec<(ImageId, bool)>> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT id, scene_anchor FROM images WHERE scene_id = ?1 ORDER BY {MEMBER_ORDER}"))?;
    let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn load(conn: &Connection, id: SceneId) -> AppResult<Option<Scene>> {
    let row = conn
        .query_row(
            "SELECT folder_id, started_at_ms, ended_at_ms, method, created_at, updated_at FROM scenes WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, Option<FolderId>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((folder_id, started_at_ms, ended_at_ms, method, created_at_ms, updated_at_ms)) = row else {
        return Ok(None);
    };
    let method = SceneMethod::parse(&method).ok_or_else(|| AppError::internal(format!("bad scene method {method}")))?;
    let members = members(conn, id)?;
    Ok(Some(Scene {
        id,
        folder_id,
        started_at_ms,
        ended_at_ms,
        anchor_ids: members.iter().filter(|(_, a)| *a).map(|(i, _)| *i).collect(),
        image_ids: members.into_iter().map(|(i, _)| i).collect(),
        method,
        created_at_ms,
        updated_at_ms,
    }))
}

pub fn get_scene(conn: &Connection, id: SceneId) -> AppResult<Scene> {
    load(conn, id)?.ok_or_else(|| AppError::not_found(format!("scene {id}")))
}

/// SQL predicate on `s.id`: scenes with a member in `scope`.
fn scene_scope(scope: &FolderScope) -> String {
    match scope.is_all() {
        true => "1".into(),
        false => format!(
            "s.id IN (SELECT scene_id FROM images WHERE {} AND scene_id IS NOT NULL)",
            scope.predicate("folder_id")
        ),
    }
}

/// Scenes with at least one member in `scope` (a folder, a project's folders, or all scenes),
/// in capture order (scenes without capture times last).
pub fn list_scenes(conn: &Connection, scope: impl Into<FolderScope>) -> AppResult<Vec<Scene>> {
    let scope: FolderScope = scope.into();
    let ids: Vec<SceneId> = {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT s.id FROM scenes s
             WHERE {}
             ORDER BY s.started_at_ms IS NULL, s.started_at_ms, s.id",
            scene_scope(&scope)
        ))?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    ids.into_iter().filter_map(|id| load(conn, id).transpose()).collect()
}

/// Recomputes the derived columns of `id` (and `updated_at` if `touch`); deletes it when empty.
fn refresh(conn: &Connection, id: SceneId, touch: bool, now: i64) -> AppResult<()> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM images WHERE scene_id = ?1", [id], |r| r.get(0))?;
    if count == 0 {
        conn.execute("DELETE FROM scenes WHERE id = ?1", [id])?;
        return Ok(());
    }
    conn.execute(
        "UPDATE scenes SET
             folder_id = (SELECT CASE WHEN COUNT(DISTINCT folder_id) = 1 THEN MIN(folder_id) END
                          FROM images WHERE scene_id = ?1),
             started_at_ms = (SELECT MIN(captured_at_ms) FROM images WHERE scene_id = ?1),
             ended_at_ms = (SELECT MAX(captured_at_ms) FROM images WHERE scene_id = ?1),
             updated_at = CASE WHEN ?2 THEN ?3 ELSE updated_at END
         WHERE id = ?1",
        params![id, touch, now],
    )?;
    Ok(())
}

fn insert_scene(conn: &Connection, method: SceneMethod, now: i64) -> AppResult<SceneId> {
    conn.execute(
        "INSERT INTO scenes (method, created_at, updated_at) VALUES (?1, ?2, ?2)",
        params![method.as_str(), now],
    )?;
    Ok(conn.last_insert_rowid())
}

fn set_method(conn: &Connection, id: SceneId, method: SceneMethod) -> AppResult<()> {
    conn.execute("UPDATE scenes SET method = ?2 WHERE id = ?1", params![id, method.as_str()])?;
    Ok(())
}

/// Moves `ids` into scene `id` (images changing scene lose their anchor flag) and refreshes
/// the scenes they left (deleting emptied ones). Does not refresh `id` itself.
fn assign(conn: &Connection, id: SceneId, ids: &[ImageId], now: i64) -> AppResult<()> {
    let mut previous: Vec<SceneId> = Vec::new();
    {
        let mut get = conn.prepare_cached("SELECT scene_id FROM images WHERE id = ?1")?;
        let mut set = conn
            .prepare_cached("UPDATE images SET scene_id = ?2, scene_anchor = 0 WHERE id = ?1 AND scene_id IS NOT ?2")?;
        for &image in ids {
            let old: Option<SceneId> = get.query_row([image], |r| r.get(0))?;
            if let Some(old) = old.filter(|o| *o != id) {
                if !previous.contains(&old) {
                    previous.push(old);
                }
            }
            set.execute(params![image, id])?;
        }
    }
    for old in previous {
        refresh(conn, old, true, now)?;
    }
    Ok(())
}

/// Removes `ids` from whatever scene they are in (refreshing / deleting those scenes).
fn unassign(conn: &Connection, ids: &[ImageId], now: i64) -> AppResult<()> {
    let mut previous: Vec<SceneId> = Vec::new();
    for &image in ids {
        let old: Option<SceneId> =
            conn.query_row("SELECT scene_id FROM images WHERE id = ?1", [image], |r| r.get(0)).optional()?.flatten();
        if let Some(old) = old {
            if !previous.contains(&old) {
                previous.push(old);
            }
        }
        conn.execute("UPDATE images SET scene_id = NULL, scene_anchor = 0 WHERE id = ?1", [image])?;
    }
    for old in previous {
        refresh(conn, old, true, now)?;
    }
    Ok(())
}

/// Sets the anchor flags of scene `id` to exactly `anchors` (must be members).
fn write_anchors(conn: &Connection, id: SceneId, anchors: &[ImageId]) -> AppResult<()> {
    conn.execute("UPDATE images SET scene_anchor = 0 WHERE scene_id = ?1 AND scene_anchor = 1", [id])?;
    for &a in anchors {
        conn.execute("UPDATE images SET scene_anchor = 1 WHERE id = ?1 AND scene_id = ?2", params![a, id])?;
    }
    Ok(())
}

/// New `manual` scene with `ids` (moved out of their current scenes). `ids` non-empty.
pub fn create_scene(conn: &mut Connection, ids: &[ImageId]) -> AppResult<Scene> {
    let ids = dedup(ids);
    if ids.is_empty() {
        return Err(AppError::invalid("a scene needs at least one image"));
    }
    let tx = conn.savepoint()?;
    require_images(&tx, &ids)?;
    let now = now_ms();
    let id = insert_scene(&tx, SceneMethod::Manual, now)?;
    assign(&tx, id, &ids, now)?;
    refresh(&tx, id, true, now)?;
    let scene = get_scene(&tx, id)?;
    tx.commit()?;
    Ok(scene)
}

/// Replaces the members of `id` with `ids` (non-empty; images from other scenes are moved in,
/// members not listed leave). The scene becomes `manual`; anchors that stay members are kept.
pub fn set_scene_members(conn: &mut Connection, id: SceneId, ids: &[ImageId]) -> AppResult<Scene> {
    let ids = dedup(ids);
    if ids.is_empty() {
        return Err(AppError::invalid("a scene needs at least one image (use delete_scene)"));
    }
    let tx = conn.savepoint()?;
    require_scene(&tx, id)?;
    require_images(&tx, &ids)?;
    let now = now_ms();
    let leaving: Vec<ImageId> = members(&tx, id)?.into_iter().map(|(i, _)| i).filter(|i| !ids.contains(i)).collect();
    unassign(&tx, &leaving, now)?;
    // `unassign` may have deleted the scene if every member left; recreate the row if so.
    let exists = tx.prepare_cached("SELECT 1 FROM scenes WHERE id = ?1")?.exists([id])?;
    if !exists {
        tx.execute(
            "INSERT INTO scenes (id, method, created_at, updated_at) VALUES (?1, 'manual', ?2, ?2)",
            params![id, now],
        )?;
    }
    assign(&tx, id, &ids, now)?;
    set_method(&tx, id, SceneMethod::Manual)?;
    refresh(&tx, id, true, now)?;
    let scene = get_scene(&tx, id)?;
    tx.commit()?;
    Ok(scene)
}

/// Sets the anchors of `id` (0..=`Scene::MAX_ANCHORS` distinct members; `[]` clears).
pub fn set_scene_anchors(conn: &mut Connection, id: SceneId, anchor_ids: &[ImageId]) -> AppResult<Scene> {
    let anchors = dedup(anchor_ids);
    if anchors.len() > Scene::MAX_ANCHORS {
        return Err(AppError::invalid(format!("at most {} anchors per scene", Scene::MAX_ANCHORS)));
    }
    let tx = conn.savepoint()?;
    require_scene(&tx, id)?;
    require_images(&tx, &anchors)?;
    let member_ids: Vec<ImageId> = members(&tx, id)?.into_iter().map(|(i, _)| i).collect();
    if let Some(a) = anchors.iter().find(|a| !member_ids.contains(a)) {
        return Err(AppError::invalid(format!("image {a} is not a member of scene {id}")));
    }
    write_anchors(&tx, id, &anchors)?;
    refresh(&tx, id, true, now_ms())?;
    let scene = get_scene(&tx, id)?;
    tx.commit()?;
    Ok(scene)
}

/// Merges `ids` (>= 2 distinct scenes) into the first; the others are deleted. The result is
/// `manual`; anchors are kept in `ids` order (then capture order) up to `Scene::MAX_ANCHORS`.
pub fn merge_scenes(conn: &mut Connection, ids: &[SceneId]) -> AppResult<Scene> {
    let ids = dedup(ids);
    if ids.len() < 2 {
        return Err(AppError::invalid("merge needs at least two scenes"));
    }
    let tx = conn.savepoint()?;
    for &s in &ids {
        require_scene(&tx, s)?;
    }
    let now = now_ms();
    let into = ids[0];
    let mut anchors: Vec<ImageId> = Vec::new();
    let mut moved: Vec<ImageId> = Vec::new();
    for &s in &ids {
        for (image, anchor) in members(&tx, s)? {
            if anchor && anchors.len() < Scene::MAX_ANCHORS {
                anchors.push(image);
            }
            if s != into {
                moved.push(image);
            }
        }
    }
    assign(&tx, into, &moved, now)?;
    write_anchors(&tx, into, &anchors)?;
    set_method(&tx, into, SceneMethod::Manual)?;
    refresh(&tx, into, true, now)?;
    let scene = get_scene(&tx, into)?;
    tx.commit()?;
    Ok(scene)
}

/// Splits `id` before member `first_image_id` (capture order): it and later members move to a
/// new scene. Anchors follow their images. Both scenes become `manual`. Returns `[id, new]`.
/// `first_image_id` must be a member other than the first.
pub fn split_scene(conn: &mut Connection, id: SceneId, first_image_id: ImageId) -> AppResult<Vec<Scene>> {
    let tx = conn.savepoint()?;
    require_scene(&tx, id)?;
    let list = members(&tx, id)?;
    let Some(pos) = list.iter().position(|(i, _)| *i == first_image_id) else {
        return Err(AppError::invalid(format!("image {first_image_id} is not a member of scene {id}")));
    };
    if pos == 0 {
        return Err(AppError::invalid("cannot split before the first member"));
    }
    let now = now_ms();
    let new_id = insert_scene(&tx, SceneMethod::Manual, now)?;
    for (image, _) in &list[pos..] {
        // Direct move keeps the anchor flag (anchors follow their images).
        tx.execute("UPDATE images SET scene_id = ?2 WHERE id = ?1", params![image, new_id])?;
    }
    set_method(&tx, id, SceneMethod::Manual)?;
    refresh(&tx, id, true, now)?;
    refresh(&tx, new_id, true, now)?;
    let out = vec![get_scene(&tx, id)?, get_scene(&tx, new_id)?];
    tx.commit()?;
    Ok(out)
}

/// Deletes scene `id`; its members become unassigned.
pub fn delete_scene(conn: &mut Connection, id: SceneId) -> AppResult<()> {
    let tx = conn.savepoint()?;
    require_scene(&tx, id)?;
    tx.execute("UPDATE images SET scene_id = NULL, scene_anchor = 0 WHERE scene_id = ?1", [id])?;
    tx.execute("DELETE FROM scenes WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

/// Images scene detection considers in `scope` (a folder, a project's folders, or all), in
/// detection order, with current features (stale/missing -> `None`). Members of manual scenes
/// are excluded unless `replace_manual`. `preview_path` only for ready thumbnails.
pub fn detection_frames(
    conn: &Connection,
    scope: impl Into<FolderScope>,
    replace_manual: bool,
) -> AppResult<Vec<DetectFrame>> {
    let scope: FolderScope = scope.into();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT i.id, i.folder_id, i.captured_at_ms, i.file_name, i.burst_group_id,
                CASE WHEN t.status = 'ready' THEN t.preview_path END,
                CASE WHEN f.version = ?3 AND (t.extracted_at IS NULL OR f.computed_at >= t.extracted_at)
                     THEN f.features_json END
         FROM images i
         LEFT JOIN thumbnails t ON t.image_id = i.id
         LEFT JOIN scene_features f ON f.image_id = i.id
         LEFT JOIN scenes s ON s.id = i.scene_id
         WHERE ({}) AND (?2 OR s.method IS NULL OR s.method <> 'manual')
         ORDER BY i.folder_id, i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id",
        scope.predicate("i.folder_id")
    ))?;
    let rows = stmt.query_map(params![Option::<i64>::None, replace_manual, FEATURES_VERSION], |r| {
        let features: Option<String> = r.get(6)?;
        Ok(DetectFrame {
            id: r.get(0)?,
            folder_id: r.get(1)?,
            captured_at_ms: r.get(2)?,
            file_name: r.get(3)?,
            burst_group_id: r.get(4)?,
            preview_path: r.get::<_, Option<String>>(5)?.map(Into::into),
            features: features.and_then(|j| serde_json::from_str::<SceneFeatures>(&j).ok()),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Stores freshly computed features (version [`FEATURES_VERSION`], `computed_at` = now).
pub fn save_features(conn: &mut Connection, items: &[(ImageId, SceneFeatures)]) -> AppResult<()> {
    let tx = conn.savepoint()?;
    let now = now_ms();
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO scene_features (image_id, version, features_json, computed_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(image_id) DO UPDATE SET version = excluded.version,
                 features_json = excluded.features_json, computed_at = excluded.computed_at",
        )?;
        for (id, f) in items {
            // Images deleted meanwhile are skipped (FK would fail).
            if tx.prepare_cached("SELECT 1 FROM images WHERE id = ?1")?.exists([id])? {
                stmt.execute(params![id, FEATURES_VERSION, serde_json::to_string(f)?, now])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Detection result writer. In one transaction: deletes the `auto` scenes (and `manual` ones
/// if `replace_manual`) with a member in `scope`, then creates one `auto` scene per group.
/// Anchor flags of regrouped images survive (up to `Scene::MAX_ANCHORS` per new scene, capture
/// order), and so does the edit plan: a new scene containing an old scene's representative
/// takes over its `representative_*` and `applied_*` (user choice first, then an applied one).
/// Returns `list_scenes(scope)`.
pub fn replace_scenes(
    conn: &mut Connection,
    scope: impl Into<FolderScope>,
    groups: &[Vec<ImageId>],
    replace_manual: bool,
) -> AppResult<Vec<Scene>> {
    let scope: FolderScope = scope.into();
    let tx = conn.savepoint()?;
    let now = now_ms();
    let all: Vec<ImageId> = groups.iter().flatten().copied().collect();
    require_images(&tx, &all)?;
    if dedup(&all).len() != all.len() {
        return Err(AppError::internal("scene groups overlap"));
    }
    let anchors: Vec<ImageId> = {
        let mut stmt = tx.prepare_cached("SELECT scene_anchor FROM images WHERE id = ?1")?;
        let mut out = Vec::new();
        for &id in &all {
            if stmt.query_row([id], |r| r.get::<_, bool>(0))? {
                out.push(id);
            }
        }
        out
    };
    let doomed: Vec<SceneId> = {
        let mut stmt = tx.prepare_cached(&format!(
            "SELECT s.id FROM scenes s
             WHERE (?1 OR s.method = 'auto') AND ({})",
            scene_scope(&scope)
        ))?;
        let rows = stmt.query_map(params![replace_manual], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    // Plan state of every scene the regrouped images leave (deleted ones and any emptied by
    // the moves).
    let mut left = doomed.clone();
    {
        let mut stmt = tx.prepare_cached("SELECT scene_id FROM images WHERE id = ?1 AND scene_id IS NOT NULL")?;
        for &id in &all {
            if let Some(s) = stmt.query_row([id], |r| r.get::<_, SceneId>(0)).optional()? {
                if !left.contains(&s) {
                    left.push(s);
                }
            }
        }
    }
    let plans = plan_states(&tx, &left)?;
    for chunk in doomed.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        tx.execute(
            &format!("UPDATE images SET scene_id = NULL, scene_anchor = 0 WHERE scene_id IN ({ph})"),
            params_from_iter(chunk.iter()),
        )?;
        tx.execute(&format!("DELETE FROM scenes WHERE id IN ({ph})"), params_from_iter(chunk.iter()))?;
    }
    for group in groups.iter().filter(|g| !g.is_empty()) {
        let id = insert_scene(&tx, SceneMethod::Auto, now)?;
        assign(&tx, id, group, now)?;
        let keep: Vec<ImageId> = members(&tx, id)?
            .into_iter()
            .map(|(i, _)| i)
            .filter(|i| anchors.contains(i))
            .take(Scene::MAX_ANCHORS)
            .collect();
        write_anchors(&tx, id, &keep)?;
        refresh(&tx, id, true, now)?;
        // Edit-plan state follows the representative into its new scene: a user choice first,
        // then one with an applied edit, then an automatic one (earliest old scene).
        let carried = plans
            .iter()
            .filter(|p| group.contains(&p.representative_id))
            .min_by_key(|p| (p.source.as_deref() != Some("user"), p.applied_params_json.is_none(), p.scene_id));
        if let Some(p) = carried {
            tx.execute(
                "UPDATE scenes SET representative_id = ?2, representative_source = ?3, representative_reason = ?4,
                     applied_at_ms = ?5, applied_params_json = ?6, applied_batch_id = ?7
                 WHERE id = ?1",
                params![
                    id,
                    p.representative_id,
                    p.source,
                    p.reason,
                    p.applied_at_ms,
                    p.applied_params_json,
                    p.applied_batch_id
                ],
            )?;
        }
    }
    let out = list_scenes(&tx, &scope)?;
    tx.commit()?;
    Ok(out)
}

/// Guided-workflow state of a scene (IPC v14 `scenes.representative_*` / `applied_*`).
struct PlanState {
    scene_id: SceneId,
    representative_id: ImageId,
    source: Option<String>,
    reason: Option<String>,
    applied_at_ms: Option<i64>,
    applied_params_json: Option<String>,
    applied_batch_id: Option<i64>,
}

/// Plan state of the `scenes` that have a representative.
fn plan_states(conn: &Connection, scenes: &[SceneId]) -> AppResult<Vec<PlanState>> {
    let mut stmt = conn.prepare_cached(
        "SELECT representative_id, representative_source, representative_reason, applied_at_ms,
                applied_params_json, applied_batch_id
         FROM scenes WHERE id = ?1 AND representative_id IS NOT NULL",
    )?;
    let mut out = Vec::new();
    for &scene_id in scenes {
        let row = stmt
            .query_row([scene_id], |r| {
                Ok(PlanState {
                    scene_id,
                    representative_id: r.get(0)?,
                    source: r.get(1)?,
                    reason: r.get(2)?,
                    applied_at_ms: r.get(3)?,
                    applied_params_json: r.get(4)?,
                    applied_batch_id: r.get(5)?,
                })
            })
            .optional()?;
        out.extend(row);
    }
    Ok(out)
}

/// Matching inputs for `ids` in the given order (atomic `not_found`).
pub fn match_inputs(conn: &Connection, ids: &[ImageId]) -> AppResult<Vec<MatchImage>> {
    repo::get_images(conn, ids)?
        .into_iter()
        .map(|e| {
            Ok(MatchImage {
                adjustments: repo::get_adjustments(conn, e.id)?,
                captured_at_ms: e.capture.captured_at_ms,
                src: SourceImage { id: e.id, path: e.path.into(), orientation: e.orientation },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;

    /// Folder 1: images 1..=4 at t = 0, 1000, 2000, (none); folder 2: image 5 at t = 500.
    fn seed() -> Connection {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0), (2, '/g', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at, captured_at_ms)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 1000),
                    (3, 1, '/f/c.arw', 'c.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 2000),
                    (4, 1, '/f/d.arw', 'd.arw', 'arw', 'sony', 'bayer', 1, 0, 0, NULL),
                    (5, 2, '/g/e.arw', 'e.arw', 'arw', 'sony', 'bayer', 1, 0, 0, 500);",
        )
        .unwrap();
        conn
    }

    fn kind(r: AppResult<impl std::fmt::Debug>) -> ErrorKind {
        r.unwrap_err().kind
    }

    #[test]
    fn create_orders_members_and_derives_columns() {
        let mut conn = seed();
        let s = create_scene(&mut conn, &[4, 3, 1, 3]).unwrap();
        assert_eq!(s.image_ids, vec![1, 3, 4]);
        assert_eq!((s.folder_id, s.started_at_ms, s.ended_at_ms), (Some(1), Some(0), Some(2000)));
        assert_eq!(s.method, SceneMethod::Manual);
        assert!(s.anchor_ids.is_empty());

        // Cross-folder scene has no folder; images move out of their old scene.
        let t = create_scene(&mut conn, &[3, 5]).unwrap();
        assert_eq!(t.folder_id, None);
        assert_eq!(get_scene(&conn, s.id).unwrap().image_ids, vec![1, 4]);
        // Moving every member out deletes the old scene.
        create_scene(&mut conn, &[1, 4]).unwrap();
        assert_eq!(kind(get_scene(&conn, s.id)), ErrorKind::NotFound);

        assert_eq!(kind(create_scene(&mut conn, &[])), ErrorKind::InvalidArgument);
        assert_eq!(kind(create_scene(&mut conn, &[1, 99])), ErrorKind::NotFound);
        let e = repo::get_image(&conn, 5).unwrap();
        assert_eq!((e.scene_id, e.is_scene_anchor), (Some(t.id), false));
    }

    #[test]
    fn anchors_are_members_and_capped() {
        let mut conn = seed();
        let s = create_scene(&mut conn, &[1, 2, 3]).unwrap();
        let s2 = set_scene_anchors(&mut conn, s.id, &[3, 1]).unwrap();
        assert_eq!(s2.anchor_ids, vec![1, 3]);
        assert_eq!(kind(set_scene_anchors(&mut conn, s.id, &[1, 2, 3])), ErrorKind::InvalidArgument);
        assert_eq!(kind(set_scene_anchors(&mut conn, s.id, &[5])), ErrorKind::InvalidArgument);
        assert_eq!(kind(set_scene_anchors(&mut conn, 999, &[])), ErrorKind::NotFound);
        assert!(repo::get_image(&conn, 3).unwrap().is_scene_anchor);

        // Leaving the scene drops the anchor flag; staying members keep theirs.
        let s3 = set_scene_members(&mut conn, s.id, &[1, 2]).unwrap();
        assert_eq!((s3.image_ids, s3.anchor_ids), (vec![1, 2], vec![1]));
        assert!(!repo::get_image(&conn, 3).unwrap().is_scene_anchor);
        assert_eq!(repo::get_image(&conn, 3).unwrap().scene_id, None);
        assert_eq!(set_scene_anchors(&mut conn, s.id, &[]).unwrap().anchor_ids, Vec::<ImageId>::new());
    }

    #[test]
    fn set_members_replacing_everything_keeps_the_scene() {
        let mut conn = seed();
        let s = create_scene(&mut conn, &[1, 2]).unwrap();
        let s2 = set_scene_members(&mut conn, s.id, &[3]).unwrap();
        assert_eq!((s2.id, s2.image_ids), (s.id, vec![3]));
        assert_eq!(kind(set_scene_members(&mut conn, s.id, &[])), ErrorKind::InvalidArgument);
    }

    #[test]
    fn merge_split_delete() {
        let mut conn = seed();
        let a = create_scene(&mut conn, &[1, 2]).unwrap();
        let b = create_scene(&mut conn, &[3, 4]).unwrap();
        set_scene_anchors(&mut conn, a.id, &[2]).unwrap();
        set_scene_anchors(&mut conn, b.id, &[3, 4]).unwrap();
        let m = merge_scenes(&mut conn, &[a.id, b.id]).unwrap();
        assert_eq!(m.id, a.id);
        assert_eq!(m.image_ids, vec![1, 2, 3, 4]);
        assert_eq!(m.anchor_ids, vec![2, 3]);
        assert_eq!(kind(get_scene(&conn, b.id)), ErrorKind::NotFound);
        assert_eq!(kind(merge_scenes(&mut conn, &[a.id])), ErrorKind::InvalidArgument);

        let parts = split_scene(&mut conn, a.id, 3).unwrap();
        assert_eq!(parts[0].image_ids, vec![1, 2]);
        assert_eq!(parts[1].image_ids, vec![3, 4]);
        assert_eq!((parts[0].anchor_ids.clone(), parts[1].anchor_ids.clone()), (vec![2], vec![3]));
        assert_eq!(parts[1].method, SceneMethod::Manual);
        assert_eq!(kind(split_scene(&mut conn, a.id, 1)), ErrorKind::InvalidArgument);
        assert_eq!(kind(split_scene(&mut conn, a.id, 5)), ErrorKind::InvalidArgument);

        delete_scene(&mut conn, parts[1].id).unwrap();
        let e = repo::get_image(&conn, 3).unwrap();
        assert_eq!((e.scene_id, e.is_scene_anchor), (None, false));
        assert_eq!(kind(delete_scene(&mut conn, parts[1].id)), ErrorKind::NotFound);
    }

    #[test]
    fn list_filters_by_folder_in_capture_order() {
        let mut conn = seed();
        let late = create_scene(&mut conn, &[3]).unwrap();
        let early = create_scene(&mut conn, &[1, 2]).unwrap();
        let other = create_scene(&mut conn, &[5]).unwrap();
        let ids = |f| list_scenes(&conn, f).unwrap().iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(Some(1)), vec![early.id, late.id]);
        assert_eq!(ids(Some(2)), vec![other.id]);
        assert_eq!(ids(None), vec![early.id, other.id, late.id]);
        // Query filter.
        let q = crate::ipc::types::ImageQuery { scene_id: Some(early.id), ..Default::default() };
        let got: Vec<ImageId> = repo::list_images(&conn, &q).unwrap().items.iter().map(|e| e.id).collect();
        assert_eq!(got, vec![1, 2]);
    }

    #[test]
    fn detection_frames_and_replace_scenes() {
        let mut conn = seed();
        let manual = create_scene(&mut conn, &[4]).unwrap();
        let f = SceneFeatures {
            log_mean_luma: -2.0,
            luma_hist: vec![1.0],
            ab_hist: vec![1.0],
            mean_oklab: [0.5, 0.0, 0.0],
        };
        save_features(&mut conn, &[(1, f.clone()), (99, f.clone())]).unwrap();

        let frames = detection_frames(&conn, Some(1), false).unwrap();
        assert_eq!(frames.iter().map(|f| f.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(frames[0].features.as_ref(), Some(&f));
        assert!(frames[1].features.is_none() && frames[0].preview_path.is_none());
        assert_eq!(detection_frames(&conn, Some(1), true).unwrap().len(), 4);
        // Stale version is ignored.
        conn.execute("UPDATE scene_features SET version = 'old'", []).unwrap();
        assert!(detection_frames(&conn, Some(1), false).unwrap()[0].features.is_none());

        let first = replace_scenes(&mut conn, Some(1), &[vec![1, 2], vec![3]], false).unwrap();
        assert_eq!(first.len(), 3, "two auto scenes + the kept manual one");
        let auto_a = first.iter().find(|s| s.image_ids == vec![1, 2]).unwrap().id;
        set_scene_anchors(&mut conn, auto_a, &[2]).unwrap();
        // Re-detection replaces auto scenes, keeps manual ones, carries anchors over.
        let second = replace_scenes(&mut conn, Some(1), &[vec![1, 2, 3]], false).unwrap();
        assert_eq!(second.len(), 2);
        let s = second.iter().find(|s| s.method == SceneMethod::Auto).unwrap();
        assert_eq!((s.image_ids.clone(), s.anchor_ids.clone()), (vec![1, 2, 3], vec![2]));
        assert!(second.iter().any(|s| s.id == manual.id));
        // Folder 2 untouched; replace_manual drops the manual scene.
        let other = create_scene(&mut conn, &[5]).unwrap();
        let third = replace_scenes(&mut conn, Some(1), &[vec![1, 2, 3, 4]], true).unwrap();
        assert_eq!(third.len(), 1);
        assert!(get_scene(&conn, other.id).is_ok());
        assert_eq!(kind(replace_scenes(&mut conn, Some(1), &[vec![1], vec![1]], false)), ErrorKind::Internal);
    }

    #[test]
    fn match_inputs_in_order() {
        let conn = seed();
        let m = match_inputs(&conn, &[3, 1]).unwrap();
        assert_eq!((m[0].src.id, m[0].captured_at_ms, m[1].src.id), (3, Some(2000), 1));
        assert!(m[0].adjustments.is_neutral());
        assert_eq!(kind(match_inputs(&conn, &[1, 42])), ErrorKind::NotFound);
    }
}
