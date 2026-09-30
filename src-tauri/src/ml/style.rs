//! Personal style model ("Auto edit (my style)", IPC v14). Contract by the architect; bodies:
//! vision-ml-dev.
//!
//! - Training data: every catalog image with edits (`adjustments.neutral = 0`), incl. edits read
//!   from Lightroom sidecars (`Read from XMP`). Features per image (develop-source statistics,
//!   scene context, camera, capture settings; cached in `style_features` under a version) ->
//!   target: the user's `ParametricAdjustments` groups listed in [`PREDICTED_FIELDS`]. Model on
//!   device, stored in `style_models` (newest row = active); hold out a time-split share for
//!   `StyleValidation` (render ΔE2000 vs the user's edits, also for `auto_tone` and no edit).
//! - `start_training` runs on a background thread with its own catalog connection (never the
//!   command mutex), emits `StyleModelProgress` (throttled) and exactly one
//!   `StyleModelFinished`; one run at a time (`invalid_argument` if already training).
//! - `predict` returns, per image, its current adjustments with the predicted groups replaced
//!   (crop, masks and other per-frame groups kept); `invalid_argument` when no model is
//!   trained ("Train the style model first").
//! - `apply_style_prediction` (command) = `predict` + `develop::batches::commit_recorded`
//!   (label `LABEL_STYLE`, kind `StylePrediction`), so it is undoable as one batch.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::AppHandle;

use crate::develop::DevelopCache;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::*;
use crate::lut::LutLibrary;

/// Current feature + model family (`StyleModelStatus.modelVersion`).
pub const MODEL_VERSION: &str = "style-v0";
/// Training needs at least this many edited photos.
pub const MIN_EXAMPLES: u32 = 20;
/// Groups the model predicts (everything but per-frame geometry and local masks).
pub const PREDICTED_FIELDS: &[AdjustmentField] = AdjustmentField::DEFAULT_SYNC;

#[derive(Debug, Clone)]
pub struct StyleModelConfig {
    pub catalog_path: PathBuf,
}

#[derive(Default)]
struct Run {
    training: bool,
    cancelled: bool,
    progress: Option<f32>,
    error: Option<String>,
}

/// Managed state.
#[derive(Clone)]
pub struct StyleModel {
    config: StyleModelConfig,
    run: Arc<Mutex<Run>>,
}

impl StyleModel {
    pub fn new(config: StyleModelConfig) -> Self {
        Self { config, run: Arc::new(Mutex::new(Run::default())) }
    }

    pub fn config(&self) -> &StyleModelConfig {
        &self.config
    }

    /// Edited photos in the catalog (training candidates).
    pub fn available_examples(conn: &Connection) -> AppResult<u32> {
        Ok(conn.query_row("SELECT COUNT(*) FROM adjustments WHERE neutral = 0", [], |r| r.get(0))?)
    }

    /// `style_model_status()`.
    pub fn status(&self, conn: &Connection) -> AppResult<StyleModelStatus> {
        let run = self.run.lock().unwrap_or_else(|e| e.into_inner());
        let latest: Option<(i64, u32, Option<String>)> = rusqlite::OptionalExtension::optional(conn.query_row(
            "SELECT trained_at, examples, validation_json FROM style_models ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ))?;
        let state = if run.training {
            StyleModelState::Training
        } else if run.error.is_some() {
            StyleModelState::Failed
        } else if latest.is_some() {
            StyleModelState::Ready
        } else {
            StyleModelState::Untrained
        };
        Ok(StyleModelStatus {
            state,
            model_version: MODEL_VERSION.into(),
            trained_at_ms: latest.as_ref().map(|l| l.0),
            training_examples: latest.as_ref().map_or(0, |l| l.1),
            available_examples: Self::available_examples(conn)?,
            min_examples: MIN_EXAMPLES,
            progress: if run.training { run.progress } else { None },
            error: run.error.clone(),
            validation: latest.and_then(|l| l.2).and_then(|j| serde_json::from_str(&j).ok()),
        })
    }

    /// Starts training in the background (see the module docs). vision-ml-dev.
    pub fn start_training(&self, app: &AppHandle) -> AppResult<()> {
        let _ = app;
        let run = self.run.lock().unwrap_or_else(|e| e.into_inner());
        if run.training {
            return Err(AppError::invalid("The style model is already training."));
        }
        Err(AppError::internal("Style model training is not implemented yet (IPC v14 stub, vision-ml-dev)."))
    }

    /// Stops a running training (no-op when idle).
    pub fn cancel(&self) {
        let mut run = self.run.lock().unwrap_or_else(|e| e.into_inner());
        if run.training {
            run.cancelled = true;
        }
    }

    /// Predictions for `inputs` (image + current adjustments). vision-ml-dev.
    pub fn predict(
        &self,
        cache: &DevelopCache,
        luts: &LutLibrary,
        inputs: &[crate::scene::MatchImage],
    ) -> AppResult<Vec<StylePrediction>> {
        let _ = (cache, luts, inputs);
        Err(AppError::invalid("Train the style model first (IPC v14 stub, vision-ml-dev)."))
    }
}
