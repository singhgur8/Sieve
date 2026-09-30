//! File-name template expansion and output path resolution.
//! Grammar and validation: `ipc::types::parse_filename_template` / `TemplatePart`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ipc::types::{CameraMake, CollisionPolicy, RawImageEntry, TemplatePart};

/// Longest file stem (bytes) before the extension.
pub const MAX_STEM_BYTES: usize = 240;

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '/' | '\\' | ':') || c.is_control() { '_' } else { c }).collect()
}

fn raw_stem(entry: &RawImageEntry) -> String {
    let stem = Path::new(&entry.file_name).file_stem().map(|s| s.to_string_lossy().into_owned());
    stem.filter(|s| !s.is_empty()).unwrap_or_else(|| format!("image-{}", entry.id))
}

/// Calendar fields `(Y, M, D, h, m, s)` of `ms` since the epoch, as UTC.
pub fn civil(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    // Howard Hinnant's days -> civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, sod / 3600, sod / 60 % 60, sod % 60)
}

/// Local-time fields of `ms` (the file mtime fallback of `{date}`).
fn local_civil(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let t: libc::time_t = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r writes into `tm`; both pointers are valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if !ok {
        return civil(ms);
    }
    (
        i64::from(tm.tm_year) + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
        tm.tm_sec as u32,
    )
}

fn format_date(format: &str, (y, mo, d, h, mi, s): (i64, u32, u32, u32, u32, u32)) -> String {
    let mut out = String::new();
    let mut rest = format;
    while !rest.is_empty() {
        let (tok, len) = if rest.starts_with("YYYY") {
            (format!("{y:04}"), 4)
        } else if rest.starts_with("YY") {
            (format!("{:02}", y.rem_euclid(100)), 2)
        } else if rest.starts_with("MM") {
            (format!("{mo:02}"), 2)
        } else if rest.starts_with("DD") {
            (format!("{d:02}"), 2)
        } else if rest.starts_with("hh") {
            (format!("{h:02}"), 2)
        } else if rest.starts_with("mm") {
            (format!("{mi:02}"), 2)
        } else if rest.starts_with("ss") {
            (format!("{s:02}"), 2)
        } else {
            let c = rest.chars().next().expect("non-empty");
            (c.to_string(), c.len_utf8())
        };
        out.push_str(&tok);
        rest = &rest[len..];
    }
    out
}

/// Expands parsed `parts` for `entry` at `seq` (= position in `ids` + `startNumber`) into a
/// file stem (no extension): token values sanitized (`/ \ :` and control chars -> `_`),
/// leading `.`/spaces stripped, truncated to 240 bytes on a char boundary, never empty
/// (falls back to the RAW's stem). `{date}` uses `capture.capturedAtMs` as naive wall-clock
/// time (UTC fields), else `fileMtimeMs` in local time.
pub fn expand(parts: &[TemplatePart], entry: &RawImageEntry, seq: u64) -> String {
    let mut out = String::new();
    for part in parts {
        let v = match part {
            TemplatePart::Text(t) => t.clone(),
            TemplatePart::Filename => raw_stem(entry),
            TemplatePart::Seq { digits } => format!("{seq:0width$}", width = usize::from(*digits)),
            TemplatePart::Date { format } => {
                let fields = match entry.capture.captured_at_ms {
                    Some(ms) => civil(ms),
                    None => local_civil(entry.file_mtime_ms),
                };
                format_date(format, fields)
            }
            TemplatePart::Rating => entry.rating.to_string(),
            TemplatePart::Camera => match entry.camera.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                Some(m) => m.to_owned(),
                None => match entry.camera.make {
                    CameraMake::Sony => "Sony".into(),
                    CameraMake::Fujifilm => "Fujifilm".into(),
                    CameraMake::Canon => "Canon".into(),
                    CameraMake::Other => "Unknown".into(),
                },
            },
            TemplatePart::Folder => Path::new(&entry.path)
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            TemplatePart::Id => entry.id.to_string(),
        };
        out.push_str(&sanitize(&v));
    }
    let trimmed = out.trim_start_matches(['.', ' ']);
    let mut stem = String::new();
    for c in trimmed.chars() {
        if stem.len() + c.len_utf8() > MAX_STEM_BYTES {
            break;
        }
        stem.push(c);
    }
    if stem.trim().is_empty() {
        let fallback = sanitize(&raw_stem(entry));
        let fallback = fallback.trim_start_matches(['.', ' ']);
        return if fallback.is_empty() { format!("image-{}", entry.id) } else { fallback.to_owned() };
    }
    stem
}

/// Where one image of a job goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Final path; `None` = skipped (`collision = skip` and the file exists).
    pub path: Option<PathBuf>,
    /// The template's own path exists on disk (before the collision policy).
    pub exists: bool,
}

/// Claims output paths for one job: names repeated within the job always get `-2`, `-3`...
/// suffixes; clashes with files on disk follow the collision policy. Case-insensitive (APFS
/// default), keyed by the lower-cased path.
#[derive(Default)]
pub struct Claims {
    taken: HashSet<String>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

impl Claims {
    pub fn resolve(
        &mut self,
        dir: &Path,
        stem: &str,
        ext: &str,
        policy: CollisionPolicy,
        exists: &dyn Fn(&Path) -> bool,
    ) -> Resolved {
        let base = dir.join(format!("{stem}.{ext}"));
        let on_disk = exists(&base);
        if !self.taken.contains(&key(&base)) {
            let path = match (on_disk, policy) {
                (false, _) | (true, CollisionPolicy::Overwrite) => Some(base.clone()),
                (true, CollisionPolicy::Skip) => None,
                (true, CollisionPolicy::UniqueSuffix) => Some(self.suffixed(dir, stem, ext, exists)),
            };
            if let Some(p) = &path {
                self.taken.insert(key(p));
            }
            // A skipped name is still "used" by this job (later duplicates get suffixes).
            self.taken.insert(key(&base));
            return Resolved { path, exists: on_disk };
        }
        let p = self.suffixed(dir, stem, ext, exists);
        self.taken.insert(key(&p));
        Resolved { path: Some(p), exists: on_disk }
    }

    fn suffixed(&self, dir: &Path, stem: &str, ext: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
        (2u32..)
            .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
            .find(|p| !self.taken.contains(&key(p)) && !exists(p))
            .expect("unbounded suffixes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{
        parse_filename_template, CameraInfo, CaptureMeta, PickFlag, RawFormat, SensorLayout, ThumbnailState,
        XmpSyncState,
    };

    fn entry() -> RawImageEntry {
        RawImageEntry {
            id: 42,
            folder_id: 1,
            path: "/shoots/Smith Wedding/DSC01234.ARW".into(),
            file_name: "DSC01234.ARW".into(),
            format: RawFormat::Arw,
            camera: CameraInfo {
                make: CameraMake::Sony,
                model: Some("ILCE-7M4".into()),
                sensor_layout: SensorLayout::Bayer,
            },
            capture: CaptureMeta { captured_at_ms: Some(1_718_461_353_450), ..Default::default() },
            width: None,
            height: None,
            orientation: None,
            file_size: 0,
            file_mtime_ms: 0,
            thumbnail: ThumbnailState::Pending,
            rating: 4,
            pick: PickFlag::Unflagged,
            color_label: None,
            burst_group_id: None,
            is_burst_keeper: false,
            scene_id: None,
            is_scene_anchor: false,
            companion_path: None,
            develop_warnings: Vec::new(),
            tags: Vec::new(),
            quality: None,
            has_edits: false,
            xmp: XmpSyncState::default(),
        }
    }

    fn exp(t: &str, seq: u64) -> String {
        expand(&parse_filename_template(t).unwrap(), &entry(), seq)
    }

    #[test]
    fn tokens_expand() {
        assert_eq!(exp("{filename}", 1), "DSC01234");
        assert_eq!(exp("Smith-{seq:4}", 7), "Smith-0007");
        assert_eq!(exp("{seq}", 12), "12");
        // 2024-06-15 14:22:33.450 naive wall clock.
        assert_eq!(exp("{date}_{filename}", 1), "20240615_DSC01234");
        assert_eq!(exp("{date:YYYY-MM-DD hh.mm.ss}", 1), "2024-06-15 14.22.33");
        assert_eq!(exp("{date:YY}", 1), "24");
        assert_eq!(exp("{rating}-{camera}-{folder}-{id}", 1), "4-ILCE-7M4-Smith Wedding-42");
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400_000), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil(-1000), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn sanitizing_and_fallbacks() {
        let mut e = entry();
        e.camera.model = Some("A/B:C".into());
        assert_eq!(expand(&parse_filename_template("{camera}").unwrap(), &e, 1), "A_B_C");
        assert_eq!(exp(" .x", 1), "x");
        assert_eq!(exp("...", 1), "DSC01234", "empty -> RAW stem");
        let mut e = entry();
        e.path = format!("/shoots/{}/DSC01234.ARW", "é".repeat(150));
        let stem = expand(&parse_filename_template("{folder}{folder}").unwrap(), &e, 1);
        assert!(stem.len() <= MAX_STEM_BYTES && stem.len() > 200, "{}", stem.len());
        let mut e = entry();
        e.camera.model = None;
        e.camera.make = CameraMake::Other;
        assert_eq!(expand(&parse_filename_template("{camera}").unwrap(), &e, 1), "Unknown");
    }

    #[test]
    fn claims_follow_the_policy_and_dedupe_within_a_job() {
        let dir = Path::new("/out");
        // Case-insensitive like APFS.
        let disk: HashSet<String> = ["/out/a.jpg".to_owned(), "/out/a-2.jpg".to_owned()].into();
        let exists = |p: &Path| disk.contains(&p.to_string_lossy().to_lowercase());
        let mut c = Claims::default();
        let r = c.resolve(dir, "a", "jpg", CollisionPolicy::UniqueSuffix, &exists);
        assert_eq!(r, Resolved { path: Some("/out/a-3.jpg".into()), exists: true });
        let r = c.resolve(dir, "A", "jpg", CollisionPolicy::UniqueSuffix, &exists);
        assert_eq!(r.path, Some("/out/A-4.jpg".into()), "case-insensitive duplicate within the job");

        let mut c = Claims::default();
        assert_eq!(c.resolve(dir, "a", "jpg", CollisionPolicy::Overwrite, &exists).path, Some("/out/a.jpg".into()));
        assert_eq!(c.resolve(dir, "a", "jpg", CollisionPolicy::Overwrite, &exists).path, Some("/out/a-3.jpg".into()));

        let mut c = Claims::default();
        assert_eq!(c.resolve(dir, "a", "jpg", CollisionPolicy::Skip, &exists), Resolved { path: None, exists: true });
        assert_eq!(c.resolve(dir, "b", "jpg", CollisionPolicy::Skip, &exists).path, Some("/out/b.jpg".into()));
        assert_eq!(c.resolve(dir, "b", "jpg", CollisionPolicy::Skip, &exists).path, Some("/out/b-2.jpg".into()));
    }
}
