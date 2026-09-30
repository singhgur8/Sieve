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
use crate::export::{self, Exporter};
use crate::ingest::{self, Ingest};
use crate::lut::{self, LutLibrary};
use crate::ml::{self, Analysis};
use crate::scene;
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

/// Burst groups with members, optionally limited to groups touching `folderId`.
#[tauri::command]
#[specta::specta]
pub async fn list_burst_groups(catalog: State<'_, Catalog>, folder_id: Option<FolderId>) -> AppResult<Vec<BurstGroup>> {
    catalog.run(move |c| repo::list_burst_groups(c, folder_id)).await
}

/// Copies the engine's suggested rating/pick into the user's rating/pick for `ids`.
/// Unanalyzed images are skipped; with `onlyUnset`, so are images already flagged or rated
/// (`pick != unflagged` or `rating != 0`). Atomic; unknown ids -> `not_found`.
/// For undo, take `get_cull_snapshot(ids)` first.
#[tauri::command]
#[specta::specta]
pub async fn apply_suggestions(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    xmp: State<'_, XmpSync>,
    ids: Vec<ImageId>,
    only_unset: bool,
) -> AppResult<ApplySuggestionsResult> {
    let result = catalog.run(move |c| repo::apply_suggestions(c, &ids, only_unset)).await?;
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

/// "Save all": writes sidecars for every XMP-dirty image of `folderId` (all folders for
/// `null`) now, whether or not auto-sync is on (catalog wins, like `write_xmp`).
#[tauri::command]
#[specta::specta]
pub async fn write_xmp_all_dirty(xmp: State<'_, XmpSync>, folder_id: Option<FolderId>) -> AppResult<XmpSyncReport> {
    let sync = xmp.inner().clone();
    blocking(move || sync.write_dirty(folder_id)).await
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

// ---------------------------------------------------------------------------
// Scenes & scene matching (Phase 7)
// ---------------------------------------------------------------------------

/// Groups the images of `folderId` (all folders for `null`) into scenes by capture-time gaps
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
    options: Option<SceneDetectOptions>,
) -> AppResult<Vec<Scene>> {
    let options = options.unwrap_or_default();
    options.validate().map_err(AppError::invalid)?;
    let replace_manual = options.replace_manual;
    let mut frames = catalog.run(move |c| scene::store::detection_frames(c, folder_id, replace_manual)).await?;
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
            scene::store::replace_scenes(c, folder_id, &groups, options.replace_manual)
        })
        .await
}

/// Scenes with a member in `folderId` (all for `null`), in capture order.
#[tauri::command]
#[specta::specta]
pub async fn list_scenes(catalog: State<'_, Catalog>, folder_id: Option<FolderId>) -> AppResult<Vec<Scene>> {
    catalog.run(move |c| scene::store::list_scenes(c, folder_id)).await
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
