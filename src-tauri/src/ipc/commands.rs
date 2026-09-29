//! Tauri command handlers. Thin: validate/convert arguments, run the query on a
//! blocking thread, return typed results. Signatures are the contract; bodies may
//! be reimplemented by the owning specialist without changing them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::State;

use super::error::{AppError, AppResult};
use super::types::*;
use crate::db::{self, repo};

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
pub async fn get_catalog_state(catalog: State<'_, Catalog>) -> AppResult<CatalogState> {
    let path = catalog.path.display().to_string();
    catalog.run(move |c| repo::catalog_state(c, &path)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_shoot_type(catalog: State<'_, Catalog>, shoot_type: ShootType) -> AppResult<()> {
    catalog.run(move |c| repo::set_shoot_type(c, shoot_type)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_burst_window(catalog: State<'_, Catalog>, ms: u32) -> AppResult<()> {
    catalog.run(move |c| repo::set_burst_window(c, ms)).await
}

/// Registers RAW files under `path`. Does not decode; thumbnails start `pending`.
#[tauri::command]
#[specta::specta]
pub async fn import_folder(
    catalog: State<'_, Catalog>,
    path: String,
    options: ImportOptions,
) -> AppResult<ImportSummary> {
    catalog.run(move |c| repo::import_folder(c, Path::new(&path), &options)).await
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

#[tauri::command]
#[specta::specta]
pub async fn set_rating(catalog: State<'_, Catalog>, ids: Vec<ImageId>, rating: u8) -> AppResult<()> {
    catalog.run(move |c| repo::set_rating(c, &ids, rating)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_pick(catalog: State<'_, Catalog>, ids: Vec<ImageId>, pick: PickFlag) -> AppResult<()> {
    catalog.run(move |c| repo::set_pick(c, &ids, pick)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_color_label(
    catalog: State<'_, Catalog>,
    ids: Vec<ImageId>,
    label: Option<ColorLabel>,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_color_label(c, &ids, label)).await
}

/// Adds (`present = true`) or removes a tag as the user. Removing an auto tag suppresses it.
#[tauri::command]
#[specta::specta]
pub async fn set_user_tag(
    catalog: State<'_, Catalog>,
    ids: Vec<ImageId>,
    tag: CullTag,
    present: bool,
) -> AppResult<()> {
    catalog.run(move |c| repo::set_user_tag(c, &ids, tag, present)).await
}

/// Stored adjustments, or neutral defaults for an unedited image.
#[tauri::command]
#[specta::specta]
pub async fn get_adjustments(catalog: State<'_, Catalog>, id: ImageId) -> AppResult<ParametricAdjustments> {
    catalog.run(move |c| repo::get_adjustments(c, id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_adjustments(
    catalog: State<'_, Catalog>,
    id: ImageId,
    adjustments: ParametricAdjustments,
) -> AppResult<()> {
    catalog.run(move |c| repo::save_adjustments(c, id, &adjustments)).await
}
