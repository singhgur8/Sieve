//! `.cube` LUT library and evaluation (Phase 5). Owned by rust-engine-dev.
//!
//! Fixed surface: [`LutLibrary`] (`new`, `dir`, `list`, `import`, `delete`, `load`),
//! [`references`], [`Lut`] (`parse`, `apply`). Phase 9 (reference-match grading) writes
//! generated `.cube` files into the same directory, so the directory is the source of truth
//! (no catalog table): LUTs are shared by every catalog, like Lightroom profiles.
//!
//! - Library dir: `<app_data_dir>/luts/` (`$SIEVE_LUTS` overrides), created at startup.
//!   Files are `<id>.cube`; `id` = slug of the source name + `-` + 8 hex digits of a content
//!   hash (`[a-z0-9-]{1,64}`, see `ipc::types::is_valid_lut_id`), so re-importing the same
//!   file is idempotent (returns the existing entry).
//! - `.cube` (Adobe/Resolve spec): `TITLE`, `LUT_1D_SIZE` (2..=65536) or `LUT_3D_SIZE`
//!   (2..=65), `DOMAIN_MIN`/`DOMAIN_MAX` (and Resolve's `LUT_1D_INPUT_RANGE` /
//!   `LUT_3D_INPUT_RANGE`), `#` comments, red-fastest ordering. Invalid files ->
//!   `invalid_argument` on import (the file is not copied).
//! - Evaluation: 3D tetrahedral interpolation (trilinear available), 1D linear per channel;
//!   input is display-referred sRGB-encoded 0..=1, clamped to the domain; `amount` 0..=100
//!   blends linearly with the input.
//! - `list` scans the dir (headers only), skips unparsable files, sorts by name.
//! - Parsed LUTs are cached in memory by id (`load`); `delete` evicts.
//!
//! Import keeps the source name: when the file has no `TITLE`, the library copy gets a
//! `TITLE "<source file stem>"` line prepended (the id hash is of the original bytes).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{is_valid_lut_id, LutId, LutInfo, LutKind};

pub const MAX_3D_SIZE: u32 = 65;
pub const MAX_1D_SIZE: u32 = 65536;
/// Parsed LUTs kept in memory (a 65^3 LUT is ~3.3 MB).
const CACHE_ENTRIES: usize = 16;

/// A parsed LUT ready for evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut {
    pub kind: LutKind,
    pub size: u32,
    pub title: Option<String>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// RGB triples, red fastest (`size^3` for 3D, `size` for 1D).
    pub table: Vec<[f32; 3]>,
}

/// 3D interpolation method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Tetrahedral,
    Trilinear,
}

/// Header facts (everything before the table).
#[derive(Debug, Clone, PartialEq)]
struct Header {
    kind: LutKind,
    size: u32,
    title: Option<String>,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
}

#[derive(Default)]
struct HeaderBuilder {
    title: Option<String>,
    size_1d: Option<u32>,
    size_3d: Option<u32>,
    domain_min: Option<[f32; 3]>,
    domain_max: Option<[f32; 3]>,
}

enum Line<'a> {
    Blank,
    Keyword,
    Data(&'a str),
}

impl HeaderBuilder {
    /// Classifies (and records) one line. `n` is the 1-based line number.
    fn line<'a>(&mut self, raw: &'a str, n: usize) -> Result<Line<'a>, String> {
        let line = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            return Ok(Line::Blank);
        }
        let first = line.split_whitespace().next().unwrap_or("");
        let starts_numeric = first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.');
        if starts_numeric {
            return Ok(Line::Data(line.split('#').next().unwrap_or("").trim()));
        }
        let rest = line[first.len()..].trim();
        let floats = |count: usize| -> Result<Vec<f32>, String> {
            let v: Vec<f32> = rest
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .map(|t| t.parse::<f32>().map_err(|_| format!("line {n}: bad number {t:?}")))
                .collect::<Result<_, _>>()?;
            if v.len() != count || v.iter().any(|x| !x.is_finite()) {
                return Err(format!("line {n}: {first} needs {count} numbers"));
            }
            Ok(v)
        };
        let size = || -> Result<u32, String> {
            let t = rest.split('#').next().unwrap_or("").trim();
            t.parse::<u32>().map_err(|_| format!("line {n}: bad {first} {t:?}"))
        };
        match first {
            "TITLE" => {
                let t = rest.trim();
                let t = t.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(t);
                self.title = Some(t.to_owned()).filter(|t| !t.is_empty());
            }
            "LUT_1D_SIZE" => self.size_1d = Some(size()?),
            "LUT_3D_SIZE" => self.size_3d = Some(size()?),
            "DOMAIN_MIN" => {
                let v = floats(3)?;
                self.domain_min = Some([v[0], v[1], v[2]]);
            }
            "DOMAIN_MAX" => {
                let v = floats(3)?;
                self.domain_max = Some([v[0], v[1], v[2]]);
            }
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                let v = floats(2)?;
                self.domain_min = Some([v[0]; 3]);
                self.domain_max = Some([v[1]; 3]);
            }
            // Other keywords (e.g. Resolve's LUT_IN_VIDEO_RANGE) do not change evaluation.
            k if k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') => {}
            _ => return Err(format!("line {n}: unexpected {first:?}")),
        }
        Ok(Line::Keyword)
    }

    fn finish(self) -> Result<Header, String> {
        let (kind, size) = match (self.size_1d, self.size_3d) {
            (Some(_), Some(_)) => return Err("combined 1D + 3D LUTs are not supported".into()),
            (None, None) => return Err("missing LUT_1D_SIZE / LUT_3D_SIZE".into()),
            (Some(s), None) => {
                if !(2..=MAX_1D_SIZE).contains(&s) {
                    return Err(format!("LUT_1D_SIZE {s} is outside 2..={MAX_1D_SIZE}"));
                }
                (LutKind::Lut1d, s)
            }
            (None, Some(s)) => {
                if !(2..=MAX_3D_SIZE).contains(&s) {
                    return Err(format!("LUT_3D_SIZE {s} is outside 2..={MAX_3D_SIZE}"));
                }
                (LutKind::Lut3d, s)
            }
        };
        let domain_min = self.domain_min.unwrap_or([0.0; 3]);
        let domain_max = self.domain_max.unwrap_or([1.0; 3]);
        if (0..3).any(|c| domain_max[c] <= domain_min[c]) {
            return Err("DOMAIN_MAX must exceed DOMAIN_MIN".into());
        }
        Ok(Header { kind, size, title: self.title, domain_min, domain_max })
    }
}

impl Lut {
    /// Parses `.cube` text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut hb = HeaderBuilder::default();
        let mut table: Vec<[f32; 3]> = Vec::new();
        for (i, raw) in text.split('\n').enumerate() {
            match hb.line(raw, i + 1)? {
                Line::Blank => {}
                Line::Keyword => {
                    if !table.is_empty() {
                        return Err(format!("line {}: keyword after table data", i + 1));
                    }
                }
                Line::Data(d) => {
                    let mut it = d.split_whitespace().map(str::parse::<f32>);
                    let (Some(Ok(r)), Some(Ok(g)), Some(Ok(b)), None) = (it.next(), it.next(), it.next(), it.next())
                    else {
                        return Err(format!("line {}: expected 3 numbers", i + 1));
                    };
                    if !(r.is_finite() && g.is_finite() && b.is_finite()) {
                        return Err(format!("line {}: non-finite value", i + 1));
                    }
                    table.push([r, g, b]);
                }
            }
        }
        let h = hb.finish()?;
        let expected = match h.kind {
            LutKind::Lut1d => h.size as usize,
            LutKind::Lut3d => (h.size as usize).pow(3),
        };
        if table.len() != expected {
            return Err(format!("expected {expected} table entries, found {}", table.len()));
        }
        Ok(Lut {
            kind: h.kind,
            size: h.size,
            title: h.title,
            domain_min: h.domain_min,
            domain_max: h.domain_max,
            table,
        })
    }

    /// Maps one sRGB-encoded pixel (0..=1) and blends by `amount` (0..=200; above 100
    /// extrapolates away from the input, clamped to 0..=1).
    pub fn apply(&self, rgb: [f32; 3], amount: f32) -> [f32; 3] {
        self.apply_with(rgb, amount, Interpolation::Tetrahedral)
    }

    /// [`Self::apply`] with an explicit 3D interpolation (1D LUTs ignore it).
    pub fn apply_with(&self, rgb: [f32; 3], amount: f32, interp: Interpolation) -> [f32; 3] {
        let a = (amount / 100.0).clamp(0.0, 2.0);
        if a <= 0.0 {
            return rgb;
        }
        let mapped = self.eval(rgb, interp);
        if a == 1.0 {
            return mapped;
        }
        if a > 1.0 {
            return [0, 1, 2].map(|k| (rgb[k] + (mapped[k] - rgb[k]) * a).clamp(0.0, 1.0));
        }
        [rgb[0] + (mapped[0] - rgb[0]) * a, rgb[1] + (mapped[1] - rgb[1]) * a, rgb[2] + (mapped[2] - rgb[2]) * a]
    }

    /// Full-strength LUT output.
    #[inline]
    pub fn eval(&self, rgb: [f32; 3], interp: Interpolation) -> [f32; 3] {
        let n = self.size as usize;
        let scale = (n - 1) as f32;
        let mut t = [0.0f32; 3];
        for c in 0..3 {
            let v = (rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c]);
            // NaN -> 0.
            t[c] = if v > 0.0 { v.min(1.0) * scale } else { 0.0 };
        }
        match self.kind {
            LutKind::Lut1d => {
                let mut out = [0.0f32; 3];
                for c in 0..3 {
                    let i = (t[c] as usize).min(n - 2);
                    let f = t[c] - i as f32;
                    let (a, b) = (self.table[i][c], self.table[i + 1][c]);
                    out[c] = a + (b - a) * f;
                }
                out
            }
            LutKind::Lut3d => {
                let (ir, ig, ib) = ((t[0] as usize).min(n - 2), (t[1] as usize).min(n - 2), (t[2] as usize).min(n - 2));
                let (fr, fg, fb) = (t[0] - ir as f32, t[1] - ig as f32, t[2] - ib as f32);
                let base = ir + n * (ig + n * ib);
                let (dr, dg, db) = (1, n, n * n);
                let c = |off: usize| self.table[base + off];
                match interp {
                    Interpolation::Tetrahedral => tetrahedral(&c, [dr, dg, db], fr, fg, fb),
                    Interpolation::Trilinear => trilinear(&c, [dr, dg, db], fr, fg, fb),
                }
            }
        }
    }
}

#[inline]
fn mix4(w: [f32; 4], c: [[f32; 3]; 4]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        out[k] = w[0] * c[0][k] + w[1] * c[1][k] + w[2] * c[2][k] + w[3] * c[3][k];
    }
    out
}

/// Tetrahedral interpolation inside the cell at `c(0)`; `d` = index offsets of +r, +g, +b.
#[inline]
fn tetrahedral(c: &impl Fn(usize) -> [f32; 3], d: [usize; 3], fr: f32, fg: f32, fb: f32) -> [f32; 3] {
    let [dr, dg, db] = d;
    let c000 = c(0);
    let c111 = c(dr + dg + db);
    if fr > fg {
        if fg > fb {
            mix4([1.0 - fr, fr - fg, fg - fb, fb], [c000, c(dr), c(dr + dg), c111])
        } else if fr > fb {
            mix4([1.0 - fr, fr - fb, fb - fg, fg], [c000, c(dr), c(dr + db), c111])
        } else {
            mix4([1.0 - fb, fb - fr, fr - fg, fg], [c000, c(db), c(dr + db), c111])
        }
    } else if fb > fg {
        mix4([1.0 - fb, fb - fg, fg - fr, fr], [c000, c(db), c(dg + db), c111])
    } else if fb > fr {
        mix4([1.0 - fg, fg - fb, fb - fr, fr], [c000, c(dg), c(dg + db), c111])
    } else {
        mix4([1.0 - fg, fg - fr, fr - fb, fb], [c000, c(dg), c(dr + dg), c111])
    }
}

#[inline]
fn trilinear(c: &impl Fn(usize) -> [f32; 3], d: [usize; 3], fr: f32, fg: f32, fb: f32) -> [f32; 3] {
    let [dr, dg, db] = d;
    let lerp = |a: [f32; 3], b: [f32; 3], f: f32| {
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
    };
    let c00 = lerp(c(0), c(dr), fr);
    let c10 = lerp(c(dg), c(dr + dg), fr);
    let c01 = lerp(c(db), c(dr + db), fr);
    let c11 = lerp(c(dg + db), c(dr + dg + db), fr);
    lerp(lerp(c00, c10, fg), lerp(c01, c11, fg), fb)
}

/// Reads only the header of a `.cube` file (for listing).
fn read_header(path: &Path) -> Result<Header, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hb = HeaderBuilder::default();
    for (i, line) in BufReader::new(file).split(b'\n').enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let line = String::from_utf8_lossy(&line);
        if let Line::Data(_) = hb.line(&line, i + 1)? {
            break;
        }
    }
    hb.finish()
}

/// 64-bit FNV-1a.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `[a-z0-9-]` slug of `name`, at most 55 chars (room for `-xxxxxxxx`).
fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut out: String = out.chars().take(55).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "lut".into()
    } else {
        out
    }
}

/// Managed Tauri state: the LUT directory and a parsed-LUT cache. Cheap to clone.
#[derive(Debug, Clone)]
pub struct LutLibrary {
    dir: PathBuf,
    cache: Arc<Mutex<HashMap<String, Arc<Lut>>>>,
}

impl LutLibrary {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, cache: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.cube"))
    }

    fn info(&self, id: &str, h: &Header) -> LutInfo {
        LutInfo {
            id: id.to_owned(),
            name: h.title.clone().unwrap_or_else(|| id.to_owned()),
            kind: h.kind,
            size: h.size,
            path: self.path_of(id).to_string_lossy().into_owned(),
        }
    }

    pub fn list(&self) -> AppResult<Vec<LutInfo>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("cube") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).filter(|s| is_valid_lut_id(s)) else {
                continue;
            };
            if let Ok(h) = read_header(&path) {
                out.push(self.info(id, &h));
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Validates `path` as a `.cube`, copies it into the library (atomic temp + rename).
    pub fn import(&self, path: &Path) -> AppResult<LutInfo> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::not_found(format!("{}: no such file", path.display())))
            }
            Err(e) => return Err(AppError::invalid(format!("{}: {e}", path.display()))),
        };
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| AppError::invalid(format!("{}: not a text .cube file", path.display())))?;
        let lut = Lut::parse(&text).map_err(|e| AppError::invalid(format!("{}: {e}", path.display())))?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let id = format!("{}-{:08x}", slug(&stem), fnv1a(&bytes) as u32);
        let header = Header {
            kind: lut.kind,
            size: lut.size,
            title: lut.title.clone().or_else(|| Some(stem.clone()).filter(|s| !s.is_empty())),
            domain_min: lut.domain_min,
            domain_max: lut.domain_max,
        };
        let dest = self.path_of(&id);
        if dest.exists() {
            if let Ok(h) = read_header(&dest) {
                return Ok(self.info(&id, &h));
            }
        }
        fs::create_dir_all(&self.dir)?;
        let mut content = String::with_capacity(text.len() + 64);
        if lut.title.is_none() && !stem.is_empty() {
            content.push_str(&format!("TITLE \"{}\"\n", stem.replace('"', "'")));
        }
        content.push_str(&text);
        let tmp = self.dir.join(format!(".{id}.cube.tmp"));
        let written = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &dest)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(self.info(&id, &header))
    }

    /// Removes `<id>.cube` (`not_found` if absent). Reference checks are the caller's job.
    pub fn delete(&self, id: &str) -> AppResult<()> {
        if !is_valid_lut_id(id) {
            return Err(AppError::not_found(format!("LUT {id:?}")));
        }
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        match fs::remove_file(self.path_of(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::not_found(format!("LUT {id}"))),
            Err(e) => Err(e.into()),
        }
    }

    /// Parsed LUT, `Ok(None)` if the id is not in the library (or its file is unreadable).
    pub fn load(&self, id: &str) -> AppResult<Option<Arc<Lut>>> {
        if !is_valid_lut_id(id) {
            return Ok(None);
        }
        let path = self.path_of(id);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if !path.exists() {
            cache.remove(id);
            return Ok(None);
        }
        if let Some(lut) = cache.get(id) {
            return Ok(Some(lut.clone()));
        }
        drop(cache);
        let lut = match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Lut::parse(&t)) {
            Ok(l) => Arc::new(l),
            Err(e) => {
                eprintln!("LUT {id}: {e}");
                return Ok(None);
            }
        };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_ENTRIES {
            if let Some(k) = cache.keys().next().cloned() {
                cache.remove(&k);
            }
        }
        cache.insert(id.to_owned(), lut.clone());
        Ok(Some(lut))
    }
}

/// Number of images whose stored adjustments reference `id`
/// (`json_extract(params_json, '$.lut.id') = ?`).
pub fn references(conn: &Connection, id: &LutId) -> AppResult<u32> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM adjustments WHERE json_extract(params_json, '$.lut.id') = ?1",
        [id],
        |r| r.get(0),
    )?;
    Ok(n as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|c| (a[c] - b[c]).abs() < 1e-5)
    }

    fn identity_cube(n: usize) -> String {
        let mut s = format!("TITLE \"Identity\"\nLUT_3D_SIZE {n}\n");
        let m = (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    s.push_str(&format!("{} {} {}\n", r as f32 / m, g as f32 / m, b as f32 / m));
                }
            }
        }
        s
    }

    /// 2^3 LUT: every corner maps to a distinct non-linear colour, so interpolation
    /// methods produce different (hand-computed) results.
    fn corner_lut() -> Lut {
        // Red fastest: (r,g,b) = 000,100,010,110,001,101,011,111.
        let text = "# test\r\nLUT_3D_SIZE 2\r\n\
            0 0 0\r\n1 0 0\r\n0 1 0\r\n1 1 0\r\n0 0 1\r\n1 0 1\r\n0 1 1\r\n0.5 0.5 0.5\r\n";
        Lut::parse(text).unwrap()
    }

    #[test]
    fn identity_is_identity_both_methods() {
        for n in [2, 17, 33] {
            let lut = Lut::parse(&identity_cube(n)).unwrap();
            assert_eq!((lut.kind, lut.size, lut.title.as_deref()), (LutKind::Lut3d, n as u32, Some("Identity")));
            for p in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 0.6, 0.9], [0.123, 0.987, 0.5], [0.77, 0.01, 0.4]] {
                assert!(close(lut.apply(p, 100.0), p), "tetra {n} {p:?}");
                assert!(close(lut.apply_with(p, 100.0, Interpolation::Trilinear), p), "tri {n} {p:?}");
            }
            // Out of range input is clamped to the domain.
            assert!(close(lut.apply([-0.5, 1.5, 0.5], 100.0), [0.0, 1.0, 0.5]));
        }
    }

    #[test]
    fn corner_lut_reference_values() {
        let lut = corner_lut();
        // Corners reproduce exactly.
        assert!(close(lut.apply([1.0, 1.0, 1.0], 100.0), [0.5, 0.5, 0.5]));
        assert!(close(lut.apply([1.0, 0.0, 1.0], 100.0), [1.0, 0.0, 1.0]));
        // Centre: trilinear = mean of the 8 corners = (3.5/8, 3.5/8, 3.5/8).
        let tri = lut.apply_with([0.5, 0.5, 0.5], 100.0, Interpolation::Trilinear);
        assert!(close(tri, [0.4375, 0.4375, 0.4375]), "{tri:?}");
        // Centre: tetrahedral (fr = fg = fb -> last branch): 0.5*c000 + 0*c010 + 0*c110 + 0.5*c111.
        let tet = lut.apply([0.5, 0.5, 0.5], 100.0);
        assert!(close(tet, [0.25, 0.25, 0.25]), "{tet:?}");
        // p = (0.6, 0.3, 0.2): fr > fg > fb -> 0.4*c000 + 0.3*c100 + 0.1*c110 + 0.2*c111
        //   = (0.3 + 0.1 + 0.1, 0.1 + 0.1, 0.1) = (0.5, 0.2, 0.1).
        let tet = lut.apply([0.6, 0.3, 0.2], 100.0);
        assert!(close(tet, [0.5, 0.2, 0.1]), "{tet:?}");
        // Trilinear at (0.6, 0.3, 0.2): identity except c111 = 0.5 instead of 1, so
        // result = p - 0.5 * (0.6*0.3*0.2) per channel = p - 0.018.
        let tri = lut.apply_with([0.6, 0.3, 0.2], 100.0, Interpolation::Trilinear);
        assert!(close(tri, [0.582, 0.282, 0.182]), "{tri:?}");
        // p = (0.2, 0.7, 0.4): fg > fb > fr -> 0.3*c000 + 0.3*c010 + 0.2*c011 + 0.2*c111
        //   = (0.1, 0.3 + 0.2 + 0.1, 0.2 + 0.1) = (0.1, 0.6, 0.3).
        let tet = lut.apply([0.2, 0.7, 0.4], 100.0);
        assert!(close(tet, [0.1, 0.6, 0.3]), "{tet:?}");
        // Amount blends linearly: 50% of the way from p to LUT(p).
        let half = lut.apply([0.6, 0.3, 0.2], 50.0);
        assert!(close(half, [0.55, 0.25, 0.15]), "{half:?}");
        assert!(close(lut.apply([0.6, 0.3, 0.2], 0.0), [0.6, 0.3, 0.2]));
    }

    #[test]
    fn three_cubed_lut_squares_values() {
        // 3^3 LUT storing v^2 per channel at nodes 0, 0.5, 1 -> piecewise-linear between.
        let mut s = String::from("LUT_3D_SIZE 3\n");
        let sq = |i: usize| (i as f32 / 2.0).powi(2);
        for b in 0..3 {
            for g in 0..3 {
                for r in 0..3 {
                    s.push_str(&format!("{} {} {}\n", sq(r), sq(g), sq(b)));
                }
            }
        }
        let lut = Lut::parse(&s).unwrap();
        // Separable per-channel tables: both methods agree = piecewise linear of v^2.
        // 0.25 -> halfway 0..0.25 = 0.125; 0.75 -> 0.25 + 0.5*0.75 = 0.625; 0.5 -> 0.25.
        for m in [Interpolation::Tetrahedral, Interpolation::Trilinear] {
            let out = lut.apply_with([0.25, 0.75, 0.5], 100.0, m);
            assert!(close(out, [0.125, 0.625, 0.25]), "{m:?} {out:?}");
        }
    }

    #[test]
    fn one_d_lut_and_domain() {
        let text = "TITLE \"Curve\"\nLUT_1D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n0 0 0\n0.25 0.5 1\n1 1 1\n";
        let lut = Lut::parse(text).unwrap();
        assert_eq!((lut.kind, lut.size), (LutKind::Lut1d, 3));
        // Input 0.5 is 1/4 of the 0..2 domain -> half way between entries 0 and 1.
        let out = lut.apply([0.5, 0.5, 0.5], 100.0);
        assert!(close(out, [0.125, 0.25, 0.5]), "{out:?}");
        // Input 1.5 -> 3/4 -> half way between entries 1 and 2.
        let out = lut.apply([1.5, 1.5, 1.5], 100.0);
        assert!(close(out, [0.625, 0.75, 1.0]), "{out:?}");
        assert!(close(lut.apply([5.0, -1.0, 2.0], 100.0), [1.0, 0.0, 1.0]));

        // Resolve-style input range on a 3D identity scaled to 0..4.
        let mut s = String::from("LUT_3D_SIZE 2\nLUT_3D_INPUT_RANGE 0 4\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    s.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        let lut = Lut::parse(&s).unwrap();
        assert!(close(lut.apply([1.0, 2.0, 3.0], 100.0), [0.25, 0.5, 0.75]));
    }

    #[test]
    fn malformed_files_rejected() {
        let bad = [
            "",
            "0 0 0\n",
            "LUT_3D_SIZE 2\n0 0 0\n",
            "LUT_3D_SIZE 1\n0 0 0\n",
            "LUT_3D_SIZE 66\n",
            "LUT_1D_SIZE 70000\n",
            "LUT_3D_SIZE 2\n0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n",
            "LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\n0 0 0\n1 1 nan\n",
            "LUT_1D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 0 0 0\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\n0 0 0\nDOMAIN_MIN 0 0 0\n1 1 1\n",
            "hello world\nLUT_1D_SIZE 2\n0 0 0\n1 1 1\n",
            "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n2 2 2\n",
        ];
        for text in bad {
            assert!(Lut::parse(text).is_err(), "{text:?} should be rejected");
        }
        // Comments, blank lines, CRLF and trailing comments are fine.
        assert!(Lut::parse("# c\r\n\r\nLUT_1D_SIZE 2 # two\r\n0 0 0\r\n1 1 1 # white\r\n").is_ok());
    }

    #[test]
    fn library_import_list_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let lib = LutLibrary::new(dir.path().join("luts"));
        assert!(lib.list().unwrap().is_empty());

        let src = dir.path().join("My Film (v2).cube");
        fs::write(&src, identity_cube(5).replace("TITLE \"Identity\"\n", "")).unwrap();
        let info = lib.import(&src).unwrap();
        assert!(info.id.starts_with("my-film-v2-"), "{}", info.id);
        assert!(is_valid_lut_id(&info.id));
        assert_eq!((info.name.as_str(), info.kind, info.size), ("My Film (v2)", LutKind::Lut3d, 5));
        // Idempotent.
        assert_eq!(lib.import(&src).unwrap(), info);

        let other = dir.path().join("b.cube");
        fs::write(&other, "TITLE \"A curve\"\nLUT_1D_SIZE 2\n0 0 0\n1 1 1\n").unwrap();
        let info_b = lib.import(&other).unwrap();
        let listed = lib.list().unwrap();
        assert_eq!(listed, vec![info_b.clone(), info.clone()], "sorted by name");

        let lut = lib.load(&info.id).unwrap().unwrap();
        assert!(close(lut.apply([0.2, 0.4, 0.6], 100.0), [0.2, 0.4, 0.6]));
        assert!(lib.load("nope-00000000").unwrap().is_none());
        assert!(lib.load("../x").unwrap().is_none());

        let bad = dir.path().join("bad.cube");
        fs::write(&bad, "LUT_3D_SIZE 2\n0 0 0\n").unwrap();
        assert_eq!(lib.import(&bad).unwrap_err().kind, crate::ipc::error::ErrorKind::InvalidArgument);
        assert_eq!(lib.list().unwrap().len(), 2, "invalid file not copied");

        lib.delete(&info.id).unwrap();
        assert!(lib.load(&info.id).unwrap().is_none());
        assert_eq!(lib.delete(&info.id).unwrap_err().kind, crate::ipc::error::ErrorKind::NotFound);
        assert_eq!(lib.list().unwrap(), vec![info_b]);
    }

    #[test]
    fn references_counts_stored_adjustments() {
        let conn = crate::db::open_in_memory();
        conn.execute_batch(
            "INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0);
             INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                 file_size, file_mtime_ms, imported_at)
             VALUES (1, 1, '/f/a.arw', 'a.arw', 'arw', 'sony', 'bayer', 1, 0, 0),
                    (2, 1, '/f/b.arw', 'b.arw', 'arw', 'sony', 'bayer', 1, 0, 0);
             INSERT INTO adjustments (image_id, params_json, process_version, updated_at) VALUES
                 (1, '{\"lut\":{\"id\":\"film-1\",\"amount\":50}}', 1, 0),
                 (2, '{\"lut\":null}', 1, 0);",
        )
        .unwrap();
        assert_eq!(references(&conn, &"film-1".to_owned()).unwrap(), 1);
        assert_eq!(references(&conn, &"other".to_owned()).unwrap(), 0);
    }
}
