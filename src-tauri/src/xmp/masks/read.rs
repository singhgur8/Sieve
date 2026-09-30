//! Reader: top-level `crs:MaskGroupBasedCorrections` -> [`Parsed`] (model + source spans).

use std::collections::HashSet;

use super::tree::{Tree, NS_RDF};
use super::{
    local_field_mut, md5, LightroomMatte, AI_SUBTYPES, BLEND_MODES, LANDSCAPE_CATEGORIES, LOCAL_CURVES,
    LOCAL_SCALARS, PERSON_PARTS,
};
use crate::ipc::types::{
    is_mask_id, AiMask, AiTarget, AiTargetKind, BrushMask, BrushStroke, ColorRange, ColorSample, DevelopWarning,
    DevelopWarningCode, LinearMask, LocalAdjustments, LuminanceRange, MaskBlendMode, MaskComponent, MaskGroup,
    MaskLimits, MaskShape, NormPoint, NormRect, PointCurves, RadialMask, UnsupportedMask,
};
use crate::xmp::crs::{self, CRS_NS};

/// Legacy (pre-Lightroom 11) local correction lists: reported, preserved, not rendered.
pub const LEGACY_CORRECTIONS: &[&str] =
    &["GradientBasedCorrections", "CircularGradientBasedCorrections", "PaintBasedCorrections"];

/// One component with the `rdf:li` items it came from (a brush spans several items).
#[derive(Debug, Clone)]
pub struct ParsedComponent {
    pub component: MaskComponent,
    pub items: Vec<usize>,
    /// `crs:MaskDigest` of the (first) item, whether or not its table exists.
    pub raw_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedGroup {
    pub group: MaskGroup,
    /// The group's `rdf:li`.
    pub li: usize,
    pub components: Vec<ParsedComponent>,
}

/// The packet's masks with their source structure.
pub struct Parsed {
    pub tree: Tree,
    /// Top-level `crs:MaskGroupBasedCorrections` element, if any.
    pub element: Option<usize>,
    pub groups: Vec<ParsedGroup>,
    pub mattes: Vec<LightroomMatte>,
    pub warnings: Vec<DevelopWarning>,
    /// Some mask-related property exists (masks element or legacy corrections).
    pub any: bool,
    /// `crs:Table_<d>` attributes of top-level descriptions: (description, attribute index, d).
    pub tables: Vec<(usize, usize, String)>,
}

impl Parsed {
    pub fn models(&self) -> Vec<MaskGroup> {
        self.groups.iter().map(|g| g.group.clone()).collect()
    }
}

fn num(s: &str) -> Option<f32> {
    let v = s.trim().trim_start_matches('+').parse::<f32>().ok()?;
    v.is_finite().then_some(v)
}

fn int(s: &str) -> Option<i32> {
    let t = s.trim().trim_start_matches('+');
    t.parse::<i32>().ok().or_else(|| num(t).filter(|v| v.fract() == 0.0).map(|v| v as i32))
}

pub fn boolean(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn numbers(s: &str) -> Vec<f32> {
    s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).filter_map(num).collect()
}

/// `"a/b,c/d,..."` rationals (or plain numbers).
pub fn rationals(s: &str) -> Vec<f64> {
    s.split(',')
        .filter_map(|t| {
            let t = t.trim();
            match t.split_once('/') {
                Some((n, d)) => {
                    let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
                    (d != 0.0).then_some(n / d)
                }
                None => t.parse::<f64>().ok(),
            }
        })
        .collect()
}

/// `v * 100` without f32 noise (0.3 -> 30).
fn hundred(v: f32) -> f32 {
    ((f64::from(v) * 100.0 * 1e4).round() / 1e4) as f32
}

/// `b - a` rounded to 1e-6 (the written precision).
fn span(a: f32, b: f32) -> f32 {
    (((f64::from(b) - f64::from(a)) * 1e6).round() / 1e6) as f32
}

fn truncate_name(s: &str) -> String {
    s.chars().take(MaskLimits::MAX_NAME).collect()
}

fn point(s: &str) -> Option<NormPoint> {
    let v = numbers(s);
    (v.len() >= 2).then(|| NormPoint { x: v[0], y: v[1] })
}

/// Parses `src` (BOM already stripped).
pub fn parse(src: &str) -> Result<Parsed, String> {
    let tree = Tree::parse(src)?;
    let tops = tree.top_descriptions();
    let mut tables = Vec::new();
    for &d in &tops {
        for (ai, a) in tree.node(d).attrs.iter().enumerate() {
            if a.uri == CRS_NS {
                if let Some(digest) = a.local.strip_prefix("Table_") {
                    tables.push((d, ai, digest.to_owned()));
                }
            }
        }
    }
    let element = tops.iter().find_map(|&d| tree.child(d, CRS_NS, "MaskGroupBasedCorrections"));
    let mut legacy = 0usize;
    for &d in &tops {
        for name in LEGACY_CORRECTIONS {
            if let Some(e) = tree.child(d, CRS_NS, name) {
                legacy += tree.items(e).len().max(1);
            }
        }
    }
    let mut parsed = Parsed {
        tree,
        element,
        groups: Vec::new(),
        mattes: Vec::new(),
        warnings: Vec::new(),
        any: element.is_some() || legacy > 0,
        tables,
    };
    let mut unsupported = legacy;
    if let Some(e) = element {
        let mut ids: HashSet<String> = HashSet::new();
        let group_items = parsed.tree.items(e);
        for (gi, li) in group_items.into_iter().enumerate() {
            let g = parse_group(src, &parsed, li, gi, &mut ids);
            unsupported +=
                g.components.iter().filter(|c| matches!(c.component.shape, MaskShape::Unsupported(_))).count();
            for c in &g.components {
                if let MaskShape::Ai(ai) = &c.component.shape {
                    if let Some(d) = &ai.digest {
                        if let Some(m) = matte_of(&parsed, c, ai, d) {
                            parsed.mattes.push(m);
                        }
                    }
                }
            }
            parsed.groups.push(g);
        }
    }
    if unsupported > 0 {
        parsed.warnings.push(DevelopWarning {
            code: DevelopWarningCode::MasksUnsupported,
            detail: Some(unsupported.to_string()),
        });
    }
    Ok(parsed)
}

fn table_value(p: &Parsed, digest: &str) -> Option<String> {
    p.tables.iter().find(|(_, _, d)| d == digest).map(|(desc, ai, _)| p.tree.node(*desc).attrs[*ai].value.clone())
}

fn matte_of(p: &Parsed, c: &ParsedComponent, ai: &AiMask, digest: &str) -> Option<LightroomMatte> {
    let item = *c.items.first()?;
    let t = &p.tree;
    let field = |name: &str| t.field(item, CRS_NS, name);
    let area = field("WholeImageArea").map(|s| rationals(&s)).filter(|v| v.len() == 4).unwrap_or_default();
    let whole_area = if area.len() == 4 { [area[0], area[1], area[2], area[3]] } else { [0.0; 4] };
    let origin = field("Origin").map(|s| rationals(&s)).filter(|v| v.len() == 2).unwrap_or_else(|| vec![0.0, 0.0]);
    Some(LightroomMatte {
        digest: digest.to_owned(),
        kind: ai.cache_kind(),
        target: ai.target.clone(),
        reference_point: ai.reference_point,
        model_version: field("ModelVersion"),
        input_digest: field("InputDigest"),
        whole_area,
        origin: [origin[0], origin[1]],
        table: table_value(p, digest)?,
    })
}

/// A unique valid id: the SyncID when usable, else one derived from the item's source.
fn unique_id(raw: Option<String>, src_text: &str, salt: usize, ids: &mut HashSet<String>) -> String {
    let mut id = raw.map(|s| s.trim().to_ascii_uppercase()).filter(|s| is_mask_id(s) && !ids.contains(s));
    let mut n = salt;
    while id.is_none() {
        let cand = md5::md5_hex(format!("{n}:{src_text}").as_bytes());
        if !ids.contains(&cand) {
            id = Some(cand);
        }
        n += 1000;
    }
    let id = id.unwrap_or_default();
    ids.insert(id.clone());
    id
}

fn parse_group(src: &str, p: &Parsed, li: usize, gi: usize, ids: &mut HashSet<String>) -> ParsedGroup {
    let t = &p.tree;
    let field = |name: &str| t.field(li, CRS_NS, name);
    let node = t.node(li);
    let id = unique_id(field("CorrectionSyncID"), &src[node.start..node.end], gi, ids);
    let mut adjustments = LocalAdjustments::default();
    for s in LOCAL_SCALARS {
        if let Some(v) = field(s.property).as_deref().and_then(num) {
            if let Some((slot, lo, hi)) = local_field_mut(&mut adjustments, s.field) {
                // f64 + 1e-4 rounding: "-0.3" / 0.01 reads as -30, not -30.000002.
                let ui = (f64::from(v) / f64::from(s.scale) * 1e4).round() / 1e4;
                *slot = (ui as f32).clamp(lo, hi);
            }
        }
    }
    for (prop, which) in LOCAL_CURVES {
        if let Some(e) = t.field_elem(li, CRS_NS, prop) {
            let items: Vec<String> = t.items(e).iter().map(|&i| t.node(i).text.clone()).collect();
            if let Ok(curve) = crs::parse_curve(prop, &items) {
                if PointCurves::validate_curve(prop, &curve).is_ok() {
                    let c = &mut adjustments.tone_curve;
                    match *which {
                        "master" => c.master = curve,
                        "red" => c.red = curve,
                        "green" => c.green = curve,
                        _ => c.blue = curve,
                    }
                }
            }
        }
    }
    let mut components: Vec<ParsedComponent> = Vec::new();
    if let Some(masks) = t.field_elem(li, CRS_NS, "CorrectionMasks") {
        let items = t.items(masks);
        let mut dabs_total = 0usize;
        for (ci, item) in items.into_iter().enumerate() {
            let what = t.field(item, CRS_NS, "What").unwrap_or_default();
            if what == "Mask/Paint" {
                if let Some(last) = components.last_mut() {
                    if joins_brush(t, item, last) {
                        if let MaskShape::Brush(b) = &mut last.component.shape {
                            let strokes = paint_strokes(t, item, &mut dabs_total);
                            b.strokes.extend(strokes);
                            last.items.push(item);
                            continue;
                        }
                    }
                }
            }
            let c = parse_component(src, p, item, &what, gi * 1000 + ci, ids, &mut dabs_total);
            components.push(c);
        }
        // Brush components whose strokes were all empty (no dabs) are not renderable.
        for c in &mut components {
            if let MaskShape::Brush(b) = &c.component.shape {
                if b.strokes.is_empty() {
                    c.component.shape = MaskShape::Unsupported(UnsupportedMask { what: "Mask/Paint".into() });
                }
            }
        }
    }
    let components = components.into_iter().take(MaskLimits::MAX_COMPONENTS).collect::<Vec<_>>();
    ParsedGroup {
        group: MaskGroup {
            id,
            name: truncate_name(&field("CorrectionName").unwrap_or_default()),
            active: field("CorrectionActive").as_deref().and_then(boolean).unwrap_or(true),
            amount: field("CorrectionAmount").as_deref().and_then(num).unwrap_or(1.0).clamp(0.0, 2.0),
            adjustments,
            components: components.iter().map(|c| c.component.clone()).collect(),
        },
        li,
        components,
    }
}

/// A `Mask/Paint` item continues the previous brush component: same non-empty name, or no
/// name / no SyncID of its own.
fn joins_brush(t: &Tree, item: usize, last: &ParsedComponent) -> bool {
    if !matches!(last.component.shape, MaskShape::Brush(_)) {
        return false;
    }
    let name = t.field(item, CRS_NS, "MaskName");
    let sync = t.field(item, CRS_NS, "MaskSyncID");
    match (name, sync) {
        (None, _) | (_, None) => true,
        (Some(n), Some(_)) => !n.is_empty() && n == last.component.name,
    }
}

fn paint_strokes(t: &Tree, item: usize, dabs_total: &mut usize) -> Vec<BrushStroke> {
    let f = |name: &str| t.field(item, CRS_NS, name).as_deref().and_then(num);
    let radius = f("Radius").unwrap_or(0.01).clamp(1e-6, 1.0);
    let flow = f("Flow").unwrap_or(1.0).clamp(0.0, 1.0);
    let feather = (1.0 - f("CenterWeight").unwrap_or(1.0)).clamp(0.0, 1.0);
    let value = f("MaskValue").unwrap_or(1.0).clamp(0.0, 1.0);
    let (erase, density) = if value <= 0.0 { (true, 1.0) } else { (false, value) };
    let base = BrushStroke { radius, flow, feather, density, erase, auto_mask: false, dabs: Vec::new() };
    let mut strokes = Vec::new();
    let mut cur = base.clone();
    if let Some(d) = t.field_elem(item, CRS_NS, "Dabs") {
        for i in t.items(d) {
            let text = t.node(i).text.trim().to_owned();
            let mut parts = text.split_whitespace();
            match parts.next() {
                Some("d") => {
                    let v: Vec<f32> = parts.filter_map(num).collect();
                    if v.len() >= 2 && *dabs_total < MaskLimits::MAX_DABS {
                        cur.dabs.push(NormPoint { x: v[0].clamp(-1.0, 2.0), y: v[1].clamp(-1.0, 2.0) });
                        *dabs_total += 1;
                    }
                }
                Some("r") => {
                    if let Some(r) = parts.next().and_then(num) {
                        let r = r.clamp(1e-6, 1.0);
                        if !cur.dabs.is_empty() {
                            strokes.push(std::mem::replace(&mut cur, BrushStroke { radius: r, ..base.clone() }));
                        } else {
                            cur.radius = r;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if !cur.dabs.is_empty() {
        strokes.push(cur);
    }
    strokes
}

fn parse_component(
    src: &str,
    p: &Parsed,
    item: usize,
    what: &str,
    salt: usize,
    ids: &mut HashSet<String>,
    dabs_total: &mut usize,
) -> ParsedComponent {
    let t = &p.tree;
    let field = |name: &str| t.field(item, CRS_NS, name);
    let fnum = |name: &str| field(name).as_deref().and_then(num);
    let node = t.node(item);
    let id = unique_id(field("MaskSyncID"), &src[node.start..node.end], salt, ids);
    let unsupported = |w: &str| MaskShape::Unsupported(UnsupportedMask { what: if w.is_empty() { "?".into() } else { w.into() } });
    let mode_code = field("MaskBlendMode").as_deref().and_then(int).unwrap_or(0);
    let mode = BLEND_MODES.iter().find(|b| b.0 == mode_code).map(|b| b.1);
    let mut inverted = field("MaskInverted").as_deref().and_then(boolean).unwrap_or(false);
    let mut opacity = fnum("MaskValue").unwrap_or(1.0).clamp(0.0, 1.0);
    let raw_digest = field("MaskDigest").map(|d| d.trim().to_ascii_uppercase());
    let shape = match (what, mode) {
        (_, None) => unsupported(what),
        ("Mask/Image", _) => {
            let sub_type = field("MaskSubType").as_deref().and_then(int).unwrap_or(1);
            let cats: Vec<i32> = field("MaskSubCategoryID")
                .map(|s| s.split(|c: char| c == ',' || c.is_whitespace()).filter_map(int).collect())
                .unwrap_or_default();
            let sub_category = cats.first().copied();
            let other = AiTarget::Other { sub_type, sub_category };
            let kind = AI_SUBTYPES.iter().find(|s| s.0 == sub_type).map(|s| s.1);
            let target = match kind {
                Some(AiTargetKind::Subject) if cats.is_empty() => AiTarget::Subject,
                Some(AiTargetKind::Sky) if cats.is_empty() => AiTarget::Sky,
                Some(AiTargetKind::People) => {
                    let parts: Option<Vec<_>> =
                        cats.iter().map(|c| PERSON_PARTS.iter().find(|p| p.0 == *c).map(|p| p.1)).collect();
                    match parts {
                        Some(parts) => AiTarget::People { parts },
                        None => other,
                    }
                }
                Some(AiTargetKind::Object) => match field("ObjectRegion").map(|s| numbers(&s)) {
                    Some(v) if v.len() == 4 && v[2] > 0.0 && v[3] > 0.0 => {
                        let r = NormRect { x: v[0], y: v[1], width: v[2], height: v[3] };
                        let ok = r.x >= 0.0 && r.y >= 0.0 && r.x + r.width <= 1.0 + 1e-4 && r.y + r.height <= 1.0 + 1e-4;
                        if ok {
                            AiTarget::Object { region: r }
                        } else {
                            other
                        }
                    }
                    _ => other,
                },
                Some(AiTargetKind::Landscape) if cats.len() == 1 => {
                    match LANDSCAPE_CATEGORIES.iter().find(|c| c.0 == cats[0]) {
                        Some(c) => AiTarget::Landscape { category: c.1 },
                        None => other,
                    }
                }
                _ => other,
            };
            let reference_point = field("ReferencePoint")
                .as_deref()
                .and_then(point)
                .map(|p| NormPoint { x: p.x.clamp(0.0, 1.0), y: p.y.clamp(0.0, 1.0) });
            let digest = raw_digest
                .clone()
                .filter(|d| is_mask_id(d) && p.tables.iter().any(|(_, _, td)| td.eq_ignore_ascii_case(d)));
            MaskShape::Ai(AiMask { target, reference_point, digest })
        }
        ("Mask/Paint", _) => {
            let strokes = paint_strokes(t, item, dabs_total);
            // Stroke density carries MaskValue; the component itself is fully opaque.
            opacity = 1.0;
            MaskShape::Brush(BrushMask { strokes })
        }
        ("Mask/Gradient", _) => match (fnum("ZeroX"), fnum("ZeroY"), fnum("FullX"), fnum("FullY")) {
            (Some(zx), Some(zy), Some(fx), Some(fy)) => {
                let c = |v: f32| v.clamp(-10.0, 11.0);
                let zero = NormPoint { x: c(zx), y: c(zy) };
                let full = NormPoint { x: c(fx), y: c(fy) };
                if zero == full {
                    unsupported(what)
                } else {
                    MaskShape::Linear(LinearMask { zero, full })
                }
            }
            _ => unsupported(what),
        },
        ("Mask/CircularGradient", _) => match (fnum("Top"), fnum("Left"), fnum("Bottom"), fnum("Right")) {
            (Some(top), Some(left), Some(bottom), Some(right)) if left < right && top < bottom => {
                let c = |v: f32| v.clamp(-10.0, 11.0);
                MaskShape::Radial(RadialMask {
                    top: c(top),
                    left: c(left),
                    bottom: c(bottom),
                    right: c(right),
                    angle: fnum("Angle").unwrap_or(0.0).clamp(-360.0, 360.0),
                    midpoint: fnum("Midpoint").unwrap_or(50.0).clamp(0.0, 100.0),
                    roundness: fnum("Roundness").unwrap_or(0.0).clamp(-100.0, 100.0),
                    feather: fnum("Feather").unwrap_or(50.0).clamp(0.0, 100.0),
                    flipped: field("Flipped").as_deref().and_then(boolean).unwrap_or(false),
                })
            }
            _ => unsupported(what),
        },
        ("Mask/RangeMask", _) => parse_range(src, t, item).unwrap_or_else(|| unsupported(what)),
        _ => unsupported(what),
    };
    if matches!(shape, MaskShape::Unsupported(_)) {
        inverted = false;
        opacity = 1.0;
    }
    ParsedComponent {
        component: MaskComponent {
            id,
            name: truncate_name(&field("MaskName").unwrap_or_default()),
            active: field("MaskActive").as_deref().and_then(boolean).unwrap_or(true),
            mode: mode.unwrap_or(MaskBlendMode::Add),
            inverted,
            opacity,
            shape,
        },
        items: vec![item],
        raw_digest,
    }
}

/// `crs:CorrectionRangeMask`: `Type` 1 = colour, 2 = luminance (provisional); others None.
fn parse_range(src: &str, t: &Tree, item: usize) -> Option<MaskShape> {
    let e = t.field_elem(item, CRS_NS, "CorrectionRangeMask")?;
    let f = |name: &str| t.field(e, CRS_NS, name);
    match f("Type").as_deref().and_then(int)? {
        2 => {
            let v = f("LumRange").map(|s| numbers(&s)).unwrap_or_default();
            if v.len() != 4 {
                return None;
            }
            let mut r: Vec<f32> = v.iter().map(|x| x.clamp(0.0, 1.0)).collect();
            r.sort_by(|a, b| a.total_cmp(b));
            let smooth = f("LumFeather").as_deref().and_then(num).map_or(50.0, |v| hundred(v).clamp(0.0, 100.0));
            Some(MaskShape::Luminance(LuminanceRange {
                feather_low: r[0],
                low: r[1],
                high: r[2],
                feather_high: r[3],
                smoothness: smooth,
            }))
        }
        1 => {
            let amount = f("ColorAmount").as_deref().and_then(num).map_or(50.0, |v| hundred(v).clamp(0.0, 100.0));
            let mut samples = Vec::new();
            for (name, is_area) in [("PointModels", false), ("AreaModels", true)] {
                let Some(list) = t.field_elem(e, CRS_NS, name) else { continue };
                for li in t.items(list) {
                    let n = t.node(li);
                    let (model, text) = if n.children.is_empty() && n.attrs.iter().all(|a| a.uri == NS_RDF || a.is_xmlns())
                    {
                        (n.text.trim().to_owned(), n.text.clone())
                    } else {
                        // Structured item: keep its XML; read numbers from its values.
                        let vals: Vec<String> = t
                            .struct_holders(li)
                            .iter()
                            .flat_map(|&h| t.node(h).attrs.iter().filter(|a| a.uri == CRS_NS).map(|a| a.value.clone()))
                            .collect();
                        (src[n.start..n.end].to_owned(), vals.join(" "))
                    };
                    let v = numbers(&text);
                    let sample = if is_area && v.len() >= 4 {
                        let (x0, y0, x1, y1) = (v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3]));
                        let (x0, y0, x1, y1) = (x0.clamp(0.0, 1.0), y0.clamp(0.0, 1.0), x1.clamp(0.0, 1.0), y1.clamp(0.0, 1.0));
                        let area = (x1 > x0 && y1 > y0)
                            .then_some(NormRect { x: x0, y: y0, width: span(x0, x1), height: span(y0, y1) });
                        ColorSample { point: NormPoint { x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 }, area, lightroom_model: None }
                    } else if v.len() >= 2 {
                        ColorSample {
                            point: NormPoint { x: v[0].clamp(0.0, 1.0), y: v[1].clamp(0.0, 1.0) },
                            area: None,
                            lightroom_model: None,
                        }
                    } else {
                        continue;
                    };
                    let model = if model.is_empty() { None } else { Some(model) };
                    samples.push(ColorSample { lightroom_model: model, ..sample });
                }
            }
            samples.truncate(MaskLimits::MAX_COLOR_SAMPLES);
            if samples.is_empty() {
                return None;
            }
            Some(MaskShape::Color(ColorRange { samples, amount }))
        }
        _ => None,
    }
}
