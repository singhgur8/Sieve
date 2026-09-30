pub mod db;
pub mod ingest;
pub mod ipc;
pub mod ml;
pub mod raw;
pub mod xmp;

use std::path::PathBuf;

use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder, Event};

use ingest::{Ingest, IngestConfig};
use ipc::commands::{self, Catalog};
use ipc::events::{
    AnalysisFailed, AnalysisFinished, AnalysisProgress, AnalysisReady, ImportProgress, ThumbnailFailed, ThumbnailReady,
    XmpSynced, XmpWriteFailed,
};
use ipc::types::AnalysisScope;
use ml::{Analysis, AnalysisConfig};
use xmp::{XmpSync, XmpSyncConfig};

/// Overrides the catalog location (useful for tests and scratch catalogs).
const CATALOG_ENV: &str = "LUMENRAW_CATALOG";
/// Overrides the derived-file cache root (thumbnails live in `<cache>/thumbs/`).
const CACHE_ENV: &str = "LUMENRAW_CACHE";
/// Overrides the ONNX model directory (default: `src-tauri/models` in debug builds,
/// `<resource_dir>/models` in release).
const MODELS_ENV: &str = "LUMENRAW_MODELS";

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
            XmpWriteFailed
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
            let config = IngestConfig { catalog_path: path.clone(), cache_dir };
            std::fs::create_dir_all(config.thumbs_dir())?;
            // tauri.conf.json scopes the asset protocol to `$APPCACHE/thumbs/**`; this also
            // covers a `LUMENRAW_CACHE` override (and is a no-op widening otherwise).
            app.asset_protocol_scope().allow_directory(config.thumbs_dir(), true)?;

            let catalog = Catalog::open(path.clone())?;
            let auto_analyze = catalog.auto_analyze_blocking()?;
            app.manage(catalog);
            app.manage(Ingest::new(config));
            app.manage(Analysis::new(AnalysisConfig { catalog_path: path.clone(), models_dir }));
            app.manage(XmpSync::new(XmpSyncConfig { catalog_path: path }));
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
