use serde::Serialize;
use specta::Type;

/// Error category. The `message` is always user-facing; the kind lets the UI pick a
/// remedy (IPC v13 added the file/volume/catalog kinds; earlier they were `not_found` /
/// `io` / `database` with the same messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// A catalog row (image, preset, job, ...) does not exist.
    NotFound,
    InvalidArgument,
    Io,
    Database,
    Internal,
    /// An original is not at its catalogued path (moved, renamed, drive disconnected).
    /// The image is flagged `RawImageEntry.missingSinceMs`; `relocate_folder` fixes it.
    FileMissing,
    /// The destination volume is out of space (export, sidecar, catalog write).
    DiskFull,
    /// The destination volume is read-only or the folder is not writable.
    ReadOnly,
    /// The original exists but could not be decoded (damaged, still copying, unsupported).
    DecodeFailed,
    /// The catalog is damaged and was opened read-only (`CatalogState.health`).
    CatalogReadOnly,
    /// The operation would undo or overwrite something a later edit was built on (v16:
    /// `undo_edit_batch` of a batch whose photos were edited since). Nothing was changed;
    /// the message says what to undo first.
    Conflict,
}

/// Error returned by every command. Serialized as `{ kind, message }`.
#[derive(Debug, Clone, PartialEq, Serialize, Type, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidArgument, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::not_found("no matching catalog row"),
            e => Self::new(ErrorKind::Database, e.to_string()),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::internal(e.to_string())
    }
}
