pub mod db;
pub mod develop;
pub mod export;
pub mod ingest;
pub mod ipc;
pub mod lut;
pub mod ml;
pub mod raw;
pub mod xmp;

use std::path::PathBuf;

use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder, Event};

use develop::{DevelopCache, DevelopConfig};
use export::{ExportConfig, Exporter};
use ingest::{Ingest, IngestConfig};
use ipc::commands::{self, Catalog};
use ipc::events::{
    AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady, ExportFinished, ExportProgress, ImportProgress,
    ThumbnailFailed, ThumbnailReady, XmpSynced, XmpWriteFailed,
};
use ipc::types::AnalysisScope;
use lut::LutLibrary;
use ml::{Analysis, AnalysisConfig};
use xmp::{XmpSync, XmpSyncConfig};

/// Overrides the catalog location (useful for tests and scratch catalogs).
const CATALOG_ENV: &str = "SIEVE_CATALOG";
/// Overrides the derived-file cache root (thumbnails live in `<cache>/thumbs/`).
const CACHE_ENV: &str = "SIEVE_CACHE";
/// Overrides the ONNX model directory (default: `src-tauri/models` in debug builds,
/// `<resource_dir>/models` in release).
const MODELS_ENV: &str = "SIEVE_MODELS";
/// Overrides the LUT library directory (default `<app_data_dir>/luts`).
const LUTS_ENV: &str = "SIEVE_LUTS";
/// Overrides the develop cache budget in MiB (default `DevelopConfig::DEFAULT_CACHE_MB`).
const DEVELOP_CACHE_ENV: &str = "SIEVE_DEVELOP_CACHE_MB";
/// Overrides the export memory budget in MiB (default: 25% of RAM, clamped to 2..=8 GiB).
const EXPORT_MEMORY_ENV: &str = "SIEVE_EXPORT_MEMORY_MB";

/// Generated TypeScript bindings, relative to this crate.
pub const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/ipc/bindings.ts");

/// Every command and event exposed to the frontend. Single registration point.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::get_catalog_state,
            commands::set_shoot_type,
            commands::set_burst_window,
            commands::import_folder,
            commands::list_images,
            commands::get_image,
            commands::set_rating,
            commands::set_pick,
            commands::set_color_label,
            commands::set_user_tag,
            commands::get_adjustments,
            commands::save_adjustments,
            commands::regenerate_thumbnails,
            commands::get_import_status,
            commands::analyze_images,
            commands::cancel_analysis,
            commands::get_analysis_status,
            commands::set_auto_analyze,
            commands::get_cull_thresholds,
            commands::set_cull_thresholds,
            commands::get_faces,
            commands::list_burst_groups,
            commands::apply_suggestions,
            commands::get_images,
            commands::list_image_ids,
            commands::get_filter_counts,
            commands::write_xmp,
            commands::read_xmp,
            commands::set_xmp_auto_sync,
            commands::get_xmp_status,
            commands::render_preview,
            commands::get_develop_info,
            commands::prepare_develop,
            commands::get_history,
            commands::undo_adjustments,
            commands::redo_adjustments,
            commands::goto_history,
            commands::paste_settings,
            commands::sync_settings,
            commands::reset_adjustments,
            commands::apply_preset,
            commands::list_presets,
            commands::save_preset,
            commands::delete_preset,
            commands::list_luts,
            commands::import_lut,
            commands::delete_lut,
            commands::get_export_capabilities,
            commands::list_export_presets,
            commands::save_export_preset,
            commands::delete_export_preset,
            commands::plan_export,
            commands::export_images,
            commands::cancel_export,
            commands::get_export_jobs,
        ])
        .events(collect_events![
            ImportProgress,
            ThumbnailReady,
            ThumbnailFailed,
            AnalysisProgress,
            AnalysisReady,
            AnalysisFailed,
            AnalysisFinished,
            XmpSynced,
            XmpWriteFailed,
            ExportProgress,
            ExportFinished
        ])
        // IDs and unix-ms timestamps are i64 but always < 2^53.
        .dangerously_cast_bigints_to_number()
}

pub fn export_bindings(builder: &Builder<tauri::Wry>, path: &str) {
    builder.export(specta_typescript::Typescript::default(), path).expect("failed to export TypeScript bindings");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    export_bindings(&builder, BINDINGS_PATH);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(builder.invoke_handler())
        // Rendered previews (`sieve://localhost/render/<id>/<slot>?v=<seq>`), served from
        // memory off the main thread.
        .register_asynchronous_uri_scheme_protocol(develop::RENDER_SCHEME, |ctx, request, responder| {
            let Some(cache) = ctx.app_handle().try_state::<DevelopCache>().map(|s| s.inner().clone()) else {
                let mut unavailable = tauri::http::Response::new(Vec::new());
                *unavailable.status_mut() = tauri::http::StatusCode::SERVICE_UNAVAILABLE;
                responder.respond(unavailable);
                return;
            };
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(develop::handle_protocol(&cache, &request));
            });
        })
        .setup(move |app| {
            builder.mount_events(app);
            let path = match std::env::var_os(CATALOG_ENV) {
                Some(p) => PathBuf::from(p),
                None => app.path().app_data_dir()?.join("catalog.sqlite"),
            };
            let cache_dir = match std::env::var_os(CACHE_ENV) {
                Some(p) => PathBuf::from(p),
                None => app.path().app_cache_dir()?,
            };
            let models_dir = match std::env::var_os(MODELS_ENV) {
                Some(p) => PathBuf::from(p),
                None if cfg!(debug_assertions) => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models")),
                None => app.path().resource_dir()?.join("models"),
            };
            let luts_dir = match std::env::var_os(LUTS_ENV) {
                Some(p) => PathBuf::from(p),
                None => app.path().app_data_dir()?.join("luts"),
            };
            std::fs::create_dir_all(&luts_dir)?;
            let develop_cache_mb = std::env::var(DEVELOP_CACHE_ENV)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DevelopConfig::DEFAULT_CACHE_MB);
            let export_memory_mb = std::env::var(EXPORT_MEMORY_ENV).ok().and_then(|v| v.parse::<u64>().ok());
            let config = IngestConfig { catalog_path: path.clone(), cache_dir };
            std::fs::create_dir_all(config.thumbs_dir())?;
            // tauri.conf.json scopes the asset protocol to `$APPCACHE/thumbs/**`; this also
            // covers a `SIEVE_CACHE` override (and is a no-op widening otherwise).
            app.asset_protocol_scope().allow_directory(config.thumbs_dir(), true)?;

            let catalog = Catalog::open(path.clone())?;
            let auto_analyze = catalog.auto_analyze_blocking()?;
            app.manage(catalog);
            app.manage(Ingest::new(config));
            app.manage(Analysis::new(AnalysisConfig { catalog_path: path.clone(), models_dir }));
            app.manage(XmpSync::new(XmpSyncConfig { catalog_path: path.clone() }));
            app.manage(DevelopCache::new(DevelopConfig { cache_bytes: develop_cache_mb * 1024 * 1024 }));
            let luts = LutLibrary::new(luts_dir);
            let exporter =
                Exporter::new(ExportConfig { catalog_path: path, memory_budget_mb: export_memory_mb }, luts.clone());
            // Jobs cut off by a previous quit become `interrupted` (never resumed).
            exporter.recover_interrupted()?;
            app.manage(luts);
            app.manage(exporter);
            // Auto tags change during analysis; flush them (if auto-sync is on) once it settles.
            let handle = app.handle().clone();
            AnalysisFinished::listen(app.handle(), move |_| handle.state::<XmpSync>().notify(&handle));
            // Resume thumbnails left `pending` (and analysis left undone) by a previous session.
            app.state::<Ingest>().start(app.handle())?;
            if auto_analyze {
                app.state::<Analysis>().start(app.handle(), AnalysisScope::Pending)?;
            }
            // Flush sidecar changes left dirty by a previous session (no-op unless auto-sync).
            app.state::<XmpSync>().notify(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails when `src/ipc/bindings.ts` is stale. Fix: `cargo run` (debug) or
    /// `UPDATE_BINDINGS=1 cargo test bindings`.
    #[test]
    fn bindings_are_up_to_date() {
        let builder = specta_builder();
        if std::env::var_os("UPDATE_BINDINGS").is_some() {
            export_bindings(&builder, BINDINGS_PATH);
        }
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("bindings.ts");
        export_bindings(&builder, fresh.to_str().unwrap());

        let committed = std::fs::read_to_string(BINDINGS_PATH).unwrap_or_default();
        assert!(
            committed == std::fs::read_to_string(&fresh).unwrap(),
            "src/ipc/bindings.ts is out of date; run `UPDATE_BINDINGS=1 cargo test bindings`"
        );
    }
}
