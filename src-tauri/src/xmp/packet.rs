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

use super::crs::{self, PropertyEdit};
use crate::ipc::types::ParametricAdjustments;

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
    let get = |ns: &str, local: &str| doc.scalars(ns, local).into_iter().next().map(|s| s.value.to_owned());
    let (develop, develop_error) = match crs::decode(&get) {
        Ok(d) => (d, None),
        Err(e) => (None, Some(e)),
    };
    Ok(SidecarValues {
        rating,
        label,
        hierarchical_subjects: list(NS_LR, "hierarchicalSubject"),
        subjects: list(NS_DC, "subject"),
        develop,
        develop_error,
    })
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
    for edit in &want.develop {
        match &edit.value {
            Some(v) => ed.set_scalar(edit.ns, &edit.name, v),
            None => ed.remove_scalar_if(edit.ns, &edit.name, |_| true),
        }
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
}

#[derive(Debug, Clone)]
struct AttrProp {
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

#[derive(Debug, Clone, Copy)]
enum Role {
    Other,
    Rdf,
    Desc(usize),
    Prop(usize),
    Container(usize),
    Item(usize, usize),
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
                            let mut scope: HashMap<String, String> = HashMap::new();
                            for f in &stack {
                                for (p, u) in &f.bindings {
                                    scope.insert(p.clone(), u.clone());
                                }
                            }
                            for (p, u) in &bindings {
                                scope.insert(p.clone(), u.clone());
                            }
                            let idx = doc.descs.len();
                            for a in &attrs {
                                if a.qname == NS_XMLNS_PREFIX || a.qname.starts_with("xmlns:") {
                                    continue;
                                }
                                let (ap, al) = split_qname(&a.qname);
                                // Unprefixed attributes have no namespace.
                                let auri = if ap.is_empty() {
                                    String::new()
                                } else {
                                    scope.get(ap).cloned().unwrap_or_default()
                                };
                                doc.attr_props.push(AttrProp { uri: auri, local: al.to_owned(), attr: a.clone() });
                            }
                            doc.descs.push(Desc { elem: elem.clone(), scope });
                            Role::Desc(idx)
                        }
                        Role::Desc(d) => {
                            doc.elem_props.push(ElemProp {
                                desc: d,
                                uri: uri.clone(),
                                local: local.to_owned(),
                                elem: elem.clone(),
                                container: None,
                                items: Vec::new(),
                            });
                            Role::Prop(doc.elem_props.len() - 1)
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

    fn scalars(&self, uri: &str, local: &str) -> Vec<ScalarRef<'_>> {
        let mut out: Vec<(usize, ScalarRef<'_>)> = Vec::new();
        for a in &self.attr_props {
            if a.uri == uri && a.local == local {
                out.push((a.attr.ws_start, ScalarRef { value: &a.attr.value }));
            }
        }
        for p in &self.elem_props {
            if p.uri == uri && p.local == local && p.container.is_none() {
                out.push((p.elem.start, ScalarRef { value: p.items.first().map_or("", |i| &i.value) }));
            }
        }
        out.sort_by_key(|(pos, _)| *pos);
        out.into_iter().map(|(_, s)| s).collect()
    }

    fn list(&self, uri: &str, local: &str) -> Option<&ElemProp> {
        self.elem_props.iter().find(|p| p.uri == uri && p.local == local)
    }
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
        self.doc.descs.iter().position(|d| d.scope.values().any(|u| u == uri)).unwrap_or(0)
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
        let esc = escape(value).into_owned();
        let mut found = false;
        let attr_hits: Vec<Attr> =
            self.doc.attr_props.iter().filter(|a| a.uri == uri && a.local == local).map(|a| a.attr.clone()).collect();
        for a in attr_hits {
            found = true;
            if a.value != value {
                self.splice(a.value_start, a.value_end, esc.clone());
            }
        }
        let elem_hits: Vec<ElemProp> = self
            .doc
            .elem_props
            .iter()
            .filter(|p| p.uri == uri && p.local == local && p.container.is_none())
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
            let d = self.host_desc(uri);
            let prefix = self.prefix_for(d, uri);
            self.new_attrs.entry(d).or_default().push(format!("{}=\"{esc}\"", qualify(&prefix, local)));
        }
    }

    fn remove_scalar_if(&mut self, uri: &str, local: &str, pred: impl Fn(&str) -> bool) {
        let attr_hits: Vec<Attr> = self
            .doc
            .attr_props
            .iter()
            .filter(|a| a.uri == uri && a.local == local && pred(&a.attr.value))
            .map(|a| a.attr.clone())
            .collect();
        for a in attr_hits {
            self.splice(a.ws_start, a.end, "");
        }
        let elem_hits: Vec<Elem> = self
            .doc
            .elem_props
            .iter()
            .filter(|p| {
                p.uri == uri
                    && p.local == local
                    && p.container.is_none()
                    && pred(p.items.first().map_or("", |i| &i.value))
            })
            .map(|p| p.elem.clone())
            .collect();
        for e in elem_hits {
            self.splice(ws_before(self.src, e.start), e.end, "");
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
