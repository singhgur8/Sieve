//! Metadata for exported files.
//!
//! Sources: the RAW's EXIF (`raw::exif_dirs`: IFD0 / EXIF / GPS directories copied tag by
//! tag through a whitelist, so maker notes and RAW-structure tags never leak) and the XMP
//! sidecar (`xmp::resolve_sidecar`, read-only; descriptive fields only). Rules per
//! `MetadataInclude` are documented on the IPC type; always: orientation = 1, `crs:`
//! develop settings, `Sieve|*` keywords (and their flat `dc:subject` twins) and the Sieve
//! pick label are never copied, `MetadataOptions.copyright` / `creator` override the source
//! values, software = "Sieve". A new XMP packet is built (the sidecar is never copied as a
//! whole). Verified with `exiftool` in the Phase 6 QA gate.

use std::path::Path;

use quick_xml::escape::escape;

use super::tiffw::{self, Dirs};
use crate::ipc::error::AppResult;
use crate::ipc::types::{CullTag, ExportColorSpace, MetadataInclude, MetadataOptions};
use crate::raw::tiff::{ExifDirs, RawTag};
use crate::xmp::packet::{self, ExportValues, NS_EXIF, NS_IPTC_CORE, NS_PHOTOSHOP};

pub const SOFTWARE: &str = "Sieve";

/// IFD0 tags copied from the RAW with `all` (Artist/Copyright are resolved separately).
const IFD0_ALL: &[u16] = &[tiffw::MAKE, tiffw::MODEL, tiffw::DATE_TIME];
/// EXIF sub-IFD tags copied with `all`: exposure, capture time/zone, flash, lens, serials.
const EXIF_ALL: &[u16] = &[
    0x829A, 0x829D, 0x8822, 0x8827, 0x8830, 0x8831, 0x8832, 0x8833, 0x9003, 0x9004, 0x9010, 0x9011, 0x9012, 0x9201,
    0x9202, 0x9203, 0x9204, 0x9205, 0x9206, 0x9207, 0x9208, 0x9209, 0x920A, 0x9290, 0x9291, 0x9292, 0xA402, 0xA403,
    0xA404, 0xA405, 0xA406, 0xA407, 0xA408, 0xA409, 0xA40A, 0xA40C, 0xA430, 0xA431, 0xA432, 0xA433, 0xA434, 0xA435,
];

/// Metadata to embed, already filtered by the options. `Default` = nothing but the
/// technical tags the encoder always writes (ICC, resolution, orientation 1, software).
#[derive(Debug, Clone, Default)]
pub struct ExportMetadata {
    /// Descriptive IFD0 tags (make, model, date, artist, copyright), little-endian values.
    pub ifd0: Vec<RawTag>,
    /// EXIF sub-IFD tags from the RAW (only with `all`).
    pub exif: Vec<RawTag>,
    /// GPS sub-IFD tags (only with `all` and not `removeLocation`).
    pub gps: Vec<RawTag>,
    /// XMP packet, if any.
    pub xmp: Option<String>,
}

/// Container of an EXIF block (decides the container-specific required tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExifContainer {
    /// JPEG APP1: adds `YCbCrPositioning` and `ComponentsConfiguration`.
    Jpeg,
    /// PNG `eXIf` / WebP `EXIF`.
    Other,
}

impl ExportMetadata {
    /// EXIF directories for an image file (`w x h`, `ppi`, `space`): orientation 1,
    /// resolution, software plus the collected tags; the EXIF sub-IFD gets the version,
    /// FlashPix version, colour space and pixel dimensions when it carries RAW tags.
    pub fn exif_dirs(&self, w: u32, h: u32, ppi: u32, space: ExportColorSpace, container: ExifContainer) -> Dirs {
        let mut ifd0 = vec![
            tiffw::short(tiffw::ORIENTATION, &[1]),
            tiffw::rational(tiffw::X_RESOLUTION, ppi, 1),
            tiffw::rational(tiffw::Y_RESOLUTION, ppi, 1),
            tiffw::short(tiffw::RESOLUTION_UNIT, &[2]),
            tiffw::ascii(tiffw::SOFTWARE, SOFTWARE),
        ];
        let jpeg = container == ExifContainer::Jpeg;
        if jpeg {
            // 1 = centred (JFIF / TurboJPEG chroma siting).
            ifd0.push(tiffw::short(YCBCR_POSITIONING, &[1]));
        }
        ifd0.extend(self.ifd0.iter().cloned());
        let mut exif = self.exif_ifd(space, Some((w, h)));
        if jpeg && !exif.is_empty() {
            exif.push(tiffw::undefined(COMPONENTS_CONFIGURATION, &[1, 2, 3, 0]));
        }
        Dirs { ifd0, exif, gps: self.gps.clone() }
    }

    /// Tags for a TIFF file's own IFD0 (the writer adds the image structure tags). The EXIF
    /// IFD omits `PixelX/YDimension` (not allowed in TIFF; ImageWidth/Length are used).
    pub fn tiff_dirs(&self, space: ExportColorSpace, icc: &[u8]) -> Dirs {
        let mut ifd0 = vec![tiffw::ascii(tiffw::SOFTWARE, SOFTWARE), tiffw::undefined(tiffw::ICC_PROFILE, icc)];
        if let Some(x) = &self.xmp {
            ifd0.push(tiffw::bytes(tiffw::XMP, x.as_bytes()));
        }
        ifd0.extend(self.ifd0.iter().cloned());
        Dirs { ifd0, exif: self.exif_ifd(space, None), gps: self.gps.clone() }
    }

    fn exif_ifd(&self, space: ExportColorSpace, dims: Option<(u32, u32)>) -> Vec<RawTag> {
        if self.exif.is_empty() {
            return Vec::new();
        }
        let mut exif = vec![
            tiffw::undefined(tiffw::EXIF_VERSION, b"0232"),
            tiffw::undefined(FLASHPIX_VERSION, b"0100"),
            // 1 = sRGB; 0xFFFF = uncalibrated (the ICC profile defines the space).
            tiffw::short(tiffw::COLOR_SPACE, &[if space == ExportColorSpace::Srgb { 1 } else { 0xFFFF }]),
        ];
        if let Some((w, h)) = dims {
            exif.push(tiffw::long(tiffw::PIXEL_X_DIMENSION, &[w]));
            exif.push(tiffw::long(tiffw::PIXEL_Y_DIMENSION, &[h]));
        }
        exif.extend(self.exif.iter().cloned());
        exif
    }
}

const YCBCR_POSITIONING: u16 = 0x0213;
const COMPONENTS_CONFIGURATION: u16 = 0x9101;
const FLASHPIX_VERSION: u16 = 0xA000;

impl ExportMetadata {
    /// The XMP packet for containers whose writer only takes XMP (HEIC via ImageIO): the
    /// descriptive packet plus the collected EXIF as `tiff:` / `exif:` / `exifEX:`
    /// properties (ImageIO derives the file's EXIF from them). Always has orientation 1.
    pub fn xmp_with_exif(&self) -> String {
        let mut attrs = vec!["tiff:Orientation=\"1\"".to_owned(), format!("tiff:Software=\"{SOFTWARE}\"")];
        let mut push = |name: &str, v: Option<String>| {
            if let Some(v) = v {
                attrs.push(format!("{name}=\"{}\"", escape(v.as_str())));
            }
        };
        push("tiff:Make", ascii_value(&self.ifd0, tiffw::MAKE));
        push("tiff:Model", ascii_value(&self.ifd0, tiffw::MODEL));
        push("tiff:Artist", ascii_value(&self.ifd0, tiffw::ARTIST));
        push("tiff:Copyright", ascii_value(&self.ifd0, tiffw::COPYRIGHT));
        push("exif:ExposureTime", rational_value(&self.exif, 0x829A));
        push("exif:FNumber", rational_value(&self.exif, 0x829D));
        push("exif:FocalLength", rational_value(&self.exif, 0x920A));
        push("exif:ExposureBiasValue", rational_value(&self.exif, 0x9204));
        push("exifEX:LensModel", ascii_value(&self.exif, 0xA434));
        push("exif:DateTimeOriginal", ascii_value(&self.exif, 0x9003).and_then(|d| xmp_date(&d)));
        push("exif:GPSLatitude", gps_coord(&self.gps, 2, 1));
        push("exif:GPSLongitude", gps_coord(&self.gps, 4, 3));
        push("exif:GPSAltitude", rational_value(&self.gps, 6));
        let iso = uint_value(&self.exif, 0x8827).map(|v| {
            format!("   <exif:ISOSpeedRatings>\n    <rdf:Seq>\n     <rdf:li>{v}</rdf:li>\n    </rdf:Seq>\n   </exif:ISOSpeedRatings>\n")
        });
        let desc = format!(
            "  <rdf:Description rdf:about=\"\"\n    xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n    \
xmlns:exif=\"{NS_EXIF}\"\n    xmlns:exifEX=\"http://cipa.jp/exif/1.0/\"\n    {}>\n{}  </rdf:Description>\n",
            attrs.join("\n    "),
            iso.unwrap_or_default()
        );
        let base = self.xmp.clone().unwrap_or_else(|| XmpOut::default().render());
        match base.rfind(" </rdf:RDF>") {
            Some(i) => format!("{}{desc}{}", &base[..i], &base[i..]),
            None => base,
        }
    }
}

fn tag(tags: &[RawTag], tag: u16) -> Option<&RawTag> {
    tags.iter().find(|t| t.tag == tag)
}

fn rationals(t: &RawTag) -> Vec<(i64, i64)> {
    t.data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            let (n, d) = ([c[0], c[1], c[2], c[3]], [c[4], c[5], c[6], c[7]]);
            if t.typ == 10 {
                (i64::from(i32::from_le_bytes(n)), i64::from(i32::from_le_bytes(d)))
            } else {
                (i64::from(u32::from_le_bytes(n)), i64::from(u32::from_le_bytes(d)))
            }
        })
        .collect()
}

/// XMP rational `n/d` of a RATIONAL / SRATIONAL tag.
fn rational_value(tags: &[RawTag], id: u16) -> Option<String> {
    let t = tag(tags, id).filter(|t| t.typ == 5 || t.typ == 10)?;
    let (n, d) = *rationals(t).first()?;
    (d != 0).then(|| format!("{n}/{d}"))
}

fn uint_value(tags: &[RawTag], id: u16) -> Option<u32> {
    let t = tag(tags, id)?;
    match t.typ {
        3 => t.data.get(..2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]]))),
        4 => t.data.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        _ => None,
    }
}

/// EXIF `YYYY:MM:DD hh:mm:ss` -> XMP `YYYY-MM-DDThh:mm:ss`.
fn xmp_date(d: &str) -> Option<String> {
    let (date, time) = d.trim().split_once(' ')?;
    (date.len() == 10 && time.len() >= 8).then(|| format!("{}T{}", date.replace(':', "-"), &time[..8]))
}

/// XMP GPS coordinate `DDD,MM.mmmmmR` from the EXIF value (3 rationals) and ref tags.
fn gps_coord(gps: &[RawTag], value: u16, reference: u16) -> Option<String> {
    let r = ascii_value(gps, reference)?;
    let v = rationals(tag(gps, value).filter(|t| t.typ == 5)?);
    if v.len() < 3 || v.iter().any(|(_, d)| *d == 0) {
        return None;
    }
    let f = |i: usize| v[i].0 as f64 / v[i].1 as f64;
    let minutes = f(1) + f(2) / 60.0;
    Some(format!("{},{:.6}{}", f(0) as i64, minutes, r.chars().next()?))
}

fn ascii_value(tags: &[RawTag], tag: u16) -> Option<String> {
    let t = tags.iter().find(|t| t.tag == tag && t.typ == tiffw::T_ASCII)?;
    let end = t.data.iter().position(|&b| b == 0).unwrap_or(t.data.len());
    let s = String::from_utf8_lossy(&t.data[..end]).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn read_sidecar(raw_path: &Path) -> Option<ExportValues> {
    let path = crate::xmp::resolve_sidecar(raw_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // Non-RAW without a sidecar: the file's own (embedded) XMP, read-only.
        Err(_) => {
            let format = crate::raw::format_from_extension(raw_path).filter(|f| !f.is_raw())?;
            crate::raw::raster::embedded_xmp(raw_path, format).ok().flatten()?
        }
    };
    packet::export_values(&text).ok()
}

fn is_sieve_keyword(k: &str) -> bool {
    k == packet::KEYWORD_ROOT || k.starts_with(&format!("{}|", packet::KEYWORD_ROOT))
}

/// Blocking. Reads the RAW's EXIF and its sidecar (if present) and filters them per `options`.
/// A missing/unreadable sidecar is not an error (EXIF only); unreadable RAW EXIF exports
/// without camera metadata.
pub fn collect(raw_path: &Path, options: &MetadataOptions) -> AppResult<ExportMetadata> {
    if options.include == MetadataInclude::None {
        return Ok(ExportMetadata::default());
    }
    let dirs = crate::raw::exif_dirs(raw_path).unwrap_or_default();
    let side = read_sidecar(raw_path).unwrap_or_default();
    Ok(build(&dirs, &side, options))
}

/// Pure part of [`collect`].
pub fn build(dirs: &ExifDirs, side: &ExportValues, options: &MetadataOptions) -> ExportMetadata {
    let copyright = non_empty(&options.copyright)
        .or_else(|| side.rights.clone())
        .or_else(|| ascii_value(&dirs.ifd0, tiffw::COPYRIGHT));
    let creators: Vec<String> = match non_empty(&options.creator) {
        Some(c) => vec![c],
        None if !side.creator.is_empty() => side.creator.clone(),
        None => ascii_value(&dirs.ifd0, tiffw::ARTIST).into_iter().collect(),
    };
    let mut out = ExportMetadata::default();
    let mut x = XmpOut { rights: copyright.clone(), ..Default::default() };
    if let Some(c) = &copyright {
        out.ifd0.push(tiffw::ascii(tiffw::COPYRIGHT, c));
    }
    let with_contact = matches!(options.include, MetadataInclude::CopyrightAndContact | MetadataInclude::All);
    if with_contact {
        if !creators.is_empty() {
            out.ifd0.push(tiffw::ascii(tiffw::ARTIST, &creators.join("; ")));
        }
        x.creator = creators;
        x.contact = side.contact.clone();
    }
    if options.include == MetadataInclude::All {
        out.ifd0.extend(dirs.ifd0.iter().filter(|t| IFD0_ALL.contains(&t.tag)).cloned());
        if let Some(d) = &side.description {
            out.ifd0.push(tiffw::ascii(tiffw::IMAGE_DESCRIPTION, d));
        }
        out.exif = dirs.exif.iter().filter(|t| EXIF_ALL.contains(&t.tag)).cloned().collect();
        if !options.remove_location {
            out.gps = dirs.gps.clone();
            x.location = side.location.clone();
        }
        x.title = side.title.clone();
        x.description = side.description.clone();
        x.headline = side.headline.clone();
        x.rating = side.rating.filter(|r| (1..=5).contains(r));
        x.label = side.label.clone().filter(|l| l != "Pick");
        if options.include_keywords {
            let cull: Vec<&str> = CullTag::ALL.iter().map(|t| t.as_str()).collect();
            x.hierarchical = side.hierarchical_subjects.iter().filter(|k| !is_sieve_keyword(k)).cloned().collect();
            x.subjects = side
                .subjects
                .iter()
                .filter(|k| {
                    let ours = format!("{}|{k}", packet::KEYWORD_ROOT);
                    !is_sieve_keyword(k)
                        && !cull.contains(&String::as_str(k))
                        && !side.hierarchical_subjects.contains(&ours)
                })
                .cloned()
                .collect();
        }
    }
    out.xmp = Some(x.render());
    out
}

#[derive(Debug, Default)]
struct XmpOut {
    creator: Vec<String>,
    rights: Option<String>,
    title: Option<String>,
    description: Option<String>,
    headline: Option<String>,
    rating: Option<i32>,
    label: Option<String>,
    subjects: Vec<String>,
    hierarchical: Vec<String>,
    contact: Vec<(String, String)>,
    location: Vec<(&'static str, String, String)>,
}

fn prefix(ns: &str) -> &'static str {
    match ns {
        NS_PHOTOSHOP => "photoshop",
        NS_IPTC_CORE => "Iptc4xmpCore",
        NS_EXIF => "exif",
        _ => "x",
    }
}

fn is_xml_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl XmpOut {
    fn render(&self) -> String {
        let mut attrs = vec![format!("xmp:CreatorTool=\"{SOFTWARE}\"")];
        if let Some(r) = self.rating {
            attrs.push(format!("xmp:Rating=\"{r}\""));
        }
        if let Some(l) = &self.label {
            attrs.push(format!("xmp:Label=\"{}\"", escape(l)));
        }
        if let Some(h) = &self.headline {
            attrs.push(format!("photoshop:Headline=\"{}\"", escape(h)));
        }
        for (ns, local, v) in &self.location {
            if is_xml_name(local) {
                attrs.push(format!("{}:{local}=\"{}\"", prefix(ns), escape(v)));
            }
        }
        let mut body = String::new();
        let list = |body: &mut String, name: &str, kind: &str, items: &[String], lang: bool| {
            if items.is_empty() {
                return;
            }
            body.push_str(&format!("   <{name}>\n    <rdf:{kind}>\n"));
            for i in items {
                let l = if lang { " xml:lang=\"x-default\"" } else { "" };
                body.push_str(&format!("     <rdf:li{l}>{}</rdf:li>\n", escape(i.as_str())));
            }
            body.push_str(&format!("    </rdf:{kind}>\n   </{name}>\n"));
        };
        list(&mut body, "dc:creator", "Seq", &self.creator, false);
        list(&mut body, "dc:rights", "Alt", self.rights.as_slice(), true);
        list(&mut body, "dc:title", "Alt", self.title.as_slice(), true);
        list(&mut body, "dc:description", "Alt", self.description.as_slice(), true);
        list(&mut body, "dc:subject", "Bag", &self.subjects, false);
        list(&mut body, "lr:hierarchicalSubject", "Bag", &self.hierarchical, false);
        let contact: Vec<&(String, String)> = self.contact.iter().filter(|(k, _)| is_xml_name(k)).collect();
        if !contact.is_empty() {
            body.push_str("   <Iptc4xmpCore:CreatorContactInfo rdf:parseType=\"Resource\">\n");
            for (k, v) in contact {
                body.push_str(&format!("    <Iptc4xmpCore:{k}>{}</Iptc4xmpCore:{k}>\n", escape(v.as_str())));
            }
            body.push_str("   </Iptc4xmpCore:CreatorContactInfo>\n");
        }
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"{SOFTWARE}\">\n \
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    \
xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\"\n    \
xmlns:photoshop=\"{NS_PHOTOSHOP}\"\n    \
xmlns:Iptc4xmpCore=\"{NS_IPTC_CORE}\"\n    \
xmlns:exif=\"{NS_EXIF}\"\n    \
{}>\n{body}  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
            attrs.join("\n    ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"
    xmp:Rating="4" xmp:Label="Pick" crs:Exposure2012="+0.50" photoshop:City="Lisbon"
    exif:GPSLatitude="38,42.5N">
   <dc:creator><rdf:Seq><rdf:li>Jane Doe</rdf:li></rdf:Seq></dc:creator>
   <dc:rights><rdf:Alt><rdf:li xml:lang="x-default">(c) Jane Doe</rdf:li></rdf:Alt></dc:rights>
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">First dance</rdf:li></rdf:Alt></dc:title>
   <dc:subject><rdf:Bag><rdf:li>wedding</rdf:li><rdf:li>blink</rdf:li></rdf:Bag></dc:subject>
   <lr:hierarchicalSubject><rdf:Bag><rdf:li>Events|wedding</rdf:li><rdf:li>Sieve|blink</rdf:li></rdf:Bag></lr:hierarchicalSubject>
   <Iptc4xmpCore:CreatorContactInfo rdf:parseType="Resource">
    <Iptc4xmpCore:CiEmailWork>jane@example.com</Iptc4xmpCore:CiEmailWork>
    <Iptc4xmpCore:CiUrlWork>https://jane.example</Iptc4xmpCore:CiUrlWork>
   </Iptc4xmpCore:CreatorContactInfo>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    fn dirs() -> ExifDirs {
        ExifDirs {
            ifd0: vec![
                tiffw::ascii(tiffw::MAKE, "SONY"),
                tiffw::ascii(tiffw::MODEL, "ILCE-7M4"),
                tiffw::ascii(tiffw::COPYRIGHT, "camera copyright"),
                tiffw::short(0x0112, &[6]),
                tiffw::long(0x014A, &[1234]),
            ],
            exif: vec![tiffw::rational(0x829A, 1, 250), tiffw::undefined(0x927C, b"makernote")],
            gps: vec![tiffw::ascii(1, "N")],
        }
    }

    fn opts(include: MetadataInclude) -> MetadataOptions {
        MetadataOptions { include, remove_location: false, include_keywords: true, copyright: None, creator: None }
    }

    #[test]
    fn sidecar_export_values() {
        let v = packet::export_values(SIDECAR).unwrap();
        assert_eq!(v.creator, vec!["Jane Doe"]);
        assert_eq!(v.rights.as_deref(), Some("(c) Jane Doe"));
        assert_eq!(v.title.as_deref(), Some("First dance"));
        assert_eq!(v.rating, Some(4));
        assert_eq!(
            v.contact,
            vec![
                ("CiEmailWork".into(), "jane@example.com".into()),
                ("CiUrlWork".into(), "https://jane.example".into())
            ]
        );
        assert!(v.location.iter().any(|(_, k, v)| k == "City" && v == "Lisbon"));
        assert!(v.location.iter().any(|(_, k, _)| k == "GPSLatitude"));
    }

    #[test]
    fn all_copies_whitelisted_exif_and_filters_keywords() {
        let side = packet::export_values(SIDECAR).unwrap();
        let m = build(&dirs(), &side, &opts(MetadataInclude::All));
        let tags: Vec<u16> = m.ifd0.iter().map(|t| t.tag).collect();
        assert!(tags.contains(&tiffw::MAKE) && tags.contains(&tiffw::ARTIST));
        assert!(!tags.contains(&0x0112), "orientation never copied");
        assert!(!tags.contains(&0x014A), "RAW structure never copied");
        assert_eq!(ascii_value(&m.ifd0, tiffw::COPYRIGHT).as_deref(), Some("(c) Jane Doe"), "sidecar wins over EXIF");
        assert_eq!(m.exif.len(), 1, "maker note dropped");
        assert_eq!(m.gps.len(), 1);
        let x = m.xmp.unwrap();
        assert!(x.contains("Events|wedding") && x.contains(">wedding<"));
        assert!(!x.contains("Sieve|") && !x.contains(">blink<"), "{x}");
        assert!(!x.contains("crs:") && !x.contains("Exposure2012"));
        assert!(!x.contains("xmp:Label"), "the pick label is Sieve's");
        assert!(x.contains("xmp:Rating=\"4\"") && x.contains("photoshop:City=\"Lisbon\""));
        assert!(x.contains("CiEmailWork") && x.contains("First dance"));
        // The packet parses back.
        let back = packet::export_values(&x).unwrap();
        assert_eq!(back.creator, vec!["Jane Doe"]);

        let mut o = opts(MetadataInclude::All);
        o.remove_location = true;
        o.include_keywords = false;
        let m = build(&dirs(), &side, &o);
        assert!(m.gps.is_empty());
        let x = m.xmp.unwrap();
        assert!(!x.contains("Lisbon") && !x.contains("GPS") && !x.contains("wedding"));
    }

    #[test]
    fn copyright_modes_and_overrides() {
        let side = packet::export_values(SIDECAR).unwrap();
        let m = build(&dirs(), &side, &opts(MetadataInclude::CopyrightOnly));
        assert_eq!(m.ifd0.len(), 1);
        assert!(m.exif.is_empty() && m.gps.is_empty());
        let x = m.xmp.unwrap();
        assert!(x.contains("(c) Jane Doe") && !x.contains("Jane Doe</rdf:li>\n    </rdf:Seq>"));
        assert!(!x.contains("dc:creator") && !x.contains("CiEmailWork"));

        let mut o = opts(MetadataInclude::CopyrightAndContact);
        o.copyright = Some("(c) Studio".into());
        o.creator = Some("Studio".into());
        let m = build(&dirs(), &side, &o);
        assert_eq!(ascii_value(&m.ifd0, tiffw::COPYRIGHT).as_deref(), Some("(c) Studio"));
        assert_eq!(ascii_value(&m.ifd0, tiffw::ARTIST).as_deref(), Some("Studio"));
        let x = m.xmp.unwrap();
        assert!(x.contains("CiEmailWork") && !x.contains("First dance") && !x.contains("Lisbon"));

        // No sidecar: EXIF copyright is the fallback.
        let m = build(&dirs(), &ExportValues::default(), &opts(MetadataInclude::CopyrightOnly));
        assert_eq!(ascii_value(&m.ifd0, tiffw::COPYRIGHT).as_deref(), Some("camera copyright"));

        // Technical EXIF: orientation 1 always.
        let d = ExportMetadata::default().exif_dirs(10, 10, 72, ExportColorSpace::Srgb, ExifContainer::Other);
        assert!(d.ifd0.iter().any(|t| t.tag == tiffw::ORIENTATION && t.data == [1, 0]));
        assert!(d.exif.is_empty());
    }
}
