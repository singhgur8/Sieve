//! Projects (IPC v14): one shoot = a name + its source folder(s), cover, shoot type and
//! guided-workflow step. Every `folders` row belongs to exactly one project
//! (`folders.project_id`, migration 0012); import assigns it ([`ImportTarget`]).
//!
//! Scoping: inside a project the UI passes `projectId` to image queries, filter counts,
//! bursts, scenes, the edit plan and analysis; [`FolderScope`] turns `(folderId, projectId)`
//! into the folder set those queries filter on.
//!
//! Removing a project deletes its catalog rows only (folders -> images -> every per-image
//! table cascades). Originals, sidecars and exports on disk are never touched.
//!
//! Ownership: architect wrote the catalog side (contract v14); rust-engine-dev owns it from
//! here (cache-file cleanup on removal is theirs, see `remove_project` in `ipc::commands`).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_ms, repo};
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;

/// The folders a scoped query covers: `None` = the whole catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderScope(Option<Vec<FolderId>>);

impl From<Option<FolderId>> for FolderScope {
    fn from(folder: Option<FolderId>) -> Self {
        Self(folder.map(|f| vec![f]))
    }
}

impl From<&FolderScope> for FolderScope {
    fn from(scope: &FolderScope) -> Self {
        scope.clone()
    }
}

impl FolderScope {
    /// The whole catalog.
    pub fn all() -> Self {
        Self(None)
    }

    pub fn folders(folders: Vec<FolderId>) -> Self {
        Self(Some(folders))
    }

    /// `folder` AND `project` (either may be `None`). Unknown project -> `not_found`; a folder
    /// outside the project -> an empty scope (matches nothing).
    pub fn resolve(conn: &Connection, folder: Option<FolderId>, project: Option<ProjectId>) -> AppResult<Self> {
        let Some(project) = project else {
            return Ok(folder.into());
        };
        let folders = project_folder_ids(conn, project)?;
        Ok(Self(Some(match folder {
            Some(f) => folders.into_iter().filter(|&x| x == f).collect(),
            None => folders,
        })))
    }

    pub fn is_all(&self) -> bool {
        self.0.is_none()
    }

    /// The folder ids (`None` = all).
    pub fn ids(&self) -> Option<&[FolderId]> {
        self.0.as_deref()
    }

    /// SQL predicate restricting `column` (a folder id column, e.g. `i.folder_id`) to the
    /// scope: `1` for all, `0` for none, else `column IN (..)`. Ids are integers, so inlining
    /// them is injection-safe.
    pub fn predicate(&self, column: &str) -> String {
        match &self.0 {
            None => "1".into(),
            Some(ids) if ids.is_empty() => "0".into(),
            Some(ids) if ids.len() == 1 => format!("{column} = {}", ids[0]),
            Some(ids) => {
                let list: Vec<String> = ids.iter().map(i64::to_string).collect();
                format!("{column} IN ({})", list.join(","))
            }
        }
    }
}

/// Folders of project `id`, by id. Unknown project -> `not_found`.
pub fn project_folder_ids(conn: &Connection, id: ProjectId) -> AppResult<Vec<FolderId>> {
    require_project(conn, id)?;
    let ids = conn
        .prepare_cached("SELECT id FROM folders WHERE project_id = ?1 ORDER BY id")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

pub fn require_project(conn: &Connection, id: ProjectId) -> AppResult<()> {
    if conn.prepare_cached("SELECT 1 FROM projects WHERE id = ?1")?.exists([id])? {
        Ok(())
    } else {
        Err(AppError::not_found(format!("project {id}")))
    }
}

/// Last path component (the default project name), or the whole path when it has none.
pub fn default_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| path.to_owned())
}

/// Which project an imported folder goes to.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportTarget {
    /// `import_folder(path, null)`: the folder's project if the folder (or a folder containing
    /// it) is already in the catalog, else a new project named after the folder.
    Auto,
    /// `create_project`: like `Auto`, but a new project gets this name (default: folder name)
    /// and shoot type (default: the catalog's).
    New { name: Option<String>, shoot_type: Option<ShootType> },
    /// `import_folder(path, projectId)`: add the folder to this project. A folder already in
    /// another project -> `invalid_argument`.
    Existing(ProjectId),
}

/// Where an import registers its images: the folder row and its project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportFolder {
    pub folder_id: FolderId,
    pub project_id: ProjectId,
    /// The folder (or a folder containing it) was already in the catalog.
    pub existing: bool,
}

/// Finds or creates the folder row for canonical `path` and its project (inside the import
/// savepoint). A path inside an already imported folder reuses that folder (its images keep
/// one folder row and one project).
pub fn import_folder_row(conn: &Connection, path: &str, target: &ImportTarget) -> AppResult<ImportFolder> {
    let found: Option<(FolderId, Option<ProjectId>)> = conn
        .query_row(
            "SELECT id, project_id FROM folders
             WHERE path = ?1 OR (substr(?1, 1, length(path) + 1) = path || '/')
             ORDER BY length(path) DESC LIMIT 1",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let ImportTarget::Existing(p) = target {
        require_project(conn, *p)?;
    }
    if let Some((folder_id, project)) = found {
        let project_id = match project {
            Some(p) => p,
            // Pre-v12 rows are migrated, so this only happens for rows inserted by old code
            // paths (tests): give the folder its own project.
            None => {
                let p = insert_project(conn, &default_name(path), repo::shoot_type(conn)?)?;
                conn.execute("UPDATE folders SET project_id = ?2 WHERE id = ?1", params![folder_id, p])?;
                p
            }
        };
        if let ImportTarget::Existing(p) = target {
            if *p != project_id {
                let name: String =
                    conn.query_row("SELECT name FROM projects WHERE id = ?1", [project_id], |r| r.get(0))?;
                return Err(AppError::invalid(format!("{path} is already in project \"{name}\"")));
            }
        }
        return Ok(ImportFolder { folder_id, project_id, existing: true });
    }
    let project_id = match target {
        ImportTarget::Existing(p) => *p,
        ImportTarget::Auto => insert_project(conn, &default_name(path), repo::shoot_type(conn)?)?,
        ImportTarget::New { name, shoot_type } => {
            let name = match name {
                Some(n) => Project::validate_name(n).map_err(AppError::invalid)?,
                None => default_name(path),
            };
            let shoot_type = match shoot_type {
                Some(s) => *s,
                None => repo::shoot_type(conn)?,
            };
            insert_project(conn, &name, shoot_type)?
        }
    };
    conn.execute(
        "INSERT INTO folders (path, added_at, project_id) VALUES (?1, ?2, ?3)",
        params![path, now_ms(), project_id],
    )?;
    Ok(ImportFolder { folder_id: conn.last_insert_rowid(), project_id, existing: false })
}

fn insert_project(conn: &Connection, name: &str, shoot_type: ShootType) -> AppResult<ProjectId> {
    conn.execute(
        "INSERT INTO projects (name, shoot_type, created_at) VALUES (?1, ?2, ?3)",
        params![name, shoot_type.as_str(), now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// SQL of `Project` rows (without folders / cover path); `?1` = project id or NULL for all,
/// `?2` / `?3` / `?4` = keeper rule `minRating` / `useSuggestions` / mode is `not_rejected`
/// (`KeeperRule::is_keeper_values` mirror).
const PROJECT_SQL: &str = "
    SELECT p.id, p.name, p.cover_image_id, p.shoot_type, p.workflow_step, p.created_at, p.last_opened_at,
           COUNT(i.id),
           COALESCE(SUM(CASE WHEN i.pick = 'reject' THEN 0
                             WHEN i.pick = 'pick' THEN 1
                             WHEN ?4 THEN 1
                             WHEN i.rating >= ?2 THEN 1
                             WHEN i.rating = 0 AND ?3 AND q.suggested_pick = 'pick' THEN 1
                             ELSE 0 END), 0),
           COALESCE(SUM(EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = i.id AND a.neutral = 0)), 0),
           COALESCE(SUM(i.pick = 'pick'), 0),
           COALESCE(SUM(i.pick = 'reject'), 0),
           COALESCE(SUM(i.missing_since_ms IS NOT NULL), 0),
           MIN(i.captured_at_ms), MAX(i.captured_at_ms)
      FROM projects p
      LEFT JOIN folders f ON f.project_id = p.id
      LEFT JOIN images i ON i.folder_id = f.id
      LEFT JOIN quality_scores q ON q.image_id = i.id
     WHERE ?1 IS NULL OR p.id = ?1
     GROUP BY p.id
     ORDER BY p.last_opened_at IS NULL, p.last_opened_at DESC, p.created_at DESC, p.id DESC";

fn query_projects(conn: &Connection, id: Option<ProjectId>) -> AppResult<Vec<Project>> {
    let rule = repo::keeper_rule(conn)?;
    let mut projects = conn
        .prepare_cached(PROJECT_SQL)?
        .query_map(params![id, rule.min_rating, rule.use_suggestions, rule.mode == KeeperMode::NotRejected], |r| {
            let cover: Option<ImageId> = r.get(2)?;
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                folders: Vec::new(),
                cover_image_id: cover,
                cover_chosen: cover.is_some(),
                cover_thumbnail_path: None,
                shoot_type: ShootType::parse(&r.get::<_, String>(3)?).unwrap_or(ShootType::General),
                workflow_step: WorkflowStep::parse(&r.get::<_, String>(4)?).unwrap_or(WorkflowStep::Cull),
                created_at_ms: r.get(5)?,
                last_opened_at_ms: r.get(6)?,
                photo_count: r.get(7)?,
                keeper_count: r.get(8)?,
                edited_count: r.get(9)?,
                picked_count: r.get(10)?,
                rejected_count: r.get(11)?,
                missing_count: r.get(12)?,
                captured_from_ms: r.get(13)?,
                captured_to_ms: r.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut folders = conn.prepare_cached(
        "SELECT f.id, f.path, COUNT(i.id) FROM folders f LEFT JOIN images i ON i.folder_id = f.id
         WHERE f.project_id = ?1 GROUP BY f.id ORDER BY f.path",
    )?;
    let mut auto_cover = conn.prepare_cached(
        "SELECT i.id FROM images i JOIN folders f ON f.id = i.folder_id
         WHERE f.project_id = ?1
         ORDER BY i.pick = 'reject', i.pick = 'pick' DESC, i.rating DESC,
                  i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id
         LIMIT 1",
    )?;
    let mut thumb = conn
        .prepare_cached("SELECT path FROM thumbnails WHERE image_id = ?1 AND status = 'ready' AND path IS NOT NULL")?;
    for p in &mut projects {
        p.folders = folders
            .query_map([p.id], |r| {
                let path: String = r.get(1)?;
                Ok(ProjectFolder { id: r.get(0)?, exists: Path::new(&path).is_dir(), path, image_count: r.get(2)? })
            })?
            .collect::<Result<_, _>>()?;
        if p.cover_image_id.is_none() {
            p.cover_image_id = auto_cover.query_row([p.id], |r| r.get(0)).optional()?;
        }
        if let Some(cover) = p.cover_image_id {
            p.cover_thumbnail_path = thumb.query_row([cover], |r| r.get(0)).optional()?;
        }
    }
    Ok(projects)
}

/// `list_projects()`: most recently opened first, then newest.
pub fn list_projects(conn: &Connection) -> AppResult<Vec<Project>> {
    query_projects(conn, None)
}

/// Unknown project -> `not_found`.
pub fn get_project(conn: &Connection, id: ProjectId) -> AppResult<Project> {
    query_projects(conn, Some(id))?.pop().ok_or_else(|| AppError::not_found(format!("project {id}")))
}

/// `open_project`: stamps `last_opened_at`.
pub fn open_project(conn: &Connection, id: ProjectId) -> AppResult<Project> {
    update(conn, id, "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1", now_ms())?;
    get_project(conn, id)
}

pub fn rename_project(conn: &Connection, id: ProjectId, name: &str) -> AppResult<Project> {
    let name = Project::validate_name(name).map_err(AppError::invalid)?;
    update(conn, id, "UPDATE projects SET name = ?2 WHERE id = ?1", name)?;
    get_project(conn, id)
}

/// `Some(image)` must be an image of the project (`invalid_argument` otherwise); `None`
/// returns to the automatic cover.
pub fn set_project_cover(conn: &Connection, id: ProjectId, image: Option<ImageId>) -> AppResult<Project> {
    require_project(conn, id)?;
    if let Some(image) = image {
        let inside = conn
            .prepare_cached(
                "SELECT 1 FROM images i JOIN folders f ON f.id = i.folder_id WHERE i.id = ?1 AND f.project_id = ?2",
            )?
            .exists(params![image, id])?;
        if !inside {
            return Err(AppError::invalid(format!("image {image} is not in project {id}")));
        }
    }
    update(conn, id, "UPDATE projects SET cover_image_id = ?2 WHERE id = ?1", image)?;
    get_project(conn, id)
}

pub fn set_project_shoot_type(conn: &Connection, id: ProjectId, shoot_type: ShootType) -> AppResult<()> {
    update(conn, id, "UPDATE projects SET shoot_type = ?2 WHERE id = ?1", shoot_type.as_str())
}

/// Shoot type culling uses for image `image` (its project's; the catalog default for an
/// image without one). For the analysis worker (vision-ml-dev).
pub fn shoot_type_of_image(conn: &Connection, image: ImageId) -> AppResult<ShootType> {
    let v: Option<String> = conn
        .prepare_cached(
            "SELECT p.shoot_type FROM images i JOIN folders f ON f.id = i.folder_id
             JOIN projects p ON p.id = f.project_id WHERE i.id = ?1",
        )?
        .query_row([image], |r| r.get(0))
        .optional()?;
    match v.as_deref().and_then(ShootType::parse) {
        Some(s) => Ok(s),
        None => repo::shoot_type(conn),
    }
}

/// Guided-workflow step of project `id`. Unknown project -> `not_found`.
pub fn workflow_step(conn: &Connection, id: ProjectId) -> AppResult<WorkflowStep> {
    let v: Option<String> =
        conn.query_row("SELECT workflow_step FROM projects WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let v = v.ok_or_else(|| AppError::not_found(format!("project {id}")))?;
    Ok(WorkflowStep::parse(&v).unwrap_or(WorkflowStep::Cull))
}

pub fn set_workflow_step(conn: &Connection, id: ProjectId, step: WorkflowStep) -> AppResult<()> {
    update(conn, id, "UPDATE projects SET workflow_step = ?2 WHERE id = ?1", step.as_str())
}

fn update(conn: &Connection, id: ProjectId, sql: &str, value: impl rusqlite::ToSql) -> AppResult<()> {
    if conn.execute(sql, params![id, value])? == 0 {
        return Err(AppError::not_found(format!("project {id}")));
    }
    Ok(())
}

/// `remove_project`: deletes the project and every catalog row of its folders and images in
/// one transaction; burst groups and scenes left without members go too. Returns the result
/// and the removed image ids (for cache-file cleanup). Never touches files.
pub fn remove_project(conn: &mut Connection, id: ProjectId) -> AppResult<(RemoveProjectResult, Vec<ImageId>)> {
    let tx = conn.transaction()?;
    let folders = project_folder_ids(&tx, id)?;
    let images: Vec<ImageId> = tx
        .prepare(&format!(
            "SELECT id FROM images WHERE {} ORDER BY id",
            FolderScope::folders(folders.clone()).predicate("folder_id")
        ))?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    tx.execute(
        "DELETE FROM burst_groups WHERE id NOT IN (SELECT burst_group_id FROM images WHERE burst_group_id IS NOT NULL)",
        [],
    )?;
    tx.execute("DELETE FROM scenes WHERE id NOT IN (SELECT scene_id FROM images WHERE scene_id IS NOT NULL)", [])?;
    tx.commit()?;
    Ok((RemoveProjectResult { removed_images: images.len() as u32, removed_folders: folders.len() as u32 }, images))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// A file that passes the TIFF-header check of the ARW importer.
    fn touch(dir: &Path, name: &str) {
        let mut bytes = b"II*\0".to_vec();
        bytes.resize(64, 0);
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    /// Two shoot folders with two ARWs each, imported as two projects.
    fn two_projects() -> (Connection, tempfile::TempDir, [ImportSummary; 2]) {
        let mut conn = open_in_memory();
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("Smith Wedding");
        let b = root.path().join("Jones Portraits");
        for d in [&a, &b] {
            std::fs::create_dir(d).unwrap();
            touch(d, "A.ARW");
            touch(d, "B.ARW");
        }
        let opts = ImportOptions::raw_only(false);
        let sa = repo::import_folder_to(&mut conn, &a, &opts, &ImportTarget::Auto).unwrap().0;
        let sb = repo::import_folder_to(
            &mut conn,
            &b,
            &opts,
            &ImportTarget::New { name: Some("  Jones  ".into()), shoot_type: Some(ShootType::Portrait) },
        )
        .unwrap()
        .0;
        (conn, root, [sa, sb])
    }

    fn ids(conn: &Connection, q: &ImageQuery) -> Vec<ImageId> {
        repo::list_image_ids(conn, q).unwrap()
    }

    #[test]
    fn import_creates_projects_and_queries_are_scoped() {
        let (mut conn, root, [a, b]) = two_projects();
        assert_ne!(a.project_id, b.project_id);
        let list = list_projects(&conn).unwrap();
        assert_eq!(list.len(), 2);
        let pa = get_project(&conn, a.project_id).unwrap();
        assert_eq!(pa.name, "Smith Wedding");
        assert_eq!(pa.shoot_type, ShootType::General);
        assert_eq!(pa.workflow_step, WorkflowStep::Cull);
        assert_eq!((pa.photo_count, pa.folders.len(), pa.folders[0].exists), (2, 1, true));
        let pb = get_project(&conn, b.project_id).unwrap();
        assert_eq!((pb.name.as_str(), pb.shoot_type), ("Jones", ShootType::Portrait));

        // Image queries, filter counts and scenes only see the project's photos.
        let q = |p: Option<ProjectId>| ImageQuery { project_id: p, ..Default::default() };
        assert_eq!(ids(&conn, &q(None)).len(), 4);
        let in_a = ids(&conn, &q(Some(a.project_id)));
        assert_eq!(in_a.len(), 2);
        assert!(repo::get_images(&conn, &in_a).unwrap().iter().all(|e| e.folder_id == a.folder_id));
        let wrong = ImageQuery { folder_id: Some(a.folder_id), ..q(Some(b.project_id)) };
        assert!(ids(&conn, &wrong).is_empty(), "folder of another project matches nothing");
        let counts = |p| repo::filter_counts(&conn, FolderScope::resolve(&conn, None, p).unwrap()).unwrap();
        assert_eq!((counts(None).total, counts(Some(a.project_id)).total, counts(Some(b.project_id)).total), (4, 2, 2));
        assert_eq!(
            FolderScope::resolve(&conn, None, Some(999)).unwrap_err().kind,
            crate::ipc::error::ErrorKind::NotFound
        );

        // Counts: a pick, a reject, an edit.
        repo::set_pick(&mut conn, &[in_a[0]], PickFlag::Pick).unwrap();
        repo::set_pick(&mut conn, &[in_a[1]], PickFlag::Reject).unwrap();
        conn.execute(
            "INSERT INTO adjustments (image_id, params_json, process_version, updated_at, neutral) VALUES (?1, '{}', 1, 0, 0)",
            [in_a[0]],
        )
        .unwrap();
        let pa = get_project(&conn, a.project_id).unwrap();
        assert_eq!((pa.keeper_count, pa.edited_count, pa.picked_count, pa.rejected_count), (1, 1, 1, 1));
        // Auto cover = the pick; a chosen cover must be in the project.
        assert_eq!((pa.cover_image_id, pa.cover_chosen), (Some(in_a[0]), false));
        let in_b = ids(&conn, &q(Some(b.project_id)));
        assert!(set_project_cover(&conn, a.project_id, Some(in_b[0])).is_err());
        let pa = set_project_cover(&conn, a.project_id, Some(in_a[1])).unwrap();
        assert_eq!((pa.cover_image_id, pa.cover_chosen), (Some(in_a[1]), true));

        // Open / rename / step / shoot type.
        let opened = open_project(&conn, b.project_id).unwrap();
        assert!(opened.last_opened_at_ms.is_some());
        assert_eq!(list_projects(&conn).unwrap()[0].id, b.project_id, "last opened first");
        assert!(rename_project(&conn, a.project_id, "   ").is_err());
        assert_eq!(rename_project(&conn, a.project_id, "Smith & Co").unwrap().name, "Smith & Co");
        set_workflow_step(&conn, a.project_id, WorkflowStep::Edit).unwrap();
        assert_eq!(workflow_step(&conn, a.project_id).unwrap(), WorkflowStep::Edit);
        assert_eq!(workflow_step(&conn, b.project_id).unwrap(), WorkflowStep::Cull);
        set_project_shoot_type(&conn, a.project_id, ShootType::Wedding).unwrap();
        assert_eq!(shoot_type_of_image(&conn, in_a[0]).unwrap(), ShootType::Wedding);
        assert_eq!(shoot_type_of_image(&conn, in_b[0]).unwrap(), ShootType::Portrait);

        // Re-import of the same folder, or a folder inside it, reuses the project; adding it to
        // another project is refused.
        let opts = ImportOptions::raw_only(false);
        let dir_a = root.path().join("Smith Wedding");
        let (again, existing) = repo::import_folder_to(&mut conn, &dir_a, &opts, &ImportTarget::Auto).unwrap();
        assert!(existing && again.project_id == a.project_id && again.folder_id == a.folder_id);
        std::fs::create_dir(dir_a.join("Extra")).unwrap();
        touch(&dir_a.join("Extra"), "C.ARW");
        let (sub, existing) =
            repo::import_folder_to(&mut conn, &dir_a.join("Extra"), &opts, &ImportTarget::Auto).unwrap();
        assert!(existing && sub.folder_id == a.folder_id && sub.added == 1);
        let err = repo::import_folder_to(&mut conn, &dir_a, &opts, &ImportTarget::Existing(b.project_id)).unwrap_err();
        assert_eq!(err.kind, crate::ipc::error::ErrorKind::InvalidArgument);
        assert_eq!(list_projects(&conn).unwrap().len(), 2);

        // Remove: catalog rows only; files stay.
        let (removed, gone) = remove_project(&mut conn, a.project_id).unwrap();
        assert_eq!((removed.removed_images, removed.removed_folders, gone.len()), (3, 1, 3));
        assert_eq!(get_project(&conn, a.project_id).unwrap_err().kind, crate::ipc::error::ErrorKind::NotFound);
        assert_eq!(ids(&conn, &q(None)).len(), 2);
        assert!(dir_a.join("A.ARW").exists() && dir_a.join("Extra/C.ARW").exists());
        let fk: Vec<String> = conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(fk.is_empty(), "{fk:?}");
    }

    #[test]
    fn scope_predicates() {
        assert_eq!(FolderScope::all().predicate("f"), "1");
        assert_eq!(FolderScope::from(Some(3)).predicate("f"), "f = 3");
        assert_eq!(FolderScope::folders(vec![]).predicate("f"), "0");
        assert_eq!(FolderScope::folders(vec![1, 2]).predicate("i.folder_id"), "i.folder_id IN (1,2)");
        assert_eq!(default_name("/Users/me/Shoots/Smith Wedding"), "Smith Wedding");
        assert_eq!(default_name("/"), "/");
    }
}
