//! Matte storage: 8-bit grey PNG files (`<cacheDir>/masks/<imageId>/<digest>.png`, bounds in
//! a `tEXt` chunk so a file is self-describing) + `mask_cache` rows + an in-memory LRU.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::AlphaMask;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{AiMask, AiMaskInfo, AiMaskOrigin, ImageId, NormRect};

/// Decoded mattes kept in memory.
pub const LRU_BYTES: usize = 192 << 20;

const BOUNDS_KEY: &str = "sieve:bounds";

#[derive(Default)]
pub struct Lru {
    entries: VecDeque<((ImageId, String), Arc<AlphaMask>)>,
    bytes: usize,
}

impl Lru {
    pub fn get(&mut self, key: &(ImageId, String)) -> Option<Arc<AlphaMask>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let e = self.entries.remove(i)?;
        let m = e.1.clone();
        self.entries.push_back(e);
        Some(m)
    }

    pub fn put(&mut self, key: (ImageId, String), m: Arc<AlphaMask>) {
        self.remove(&key);
        self.bytes += m.data.len();
        self.entries.push_back((key, m));
        while self.bytes > LRU_BYTES && self.entries.len() > 1 {
            if let Some((_, old)) = self.entries.pop_front() {
                self.bytes -= old.data.len();
            }
        }
    }

    pub fn remove(&mut self, key: &(ImageId, String)) {
        if let Some(i) = self.entries.iter().position(|(k, _)| k == key) {
            if let Some((_, old)) = self.entries.remove(i) {
                self.bytes -= old.data.len();
            }
        }
    }
}

pub type SharedLru = Arc<Mutex<Lru>>;

pub fn lock(l: &SharedLru) -> std::sync::MutexGuard<'_, Lru> {
    l.lock().unwrap_or_else(|e| e.into_inner())
}

/// `masks/<imageId>/<digest>.png` (relative to the cache dir; `mask_cache.path`).
pub fn rel_path(image_id: ImageId, digest: &str) -> String {
    format!("masks/{image_id}/{digest}.png")
}

pub fn encode_png(mask: &AlphaMask) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, mask.width, mask.height);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let b = &mask.bounds;
        enc.add_text_chunk(BOUNDS_KEY.into(), format!("{} {} {} {}", b.x, b.y, b.width, b.height))
            .map_err(|e| format!("PNG: {e}"))?;
        let mut w = enc.write_header().map_err(|e| format!("PNG: {e}"))?;
        w.write_image_data(&mask.data).map_err(|e| format!("PNG: {e}"))?;
        w.finish().map_err(|e| format!("PNG: {e}"))?;
    }
    Ok(out)
}

pub fn decode_png(bytes: &[u8]) -> Result<AlphaMask, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| format!("PNG: {e}"))?;
    let bounds = reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .find(|t| t.keyword == BOUNDS_KEY)
        .map(|t| t.text.split_whitespace().filter_map(|v| v.parse::<f32>().ok()).collect::<Vec<_>>())
        .filter(|v| v.len() == 4)
        .map_or(NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }, |v| NormRect {
            x: v[0],
            y: v[1],
            width: v[2],
            height: v[3],
        });
    let size = reader.output_buffer_size().ok_or("PNG: output too large")?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    if frame.color_type != png::ColorType::Grayscale || frame.bit_depth != png::BitDepth::Eight {
        return Err("matte PNG is not 8-bit grey".into());
    }
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut data = Vec::with_capacity(w * h);
    for row in buf.chunks(frame.line_size).take(h) {
        data.extend_from_slice(&row[..w]);
    }
    Ok(AlphaMask { width: frame.width, height: frame.height, bounds, data })
}

/// Writes `bytes` to `path` atomically (temp file + rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A `mask_cache` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub digest: String,
    pub kind: String,
    pub origin: AiMaskOrigin,
    pub model_version: String,
    pub width: u32,
    pub height: u32,
    pub bounds: NormRect,
    pub coverage: f32,
}

impl Row {
    pub fn info(&self, ai: &AiMask) -> AiMaskInfo {
        AiMaskInfo {
            digest: self.digest.clone(),
            target: ai.target.clone(),
            reference_point: ai.reference_point,
            origin: self.origin,
            model_version: self.model_version.clone(),
            width: self.width,
            height: self.height,
            bounds: self.bounds,
            coverage: self.coverage,
        }
    }
}

const COLUMNS: &str =
    "digest, kind, origin, model_version, width, height, bounds_x, bounds_y, bounds_w, bounds_h, coverage";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let origin: String = r.get(2)?;
    Ok(Row {
        digest: r.get(0)?,
        kind: r.get(1)?,
        origin: if origin == "lightroom" { AiMaskOrigin::Lightroom } else { AiMaskOrigin::Sieve },
        model_version: r.get(3)?,
        width: r.get(4)?,
        height: r.get(5)?,
        bounds: NormRect {
            x: r.get::<_, f64>(6)? as f32,
            y: r.get::<_, f64>(7)? as f32,
            width: r.get::<_, f64>(8)? as f32,
            height: r.get::<_, f64>(9)? as f32,
        },
        coverage: r.get::<_, f64>(10)? as f32,
    })
}

pub fn by_digest(conn: &Connection, image_id: ImageId, digest: &str) -> AppResult<Option<Row>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM mask_cache WHERE image_id = ?1 AND digest = ?2"),
            params![image_id, digest],
            row,
        )
        .optional()?)
}

/// Newest `sieve` row of (image, kind[, model version]).
pub fn newest_sieve(conn: &Connection, image_id: ImageId, kind: &str, model: Option<&str>) -> AppResult<Option<Row>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM mask_cache WHERE image_id = ?1 AND kind = ?2 AND origin = 'sieve' \
                 AND (?3 IS NULL OR model_version = ?3) ORDER BY created_at DESC, rowid DESC LIMIT 1"
            ),
            params![image_id, kind, model],
            row,
        )
        .optional()?)
}

#[allow(clippy::too_many_arguments)]
pub fn insert(
    conn: &Connection,
    image_id: ImageId,
    r: &Row,
    input_digest: Option<&str>,
    path: &str,
    now_ms: i64,
) -> AppResult<()> {
    let origin = match r.origin {
        AiMaskOrigin::Lightroom => "lightroom",
        AiMaskOrigin::Sieve => "sieve",
    };
    conn.execute(
        "INSERT OR REPLACE INTO mask_cache (image_id, digest, kind, origin, model_version, input_digest, path, width, \
         height, bounds_x, bounds_y, bounds_w, bounds_h, coverage, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            image_id,
            r.digest,
            r.kind,
            origin,
            r.model_version,
            input_digest,
            path,
            r.width,
            r.height,
            f64::from(r.bounds.x),
            f64::from(r.bounds.y),
            f64::from(r.bounds.width),
            f64::from(r.bounds.height),
            f64::from(r.coverage),
            now_ms,
        ],
    )?;
    Ok(())
}

/// Read-only connection to the catalog (renders resolve digest-less AI components).
pub fn open_read_only(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_millis(2000))?;
    Ok(conn)
}

/// Loads a matte file.
pub fn load_file(path: &PathBuf) -> AppResult<Option<AlphaMask>> {
    match std::fs::read(path) {
        Ok(bytes) => decode_png(&bytes).map(Some).map_err(AppError::internal),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Deletes superseded, unreferenced Sieve rows and files without a row; returns the number of
/// files removed.
pub fn sweep(conn: &Connection, masks_dir: &Path, lru: &SharedLru) -> AppResult<usize> {
    // 1. Sieve mattes superseded by a newer model version of the same (image, kind) that no
    //    adjustment / history entry references.
    let superseded: Vec<(ImageId, String)> = {
        let mut st = conn.prepare(
            "SELECT m.image_id, m.digest FROM mask_cache m WHERE m.origin = 'sieve' AND EXISTS ( \
               SELECT 1 FROM mask_cache n WHERE n.image_id = m.image_id AND n.kind = m.kind AND n.origin = 'sieve' \
                 AND n.model_version <> m.model_version AND n.created_at > m.created_at) \
             AND NOT EXISTS (SELECT 1 FROM adjustments a WHERE a.image_id = m.image_id AND instr(a.params_json, m.digest) > 0) \
             AND NOT EXISTS (SELECT 1 FROM adjustment_history h WHERE h.image_id = m.image_id AND instr(h.params_json, m.digest) > 0)",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (id, d) in &superseded {
        conn.execute("DELETE FROM mask_cache WHERE image_id = ?1 AND digest = ?2", params![id, d])?;
        lock(lru).remove(&(*id, d.clone()));
    }
    // 2. Files without a row (images removed, rows superseded).
    let mut removed = 0;
    let Ok(dirs) = std::fs::read_dir(masks_dir) else { return Ok(0) };
    for dir in dirs.flatten() {
        let Some(image_id) = dir.file_name().to_str().and_then(|s| s.parse::<ImageId>().ok()) else { continue };
        let Ok(files) = std::fs::read_dir(dir.path()) else { continue };
        let mut kept = 0;
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let digest = name.strip_suffix(".png").unwrap_or(&name).to_owned();
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM mask_cache WHERE image_id = ?1 AND digest = ?2)",
                    params![image_id, digest],
                    |r| r.get(0),
                )
                .unwrap_or(true);
            if exists && name.ends_with(".png") {
                kept += 1;
            } else if std::fs::remove_file(f.path()).is_ok() {
                removed += 1;
                lock(lru).remove(&(image_id, digest));
            }
        }
        if kept == 0 {
            let _ = std::fs::remove_dir(dir.path());
        }
    }
    Ok(removed)
}
