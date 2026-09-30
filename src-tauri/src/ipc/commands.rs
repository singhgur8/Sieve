//! Tauri command handlers. Thin: validate/convert arguments, run the query on a
//! blocking thread, return typed results. Signatures are the contract; bodies may
//! be reimplemented by the owning specialist without changing them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::{AppHandle, State};

use super::error::{AppError, AppResult};
use super::types::*;
use crate::db::{self, repo};
use crate::develop::{self, DevelopCache, SourceImage};
use crate::ingest::{self, Ingest};
use crate::lut::{self, LutLibrary};
use crate::ml::{self, Analysis};
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
        tauri::async_runtime::spawn_blocking(move || {
            let mut conn = conn.lock().map_err(|_| AppError::internal("catalog lock poisoned"))?;
            f(&mut conn)
        })
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
    }
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
#[tauri::command]
#[specta::specta]
pub async fn import_folder(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    ingest: State<'_, Ingest>,
    analysis: State<'_, Analysis>,
    xmp: State<'_, XmpSync>,
    path: String,
    options: ImportOptions,
) -> AppResult<ImportSummary> {
    let (mut summary, auto) =
        catalog.run(move |c| Ok((repo::import_folder(c, Path::new(&path), &options)?, repo::auto_analyze(c)?))).await?;
    let sync = xmp.inner().clone();
    let folder_id = summary.folder_id;
    summary.sidecars_read = blocking(move || sync.refresh_folder(folder_id)).await?;
    ingest.start(&app)?;
    if auto {
        analysis.start(&app, AnalysisScope::Pending)?;
    }
    Ok(summary)
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

/// Filter-bar facet counts for `folderId` (`null` = whole catalog).
#[tauri::command]
#[specta::specta]
pub async fn get_filter_counts(catalog: State<'_, Catalog>, folder_id: Option<FolderId>) -> AppResult<FilterCounts> {
    catalog.run(move |c| repo::filter_counts(c, folder_id)).await
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
    let src = source_images(&catalog, vec![id]).await?.remove(0);
    let cache = develop.inner().clone();
    let luts = luts.inner().clone();
    blocking(move || {
        if !cache.is_current(ticket) {
            return Ok(None);
        }
        cache.render(ticket, &src, &adjustments, &options, &luts)
    })
    .await
}

/// As-shot white balance and develop-source sizes (decodes the RAW if not cached).
#[tauri::command]
#[specta::specta]
pub async fn get_develop_info(
    catalog: State<'_, Catalog>,
    develop: State<'_, DevelopCache>,
    id: ImageId,
) -> AppResult<DevelopInfo> {
    let src = source_images(&catalog, vec![id]).await?.remove(0);
    let cache = develop.inner().clone();
    blocking(move || cache.info(&src)).await
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
/// every image in `ids`; one "Paste Settings" history entry per changed image. Atomic.
#[tauri::command]
#[specta::specta]
pub async fn paste_settings(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    adjustments: ParametricAdjustments,
    fields: Vec<AdjustmentField>,
) -> AppResult<()> {
    require_fields(&fields)?;
    adjustments.validate().map_err(AppError::invalid)?;
    catalog
        .run(move |c| develop::history::apply_fields(c, &ids, &adjustments, &fields, develop::history::LABEL_PASTE))
        .await?;
    xmp.notify(&app);
    Ok(())
}

/// Copies the `fields` groups of `sourceId`'s stored adjustments onto `targetIds`
/// ("Sync Settings"). Atomic.
#[tauri::command]
#[specta::specta]
pub async fn sync_settings(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    source_id: ImageId,
    target_ids: Vec<ImageId>,
    fields: Vec<AdjustmentField>,
) -> AppResult<()> {
    require_fields(&fields)?;
    catalog
        .run(move |c| {
            let src = repo::get_adjustments(c, source_id)?;
            develop::history::apply_fields(c, &target_ids, &src, &fields, develop::history::LABEL_SYNC)
        })
        .await?;
    xmp.notify(&app);
    Ok(())
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
            let neutral = ParametricAdjustments::default();
            develop::history::apply_fields(c, &ids, &neutral, AdjustmentField::ALL, develop::history::LABEL_RESET)
        })
        .await?;
    xmp.notify(&app);
    Ok(())
}

/// Applies preset `presetId` (its `fields` only) to `ids` ("Preset: <name>"). Atomic.
#[tauri::command]
#[specta::specta]
pub async fn apply_preset(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    preset_id: PresetId,
) -> AppResult<()> {
    catalog
        .run(move |c| {
            let preset = develop::presets::get(c, preset_id)?;
            let label = format!("{}{}", develop::history::LABEL_PRESET_PREFIX, preset.name);
            develop::history::apply_fields(c, &ids, &preset.adjustments, &preset.fields, &label)
        })
        .await?;
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

/// Burst groups with members, optionally limited to groups touching `folderId`.
#[tauri::command]
#[specta::specta]
pub async fn list_burst_groups(catalog: State<'_, Catalog>, folder_id: Option<FolderId>) -> AppResult<Vec<BurstGroup>> {
    catalog.run(move |c| repo::list_burst_groups(c, folder_id)).await
}

/// Copies the engine's suggested rating/pick into the user's rating/pick for `ids`
/// (unanalyzed images skipped). Returns the number of images updated.
#[tauri::command]
#[specta::specta]
pub async fn apply_suggestions(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
) -> AppResult<u32> {
    let updated = catalog.run(move |c| repo::apply_suggestions(c, &ids)).await?;
    xmp.notify(&app);
    Ok(updated)
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
pub async fn write_xmp(xmp: State<'_, XmpSync>, ids: Vec<ImageId>) -> AppResult<XmpSyncReport> {
    let sync = xmp.inner().clone();
    blocking(move || sync.write_images(&ids)).await
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

#[tauri::command]
#[specta::specta]
pub async fn get_xmp_status(catalog: State<'_, Catalog>, xmp: State<'_, XmpSync>) -> AppResult<XmpStatus> {
    let running = xmp.is_running();
    catalog.run(move |c| repo::xmp_status(c, running)).await
}
