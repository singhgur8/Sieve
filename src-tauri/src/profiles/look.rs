//! Look profiles (`crs:PresetType="Look"` XMP files, and `<crs:Look>` structs in sidecars)
//! (rust-engine-dev).
//!
//! A look file's top-level `rdf:Description` carries: `crs:UUID`, `crs:Name` (rdf:Alt),
//! `crs:Group` (rdf:Alt), `crs:SupportsAmount`, `crs:SupportsMonochrome`,
//! `crs:CameraModelRestriction`, `crs:CameraProfile`, `crs:ConvertToGrayscale`,
//! `crs:LookTable` / `crs:RGBTable` (MD5 references) + the matching `crs:Table_<MD5>` values,
//! `crs:RGBTableAmount`, and ordinary develop settings (e.g. `crs:Clarity2012`,
//! `crs:ToneCurvePV2012*`) that the look applies. A sidecar's `<crs:Look>` holds the same
//! inside `crs:Parameters` (without the tables for Adobe's bundled looks; with `crs:Table_*`
//! on the sidecar's top level for looks that are not installed).
//!
//! Parsing uses a small namespace-aware reader over `quick-xml` (read-only; independent of
//! `xmp::packet`, which edits sidecars span-preservingly).

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::ipc::types::{ParametricAdjustments, ProfileSettings};
use crate::xmp::crs::{self, CrsSource, CRS_NS};

use super::table::{self, BigTable};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

#[derive(Debug, Clone, PartialEq)]
pub struct LookProfile {
    pub uuid: String,
    pub name: String,
    pub group: String,
    pub supports_amount: bool,
    pub monochrome: bool,
    pub camera_profile: Option<String>,
    /// `crs:CameraModelRestriction` (empty = any camera).
    pub camera_model_restriction: Option<String>,
    /// `crs:SupportsOutputReferred`: usable on non-RAW (display-referred) sources.
    pub supports_output_referred: bool,
    /// Decoded `LookTable` and/or `RGBTable`.
    pub tables: Vec<BigTable>,
    /// `crs:RGBTableAmount` (scales the RGB table's blend; 1 when absent).
    pub rgb_table_amount: f32,
    /// The look's own develop settings (parsed with `xmp::crs::decode` over the look's
    /// properties, neutral where absent): applied at `amount` beneath the user's settings.
    pub parameters: ParametricAdjustments,
}

// ---------------------------------------------------------------------------
// Minimal namespace-aware DOM
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub(crate) struct Node {
    pub ns: String,
    pub local: String,
    /// (namespace, local, value)
    pub attrs: Vec<(String, String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

impl Node {
    fn attr(&self, ns: &str, local: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, l, _)| n == ns && l == local).map(|(_, _, v)| v.as_str())
    }

    fn child(&self, ns: &str, local: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.ns == ns && c.local == local)
    }

    fn descendants<'a>(&'a self, out: &mut Vec<&'a Node>) {
        for c in &self.children {
            out.push(c);
            c.descendants(out);
        }
    }

    /// Text of a simple property: attribute or element text (`rdf:Alt` -> first item).
    fn prop(&self, ns: &str, local: &str) -> Option<String> {
        if let Some(v) = self.attr(ns, local) {
            return Some(v.to_owned());
        }
        let el = self.child(ns, local)?;
        if let Some(alt) = el.children.iter().find(|c| c.ns == RDF_NS && (c.local == "Alt" || c.local == "Bag")) {
            return alt.children.first().map(|li| li.text.trim().to_owned());
        }
        Some(el.text.trim().to_owned())
    }

    fn seq(&self, ns: &str, local: &str) -> Option<Vec<String>> {
        let el = self.child(ns, local)?;
        let seq = el.children.iter().find(|c| c.ns == RDF_NS && (c.local == "Seq" || c.local == "Bag"))?;
        Some(seq.children.iter().map(|li| li.text.trim().to_owned()).collect())
    }

    /// The struct value of property `local`: its `rdf:Description` child, or the element
    /// itself (attributes / `rdf:parseType="Resource"`).
    fn structure(&self, ns: &str, local: &str) -> Option<&Node> {
        let el = self.child(ns, local)?;
        Some(el.child(RDF_NS, "Description").unwrap_or(el))
    }
}

/// Parses XML into a tree with resolved namespaces. The root is a synthetic node.
pub(crate) fn parse_dom(xml: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<(Node, HashMap<String, String>)> = vec![(Node::default(), HashMap::new())];
    fn open(e: &BytesStart, scopes: &HashMap<String, String>) -> Result<(Node, HashMap<String, String>), String> {
        let mut ns_map = scopes.clone();
        let mut raw_attrs = Vec::new();
        for a in e.attributes().with_checks(false) {
            let a = a.map_err(|e| format!("xml attribute: {e}"))?;
            let key = a.key.as_ref().to_owned();
            let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|e| format!("xml value: {e}"))?.into_owned();
            if let Some(p) = key.strip_prefix("xmlns:") {
                ns_map.insert(p.to_owned(), value);
            } else if key == "xmlns" {
                ns_map.insert(String::new(), value);
            } else {
                raw_attrs.push((key, value));
            }
        }
        let resolve = |q: &str, default_ns: bool| -> (String, String) {
            match q.split_once(':') {
                Some((p, l)) => (ns_map.get(p).cloned().unwrap_or_default(), l.to_owned()),
                None => {
                    let ns = if default_ns { ns_map.get("").cloned().unwrap_or_default() } else { String::new() };
                    (ns, q.to_owned())
                }
            }
        };
        let qname = e.name().as_ref().to_owned();
        let (ns, local) = resolve(&qname, true);
        let attrs = raw_attrs
            .into_iter()
            .filter(|(k, _)| !k.starts_with("xml:"))
            .map(|(k, v)| {
                let (n, l) = resolve(&k, false);
                (n, l, v)
            })
            .collect();
        Ok((Node { ns, local, attrs, children: Vec::new(), text: String::new() }, ns_map))
    }
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let scopes = &stack.last().expect("root").1;
                let (node, map) = open(&e, scopes)?;
                stack.push((node, map));
            }
            Ok(Event::Empty(e)) => {
                let scopes = &stack.last().expect("root").1;
                let (node, _) = open(&e, scopes)?;
                stack.last_mut().expect("root").0.children.push(node);
            }
            Ok(Event::Text(t)) => {
                stack.last_mut().expect("root").0.text.push_str(&t);
            }
            Ok(Event::GeneralRef(r)) => {
                let name: String = r.to_string();
                let s = match name.as_str() {
                    "amp" => "&".to_owned(),
                    "lt" => "<".to_owned(),
                    "gt" => ">".to_owned(),
                    "quot" => "\"".to_owned(),
                    "apos" => "'".to_owned(),
                    _ => r.resolve_char_ref().ok().flatten().map(String::from).unwrap_or_default(),
                };
                stack.last_mut().expect("root").0.text.push_str(&s);
            }
            Ok(Event::CData(t)) => {
                stack.last_mut().expect("root").0.text.push_str(&t);
            }
            Ok(Event::End(_)) => {
                if stack.len() < 2 {
                    return Err("xml: unbalanced end tag".into());
                }
                let (node, _) = stack.pop().expect("len >= 2");
                stack.last_mut().expect("root").0.children.push(node);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("xml: {e}")),
        }
    }
    if stack.len() != 1 {
        return Err("xml: unclosed elements".into());
    }
    Ok(stack.pop().expect("root").0)
}

/// Top-level `rdf:Description`s of a packet.
fn descriptions(root: &Node) -> Vec<&Node> {
    let mut all = Vec::new();
    root.descendants(&mut all);
    let rdf = all.iter().find(|n| n.ns == RDF_NS && n.local == "RDF");
    match rdf {
        Some(rdf) => rdf.children.iter().filter(|c| c.ns == RDF_NS && c.local == "Description").collect(),
        None => Vec::new(),
    }
}

/// `CrsSource` over one description node (plus fallbacks, e.g. the other top-level
/// descriptions of the same packet).
pub(crate) struct NodeSource<'a>(pub Vec<&'a Node>);

impl CrsSource for NodeSource<'_> {
    fn scalar(&self, ns: &str, name: &str) -> Option<String> {
        self.0.iter().find_map(|n| n.prop(ns, name))
    }
    fn seq(&self, ns: &str, name: &str) -> Option<Vec<String>> {
        self.0.iter().find_map(|n| n.seq(ns, name))
    }
    fn has(&self, ns: &str, name: &str) -> bool {
        self.0.iter().any(|n| n.attr(ns, name).is_some() || n.child(ns, name).is_some())
    }
    fn look(&self) -> Option<crate::ipc::types::LookSettings> {
        None
    }
}

fn parse_bool(v: Option<String>) -> bool {
    v.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
}

/// Neutral develop settings (nothing applied): the base the look's own settings go on.
pub fn neutral_parameters() -> ParametricAdjustments {
    let mut p = ParametricAdjustments::default();
    p.detail.sharpening.amount = 0.0;
    p.detail.noise_reduction.color = 0.0;
    p.profile = ProfileSettings::none();
    p
}

/// The look's develop settings from its properties (`crs::decode_source`, or just the v9
/// groups when no basic slider is present).
fn parameters(src: &NodeSource) -> Result<ParametricAdjustments, String> {
    let mut adj = match crs::decode_source(src)? {
        Some(mut a) => {
            a.detail = neutral_parameters().detail;
            a.profile = ProfileSettings::none();
            a
        }
        None => {
            let mut a = neutral_parameters();
            crs::decode_parity(src, &mut a)?;
            a
        }
    };
    adj.lut = None;
    Ok(adj)
}

/// Decodes the tables referenced by `LookTable` / `RGBTable` (MD5s) found as
/// `crs:Table_<md5>` in any of `holders`. Missing or undecodable tables are skipped.
fn tables(params: &NodeSource, holders: &NodeSource) -> Vec<BigTable> {
    let mut out = Vec::new();
    for key in ["LookTable", "RGBTable"] {
        let Some(md5) = params.scalar(CRS_NS, key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) else {
            continue;
        };
        let attr = format!("Table_{md5}");
        let value = holders.scalar(CRS_NS, &attr).or_else(|| params.scalar(CRS_NS, &attr));
        if let Some(value) = value {
            match table::decode(&value, &md5) {
                Ok(t) => out.push(t),
                Err(e) => eprintln!("look table {md5}: {e}"),
            }
        }
    }
    out
}

impl LookProfile {
    /// Parses a look profile file (`Settings/**/*.xmp`). `Ok(None)` if it is not a look
    /// (`crs:PresetType` other than "Look").
    pub fn parse_file(xmp: &str) -> Result<Option<LookProfile>, String> {
        Self::parse_file_opts(xmp, true)
    }

    /// As [`Self::parse_file`]; `with_tables = false` skips table decoding (index scans).
    pub fn parse_file_opts(xmp: &str, with_tables: bool) -> Result<Option<LookProfile>, String> {
        let root = parse_dom(xmp)?;
        let descs = descriptions(&root);
        let Some(desc) = descs.iter().find(|d| d.prop(CRS_NS, "PresetType").is_some()) else { return Ok(None) };
        if desc.prop(CRS_NS, "PresetType").as_deref() != Some("Look") {
            return Ok(None);
        }
        let src = NodeSource(vec![desc]);
        let uuid = src.scalar(CRS_NS, "UUID").unwrap_or_default().trim().to_ascii_uppercase();
        if uuid.is_empty() {
            return Err("look without crs:UUID".into());
        }
        let parameters = parameters(&src)?;
        let tables = if with_tables { tables(&src, &src) } else { Vec::new() };
        Ok(Some(LookProfile {
            uuid,
            name: src.scalar(CRS_NS, "Name").unwrap_or_default(),
            group: src.scalar(CRS_NS, "Group").unwrap_or_default(),
            supports_amount: parse_bool(src.scalar(CRS_NS, "SupportsAmount")),
            monochrome: parse_bool(src.scalar(CRS_NS, "ConvertToGrayscale")),
            camera_profile: src.scalar(CRS_NS, "CameraProfile").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            camera_model_restriction: src
                .scalar(CRS_NS, "CameraModelRestriction")
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            supports_output_referred: parse_bool(src.scalar(CRS_NS, "SupportsOutputReferred")),
            tables,
            rgb_table_amount: src
                .scalar(CRS_NS, "RGBTableAmount")
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(1.0),
            parameters,
        }))
    }

    /// The look recorded in a sidecar (`<crs:Look>` + top-level `crs:Table_*`), used when the
    /// look is not installed. `Ok(None)` if the sidecar has no look.
    pub fn from_sidecar(xmp: &str) -> Result<Option<LookProfile>, String> {
        let root = parse_dom(xmp)?;
        let descs = descriptions(&root);
        let Some(look) = descs.iter().find_map(|d| d.structure(CRS_NS, "Look")) else { return Ok(None) };
        let head = NodeSource(vec![look]);
        let params_node = look.structure(CRS_NS, "Parameters");
        let params = NodeSource(params_node.into_iter().collect());
        let holders = NodeSource(descs.clone());
        let parameters = parameters(&params)?;
        let tables = tables(&params, &holders);
        let uuid = head.scalar(CRS_NS, "UUID").unwrap_or_default().trim().to_ascii_uppercase();
        Ok(Some(LookProfile {
            uuid,
            name: head.scalar(CRS_NS, "Name").unwrap_or_default(),
            group: head.scalar(CRS_NS, "Group").unwrap_or_default(),
            supports_amount: parse_bool(head.scalar(CRS_NS, "SupportsAmount")),
            monochrome: parse_bool(params.scalar(CRS_NS, "ConvertToGrayscale"))
                || parse_bool(head.scalar(CRS_NS, "SupportsMonochrome")) && parameters.black_and_white.enabled,
            camera_profile: params.scalar(CRS_NS, "CameraProfile").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            camera_model_restriction: None,
            supports_output_referred: parse_bool(head.scalar(CRS_NS, "SupportsOutputReferred")),
            tables,
            rgb_table_amount: params
                .scalar(CRS_NS, "RGBTableAmount")
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(1.0),
            parameters,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Look" crs:UUID="0CFE8F8AB5F63B2A73CE0B0077D20817" crs:SupportsAmount="False"
   crs:SupportsOutputReferred="False" crs:CameraModelRestriction=""
   crs:ProcessVersion="10.0" crs:Clarity2012="+8" crs:ConvertToGrayscale="True"
   crs:CameraProfile="Adobe Standard" crs:LookTable="0123456789ABCDEF0123456789ABCDEF">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Adobe Monochrome</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
   <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>64, 56</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn parses_look_file() {
        let l = LookProfile::parse_file(LOOK).unwrap().unwrap();
        assert_eq!(l.uuid, "0CFE8F8AB5F63B2A73CE0B0077D20817");
        assert_eq!(l.name, "Adobe Monochrome");
        assert_eq!(l.group, "Profiles");
        assert!(!l.supports_amount && l.monochrome && !l.supports_output_referred);
        assert_eq!(l.camera_profile.as_deref(), Some("Adobe Standard"));
        assert_eq!(l.camera_model_restriction, None);
        assert_eq!(l.parameters.clarity, 8.0);
        assert!(l.parameters.black_and_white.enabled);
        assert_eq!(l.parameters.tone_curve.point.master, vec![[0.0, 0.0], [64.0, 56.0], [255.0, 255.0]]);
        assert_eq!(l.parameters.detail.sharpening.amount, 0.0);
        assert!(l.tables.is_empty(), "referenced table not present");
        // Not a look.
        let preset = LOOK.replace("crs:PresetType=\"Look\"", "crs:PresetType=\"Normal\"");
        assert!(LookProfile::parse_file(&preset).unwrap().is_none());
        assert!(LookProfile::parse_file("<a><b></a>").is_err());
    }

    #[test]
    fn reads_look_from_sidecar() {
        let sidecar = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="-0.87" crs:CameraProfile="Adobe Standard">
   <crs:Look>
    <rdf:Description crs:Name="Adobe Color" crs:Amount="1" crs:UUID="B952C231111CD8E0ECCF14B86BAA7077"
     crs:SupportsAmount="false" crs:SupportsMonochrome="false" crs:SupportsOutputReferred="false">
    <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
    <crs:Parameters>
     <rdf:Description crs:Version="17.1" crs:ProcessVersion="15.4" crs:ConvertToGrayscale="False"
      crs:CameraProfile="Adobe Standard" crs:LookTable="E1095149FDB39D7A057BAB208837E2E1">
     <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>22, 16</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
     </rdf:Description>
    </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let l = LookProfile::from_sidecar(sidecar).unwrap().unwrap();
        assert_eq!(l.uuid, "B952C231111CD8E0ECCF14B86BAA7077");
        assert_eq!(l.name, "Adobe Color");
        assert_eq!(l.group, "Profiles");
        assert!(!l.monochrome);
        assert_eq!(l.parameters.tone_curve.point.master[1], [22.0, 16.0]);
        assert_eq!(l.parameters.exposure, 0.0, "the sidecar's own settings do not leak in");
        assert!(l.tables.is_empty());
        assert!(LookProfile::from_sidecar(LOOK).unwrap().is_none());
    }

    #[test]
    #[ignore = "needs Adobe Camera Raw look profiles installed"]
    fn parses_installed_adobe_color() {
        let path = "/Library/Application Support/Adobe/CameraRaw/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp";
        let l = LookProfile::parse_file(&std::fs::read_to_string(path).unwrap()).unwrap().unwrap();
        assert_eq!(l.uuid, "B952C231111CD8E0ECCF14B86BAA7077");
        assert_eq!(l.name, "Adobe Color");
        assert_eq!(l.tables.len(), 1);
        assert_eq!(l.parameters.tone_curve.point.master.len(), 7);
    }
}
