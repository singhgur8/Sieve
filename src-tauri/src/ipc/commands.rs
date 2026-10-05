//! Tauri command handlers. Thin: validate/convert arguments, run the query on a
//! blocking thread, return typed results. Signatures are the contract; bodies may
//! be reimplemented by the owning specialist without changing them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use super::error::{AppError, AppResult, ErrorKind};
use super::types::*;
use crate::db::projects::{self, FolderScope, ImportTarget};
use crate::db::{self, repo};
use crate::develop::masks::MaskCache;
use crate::develop::{self, DevelopCache, SourceImage};
use crate::export::{self, Exporter};
use crate::ingest::{self, Ingest};
use crate::lut::{self, LutLibrary};
use crate::ml::masking::Segmenter;
use crate::ml::style::StyleModel;
use crate::ml::{self, Analysis};
use crate::model_fetch::ModelDownloads;
use crate::profiles::{CameraKey, ProfileLibrary};
use crate::scene;
use crate::styles;
use crate::xmp::XmpSync;

/// Managed state: the open catalog.
pub struct Catalog {
    pub path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl Catalog {
    pub fn open(path: PathBuf) -> AppResult<Self> {
        let conn = db::open(&path)?;
        Ok(Self { path, conn: Arc::new(Mutex::new(conn)) })
    }

    /// Synchronous read of the `autoAnalyze` setting (startup only).
    pub fn auto_analyze_blocking(&self) -> AppResult<bool> {
        let conn = self.conn.lock().map_err(|_| AppError::internal("catalog lock poisoned"))?;
        repo::auto_analyze(&conn)
    }

    /// Runs `f` against the connection on the blocking pool so SQLite and
    /// filesystem work never stalls the async runtime.
    async fn run<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        let path = self.path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut conn = conn.lock().map_err(|_| AppError::internal("catalog lock poisoned"))?;
            f(&mut conn).map_err(|e| explain_catalog_error(&path, e))
        })
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
    }
}

/// Managed state (IPC v15): cancel flag of the running `apply_scene_edit` /
/// `apply_all_edited_scenes` (`cancel_scene_apply`).
#[derive(Clone, Default)]
pub struct SceneApplyControl {
    cancel: Arc<AtomicBool>,
}

impl SceneApplyControl {
    pub fn new() -> Self {
        Self::default()
    }

    fn reset(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Database errors get an actionable message: a damaged catalog (opened read-only, see
/// `db::health`) says how to restore a backup; a full disk says so.
fn explain_catalog_error(path: &Path, e: AppError) -> AppError {
    db::explain_error(path, e)
}

#[tauri::command]
#[specta::specta]
pub async fn get_catalog_state(catalog: State<'_, Catalog>, ingest: State<'_, Ingest>) -> AppResult<CatalogState> {
    let path = catalog.path.display().to_string();
    let cache_dir = ingest.config().cache_dir.display().to_string();
    catalog.run(move |c| repo::catalog_state(c, &path, &cache_dir)).await
}

/// Also kicks a rescore (tags/scores/suggestions use the new shoot type's thresholds).
#[tauri::command]
#[specta::specta]
pub async fn set_shoot_type(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    shoot_type: ShootType,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_shoot_type(c, shoot_type)).await?;
    analysis.start(&app, AnalysisScope::Rescore)
}

/// Also kicks a rescore (burst regrouping).
#[tauri::command]
#[specta::specta]
pub async fn set_burst_window(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    ms: u32,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_burst_window(c, ms)).await?;
    analysis.start(&app, AnalysisScope::Rescore)
}

/// Registers RAW files under `path` (fast: no decoding; new thumbnails start `pending`),
/// reads existing XMP sidecars (rating/pick/label, and crs: develop settings from v5) of new or
/// externally changed images, then kicks the background ingest pipeline (and analysis, if `autoAnalyze`) and
/// returns. Progress arrives as `importProgress` / `thumbnailReady` / `thumbnailFailed`
/// (and `analysis*`) events.
///
/// Project (v14): `projectId` adds the folder to that project ("Add folder to project"; a
/// folder already in another project -> `invalid_argument`); `null` = the folder's project
/// if it (or a folder containing it) is in the catalog, else a new project named after it.
/// `create_project` is the home page's "New project".
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn import_folder(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    analysis: State<'_, Analysis>,
    xmp: State<'_, XmpSync>,
    path: String,
    options: ImportOptions,
    project_id: Option<ProjectId>,
) -> AppResult<ImportSummary> {
    let target = project_id.map_or(ImportTarget::Auto, ImportTarget::Existing);
    Ok(run_import(&app, &catalog, &ingest, &analysis, &xmp, path, options, target).await?.0)
}

/// `import_folder` / `create_project`: registers the files, reads sidecars, starts ingest (and
/// analysis). Returns the summary and whether the folder was already in the catalog.
#[allow(clippy::too_many_arguments)]
async fn run_import(
    app: &AppHandle,
    catalog: &Catalog,
    ingest: &Ingest,
    analysis: &Analysis,
    xmp: &XmpSync,
    path: String,
    options: ImportOptions,
    target: ImportTarget,
) -> AppResult<(ImportSummary, bool)> {
    let ((mut summary, existing), auto) = catalog
        .run(move |c| Ok((repo::import_folder_to(c, Path::new(&path), &options, &target)?, repo::auto_analyze(c)?)))
        .await?;
    let sync = xmp.clone();
    let folder_id = summary.folder_id;
    summary.sidecars_read = blocking(move || sync.refresh_folder(folder_id)).await?;
    // Re-registered files may have changed on disk: re-resolve develop sources.
    if let Some(develop) = app.try_state::<DevelopCache>() {
        develop.forget_sources(None);
    }
    ingest.start(app)?;
    if auto {
        analysis.start(app, AnalysisScope::Pending)?;
    }
    Ok((summary, existing))
}

/// Re-extracts thumbnails/previews/EXIF for `ids` (e.g. after a failure). Resets them to
/// `pending` and returns immediately; results arrive as events. With `autoAnalyze`, the
/// new previews are re-analyzed.
#[tauri::command]
#[specta::specta]
pub async fn regenerate_thumbnails(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    analysis: State<'_, Analysis>,
    ids: Vec<ImageId>,
) -> AppResult<()> {
    // Re-extraction may change the orientation: re-resolve develop sources.
    if let Some(develop) = app.try_state::<DevelopCache>() {
        develop.forget_sources(Some(&ids));
    }
    ingest.regenerate(&app, ids)?;
    if catalog.run(|c| repo::auto_analyze(c)).await? {
        analysis.start(&app, AnalysisScope::Pending)?;
    }
    Ok(())
}

/// Catalog-wide pending/ready/failed counts and whether the pipeline is running.
#[tauri::command]
#[specta::specta]
pub async fn get_import_status(catalog: State<'_, Catalog>, ingest: State<'_, Ingest>) -> AppResult<ImportStatus> {
    let running = ingest.is_running();
    catalog.run(move |c| ingest::import_status(c, running)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_images(catalog: State<'_, Catalog>, query: ImageQuery) -> AppResult<ImagePage> {
    catalog.run(move |c| repo::list_images(c, &query)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_image(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<RawImageEntry> {
    catalog.run(move |c| repo::get_image(c, id)).await
}

/// Entries for `ids`, in the given order (e.g. to refresh rows after events or batch
/// edits). Atomic: an unknown id fails with `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_images(catalog: State<'_, Catalog>, ids: Vec<ImageId>) -> AppResult<Vec<RawImageEntry>> {
    catalog.run(move |c| repo::get_images(c, &ids)).await
}

/// Every id matching `query` in its sort order, ignoring `offset`/`limit`
/// (select-all, loupe navigation, batch actions over a filter).
#[tauri::command]
#[specta::specta]
pub async fn list_image_ids(catalog: State<'_, Catalog>, query: ImageQuery) -> AppResult<Vec<ImageId>> {
    catalog.run(move |c| repo::list_image_ids(c, &query)).await
}

/// Filter-bar facet counts for `folderId` AND `projectId` (both `null` = whole catalog; v14:
/// inside a project pass its id). `keepersOnly` (v15; `null` = false) counts keepers only
/// (`ImageQuery.keepersOnly`, the Edit / Export steps). `metadata` (v18; `null` = none) counts
/// only images passing the Library Filter metadata constraints (`ImageQuery.metadata`), so
/// the facet counts follow the metadata row. `pickOrigin` (v18.1; `null` = anyone) counts
/// only images flagged by that origin (`ImageQuery.pickOrigin`). Unknown project ->
/// `not_found`; an invalid constraint -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn get_filter_counts(
    catalog: State<'_, Catalog>,
    folder_id: Option<FolderId>,
    project_id: Option<ProjectId>,
    keepers_only: Option<bool>,
    metadata: Option<MetadataFilter>,
    pick_origin: Option<PickOrigin>,
) -> AppResult<FilterCounts> {
    catalog
        .run(move |c| {
            let scope = FolderScope::resolve(c, folder_id, project_id)?;
            let metadata = metadata.unwrap_or_default();
            repo::filter_counts_with(c, scope, keepers_only.unwrap_or(false), &metadata, pick_origin)
        })
        .await
}

/// Cull step summary for `projectId` (`null` = whole catalog) (v18): picked / unflagged /
/// rejected (by you vs. "Auto") / keepers with the breakdown under the catalog's keeper rule /
/// what Apply suggestions would change with its defaults (v18.1). `keepers` equals a
/// `keepersOnly` query's total over the same scope. Unknown project -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_cull_summary(catalog: State<'_, Catalog>, project_id: Option<ProjectId>) -> AppResult<CullSummary> {
    catalog
        .run(move |c| {
            let scope = FolderScope::resolve(c, None, project_id)?;
            repo::cull_summary(c, &scope)
        })
        .await
}

/// Distinct values with image counts for each Library Filter metadata facet (v18) over
/// `query`'s images (pass the grid's query: scope, flags, tags and metadata; sort / offset /
/// limit are ignored). Each facet ignores its own constraint (Lightroom's cascading columns).
/// Like `list_images`, an unknown project matches nothing; an invalid constraint ->
/// `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn get_metadata_filter_options(
    catalog: State<'_, Catalog>,
    query: ImageQuery,
) -> AppResult<MetadataFilterOptions> {
    catalog.run(move |c| repo::metadata_filter_options(c, &query)).await
}

// Culling writes. Each marks changed images `xmp.dirty` (DB triggers) and notifies the
// XMP auto-sync writer (no-op unless `xmpAutoSync`).

#[tauri::command]
#[specta::specta]
pub async fn set_rating(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    rating: u8,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_rating(c, &ids, rating)).await?;
    xmp.notify(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_pick(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    pick: PickFlag,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_pick(c, &ids, pick)).await?;
    xmp.notify(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_color_label(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    label: Option<ColorLabel>,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_color_label(c, &ids, label)).await?;
    xmp.notify(&app);
    Ok(())
}

/// Adds (`present = true`) or removes a tag as the user. Removing an auto tag suppresses it.
#[tauri::command]
#[specta::specta]
pub async fn set_user_tag(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    tag: CullTag,
    present: bool,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_user_tag(c, &ids, tag, present)).await?;
    xmp.notify(&app);
    Ok(())
}

/// Stored adjustments, or neutral defaults for an unedited image.
#[tauri::command]
#[specta::specta]
pub async fn get_adjustments(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<ParametricAdjustments> {
    catalog.run(move |c| repo::get_adjustments(c, id)).await
}

/// Persists `adjustments` and records a history entry labelled `label` (e.g. the slider
/// name). Call on slider release / debounced (not per drag frame: use `render_preview`
/// for live feedback). Consecutive saves with the same label within 1.5 s coalesce into
/// one entry; saving unchanged values is a no-op. Marks the sidecar dirty (crs:).
#[tauri::command]
#[specta::specta]
pub async fn save_adjustments(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    id: ImageId,
    adjustments: ParametricAdjustments,
    label: String,
) -> AppResult<AdjustmentHistory> {
    let history = catalog.run(move |c| develop::history::commit(c, id, &adjustments, &label)).await?;
    xmp.notify(&app);
    Ok(history)
}

// ---------------------------------------------------------------------------
// Editor (Phase 5)
// ---------------------------------------------------------------------------

async fn source_images(catalog: &Catalog, ids: Vec<ImageId>) -> AppResult<Vec<SourceImage>> {
    let entries = catalog.run(move |c| repo::get_images(c, &ids)).await?;
    Ok(entries
        .into_iter()
        .map(|e| SourceImage { id: e.id, path: PathBuf::from(e.path), orientation: e.orientation })
        .collect())
}

/// The develop source of `id` for per-frame commands (slider renders, overlays, WB
/// picker): remembered in the `DevelopCache` after the first lookup, so a slider drag never
/// touches the catalog (or waits for its lock). Images still pending thumbnail extraction
/// are not remembered (their orientation may still change).
async fn develop_source(catalog: &Catalog, develop: &DevelopCache, id: ImageId) -> AppResult<SourceImage> {
    if let Some(src) = develop.source(id) {
        return Ok(src);
    }
    let entry = catalog.run(move |c| repo::get_image(c, id)).await?;
    let src = source_of(&entry);
    if !matches!(entry.thumbnail, ThumbnailState::Pending) {
        develop.remember_source(src.clone());
    }
    Ok(src)
}

fn require_fields(fields: &[AdjustmentField]) -> AppResult<()> {
    if fields.is_empty() {
        return Err(AppError::invalid("fields must not be empty"));
    }
    Ok(())
}

/// Renders `adjustments` (live, unsaved) for image `id`. Latest-wins per (id, slot):
/// resolves `null` when a newer request for the same (id, slot) superseded this one
/// (ignore it). First call per image decodes the RAW (~0.3-0.8 s); later calls reuse it.
#[tauri::command]
#[specta::specta]
pub async fn render_preview(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    id: ImageId,
    adjustments: ParametricAdjustments,
    options: RenderOptions,
) -> AppResult<Option<RenderedPreview>> {
    adjustments.validate().map_err(AppError::invalid)?;
    options.validate().map_err(AppError::invalid)?;
    let ticket = develop.ticket(id, options.slot);
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let luts = luts.inner().clone();
    let rendered = blocking(move || {
        if !cache.is_current(ticket) {
            return Ok(None);
        }
        cache.render(ticket, &src, &adjustments, &options, &luts)
    })
    .await;
    note_if_missing(&catalog, id, rendered).await
}

/// Flags image `id` missing (`RawImageEntry.missingSinceMs`, IPC v13) when `result` failed
/// because its original is gone, then returns `result` unchanged. Presence is recorded by
/// `get_develop_info` (which reads the row anyway), so a successful slider render never
/// writes to the catalog.
async fn note_if_missing<T>(catalog: &Catalog, id: ImageId, result: AppResult<T>) -> AppResult<T> {
    if let Err(e) = &result {
        if e.kind == ErrorKind::FileMissing {
            if let Err(db) = catalog.run(move |c| repo::set_original_missing(c, id, true)).await {
                eprintln!("image {id}: recording missing original: {}", db.message);
            }
        }
    }
    result
}

/// As-shot white balance, develop-source sizes and render warnings (decodes the source if
/// not cached). `warnings` = the image's stored sidecar warnings + what `DevelopCache::info`
/// reports (profile/look availability, source colour; rust-engine-dev).
#[tauri::command]
#[specta::specta]
pub async fn get_develop_info(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
) -> AppResult<DevelopInfo> {
    let entry = catalog.run(move |c| repo::get_image(c, id)).await?;
    let src = source_of(&entry);
    if !matches!(entry.thumbnail, ThumbnailState::Pending) {
        develop.remember_source(src.clone());
    }
    let cache = develop.inner().clone();
    let info = blocking(move || cache.info(&src)).await;
    let mut info = note_if_missing(&catalog, id, info).await?;
    if entry.missing_since_ms.is_some() && Path::new(&entry.path).is_file() {
        // The original is back at its path (the source may come from the develop cache).
        catalog.run(move |c| repo::set_original_missing(c, id, false)).await?;
    }
    let mut warnings = entry.develop_warnings;
    warnings.append(&mut info.warnings);
    info.warnings = warnings;
    Ok(info)
}

/// White balance picker (IPC v11): the Temp/Tint that neutralizes the 5x5 develop-source
/// pixel neighbourhood around `point`. `point` is in the **sensor frame** (normalized 0..=1
/// of the un-oriented, uncropped image; same convention as masks: convert a viewer click with
/// `unorientPoint` + the crop mapping). `adjustments` = the live (unsaved) edit; only its
/// `profile` matters (colour matrices, as for `DevelopInfo.asShot`). The result is clamped to
/// the slider ranges; the caller commits `whiteBalance: custom` itself. Errors:
/// `invalid_argument` if the point is outside 0..=1 or the sample is clipped / too dark
/// (message is user-facing). Body: architect (thin wrapper over
/// `DevelopCache::sample_white_balance`; rust-engine-dev owns it from here).
#[tauri::command]
#[specta::specta]
pub async fn sample_white_balance(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
    point: NormPoint,
    adjustments: ParametricAdjustments,
) -> AppResult<WhiteBalanceValues> {
    adjustments.validate().map_err(AppError::invalid)?;
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let sampled = blocking(move || cache.sample_white_balance(&src, point, &adjustments)).await;
    note_if_missing(&catalog, id, sampled).await
}

/// Decodes `ids` into the develop cache in the background (e.g. filmstrip neighbours of
/// the image being edited). Returns immediately.
#[tauri::command]
#[specta::specta]
pub async fn prepare_develop(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    ids: Vec<ImageId>,
) -> AppResult<()> {
    let sources = source_images(&catalog, ids).await?;
    develop.prefetch(sources);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_history(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<AdjustmentHistory> {
    catalog.run(move |c| develop::history::history(c, id)).await
}

/// Steps back one history entry (no-op at the first). Marks the sidecar dirty.
#[tauri::command]
#[specta::specta]
pub async fn undo_adjustments(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    id: ImageId,
) -> AppResult<EditState> {
    let state = catalog.run(move |c| develop::history::undo(c, id)).await?;
    xmp.notify(&app);
    Ok(state)
}

/// Steps forward one history entry (no-op at the last). Marks the sidecar dirty.
#[tauri::command]
#[specta::specta]
pub async fn redo_adjustments(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    id: ImageId,
) -> AppResult<EditState> {
    let state = catalog.run(move |c| develop::history::redo(c, id)).await?;
    xmp.notify(&app);
    Ok(state)
}

/// Jumps to history entry `entryId` of image `id` (Lightroom's History panel click).
#[tauri::command]
#[specta::specta]
pub async fn goto_history(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    id: ImageId,
    entry_id: HistoryEntryId,
) -> AppResult<EditState> {
    let state = catalog.run(move |c| develop::history::goto(c, id, entry_id)).await?;
    xmp.notify(&app);
    Ok(state)
}

/// Pastes the `fields` groups of `adjustments` (the frontend's copied settings) onto
/// every image in `ids` (any selection; duplicates ignored); one "Paste Settings" history
/// entry per changed image. Atomic. v19: recorded as one undoable edit batch (kind `paste`,
/// `undo_edit_batch(result.batchId)`; `batchId = null` when nothing changed).
#[tauri::command]
#[specta::specta]
pub async fn paste_settings(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    adjustments: ParametricAdjustments,
    fields: Vec<AdjustmentField>,
) -> AppResult<EditBatchResult> {
    require_fields(&fields)?;
    adjustments.validate().map_err(AppError::invalid)?;
    let n = ids.len() as u32;
    let activity = start_multi_activity(&app, n, "Pasting settings to");
    let result = catalog
        .run(move |c| {
            develop::batches::apply_fields_recorded(c, &ids, &adjustments, &fields, develop::history::LABEL_PASTE)
        })
        .await;
    end_activity(activity, &result, |_| format!("Pasted settings to {}", super::activity::photos(n)));
    let r = result?;
    if !r.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

/// A `paste_sync` activity (IPC v18) for writes to more than one photo ("Pasting settings to
/// 12 photos"); `None` for a single photo or without a reporter.
fn start_multi_activity(app: &AppHandle, n: u32, verb: &str) -> Option<super::activity::ActivityHandle> {
    if n < 2 {
        return None;
    }
    let label = format!("{verb} {}", super::activity::photos(n));
    super::activity::activities(app).map(|a| a.start(super::events::ActivityKind::PasteSync, label, None))
}

/// Ends `activity` from a command result.
fn end_activity<T>(
    activity: Option<super::activity::ActivityHandle>,
    result: &AppResult<T>,
    ok: impl FnOnce(&T) -> String,
) {
    if let Some(a) = activity {
        match result {
            Ok(v) => a.finish(Some(ok(v))),
            Err(e) => a.fail(e.message.clone()),
        }
    }
}

/// Copies the `fields` groups of `sourceId`'s stored adjustments onto `targetIds`
/// ("Sync Settings"; any selection, duplicates ignored). Atomic. v19: one undoable edit batch
/// (kind `paste`), like `paste_settings`.
#[tauri::command]
#[specta::specta]
pub async fn sync_settings(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    source_id: ImageId,
    target_ids: Vec<ImageId>,
    fields: Vec<AdjustmentField>,
) -> AppResult<EditBatchResult> {
    require_fields(&fields)?;
    let n = target_ids.len() as u32;
    let activity = start_multi_activity(&app, n, "Syncing settings to");
    let result = catalog
        .run(move |c| {
            let src = repo::get_adjustments(c, source_id)?;
            develop::batches::apply_fields_recorded(c, &target_ids, &src, &fields, develop::history::LABEL_SYNC)
        })
        .await;
    end_activity(activity, &result, |_| format!("Synced settings to {}", super::activity::photos(n)));
    let r = result?;
    if !r.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

/// Auto Sync (v19.2): commits the active photo's edit `before` -> `after` to `sourceId` (its
/// stored settings become `after`) and the same change to every photo in `targetIds`, as one
/// undoable batch of kind `sync` (`undo_edit_batch(result.batch.batchId)` reverts the source
/// and all targets). Only the groups that differ between `before` and `after` are touched
/// (never crop / masks / transform); `options.relative` groups (default exposure + white
/// balance) are applied relatively, the others copied (see [`SyncDeltaOptions`]). In Auto
/// Sync mode the UI commits through this command **instead of** `save_adjustments` (one
/// call per committed edit). Duplicates and the source in `targetIds` are ignored. Resolving
/// an `as_shot` white balance decodes that photo (cached by the develop cache). Atomic;
/// unknown image -> `not_found`; invalid settings / options -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn sync_delta(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    xmp: State<'_, XmpSync>,
    source_id: ImageId,
    before: ParametricAdjustments,
    after: ParametricAdjustments,
    target_ids: Vec<ImageId>,
    options: Option<SyncDeltaOptions>,
) -> AppResult<SyncDeltaResult> {
    let options = options.unwrap_or_default();
    let (b, a, t, o) = (before.clone(), after.clone(), target_ids.clone(), options.clone());
    let need = catalog.run(move |c| develop::sync_delta::as_shot_needed(c, source_id, &b, &a, &t, &o)).await?;
    let mut as_shot = std::collections::HashMap::new();
    if !need.is_empty() {
        let sources = source_images(&catalog, need).await?;
        let cache = develop.inner().clone();
        let found = blocking(move || {
            use rayon::prelude::*;
            Ok(sources
                .par_iter()
                .filter_map(|src| cache.info(src).ok().and_then(|i| i.as_shot).map(|v| (src.id, v)))
                .collect::<Vec<_>>())
        })
        .await?;
        as_shot.extend(found);
    }
    let n = target_ids.len() as u32 + 1;
    let activity = start_multi_activity(&app, n, "Syncing settings to");
    let result = catalog
        .run(move |c| {
            develop::sync_delta::sync_delta_recorded(c, source_id, &before, &after, &target_ids, &options, &as_shot)
        })
        .await;
    end_activity(activity, &result, |_| format!("Synced settings to {}", super::activity::photos(n)));
    let r = result?;
    if !r.batch.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

/// Resets `ids` to neutral adjustments ("Reset" history entry). Atomic.
#[tauri::command]
#[specta::specta]
pub async fn reset_adjustments(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
) -> AppResult<()> {
    catalog
        .run(move |c| {
            // Neutral depends on the source (non-RAW: no profile, no default sharpening).
            // Resolve every id first so an unknown id writes nothing.
            let mut raw_ids = Vec::new();
            let mut other_ids = Vec::new();
            for &id in &ids {
                if repo::image_format(c, id)?.is_raw() {
                    raw_ids.push(id);
                } else {
                    other_ids.push(id);
                }
            }
            for (group, format) in [(raw_ids, ImageFormat::Arw), (other_ids, ImageFormat::Jpeg)] {
                if !group.is_empty() {
                    let neutral = ParametricAdjustments::defaults_for(format);
                    develop::history::apply_fields(
                        c,
                        &group,
                        &neutral,
                        AdjustmentField::ALL,
                        develop::history::LABEL_RESET,
                    )?;
                }
            }
            Ok(())
        })
        .await?;
    xmp.notify(&app);
    Ok(())
}

/// Applies preset `presetId` to `ids` ("Preset: <name>" entry per changed image). Atomic.
/// Sieve presets copy their `fields` groups; imported Lightroom presets (v14) set exactly the
/// `crs:` settings they contain and leave every other setting alone (`styles::resolve_preset`).
#[tauri::command]
#[specta::specta]
pub async fn apply_preset(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    preset_id: PresetId,
) -> AppResult<()> {
    catalog.run(move |c| styles::apply_preset(c, &ids, preset_id)).await?;
    xmp.notify(&app);
    Ok(())
}

/// Presets sorted by name.
#[tauri::command]
#[specta::specta]
pub async fn list_presets(catalog: State<'_, Catalog>) -> AppResult<Vec<Preset>> {
    catalog.run(|c| develop::presets::list(c)).await
}

/// Creates (`id = null`) or overwrites a preset. Names are unique (case-insensitive).
#[tauri::command]
#[specta::specta]
pub async fn save_preset(
    catalog: State<'_, Catalog>,
    id: Option<PresetId>,
    name: String,
    adjustments: ParametricAdjustments,
    fields: Vec<AdjustmentField>,
) -> AppResult<Preset> {
    require_fields(&fields)?;
    catalog.run(move |c| develop::presets::save(c, id, &name, &adjustments, &fields)).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_preset(catalog: State<'_, Catalog>, id: PresetId) -> AppResult<()> {
    catalog.run(move |c| develop::presets::delete(c, id)).await
}

/// LUTs in the library, sorted by name.
#[tauri::command]
#[specta::specta]
pub async fn list_luts(luts: State<'_, LutLibrary>) -> AppResult<Vec<LutInfo>> {
    let luts = luts.inner().clone();
    blocking(move || luts.list()).await
}

/// Validates and copies a `.cube` file into the library. Idempotent per file content.
/// Invalid file -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn import_lut(luts: State<'_, LutLibrary>, path: String) -> AppResult<LutInfo> {
    let luts = luts.inner().clone();
    blocking(move || luts.import(Path::new(&path))).await
}

/// Deletes a LUT from the library. If images still reference it, fails with
/// `invalid_argument` unless `force` (they then render without it, `lutMissing`).
#[tauri::command]
#[specta::specta]
pub async fn delete_lut(
    catalog: State<'_, Catalog>,
    luts: State<'_, LutLibrary>,
    id: LutId,
    force: bool,
) -> AppResult<()> {
    if !is_valid_lut_id(&id) {
        return Err(AppError::invalid(format!("invalid LUT id {id:?}")));
    }
    let key = id.clone();
    let refs = catalog.run(move |c| lut::references(c, &key)).await?;
    if refs > 0 && !force {
        return Err(AppError::invalid(format!("LUT {id} is used by {refs} image(s)")));
    }
    let luts = luts.inner().clone();
    blocking(move || luts.delete(&id)).await
}

// ---------------------------------------------------------------------------
// Export (Phase 6)
// ---------------------------------------------------------------------------

/// Which formats can be written here (WebP/HEIC depend on encoders), plus concurrency limits.
#[tauri::command]
#[specta::specta]
pub async fn get_export_capabilities(exporter: State<'_, Exporter>) -> AppResult<ExportCapabilities> {
    Ok(exporter.capabilities())
}

/// Built-in presets (read-only, negative ids) first, then user presets by name.
#[tauri::command]
#[specta::specta]
pub async fn list_export_presets(catalog: State<'_, Catalog>) -> AppResult<Vec<ExportPreset>> {
    catalog.run(|c| export::presets::list(c)).await
}

/// Creates (`id = null`) or overwrites a user preset. Built-ins cannot be overwritten
/// (`invalid_argument`; save under a new name instead). Names are unique (case-insensitive).
#[tauri::command]
#[specta::specta]
pub async fn save_export_preset(
    catalog: State<'_, Catalog>,
    id: Option<ExportPresetId>,
    name: String,
    settings: ExportSettings,
) -> AppResult<ExportPreset> {
    settings.validate().map_err(AppError::invalid)?;
    catalog.run(move |c| export::presets::save(c, id, &name, &settings)).await
}

/// Deletes a user preset (built-ins -> `invalid_argument`).
#[tauri::command]
#[specta::specta]
pub async fn delete_export_preset(catalog: State<'_, Catalog>, id: ExportPresetId) -> AppResult<()> {
    catalog.run(move |c| export::presets::delete(c, id)).await
}

/// Dry run: resolved output paths and which already exist (for the export dialog's
/// "N files exist" warning). No pixels are developed.
#[tauri::command]
#[specta::specta]
pub async fn plan_export(
    catalog: State<'_, Catalog>,
    ids: Vec<ImageId>,
    settings: ExportSettings,
) -> AppResult<ExportPlan> {
    let ids = export_request(ids, &settings)?;
    catalog.run(move |c| export::plan(c, &ids, &settings)).await
}

/// Queues an export of `ids` (in this order; `{seq}` follows it; duplicates dropped) with
/// `settings` and returns the `queued` job immediately. Adjustments are snapshotted now.
/// `presetName` labels the job. Progress: `exportProgress`; end: `exportFinished`.
/// `settings.destination` must not be `choose`. Unknown ids -> `not_found`, nothing queued.
#[tauri::command]
#[specta::specta]
pub async fn export_images(
    app: AppHandle,
    exporter: State<'_, Exporter>,
    ids: Vec<ImageId>,
    settings: ExportSettings,
    preset_name: Option<String>,
) -> AppResult<ExportJob> {
    let ids = export_request(ids, &settings)?;
    let exporter = exporter.inner().clone();
    blocking(move || exporter.enqueue(&app, ids, settings, preset_name)).await
}

/// Cancels a queued or running job (files already written are kept). No-op for finished
/// jobs; unknown id -> `not_found`. The job ends with `exportFinished { cancelled: true }`.
#[tauri::command]
#[specta::specta]
pub async fn cancel_export(app: AppHandle, exporter: State<'_, Exporter>, job_id: ExportJobId) -> AppResult<()> {
    exporter.cancel(&app, job_id)
}

/// Queued/running jobs first, then recent finished jobs (newest first, max 50).
#[tauri::command]
#[specta::specta]
pub async fn get_export_jobs(exporter: State<'_, Exporter>) -> AppResult<Vec<ExportJob>> {
    let exporter = exporter.inner().clone();
    blocking(move || exporter.jobs()).await
}

/// Validates an export request; returns `ids` de-duplicated (first occurrence kept).
fn export_request(ids: Vec<ImageId>, settings: &ExportSettings) -> AppResult<Vec<ImageId>> {
    if ids.is_empty() {
        return Err(AppError::invalid("ids must not be empty"));
    }
    settings.validate().map_err(AppError::invalid)?;
    if settings.destination == ExportDestination::Choose {
        return Err(AppError::invalid("choose a destination folder before exporting"));
    }
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    Ok(ids.into_iter().filter(|id| seen.insert(*id)).collect())
}

// ---------------------------------------------------------------------------
// Analysis (Phase 3)
// ---------------------------------------------------------------------------

/// Queues `scope` for the background analysis worker and returns immediately.
/// Progress: `analysisProgress`, `analysisReady`, `analysisFailed`, `analysisFinished`.
#[tauri::command]
#[specta::specta]
pub async fn analyze_images(app: AppHandle, analysis: State<'_, Analysis>, scope: AnalysisScope) -> AppResult<()> {
    analysis.start(&app, scope)
}

/// Stops the worker after the images in flight; remaining work stays pending.
#[tauri::command]
#[specta::specta]
pub async fn cancel_analysis(analysis: State<'_, Analysis>) -> AppResult<()> {
    analysis.cancel();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_analysis_status(
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
) -> AppResult<AnalysisStatus> {
    let running = analysis.is_running();
    catalog.run(move |c| ml::analysis_status(c, running)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_auto_analyze(catalog: State<'_, Catalog>, enabled: bool) -> AppResult<()> {
    catalog.run(move |c| repo::set_auto_analyze(c, enabled)).await
}

/// Effective thresholds for `shootType` (stored overrides over the defaults).
#[tauri::command]
#[specta::specta]
pub async fn get_cull_thresholds(catalog: State<'_, Catalog>, shoot_type: ShootType) -> AppResult<CullThresholds> {
    catalog.run(move |c| repo::cull_thresholds(c, shoot_type)).await
}

/// Stores thresholds for `shootType` (`null` resets to defaults). Kicks a rescore when
/// `shootType` is the catalog's current shoot type.
#[tauri::command]
#[specta::specta]
pub async fn set_cull_thresholds(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    shoot_type: ShootType,
    thresholds: Option<CullThresholds>,
) -> AppResult<()> {
    let current = catalog
        .run(move |c| {
            repo::set_cull_thresholds(c, shoot_type, thresholds.as_ref())?;
            repo::shoot_type(c)
        })
        .await?;
    if current == shoot_type {
        analysis.start(&app, AnalysisScope::Rescore)?;
    }
    Ok(())
}

/// Faces from the last analysis (empty if unanalyzed); for face-crop zoom.
#[tauri::command]
#[specta::specta]
pub async fn get_faces(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<Vec<FaceInfo>> {
    catalog.run(move |c| repo::get_faces(c, id)).await
}

/// Burst groups with members, optionally limited to groups touching `folderId` AND
/// `projectId` (v14).
#[tauri::command]
#[specta::specta]
pub async fn list_burst_groups(
    catalog: State<'_, Catalog>,
    folder_id: Option<FolderId>,
    project_id: Option<ProjectId>,
) -> AppResult<Vec<BurstGroup>> {
    catalog.run(move |c| repo::list_burst_groups(c, FolderScope::resolve(c, folder_id, project_id)?)).await
}

/// Copies the engine's suggested rating/pick into the user's rating/pick for `ids`.
/// Unanalyzed images and images already matching their suggestion (v18.1) are skipped; with
/// `onlyUnset`, so are images already flagged or rated (`pick != unflagged` or `rating != 0`).
/// With `onlyUnset` over a project it changes exactly the `CullSummary.suggested*Pending`
/// photos. v19.2 `kinds` (`null` = all): copy only suggested picks / rejects / stars (see
/// [`SuggestionKinds`]), e.g. `{picks: false, rejects: true, stars: false}` flags only the
/// suggested rejects. Atomic; unknown ids -> `not_found`.
/// For undo, take `get_cull_snapshot(ids)` first.
#[tauri::command]
#[specta::specta]
pub async fn apply_suggestions(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    only_unset: bool,
    kinds: Option<SuggestionKinds>,
) -> AppResult<ApplySuggestionsResult> {
    let kinds = kinds.unwrap_or_default();
    let result = catalog.run(move |c| repo::apply_suggestions_kinds(c, &ids, only_unset, kinds)).await?;
    if result.applied > 0 {
        xmp.notify(&app);
    }
    Ok(result)
}

/// Makes `imageId` the keeper of burst `groupId`: it loses `duplicate_burst`, the other
/// members gain it (suppressed/user tag rows are left alone). The choice is pinned, so
/// regrouping keeps it. Kicks a rescore so suggested rating/pick follow the new keeper
/// (`analysisFinished` when done). Unknown group/image -> `not_found`; image not a member ->
/// `invalid_argument`. Returns the updated group.
#[tauri::command]
#[specta::specta]
pub async fn set_burst_keeper(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    xmp: State<'_, XmpSync>,
    group_id: BurstGroupId,
    image_id: ImageId,
) -> AppResult<BurstGroup> {
    let group = catalog
        .run(move |c| {
            ml::store::set_burst_keeper(c, group_id, image_id)?;
            repo::list_burst_groups(c, None)?
                .into_iter()
                .find(|g| g.id == group_id)
                .ok_or_else(|| AppError::not_found(format!("burst group {group_id}")))
        })
        .await?;
    xmp.notify(&app);
    analysis.start(&app, AnalysisScope::Rescore)?;
    Ok(group)
}

// ---------------------------------------------------------------------------
// Culling undo & UI preferences (v8)
// ---------------------------------------------------------------------------

/// Current rating/pick/label of `ids` (in order), to push on a culling undo stack before a
/// change. Unknown ids -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_cull_snapshot(catalog: State<'_, Catalog>, ids: Vec<ImageId>) -> AppResult<Vec<CullSnapshot>> {
    catalog.run(move |c| repo::cull_snapshot(c, &ids)).await
}

/// Writes snapshots back (undo/redo). Atomic: unknown id -> `not_found`, rating > 5 ->
/// `invalid_argument`, nothing written. Changed images become XMP-dirty (auto-sync notified).
/// Returns the ids whose values changed (refetch them with `get_images`).
#[tauri::command]
#[specta::specta]
pub async fn restore_cull_snapshot(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    snapshots: Vec<CullSnapshot>,
) -> AppResult<Vec<ImageId>> {
    let changed = catalog.run(move |c| repo::restore_cull_snapshot(c, &snapshots)).await?;
    if !changed.is_empty() {
        xmp.notify(&app);
    }
    Ok(changed)
}

/// Per-catalog UI preferences (defaults when never set).
#[tauri::command]
#[specta::specta]
pub async fn get_ui_prefs(catalog: State<'_, Catalog>) -> AppResult<UiPrefs> {
    catalog.run(|c| repo::ui_prefs(c)).await
}

/// Replaces the stored UI preferences (read-modify-write: send the full struct).
#[tauri::command]
#[specta::specta]
pub async fn set_ui_prefs(catalog: State<'_, Catalog>, prefs: UiPrefs) -> AppResult<()> {
    catalog.run(move |c| repo::set_ui_prefs(c, &prefs)).await
}

/// Reveals `path` (file or folder) in Finder, selected. Must be absolute and exist
/// (`invalid_argument` / `not_found`). macOS only (`internal` elsewhere).
#[tauri::command]
#[specta::specta]
pub async fn reveal_in_finder(path: String) -> AppResult<()> {
    blocking(move || reveal(Path::new(&path))).await
}

fn validate_reveal_path(path: &Path) -> AppResult<()> {
    if !path.is_absolute() {
        return Err(AppError::invalid(format!("path must be absolute: {}", path.display())));
    }
    if !path.exists() {
        return Err(AppError::not_found(format!("{} does not exist", path.display())));
    }
    Ok(())
}

fn reveal(path: &Path) -> AppResult<()> {
    validate_reveal_path(path)?;
    if !cfg!(target_os = "macos") {
        return Err(AppError::internal("reveal_in_finder is only supported on macOS"));
    }
    // No shell: the path is a single argument. `open -R` returns once Finder has the request.
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .status()
        .map_err(|e| AppError::internal(format!("open -R: {e}")))?;
    if !status.success() {
        return Err(AppError::internal(format!("open -R exited with {status}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 8 hardening (IPC v13): missing originals, catalog backups
// ---------------------------------------------------------------------------

/// Points folder `folderId` at `newPath` (the shoot was moved, renamed or its drive mounted
/// elsewhere). Each image is found at its path relative to the old folder, else by file
/// name anywhere under `newPath`; found images are repointed and their missing flag
/// cleared, the others stay flagged missing (`stillMissing`). Errors (nothing changed):
/// `not_found` for an unknown folder; `invalid_argument` when `newPath` is not a folder, is
/// already another catalog folder, or holds none of the folder's photos. Develop sources
/// of the moved images are forgotten, and thumbnails that failed because the file was
/// missing are re-extracted. Refetch `get_catalog_state` / the visible rows afterwards.
#[tauri::command]
#[specta::specta]
pub async fn relocate_folder(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    folder_id: FolderId,
    new_path: String,
) -> AppResult<RelocateResult> {
    let relocated = catalog.run(move |c| repo::relocate_folder(c, folder_id, Path::new(&new_path))).await?;
    if let Some(develop) = app.try_state::<DevelopCache>() {
        develop.forget_sources(Some(&relocated.moved));
    }
    if !relocated.retry_thumbnails.is_empty() {
        ingest.regenerate(&app, relocated.retry_thumbnails)?;
    }
    Ok(relocated.result)
}

/// Stages automatic backup `index` (`CatalogState.health.backups[].index`, 1 = newest) to
/// replace the catalog at the next launch; the current catalog is kept aside as
/// `<catalog>.corrupt-<ms>`. The UI tells the user to relaunch; changes made before the
/// relaunch are lost. Returns the updated health (`restorePending: true`). Errors:
/// `not_found` (no such backup), `invalid_argument` (the backup is damaged too).
#[tauri::command]
#[specta::specta]
pub async fn restore_catalog_backup(catalog: State<'_, Catalog>, index: u32) -> AppResult<CatalogHealth> {
    let path = catalog.path.clone();
    blocking(move || {
        db::stage_restore(&path, index as usize)?;
        Ok(db::health_state(&path))
    })
    .await
}

// ---------------------------------------------------------------------------
// XMP sidecars (Phase 4)
// ---------------------------------------------------------------------------

/// Runs blocking file/DB work off the async runtime.
async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::internal(e.to_string()))?
}

/// Writes `<basename>.xmp` sidecars for `ids` now (catalog wins; unrelated XMP fields are
/// preserved). Unknown ids -> `not_found`; per-file errors are listed in the report.
#[tauri::command]
#[specta::specta]
pub async fn write_xmp(app: AppHandle, xmp: State<'_, XmpSync>, ids: Vec<ImageId>) -> AppResult<XmpSyncReport> {
    let sync = xmp.inner().clone();
    blocking(move || write_xmp_reporting(&app, &sync, &ids)).await
}

/// Explicit XMP save with an `xmp_save` activity (IPC v18).
fn write_xmp_reporting(app: &AppHandle, sync: &XmpSync, ids: &[ImageId]) -> AppResult<XmpSyncReport> {
    use super::activity::{activities, xmp_message};
    use super::events::ActivityKind;
    let Some(a) = activities(app).filter(|_| !ids.is_empty()) else { return sync.write_images(ids) };
    let handle = a.start(ActivityKind::XmpSave, "Saving metadata to XMP", Some(ids.len() as u32));
    let result = sync.write_images_with(ids, &|done| handle.progress(done));
    match result {
        Ok(report) => {
            handle.finish(Some(xmp_message(report.succeeded as usize, report.failed.len())));
            Ok(report)
        }
        Err(e) => {
            handle.fail(e.message.clone());
            Err(e)
        }
    }
}

/// "Save all": writes sidecars for every XMP-dirty image of `folderId` (all folders for
/// `null`) now, whether or not auto-sync is on (catalog wins, like `write_xmp`).
#[tauri::command]
#[specta::specta]
pub async fn write_xmp_all_dirty(
    app: AppHandle,
    xmp: State<'_, XmpSync>,
    folder_id: Option<FolderId>,
) -> AppResult<XmpSyncReport> {
    let sync = xmp.inner().clone();
    blocking(move || {
        let ids = sync.dirty_ids(folder_id)?;
        write_xmp_reporting(&app, &sync, &ids)
    })
    .await
}

/// Reads rating/pick/label (and crs: develop settings, see `xmp::crs`) from existing sidecars
/// of `ids` into the catalog (sidecar wins).
/// Images without a sidecar are `skipped`; refetch `report.changed`.
#[tauri::command]
#[specta::specta]
pub async fn read_xmp(xmp: State<'_, XmpSync>, ids: Vec<ImageId>) -> AppResult<XmpSyncReport> {
    let sync = xmp.inner().clone();
    blocking(move || sync.read_images(&ids)).await
}

/// Turns automatic (debounced) sidecar writing on/off. Enabling flushes every dirty image.
#[tauri::command]
#[specta::specta]
pub async fn set_xmp_auto_sync(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    enabled: bool,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_xmp_auto_sync(c, enabled)).await?;
    if enabled {
        xmp.notify(&app);
    }
    Ok(())
}

/// Every image whose last sidecar write / read failed (`XmpSyncState.error`), in capture
/// order, scoped like `get_filter_counts` (`projectId` `null` = whole catalog) (v15). Unknown
/// project -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn list_xmp_failures(
    catalog: State<'_, Catalog>,
    project_id: Option<ProjectId>,
) -> AppResult<Vec<XmpFailure>> {
    catalog.run(move |c| repo::xmp_failures(c, &FolderScope::resolve(c, None, project_id)?)).await
}

/// Re-reads sidecars another app (Lightroom, Bridge) changed since Sieve last wrote / read them,
/// for `projectId`'s folders (`null` = every catalog folder) (v18). Only sidecars whose mtime
/// changed are read; images with unsaved catalog changes are left to auto-sync (newer wins).
/// Returns the images whose rating / flag / label / develop settings changed (refetch them with
/// `get_images`). Call on project open and on window focus (debounced). Unknown project ->
/// `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn refresh_sidecars(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    develop: State<'_, DevelopCache>,
    analysis: State<'_, Analysis>,
    project_id: Option<ProjectId>,
) -> AppResult<Vec<ImageId>> {
    let folders: Vec<FolderId> = catalog
        .run(move |c| match project_id {
            Some(p) => projects::project_folder_ids(c, p),
            None => Ok(c
                .prepare("SELECT id FROM folders ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?),
        })
        .await?;
    if folders.is_empty() {
        return Ok(Vec::new());
    }
    let sync = xmp.inner().clone();
    let changed = blocking(move || sync.refresh_folders(&folders)).await?;
    if !changed.is_empty() {
        // Like import's sidecar read: cached develop state of these images is stale.
        develop.forget_sources(Some(&changed));
    }
    if xmp.take_capture_time_changes() {
        // Lightroom-corrected capture times: bursts and scenes regroup on the new order.
        analysis.start(&app, AnalysisScope::Rescore)?;
    }
    Ok(changed)
}

#[tauri::command]
#[specta::specta]
pub async fn get_xmp_status(catalog: State<'_, Catalog>, xmp: State<'_, XmpSync>) -> AppResult<XmpStatus> {
    let running = xmp.is_running();
    catalog.run(move |c| repo::xmp_status(c, running)).await
}

// ---------------------------------------------------------------------------
// Scenes & scene matching (Phase 7)
// ---------------------------------------------------------------------------

/// Groups the images of `folderId` AND `projectId` (v14; both `null` = all folders; scenes
/// never span folders) into scenes by capture-time gaps
/// and appearance similarity (`options` `null` = defaults). Replaces the `auto` scenes in scope
/// (and `manual` ones if `replaceManual`); members of kept manual scenes are not regrouped;
/// anchor flags survive regrouping. Blocking until done (first run computes preview features,
/// ~10 ms/image in parallel; later runs reuse them); progress via `sceneProgress {task:
/// "detect"}`. Returns `list_scenes(folderId)`.
#[tauri::command]
#[specta::specta]
pub async fn detect_scenes(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    folder_id: Option<FolderId>,
    project_id: Option<ProjectId>,
    options: Option<SceneDetectOptions>,
) -> AppResult<Vec<Scene>> {
    let options = options.unwrap_or_default();
    options.validate().map_err(AppError::invalid)?;
    let replace_manual = options.replace_manual;
    let scope = catalog.run(move |c| FolderScope::resolve(c, folder_id, project_id)).await?;
    let frames_scope = scope.clone();
    let mut frames = catalog.run(move |c| scene::store::detection_frames(c, &frames_scope, replace_manual)).await?;
    let progress = scene::progress_emitter(app, SceneTask::Detect);
    let (frames, computed) = blocking(move || {
        let computed = scene::features::compute_missing(&mut frames, &progress);
        Ok((frames, computed))
    })
    .await?;
    catalog
        .run(move |c| {
            scene::store::save_features(c, &computed)?;
            let groups = scene::detect::group(&frames, &options);
            scene::store::replace_scenes(c, &scope, &groups, options.replace_manual)
        })
        .await
}

/// Scenes with a member in `folderId` AND `projectId` (v14; both `null` = all), in capture
/// order.
#[tauri::command]
#[specta::specta]
pub async fn list_scenes(
    catalog: State<'_, Catalog>,
    folder_id: Option<FolderId>,
    project_id: Option<ProjectId>,
) -> AppResult<Vec<Scene>> {
    catalog.run(move |c| scene::store::list_scenes(c, FolderScope::resolve(c, folder_id, project_id)?)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_scene(catalog: State<'_, Catalog>, id: SceneId) -> AppResult<Scene> {
    catalog.run(move |c| scene::store::get_scene(c, id)).await
}

/// New `manual` scene from `imageIds` (non-empty; moved out of their current scenes; scenes
/// left empty are deleted).
#[tauri::command]
#[specta::specta]
pub async fn create_scene(catalog: State<'_, Catalog>, image_ids: Vec<ImageId>) -> AppResult<Scene> {
    catalog.run(move |c| scene::store::create_scene(c, &image_ids)).await
}

/// Replaces the members of scene `id` (non-empty; images in other scenes are moved in).
/// The scene becomes `manual`; anchors that stay members are kept.
#[tauri::command]
#[specta::specta]
pub async fn set_scene_members(catalog: State<'_, Catalog>, id: SceneId, image_ids: Vec<ImageId>) -> AppResult<Scene> {
    catalog.run(move |c| scene::store::set_scene_members(c, id, &image_ids)).await
}

/// Marks the graded anchors of scene `id`: 0..=2 members (`[]` clears). Keeps the method.
#[tauri::command]
#[specta::specta]
pub async fn set_scene_anchors(catalog: State<'_, Catalog>, id: SceneId, anchor_ids: Vec<ImageId>) -> AppResult<Scene> {
    catalog.run(move |c| scene::store::set_scene_anchors(c, id, &anchor_ids)).await
}

/// Merges `ids` (>= 2 scenes) into `ids[0]` (`manual`; anchors kept up to 2, in `ids` order).
#[tauri::command]
#[specta::specta]
pub async fn merge_scenes(catalog: State<'_, Catalog>, ids: Vec<SceneId>) -> AppResult<Scene> {
    catalog.run(move |c| scene::store::merge_scenes(c, &ids)).await
}

/// Splits scene `id` before member `firstImageId` (capture order; not the first member).
/// Returns `[id, newScene]`, both `manual`; anchors follow their images.
#[tauri::command]
#[specta::specta]
pub async fn split_scene(catalog: State<'_, Catalog>, id: SceneId, first_image_id: ImageId) -> AppResult<Vec<Scene>> {
    catalog.run(move |c| scene::store::split_scene(c, id, first_image_id)).await
}

/// Deletes scene `id` (its images become unassigned; adjustments untouched).
#[tauri::command]
#[specta::specta]
pub async fn delete_scene(catalog: State<'_, Catalog>, id: SceneId) -> AppResult<()> {
    catalog.run(move |c| scene::store::delete_scene(c, id)).await
}

/// Proposes relative grades for `targetIds` from 1-2 graded `anchorIds` (nothing is saved).
/// Anchors: 1..=2 distinct images; anchors listed in `targetIds` are skipped; duplicates
/// dropped; no targets left or more than `MatchOptions::MAX_TARGETS` -> `invalid_argument`;
/// unknown ids -> `not_found`. Blocking until all previews are ready (renders at 640 px; the
/// first render of an image decodes its RAW); progress via `sceneProgress {task: "match"}`.
/// Result in `targetIds` order.
#[tauri::command]
#[specta::specta]
pub async fn match_scene(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    anchor_ids: Vec<ImageId>,
    target_ids: Vec<ImageId>,
    options: MatchOptions,
) -> AppResult<Vec<MatchPreview>> {
    options.validate().map_err(AppError::invalid)?;
    let mut anchors: Vec<ImageId> = Vec::new();
    for id in anchor_ids {
        if !anchors.contains(&id) {
            anchors.push(id);
        }
    }
    if anchors.is_empty() || anchors.len() > Scene::MAX_ANCHORS {
        return Err(AppError::invalid(format!("match_scene needs 1..={} anchors", Scene::MAX_ANCHORS)));
    }
    let mut targets: Vec<ImageId> = Vec::new();
    for id in target_ids {
        if !anchors.contains(&id) && !targets.contains(&id) {
            targets.push(id);
        }
    }
    if targets.is_empty() {
        return Err(AppError::invalid("no targets besides the anchors"));
    }
    if targets.len() > MatchOptions::MAX_TARGETS {
        return Err(AppError::invalid(format!("at most {} targets per call", MatchOptions::MAX_TARGETS)));
    }
    let (anchors, targets) = catalog
        .run(move |c| Ok((scene::store::match_inputs(c, &anchors)?, scene::store::match_inputs(c, &targets)?)))
        .await?;
    let cache = develop.inner().clone();
    let luts = luts.inner().clone();
    let progress = scene::progress_emitter(app, SceneTask::Match);
    blocking(move || scene::matching::match_images(&cache, &luts, &anchors, &targets, &options, &progress)).await
}

/// Commits scene-match results: one history entry labelled `label` (default "Match Scene")
/// per image whose adjustments change. Atomic (unknown id -> `not_found`, invalid values ->
/// `invalid_argument`, nothing written). Marks sidecars dirty (crs:). Returns the changed ids.
#[tauri::command]
#[specta::specta]
pub async fn apply_scene_match(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    applications: Vec<MatchApplication>,
    label: Option<String>,
) -> AppResult<Vec<ImageId>> {
    let label = label.unwrap_or_else(|| scene::LABEL_MATCH.to_string());
    let mut items: Vec<(ImageId, ParametricAdjustments)> = Vec::with_capacity(applications.len());
    for a in applications {
        if items.iter().any(|(id, _)| *id == a.image_id) {
            return Err(AppError::invalid(format!("image {} listed twice", a.image_id)));
        }
        items.push((a.image_id, a.adjustments));
    }
    let changed = catalog.run(move |c| develop::history::commit_batch(c, &items, &label)).await?;
    if !changed.is_empty() {
        xmp.notify(&app);
    }
    Ok(changed)
}

/// Render-space statistics of image `id` rendered with `adjustments` (`null` = its stored
/// adjustments), whole frame or `region` (oriented, normalized). Same pixels as the editor
/// (LUT included) at 640 px. Used to verify matches (mean luma / neutral within tolerance).
#[tauri::command]
#[specta::specta]
pub async fn get_render_stats(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    id: ImageId,
    adjustments: Option<ParametricAdjustments>,
    region: Option<NormRect>,
) -> AppResult<ImageStats> {
    if let Some(a) = &adjustments {
        a.validate().map_err(AppError::invalid)?;
    }
    if let Some(r) = region {
        RenderOptions { max_edge: scene::STATS_MAX_EDGE, slot: RenderSlot::Detail, region: Some(r) }
            .validate()
            .map_err(AppError::invalid)?;
    }
    let input = catalog.run(move |c| scene::store::match_inputs(c, &[id])).await?.remove(0);
    let adjustments = adjustments.unwrap_or_else(|| input.adjustments.clone());
    let cache = develop.inner().clone();
    let luts = luts.inner().clone();
    blocking(move || scene::stats::render_stats(&cache, &luts, &input.src, &adjustments, region)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::error::ErrorKind;

    #[test]
    fn reveal_validates_path() {
        assert_eq!(validate_reveal_path(Path::new("relative/x.jpg")).unwrap_err().kind, ErrorKind::InvalidArgument);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(validate_reveal_path(&dir.path().join("missing.jpg")).unwrap_err().kind, ErrorKind::NotFound);
        let file = dir.path().join("a b;$(x).jpg");
        std::fs::write(&file, b"x").unwrap();
        validate_reveal_path(&file).unwrap();
        validate_reveal_path(dir.path()).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Profiles (Phase 7b)
// ---------------------------------------------------------------------------

/// Profile browser contents for image `id`: camera profiles (DCPs) installed for its camera
/// and the installed looks (read in place from the user's Adobe installation; empty lists
/// when none are installed), plus (v14) the style library's imported looks / DCPs for this
/// camera (`styleId` set, `group` = style group name) and every LUT profile (`luts`).
/// Select one by saving `adjustments.profile` / `adjustments.lut` (`applyStyleProfile`).
#[tauri::command]
#[specta::specta]
pub async fn list_profiles(
    catalog: State<'_, Catalog>,
    profiles: State<'_, ProfileLibrary>,
    luts: State<'_, LutLibrary>,
    id: ImageId,
) -> AppResult<ProfileCatalog> {
    let entry = catalog.run(move |c| repo::get_image(c, id)).await?;
    let camera = camera_key(&entry);
    let mut listing = profiles.catalog(entry.id, &camera);
    let luts = luts.inner().clone();
    catalog
        .run(move |c| {
            styles::sync_lut_library(c, &luts)?;
            styles::extend_profile_catalog(c, &mut listing, &camera)?;
            Ok(listing)
        })
        .await
}

fn camera_key(entry: &RawImageEntry) -> CameraKey {
    CameraKey {
        format: entry.format,
        // Adobe's spelling of the makes Sieve identifies ("Sony ILCE-7M4", "Fujifilm X-M5").
        make: match entry.camera.make {
            CameraMake::Sony => Some("Sony".to_owned()),
            CameraMake::Fujifilm => Some("Fujifilm".to_owned()),
            CameraMake::Canon => Some("Canon".to_owned()),
            CameraMake::Other => None,
        },
        model: entry.camera.model.clone(),
    }
}

// ---------------------------------------------------------------------------
// Masks / local adjustments (Phase 7c, IPC v10)
// ---------------------------------------------------------------------------

fn preview_path(entry: &RawImageEntry) -> Option<PathBuf> {
    match &entry.thumbnail {
        ThumbnailState::Ready { preview_path: Some(p), .. } => Some(PathBuf::from(p)),
        _ => None,
    }
}

fn source_of(entry: &RawImageEntry) -> SourceImage {
    SourceImage { id: entry.id, path: PathBuf::from(&entry.path), orientation: entry.orientation }
}

/// The image's stored mask groups (`getAdjustments(id).masks`) + the render status of each
/// AI component (`MaskCache::status`, rust-engine-dev).
#[tauri::command]
#[specta::specta]
pub async fn list_masks(
    catalog: State<'_, Catalog>,
    masks: State<'_, MaskCache>,
    segmenter: State<'_, Segmenter>,
    id: ImageId,
) -> AppResult<MaskList> {
    let masks = masks.inner().clone();
    let segmenter = segmenter.inner().clone();
    catalog
        .run(move |c| {
            let groups = repo::get_adjustments(c, id)?.masks;
            let ai = masks.status(c, id, &groups, &segmenter)?;
            Ok(MaskList { image_id: id, groups, ai })
        })
        .await
}

/// Replaces the image's mask groups (everything else in its adjustments is kept) with one
/// history entry labelled `label` (e.g. "Mask: Brush", "Mask: Exposure"; same-label saves
/// within 1.5 s coalesce, so saving on every stroke / slider release is fine). XMP dirty.
#[tauri::command]
#[specta::specta]
pub async fn save_masks(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    id: ImageId,
    masks: Vec<MaskGroup>,
    label: String,
) -> AppResult<AdjustmentHistory> {
    validate_masks(&masks).map_err(AppError::invalid)?;
    let history = catalog
        .run(move |c| {
            let mut adj = repo::get_adjustments(c, id)?;
            adj.masks = masks;
            develop::history::commit(c, id, &adj, &label)
        })
        .await?;
    xmp.notify(&app);
    Ok(history)
}

/// Computes (or returns the cached) AI matte for `request` on image `id`; resolves when
/// done (first run per image and kind: model load + inference; cached: instant). Put
/// `digest` into the component's `AiMask.digest` and save. `invalid` when the family is
/// unavailable (`getMaskCapabilities`).
#[tauri::command]
#[specta::specta]
pub async fn compute_ai_mask(
    catalog: State<'_, Catalog>,
    segmenter: State<'_, Segmenter>,
    id: ImageId,
    request: AiMaskRequest,
) -> AppResult<AiMaskInfo> {
    if let Some(p) = &request.reference_point {
        if ![p.x, p.y].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            return Err(AppError::invalid("referencePoint must lie within 0..=1"));
        }
    }
    let entry = catalog.run(move |c| repo::get_image(c, id)).await?;
    let segmenter = segmenter.inner().clone();
    blocking(move || segmenter.compute(&source_of(&entry), preview_path(&entry).as_deref(), &request)).await
}

/// People in image `id` for the People mask picker (left to right).
#[tauri::command]
#[specta::specta]
pub async fn detect_people(
    catalog: State<'_, Catalog>,
    segmenter: State<'_, Segmenter>,
    id: ImageId,
) -> AppResult<Vec<DetectedPerson>> {
    let entry = catalog.run(move |c| repo::get_image(c, id)).await?;
    let segmenter = segmenter.inner().clone();
    blocking(move || segmenter.detect_people(&source_of(&entry), preview_path(&entry).as_deref())).await
}

/// Renders the mask of `target` (a group, or one component, of `adjustments.masks`: live
/// and unsaved) as a grayscale JPEG matching `render_preview`'s frame for the same
/// `maxEdge` / `region`. Latest-wins per image on the `mask` slot: `null` = superseded.
#[tauri::command]
#[specta::specta]
pub async fn render_mask_overlay(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    masks: State<'_, MaskCache>,
    id: ImageId,
    adjustments: ParametricAdjustments,
    target: MaskOverlayTarget,
    options: MaskOverlayOptions,
) -> AppResult<Option<RenderedMaskOverlay>> {
    adjustments.validate().map_err(AppError::invalid)?;
    options.validate().map_err(AppError::invalid)?;
    let known =
        adjustments.masks.iter().find(|g| g.id == target.group_id).is_some_and(|g| match &target.component_id {
            None => true,
            Some(c) => g.components.iter().any(|x| &x.id == c),
        });
    if !known {
        return Err(AppError::invalid("target does not name a mask group/component of adjustments.masks"));
    }
    let ticket = develop.ticket(id, RenderSlot::Mask);
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let masks = masks.inner().clone();
    blocking(move || {
        if !cache.is_current(ticket) {
            return Ok(None);
        }
        develop::masks::render_overlay(&cache, &masks, ticket, &src, &adjustments, &target, &options)
    })
    .await
}

/// Which AI mask families can be computed on this Mac (model files present).
#[tauri::command]
#[specta::specta]
pub async fn get_mask_capabilities(segmenter: State<'_, Segmenter>) -> AppResult<MaskCapabilities> {
    let segmenter = segmenter.inner().clone();
    blocking(move || Ok(segmenter.capabilities())).await
}

// ---------------------------------------------------------------------------
// Model downloads (IPC v12)
// ---------------------------------------------------------------------------

/// Downloadable model groups (AI masking), which files are installed, and the download in
/// flight. Cheap (file sizes only).
#[tauri::command]
#[specta::specta]
pub async fn model_downloads_status(downloads: State<'_, ModelDownloads>) -> AppResult<ModelDownloadStatus> {
    let downloads = downloads.inner().clone();
    blocking(move || Ok(downloads.status())).await
}

/// Starts downloading model group `group` (e.g. `"segmentation"`) in the background and
/// resolves immediately. Emits `ModelDownloadProgress` and, exactly once, `ModelDownloadFinished`.
/// Installed files are skipped, partial files resumed, every file SHA-256 verified.
/// `invalid_argument` for an unknown group or while a download is already running.
#[tauri::command]
#[specta::specta]
pub async fn download_models(app: AppHandle, downloads: State<'_, ModelDownloads>, group: String) -> AppResult<()> {
    let progress_app = app.clone();
    // Background-activity indicator (IPC v18), in KiB so large files fit a u32.
    let activity = super::activity::activities(&app);
    let finish_activity = activity.clone();
    downloads.start(
        &group,
        move |p| {
            if let Some(a) = &activity {
                let total = u32::try_from(p.bytes_total / 1024).unwrap_or(u32::MAX);
                let done = u32::try_from(p.bytes_done / 1024).unwrap_or(u32::MAX).min(total);
                let kind = super::events::ActivityKind::ModelDownload;
                a.progress("model_download", kind, "Downloading AI models", done, Some(total));
            }
            let _ = p.emit(&progress_app);
        },
        move |f| {
            if let Some(e) = &f.error {
                eprintln!("model download ({}): {e}", f.group);
            }
            if let Some(a) = &finish_activity {
                use super::events::ActivityState;
                let (state, message) = match (f.ok, f.cancelled) {
                    (true, _) => (ActivityState::Finished, Some("AI models installed".to_owned())),
                    (false, true) => (ActivityState::Cancelled, None),
                    (false, false) => (ActivityState::Error, f.error.clone()),
                };
                a.finish("model_download", state, message);
            }
            let _ = f.emit(&app);
        },
    )
}

/// Cancels the model download in flight (no-op when idle); `ModelDownloadFinished`
/// (`cancelled: true`) follows.
#[tauri::command]
#[specta::specta]
pub async fn cancel_model_download(downloads: State<'_, ModelDownloads>) -> AppResult<()> {
    downloads.cancel();
    Ok(())
}

// ---------------------------------------------------------------------------
// IPC v14 (Phase 8b): style library, auto tone / WB, guided workflow, edit plan, edit
// batches, style model, paste from previous
// ---------------------------------------------------------------------------

/// Imports every Lightroom develop preset (`.xmp` `crs:PresetType="Normal"`, legacy
/// `.lrtemplate`), creative profile (`.xmp` `crs:PresetType="Look"`), camera profile (`.dcp`)
/// and `.cube` LUT under `path` (recursive), one style group per source folder; re-importing a
/// folder replaces its group. Looks/DCPs are read in place later (never copied); LUTs are
/// copied into the LUT library. Errors: `not_found` (no such folder), `invalid_argument`
/// (nothing importable found). Body: rust-engine-dev (`styles::import_folder`).
#[tauri::command]
#[specta::specta]
pub async fn import_style_folder(
    catalog: State<'_, Catalog>,
    luts: State<'_, LutLibrary>,
    path: String,
) -> AppResult<ImportStyleReport> {
    let luts = luts.inner().clone();
    // Walk + parse (and copy LUTs) off the catalog thread; one transaction to store. Storing
    // also registers the imported looks / DCPs with the render engine (`profiles`).
    let scan = blocking(move || styles::scan_folder(&luts, Path::new(&path))).await?;
    catalog.run(move |c| styles::store_scan(c, scan)).await
}

/// The whole style library (every project sees every group): "User Presets", imported groups
/// by name, "LUTs" (the pre-v14 LUT library, registered on first listing).
#[tauri::command]
#[specta::specta]
pub async fn list_styles(catalog: State<'_, Catalog>, luts: State<'_, LutLibrary>) -> AppResult<StyleLibrary> {
    let luts = luts.inner().clone();
    catalog
        .run(move |c| {
            styles::sync_lut_library(c, &luts)?;
            styles::list(c)
        })
        .await
}

/// Removes an imported style group and its presets/profiles (source files untouched; images
/// using them keep their settings). Built-in groups -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn remove_style_group(catalog: State<'_, Catalog>, group_id: StyleGroupId) -> AppResult<()> {
    catalog.run(move |c| styles::remove_group(c, group_id)).await
}

/// What `apply_preset(presetId)` would make of image `id`'s settings (`adjustments` = the live
/// settings, `null` = stored ones), for hover previews (`renderPreview` with slot
/// `navigator`). Nothing is saved.
#[tauri::command]
#[specta::specta]
pub async fn resolve_preset(
    catalog: State<'_, Catalog>,
    id: ImageId,
    preset_id: PresetId,
    adjustments: Option<ParametricAdjustments>,
) -> AppResult<ParametricAdjustments> {
    if let Some(a) = &adjustments {
        a.validate().map_err(AppError::invalid)?;
    }
    catalog
        .run(move |c| {
            let base = match adjustments {
                Some(a) => a,
                None => repo::get_adjustments(c, id)?,
            };
            styles::resolve_preset(c, preset_id, &base)
        })
        .await
}

/// Lightroom's Basic "Auto": absolute values for the tone + presence sliders in `keys`
/// (`null` = all of `AdjustmentField::AUTO_TONE`; Shift-double-click a slider = just that one)
/// given the live `adjustments` (`null` = stored). Nothing is saved: the UI merges the values
/// (`applyAutoTone`) and saves one "Auto Tone" history entry. Body: rust-engine-dev.
// Unanalysed photos get their face boxes from on-demand detection
// (`develop::auto::resolve_faces`); a plain comment so `bindings.ts` is unchanged.
#[tauri::command]
#[specta::specta]
pub async fn auto_tone(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
    adjustments: Option<ParametricAdjustments>,
    keys: Option<Vec<AdjustmentField>>,
) -> AppResult<AutoToneValues> {
    let keys = keys.unwrap_or_else(|| AdjustmentField::AUTO_TONE.to_vec());
    if keys.is_empty() {
        return Err(AppError::invalid("keys must not be empty"));
    }
    if let Some(k) = keys.iter().find(|k| !AdjustmentField::AUTO_TONE.contains(k)) {
        return Err(AppError::invalid(format!("{} has no Auto", k.as_str())));
    }
    if let Some(a) = &adjustments {
        a.validate().map_err(AppError::invalid)?;
    }
    let (adjustments, faces) = catalog
        .run(move |c| {
            let a = match adjustments {
                Some(a) => a,
                None => repo::get_adjustments(c, id)?,
            };
            // Face boxes from the analysis (skin guard + face-weighted exposure); `None` when
            // the photo has not been analysed yet (detected on demand below).
            let faces = develop::auto::analysis_faces(c, id)?.map(|f| develop::auto::face_boxes(&f));
            Ok((a, faces))
        })
        .await?;
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let r = blocking(move || {
        let faces = develop::auto::resolve_faces(&cache, &src, faces);
        develop::auto::auto_tone_with_faces(&cache, &src, &adjustments, &keys, faces.as_deref())
    })
    .await;
    note_if_missing(&catalog, id, r).await
}

/// Lightroom's "Auto" white balance: temperature/tint for the live `adjustments` (`null` =
/// stored). Nothing is saved: the UI commits `whiteBalance: custom` ("Auto White Balance").
/// Body: rust-engine-dev.
#[tauri::command]
#[specta::specta]
pub async fn auto_white_balance(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
    adjustments: Option<ParametricAdjustments>,
) -> AppResult<WhiteBalanceValues> {
    if let Some(a) = &adjustments {
        a.validate().map_err(AppError::invalid)?;
    }
    let adjustments = match adjustments {
        Some(a) => a,
        None => catalog.run(move |c| repo::get_adjustments(c, id)).await?,
    };
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let r = blocking(move || develop::auto::auto_white_balance(&cache, &src, &adjustments)).await;
    note_if_missing(&catalog, id, r).await
}

/// Guided-workflow step of project `projectId` (also `Project.workflowStep`).
#[tauri::command]
#[specta::specta]
pub async fn get_workflow_step(catalog: State<'_, Catalog>, project_id: ProjectId) -> AppResult<WorkflowStep> {
    catalog.run(move |c| projects::workflow_step(c, project_id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_workflow_step(
    catalog: State<'_, Catalog>,
    project_id: ProjectId,
    step: WorkflowStep,
) -> AppResult<()> {
    catalog.run(move |c| projects::set_workflow_step(c, project_id, step)).await
}

/// Changes which images count as keepers (`CatalogState.keeperRule`). `minRating` 1..=5.
#[tauri::command]
#[specta::specta]
pub async fn set_keeper_rule(catalog: State<'_, Catalog>, rule: KeeperRule) -> AppResult<()> {
    catalog.run(move |c| repo::set_keeper_rule(c, &rule)).await
}

/// The Edit step of project `projectId`: keepers, their scenes, one representative per scene
/// and the checklist status. Proposes (and remembers) representatives for scenes without one.
/// Keepers outside every scene are listed in `unassignedKeeperIds` (run `detect_scenes`).
#[tauri::command]
#[specta::specta]
pub async fn get_edit_plan(catalog: State<'_, Catalog>, project_id: ProjectId) -> AppResult<EditPlan> {
    catalog.run(move |c| scene::workflow::edit_plan(c, project_id)).await
}

/// Chooses scene `sceneId`'s representative (`imageId` must be a keeper member) or hands the
/// choice back to Sieve (`null`).
#[tauri::command]
#[specta::specta]
pub async fn set_scene_representative(
    catalog: State<'_, Catalog>,
    scene_id: SceneId,
    image_id: Option<ImageId>,
) -> AppResult<SceneEditEntry> {
    catalog.run(move |c| scene::workflow::set_representative(c, scene_id, image_id)).await
}

/// Skips scene `sceneId` in the Edit step (`skipped = true`: counts as done, nothing is
/// copied to it, `apply_all_edited_scenes` leaves it alone) or includes it again (v15).
/// Settings are never touched; applying the scene includes it again. Unknown scene ->
/// `not_found`; a scene without keepers -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn set_scene_skipped(
    catalog: State<'_, Catalog>,
    scene_id: SceneId,
    skipped: bool,
) -> AppResult<SceneEditEntry> {
    catalog.run(move |c| scene::workflow::set_skipped(c, scene_id, skipped)).await
}

/// Per-photo workflow state of `imageIds` (given order): edit source, the batch / scene the
/// current settings were applied from, "needs a look" (v15). Unknown ids -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_edit_states(catalog: State<'_, Catalog>, image_ids: Vec<ImageId>) -> AppResult<Vec<ImageEditState>> {
    catalog.run(move |c| develop::batches::edit_states(c, &image_ids)).await
}

/// Persisted state of edit batches (v16), given order: undone, and whether
/// `undo_edit_batch` would succeed now (`undoable`; `conflictCount` photos edited since).
/// The UI shows a toast's / row's Undo only while its batch is `undoable`. Unknown ids ->
/// `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_edit_batches(
    catalog: State<'_, Catalog>,
    batch_ids: Vec<EditBatchId>,
) -> AppResult<Vec<EditBatchInfo>> {
    catalog.run(move |c| develop::batches::batch_infos(c, &batch_ids)).await
}

/// Clears "needs a look" on `imageIds` without changing their settings ("Looks good") (v15).
/// Images that do not need a look are ignored. Returns the ids that were cleared. Unknown
/// ids -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn mark_reviewed(catalog: State<'_, Catalog>, image_ids: Vec<ImageId>) -> AppResult<Vec<ImageId>> {
    catalog.run(move |c| develop::batches::mark_reviewed(c, &image_ids)).await
}

/// Stops the running `apply_scene_edit` / `apply_all_edited_scenes` (v15; no-op when none):
/// scenes whose matching finished are committed as the call's batch, the rest are left
/// alone; that call resolves with `cancelled: true`.
#[tauri::command]
#[specta::specta]
pub async fn cancel_scene_apply(control: State<'_, SceneApplyControl>) -> AppResult<()> {
    control.cancel();
    Ok(())
}

/// Targets matched per step of an apply; `cancel_scene_apply` takes effect between steps.
const APPLY_CANCEL_CHUNK: usize = 32;

/// Matches every job's targets to its representative (`match_scene` machinery) off the
/// catalog lock, with one `sceneProgress {task: "apply"}` stream over all targets
/// (`scene::workflow::match_jobs`: cancel between steps of [`APPLY_CANCEL_CHUNK`] targets;
/// with `skip_failures` (apply all, v17) a scene whose matching fails is reported, not fatal).
async fn match_scene_jobs(
    app: AppHandle,
    develop: &DevelopCache,
    luts: &LutLibrary,
    control: SceneApplyControl,
    jobs: Vec<scene::workflow::SceneApplyJob>,
    options: MatchOptions,
    skip_failures: bool,
) -> AppResult<scene::workflow::MatchedJobs> {
    let cache = develop.clone();
    let luts = luts.clone();
    blocking(move || {
        let emit = scene::progress_emitter(app, SceneTask::Apply);
        let total: u32 = jobs.iter().map(|j| j.targets.len() as u32).sum();
        let matched = scene::workflow::match_jobs(
            jobs,
            skip_failures,
            APPLY_CANCEL_CHUNK,
            &|| control.cancelled(),
            &mut |job, chunk, base| {
                let progress = |done: u32, _: u32| emit(base + done, total);
                scene::matching::match_images(
                    &cache,
                    &luts,
                    std::slice::from_ref(&job.representative),
                    chunk,
                    &options,
                    &progress,
                )
            },
        )?;
        if !matched.cancelled {
            emit(total, total);
        }
        Ok(matched)
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_scenes(
    app: AppHandle,
    catalog: &Catalog,
    develop: &DevelopCache,
    luts: &LutLibrary,
    xmp: &XmpSync,
    control: &SceneApplyControl,
    scene_ids: SceneIds,
    options: Option<SceneApplyOptions>,
) -> AppResult<ApplyScenesResult> {
    control.reset();
    let options = options.unwrap_or_default();
    options.match_options.validate().map_err(AppError::invalid)?;
    let opts = options.clone();
    let all = matches!(scene_ids, SceneIds::EditedIn(_));
    // Background-activity indicator (IPC v18); indeterminate (`sceneProgress` has the counts).
    let activity = super::activity::activities(&app).map(|a| {
        let label = if all { "Applying edits to scenes" } else { "Applying edit to scene" };
        a.start(super::events::ActivityKind::ApplyScene, label, None)
    });
    let result = apply_scenes_inner(app, catalog, develop, luts, xmp, control, scene_ids, options, opts, all).await;
    if let Some(a) = activity {
        match &result {
            Ok(r) if r.cancelled => a.cancel(Some(format!("Stopped after {} scene(s)", r.scenes.len()))),
            Ok(r) => {
                a.finish(Some(format!("Applied to {}", super::activity::photos(r.batch.changed_ids.len() as u32))))
            }
            Err(e) => a.fail(e.message.clone()),
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn apply_scenes_inner(
    app: AppHandle,
    catalog: &Catalog,
    develop: &DevelopCache,
    luts: &LutLibrary,
    xmp: &XmpSync,
    control: &SceneApplyControl,
    scene_ids: SceneIds,
    options: SceneApplyOptions,
    opts: SceneApplyOptions,
    all: bool,
) -> AppResult<ApplyScenesResult> {
    let (jobs, mut skipped) = catalog
        .run(move |c| match scene_ids {
            SceneIds::One(id) => Ok((scene::workflow::apply_inputs(c, &[id], &opts)?, Vec::new())),
            SceneIds::EditedIn(project) => scene::workflow::apply_all_inputs(c, project, &opts),
        })
        .await?;
    let m = match_scene_jobs(app.clone(), develop, luts, control.clone(), jobs, options.match_options, all).await?;
    skipped.extend(m.failed);
    let (jobs, previews, cancelled) = (m.jobs, m.previews, m.cancelled);
    let result = catalog.run(move |c| scene::workflow::commit_apply(c, &jobs, &previews, cancelled, skipped)).await?;
    if !result.batch.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(result)
}

enum SceneIds {
    One(SceneId),
    EditedIn(ProjectId),
}

/// "Apply to scene": copies the representative's edit to the scene's other keepers (and
/// non-keepers with `includeNonKeepers`) with relative matching (exposure / white balance
/// normalised per frame, `SceneApplyOptions.matchOptions`), skipping frames the user retouched
/// after the last apply (`skipUserEdited`). One undoable batch (`undo_edit_batch`); one "Apply
/// to Scene" history entry per changed image. Blocking until done (`sceneProgress` task
/// `apply`; `cancel_scene_apply` stops it). Representative without edits ->
/// `invalid_argument`. v15: `excludeIds` are left alone; non-converged targets are stored as
/// "needs a look" (`ImageEditState`); a skipped scene is included again; the scene's coverage
/// is recorded (`SceneEditEntry.unappliedKeeperIds`). v17: error messages name the scene by
/// its plan number ("Scene 1: edit its representative first, then apply."); an apply from a
/// representative whose settings came from an edit batch (Auto edit) blocks that batch's undo
/// until the apply is undone.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn apply_scene_edit(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    xmp: State<'_, XmpSync>,
    control: State<'_, SceneApplyControl>,
    scene_id: SceneId,
    options: Option<SceneApplyOptions>,
) -> AppResult<ApplyScenesResult> {
    apply_scenes(app, &catalog, &develop, &luts, &xmp, &control, SceneIds::One(scene_id), options).await
}

/// `apply_scene_edit` for every scene of `projectId` that is not skipped and whose status is
/// `edited` or `outdated`, or `applied` with `unappliedKeeperIds` (v15), as one undoable
/// batch (never `to_edit` / `reset` scenes, v17). No such scene -> empty result
/// (`batch.batchId = null`). `excludeIds` apply to every scene. v17: a scene that cannot be
/// applied (e.g. its original is missing) is left out and reported in `skippedScenes`; the
/// others are applied. Catalog-level errors still fail the call, named after the scene.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn apply_all_edited_scenes(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    xmp: State<'_, XmpSync>,
    control: State<'_, SceneApplyControl>,
    project_id: ProjectId,
    options: Option<SceneApplyOptions>,
) -> AppResult<ApplyScenesResult> {
    apply_scenes(app, &catalog, &develop, &luts, &xmp, &control, SceneIds::EditedIn(project_id), options).await
}

/// Undoes an edit batch (`apply_scene_edit`, `apply_all_edited_scenes`,
/// `apply_style_prediction`): images still carrying what the batch wrote get their previous
/// settings back ("Undo <label>" entry each); images edited since are left alone
/// (`skippedIds`). Unknown batch -> `not_found`; already undone -> `invalid_argument`.
/// Undo is linear (v16): when any photo of the batch has a later history entry (a later
/// batch or a manual edit), nothing changes and the call fails with `conflict` ("Later edits
/// on n photos; undo those first"). Scenes whose last apply was this batch go back to
/// `edited` (`SceneEditEntry.appliedBatch` = null). v17: a scene apply (not undone) made from
/// a representative whose settings this batch wrote is a later edit too ("A scene was applied
/// from this edit since; undo that apply first").
#[tauri::command]
#[specta::specta]
pub async fn undo_edit_batch(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    batch_id: EditBatchId,
) -> AppResult<UndoBatchResult> {
    let r = catalog.run(move |c| develop::batches::undo(c, batch_id)).await?;
    if !r.restored_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

/// Lightroom's "Previous" / Paste from previous (Cmd+Alt+V): copies the stored settings of
/// `previousId` (the previously selected photo, tracked by the UI) onto `targetIds` (`fields`
/// `null` = `AdjustmentField::PASTE_PREVIOUS`, everything but masks). `previousId` in
/// `targetIds` is skipped. One "Paste from Previous" entry per changed image. Atomic.
/// v19: one undoable edit batch (kind `paste`), like `paste_settings`.
#[tauri::command]
#[specta::specta]
pub async fn paste_previous(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    target_ids: Vec<ImageId>,
    previous_id: ImageId,
    fields: Option<Vec<AdjustmentField>>,
) -> AppResult<EditBatchResult> {
    let fields = fields.unwrap_or_else(|| AdjustmentField::PASTE_PREVIOUS.to_vec());
    require_fields(&fields)?;
    let targets: Vec<ImageId> = target_ids.into_iter().filter(|&id| id != previous_id).collect();
    let r = catalog
        .run(move |c| {
            let src = repo::get_adjustments(c, previous_id)?;
            develop::batches::apply_fields_recorded(c, &targets, &src, &fields, develop::history::LABEL_PASTE_PREVIOUS)
        })
        .await?;
    if !r.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

/// State of the personal style model ("Auto edit (my style)").
#[tauri::command]
#[specta::specta]
pub async fn style_model_status(
    catalog: State<'_, Catalog>,
    style: State<'_, StyleModel>,
) -> AppResult<StyleModelStatus> {
    let style = style.inner().clone();
    catalog.run(move |c| style.status(c)).await
}

/// Trains the style model from every edited photo in the catalog, in the background
/// (`styleModelProgress`, then exactly one `styleModelFinished`). Already training ->
/// `invalid_argument`. Body: vision-ml-dev.
#[tauri::command]
#[specta::specta]
pub async fn train_style_model(app: AppHandle, style: State<'_, StyleModel>) -> AppResult<()> {
    style.start_training(&app)
}

/// Stops a running training (no-op when idle); `styleModelFinished {cancelled: true}` follows.
#[tauri::command]
#[specta::specta]
pub async fn cancel_style_training(style: State<'_, StyleModel>) -> AppResult<()> {
    style.cancel();
    Ok(())
}

/// Predicted settings in the user's style for `imageIds` (given order; nothing is saved).
/// No trained model -> `invalid_argument`. Body: vision-ml-dev.
#[tauri::command]
#[specta::specta]
pub async fn predict_style(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    style: State<'_, StyleModel>,
    image_ids: Vec<ImageId>,
) -> AppResult<Vec<StylePrediction>> {
    let inputs = catalog.run(move |c| scene::store::match_inputs(c, &image_ids)).await?;
    let (style, cache, luts) = (style.inner().clone(), develop.inner().clone(), luts.inner().clone());
    blocking(move || style.predict(&cache, &luts, &inputs)).await
}

/// Predicts and commits the user's style for `imageIds` as one undoable batch ("Auto Edit (My
/// Style)" entry per changed image; `undo_edit_batch`).
#[tauri::command]
#[specta::specta]
pub async fn apply_style_prediction(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    style: State<'_, StyleModel>,
    xmp: State<'_, XmpSync>,
    image_ids: Vec<ImageId>,
) -> AppResult<EditBatchResult> {
    let inputs = catalog.run(move |c| scene::store::match_inputs(c, &image_ids)).await?;
    let (model, cache, lut_lib) = (style.inner().clone(), develop.inner().clone(), luts.inner().clone());
    let predictions = blocking(move || model.predict(&cache, &lut_lib, &inputs)).await?;
    let items: Vec<develop::batches::BatchItem> = predictions
        .into_iter()
        .map(|p| develop::batches::BatchItem {
            image_id: p.image_id,
            adjustments: p.adjustments,
            scene_id: None,
            review_reason: None,
        })
        .collect();
    let r = catalog
        .run(move |c| {
            develop::batches::commit_recorded(
                c,
                &items,
                develop::batches::LABEL_STYLE,
                develop::batches::BatchKind::StylePrediction,
            )
        })
        .await?;
    if !r.changed_ids.is_empty() {
        xmp.notify(&app);
    }
    Ok(r)
}

// ---------------------------------------------------------------------------
// IPC v14 (Phase 8b): projects (home page). A project is one shoot: name, source folder(s),
// cover, shoot type, workflow step. Inside a project the UI passes `projectId` to
// `ImageQuery`, `get_filter_counts`, `list_burst_groups`, `list_scenes`, `detect_scenes`,
// `get_edit_plan`, `apply_all_edited_scenes` and `AnalysisScope::Project`. "Reveal in Finder"
// = `reveal_in_finder(project.folders[i].path)`; "Locate folder..." = `relocate_folder`.
// ---------------------------------------------------------------------------

/// Every project, most recently opened first, then newest (the home page sorts/searches
/// client-side).
#[tauri::command]
#[specta::specta]
pub async fn list_projects(catalog: State<'_, Catalog>) -> AppResult<Vec<Project>> {
    catalog.run(|c| projects::list_projects(c)).await
}

/// One project. Unknown id -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn get_project(catalog: State<'_, Catalog>, project_id: ProjectId) -> AppResult<Project> {
    catalog.run(move |c| projects::get_project(c, project_id)).await
}

/// Home page "New project": imports `path` (like `import_folder`) as a new project named
/// `name` (`null` = the folder's name) with `shootType` (`null` = `CatalogState.shootType`).
/// If the folder (or a folder containing it) is already in the catalog, its project is
/// re-scanned and returned with `existing: true` instead. Errors as `import_folder`; bad name
/// -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn create_project(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    analysis: State<'_, Analysis>,
    xmp: State<'_, XmpSync>,
    path: String,
    name: Option<String>,
    shoot_type: Option<ShootType>,
    options: ImportOptions,
) -> AppResult<CreateProjectResult> {
    if let Some(n) = &name {
        Project::validate_name(n).map_err(AppError::invalid)?;
    }
    let target = ImportTarget::New { name, shoot_type };
    let (import, existing) = run_import(&app, &catalog, &ingest, &analysis, &xmp, path, options, target).await?;
    let id = import.project_id;
    let project = catalog.run(move |c| projects::get_project(c, id)).await?;
    Ok(CreateProjectResult { project, import, existing })
}

/// Entering a project: stamps `lastOpenedAtMs` and returns it. The app always starts on the
/// home page (the last project is not reopened automatically).
#[tauri::command]
#[specta::specta]
pub async fn open_project(catalog: State<'_, Catalog>, project_id: ProjectId) -> AppResult<Project> {
    catalog.run(move |c| projects::open_project(c, project_id)).await
}

/// Renames a project (trimmed, 1..=200 characters; the folder on disk is not renamed).
#[tauri::command]
#[specta::specta]
pub async fn rename_project(catalog: State<'_, Catalog>, project_id: ProjectId, name: String) -> AppResult<Project> {
    catalog.run(move |c| projects::rename_project(c, project_id, &name)).await
}

/// Sets the cover photo (`imageId` must be in the project) or returns to the automatic
/// cover (`null`).
#[tauri::command]
#[specta::specta]
pub async fn set_project_cover(
    catalog: State<'_, Catalog>,
    project_id: ProjectId,
    image_id: Option<ImageId>,
) -> AppResult<Project> {
    catalog.run(move |c| projects::set_project_cover(c, project_id, image_id)).await
}

/// Sets the project's shoot type and rescores (tags/scores/suggestions of its photos follow
/// the new type's thresholds).
#[tauri::command]
#[specta::specta]
pub async fn set_project_shoot_type(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    project_id: ProjectId,
    shoot_type: ShootType,
) -> AppResult<()> {
    catalog.run(move |c| projects::set_project_shoot_type(c, project_id, shoot_type)).await?;
    // vision-ml-dev: the rescore must score each image with its project's shoot type
    // (`db::projects::shoot_type_of_image`), not the catalog default.
    analysis.start(&app, AnalysisScope::Rescore)
}

/// Removes a project from the catalog: its folders, photos and everything Sieve stored about
/// them (ratings, tags, edits, scenes, history) and their cached thumbnails/previews. Never
/// deletes or modifies originals, sidecars or exports; re-importing the folder brings the
/// photos back with what the sidecars hold. Unknown id -> `not_found`.
#[tauri::command]
#[specta::specta]
pub async fn remove_project(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    project_id: ProjectId,
) -> AppResult<RemoveProjectResult> {
    // A running analysis would fail writing results for rows that vanish under it: pause it
    // around the removal and resume the remaining work afterwards. (Ingest drops extractions
    // of removed images itself; the XMP auto-sync reloads every row before writing.)
    let resume = match app.try_state::<Analysis>() {
        Some(a) if a.is_running() => {
            a.cancel();
            let handle = app.clone();
            blocking(move || {
                let t = std::time::Instant::now();
                while handle.state::<Analysis>().is_running() && t.elapsed() < std::time::Duration::from_secs(30) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Ok(())
            })
            .await?;
            true
        }
        _ => false,
    };
    let removal = catalog.run(move |c| projects::remove_project(c, project_id)).await;
    if resume {
        if let Some(a) = app.try_state::<Analysis>() {
            let _ = a.start(&app, AnalysisScope::Pending);
        }
    }
    let (result, removed) = removal?;
    // Image ids can be reused by the next import: drop everything held for them (decoded
    // sources, renders, mask weights, mattes) and their cached files.
    if let Some(develop) = app.try_state::<DevelopCache>() {
        develop.forget_images(&removed);
    }
    let config = ingest.config().clone();
    blocking(move || {
        for id in removed {
            for p in [config.thumb_path(id), config.preview_path(id)] {
                let _ = std::fs::remove_file(p);
            }
        }
        Ok(())
    })
    .await?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// IPC v19 (Phase 8d): capture time, per-photo metadata, Upright, preview variants, reject
// strictness. Paste / Sync / Paste from Previous return `EditBatchResult` (see above).
// ---------------------------------------------------------------------------

/// Lightroom's "Edit Capture Time" for `ids` (any selection): shift by an offset, set the
/// active photo to an exact time (the others follow by the same offset), sync two cameras
/// from a reference pair (v19.2: the selected photos, or every photo of the target's body /
/// model in its project), or revert to the files' own time (see [`CaptureTimeEdit`]). The
/// original EXIF time is kept (`CaptureMeta.originalCapturedAtMs`); the corrected time is
/// what sorting, bursts, scenes, filters and export naming use, and is written to the
/// sidecars (`exif:DateTimeOriginal` / `photoshop:DateCreated`; marks them dirty, notifies
/// auto-sync). Kicks a rescore so bursts regroup. Atomic; undo with
/// `restore_capture_times(result.previous)`.
#[tauri::command]
#[specta::specta]
pub async fn edit_capture_time(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    analysis: State<'_, Analysis>,
    ids: Vec<ImageId>,
    mode: CaptureTimeEdit,
) -> AppResult<CaptureTimeEditResult> {
    let r = catalog.run(move |c| db::capture_time::edit(c, &ids, &mode)).await?;
    if !r.changed_ids.is_empty() {
        xmp.notify(&app);
        analysis.start(&app, AnalysisScope::Rescore)?;
    }
    Ok(r)
}

/// Puts corrected capture times back (undo / redo of `edit_capture_time`: pass its
/// `previous`, or snapshots taken before). Atomic (unknown id -> `not_found`). Returns the ids
/// whose time changed (refetch them). Marks sidecars dirty and kicks a rescore like
/// `edit_capture_time`.
#[tauri::command]
#[specta::specta]
pub async fn restore_capture_times(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    analysis: State<'_, Analysis>,
    snapshots: Vec<CaptureTimeSnapshot>,
) -> AppResult<Vec<ImageId>> {
    let changed = catalog.run(move |c| db::capture_time::restore(c, &snapshots)).await?;
    if !changed.is_empty() {
        xmp.notify(&app);
        analysis.start(&app, AnalysisScope::Rescore)?;
    }
    Ok(changed)
}

/// Everything the Library Metadata panel shows for photo `id`: file facts, original and
/// corrected capture time, camera, lens, exposure, size, GPS, sidecar. Unknown id ->
/// `not_found`. Catalog values plus a few read from the file (`gps`, `focalLength35mm`,
/// `exposureCompensationEv`, `flashFired`; `cameraSerial` from the catalog since v19.2, else
/// the file).
#[tauri::command]
#[specta::specta]
pub async fn get_image_metadata(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<ImageMetadata> {
    let mut m = catalog.run(move |c| db::capture_time::image_metadata(c, id)).await?;
    if m.missing {
        return Ok(m);
    }
    // File EXIF outside the catalog lock (a few hundred KB of the original at most).
    let path = std::path::PathBuf::from(&m.path);
    let f = blocking(move || Ok(crate::raw::exif_info::read(&path))).await?;
    m.focal_length_35mm = f.focal_length_35mm;
    m.exposure_compensation_ev = f.exposure_compensation_ev;
    m.flash_fired = f.flash_fired;
    // v19.2: the catalog's serial (read at import) wins; the file's for photos not read yet.
    m.camera_serial = m.camera_serial.or(f.camera_serial);
    m.gps = f.gps;
    Ok(m)
}

/// Solves Upright `mode` for photo `id` from the live `adjustments` (`null` = stored; Guided
/// uses `adjustments.transform.guides`, the crop is ignored). Nothing is saved: the UI sets
/// `transform.upright` + `transform.solution` and commits one history entry. `off` returns no
/// solution. Body: stub (no solution, `message` says Upright is not available yet) until
/// vision-ml-dev (line detection) / rust-engine-dev (solver) implement it.
// Implemented (vision-ml-dev, `ml::upright`): LSD-style line detection on a ~1024 px render,
// vanishing points, Level / Vertical / Full / Auto / Guided solve. "Auto straighten" for the
// crop tool = `level`, crop angle = `ml::upright::crop_angle_for_rotation(rotationDeg, o)`.
// A plain comment so `bindings.ts` is unchanged.
#[tauri::command]
#[specta::specta]
pub async fn auto_upright(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    id: ImageId,
    mode: UprightMode,
    adjustments: Option<ParametricAdjustments>,
) -> AppResult<UprightResult> {
    if let Some(a) = &adjustments {
        a.validate().map_err(AppError::invalid)?;
    }
    // Line detection + solve: `ml::upright` (vision-ml-dev). Detection runs on a ~1024 px
    // render of the live colour settings without crop / transform / masks / LUT.
    let (adjustments, entry) = catalog
        .run(move |c| {
            let e = repo::get_image(c, id)?;
            let a = match adjustments {
                Some(a) => a,
                None => repo::get_adjustments(c, id)?,
            };
            Ok((a, e))
        })
        .await?;
    if mode == UprightMode::Off {
        return Ok(UprightResult { mode, solution: None, message: None });
    }
    // A photo Lightroom already solved stores every mode's matrix: switching modes reuses
    // Lightroom's own solve (matches what Lightroom shows) instead of detecting lines again.
    if mode != UprightMode::Guided {
        let lightroom = adjustments.transform.solution.as_ref().filter(|s| !s.crs.is_empty());
        if let Some(solution) = lightroom.and_then(|s| develop::transform::lightroom_solution(&s.crs, mode)) {
            return Ok(UprightResult { mode, solution: Some(solution), message: None });
        }
    }
    let f35 = ml::upright::focal_35mm_estimate(
        entry.camera.make,
        entry.camera.model.as_deref(),
        entry.capture.focal_length_mm,
    );
    let orientation = entry.orientation.filter(|o| (1..=8).contains(o)).unwrap_or(1);
    let guides = adjustments.transform.guides.clone();
    let outcome = if mode == UprightMode::Guided {
        // Guides only: no render needed.
        let (w, h) = match (entry.width, entry.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => develop::source::oriented_size(w, h, orientation),
            _ => (3, 2),
        };
        ml::upright::solve_rgb8(&[], w, h, orientation, mode, &guides, f35)
    } else {
        let src = develop_source(&catalog, &develop, id).await?;
        let cache = develop.inner().clone();
        let luts = luts.inner().clone();
        let r = blocking(move || {
            let a = ml::upright::detection_adjustments(&adjustments);
            let px = cache.render_image(&src, &a, None, ml::upright::DETECT_EDGE, &luts)?;
            let img = px.image;
            Ok(ml::upright::solve_rgb8(&img.rgb, img.width, img.height, orientation, mode, &guides, f35))
        })
        .await;
        note_if_missing(&catalog, id, r).await?
    };
    Ok(UprightResult { mode, solution: outcome.solution, message: outcome.message })
}

/// Crop-tool bounds of the live `adjustments` for photo `id` (v19.3, docs/ux-review-8d.md
/// R1-3): the warped image's outline in the uncropped corrected frame as displayed, and what
/// Constrain Crop makes of `adjustments.crop`. Pure geometry, no render (decodes the source
/// on first use, as `get_develop_info`; instant while the photo is open in Develop). Both
/// `null` without a Transform / Upright warp. The crop tool should render with
/// `crop.enabled = false` and `transform.constrainCrop = false`: that frame is the one the
/// quad refers to (full warped image, white outside the quad).
#[tauri::command]
#[specta::specta]
pub async fn get_transform_bounds(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
    adjustments: ParametricAdjustments,
) -> AppResult<TransformBounds> {
    adjustments.validate().map_err(AppError::invalid)?;
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let bounds = blocking(move || cache.transform_bounds(&src, &adjustments)).await;
    note_if_missing(&catalog, id, bounds).await
}

/// Renders a temporary variation of the live `adjustments` (no save, no history entry): a
/// preset applied on top (hover preview on the main image) or some groups reset to the
/// format defaults (press-and-hold "without this panel"). Same render path as
/// `render_preview` (latest-wins per (id, slot), `sieve://` URL); use slot `preview` so the
/// edit's `main` render stays valid. Unknown preset / image -> `not_found`; empty
/// `withoutFields.fields` -> `invalid_argument`.
#[tauri::command]
#[specta::specta]
pub async fn render_preview_variant(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    luts: State<'_, LutLibrary>,
    id: ImageId,
    adjustments: ParametricAdjustments,
    variant: PreviewVariant,
    options: RenderOptions,
) -> AppResult<Option<RenderedPreview>> {
    adjustments.validate().map_err(AppError::invalid)?;
    options.validate().map_err(AppError::invalid)?;
    if let PreviewVariant::WithoutFields { fields } = &variant {
        require_fields(fields)?;
    }
    // Ticket first: arrival order decides latest-wins, as in `render_preview`.
    let ticket = develop.ticket(id, options.slot);
    let resolved = catalog.run(move |c| styles::resolve_preview_variant(c, id, &adjustments, &variant)).await?;
    resolved.validate().map_err(AppError::invalid)?;
    let src = develop_source(&catalog, &develop, id).await?;
    let cache = develop.inner().clone();
    let luts = luts.inner().clone();
    let rendered = blocking(move || {
        if !cache.is_current(ticket) {
            return Ok(None);
        }
        cache.render(ticket, &src, &resolved, &options, &luts)
    })
    .await;
    note_if_missing(&catalog, id, rendered).await
}

/// Sets how readily culling suggests reject for the project's photos (conservative /
/// balanced / aggressive) and rescores. Unknown project -> `not_found`. The scorer reads it
/// per image with `db::projects::reject_strictness_of_image` (vision-ml-dev).
#[tauri::command]
#[specta::specta]
pub async fn set_project_reject_strictness(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    analysis: State<'_, Analysis>,
    project_id: ProjectId,
    strictness: RejectStrictness,
) -> AppResult<()> {
    catalog.run(move |c| projects::set_project_reject_strictness(c, project_id, strictness)).await?;
    analysis.start(&app, AnalysisScope::Rescore)
}
