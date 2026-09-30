//! HEIF/HEIC metadata items (Phase 7b): the `Exif` item (TIFF), the XMP `mime` item
//! (`application/rdf+xml`) and the primary image's `ispe` size / `irot` + `imir`
//! transforms. Pixels are decoded by macOS ImageIO (`raw::imageio`); this parser only reads
//! the few KB of `meta` box plus the item payloads.

use super::cr3::{children, find, BoxHdr};
use super::source::ByteSource;

/// Items larger than this are ignored (metadata, not pixels).
const MAX_ITEM: u64 = 16 << 20;
const MAX_ITEMS: usize = 4096;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct HeifMeta {
    /// TIFF bytes of the Exif item (after its 4-byte header offset).
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    /// Primary image size (before `irot`).
    pub size: Option<(u32, u32)>,
    /// EXIF-style orientation from `irot`/`imir` of the primary item (1 when absent).
    pub orientation: Option<u8>,
}

#[derive(Debug, Clone, Default)]
struct Item {
    id: u32,
    typ: [u8; 4],
    content_type: String,
}

#[derive(Debug, Clone)]
struct Extent {
    offset: u64,
    len: u64,
}

#[derive(Debug, Clone)]
struct Location {
    id: u32,
    method: u8,
    extents: Vec<Extent>,
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn uint(&mut self, size: u8) -> Option<u64> {
        match size {
            0 => Some(0),
            4 => self.u32().map(u64::from),
            8 => Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?)),
            _ => None,
        }
    }
    fn cstr(&mut self) -> Option<String> {
        let rest = self.b.get(self.pos..)?;
        let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.pos += (n + 1).min(rest.len());
        Some(s)
    }
}

fn payload(src: &(impl ByteSource + ?Sized), b: &BoxHdr) -> Option<Vec<u8>> {
    let len = b.end.checked_sub(b.start)?;
    if len > MAX_ITEM {
        return None;
    }
    src.read_vec(b.start, len as usize).ok()
}

fn parse_iinf(b: &[u8]) -> Vec<Item> {
    let mut r = Reader::new(b);
    let Some(version) = r.u8() else { return Vec::new() };
    let _ = r.take(3);
    let count = if version == 0 { r.u16().map(u32::from) } else { r.u32() }.unwrap_or(0) as usize;
    let mut out = Vec::new();
    for _ in 0..count.min(MAX_ITEMS) {
        let (Some(size), Some(typ)) = (r.u32(), r.take(4)) else { break };
        let start = r.pos;
        let end = start + (size as usize).saturating_sub(8);
        if typ == b"infe" {
            if let Some(body) = b.get(start..end) {
                if let Some(item) = parse_infe(body) {
                    out.push(item);
                }
            }
        }
        r.pos = end;
    }
    out
}

fn parse_infe(b: &[u8]) -> Option<Item> {
    let mut r = Reader::new(b);
    let version = r.u8()?;
    r.take(3)?;
    if version < 2 {
        return None;
    }
    let id = if version == 2 { u32::from(r.u16()?) } else { r.u32()? };
    r.u16()?; // protection index
    let typ: [u8; 4] = r.take(4)?.try_into().ok()?;
    let _name = r.cstr();
    let content_type = if &typ == b"mime" { r.cstr().unwrap_or_default() } else { String::new() };
    Some(Item { id, typ, content_type })
}

fn parse_iloc(b: &[u8]) -> Vec<Location> {
    let mut r = Reader::new(b);
    let mut out = Vec::new();
    let Some(version) = r.u8() else { return out };
    if r.take(3).is_none() {
        return out;
    }
    let (Some(a), Some(c)) = (r.u8(), r.u8()) else { return out };
    let (offset_size, length_size, base_size) = (a >> 4, a & 15, c >> 4);
    let index_size = if version >= 1 { c & 15 } else { 0 };
    let count = if version < 2 { r.u16().map(u32::from) } else { r.u32() }.unwrap_or(0) as usize;
    for _ in 0..count.min(MAX_ITEMS) {
        let parsed = (|| {
            let id = if version < 2 { u32::from(r.u16()?) } else { r.u32()? };
            let method = if version >= 1 { (r.u16()? & 15) as u8 } else { 0 };
            r.u16()?; // data reference index
            let base = r.uint(base_size)?;
            let n = r.u16()? as usize;
            let mut extents = Vec::with_capacity(n.min(64));
            for _ in 0..n {
                if index_size > 0 {
                    r.uint(index_size)?;
                }
                let offset = base.checked_add(r.uint(offset_size)?)?;
                let len = r.uint(length_size)?;
                extents.push(Extent { offset, len });
            }
            Some(Location { id, method, extents })
        })();
        match parsed {
            Some(l) => out.push(l),
            None => break,
        }
    }
    out
}

/// `(item id -> property indices (1-based))` from `ipma`.
fn parse_ipma(b: &[u8], item: u32) -> Vec<u16> {
    let mut r = Reader::new(b);
    let Some(version) = r.u8() else { return Vec::new() };
    let Some(flags) = r.take(3) else { return Vec::new() };
    let wide = flags[2] & 1 == 1;
    let count = r.u32().unwrap_or(0) as usize;
    for _ in 0..count.min(MAX_ITEMS) {
        let Some(id) = (if version < 1 { r.u16().map(u32::from) } else { r.u32() }) else { break };
        let Some(n) = r.u8() else { break };
        let mut props = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let idx = if wide { r.u16().map(|v| v & 0x7FFF) } else { r.u8().map(|v| u16::from(v & 0x7F)) };
            match idx {
                Some(i) => props.push(i),
                None => return Vec::new(),
            }
        }
        if id == item {
            return props;
        }
    }
    Vec::new()
}

/// EXIF orientation for a HEIF `irot` (anticlockwise quarter turns) followed by `imir`
/// (`axis` 0 = vertical axis, i.e. left/right flip; 1 = horizontal axis).
pub fn orientation_from(irot: u8, imir: Option<u8>) -> u8 {
    let rot = irot & 3;
    match (rot, imir.map(|a| a & 1)) {
        (0, None) => 1,
        (1, None) => 8,
        (2, None) => 3,
        (_, None) => 6,
        (0, Some(0)) => 2,
        (0, Some(_)) => 4,
        (1, Some(0)) => 7,
        (1, Some(_)) => 5,
        (2, Some(0)) => 4,
        (2, Some(_)) => 2,
        (3, Some(0)) => 5,
        (_, Some(_)) => 7,
    }
}

fn read_item(src: &(impl ByteSource + ?Sized), loc: &Location, idat: Option<&BoxHdr>) -> Option<Vec<u8>> {
    let total: u64 = loc.extents.iter().map(|e| e.len).sum();
    if total == 0 || total > MAX_ITEM {
        return None;
    }
    let mut out = Vec::with_capacity(total as usize);
    for e in &loc.extents {
        let at = match loc.method {
            0 => e.offset,
            1 => idat?.start.checked_add(e.offset)?,
            _ => return None,
        };
        if loc.method == 1 && at.checked_add(e.len)? > idat?.end {
            return None;
        }
        out.extend_from_slice(&src.read_vec(at, e.len as usize).ok()?);
    }
    Some(out)
}

/// Parses the `meta` box of a HEIF file.
pub fn parse(src: &(impl ByteSource + ?Sized)) -> Result<HeifMeta, String> {
    let top = children(src, 0, src.len());
    if top.first().map(|b| &b.typ) != Some(b"ftyp") {
        return Err("not an ISO-BMFF file".into());
    }
    let meta = find(&top, b"meta").ok_or("HEIF: no meta box")?;
    // `meta` is a FullBox: 4 bytes of version/flags before its children.
    let boxes = children(src, meta.start + 4, meta.end);
    let get = |t: &[u8; 4]| find(&boxes, t).and_then(|b| payload(src, b));
    let items = get(b"iinf").map(|b| parse_iinf(&b)).unwrap_or_default();
    let locs = get(b"iloc").map(|b| parse_iloc(&b)).unwrap_or_default();
    let idat = find(&boxes, b"idat");
    let primary = get(b"pitm").and_then(|b| {
        let mut r = Reader::new(&b);
        let v = r.u8()?;
        r.take(3)?;
        if v == 0 {
            r.u16().map(u32::from)
        } else {
            r.u32()
        }
    });
    let content = |pred: &dyn Fn(&Item) -> bool| -> Option<Vec<u8>> {
        let item = items.iter().find(|i| pred(i))?;
        let loc = locs.iter().find(|l| l.id == item.id)?;
        read_item(src, loc, idat)
    };
    let exif = content(&|i| &i.typ == b"Exif").and_then(|b| {
        let off = u32::from_be_bytes(b.get(..4)?.try_into().ok()?) as usize;
        let tiff = b.get(4 + off..)?;
        // Some writers put "Exif\0\0" before the TIFF header without counting it.
        let tiff = tiff.strip_prefix(b"Exif\0\0").unwrap_or(tiff);
        (tiff.starts_with(b"II*\0") || tiff.starts_with(b"MM\0*")).then(|| tiff.to_vec())
    });
    let xmp = content(&|i| {
        &i.typ == b"mime" && matches!(i.content_type.trim(), "application/rdf+xml" | "application/xmp+xml")
    });

    let mut size = None;
    let mut orientation = None;
    if let (Some(pid), Some(iprp)) = (primary, find(&boxes, b"iprp")) {
        let props = children(src, iprp.start, iprp.end);
        if let (Some(ipco), Some(ipma)) = (find(&props, b"ipco"), find(&props, b"ipma").and_then(|b| payload(src, b))) {
            let list = children(src, ipco.start, ipco.end);
            let (mut irot, mut imir) = (0u8, None);
            for idx in parse_ipma(&ipma, pid) {
                let Some(p) = idx.checked_sub(1).and_then(|i| list.get(i as usize)) else { continue };
                let Some(body) = payload(src, p) else { continue };
                match &p.typ {
                    b"ispe" if body.len() >= 12 => {
                        let w = u32::from_be_bytes(body[4..8].try_into().unwrap_or_default());
                        let h = u32::from_be_bytes(body[8..12].try_into().unwrap_or_default());
                        if w > 0 && h > 0 {
                            size = Some((w, h));
                        }
                    }
                    b"irot" if !body.is_empty() => irot = body[0] & 3,
                    b"imir" if !body.is_empty() => imir = Some(body[0] & 1),
                    _ => {}
                }
            }
            orientation = Some(orientation_from(irot, imir));
        }
    }
    Ok(HeifMeta { exif, xmp, size, orientation })
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Builds a minimal HEIF container (no image data) with Exif + XMP items.

    fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(body);
        v
    }

    fn full(typ: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(body);
        bx(typ, &b)
    }

    fn infe(id: u16, typ: &[u8; 4], content_type: Option<&str>) -> Vec<u8> {
        let mut b = id.to_be_bytes().to_vec();
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(typ);
        b.push(0);
        if let Some(c) = content_type {
            b.extend_from_slice(c.as_bytes());
            b.push(0);
        }
        full(b"infe", 2, 0, &b)
    }

    /// `ftyp heic` + `meta` (pitm 1, iinf, iloc with file offsets, iprp ispe/irot) + `mdat`.
    pub fn heif(exif_tiff: Option<&[u8]>, xmp: Option<&str>, size: (u32, u32), irot: u8) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let mut exif_item = Vec::new();
        if let Some(t) = exif_tiff {
            exif_item.extend_from_slice(&6u32.to_be_bytes());
            exif_item.extend_from_slice(b"Exif\0\0");
            exif_item.extend_from_slice(t);
        }
        let xmp_item = xmp.map(|x| x.as_bytes().to_vec()).unwrap_or_default();
        let build = |mdat_at: u32| -> Vec<u8> {
            let mut iinf_body = 3u16.to_be_bytes().to_vec();
            iinf_body.extend(infe(1, b"hvc1", None));
            iinf_body.extend(infe(2, b"Exif", None));
            iinf_body.extend(infe(3, b"mime", Some("application/rdf+xml")));
            let iinf = full(b"iinf", 0, 0, &iinf_body);
            // iloc v1: offset 4, length 4, base 0, index 0.
            let mut il = vec![0x44, 0x00];
            il.extend_from_slice(&2u16.to_be_bytes());
            let mut at = mdat_at + 8;
            for (id, data) in [(2u16, &exif_item), (3u16, &xmp_item)] {
                il.extend_from_slice(&id.to_be_bytes());
                il.extend_from_slice(&0u16.to_be_bytes()); // construction method 0
                il.extend_from_slice(&0u16.to_be_bytes()); // data ref
                il.extend_from_slice(&1u16.to_be_bytes());
                il.extend_from_slice(&at.to_be_bytes());
                il.extend_from_slice(&(data.len() as u32).to_be_bytes());
                at += data.len() as u32;
            }
            let iloc = full(b"iloc", 1, 0, &il);
            let pitm = full(b"pitm", 0, 0, &1u16.to_be_bytes());
            let mut ispe_b = size.0.to_be_bytes().to_vec();
            ispe_b.extend_from_slice(&size.1.to_be_bytes());
            let ipco = bx(b"ipco", &[full(b"ispe", 0, 0, &ispe_b), bx(b"irot", &[irot])].concat());
            let mut ipma_b = 1u32.to_be_bytes().to_vec();
            ipma_b.extend_from_slice(&1u16.to_be_bytes());
            ipma_b.extend_from_slice(&[2, 1, 2]);
            let iprp = bx(b"iprp", &[ipco, full(b"ipma", 0, 0, &ipma_b)].concat());
            let hdlr = full(b"hdlr", 0, 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
            full(b"meta", 0, 0, &[hdlr, pitm, iinf, iloc, iprp].concat())
        };
        let probe = build(0);
        let mdat_at = (ftyp.len() + probe.len()) as u32;
        let meta = build(mdat_at);
        let mdat = bx(b"mdat", &[exif_item.clone(), xmp_item.clone()].concat());
        [ftyp, meta, mdat].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_items_and_properties() {
        let tiff = b"MM\0*\0\0\0\x08\0\0\0\0\0\0".to_vec();
        let file = test_support::heif(Some(&tiff), Some("<x:xmpmeta/>"), (4032, 3024), 3);
        let m = parse(file.as_slice()).unwrap();
        assert_eq!(m.exif.as_deref(), Some(tiff.as_slice()));
        assert_eq!(m.xmp.as_deref(), Some(b"<x:xmpmeta/>".as_slice()));
        assert_eq!(m.size, Some((4032, 3024)));
        assert_eq!(m.orientation, Some(6));
        let bare = test_support::heif(None, None, (10, 20), 0);
        let m = parse(bare.as_slice()).unwrap();
        assert_eq!((m.exif, m.xmp, m.orientation), (None, None, Some(1)));
        assert!(parse(b"garbage bytes here".as_slice()).is_err());
    }

    #[test]
    fn irot_imir_to_exif() {
        assert_eq!(orientation_from(0, None), 1);
        assert_eq!(orientation_from(1, None), 8);
        assert_eq!(orientation_from(3, None), 6);
        assert_eq!(orientation_from(0, Some(0)), 2);
        assert_eq!(orientation_from(0, Some(1)), 4);
    }
}
