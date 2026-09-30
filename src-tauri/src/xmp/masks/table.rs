//! `crs:Table_<MaskDigest>` values: Adobe base-85 -> 16-byte header + TIFF (one 8-bit grey
//! JPEG XL tile) -> [`AlphaMask`] placed in the sensor frame.

use super::{jxl, LightroomMatte, MATTE_COMPRESSION_JXL, TABLE_ALPHABET};
use crate::develop::masks::AlphaMask;
use crate::ipc::types::NormRect;

/// Header of every Lightroom matte table (u32 LE).
pub const MATTE_HEADER: [u32; 4] = [2, 1, 0, 0];

fn decode_map() -> [u8; 256] {
    let mut map = [255u8; 256];
    for (i, &c) in TABLE_ALPHABET.iter().enumerate() {
        map[c as usize] = i as u8;
    }
    map
}

/// DNG SDK `dng_big_table` base-85: 5 chars -> one u32 LE, least-significant digit first; a
/// final group of `n + 1` chars carries `n` bytes. Characters outside the alphabet
/// (whitespace, line breaks) are skipped.
pub fn decode_base85(value: &str) -> Result<Vec<u8>, String> {
    let map = decode_map();
    let mut out = Vec::with_capacity(value.len() * 4 / 5 + 4);
    let (mut acc, mut mul, mut phase) = (0u64, 1u64, 0u32);
    for &c in value.as_bytes() {
        let d = map[c as usize];
        if d == 255 {
            if c.is_ascii_whitespace() {
                continue;
            }
            return Err(format!("invalid table character {:?}", c as char));
        }
        acc += u64::from(d) * mul;
        mul *= 85;
        phase += 1;
        if phase == 5 {
            if acc > u64::from(u32::MAX) {
                return Err("table group overflows 32 bits".into());
            }
            out.extend_from_slice(&(acc as u32).to_le_bytes());
            (acc, mul, phase) = (0, 1, 0);
        }
    }
    match phase {
        0 => {}
        1 => return Err("truncated table (dangling character)".into()),
        n => out.extend_from_slice(&(acc as u32).to_le_bytes()[..(n - 1) as usize]),
    }
    Ok(out)
}

/// Inverse of [`decode_base85`] (tests and tools).
#[cfg(test)]
pub fn encode_base85(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_le_bytes(b) as u64;
        let n = if chunk.len() == 4 { 5 } else { chunk.len() + 1 };
        for _ in 0..n {
            out.push(TABLE_ALPHABET[(v % 85) as usize] as char);
            v /= 85;
        }
    }
    out
}

/// A parsed single-IFD TIFF matte.
#[derive(Debug, Clone, PartialEq)]
pub struct MatteTiff {
    pub width: u32,
    pub height: u32,
    pub compression: u16,
    pub tile_width: u32,
    pub tile_height: u32,
    /// (offset, length) per tile (or strip), row-major.
    pub tiles: Vec<(usize, usize)>,
    pub bits: u16,
    pub samples: u16,
}

struct Rd<'a> {
    b: &'a [u8],
    le: bool,
}

impl Rd<'_> {
    fn u16(&self, at: usize) -> Result<u16, String> {
        let s = self.b.get(at..at + 2).ok_or("TIFF truncated")?;
        Ok(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, at: usize) -> Result<u32, String> {
        let s = self.b.get(at..at + 4).ok_or("TIFF truncated")?;
        let a = [s[0], s[1], s[2], s[3]];
        Ok(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    /// Values of an IFD entry as u32 (SHORT or LONG).
    fn values(&self, entry: usize) -> Result<Vec<u32>, String> {
        let typ = self.u16(entry + 2)?;
        let count = self.u32(entry + 4)? as usize;
        let size = match typ {
            3 => 2,
            4 => 4,
            _ => return Err(format!("unsupported TIFF field type {typ}")),
        };
        if count > 1 << 20 {
            return Err("TIFF field too large".into());
        }
        let base = if size * count <= 4 { entry + 8 } else { self.u32(entry + 8)? as usize };
        (0..count)
            .map(|i| if size == 2 { self.u16(base + 2 * i).map(u32::from) } else { self.u32(base + 4 * i) })
            .collect()
    }
}

pub fn parse_tiff(b: &[u8]) -> Result<MatteTiff, String> {
    let le = match b.get(..4) {
        Some([b'I', b'I', 42, 0]) => true,
        Some([b'M', b'M', 0, 42]) => false,
        _ => return Err("matte table is not a TIFF".into()),
    };
    let r = Rd { b, le };
    let ifd = r.u32(4)? as usize;
    let n = r.u16(ifd)? as usize;
    let mut t = MatteTiff {
        width: 0,
        height: 0,
        compression: 1,
        tile_width: 0,
        tile_height: 0,
        tiles: Vec::new(),
        bits: 8,
        samples: 1,
    };
    let (mut offsets, mut counts, mut rows_per_strip) = (Vec::new(), Vec::new(), 0u32);
    for i in 0..n {
        let e = ifd + 2 + 12 * i;
        let tag = r.u16(e)?;
        let first = || r.values(e).and_then(|v| v.first().copied().ok_or_else(|| "empty TIFF field".to_owned()));
        match tag {
            256 => t.width = first()?,
            257 => t.height = first()?,
            258 => t.bits = first()? as u16,
            259 => t.compression = first()? as u16,
            277 => t.samples = first()? as u16,
            278 => rows_per_strip = first()?,
            322 => t.tile_width = first()?,
            323 => t.tile_height = first()?,
            273 | 324 => offsets = r.values(e)?,
            279 | 325 => counts = r.values(e)?,
            _ => {}
        }
    }
    if t.width == 0 || t.height == 0 || offsets.is_empty() || offsets.len() != counts.len() {
        return Err("matte TIFF lacks size or data".into());
    }
    if t.tile_width == 0 || t.tile_height == 0 {
        // Strips: one full-width "tile" per strip.
        t.tile_width = t.width;
        t.tile_height = if rows_per_strip == 0 { t.height } else { rows_per_strip.min(t.height) };
    }
    for (o, c) in offsets.iter().zip(&counts) {
        let (o, c) = (*o as usize, *c as usize);
        if o.checked_add(c).is_none_or(|end| end > b.len()) {
            return Err("matte tile outside the table".into());
        }
        t.tiles.push((o, c));
    }
    Ok(t)
}

/// Grey 8-bit pixels of a matte table's TIFF (tiles assembled).
pub fn decode_tiff_pixels(b: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let t = parse_tiff(b)?;
    if t.bits != 8 || t.samples != 1 {
        return Err(format!("unsupported matte format ({} bits, {} samples)", t.bits, t.samples));
    }
    let (w, h) = (t.width as usize, t.height as usize);
    let (tw, th) = (t.tile_width as usize, t.tile_height as usize);
    let across = w.div_ceil(tw);
    let down = h.div_ceil(th);
    if t.tiles.len() < across * down {
        return Err("matte TIFF has too few tiles".into());
    }
    let mut out = vec![0u8; w * h];
    for (k, &(off, len)) in t.tiles.iter().enumerate().take(across * down) {
        let data = &b[off..off + len];
        let (pw, ph, px) = match t.compression {
            MATTE_COMPRESSION_JXL => jxl::decode_gray8(data)?,
            1 => (tw as u32, (len / tw.max(1)) as u32, data.to_vec()),
            c => return Err(format!("unsupported matte compression {c}")),
        };
        let (pw, ph) = (pw as usize, ph as usize);
        let (x0, y0) = ((k % across) * tw, (k / across) * th);
        for y in 0..ph.min(th) {
            let oy = y0 + y;
            if oy >= h {
                break;
            }
            let n = pw.min(tw).min(w - x0);
            if (y + 1) * pw > px.len() {
                break;
            }
            out[oy * w + x0..oy * w + x0 + n].copy_from_slice(&px[y * pw..y * pw + n]);
        }
    }
    Ok((t.width, t.height, out))
}

/// See [`super::decode_matte`].
pub fn decode_matte(m: &LightroomMatte) -> Result<AlphaMask, String> {
    let bytes = decode_base85(&m.table)?;
    if bytes.len() < 16 + 8 {
        return Err("matte table too short".into());
    }
    let header: Vec<u32> = bytes[..16].chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    if header != MATTE_HEADER {
        eprintln!("matte {}: unexpected table header {header:?}; decoding anyway", m.digest);
    }
    let (w, h, data) = decode_tiff_pixels(&bytes[16..])?;
    let bounds = placement(m, w, h);
    Ok(AlphaMask { width: w, height: h, bounds, data })
}

/// Matte bounds in the sensor frame: `Origin` inside `WholeImageArea` (top, left, bottom,
/// right); an unknown area means the bitmap covers the whole frame.
pub fn placement(m: &LightroomMatte, w: u32, h: u32) -> NormRect {
    let [top, left, bottom, right] = m.whole_area;
    let (aw, ah) = (right - left, bottom - top);
    if aw <= 0.0 || ah <= 0.0 {
        return NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
    }
    NormRect {
        x: ((m.origin[0] - left) / aw) as f32,
        y: ((m.origin[1] - top) / ah) as f32,
        width: (f64::from(w) / aw) as f32,
        height: (f64::from(h) / ah) as f32,
    }
}
