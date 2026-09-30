pub mod db;
pub mod ingest;
pub mod ipc;
pub mod ml;
pub mod raw;

use std::path::PathBuf;

use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

use ingest::{Ingest, IngestConfig};
use ipc::commands::{self, Catalog};
use ipc::events::{AnalysisProgress, ImportProgress, ThumbnailFailed, ThumbnailReady};

/// Overrides the catalog location (useful for tests and scratch catalogs).
const CATALOG_ENV: &str = "LUMENRAW_CATALOG";
/// Overrides the derived-file cache root (thumbnails live in `<cache>/thumbs/`).
const CACHE_ENV: &str = "LUMENRAW_CACHE";

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
        ])
        .events(collect_events![ImportProgress, ThumbnailReady, ThumbnailFailed, AnalysisProgress])
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
            let config = IngestConfig { catalog_path: path.clone(), cache_dir };
            std::fs::create_dir_all(config.thumbs_dir())?;
            // tauri.conf.json scopes the asset protocol to `$APPCACHE/thumbs/**`; this also
            // covers a `LUMENRAW_CACHE` override (and is a no-op widening otherwise).
            app.asset_protocol_scope().allow_directory(config.thumbs_dir(), true)?;

            app.manage(Catalog::open(path)?);
            app.manage(Ingest::new(config));
            // Resume thumbnails left `pending` by a previous session.
            app.state::<Ingest>().start(app.handle())?;
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
