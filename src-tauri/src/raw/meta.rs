//! Capture metadata gathered from containers, normalized for the catalog.

use crate::ipc::types::{CameraMake, SensorLayout};

/// Raw tag values as found in the file (first occurrence wins).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaBuilder {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub orientation: Option<u16>,
    pub date_original: Option<String>,
    pub subsec_original: Option<String>,
    pub date_digitized: Option<String>,
    pub subsec_digitized: Option<String>,
    pub date_modified: Option<String>,
    pub subsec_modified: Option<String>,
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub focal_length: Option<f64>,
    pub iso: Option<u32>,
    pub recommended_exposure_index: Option<u32>,
    pub iso_speed: Option<u32>,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    /// Sensor size from a container-specific source (e.g. RAF header), preferred over EXIF.
    pub sensor_size: Option<(u32, u32)>,
    /// Container says the CFA is X-Trans (RAF `XTransLayout` tag).
    pub xtrans_hint: bool,
}

impl MetaBuilder {
    /// Fills every unset field of `self` from `other`.
    pub fn merge(&mut self, other: MetaBuilder) {
        macro_rules! fill {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = other.$f; } )* };
        }
        fill!(
            make,
            model,
            lens,
            orientation,
            date_original,
            subsec_original,
            date_digitized,
            subsec_digitized,
            date_modified,
            subsec_modified,
            exposure_time,
            f_number,
            focal_length,
            iso,
            recommended_exposure_index,
            iso_speed,
            pixel_width,
            pixel_height,
            sensor_size
        );
        self.xtrans_hint |= other.xtrans_hint;
    }

    pub fn finish(self) -> ImageMeta {
        let captured_at_ms = [
            (&self.date_original, &self.subsec_original),
            (&self.date_digitized, &self.subsec_digitized),
            (&self.date_modified, &self.subsec_modified),
        ]
        .into_iter()
        .find_map(|(d, s)| parse_exif_datetime(d.as_deref()?, s.as_deref()));

        // ISO tag saturates at 65535; the real value is in REI / ISOSpeed then.
        let iso = match self.iso {
            Some(v) if v > 0 && v < 65535 => Some(v),
            other => self.recommended_exposure_index.or(self.iso_speed).or(other),
        }
        .filter(|&v| v > 0);

        let (width, height) = match (self.sensor_size, self.pixel_width, self.pixel_height) {
            (Some((w, h)), _, _) => (Some(w), Some(h)),
            (None, Some(w), Some(h)) if w > 0 && h > 0 => (Some(w), Some(h)),
            _ => (None, None),
        };

        let make = self.make.as_deref().map(camera_make);
        let sensor_layout = match make {
            Some(CameraMake::Fujifilm) => {
                let from_model = self.model.as_deref().map(fuji_sensor_layout).unwrap_or(SensorLayout::Unknown);
                if from_model == SensorLayout::Unknown && self.xtrans_hint {
                    Some(SensorLayout::XTrans)
                } else {
                    Some(from_model)
                }
            }
            Some(CameraMake::Sony | CameraMake::Canon) => Some(SensorLayout::Bayer),
            _ => None,
        };

        ImageMeta {
            make,
            model: self.model,
            lens: self.lens,
            sensor_layout,
            captured_at_ms,
            iso,
            shutter_seconds: self.exposure_time.filter(|v| *v > 0.0),
            aperture: self.f_number.filter(|v| *v > 0.0).map(|v| v as f32),
            focal_length_mm: self.focal_length.filter(|v| *v > 0.0).map(|v| v as f32),
            width,
            height,
            orientation: self.orientation.filter(|o| (1..=8).contains(o)),
        }
    }
}

/// Normalized metadata written to `images`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageMeta {
    pub make: Option<CameraMake>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub sensor_layout: Option<SensorLayout>,
    pub captured_at_ms: Option<i64>,
    pub iso: Option<u32>,
    pub shutter_seconds: Option<f64>,
    pub aperture: Option<f32>,
    pub focal_length_mm: Option<f32>,
    /// Developed image size (unrotated), not the embedded preview's.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF orientation 1..=8.
    pub orientation: Option<u16>,
}

pub fn camera_make(make: &str) -> CameraMake {
    let m = make.trim().to_ascii_lowercase();
    if m.starts_with("sony") {
        CameraMake::Sony
    } else if m.starts_with("fuji") {
        CameraMake::Fujifilm
    } else if m.starts_with("canon") {
        CameraMake::Canon
    } else {
        CameraMake::Other
    }
}

/// Fuji bodies with a Bayer CFA; everything else in the X series uses X-Trans.
const FUJI_BAYER: &[&str] = &[
    "X100", "X-A1", "X-A2", "X-A3", "X-A5", "X-A7", "X-A10", "X-A20", "X-T100", "X-T200", "XF10", "X10", "XF1", "X-S1",
];

/// X-Trans vs Bayer from a Fuji model string (EXIF `Model`, e.g. "X-T5", "GFX100S").
pub fn fuji_sensor_layout(model: &str) -> SensorLayout {
    let m = model.trim().to_ascii_uppercase();
    let m = m.strip_prefix("FUJIFILM ").unwrap_or(&m);
    if m.starts_with("GFX") || m.starts_with("FINEPIX") || FUJI_BAYER.contains(&m) {
        SensorLayout::Bayer
    } else if m.starts_with("X-") || m.starts_with("X100") || m.starts_with("XQ") || m == "X20" || m == "X30" {
        SensorLayout::XTrans
    } else {
        SensorLayout::Unknown
    }
}

/// Parses EXIF `YYYY:MM:DD HH:MM:SS` plus optional `SubSecTime*` into ms since the Unix
/// epoch, treating the camera wall-clock time as UTC (see `CaptureMeta.captured_at_ms`).
pub fn parse_exif_datetime(dt: &str, subsec: Option<&str>) -> Option<i64> {
    let nums: Vec<i64> = dt
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(6)
        .map(|s| s.parse().ok())
        .collect::<Option<_>>()?;
    let [y, mo, d, h, mi, s] = <[i64; 6]>::try_from(nums).ok()?;
    if !(1900..=9999).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let mut ms = ((days * 24 + h) * 60 + mi) * 60_000 + s * 1000;
    if let Some(frac) = subsec.map(str::trim).filter(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit())) {
        // Fractional-second digits: "1" = 100 ms, "10" = 100 ms, "106" = 106 ms, "1065" = 106 ms.
        let digits: String = frac.chars().chain("000".chars()).take(3).collect();
        ms += digits.parse::<i64>().ok()?;
    }
    Some(ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_datetime_to_naive_utc_ms() {
        assert_eq!(parse_exif_datetime("1970:01:01 00:00:00", None), Some(0));
        // 2026-09-26T18:36:04.106Z
        assert_eq!(parse_exif_datetime("2026:09:26 18:36:04", Some("106")), Some(1_790_447_764_106));
        assert_eq!(parse_exif_datetime("2026:09:26 18:36:04", Some("1")), Some(1_790_447_764_100));
        assert_eq!(parse_exif_datetime("2026:09:26 18:36:04", Some("10654")), Some(1_790_447_764_106));
        assert_eq!(parse_exif_datetime("2000:02:29 12:00:00", Some(" ")), Some(951_825_600_000));
        assert_eq!(parse_exif_datetime("2026-09-26T18:36:04", None), Some(1_790_447_764_000));
        assert_eq!(parse_exif_datetime("    :  :     :  :  ", None), None);
        assert_eq!(parse_exif_datetime("2026:13:01 00:00:00", None), None);
    }

    #[test]
    fn finish_normalizes() {
        let b = MetaBuilder {
            make: Some("FUJIFILM".into()),
            model: Some("X-T5".into()),
            date_modified: Some("2026:01:01 00:00:01".into()),
            iso: Some(65535),
            recommended_exposure_index: Some(102_400),
            f_number: Some(0.0),
            orientation: Some(9),
            pixel_width: Some(7728),
            pixel_height: Some(5152),
            sensor_size: Some((7752, 5178)),
            ..Default::default()
        };
        let m = b.finish();
        assert_eq!(m.make, Some(CameraMake::Fujifilm));
        assert_eq!(m.sensor_layout, Some(SensorLayout::XTrans));
        assert_eq!(m.iso, Some(102_400));
        assert_eq!(m.aperture, None);
        assert_eq!(m.orientation, None);
        assert_eq!((m.width, m.height), (Some(7752), Some(5178)));
        assert!(m.captured_at_ms.is_some(), "falls back to DateTime");
    }

    #[test]
    fn fuji_layouts() {
        for x in ["X-T5", "X-H2S", "X100V", "X100VI", "X-Pro3", "X-E4", "X-S20", "X30", "XQ2", "FUJIFILM X-T3"] {
            assert_eq!(fuji_sensor_layout(x), SensorLayout::XTrans, "{x}");
        }
        for b in ["GFX100S", "GFX 50R", "X-A5", "X-T200", "XF10", "X100"] {
            assert_eq!(fuji_sensor_layout(b), SensorLayout::Bayer, "{b}");
        }
        assert_eq!(fuji_sensor_layout("S5Pro"), SensorLayout::Unknown);
    }
}
