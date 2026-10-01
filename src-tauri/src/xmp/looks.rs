//! Locates installed Adobe look profiles by UUID for `<crs:Look>` writes (Phase 7b).
//!
//! The files are read in place from the look directories of `profiles::ProfileConfig`
//! (never copied); the index (UUID -> path) is built once per process from a cheap text
//! scan (`crs:PresetType="Look"` + `crs:UUID="..."`), and file texts are cached (a handful
//! of looks are in use at a time).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::profiles::ProfileConfig;

/// Look files larger than this are skipped (the biggest Adobe looks are a few MB).
const MAX_LOOK_FILE: u64 = 32 << 20;

fn uuid_of(text: &str) -> Option<String> {
    if !text.contains("PresetType=\"Look\"") && !text.contains("<crs:PresetType>Look<") {
        return None;
    }
    let i = text.find("crs:UUID=\"")? + "crs:UUID=\"".len();
    let uuid: String = text[i..].chars().take_while(|c| *c != '"').collect();
    let uuid = uuid.trim().to_ascii_uppercase();
    crate::ipc::types::LookSettings::is_valid_uuid(&uuid).then_some(uuid)
}

/// UUID -> file for every look under `dirs` (recursive; first file wins).
pub fn scan(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    for dir in dirs {
        for entry in walkdir::WalkDir::new(dir).follow_links(true).into_iter().filter_map(Result::ok) {
            let p = entry.path();
            let is_xmp = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xmp"));
            if !is_xmp || entry.metadata().map_or(true, |m| m.len() > MAX_LOOK_FILE) {
                continue;
            }
            // The header (UUID, PresetType) is in the first few KB.
            let Some(head) = read_head(p, 16 * 1024) else { continue };
            if let Some(uuid) = uuid_of(&head) {
                out.entry(uuid).or_insert_with(|| p.to_path_buf());
            }
        }
    }
    out
}

fn read_head(p: &Path, n: usize) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; n];
    let mut len = 0;
    while len < n {
        match f.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(k) => len += k,
            Err(_) => return None,
        }
    }
    buf.truncate(len);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

struct Library {
    index: HashMap<String, PathBuf>,
    texts: Mutex<HashMap<String, Arc<str>>>,
}

fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| Library { index: scan(&ProfileConfig::from_env().look_dirs), texts: Mutex::new(HashMap::new()) })
}

/// The installed (or style-library imported) look file (XMP text) with `uuid`, if any.
pub fn installed(uuid: &str) -> Option<Arc<str>> {
    let lib = library();
    let uuid = uuid.to_ascii_uppercase();
    if let Some(t) = lib.texts.lock().unwrap_or_else(|e| e.into_inner()).get(&uuid) {
        return Some(t.clone());
    }
    // Adobe-installed first, then looks imported into the style library (IPC v14).
    let path = lib.index.get(&uuid).cloned().or_else(|| crate::profiles::imported_look_path(&uuid))?;
    let text: Arc<str> = std::fs::read_to_string(path).ok()?.into();
    let mut cache = lib.texts.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(uuid, text.clone());
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_look_files_by_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Adobe/Profiles");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("Look.xmp"),
            "<x:xmpmeta><rdf:RDF><rdf:Description crs:PresetType=\"Look\" crs:UUID=\"b952c231111cd8e0eccf14b86baa7077\"/></rdf:RDF></x:xmpmeta>",
        )
        .unwrap();
        std::fs::write(
            sub.join("Preset.xmp"),
            "crs:PresetType=\"Normal\" crs:UUID=\"0123456789ABCDEF0123456789ABCDEF\"",
        )
        .unwrap();
        std::fs::write(sub.join("notes.txt"), "crs:PresetType=\"Look\"").unwrap();
        let idx = scan(&[dir.path().to_path_buf(), dir.path().join("missing")]);
        assert_eq!(idx.len(), 1);
        assert!(idx["B952C231111CD8E0ECCF14B86BAA7077"].ends_with("Look.xmp"));
    }
}
