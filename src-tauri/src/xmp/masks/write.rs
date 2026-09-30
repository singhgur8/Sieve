//! Writer: `masks` -> top-level `crs:MaskGroupBasedCorrections` (rules in the parent module
//! docs). Works by splicing the packet text; untouched items are copied byte-for-byte.

use std::collections::{HashMap, HashSet};

use quick_xml::escape::escape;

use super::read::{self, boolean, Parsed, ParsedComponent, ParsedGroup};
use super::tree::{indent_at, line_ws_start, Tree, NS_RDF};
use super::{
    local_field, AI_SUBTYPES, BLEND_MODES, LANDSCAPE_CATEGORIES, LEGACY_LOCAL_ZEROS, LOCAL_CURVES, LOCAL_SCALARS,
    PERSON_PARTS,
};
use crate::ipc::types::{
    AiMask, AiTarget, AiTargetKind, BrushStroke, ColorRange, CurvePoint, LuminanceRange, MaskComponent, MaskGroup,
    MaskShape, PointCurves,
};
use crate::xmp::crs::{self, CRS_NS};

const BOM: &str = "\u{feff}";

/// Attributes tied to a Lightroom matte; dropped when the AI component's selection changes.
pub const DIGEST_ATTRS: &[&str] =
    &["MaskDigest", "InputDigest", "InputDigestVersion", "WholeImageArea", "Origin", "ModelVersion"];

/// Lightroom's attribute order on a Correction (new groups).
const GROUP_ORDER: &[&str] = &[
    "What",
    "CorrectionAmount",
    "CorrectionActive",
    "CorrectionName",
    "CorrectionSyncID",
    "LocalExposure",
    "LocalHue",
    "LocalSaturation",
    "LocalContrast",
    "LocalClarity",
    "LocalSharpness",
    "LocalBrightness",
    "LocalToningHue",
    "LocalToningSaturation",
    "LocalExposure2012",
    "LocalContrast2012",
    "LocalHighlights2012",
    "LocalShadows2012",
    "LocalWhites2012",
    "LocalBlacks2012",
    "LocalClarity2012",
    "LocalDehaze",
    "LocalLuminanceNoise",
    "LocalMoire",
    "LocalDefringe",
    "LocalTemperature",
    "LocalTint",
    "LocalTexture",
    "LocalCurveRefineSaturation",
];

/// Lightroom's number style: up to 6 decimals, no trailing zeros.
pub fn fmt6(v: f32) -> String {
    let s = format!("{:.6}", v);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Num(f32),
    Int(i32),
    Bool(bool),
    Str(String),
    Point(f32, f32),
}

impl Val {
    fn text(&self) -> String {
        match self {
            Val::Num(v) => fmt6(*v),
            Val::Int(v) => v.to_string(),
            Val::Bool(b) => if *b { "true" } else { "false" }.into(),
            Val::Str(s) => s.clone(),
            Val::Point(x, y) => format!("{x:.6} {y:.6}"),
        }
    }

    /// The old value already says the same (keep its bytes).
    fn same_as(&self, old: &str) -> bool {
        let n = |s: &str| s.trim().trim_start_matches('+').parse::<f32>().ok();
        match self {
            Val::Num(v) => n(old).is_some_and(|o| fmt6(o) == fmt6(*v)),
            Val::Int(v) => n(old).is_some_and(|o| o == *v as f32),
            Val::Bool(b) => boolean(old) == Some(*b),
            Val::Str(s) => old == s,
            Val::Point(x, y) => {
                let v: Vec<f32> = old.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                v.len() == 2 && fmt6(v[0]) == fmt6(*x) && fmt6(v[1]) == fmt6(*y)
            }
        }
    }
}

type Modelled = Vec<(String, Option<Val>)>;

fn m(local: &str, v: Val) -> (String, Option<Val>) {
    (local.to_owned(), Some(v))
}

struct Ctx<'a> {
    src: &'a str,
    t: &'a Tree,
    crs: String,
    rdf: String,
    old_groups: HashMap<&'a str, &'a ParsedGroup>,
    old_components: HashMap<&'a str, &'a ParsedComponent>,
    /// Digests referenced by the written element.
    written: HashSet<String>,
}

impl Ctx<'_> {
    fn q(&self, prefix: &str, local: &str) -> String {
        if prefix.is_empty() {
            local.to_owned()
        } else {
            format!("{prefix}:{local}")
        }
    }
    fn c(&self, local: &str) -> String {
        self.q(&self.crs, local)
    }
    fn r(&self, local: &str) -> String {
        self.q(&self.rdf, local)
    }

    fn new_attr(&self, local: &str, v: &Val) -> String {
        format!("{}=\"{}\"", self.c(local), escape(v.text().as_str()))
    }

    /// Old attributes of `holders` in order (modelled ones replaced, `drop` removed),
    /// then modelled attributes that were not present.
    fn merge_attrs(&self, holders: &[usize], modelled: &Modelled, drop: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        let mut used: HashSet<&str> = HashSet::new();
        for &h in holders {
            let n = self.t.node(h);
            for a in &n.attrs {
                if a.uri == NS_RDF {
                    continue;
                }
                let raw = &self.src[a.name_start..a.end];
                if a.uri == CRS_NS {
                    if let Some((k, v)) = modelled.iter().find(|(k, _)| k == &a.local) {
                        used.insert(k.as_str());
                        if let Some(v) = v {
                            out.push(if v.same_as(&a.value) { raw.to_owned() } else { self.new_attr(k, v) });
                        }
                        continue;
                    }
                    if drop.contains(&a.local.as_str()) {
                        continue;
                    }
                }
                out.push(raw.to_owned());
            }
        }
        for (k, v) in modelled {
            if let (false, Some(v)) = (used.contains(k.as_str()), v) {
                out.push(self.new_attr(k, v));
            }
        }
        out
    }

    /// Child elements of `holders` other than `modelled` (crs locals) and structure nodes.
    fn carried_children(&self, holders: &[usize], modelled: &[&str], indent: &str) -> Vec<String> {
        let mut out = Vec::new();
        for &h in holders {
            for (_, n) in self.t.children(h) {
                if n.uri == NS_RDF || (n.uri == CRS_NS && modelled.contains(&n.local.as_str())) {
                    continue;
                }
                out.push(format!("{indent}{}", &self.src[n.start..n.end]));
            }
        }
        out
    }

    /// An `rdf:li` struct item: attribute form when there are no children and `!desc_form`.
    fn item(&self, indent: &str, attrs: &[String], children: &[String], desc_form: bool) -> String {
        let li = self.r("li");
        let mut s = String::new();
        if children.is_empty() && !desc_form {
            s.push_str(&format!("{indent}<{li}"));
            for a in attrs {
                s.push_str(&format!("\n{indent} {a}"));
            }
            s.push_str("/>");
            return s;
        }
        let desc = self.r("Description");
        s.push_str(&format!("{indent}<{li}>\n{indent} <{desc}"));
        for a in attrs {
            s.push_str(&format!("\n{indent}  {a}"));
        }
        if children.is_empty() {
            s.push_str("/>");
        } else {
            s.push('>');
            for c in children {
                s.push('\n');
                s.push_str(c);
            }
            s.push_str(&format!("\n{indent} </{desc}>"));
        }
        s.push_str(&format!("\n{indent}</{li}>"));
        s
    }

    /// `<prop><rdf:Seq><rdf:li>text</rdf:li>...</rdf:Seq></prop>` (texts escaped; items starting
    /// with `<` are raw XML).
    fn seq(&self, indent: &str, prop: &str, items: &[String]) -> String {
        let (seq, li) = (self.r("Seq"), self.r("li"));
        let mut s = format!("{indent}<{prop}>\n{indent} <{seq}>");
        for it in items {
            if it.starts_with('<') {
                s.push_str(&format!("\n{indent}  {it}"));
            } else {
                s.push_str(&format!("\n{indent}  <{li}>{}</{li}>", escape(it.as_str())));
            }
        }
        s.push_str(&format!("\n{indent} </{seq}>\n{indent}</{prop}>"));
        s
    }

    fn note_digests(&mut self, items: &[usize]) {
        for &i in items {
            for h in self.t.struct_holders(i) {
                if let Some(a) = self.t.node(h).attr(CRS_NS, "MaskDigest") {
                    self.written.insert(a.value.trim().to_ascii_uppercase());
                }
            }
        }
    }

    fn verbatim_items(&mut self, pc: &ParsedComponent, indent: &str) -> Vec<String> {
        self.note_digests(&pc.items);
        pc.items.iter().map(|&i| format!("{indent}{}", &self.src[self.t.node(i).start..self.t.node(i).end])).collect()
    }

    fn group(&mut self, g: &MaskGroup, indent: &str) -> String {
        if let Some(old) = self.old_groups.get(g.id.as_str()).copied() {
            if old.group == *g {
                let items: Vec<usize> = old.components.iter().flat_map(|c| c.items.clone()).collect();
                self.note_digests(&items);
                let n = self.t.node(old.li);
                return format!("{indent}{}", &self.src[n.start..n.end]);
            }
        }
        let old = self.old_groups.get(g.id.as_str()).copied();
        let holders: Vec<usize> = old.map(|o| self.t.struct_holders(o.li)).unwrap_or_default();
        let mut modelled: Modelled = vec![
            m("What", Val::Str("Correction".into())),
            m("CorrectionAmount", Val::Num(g.amount)),
            m("CorrectionActive", Val::Bool(g.active)),
            m("CorrectionName", Val::Str(g.name.clone())),
            m("CorrectionSyncID", Val::Str(g.id.clone())),
        ];
        let defaults = crate::ipc::types::LocalAdjustments::default();
        for s in LOCAL_SCALARS {
            let v = local_field(&g.adjustments, s.field);
            let present = holders.iter().any(|&h| self.t.node(h).attr(CRS_NS, s.property).is_some());
            // Existing groups: don't add default-valued sliders Lightroom did not write.
            if old.is_some() && !present && v == local_field(&defaults, s.field) {
                continue;
            }
            modelled.push(m(s.property, Val::Num(v * s.scale)));
        }
        if old.is_none() {
            for z in LEGACY_LOCAL_ZEROS {
                if !modelled.iter().any(|(k, _)| k == z) {
                    modelled.push(m(z, Val::Num(0.0)));
                }
            }
            modelled.sort_by_key(|(k, _)| GROUP_ORDER.iter().position(|o| o == k).unwrap_or(usize::MAX));
        }
        let attrs = self.merge_attrs(&holders, &modelled, &[]);
        let child_indent = format!("{indent} ");
        let mut children = Vec::new();
        // Components.
        let comp_indent = format!("{indent}   ");
        let mut items = Vec::new();
        for c in &g.components {
            items.extend(self.component(c, &comp_indent));
        }
        let (seq, cm) = (self.r("Seq"), self.c("CorrectionMasks"));
        let mut s = format!("{child_indent}<{cm}>\n{child_indent} <{seq}>");
        for it in &items {
            s.push('\n');
            s.push_str(it);
        }
        s.push_str(&format!("\n{child_indent} </{seq}>\n{child_indent}</{cm}>"));
        children.push(s);
        // Per-mask curves (only non-identity ones are written).
        let curves = &g.adjustments.tone_curve;
        for (prop, which) in LOCAL_CURVES {
            let curve: &Vec<CurvePoint> = match *which {
                "master" => &curves.master,
                "red" => &curves.red,
                "green" => &curves.green,
                _ => &curves.blue,
            };
            if PointCurves::is_identity(curve) {
                continue;
            }
            let old_elem = holders.iter().find_map(|&h| self.t.child(h, CRS_NS, prop));
            let same = old_elem.is_some_and(|e| {
                let items: Vec<String> = self.t.items(e).iter().map(|&i| self.t.node(i).text.clone()).collect();
                crs::parse_curve(prop, &items).ok().as_ref() == Some(curve)
            });
            match (same, old_elem) {
                (true, Some(e)) => {
                    let n = self.t.node(e);
                    children.push(format!("{child_indent}{}", &self.src[n.start..n.end]));
                }
                _ => children.push(self.seq(&child_indent, &self.c(prop), &crs::format_curve(curve))),
            }
        }
        let mut modelled_children: Vec<&str> = vec!["CorrectionMasks"];
        modelled_children.extend(LOCAL_CURVES.iter().map(|(p, _)| *p));
        children.extend(self.carried_children(&holders, &modelled_children, &child_indent));
        self.item(indent, &attrs, &children, true)
    }

    fn common(&self, c: &MaskComponent, is_new: bool, what: &str) -> Modelled {
        let code = BLEND_MODES.iter().find(|b| b.1 == c.mode).map_or(0, |b| b.0);
        let mut v = vec![
            m("What", Val::Str(what.into())),
            m("MaskActive", Val::Bool(c.active)),
            m("MaskName", Val::Str(c.name.clone())),
            m("MaskBlendMode", Val::Int(code)),
            m("MaskInverted", Val::Bool(c.inverted)),
            m("MaskSyncID", Val::Str(c.id.clone())),
            m("MaskValue", Val::Num(c.opacity)),
        ];
        if is_new {
            v.push(m("MaskVersion", Val::Int(1)));
        }
        v
    }

    /// Items for one component (a brush writes one item per stroke).
    fn component(&mut self, c: &MaskComponent, indent: &str) -> Vec<String> {
        let old = self.old_components.get(c.id.as_str()).copied();
        if let Some(o) = old {
            if o.component == *c {
                return self.verbatim_items(o, indent);
            }
        }
        let first_holders: Vec<usize> =
            old.and_then(|o| o.items.first()).map(|&i| self.t.struct_holders(i)).unwrap_or_default();
        match &c.shape {
            MaskShape::Unsupported(_) => match old {
                // Only Lightroom components can be unsupported: keep them as they were.
                Some(o) => self.verbatim_items(o, indent),
                None => Vec::new(),
            },
            MaskShape::Ai(ai) => vec![self.ai_item(c, ai, old, &first_holders, indent)],
            MaskShape::Brush(b) => b
                .strokes
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let holders: Vec<usize> =
                        old.and_then(|o| o.items.get(i)).map(|&it| self.t.struct_holders(it)).unwrap_or_default();
                    self.stroke_item(c, s, i, old.is_none(), &holders, indent)
                })
                .collect(),
            MaskShape::Linear(l) => {
                let mut md = self.common(c, old.is_none(), "Mask/Gradient");
                md.extend([
                    m("ZeroX", Val::Num(l.zero.x)),
                    m("ZeroY", Val::Num(l.zero.y)),
                    m("FullX", Val::Num(l.full.x)),
                    m("FullY", Val::Num(l.full.y)),
                ]);
                vec![self.simple_item(&first_holders, &md, &[], indent)]
            }
            MaskShape::Radial(r) => {
                let mut md = self.common(c, old.is_none(), "Mask/CircularGradient");
                md.extend([
                    m("Top", Val::Num(r.top)),
                    m("Left", Val::Num(r.left)),
                    m("Bottom", Val::Num(r.bottom)),
                    m("Right", Val::Num(r.right)),
                    m("Angle", Val::Num(r.angle)),
                    m("Midpoint", Val::Num(r.midpoint)),
                    m("Roundness", Val::Num(r.roundness)),
                    m("Feather", Val::Num(r.feather)),
                    m("Flipped", Val::Bool(r.flipped)),
                ]);
                vec![self.simple_item(&first_holders, &md, &[], indent)]
            }
            MaskShape::Luminance(_) | MaskShape::Color(_) => vec![self.range_item(c, old, &first_holders, indent)],
        }
    }

    fn simple_item(&self, holders: &[usize], md: &Modelled, drop: &[&str], indent: &str) -> String {
        let attrs = self.merge_attrs(holders, md, drop);
        let children = self.carried_children(holders, &[], &format!("{indent} "));
        self.item(indent, &attrs, &children, false)
    }

    fn ai_item(
        &mut self,
        c: &MaskComponent,
        ai: &AiMask,
        old: Option<&ParsedComponent>,
        holders: &[usize],
        indent: &str,
    ) -> String {
        let mut md = self.common(c, old.is_none(), "Mask/Image");
        let fmt_codes = |codes: Vec<i32>| codes.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
        let sub = |k: AiTargetKind| AI_SUBTYPES.iter().find(|s| s.1 == k).map_or(1, |s| s.0);
        let (sub_type, sub_cat, region): (i32, Option<String>, Option<String>) = match &ai.target {
            AiTarget::Subject => (sub(AiTargetKind::Subject), None, None),
            AiTarget::Sky => (sub(AiTargetKind::Sky), None, None),
            AiTarget::Background => {
                // Written as an inverted subject (provisional, see decisions).
                if let Some(e) = md.iter_mut().find(|(k, _)| k == "MaskInverted") {
                    e.1 = Some(Val::Bool(!c.inverted));
                }
                (sub(AiTargetKind::Subject), None, None)
            }
            AiTarget::People { parts } => {
                let codes: Vec<i32> =
                    parts.iter().filter_map(|p| PERSON_PARTS.iter().find(|x| x.1 == *p).map(|x| x.0)).collect();
                (sub(AiTargetKind::People), (!codes.is_empty()).then(|| fmt_codes(codes)), None)
            }
            AiTarget::Object { region } => (
                sub(AiTargetKind::Object),
                None,
                Some(format!("{} {} {} {}", fmt6(region.x), fmt6(region.y), fmt6(region.width), fmt6(region.height))),
            ),
            AiTarget::Landscape { category } => {
                let code = LANDSCAPE_CATEGORIES.iter().find(|x| x.1 == *category).map_or(1, |x| x.0);
                (sub(AiTargetKind::Landscape), Some(code.to_string()), None)
            }
            AiTarget::Other { sub_type, sub_category } => (*sub_type, sub_category.map(|c| c.to_string()), None),
        };
        md.push(m("MaskSubType", Val::Int(sub_type)));
        let old_cat =
            holders.iter().find_map(|&h| self.t.node(h).attr(CRS_NS, "MaskSubCategoryID")).map(|a| a.value.clone());
        md.push((
            "MaskSubCategoryID".into(),
            sub_cat.map(|s| {
                // Keep Lightroom's own list formatting when the codes are the same.
                match &old_cat {
                    Some(o)
                        if o.split(|ch: char| ch == ',' || ch.is_whitespace())
                            .filter(|t| !t.is_empty())
                            .collect::<Vec<_>>()
                            .join(",")
                            == s =>
                    {
                        Val::Str(o.clone())
                    }
                    _ => Val::Str(s),
                }
            }),
        ));
        md.push(("ObjectRegion".into(), region.map(Val::Str)));
        md.push(("ReferencePoint".into(), ai.reference_point.map(|p| Val::Point(p.x, p.y))));
        // Keep the Lightroom matte attributes only while the selection is unchanged.
        let keep = old.is_some_and(|o| match &o.component.shape {
            MaskShape::Ai(oa) => {
                oa.target == ai.target
                    && oa.reference_point == ai.reference_point
                    && oa.digest == ai.digest
                    && (ai.digest.is_none() || ai.digest == o.raw_digest)
            }
            _ => false,
        });
        let drop: &[&str] = if keep { &[] } else { DIGEST_ATTRS };
        if keep {
            if let Some(d) = old.and_then(|o| o.raw_digest.clone()) {
                self.written.insert(d);
            }
        }
        self.simple_item(holders, &md, drop, indent)
    }

    fn stroke_item(
        &self,
        c: &MaskComponent,
        s: &BrushStroke,
        i: usize,
        is_new: bool,
        holders: &[usize],
        indent: &str,
    ) -> String {
        let mut md = self.common(c, is_new, "Mask/Paint");
        let value = if s.erase { 0.0 } else { s.density * c.opacity };
        for (k, v) in md.iter_mut() {
            match k.as_str() {
                "MaskValue" => *v = Some(Val::Num(value)),
                // Later strokes: no SyncID of their own (the reader joins them to the first).
                "MaskSyncID" if i > 0 => *v = None,
                "MaskName" if i > 0 && c.name.is_empty() => *v = None,
                _ => {}
            }
        }
        if i > 0 {
            // Keep a later item's own SyncID if Lightroom gave it one.
            md.retain(|(k, _)| k != "MaskSyncID");
        }
        md.extend([
            m("Radius", Val::Num(s.radius)),
            m("Flow", Val::Num(s.flow)),
            m("CenterWeight", Val::Num(1.0 - s.feather)),
        ]);
        let attrs = self.merge_attrs(holders, &md, &[]);
        let ci = format!("{indent} ");
        let dabs: Vec<String> = s.dabs.iter().map(|d| format!("d {:.6} {:.6}", d.x, d.y)).collect();
        let mut children = vec![self.seq(&ci, &self.c("Dabs"), &dabs)];
        children.extend(self.carried_children(holders, &["Dabs"], &ci));
        self.item(indent, &attrs, &children, true)
    }

    fn range_item(&self, c: &MaskComponent, old: Option<&ParsedComponent>, holders: &[usize], indent: &str) -> String {
        let md = self.common(c, old.is_none(), "Mask/RangeMask");
        let attrs = self.merge_attrs(holders, &md, &[]);
        let ci = format!("{indent} ");
        let old_elem = holders.iter().find_map(|&h| self.t.child(h, CRS_NS, "CorrectionRangeMask"));
        let same_shape = old.is_some_and(|o| o.component.shape == c.shape);
        let range = match (same_shape, old_elem) {
            (true, Some(e)) => {
                let n = self.t.node(e);
                format!("{ci}{}", &self.src[n.start..n.end])
            }
            _ => self.range_elem(&c.shape, old_elem, &ci),
        };
        let mut children = vec![range];
        children.extend(self.carried_children(holders, &["CorrectionRangeMask"], &ci));
        self.item(indent, &attrs, &children, true)
    }

    fn range_elem(&self, shape: &MaskShape, old: Option<usize>, indent: &str) -> String {
        let holders = old.map(|e| self.t.struct_holders(e)).unwrap_or_default();
        let mut md: Modelled = Vec::new();
        if old.is_none() {
            md.push(m("Version", Val::Int(3)));
        }
        let mut lists: Vec<(&str, Vec<String>)> = Vec::new();
        match shape {
            MaskShape::Luminance(LuminanceRange { feather_low, low, high, feather_high, smoothness }) => {
                md.push(m("Type", Val::Int(2)));
                let r = [feather_low, low, high, feather_high].map(|v| fmt6(*v)).join(" ");
                md.push(m("LumRange", Val::Str(r)));
                md.push(m("LumFeather", Val::Num(smoothness / 100.0)));
            }
            MaskShape::Color(ColorRange { samples, amount }) => {
                md.push(m("Type", Val::Int(1)));
                md.push(m("ColorAmount", Val::Num(amount / 100.0)));
                let (mut points, mut areas) = (Vec::new(), Vec::new());
                for s in samples {
                    let text = match (&s.lightroom_model, &s.area) {
                        (Some(model), _) => model.clone(),
                        (None, Some(a)) => {
                            format!("{} {} {} {}", fmt6(a.x), fmt6(a.y), fmt6(a.x + a.width), fmt6(a.y + a.height))
                        }
                        (None, None) => format!("{} {}", fmt6(s.point.x), fmt6(s.point.y)),
                    };
                    if s.area.is_some() {
                        areas.push(text);
                    } else {
                        points.push(text);
                    }
                }
                if !points.is_empty() {
                    lists.push(("PointModels", points));
                }
                if !areas.is_empty() {
                    lists.push(("AreaModels", areas));
                }
            }
            _ => {}
        }
        let attrs = self.merge_attrs(&holders, &md, &["LumRange", "LumFeather", "ColorAmount"]);
        let ci = format!("{indent} ");
        let mut children: Vec<String> = lists.iter().map(|(p, items)| self.seq(&ci, &self.c(p), items)).collect();
        children.extend(self.carried_children(&holders, &["PointModels", "AreaModels"], &ci));
        let name = self.c("CorrectionRangeMask");
        if children.is_empty() {
            let mut s = format!("{indent}<{name}");
            for a in &attrs {
                s.push_str(&format!("\n{indent} {a}"));
            }
            s.push_str("/>");
            s
        } else {
            let desc = self.r("Description");
            let mut s = format!("{indent}<{name}>\n{indent} <{desc}");
            for a in &attrs {
                s.push_str(&format!("\n{indent}  {a}"));
            }
            s.push('>');
            for c in &children {
                s.push('\n');
                s.push_str(c);
            }
            s.push_str(&format!("\n{indent} </{desc}>\n{indent}</{name}>"));
            s
        }
    }
}

/// See [`super::apply`].
pub fn apply(packet: &str, masks: &[MaskGroup]) -> Result<String, String> {
    let (bom, src) = match packet.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => ("", packet),
    };
    let parsed: Parsed = read::parse(src)?;
    if parsed.models().as_slice() == masks {
        return Ok(packet.to_owned());
    }
    let t = &parsed.tree;
    let tops = t.top_descriptions();
    let host = match parsed.element {
        Some(e) => t.node(e).parent.ok_or("masks element without a parent")?,
        None => *tops
            .iter()
            .find(|&&d| t.node(d).attrs.iter().any(|a| a.uri == CRS_NS))
            .or(tops.first())
            .ok_or("XMP packet has no rdf:Description")?,
    };
    let host_node = t.node(host);
    let rdf = host_node.prefix_for(NS_RDF).ok_or("rdf namespace is not declared")?;
    let mut splices: Vec<(usize, usize, String)> = Vec::new();
    let crs = match host_node.prefix_for(CRS_NS) {
        Some(p) => p,
        None => {
            let mut p = "crs".to_owned();
            let mut n = 1;
            while host_node.scope.contains_key(&p) {
                p = format!("crs{n}");
                n += 1;
            }
            let at = host_node.start
                + 1
                + host_node.prefix.len()
                + usize::from(!host_node.prefix.is_empty())
                + host_node.local.len();
            splices.push((at, at, format!(" xmlns:{p}=\"{CRS_NS}\"")));
            p
        }
    };
    let mut ctx = Ctx {
        src,
        t,
        crs,
        rdf,
        old_groups: parsed.groups.iter().map(|g| (g.group.id.as_str(), g)).collect(),
        old_components: parsed
            .groups
            .iter()
            .flat_map(|g| &g.components)
            .map(|c| (c.component.id.as_str(), c))
            .collect(),
        written: HashSet::new(),
    };
    if masks.is_empty() {
        if let Some(e) = parsed.element {
            let n = t.node(e);
            splices.push((line_ws_start(src, n.start), n.end, String::new()));
        }
    } else {
        let indent = match parsed.element {
            Some(e) => indent_at(src, t.node(e).start),
            None => format!("{} ", indent_at(src, host_node.start)),
        };
        let name = ctx.c("MaskGroupBasedCorrections");
        let seq = ctx.r("Seq");
        let gi = format!("{indent}  ");
        let mut text = format!("<{name}>\n{indent} <{seq}>");
        for g in masks {
            text.push('\n');
            text.push_str(&ctx.group(g, &gi));
        }
        text.push_str(&format!("\n{indent} </{seq}>\n{indent}</{name}>"));
        match parsed.element {
            Some(e) => {
                let n = t.node(e);
                splices.push((n.start, n.end, text));
            }
            None if host_node.close_start < host_node.end && host_node.open_end < host_node.end => {
                let at = line_ws_start(src, host_node.close_start);
                splices.push((at, at, format!("\n{indent}{text}")));
            }
            None => {
                // `<rdf:Description .../>`: open it.
                let end = host_node.end;
                if !src[..end].ends_with("/>") {
                    return Err("unexpected rdf:Description form".into());
                }
                let desc_indent = indent_at(src, host_node.start);
                let qn = &src[host_node.start + 1
                    ..host_node.start
                        + 1
                        + host_node.prefix.len()
                        + usize::from(!host_node.prefix.is_empty())
                        + host_node.local.len()];
                splices.push((end - 2, end, format!(">\n{indent}{text}\n{desc_indent}</{qn}>")));
            }
        }
    }
    // Tables of Lightroom mattes that are no longer referenced.
    let old_digests: HashSet<String> =
        parsed.groups.iter().flat_map(|g| &g.components).filter_map(|c| c.raw_digest.clone()).collect();
    for (desc, ai, d) in &parsed.tables {
        let d = d.to_ascii_uppercase();
        if old_digests.contains(&d) && !ctx.written.contains(&d) {
            let a = &t.node(*desc).attrs[*ai];
            splices.push((a.ws_start, a.end, String::new()));
        }
    }
    splices.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = src.to_owned();
    let mut last = usize::MAX;
    for (s, e, text) in splices {
        if e > last {
            return Err("overlapping mask edits".into());
        }
        out.replace_range(s..e, &text);
        last = s;
    }
    Ok(format!("{bom}{out}"))
}
