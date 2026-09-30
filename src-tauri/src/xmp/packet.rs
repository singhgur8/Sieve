//! XMP packet parsing and surgical merging.
//!
//! Approach: `quick-xml` is used only as a validating tokenizer (it handles comments, PIs,
//! CDATA, DOCTYPE and checks end-tag names). From its byte positions we build a small
//! index of the RDF structure we care about (every `rdf:Description`, its attribute-form
//! and element-form properties, `rdf:Bag`/`rdf:Seq` items) with exact byte spans, resolving
//! namespace prefixes ourselves. A merge is then a list of byte-range splices applied to
//! the original text, so everything we do not own (other namespaces, `crs:` develop
//! settings, other keywords, `x:xmptk`, whitespace, comments, `xpacket` wrappers, BOM)
//! stays byte-for-byte identical.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event;
use quick_xml::Reader;

use super::crs::{self, LookChange, PropertyEdit, SeqEdit};
use crate::ipc::types::{DevelopWarning, ImageFormat, LookSettings, ParametricAdjustments, ProfileSettings};

pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
pub const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
pub const NS_LR: &str = "http://ns.adobe.com/lightroom/1.0/";
const NS_XMLNS_PREFIX: &str = "xmlns";

/// Hierarchical keyword root owned by Sieve.
pub const KEYWORD_ROOT: &str = "Sieve";
/// `xmp:Label` values Sieve writes (and may therefore remove). Lightroom's default label set.
pub const OWNED_LABELS: [&str; 6] = ["Pick", "Red", "Yellow", "Green", "Blue", "Purple"];

const BOM: &str = "\u{feff}";

/// Minimal Lightroom-style packet used when no sidecar exists yet.
pub const NEW_PACKET: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Sieve\">\n \
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n  \
</rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketError(pub String);

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid XMP: {}", self.0)
    }
}

impl std::error::Error for PacketError {}

type Result<T> = std::result::Result<T, PacketError>;

fn err(msg: impl Into<String>) -> PacketError {
    PacketError(msg.into())
}

/// Values Sieve reads from a sidecar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidecarValues {
    /// `xmp:Rating` rounded (`None` if absent or unparsable).
    pub rating: Option<i32>,
    pub label: Option<String>,
    pub hierarchical_subjects: Vec<String>,
    pub subjects: Vec<String>,
    /// Importable develop settings (`crs:` PV2012+, see `crs::decode`).
    pub develop: Option<ParametricAdjustments>,
    /// Why the develop settings could not be read (malformed value), if so.
    pub develop_error: Option<String>,
    /// `crs:` features preserved but not rendered (`crs::unsupported_warnings`), stored as
    /// `RawImageEntry.developWarnings` by the read path (v9).
    pub warnings: Vec<DevelopWarning>,
}

/// The XMP-mapped state Sieve wants in the sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    /// `crs:` / `sieve:` develop properties to set or remove (empty = leave untouched).
    pub develop: Vec<PropertyEdit>,
    /// -1 (reject) ..= 5.
    pub rating: i32,
    /// One of [`OWNED_LABELS`], or `None` to remove an owned label.
    pub label: Option<&'static str>,
    /// Visible tags (snake_case), written as `Sieve|<tag>` and `<tag>`.
    pub tags: Vec<String>,
    /// `xmp:MetadataDate` value (ISO 8601).
    pub metadata_date: String,
    /// `rdf:Seq` develop properties (point curves, `crs::encode_curves`) to create/replace
    /// (`items: None` removes). Empty = leave untouched.
    pub seqs: Vec<SeqEdit>,
    /// Camera profile + look to write (`crs::encode_profile` rules, `<crs:Look>` struct);
    /// `None` = leave the sidecar's profile as it is.
    pub profile: Option<ProfileWrite>,
    /// Format of the image the sidecar belongs to (`None` = RAW). Its develop defaults
    /// (`ParametricAdjustments::defaults_for`) are what absent `crs:` properties read as, so a
    /// develop property (or point curve) the sidecar does not carry is only added when its
    /// value differs from that default ([`omitted_defaults`]); present ones are always updated.
    pub format: Option<ImageFormat>,
}

/// Profile part of a develop write.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileWrite {
    pub settings: ProfileSettings,
    /// The installed look profile file (XMP text) for `settings.look`, copied into a new
    /// `<crs:Look>` struct when the look changes (`None`: Name/Amount/UUID only).
    pub look_source: Option<std::sync::Arc<str>>,
}

impl Desired {
    /// Culling-only write (no develop settings).
    pub fn culling(rating: i32, label: Option<&'static str>, tags: Vec<String>, metadata_date: String) -> Self {
        Desired {
            develop: Vec::new(),
            rating,
            label,
            tags,
            metadata_date,
            seqs: Vec::new(),
            profile: None,
            format: None,
        }
    }
}

/// [`parse`] for an image of `format`: develop properties missing from the packet read as
/// that format's defaults (`ParametricAdjustments::defaults_for`; non-RAW: no sharpening /
/// colour NR / profile unless recorded).
pub fn parse_for(src: &str, format: ImageFormat) -> Result<SidecarValues> {
    let mut values = parse(src)?;
    if let Some(adj) = values.develop.as_mut() {
        let body = src.strip_prefix(BOM).unwrap_or(src);
        let doc = Doc::parse(body)?;
        crs::overlay_format_defaults(&ScopeSource { doc: &doc, scope: Scope::Top }, adj, format);
    }
    Ok(values)
}

/// Reads the Sieve-relevant values of a packet.
pub fn parse(src: &str) -> Result<SidecarValues> {
    let body = src.strip_prefix(BOM).unwrap_or(src);
    let doc = Doc::parse(body)?;
    let scalar = |local: &str| -> Option<String> {
        doc.scalars(NS_XMP, local).into_iter().next().map(|s| s.value.trim().to_owned())
    };
    let rating =
        scalar("Rating").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v.round() as i32);
    let label = scalar("Label").filter(|v| !v.is_empty());
    let list = |ns: &str, local: &str| -> Vec<String> {
        doc.list(ns, local).map(|p| p.items.iter().map(|i| i.value.clone()).collect()).unwrap_or_default()
    };
    let source = ScopeSource { doc: &doc, scope: Scope::Top };
    let (develop, develop_error) = match crs::decode_source(&source) {
        Ok(d) => (d, None),
        Err(e) => (None, Some(e)),
    };
    let warnings = crs::unsupported_warnings(&source);
    Ok(SidecarValues {
        rating,
        label,
        hierarchical_subjects: list(NS_LR, "hierarchicalSubject"),
        subjects: list(NS_DC, "subject"),
        develop,
        develop_error,
        warnings,
    })
}

pub const NS_PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
pub const NS_IPTC_CORE: &str = "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/";
pub const NS_EXIF: &str = "http://ns.adobe.com/exif/1.0/";

/// Descriptive metadata of a sidecar that exports may copy (never `crs:` settings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportValues {
    /// `dc:creator` (ordered).
    pub creator: Vec<String>,
    /// `dc:rights` (x-default / first alternative).
    pub rights: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// `photoshop:Headline`.
    pub headline: Option<String>,
    pub rating: Option<i32>,
    pub label: Option<String>,
    pub subjects: Vec<String>,
    pub hierarchical_subjects: Vec<String>,
    /// `Iptc4xmpCore:CreatorContactInfo` fields `(local name, value)`, e.g. `("CiEmailWork", ..)`.
    pub contact: Vec<(String, String)>,
    /// Location fields `(namespace uri, local name, value)`: `photoshop:City/State/Country`,
    /// `Iptc4xmpCore:Location/CountryCode`, `exif:GPS*`.
    pub location: Vec<(&'static str, String, String)>,
}

/// Reads the fields an export may copy from a sidecar packet.
pub fn export_values(src: &str) -> Result<ExportValues> {
    let body = src.strip_prefix(BOM).unwrap_or(src);
    let doc = Doc::parse(body)?;
    let scalar = |ns: &str, local: &str| -> Option<String> {
        doc.scalars(ns, local).into_iter().next().map(|s| s.value.trim().to_owned()).filter(|v| !v.is_empty())
    };
    let list = |ns: &str, local: &str| -> Vec<String> {
        doc.list(ns, local)
            .filter(|p| p.container.is_some())
            .map(|p| p.items.iter().map(|i| i.value.trim().to_owned()).filter(|v| !v.is_empty()).collect())
            .unwrap_or_default()
    };
    // Lang-alt / seq values, or a plain scalar written by a lenient tool.
    let first = |ns: &str, local: &str| list(ns, local).into_iter().next().or_else(|| scalar(ns, local));
    let mut creator = list(NS_DC, "creator");
    if creator.is_empty() {
        creator.extend(scalar(NS_DC, "creator"));
    }
    let mut contact = Vec::new();
    for a in doc.top_attrs() {
        if a.uri == NS_IPTC_CORE && a.local.starts_with("Ci") && !a.attr.value.trim().is_empty() {
            contact.push((a.local.clone(), a.attr.value.trim().to_owned()));
        }
    }
    if let Some(p) = doc.top_elems().find(|p| p.uri == NS_IPTC_CORE && p.local == "CreatorContactInfo") {
        for a in &p.elem.attrs {
            let (_, local) = split_qname(&a.qname);
            if local.starts_with("Ci") && !a.value.trim().is_empty() {
                contact.push((local.to_owned(), a.value.trim().to_owned()));
            }
        }
        if !p.elem.empty {
            contact.extend(child_texts(&body[p.elem.open_end..p.elem.close_start]));
        }
    }
    let mut location = Vec::new();
    for (ns, local) in [
        (NS_PHOTOSHOP, "City"),
        (NS_PHOTOSHOP, "State"),
        (NS_PHOTOSHOP, "Country"),
        (NS_IPTC_CORE, "Location"),
        (NS_IPTC_CORE, "CountryCode"),
    ] {
        if let Some(v) = scalar(ns, local) {
            location.push((ns, local.to_owned(), v));
        }
    }
    let mut gps: Vec<(&'static str, String, String)> = doc
        .top_attrs()
        .filter(|a| a.uri == NS_EXIF && a.local.starts_with("GPS"))
        .map(|a| (NS_EXIF, a.local.clone(), a.attr.value.trim().to_owned()))
        .chain(
            doc.top_elems().filter(|p| p.uri == NS_EXIF && p.local.starts_with("GPS") && p.container.is_none()).map(
                |p| (NS_EXIF, p.local.clone(), p.items.first().map_or(String::new(), |i| i.value.trim().to_owned())),
            ),
        )
        .filter(|(_, _, v)| !v.is_empty())
        .collect();
    location.append(&mut gps);
    Ok(ExportValues {
        creator,
        rights: first(NS_DC, "rights"),
        title: first(NS_DC, "title"),
        description: first(NS_DC, "description"),
        headline: scalar(NS_PHOTOSHOP, "Headline"),
        rating: scalar(NS_XMP, "Rating")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .map(|v| v.round() as i32),
        label: scalar(NS_XMP, "Label"),
        subjects: list(NS_DC, "subject"),
        hierarchical_subjects: list(NS_LR, "hierarchicalSubject"),
        contact,
        location,
    })
}

/// `(local name, text)` of the leaf elements in an XML fragment (struct fields).
fn child_texts(fragment: &str) -> Vec<(String, String)> {
    let mut reader = Reader::from_str(fragment);
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let q = e.name().into_inner().to_owned();
                current = Some(split_qname(&q).1.to_owned());
            }
            Ok(Event::Text(t)) => {
                if let Some(name) = &current {
                    let raw = t[..].to_owned();
                    let v = text_value(&raw);
                    if name.starts_with("Ci") && !v.is_empty() {
                        out.push((name.clone(), v));
                    }
                }
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Properties of one scope of a parsed packet (top level, or a nested struct description)
/// as a [`crs::CrsSource`].
#[derive(Clone, Copy)]
pub struct ScopeSource<'a> {
    doc: &'a Doc,
    scope: Scope,
}

impl crs::CrsSource for ScopeSource<'_> {
    fn scalar(&self, ns: &str, name: &str) -> Option<String> {
        self.doc.scalars_in(self.scope, ns, name).into_iter().next().map(|s| s.value.to_owned())
    }

    fn seq(&self, ns: &str, name: &str) -> Option<Vec<String>> {
        let p = self.doc.list_in(self.scope, ns, name)?;
        p.container.as_ref()?;
        Some(p.items.iter().map(|i| i.value.clone()).collect())
    }

    fn has(&self, ns: &str, name: &str) -> bool {
        self.doc.attrs_in(self.scope).any(|a| a.uri == ns && a.local == name)
            || self.doc.elems_in(self.scope).any(|p| p.uri == ns && p.local == name)
    }

    fn look(&self) -> Option<LookSettings> {
        look_struct(self.doc, self.scope).map(|l| l.settings)
    }
}

impl ScopeSource<'_> {
    /// Every scalar property in namespace `ns` of this scope: `(local name, value)`.
    pub fn scalars(&self, ns: &str) -> Vec<(String, String)> {
        let mut out: Vec<(usize, String, String)> = self
            .doc
            .attrs_in(self.scope)
            .filter(|a| a.uri == ns)
            .map(|a| (a.attr.ws_start, a.local.clone(), a.attr.value.clone()))
            .collect();
        out.extend(
            self.doc
                .elems_in(self.scope)
                .filter(|p| p.uri == ns && p.container.is_none() && p.struct_desc.is_none())
                .map(|p| (p.elem.start, p.local.clone(), p.items.first().map(|i| i.value.clone()).unwrap_or_default())),
        );
        out.sort_by_key(|(pos, _, _)| *pos);
        out.into_iter().map(|(_, k, v)| (k, v)).collect()
    }

    /// Names of the list (`rdf:Seq` / `Bag` / `Alt`) properties in namespace `ns`.
    pub fn lists(&self, ns: &str) -> Vec<String> {
        self.doc
            .elems_in(self.scope)
            .filter(|p| p.uri == ns && p.container.is_some())
            .map(|p| p.local.clone())
            .collect()
    }

    /// First item of an `rdf:Alt` (e.g. `crs:Name` / `crs:Group` in look files), or the
    /// scalar value.
    pub fn text(&self, ns: &str, name: &str) -> Option<String> {
        use crs::CrsSource;
        self.seq(ns, name).and_then(|v| v.into_iter().next()).or_else(|| self.scalar(ns, name))
    }
}

/// A `<crs:Look>` struct (sidecar) as read by [`Packet::look`].
pub struct LookStruct<'a> {
    pub settings: LookSettings,
    pub supports_amount: Option<bool>,
    pub supports_monochrome: Option<bool>,
    pub supports_output_referred: Option<bool>,
    pub group: Option<String>,
    pub copyright: Option<String>,
    /// `crs:Parameters`: the look's own develop settings (and `crs:LookTable` /
    /// `crs:RGBTable` references to top-level `crs:Table_<md5>` values).
    pub parameters: Option<ScopeSource<'a>>,
}

fn parse_bool_opt(v: Option<String>) -> Option<bool> {
    match v?.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Reads `crs:Look` in `scope`. `None` if absent or its UUID is not 32 hex digits.
fn look_struct(doc: &Doc, scope: Scope) -> Option<LookStruct<'_>> {
    let (_, d) = doc.struct_in(scope, crs::CRS_NS, "Look")?;
    let s = ScopeSource { doc, scope: Scope::Desc(d) };
    let uuid = s.text(crs::CRS_NS, "UUID")?.trim().to_ascii_uppercase();
    if !LookSettings::is_valid_uuid(&uuid) {
        return None;
    }
    let name: String =
        s.text(crs::CRS_NS, "Name").unwrap_or_default().trim().chars().take(ProfileSettings::MAX_NAME).collect();
    let amount = s
        .text(crs::CRS_NS, "Amount")
        .and_then(|v| v.trim().trim_start_matches('+').parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map_or(1.0, |v| v.clamp(0.0, 2.0));
    let parameters = doc
        .struct_in(Scope::Desc(d), crs::CRS_NS, "Parameters")
        .map(|(_, pd)| ScopeSource { doc, scope: Scope::Desc(pd) });
    use crs::CrsSource;
    Some(LookStruct {
        settings: LookSettings { name, uuid, amount },
        supports_amount: parse_bool_opt(s.scalar(crs::CRS_NS, "SupportsAmount")),
        supports_monochrome: parse_bool_opt(s.scalar(crs::CRS_NS, "SupportsMonochrome")),
        supports_output_referred: parse_bool_opt(s.scalar(crs::CRS_NS, "SupportsOutputReferred")),
        group: s.text(crs::CRS_NS, "Group"),
        copyright: s.text(crs::CRS_NS, "Copyright"),
        parameters,
    })
}

/// A parsed packet for read-only structured access (look profiles, sidecar looks, tables).
pub struct Packet {
    doc: Doc,
}

impl Packet {
    pub fn parse(src: &str) -> Result<Packet> {
        let body = src.strip_prefix(BOM).unwrap_or(src);
        Ok(Packet { doc: Doc::parse(body)? })
    }

    /// Top-level properties (every top-level `rdf:Description`).
    pub fn top(&self) -> ScopeSource<'_> {
        ScopeSource { doc: &self.doc, scope: Scope::Top }
    }

    /// The top-level `<crs:Look>` struct, if present and valid.
    pub fn look(&self) -> Option<LookStruct<'_>> {
        look_struct(&self.doc, Scope::Top)
    }
}

/// Raw source text of every top-level property as `(namespace, local name, text)`:
/// attributes as `qname="value"`, elements as their full span (for round-trip checks:
/// what Sieve does not own must come back byte-for-byte).
pub fn top_level_properties(src: &str) -> Result<Vec<(String, String, String)>> {
    let body = src.strip_prefix(BOM).unwrap_or(src);
    let doc = Doc::parse(body)?;
    let mut out: Vec<(usize, String, String, String)> = doc
        .top_attrs()
        .map(|a| (a.attr.name_start, a.uri.clone(), a.local.clone(), body[a.attr.name_start..a.attr.end].to_owned()))
        .collect();
    out.extend(
        doc.top_elems()
            .map(|p| (p.elem.start, p.uri.clone(), p.local.clone(), body[p.elem.start..p.elem.end].to_owned())),
    );
    out.sort_by_key(|(pos, ..)| *pos);
    Ok(out.into_iter().map(|(_, u, l, t)| (u, l, t)).collect())
}

/// Look-file properties that describe the look rather than being develop parameters.
const LOOK_META: &[&str] = &[
    "PresetType",
    "Cluster",
    "UUID",
    "SupportsAmount",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "HasSettings",
    "Name",
    "ShortName",
    "SortName",
    "Group",
    "Description",
];

/// Renders a `<crs:Look>` struct (Lightroom's sidecar layout, one space per level) for
/// `look`. With the installed look file `source`, its descriptive fields and develop
/// parameters are copied (never its `crs:Table_*`: Lightroom resolves installed looks by
/// UUID); without it only Name/Amount/UUID are written.
fn render_look(look: &LookSettings, source: Option<&str>, qname: &str, rdf: &str, crs_prefix: &str) -> Vec<String> {
    let c = |local: &str| qualify(crs_prefix, local);
    let desc = qualify(rdf, "Description");
    let parsed = source.and_then(|s| Packet::parse(s).ok());
    let top = parsed.as_ref().map(Packet::top);
    let flag = |name: &str| -> Option<String> {
        use crs::CrsSource;
        parse_bool_opt(top.as_ref()?.scalar(crs::CRS_NS, name)).map(|b| if b { "true".into() } else { "false".into() })
    };
    let mut attrs = vec![
        (c("Name"), look.name.clone()),
        (c("Amount"), crs::format_num(look.amount, crs::NumFormat::Plain)),
        (c("UUID"), look.uuid.clone()),
    ];
    for f in ["SupportsAmount", "SupportsMonochrome", "SupportsOutputReferred"] {
        if let Some(v) = flag(f) {
            attrs.push((c(f), v));
        }
    }
    if let Some(v) = top.as_ref().and_then(|t| t.text(crs::CRS_NS, "Copyright")).filter(|v| !v.is_empty()) {
        attrs.push((c("Copyright"), v));
    }
    let mut lines = vec![format!("<{qname}>")];
    let attr_lines = |indent: &str, list: &[(String, String)]| -> Vec<String> {
        list.iter().map(|(k, v)| format!("{indent}{k}=\"{}\"", escape(v.as_str()))).collect()
    };
    let open = |indent: &str, list: &[(String, String)]| -> Vec<String> {
        let mut out = vec![format!("{indent}<{desc}")];
        let mut al = attr_lines(&format!("{indent} "), list);
        if let Some(last) = al.last_mut() {
            last.push('>');
        } else {
            out[0].push('>');
        }
        out.extend(al);
        out
    };
    lines.extend(open(" ", &attrs));
    if let Some(t) = &top {
        if let Some(group) = t.text(crs::CRS_NS, "Group").filter(|g| !g.is_empty()) {
            let alt = qualify(rdf, "Alt");
            let li = qualify(rdf, "li");
            lines.push(format!(" <{}>", c("Group")));
            lines.push(format!("  <{alt}>"));
            lines.push(format!("   <{li} xml:lang=\"x-default\">{}</{li}>", escape(group.as_str())));
            lines.push(format!("  </{alt}>"));
            lines.push(format!(" </{}>", c("Group")));
        }
        let params: Vec<(String, String)> = t
            .scalars(crs::CRS_NS)
            .into_iter()
            .filter(|(k, _)| !LOOK_META.contains(&k.as_str()) && !k.starts_with("Table_"))
            .map(|(k, v)| (c(&k), v))
            .collect();
        let seqs: Vec<(String, Vec<String>)> = t
            .lists(crs::CRS_NS)
            .into_iter()
            .filter(|k| !LOOK_META.contains(&k.as_str()))
            .filter_map(|k| {
                use crs::CrsSource;
                let items = t.seq(crs::CRS_NS, &k)?;
                Some((c(&k), items))
            })
            .collect();
        if !params.is_empty() || !seqs.is_empty() {
            lines.push(format!(" <{}>", c("Parameters")));
            lines.extend(open("  ", &params));
            for (name, items) in &seqs {
                for l in render_list(name, rdf, "Seq", items) {
                    lines.push(format!("  {l}"));
                }
            }
            lines.push(format!("  </{desc}>"));
            lines.push(format!(" </{}>", c("Parameters")));
        }
    }
    lines.push(format!(" </{desc}>"));
    lines.push(format!("</{qname}>"));
    lines
}

/// Applies `want` to `existing` (or to a new minimal packet), touching only the fields
/// Sieve owns.
pub fn merge(existing: Option<&str>, want: &Desired) -> Result<String> {
    let src = existing.unwrap_or(NEW_PACKET);
    let (bom, body) = match src.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => ("", src),
    };
    let mut doc = Doc::parse(body)?;
    let mut owned_body: String;
    if doc.descs.is_empty() {
        // A packet without any rdf:Description: add an empty one inside rdf:RDF.
        let rdf = doc.rdf.as_ref().ok_or_else(|| err("no rdf:RDF element"))?;
        if rdf.empty {
            return Err(err("empty rdf:RDF element"));
        }
        let prefix = prefix_of(&rdf.qname);
        let q = qualify(prefix, "Description");
        let insert = format!(" <{q} {}=\"\">\n </{q}>\n", qualify(prefix, "about"));
        owned_body = String::with_capacity(body.len() + insert.len());
        owned_body.push_str(&body[..rdf.close_start]);
        owned_body.push_str(&insert);
        owned_body.push_str(&body[rdf.close_start..]);
        doc = Doc::parse(&owned_body)?;
        let merged = merge_doc(&owned_body, &doc, want)?;
        return Ok(format!("{bom}{merged}"));
    }
    owned_body = merge_doc(body, &doc, want)?;
    Ok(format!("{bom}{owned_body}"))
}

fn merge_doc(src: &str, doc: &Doc, want: &Desired) -> Result<String> {
    let mut ed = Editor::new(src, doc);
    ed.set_scalar(NS_XMP, "Rating", &want.rating.to_string());
    match want.label {
        Some(label) => ed.set_scalar(NS_XMP, "Label", label),
        None => ed.remove_scalar_if(NS_XMP, "Label", is_owned_label),
    }
    ed.set_scalar(NS_XMP, "MetadataDate", &want.metadata_date);
    let top = ScopeSource { doc, scope: Scope::Top };
    let mut develop_edits: Vec<PropertyEdit> = want.develop.clone();
    if let Some(pw) = &want.profile {
        let current = crs::current_profile(&top);
        let adj = ParametricAdjustments { profile: pw.settings.clone(), ..Default::default() };
        develop_edits.extend(crs::encode_profile(&adj, current.as_ref()));
        match crs::look_change(&pw.settings, current.as_ref(), crs::CrsSource::has(&top, crs::CRS_NS, "Look")) {
            LookChange::Keep => {}
            LookChange::Remove => ed.put_element(crs::CRS_NS, "Look", &|_, _| Vec::new(), false),
            LookChange::Amount(amount) => {
                let nested = doc
                    .struct_in(Scope::Top, crs::CRS_NS, "Look")
                    .filter(|(p, _)| doc.elem_props[*p].elem.has_children)
                    .map(|(_, d)| d)
                    .filter(|d| split_qname(&doc.descs[*d].elem.qname).1 == "Description");
                let value = crs::format_num(amount, crs::NumFormat::Plain);
                match nested {
                    Some(d) => ed.set_scalar_in(Scope::Desc(d), crs::CRS_NS, "Amount", &value),
                    None => put_look(&mut ed, pw),
                }
            }
            LookChange::Replace => put_look(&mut ed, pw),
        }
    }
    let skip = omitted_defaults(&top, want);
    for edit in &develop_edits {
        if edit.ns == crs::CRS_NS && skip.contains(edit.name.as_str()) {
            continue;
        }
        // Never downgrade a newer process version (Lightroom would re-render with the old one).
        if edit.ns == crs::CRS_NS && edit.name == "ProcessVersion" {
            let current = crs::CrsSource::scalar(&top, crs::CRS_NS, "ProcessVersion");
            let newer = |v: &str| v.trim().parse::<f32>().ok();
            if let (Some(cur), Some(want_pv)) =
                (current.as_deref().and_then(newer), edit.value.as_deref().and_then(newer))
            {
                if cur >= want_pv {
                    continue;
                }
            }
        }
        match &edit.value {
            Some(v) => ed.set_scalar(edit.ns, &edit.name, v),
            None => ed.remove_scalar_if(edit.ns, &edit.name, |_| true),
        }
    }
    for seq in &want.seqs {
        if seq.ns == crs::CRS_NS && skip.contains(seq.name) {
            continue;
        }
        ed.set_seq(seq.ns, seq.name, seq.items.as_deref());
    }

    // Keywords: drop every Sieve|* item, then add the wanted ones.
    let wanted_hier: Vec<String> = want.tags.iter().map(|t| format!("{KEYWORD_ROOT}|{t}")).collect();
    let existing_hier: Vec<String> = doc
        .list(NS_LR, "hierarchicalSubject")
        .map(|p| p.items.iter().map(|i| i.value.clone()).collect())
        .unwrap_or_default();
    let is_ours = |v: &str| v == KEYWORD_ROOT || v.starts_with(&format!("{KEYWORD_ROOT}|"));
    let removed_leaves: Vec<String> =
        existing_hier.iter().filter(|v| is_ours(v)).filter_map(|v| v.rsplit('|').next().map(str::to_owned)).collect();
    let kept_leaves: Vec<&str> =
        existing_hier.iter().filter(|v| !is_ours(v)).filter_map(|v| v.rsplit('|').next()).collect();
    let drop_leaf = |v: &str| {
        removed_leaves.iter().any(|l| l == v) && !want.tags.iter().any(|t| t == v) && !kept_leaves.contains(&v)
    };
    // Lightroom order: dc:subject, then lr:hierarchicalSubject.
    ed.update_list(NS_DC, "subject", &drop_leaf, &want.tags);
    ed.update_list(NS_LR, "hierarchicalSubject", &|v| is_ours(v) && !wanted_hier.iter().any(|w| w == v), &wanted_hier);
    ed.finish()
}

/// `crs:` properties Sieve writes with every develop write even at their default value
/// (Lightroom always carries them; `WhiteBalance` also marks the packet as holding develop
/// settings for `crs::decode_source`).
const ALWAYS_WRITTEN: [&str; 3] = ["ProcessVersion", "HasSettings", "WhiteBalance"];

/// Legacy split-toning properties: when any is present without `crs:ColorGradeBlending`,
/// blending reads as 100 instead of its default (`crs::decode_parity`).
const LEGACY_SPLIT_TONING: [&str; 4] = [
    "SplitToningShadowHue",
    "SplitToningShadowSaturation",
    "SplitToningHighlightHue",
    "SplitToningHighlightSaturation",
];

/// Names of `want`'s develop edits / curve seqs to skip: properties absent from the sidecar
/// whose wanted value equals the format default (reading the absent property gives the same
/// value, so the round trip stays lossless without adding default-valued keys Lightroom
/// omitted). Removals are never skipped.
fn omitted_defaults<'a>(top: &ScopeSource<'_>, want: &'a Desired) -> std::collections::HashSet<&'a str> {
    use crs::CrsSource;
    let mut skip = std::collections::HashSet::new();
    if want.develop.is_empty() && want.seqs.is_empty() {
        return skip;
    }
    let defaults = ParametricAdjustments::defaults_for(want.format.unwrap_or(ImageFormat::Arw));
    let mut default_values: HashMap<String, String> =
        crs::encode(&defaults).into_iter().filter_map(|e| Some((e.name, e.value?))).collect();
    let (default_seqs, curve_name) = crs::encode_curves(&defaults);
    if let Some(v) = curve_name.value {
        default_values.insert(curve_name.name, v);
    }
    let absent = |name: &str| !top.has(crs::CRS_NS, name);
    for e in &want.develop {
        let Some(v) = &e.value else { continue };
        if e.ns == crs::CRS_NS
            && !ALWAYS_WRITTEN.contains(&e.name.as_str())
            && default_values.get(&e.name) == Some(v)
            && absent(&e.name)
        {
            skip.insert(e.name.as_str());
        }
    }
    for s in &want.seqs {
        let default = default_seqs.iter().find(|d| d.name == s.name).and_then(|d| d.items.as_ref());
        if s.ns == crs::CRS_NS && s.items.is_some() && s.items.as_ref() == default && absent(s.name) {
            skip.insert(s.name);
        }
    }
    // Blending: absent reads as 100 (not the default) next to legacy split toning.
    let legacy_after = LEGACY_SPLIT_TONING
        .iter()
        .any(|n| !absent(n) || want.develop.iter().any(|e| e.name == *n && e.value.is_some() && !skip.contains(*n)));
    if legacy_after {
        skip.remove("ColorGradeBlending");
        let hundred = crs::format_num(100.0, crs::NumFormat::Plain);
        if let Some(e) = want.develop.iter().find(|e| e.ns == crs::CRS_NS && e.name == "ColorGradeBlending") {
            if e.value.as_deref() == Some(hundred.as_str()) && absent(&e.name) {
                skip.insert(e.name.as_str());
            }
        }
    }
    skip
}

fn put_look(ed: &mut Editor<'_>, pw: &ProfileWrite) {
    let Some(look) = pw.settings.look.clone() else {
        ed.put_element(crs::CRS_NS, "Look", &|_, _| Vec::new(), false);
        return;
    };
    let source = pw.look_source.clone();
    let d = ed.host_desc(crs::CRS_NS);
    let crs_prefix = ed.prefix_for(d, crs::CRS_NS);
    ed.put_element(crs::CRS_NS, "Look", &|q, rdf| render_look(&look, source.as_deref(), q, rdf, &crs_prefix), true);
}

pub fn is_owned_label(v: &str) -> bool {
    OWNED_LABELS.iter().any(|l| l.eq_ignore_ascii_case(v.trim()))
}

// ---------------------------------------------------------------------------
// Document index
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Attr {
    /// Start of the whitespace preceding the attribute.
    ws_start: usize,
    name_start: usize,
    end: usize,
    qname: String,
    /// Raw (escaped) value span, without quotes.
    value_start: usize,
    value_end: usize,
    value: String,
}

#[derive(Debug, Clone)]
struct Elem {
    qname: String,
    start: usize,
    /// End of the start tag (exclusive).
    open_end: usize,
    empty: bool,
    /// Start of the end tag (== `end` for empty elements).
    close_start: usize,
    end: usize,
    attrs: Vec<Attr>,
    has_children: bool,
}

#[derive(Debug, Clone)]
struct Desc {
    elem: Elem,
    /// Namespace bindings in scope inside the element (prefix -> uri), including its own.
    scope: HashMap<String, String>,
    /// Struct property (index into `elem_props`) this description is the value of;
    /// `None` for top-level descriptions (children of `rdf:RDF`).
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
struct AttrProp {
    /// Owning description.
    desc: usize,
    uri: String,
    local: String,
    attr: Attr,
}

#[derive(Debug, Clone)]
struct Item {
    elem: Elem,
    value: String,
}

#[derive(Debug, Clone)]
struct ElemProp {
    desc: usize,
    uri: String,
    local: String,
    elem: Elem,
    container: Option<Elem>,
    items: Vec<Item>,
    /// Struct value (`<prop><rdf:Description ...>` or `rdf:parseType="Resource"`).
    struct_desc: Option<usize>,
}

struct ScalarRef<'a> {
    value: &'a str,
}

#[derive(Debug, Default)]
struct Doc {
    rdf: Option<Elem>,
    descs: Vec<Desc>,
    attr_props: Vec<AttrProp>,
    elem_props: Vec<ElemProp>,
}

/// Which descriptions a query looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Every top-level `rdf:Description`.
    Top,
    /// One (nested) description.
    Desc(usize),
}

#[derive(Debug, Clone, Copy)]
enum Role {
    Other,
    Rdf,
    Desc(usize),
    Prop(usize),
    Container(usize),
    Item(usize, usize),
    /// `rdf:parseType="Resource"` struct property (prop, its description).
    ResProp(usize, usize),
}

struct Frame {
    qname: String,
    bindings: Vec<(String, String)>,
    role: Role,
    elem: Elem,
}

impl Doc {
    fn parse(src: &str) -> Result<Doc> {
        let mut reader = Reader::from_str(src);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = true;
        let mut doc = Doc::default();
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let event = reader.read_event().map_err(|e| err(format!("{e} at byte {}", reader.error_position())))?;
            let after = reader.buffer_position() as usize;
            match event {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let empty = matches!(event, Event::Empty(_));
                    let qname = e.name().into_inner().to_owned();
                    let attrs = scan_attrs(src, before + 1 + qname.len(), after)?;
                    let bindings: Vec<(String, String)> = attrs
                        .iter()
                        .filter_map(|a| {
                            if a.qname == NS_XMLNS_PREFIX {
                                Some((String::new(), a.value.clone()))
                            } else {
                                a.qname.strip_prefix("xmlns:").map(|p| (p.to_owned(), a.value.clone()))
                            }
                        })
                        .collect();
                    if let Some(parent) = stack.last_mut() {
                        parent.elem.has_children = true;
                    }
                    let lookup = |prefix: &str| -> Option<String> {
                        bindings.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.clone()).or_else(|| {
                            stack
                                .iter()
                                .rev()
                                .flat_map(|f| f.bindings.iter().rev())
                                .find(|(p, _)| p == prefix)
                                .map(|(_, u)| u.clone())
                        })
                    };
                    let (prefix, local) = split_qname(&qname);
                    let uri = lookup(prefix).unwrap_or_default();
                    let elem = Elem {
                        qname: qname.clone(),
                        start: before,
                        open_end: after,
                        empty,
                        close_start: after,
                        end: after,
                        attrs: attrs.clone(),
                        has_children: false,
                    };
                    let parent_role = stack.last().map(|f| f.role).unwrap_or(Role::Other);
                    let is_rdf = |name: &str| uri == NS_RDF && local == name;
                    let role = match parent_role {
                        _ if is_rdf("RDF") => Role::Rdf,
                        Role::Rdf if is_rdf("Description") => {
                            Role::Desc(new_desc(&mut doc, &stack, &bindings, &elem, None, true))
                        }
                        Role::Desc(d) | Role::ResProp(_, d) => {
                            doc.elem_props.push(ElemProp {
                                desc: d,
                                uri: uri.clone(),
                                local: local.to_owned(),
                                elem: elem.clone(),
                                container: None,
                                items: Vec::new(),
                                struct_desc: None,
                            });
                            let p = doc.elem_props.len() - 1;
                            let resource = attrs.iter().any(|a| {
                                let (ap, al) = split_qname(&a.qname);
                                al == "parseType" && lookup(ap).as_deref() == Some(NS_RDF) && a.value == "Resource"
                            });
                            if resource {
                                let nd = new_desc(&mut doc, &stack, &bindings, &elem, Some(p), false);
                                doc.elem_props[p].struct_desc = Some(nd);
                                Role::ResProp(p, nd)
                            } else {
                                Role::Prop(p)
                            }
                        }
                        Role::Prop(p)
                            if is_rdf("Description")
                                && doc.elem_props[p].struct_desc.is_none()
                                && doc.elem_props[p].container.is_none() =>
                        {
                            let nd = new_desc(&mut doc, &stack, &bindings, &elem, Some(p), true);
                            doc.elem_props[p].struct_desc = Some(nd);
                            Role::Desc(nd)
                        }
                        Role::Prop(p) if uri == NS_RDF && matches!(local, "Bag" | "Seq" | "Alt") => {
                            if doc.elem_props[p].container.is_none() {
                                doc.elem_props[p].container = Some(elem.clone());
                                Role::Container(p)
                            } else {
                                Role::Other
                            }
                        }
                        Role::Container(p) if is_rdf("li") => {
                            doc.elem_props[p].items.push(Item { elem: elem.clone(), value: String::new() });
                            Role::Item(p, doc.elem_props[p].items.len() - 1)
                        }
                        _ => Role::Other,
                    };
                    let frame = Frame { qname, bindings, role, elem };
                    if empty {
                        close_frame(&mut doc, src, frame);
                    } else {
                        stack.push(frame);
                    }
                }
                Event::End(_) => {
                    let mut frame = stack.pop().ok_or_else(|| err("unbalanced end tag"))?;
                    frame.elem.close_start = before;
                    frame.elem.end = after;
                    close_frame(&mut doc, src, frame);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if !stack.is_empty() {
            return Err(err(format!("unclosed element <{}>", stack[stack.len() - 1].qname)));
        }
        Ok(doc)
    }

    /// Whether description `d` is in `scope`.
    fn in_scope(&self, d: usize, scope: Scope) -> bool {
        match scope {
            Scope::Top => self.descs[d].parent.is_none(),
            Scope::Desc(x) => d == x,
        }
    }

    fn attrs_in(&self, scope: Scope) -> impl Iterator<Item = &AttrProp> + '_ {
        self.attr_props.iter().filter(move |a| self.in_scope(a.desc, scope))
    }

    fn elems_in(&self, scope: Scope) -> impl Iterator<Item = &ElemProp> + '_ {
        self.elem_props.iter().filter(move |p| self.in_scope(p.desc, scope))
    }

    fn top_attrs(&self) -> impl Iterator<Item = &AttrProp> + '_ {
        self.attrs_in(Scope::Top)
    }

    fn top_elems(&self) -> impl Iterator<Item = &ElemProp> + '_ {
        self.elems_in(Scope::Top)
    }

    fn scalars(&self, uri: &str, local: &str) -> Vec<ScalarRef<'_>> {
        self.scalars_in(Scope::Top, uri, local)
    }

    fn scalars_in(&self, scope: Scope, uri: &str, local: &str) -> Vec<ScalarRef<'_>> {
        let mut out: Vec<(usize, ScalarRef<'_>)> = Vec::new();
        for a in self.attrs_in(scope) {
            if a.uri == uri && a.local == local {
                out.push((a.attr.ws_start, ScalarRef { value: &a.attr.value }));
            }
        }
        for p in self.elems_in(scope) {
            if p.uri == uri && p.local == local && p.container.is_none() && p.struct_desc.is_none() {
                out.push((p.elem.start, ScalarRef { value: p.items.first().map_or("", |i| &i.value) }));
            }
        }
        out.sort_by_key(|(pos, _)| *pos);
        out.into_iter().map(|(_, s)| s).collect()
    }

    fn list(&self, uri: &str, local: &str) -> Option<&ElemProp> {
        self.list_in(Scope::Top, uri, local)
    }

    fn list_in(&self, scope: Scope, uri: &str, local: &str) -> Option<&ElemProp> {
        self.elems_in(scope).find(|p| p.uri == uri && p.local == local && p.struct_desc.is_none())
    }

    /// Struct property `(prop index, its description)` in `scope`.
    fn struct_in(&self, scope: Scope, uri: &str, local: &str) -> Option<(usize, usize)> {
        self.elem_props
            .iter()
            .enumerate()
            .find(|(_, p)| self.in_scope(p.desc, scope) && p.uri == uri && p.local == local && p.struct_desc.is_some())
            .map(|(i, p)| (i, p.struct_desc.unwrap_or_default()))
    }
}

/// Registers an `rdf:Description` (top-level when `parent` is `None`, else the value of
/// struct property `parent`) and, if `index_attrs`, its attribute-form properties.
fn new_desc(
    doc: &mut Doc,
    stack: &[Frame],
    bindings: &[(String, String)],
    elem: &Elem,
    parent: Option<usize>,
    index_attrs: bool,
) -> usize {
    let mut scope: HashMap<String, String> = HashMap::new();
    for f in stack {
        for (p, u) in &f.bindings {
            scope.insert(p.clone(), u.clone());
        }
    }
    for (p, u) in bindings {
        scope.insert(p.clone(), u.clone());
    }
    let idx = doc.descs.len();
    if index_attrs {
        for a in &elem.attrs {
            if a.qname == NS_XMLNS_PREFIX || a.qname.starts_with("xmlns:") {
                continue;
            }
            let (ap, al) = split_qname(&a.qname);
            // Unprefixed attributes have no namespace.
            let auri = if ap.is_empty() { String::new() } else { scope.get(ap).cloned().unwrap_or_default() };
            if auri == NS_RDF && matches!(al, "about" | "parseType" | "ID" | "nodeID") && parent.is_some() {
                continue;
            }
            doc.attr_props.push(AttrProp { desc: idx, uri: auri, local: al.to_owned(), attr: a.clone() });
        }
    }
    doc.descs.push(Desc { elem: elem.clone(), scope, parent });
    idx
}

/// Records element text / spans once the element is complete.
fn close_frame(doc: &mut Doc, src: &str, frame: Frame) {
    let elem = frame.elem;
    let text = || -> String {
        if elem.empty || elem.has_children {
            return String::new();
        }
        text_value(&src[elem.open_end..elem.close_start])
    };
    match frame.role {
        Role::Rdf => {
            if doc.rdf.is_none() {
                doc.rdf = Some(elem);
            }
        }
        Role::Desc(d) => doc.descs[d].elem = elem,
        Role::Prop(p) => {
            // Element-form scalar: keep its text as a pseudo item (no container).
            if doc.elem_props[p].container.is_none() {
                let value = text();
                doc.elem_props[p].items = vec![Item { elem: elem.clone(), value }];
            }
            doc.elem_props[p].elem = elem;
        }
        Role::Container(p) => doc.elem_props[p].container = Some(elem),
        Role::ResProp(p, d) => {
            doc.descs[d].elem = elem.clone();
            doc.elem_props[p].elem = elem;
        }
        Role::Item(p, i) => {
            let value = text();
            doc.elem_props[p].items[i] = Item { elem, value };
        }
        Role::Other => {}
    }
}

fn text_value(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")) {
        return inner.to_owned();
    }
    unescape(raw).map(Cow::into_owned).unwrap_or_else(|_| raw.to_owned())
}

/// Scans the attributes of a start tag `src[from..tag_end]` (from = just after the name).
fn scan_attrs(src: &str, from: usize, tag_end: usize) -> Result<Vec<Attr>> {
    let b = src.as_bytes();
    let mut i = from;
    let mut out = Vec::new();
    loop {
        let ws_start = i;
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || b[i] == b'>' || b[i] == b'/' {
            return Ok(out);
        }
        let name_start = i;
        while i < tag_end && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let qname = src[name_start..i].to_owned();
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || b[i] != b'=' {
            return Err(err(format!("attribute {qname} without value")));
        }
        i += 1;
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || (b[i] != b'"' && b[i] != b'\'') {
            return Err(err(format!("unquoted attribute {qname}")));
        }
        let quote = b[i];
        let value_start = i + 1;
        let value_end = value_start
            + src[value_start..tag_end]
                .bytes()
                .position(|c| c == quote)
                .ok_or_else(|| err("unterminated attribute"))?;
        let raw = &src[value_start..value_end];
        let value = unescape(raw).map(Cow::into_owned).unwrap_or_else(|_| raw.to_owned());
        i = value_end + 1;
        out.push(Attr { ws_start, name_start, end: i, qname, value_start, value_end, value });
    }
}

fn split_qname(q: &str) -> (&str, &str) {
    match q.split_once(':') {
        Some((p, l)) => (p, l),
        None => ("", q),
    }
}

fn prefix_of(q: &str) -> &str {
    split_qname(q).0
}

fn qualify(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

fn preferred_prefix(uri: &str) -> &'static str {
    match uri {
        NS_XMP => "xmp",
        NS_DC => "dc",
        NS_LR => "lr",
        NS_RDF => "rdf",
        crs::CRS_NS => "crs",
        crs::SIEVE_NS => "sieve",
        _ => "ns",
    }
}

/// Whitespace-only run immediately before `pos`.
fn ws_before(src: &str, pos: usize) -> usize {
    let b = src.as_bytes();
    let mut i = pos;
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i
}

/// Indentation of the line `pos` sits on, if only whitespace precedes it on that line.
fn line_indent(src: &str, pos: usize) -> Option<&str> {
    let line_start = src[..pos].rfind('\n').map_or(0, |i| i + 1);
    let indent = &src[line_start..pos];
    indent.bytes().all(|c| c == b' ' || c == b'\t').then_some(indent)
}

// ---------------------------------------------------------------------------
// Editor: collects splices, applies them in one pass
// ---------------------------------------------------------------------------

struct Splice {
    start: usize,
    end: usize,
    seq: usize,
    text: String,
}

struct Editor<'a> {
    src: &'a str,
    doc: &'a Doc,
    splices: Vec<Splice>,
    /// Per description: namespace declarations and attributes to add to the start tag.
    new_attrs: HashMap<usize, Vec<String>>,
    /// Per description: prefixes declared by us (prefix -> uri).
    new_ns: HashMap<usize, Vec<(String, String)>>,
    /// Per description: child elements to append (already rendered at relative indent 0).
    new_children: HashMap<usize, Vec<Vec<String>>>,
}

impl<'a> Editor<'a> {
    fn new(src: &'a str, doc: &'a Doc) -> Self {
        Editor {
            src,
            doc,
            splices: Vec::new(),
            new_attrs: HashMap::new(),
            new_ns: HashMap::new(),
            new_children: HashMap::new(),
        }
    }

    fn splice(&mut self, start: usize, end: usize, text: impl Into<String>) {
        let seq = self.splices.len();
        self.splices.push(Splice { start, end, seq, text: text.into() });
    }

    /// Description to host a new property in namespace `uri`: the first one that already
    /// binds it, else the first one.
    fn host_desc(&self, uri: &str) -> usize {
        self.doc
            .descs
            .iter()
            .position(|d| d.parent.is_none() && d.scope.values().any(|u| u == uri))
            .or_else(|| self.doc.descs.iter().position(|d| d.parent.is_none()))
            .unwrap_or(0)
    }

    /// Prefix bound to `uri` in description `d`, declaring one if needed.
    fn prefix_for(&mut self, d: usize, uri: &str) -> String {
        let scope = &self.doc.descs[d].scope;
        let mut bound: Vec<&String> =
            scope.iter().filter(|(p, u)| *u == uri && !p.is_empty()).map(|(p, _)| p).collect();
        bound.sort();
        if let Some(p) = bound.first() {
            return (*p).clone();
        }
        let pending = self.new_ns.entry(d).or_default();
        if let Some((p, _)) = pending.iter().find(|(_, u)| u == uri) {
            return p.clone();
        }
        let base = preferred_prefix(uri);
        let taken = |p: &str| scope.contains_key(p) || pending.iter().any(|(q, _)| q == p);
        let mut prefix = base.to_owned();
        let mut n = 1;
        while taken(&prefix) {
            prefix = format!("{base}{n}");
            n += 1;
        }
        pending.push((prefix.clone(), uri.to_owned()));
        self.new_attrs.entry(d).or_default().push(format!("xmlns:{prefix}=\"{}\"", escape(uri)));
        prefix
    }

    fn set_scalar(&mut self, uri: &str, local: &str, value: &str) {
        self.set_scalar_in(Scope::Top, uri, local, value);
    }

    /// Sets a scalar in `scope`; if absent, adds it as an attribute of the host description
    /// (top level) or of the scoped description.
    fn set_scalar_in(&mut self, scope: Scope, uri: &str, local: &str, value: &str) {
        let esc = escape(value).into_owned();
        let mut found = false;
        let attr_hits: Vec<Attr> =
            self.doc.attrs_in(scope).filter(|a| a.uri == uri && a.local == local).map(|a| a.attr.clone()).collect();
        for a in attr_hits {
            found = true;
            if a.value != value {
                self.splice(a.value_start, a.value_end, esc.clone());
            }
        }
        let elem_hits: Vec<ElemProp> = self
            .doc
            .elems_in(scope)
            .filter(|p| p.uri == uri && p.local == local && p.container.is_none() && p.struct_desc.is_none())
            .cloned()
            .collect();
        for p in elem_hits {
            found = true;
            let current = p.items.first().map_or("", |i| i.value.as_str());
            if p.elem.empty || p.elem.has_children {
                let q = &p.elem.qname;
                self.splice(p.elem.start, p.elem.end, format!("<{q}>{esc}</{q}>"));
            } else if current != value {
                self.splice(p.elem.open_end, p.elem.close_start, esc.clone());
            }
        }
        if !found {
            let d = match scope {
                Scope::Top => self.host_desc(uri),
                Scope::Desc(d) => d,
            };
            let prefix = self.prefix_for(d, uri);
            self.new_attrs.entry(d).or_default().push(format!("{}=\"{esc}\"", qualify(&prefix, local)));
        }
    }

    fn remove_scalar_if(&mut self, uri: &str, local: &str, pred: impl Fn(&str) -> bool) {
        let attr_hits: Vec<Attr> = self
            .doc
            .top_attrs()
            .filter(|a| a.uri == uri && a.local == local && pred(&a.attr.value))
            .map(|a| a.attr.clone())
            .collect();
        for a in attr_hits {
            self.splice(a.ws_start, a.end, "");
        }
        let elem_hits: Vec<Elem> = self
            .doc
            .top_elems()
            .filter(|p| {
                p.uri == uri
                    && p.local == local
                    && p.container.is_none()
                    && p.struct_desc.is_none()
                    && pred(p.items.first().map_or("", |i| &i.value))
            })
            .map(|p| p.elem.clone())
            .collect();
        for e in elem_hits {
            self.splice(ws_before(self.src, e.start), e.end, "");
        }
    }

    /// Replaces (or removes, `None`) every top-level element-form property `uri:local`
    /// (scalar, list or struct) and attribute of that name with `lines` (relative
    /// indentation, rendered with the property's qualified name `qname`), or appends it to
    /// the host description when absent.
    fn put_element(&mut self, uri: &str, local: &str, render: &dyn Fn(&str, &str) -> Vec<String>, keep: bool) {
        let attrs: Vec<Attr> =
            self.doc.top_attrs().filter(|a| a.uri == uri && a.local == local).map(|a| a.attr.clone()).collect();
        for a in attrs {
            self.splice(a.ws_start, a.end, "");
        }
        let elems: Vec<ElemProp> = self.doc.top_elems().filter(|p| p.uri == uri && p.local == local).cloned().collect();
        let mut placed = !keep;
        for p in elems {
            if placed {
                self.splice(ws_before(self.src, p.elem.start), p.elem.end, "");
                continue;
            }
            let rdf = self.rdf_prefix(p.desc);
            let lines = render(&p.elem.qname, &rdf);
            let indent = line_indent(self.src, p.elem.start).unwrap_or("").to_owned();
            self.splice(p.elem.start, p.elem.end, lines.join(&format!("\n{indent}")));
            placed = true;
        }
        if !placed {
            let d = self.host_desc(uri);
            let prefix = self.prefix_for(d, uri);
            let rdf = self.rdf_prefix(d);
            let lines = render(&qualify(&prefix, local), &rdf);
            self.new_children.entry(d).or_default().push(lines);
        }
    }

    /// Creates / replaces / removes an `rdf:Seq` property. Unchanged items are left as-is.
    fn set_seq(&mut self, uri: &str, local: &str, items: Option<&[String]>) {
        let existing = self.doc.list(uri, local).filter(|p| p.container.is_some()).map(|p| {
            let kind = p.container.as_ref().map(|c| split_qname(&c.qname).1.to_owned()).unwrap_or_default();
            (kind, p.items.iter().map(|i| i.value.clone()).collect::<Vec<_>>())
        });
        let has_scalar = !self.doc.scalars(uri, local).is_empty();
        match items {
            None => {
                if existing.is_some() || has_scalar {
                    self.put_element(uri, local, &|_, _| Vec::new(), false);
                }
            }
            Some(items) => {
                if existing.as_ref().is_some_and(|(k, v)| k == "Seq" && v == items) && !has_scalar {
                    return;
                }
                let items = items.to_vec();
                self.put_element(uri, local, &|q, rdf| render_list(q, rdf, "Seq", &items), true);
            }
        }
    }

    /// Removes items matching `remove` and appends missing `add` values to the list property.
    fn update_list(&mut self, uri: &str, local: &str, remove: &dyn Fn(&str) -> bool, add: &[String]) {
        let Some(prop) = self.doc.list(uri, local).cloned() else {
            if add.is_empty() {
                return;
            }
            let d = self.host_desc(uri);
            let prefix = self.prefix_for(d, uri);
            let rdf = self.rdf_prefix(d);
            let lines = render_list(&qualify(&prefix, local), &rdf, "Bag", add);
            self.new_children.entry(d).or_default().push(lines);
            return;
        };
        let values: Vec<&str> = prop.items.iter().map(|i| i.value.as_str()).collect();
        let is_list = prop.container.is_some();
        let kept: Vec<&Item> = prop.items.iter().filter(|i| !is_list || !remove(&i.value)).collect();
        let removed: Vec<&Item> =
            if is_list { prop.items.iter().filter(|i| remove(&i.value)).collect() } else { Vec::new() };
        let mut to_add: Vec<&String> = Vec::new();
        for a in add {
            let present = if is_list { values.contains(&a.as_str()) && !remove(a) } else { false };
            if !present && !to_add.contains(&a) {
                to_add.push(a);
            }
        }
        if removed.is_empty() && to_add.is_empty() {
            return;
        }
        let container = prop.container.as_ref();
        let indent = line_indent(self.src, prop.elem.start).map(str::to_owned);
        match container {
            Some(c) if !c.empty && (!kept.is_empty() || !to_add.is_empty()) => {
                for item in &removed {
                    self.splice(ws_before(self.src, item.elem.start), item.elem.end, "");
                }
                if !to_add.is_empty() {
                    let li = qualify(prefix_of(&c.qname), "li");
                    let item_indent = prop
                        .items
                        .first()
                        .and_then(|i| line_indent(self.src, i.elem.start))
                        .map(str::to_owned)
                        .or_else(|| line_indent(self.src, c.close_start).map(|s| format!("{s} ")));
                    let at = ws_before(self.src, c.close_start);
                    let mut text = String::new();
                    for v in &to_add {
                        match &item_indent {
                            Some(ind) => text.push_str(&format!("\n{ind}")),
                            None => text.push(' '),
                        }
                        text.push_str(&format!("<{li}>{}</{li}>", escape(v.as_str())));
                    }
                    self.splice(at, at, text);
                }
            }
            _ => {
                let remaining: Vec<String> = if is_list {
                    kept.iter().map(|i| i.value.clone()).chain(to_add.iter().map(|s| (*s).clone())).collect()
                } else {
                    // Scalar-shaped keyword property: keep its text as an item.
                    values
                        .iter()
                        .filter(|v| !v.is_empty())
                        .map(|v| (*v).to_owned())
                        .chain(to_add.iter().map(|s| (*s).clone()))
                        .collect()
                };
                if remaining.is_empty() {
                    self.splice(ws_before(self.src, prop.elem.start), prop.elem.end, "");
                } else {
                    let rdf = self.rdf_prefix(prop.desc);
                    let kind = container.map_or("Bag", |c| split_qname(&c.qname).1);
                    let lines = render_list(&prop.elem.qname, &rdf, kind, &remaining);
                    let ind = indent.unwrap_or_default();
                    self.splice(prop.elem.start, prop.elem.end, lines.join(&format!("\n{ind}")));
                }
            }
        }
    }

    fn rdf_prefix(&mut self, d: usize) -> String {
        let own = prefix_of(&self.doc.descs[d].elem.qname).to_owned();
        if self.doc.descs[d].scope.get(&own).map(String::as_str) == Some(NS_RDF) {
            return own;
        }
        self.prefix_for(d, NS_RDF)
    }

    fn finish(mut self) -> Result<String> {
        let src = self.src;
        let mut descs: Vec<usize> = self.new_attrs.keys().chain(self.new_children.keys()).copied().collect();
        descs.sort_unstable();
        descs.dedup();
        for d in descs {
            let desc = &self.doc.descs[d].elem;
            if let Some(mut attrs) = self.new_attrs.remove(&d) {
                // Namespace declarations before the properties that use them.
                attrs.sort_by_key(|a| !a.starts_with("xmlns"));
                let (at, sep) = match desc.attrs.last() {
                    Some(last) if last.name_start > last.ws_start => (last.end, &src[last.ws_start..last.name_start]),
                    Some(last) => (last.end, " "),
                    None => (desc.start + 1 + desc.qname.len(), " "),
                };
                let text: String = attrs.iter().map(|a| format!("{sep}{a}")).collect();
                self.splice(at, at, text);
            }
            if let Some(children) = self.new_children.remove(&d) {
                let desc_indent = line_indent(src, desc.start).unwrap_or("").to_owned();
                if desc.empty {
                    // `<rdf:Description .../>` -> open it.
                    let slash = src[..desc.open_end].rfind('/').ok_or_else(|| err("malformed empty element"))?;
                    let child_indent = format!("{desc_indent} ");
                    let mut text = String::from(">");
                    for lines in &children {
                        text.push_str(&format!("\n{child_indent}{}", lines.join(&format!("\n{child_indent}"))));
                    }
                    text.push_str(&format!("\n{desc_indent}</{}>", desc.qname));
                    self.splice(slash, desc.open_end, text);
                } else {
                    let close_indent = line_indent(src, desc.close_start);
                    // Match the existing children's indentation.
                    let sibling =
                        self.doc.elem_props.iter().find(|p| p.desc == d).and_then(|p| line_indent(src, p.elem.start));
                    let child_indent = match (sibling, close_indent) {
                        (Some(s), _) => s.to_owned(),
                        (None, Some(i)) => format!("{i} "),
                        (None, None) => format!("{desc_indent} "),
                    };
                    let at = ws_before(src, desc.close_start);
                    let mut text = String::new();
                    for lines in &children {
                        text.push_str(&format!("\n{child_indent}{}", lines.join(&format!("\n{child_indent}"))));
                    }
                    if close_indent.is_none() || at == desc.close_start {
                        text.push_str(&format!("\n{desc_indent}"));
                    }
                    self.splice(at, at, text);
                }
            }
        }

        self.splices.sort_by_key(|s| (s.start, s.seq));
        let mut out = String::with_capacity(src.len() + 512);
        let mut cursor = 0;
        for s in &self.splices {
            if s.start < cursor {
                return Err(err("internal: overlapping edits"));
            }
            out.push_str(&src[cursor..s.start]);
            out.push_str(&s.text);
            cursor = s.end.max(s.start);
        }
        out.push_str(&src[cursor..]);
        // Never emit something we cannot parse back.
        Doc::parse(&out).map_err(|e| err(format!("internal: merge produced invalid XML ({})", e.0)))?;
        Ok(out)
    }
}

/// Renders a list property as lines at relative indentation (one space per level).
fn render_list(qname: &str, rdf: &str, kind: &str, items: &[String]) -> Vec<String> {
    let container = qualify(rdf, kind);
    let li = qualify(rdf, "li");
    let mut lines = vec![format!("<{qname}>"), format!(" <{container}>")];
    for v in items {
        lines.push(format!("  <{li}>{}</{li}>", escape(v.as_str())));
    }
    lines.push(format!(" </{container}>"));
    lines.push(format!("</{qname}>"));
    lines
}
