//! Reader/writer tests on synthetic packets (Lightroom layout); the user's sidecars are
//! covered by the ignored `real_sidecars` test.

use super::table::{decode_base85, decode_matte as decode_lr_matte, encode_base85, placement};
use super::*;
use crate::xmp::packet;
use crate::ipc::types::{
    validate_masks, AiMask, BrushMask, BrushStroke, ColorRange, ColorSample, DevelopWarningCode, LinearMask,
    LuminanceRange, MaskComponent, MaskShape, NormRect, RadialMask,
};

const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";

fn id(n: u32) -> String {
    format!("{n:032X}")
}

/// A Lightroom-style sidecar: one Adaptive-subject group with a matte table, a retouch table,
/// a preset copy of the group, and other develop settings.
fn lightroom_packet() -> String {
    let table = encode_base85(&matte_table_bytes(4, 2, &[0, 64, 128, 255, 255, 255, 10, 20]));
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="{CRS}"
   xmp:Rating="2"
   crs:Version="17.1"
   crs:Exposure2012="-1.42"
   crs:Table_E71A59AFC894F4F898F72751E30113DA="{table}"
   crs:Table_72332FE69583D719E860C3ABD10EDA83="retouchdata"
   crs:HasSettings="True">
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 14</rdf:li>
     <rdf:li>255, 252</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description
       crs:What="Correction"
       crs:CorrectionAmount="1.25"
       crs:CorrectionActive="true"
       crs:CorrectionName="Cool Soft"
       crs:CorrectionSyncID="5601D9FAFB88361404A96FC371E51C68"
       crs:LocalExposure="0"
       crs:LocalExposure2012="0.0825"
       crs:LocalClarity2012="-0.195091"
       crs:LocalTemperature="-0.198124"
       crs:LocalTexture="-0.149141"
       crs:LocalCurveRefineSaturation="100">
      <crs:CorrectionMasks>
       <rdf:Seq>
        <rdf:li
         crs:What="Mask/Image"
         crs:MaskActive="true"
         crs:MaskName="Subject 1"
         crs:MaskBlendMode="0"
         crs:MaskInverted="false"
         crs:MaskSyncID="C4D6A34764A442CCB7F70C2EA1C7188A"
         crs:MaskValue="1"
         crs:MaskVersion="1"
         crs:MaskSubType="1"
         crs:ReferencePoint="0.136719 0.363636"
         crs:InputDigest="2C64DEBD90C90A49E63D44330035F83E"
         crs:InputDigestVersion="2"
         crs:MaskDigest="E71A59AFC894F4F898F72751E30113DA"
         crs:WholeImageArea="0/1,0/1,1920/1,2880/1"
         crs:Origin="0,296"
         crs:ModelVersion="251659306"/>
       </rdf:Seq>
      </crs:CorrectionMasks>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
   <crs:RetouchAreas>
    <rdf:Seq>
     <rdf:li>
      <rdf:Description crs:SpotType="heal_patchmatch" crs:pm_patch_mask="72332FE69583D719E860C3ABD10EDA83">
      <crs:Masks>
       <rdf:Seq>
        <rdf:li>
         <rdf:Description crs:What="Mask/Paint" crs:MaskSyncID="60E7A801A5DA46F2BC17DA8530896209" crs:MaskValue="1" crs:Radius="0.02" crs:Flow="1" crs:CenterWeight="1">
         <crs:Dabs>
          <rdf:Seq>
           <rdf:li>d 0.42 0.79</rdf:li>
          </rdf:Seq>
         </crs:Dabs>
         </rdf:Description>
        </rdf:li>
       </rdf:Seq>
      </crs:Masks>
      </rdf:Description>
     </rdf:li>
    </rdf:Seq>
   </crs:RetouchAreas>
   <crs:Preset>
    <rdf:Description crs:Name="Adaptive Subject - Cool Soft" crs:UUID="559E5E6432824A2A9B9EEEF8FAA7CDD5">
    <crs:Parameters>
     <rdf:Description crs:Version="17.1">
     <crs:MaskGroupBasedCorrections>
      <rdf:Seq>
       <rdf:li>
        <rdf:Description crs:What="Correction" crs:CorrectionName="Cool Soft" crs:CorrectionSyncID="5601D9FAFB88361404A96FC371E51C68">
        <crs:CorrectionMasks>
         <rdf:Seq>
          <rdf:li crs:What="Mask/Image" crs:MaskSyncID="C4D6A34764A442CCB7F70C2EA1C7188A" crs:MaskSubType="1" crs:ReferencePoint="0.500000 0.500000" crs:ErrorReason="0"/>
         </rdf:Seq>
        </crs:CorrectionMasks>
        </rdf:Description>
       </rdf:li>
      </rdf:Seq>
     </crs:MaskGroupBasedCorrections>
     </rdf:Description>
    </crs:Parameters>
    </rdf:Description>
   </crs:Preset>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#
    )
}

/// 16-byte header + little-endian TIFF with one uncompressed 8-bit strip.
fn matte_table_bytes(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let mut t: Vec<u8> = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    let entries: [(u16, u16, u32); 8] = [
        (256, 4, w),
        (257, 4, h),
        (258, 3, 8),
        (259, 3, 1),
        (262, 3, 1),
        (273, 4, 0), // patched below
        (277, 3, 1),
        (279, 4, px.len() as u32),
    ];
    t.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let data_off = 8 + 2 + 12 * entries.len() + 4;
    for (tag, typ, v) in entries {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&typ.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        let v = if tag == 273 { data_off as u32 } else { v };
        if typ == 3 {
            t.extend_from_slice(&(v as u16).to_le_bytes());
            t.extend_from_slice(&[0, 0]);
        } else {
            t.extend_from_slice(&v.to_le_bytes());
        }
    }
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(px);
    let mut out = Vec::new();
    for v in [2u32, 1, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&t);
    out
}

fn attr_of<'a>(packet: &'a str, name: &str) -> Vec<&'a str> {
    let key = format!("{name}=\"");
    packet.match_indices(&key).map(|(i, _)| {
        let rest = &packet[i + key.len()..];
        &rest[..rest.find('"').unwrap()]
    })
    .collect()
}

#[test]
fn base85_round_trips_every_tail_length() {
    for n in 0..13usize {
        let bytes: Vec<u8> = (0..n as u8).map(|b| b.wrapping_mul(37).wrapping_add(200)).collect();
        let text = encode_base85(&bytes);
        assert_eq!(text.len(), n / 4 * 5 + if n % 4 == 0 { 0 } else { n % 4 + 1 });
        assert_eq!(decode_base85(&text).unwrap(), bytes, "len {n}");
        // Whitespace (line breaks) is ignored.
        let spaced: String = text.chars().flat_map(|c| [c, '\n']).collect();
        assert_eq!(decode_base85(&spaced).unwrap(), bytes);
    }
    // Header of Lightroom mattes: "2000001000000000000..." = u32 LE 2, 1, 0, 0.
    let h = decode_base85("20000100000000000000").unwrap();
    assert_eq!(h, [2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(decode_base85("ab\"cd").is_err());
    assert!(decode_base85("123456").is_err(), "a single dangling character is invalid");
}

#[test]
fn reads_top_level_groups_and_ignores_preset_copies() {
    let p = lightroom_packet();
    let r = read(&p).unwrap().unwrap();
    assert_eq!(r.groups.len(), 1, "the preset copy is not image state");
    let g = &r.groups[0];
    assert_eq!(g.id, "5601D9FAFB88361404A96FC371E51C68");
    assert_eq!((g.name.as_str(), g.active, g.amount), ("Cool Soft", true, 1.25));
    let a = &g.adjustments;
    assert!((a.clarity + 19.5091).abs() < 1e-3 && (a.temperature + 19.8124).abs() < 1e-3);
    assert!((a.texture + 14.9141).abs() < 1e-3);
    assert!((a.exposure - 0.33).abs() < 1e-4, "LocalExposure2012 is EV / 4");
    assert_eq!(a.curve_refine_saturation, 100.0);
    assert_eq!(g.components.len(), 1);
    let c = &g.components[0];
    assert_eq!((c.id.as_str(), c.name.as_str(), c.mode, c.inverted, c.opacity), ("C4D6A34764A442CCB7F70C2EA1C7188A", "Subject 1", MaskBlendMode::Add, false, 1.0));
    match &c.shape {
        MaskShape::Ai(ai) => {
            assert_eq!(ai.target, AiTarget::Subject);
            assert_eq!(ai.reference_point, Some(NormPoint { x: 0.136719, y: 0.363636 }));
            assert_eq!(ai.digest.as_deref(), Some("E71A59AFC894F4F898F72751E30113DA"));
        }
        s => panic!("{s:?}"),
    }
    assert_eq!(r.mattes.len(), 1);
    let m = &r.mattes[0];
    assert_eq!(m.kind, "subject");
    assert_eq!(m.whole_area, [0.0, 0.0, 1920.0, 2880.0]);
    assert_eq!(m.origin, [0.0, 296.0]);
    assert_eq!(m.model_version.as_deref(), Some("251659306"));
    assert_eq!(m.input_digest.as_deref(), Some("2C64DEBD90C90A49E63D44330035F83E"));
    assert!(r.warnings.is_empty());
    validate_masks(&r.groups).unwrap();
    // Without its table the digest reads as missing (recompute).
    let no_table = p.replace("crs:Table_E71A59AFC894F4F898F72751E30113DA", "crs:Table_00000000000000000000000000000000");
    let r2 = read(&no_table).unwrap().unwrap();
    match &r2.groups[0].components[0].shape {
        MaskShape::Ai(ai) => assert_eq!(ai.digest, None),
        s => panic!("{s:?}"),
    }
    assert!(r2.mattes.is_empty());
    // No masks at all.
    assert_eq!(read(packet::NEW_PACKET).unwrap(), None);
}

#[test]
fn decodes_a_matte_table_and_places_it() {
    let p = lightroom_packet();
    let r = read(&p).unwrap().unwrap();
    let a = decode_lr_matte(&r.mattes[0]).unwrap();
    assert_eq!((a.width, a.height), (4, 2));
    assert_eq!(a.data, vec![0, 64, 128, 255, 255, 255, 10, 20]);
    // Origin (0, 296) in a 2880 x 1920 space.
    assert!((a.bounds.x - 0.0).abs() < 1e-6 && (a.bounds.y - 296.0 / 1920.0).abs() < 1e-6);
    assert!((a.bounds.width - 4.0 / 2880.0).abs() < 1e-7 && (a.bounds.height - 2.0 / 1920.0).abs() < 1e-7);
    let mut unknown = r.mattes[0].clone();
    unknown.whole_area = [0.0; 4];
    assert_eq!(placement(&unknown, 4, 2), NormRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
}

#[test]
fn unchanged_masks_keep_the_packet_byte_for_byte() {
    let p = lightroom_packet();
    let groups = read(&p).unwrap().unwrap().groups;
    assert_eq!(apply(&p, &groups).unwrap(), p);
    let bom = format!("\u{feff}{p}");
    assert_eq!(apply(&bom, &groups).unwrap(), bom);
    // Nothing to write, nothing there.
    assert_eq!(apply(packet::NEW_PACKET, &[]).unwrap(), packet::NEW_PACKET);
}

#[test]
fn changed_local_slider_rewrites_only_that_value() {
    let p = lightroom_packet();
    let mut groups = read(&p).unwrap().unwrap().groups;
    groups[0].adjustments.exposure = 1.0;
    let out = apply(&p, &groups).unwrap();
    assert_ne!(out, p);
    assert_eq!(attr_of(&out, "crs:LocalExposure2012"), vec!["0.25"]);
    // Everything else is kept byte-for-byte: the only difference is that attribute (plus
    // newly written zero sliders appended to the group).
    let diff_old: Vec<&str> = p.lines().filter(|l| !out.contains(*l)).collect();
    assert_eq!(diff_old, vec!["       crs:LocalExposure2012=\"0.0825\""]);
    // Component item, matte table and retouch table untouched.
    assert!(out.contains(&p[p.find("        <rdf:li\n         crs:What=\"Mask/Image\"").unwrap()..p.find("crs:ModelVersion=\"251659306\"/>").unwrap()]));
    assert_eq!(attr_of(&out, "crs:Table_E71A59AFC894F4F898F72751E30113DA"), attr_of(&p, "crs:Table_E71A59AFC894F4F898F72751E30113DA"));
    assert!(out.contains("crs:Table_72332FE69583D719E860C3ABD10EDA83=\"retouchdata\""));
    // The preset copy is untouched.
    assert_eq!(attr_of(&out, "crs:ErrorReason"), ["0"]);
    let back = read(&out).unwrap().unwrap().groups;
    assert_eq!(back, groups);
    // A second write of the same state is a no-op.
    assert_eq!(apply(&out, &groups).unwrap(), out);
    // The rest of the packet still parses and keeps its develop settings.
    let v = packet::parse(&out).unwrap();
    assert_eq!(v.rating, Some(2));
}

#[test]
fn deleting_an_ai_component_removes_only_its_table() {
    let p = lightroom_packet();
    let mut groups = read(&p).unwrap().unwrap().groups;
    groups[0].components.clear();
    let out = apply(&p, &groups).unwrap();
    assert!(!out.contains("Table_E71A59AFC894F4F898F72751E30113DA"));
    assert!(out.contains("crs:Table_72332FE69583D719E860C3ABD10EDA83=\"retouchdata\""));
    assert!(out.contains("<crs:RetouchAreas>"));
    assert_eq!(read(&out).unwrap().unwrap().groups, groups);
    // Removing every group drops the element (and the table); the preset copy stays.
    let out = apply(&p, &[]).unwrap();
    assert!(!out.contains("Table_E71A59AFC894F4F898F72751E30113DA"));
    assert_eq!(out.matches("<crs:MaskGroupBasedCorrections>").count(), 1, "preset copy only");
    assert_eq!(read(&out).unwrap(), None);
    assert!(out.contains("crs:HasSettings=\"True\">\n   <crs:ToneCurvePV2012>"));
}

#[test]
fn changing_the_ai_selection_drops_lightroom_matte_attributes() {
    let p = lightroom_packet();
    let mut groups = read(&p).unwrap().unwrap().groups;
    if let MaskShape::Ai(ai) = &mut groups[0].components[0].shape {
        ai.reference_point = Some(NormPoint { x: 0.5, y: 0.25 });
        ai.digest = Some(id(99)); // a Sieve matte: never written
    }
    let out = apply(&p, &groups).unwrap();
    for gone in DIGEST_ATTRS_FOR_TEST {
        assert!(!out[..out.find("<crs:RetouchAreas>").unwrap()].contains(&format!("crs:{gone}=")), "{gone}");
    }
    assert!(!out.contains(&id(99)));
    assert!(!out.contains("Table_E71A59AFC894F4F898F72751E30113DA"));
    assert!(out.contains("crs:ReferencePoint=\"0.500000 0.250000\""));
    assert!(out.contains("crs:MaskVersion=\"1\""), "unmodelled attributes are carried over");
    let back = read(&out).unwrap().unwrap().groups;
    match &back[0].components[0].shape {
        MaskShape::Ai(ai) => {
            assert_eq!(ai.digest, None, "preset form: Lightroom recomputes it");
            assert_eq!(ai.reference_point, Some(NormPoint { x: 0.5, y: 0.25 }));
        }
        s => panic!("{s:?}"),
    }
    // Only opacity changed: the matte attributes and table stay.
    let mut groups = read(&p).unwrap().unwrap().groups;
    groups[0].components[0].opacity = 0.5;
    let out = apply(&p, &groups).unwrap();
    assert!(out.contains("crs:MaskDigest=\"E71A59AFC894F4F898F72751E30113DA\""));
    assert!(out.contains("crs:Table_E71A59AFC894F4F898F72751E30113DA="));
    assert!(out.contains("crs:MaskValue=\"0.5\""));
    assert_eq!(read(&out).unwrap().unwrap().groups, groups);
}

const DIGEST_ATTRS_FOR_TEST: &[&str] =
    &["MaskDigest", "InputDigest", "InputDigestVersion", "WholeImageArea", "Origin", "ModelVersion"];

fn comp(n: u32, shape: MaskShape) -> MaskComponent {
    MaskComponent {
        id: id(100 + n),
        name: format!("C{n}"),
        active: true,
        mode: MaskBlendMode::Add,
        inverted: false,
        opacity: 1.0,
        shape,
    }
}

fn all_kinds() -> Vec<MaskGroup> {
    let stroke = |erase: bool, x: f32| BrushStroke {
        radius: 0.02,
        flow: 0.5,
        feather: 0.25,
        density: if erase { 1.0 } else { 0.75 },
        erase,
        auto_mask: false,
        dabs: vec![NormPoint { x, y: 0.5 }, NormPoint { x: x + 0.01, y: 0.51 }],
    };
    let ai = |target: AiTarget| MaskShape::Ai(AiMask { target, reference_point: Some(NormPoint { x: 0.25, y: 0.75 }), digest: None });
    let mut subtract = comp(3, MaskShape::Radial(RadialMask {
        top: 0.1,
        left: 0.2,
        bottom: 0.6,
        right: 0.9,
        angle: 12.5,
        midpoint: 40.0,
        roundness: -20.0,
        feather: 70.0,
        flipped: false,
    }));
    subtract.mode = MaskBlendMode::Subtract;
    subtract.inverted = true;
    subtract.opacity = 0.8;
    let mut intersect = comp(4, MaskShape::Luminance(LuminanceRange { feather_low: 0.1, low: 0.2, high: 0.7, feather_high: 0.9, smoothness: 30.0 }));
    intersect.mode = MaskBlendMode::Intersect;
    let g1 = MaskGroup {
        id: id(1),
        name: "Mask 1".into(),
        active: true,
        amount: 0.8,
        adjustments: LocalAdjustments {
            exposure: -0.5,
            contrast: 20.0,
            hue: 30.0,
            color: crate::ipc::types::LocalColor { hue: 200.0, saturation: 40.0 },
            tone_curve: crate::ipc::types::PointCurves {
                master: vec![[0.0, 10.0], [128.0, 140.0], [255.0, 255.0]],
                ..Default::default()
            },
            ..Default::default()
        },
        components: vec![
            comp(1, MaskShape::Brush(BrushMask { strokes: vec![stroke(false, 0.3), stroke(true, 0.31)] })),
            comp(2, MaskShape::Linear(LinearMask { zero: NormPoint { x: 0.0, y: 0.2 }, full: NormPoint { x: 0.0, y: 0.6 } })),
            subtract,
            intersect,
            comp(5, MaskShape::Color(ColorRange {
                samples: vec![
                    ColorSample { point: NormPoint { x: 0.4, y: 0.4 }, area: None, lightroom_model: None },
                    ColorSample {
                        point: NormPoint { x: 0.55, y: 0.55 },
                        area: Some(NormRect { x: 0.5, y: 0.5, width: 0.1, height: 0.1 }),
                        lightroom_model: None,
                    },
                ],
                amount: 60.0,
            })),
        ],
    };
    let g2 = MaskGroup {
        id: id(2),
        name: "AI".into(),
        active: false,
        amount: 1.0,
        adjustments: LocalAdjustments { saturation: -30.0, ..Default::default() },
        components: vec![
            comp(10, ai(AiTarget::Subject)),
            comp(11, ai(AiTarget::Sky)),
            comp(12, ai(AiTarget::People { parts: vec![crate::ipc::types::PersonPart::FaceSkin, crate::ipc::types::PersonPart::Lips] })),
            comp(13, ai(AiTarget::Object { region: NormRect { x: 0.1, y: 0.2, width: 0.3, height: 0.4 } })),
            comp(14, ai(AiTarget::Landscape { category: crate::ipc::types::LandscapeCategory::Water })),
            comp(15, ai(AiTarget::Other { sub_type: 9, sub_category: Some(3) })),
        ],
    };
    vec![g1, g2]
}

/// Brush strokes read back with density = MaskValue (component opacity folded in).
fn normalize(mut g: Vec<MaskGroup>) -> Vec<MaskGroup> {
    for grp in &mut g {
        for c in &mut grp.components {
            if let MaskShape::Brush(b) = &mut c.shape {
                for s in &mut b.strokes {
                    if !s.erase {
                        s.density *= c.opacity;
                    }
                }
                c.opacity = 1.0;
            }
        }
    }
    g
}

#[test]
fn every_component_kind_round_trips_through_a_new_packet() {
    let groups = all_kinds();
    validate_masks(&groups).unwrap();
    let out = apply(packet::NEW_PACKET, &groups).unwrap();
    assert!(out.contains("xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\""), "{out}");
    let r = read(&out).unwrap().unwrap();
    let mut want = normalize(groups.clone());
    // Numbers are written with 6 decimals.
    let back = r.groups;
    assert_eq!(back.len(), 2);
    assert_eq!(back[1], want.remove(1));
    let (b0, w0) = (&back[0], &want[0]);
    assert_eq!(b0.adjustments.tone_curve, w0.adjustments.tone_curve);
    assert!((b0.adjustments.exposure - w0.adjustments.exposure).abs() < 1e-5);
    assert!((b0.adjustments.hue - 30.0).abs() < 1e-3);
    assert_eq!(b0.adjustments.color, w0.adjustments.color);
    for (b, w) in b0.components.iter().zip(&w0.components) {
        match (&b.shape, &w.shape) {
            (MaskShape::Color(bc), MaskShape::Color(wc)) => {
                assert_eq!(bc.amount, wc.amount);
                assert_eq!(bc.samples.len(), 2);
                assert_eq!(bc.samples[1].area, wc.samples[1].area);
                assert_eq!(bc.samples[0].point, wc.samples[0].point);
            }
            _ => assert_eq!(b, w),
        }
    }
    assert!(r.warnings.is_empty());
    // Writing what was read is a no-op.
    let again = read(&out).unwrap().unwrap().groups;
    assert_eq!(apply(&out, &again).unwrap(), out);
    // New groups carry Lightroom's PV2010 zeros.
    assert!(out.contains("crs:LocalBrightness=\"0\""));
    // Background is an inverted subject.
    let mut bg = all_kinds();
    bg[1].components.truncate(1);
    if let MaskShape::Ai(a) = &mut bg[1].components[0].shape {
        a.target = AiTarget::Background;
    }
    let out = apply(packet::NEW_PACKET, &bg).unwrap();
    let c = &read(&out).unwrap().unwrap().groups[1].components[0];
    assert!(c.inverted);
    assert!(matches!(&c.shape, MaskShape::Ai(a) if a.target == AiTarget::Subject));
}

#[test]
fn inserts_into_a_self_closing_description() {
    let src = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" crs:Exposure2012=\"0\"/>\n </rdf:RDF>\n</x:xmpmeta>\n";
    let groups = all_kinds();
    let out = apply(src, &groups[1..]).unwrap();
    assert_eq!(read(&out).unwrap().unwrap().groups, groups[1..].to_vec());
    assert!(out.contains("crs:Exposure2012=\"0\">\n"));
}

#[test]
fn lightroom_brush_items_join_and_split_on_radius_changes() {
    let p = lightroom_packet().replacen(
        "crs:ModelVersion=\"251659306\"/>\n",
        r#"crs:ModelVersion="251659306"/>
        <rdf:li>
         <rdf:Description crs:What="Mask/Paint" crs:MaskActive="true" crs:MaskName="Brush 1" crs:MaskBlendMode="0" crs:MaskInverted="false" crs:MaskSyncID="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" crs:MaskValue="1" crs:Radius="0.01" crs:Flow="0.5" crs:CenterWeight="0.4">
         <crs:Dabs>
          <rdf:Seq>
           <rdf:li>d 0.1 0.2</rdf:li>
           <rdf:li>r 0.03</rdf:li>
           <rdf:li>d 0.2 0.2</rdf:li>
          </rdf:Seq>
         </crs:Dabs>
         </rdf:Description>
        </rdf:li>
        <rdf:li>
         <rdf:Description crs:What="Mask/Paint" crs:MaskValue="0" crs:Radius="0.02" crs:Flow="1" crs:CenterWeight="1">
         <crs:Dabs>
          <rdf:Seq>
           <rdf:li>d 0.15 0.2</rdf:li>
          </rdf:Seq>
         </crs:Dabs>
         </rdf:Description>
        </rdf:li>
        <rdf:li crs:What="Mask/Gradient" crs:MaskSyncID="BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB" crs:MaskBlendMode="7" crs:ZeroX="0" crs:ZeroY="0" crs:FullX="1" crs:FullY="1"/>
        <rdf:li crs:What="Mask/Depth" crs:MaskSyncID="CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC" crs:Foo="bar"/>
"#,
        1,
    );
    let r = read(&p).unwrap().unwrap();
    let comps = &r.groups[0].components;
    assert_eq!(comps.len(), 4, "{comps:#?}");
    match &comps[1].shape {
        MaskShape::Brush(b) => {
            assert_eq!(b.strokes.len(), 3);
            assert_eq!((b.strokes[0].radius, b.strokes[1].radius, b.strokes[2].radius), (0.01, 0.03, 0.02));
            assert!((b.strokes[0].feather - 0.6).abs() < 1e-6 && b.strokes[0].flow == 0.5);
            assert!(b.strokes[2].erase && !b.strokes[0].erase);
        }
        s => panic!("{s:?}"),
    }
    assert!(matches!(&comps[2].shape, MaskShape::Unsupported(u) if u.what == "Mask/Gradient"), "unknown blend code");
    assert!(matches!(&comps[3].shape, MaskShape::Unsupported(u) if u.what == "Mask/Depth"));
    assert_eq!(r.warnings, vec![DevelopWarning { code: DevelopWarningCode::MasksUnsupported, detail: Some("2".into()) }]);
    validate_masks(&r.groups).unwrap();
    // Editing the group keeps unsupported components verbatim.
    let mut groups = r.groups.clone();
    groups[0].amount = 0.5;
    groups[0].components.remove(1);
    let out = apply(&p, &groups).unwrap();
    assert!(out.contains("<rdf:li crs:What=\"Mask/Depth\" crs:MaskSyncID=\"CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC\" crs:Foo=\"bar\"/>"));
    assert!(!out[..out.find("<crs:RetouchAreas>").unwrap()].contains("Mask/Paint"));
    assert_eq!(read(&out).unwrap().unwrap().groups, groups);
}

#[test]
fn legacy_corrections_only_warn() {
    let src = packet::NEW_PACKET.replace(
        "  </rdf:Description>",
        "   <crs:GradientBasedCorrections xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"><rdf:Seq><rdf:li crs:What=\"Correction\"/><rdf:li crs:What=\"Correction\"/></rdf:Seq></crs:GradientBasedCorrections>\n  </rdf:Description>",
    );
    let r = read(&src).unwrap().unwrap();
    assert!(r.groups.is_empty());
    assert_eq!(r.warnings[0].detail.as_deref(), Some("2"));
    assert_eq!(apply(&src, &[]).unwrap(), src);
}

#[test]
fn fmt6_matches_lightroom_style() {
    use super::write::fmt6;
    assert_eq!(fmt6(-0.195091), "-0.195091");
    assert_eq!(fmt6(1.0), "1");
    assert_eq!(fmt6(0.0), "0");
    assert_eq!(fmt6(-0.0000001), "0");
    assert_eq!(fmt6(0.25), "0.25");
}
