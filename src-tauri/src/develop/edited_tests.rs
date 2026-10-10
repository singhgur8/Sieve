//! Tests of the edited-preview cache (`develop::edited`), on PNG sources in a temp catalog.

use super::*;
use crate::develop::{handle_protocol, DevelopConfig};
use crate::ipc::types::{AdjustmentField, ImageFormat, ImportOptions};
use std::sync::mpsc;
use tauri::http;

const WAIT: Duration = Duration::from_secs(20);

struct Fixture {
    _dir: tempfile::TempDir,
    conn: Connection,
    cache: DevelopCache,
    ed: EditedPreviews,
    events: mpsc::Receiver<EditedPreviewChanged>,
    ids: Vec<ImageId>,
}

/// `n` grey PNGs imported into a fresh catalog (thumbnails marked ready), the cache enabled
/// with a `max_bytes` budget.
fn fixture(n: usize, max_bytes: u64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let (w, h) = (96u32, 64u32);
    for i in 0..n {
        let px: Vec<u8> = (0..w * h).flat_map(|_| [90u8 + i as u8, 100, 110]).collect();
        std::fs::write(
            folder.join(format!("g{i}.png")),
            crate::raw::png::test_support::encode(w, h, None, Some(&px), None, None, None),
        )
        .unwrap();
    }
    let catalog = dir.path().join("cat.sqlite");
    let mut conn = crate::db::open(&catalog).unwrap();
    let opts = ImportOptions { recursive: false, include_non_raw: true, pair_jpeg_with_raw: true };
    crate::db::repo::import_folder(&mut conn, &folder, &opts).unwrap();
    conn.execute("UPDATE thumbnails SET status = 'ready', path = '/t.jpg', width = 96, height = 64", []).unwrap();
    let ids: Vec<ImageId> = {
        let mut stmt = conn.prepare("SELECT id FROM images ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let cache = DevelopCache::new(DevelopConfig { cache_bytes: 64 << 20, mask_cache: None });
    let (tx, events) = mpsc::channel();
    let tx = Mutex::new(tx);
    let config = EditedConfig { dir: dir.path().join("cache/edited"), catalog_path: catalog, max_bytes };
    let ed = cache.enable_edited_previews(
        config,
        LutLibrary::new(dir.path().join("luts")),
        Arc::new(move |ev| {
            let _ = lock(&tx).send(ev);
        }),
    );
    Fixture { _dir: dir, conn, cache, ed, events, ids }
}

fn png_defaults() -> ParametricAdjustments {
    ParametricAdjustments::defaults_for(ImageFormat::Png)
}

impl Fixture {
    fn edit(&mut self, id: ImageId, exposure: f32) {
        let adj = ParametricAdjustments { exposure, ..png_defaults() };
        // Distinct labels: same-label saves within 1.5 s coalesce into one history entry.
        crate::develop::history::commit(&mut self.conn, id, &adj, &format!("Exposure {exposure}")).unwrap();
    }

    /// Next event for `id` (others are skipped).
    fn next_for(&self, id: ImageId) -> EditedPreviewChanged {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let ev = self.events.recv_timeout(left).expect("edited preview event");
            if ev.image_id == id {
                return ev;
            }
        }
    }

    /// Events until every id in `ids` reported (last per id).
    fn next_for_all(&self, ids: &[ImageId]) -> HashMap<ImageId, EditedPreviewChanged> {
        let mut got = HashMap::new();
        let end = Instant::now() + WAIT;
        while ids.iter().any(|id| !got.contains_key(id)) {
            let left = end.saturating_duration_since(Instant::now());
            let ev = self.events.recv_timeout(left).expect("edited preview events");
            got.insert(ev.image_id, ev);
        }
        got
    }

    fn mean_of_thumb(&self, id: ImageId) -> f64 {
        let (hash, _) = self.ed.current(id).expect("cached");
        let bytes = std::fs::read(self.ed.file(id, hash, true)).unwrap();
        let img = crate::raw::preview::decode_jpeg(&bytes, 1).unwrap();
        img.pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / img.pixels.len() as f64
    }

    fn files(&self) -> usize {
        std::fs::read_dir(&self.ed.config().dir).unwrap().count()
    }

    fn get(&self, url: &str) -> http::Response<Vec<u8>> {
        let path = url.split("localhost").nth(1).unwrap();
        let req = http::Request::builder().uri(format!("sieve://localhost{path}")).body(Vec::new()).unwrap();
        handle_protocol(&self.cache, &req)
    }
}

#[test]
fn hash_is_stable_and_follows_every_setting() {
    let a = ParametricAdjustments::default();
    assert_eq!(settings_hash(&a), settings_hash(&a.clone()));
    let b = ParametricAdjustments { exposure: 0.01, ..a.clone() };
    let mut c = a.clone();
    c.effects.grain.amount = 10.0;
    assert_ne!(settings_hash(&a), settings_hash(&b));
    assert_ne!(settings_hash(&a), settings_hash(&c));
    let p = preview_urls(7, settings_hash(&b));
    assert!(p.thumb_url.ends_with("/thumb.jpg") && p.preview_url.contains("/edited/7/"));
    let path = p.preview_url.split("localhost").nth(1).unwrap();
    assert_eq!(parse_path(path), Some((7, settings_hash(&b), false)));
    assert_eq!(parse_path("/edited/7/123/thumb.jpg"), None);
    assert_eq!(parse_path("/render/7/main"), None);
}

#[test]
fn edit_renders_new_hash_replaces_old_and_reset_clears() {
    let mut f = fixture(1, 64 << 20);
    let id = f.ids[0];
    f.edit(id, 1.0);
    let first = f.next_for(id).preview.expect("rendered");
    let (h1, urls) = f.ed.current(id).unwrap();
    assert_eq!(urls, first);
    let bright1 = f.mean_of_thumb(id);
    assert_eq!(f.files(), 2);

    // Listing returns the preview of edited photos.
    let entry = crate::db::repo::get_image(&f.conn, id).unwrap();
    assert_eq!(entry.edited_preview.as_ref(), Some(&first));

    // A new edit: new hash, new URLs, old files gone, thumbnail brighter.
    f.edit(id, 2.0);
    let second = f.next_for(id).preview.expect("rendered");
    let (h2, _) = f.ed.current(id).unwrap();
    assert_ne!(h1, h2, "hash change invalidates");
    assert_ne!(first, second);
    assert!(!f.ed.file(id, h1, true).exists() && !f.ed.file(id, h1, false).exists());
    assert!(f.mean_of_thumb(id) > bright1 + 5.0, "thumbnail shows the new edit");

    // Served by the protocol, content-addressed and cacheable; the old URL is gone.
    let resp = f.get(&second.thumb_url);
    assert_eq!(resp.status(), http::StatusCode::OK);
    assert!(resp.headers()[http::header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
    assert_eq!(f.get(&second.preview_url).status(), http::StatusCode::OK);
    assert_eq!(f.get(&first.thumb_url).status(), http::StatusCode::NOT_FOUND);

    // Undo back to the first edit: the first hash again.
    crate::develop::history::undo(&mut f.conn, id).unwrap();
    assert_eq!(f.next_for(id).preview, Some(first));

    // Reset to neutral: dropped, files deleted, listing has no preview.
    crate::develop::history::commit(&mut f.conn, id, &png_defaults(), "Reset").unwrap();
    assert_eq!(f.next_for(id).preview, None);
    assert!(f.ed.current(id).is_none());
    assert_eq!(f.files(), 0);
    assert!(crate::db::repo::get_image(&f.conn, id).unwrap().edited_preview.is_none());
}

#[test]
fn batch_paste_and_batch_undo_regenerate_every_photo() {
    let mut f = fixture(4, 64 << 20);
    let ids = f.ids.clone();
    let src = ParametricAdjustments { exposure: 1.5, ..png_defaults() };
    let batch =
        crate::develop::batches::apply_fields_recorded(&mut f.conn, &ids, &src, &[AdjustmentField::Exposure], "Paste")
            .unwrap();
    let got = f.next_for_all(&ids);
    assert!(got.values().all(|e| e.preview.is_some()));
    let entries = crate::db::repo::get_images(&f.conn, &ids).unwrap();
    assert!(entries.iter().all(|e| e.has_edits && e.edited_preview.is_some()));

    crate::develop::batches::undo(&mut f.conn, batch.batch_id.unwrap()).unwrap();
    let got = f.next_for_all(&ids);
    assert!(got.values().all(|e| e.preview.is_none()), "undo to neutral clears every preview");
    assert_eq!(f.files(), 0);
}

#[test]
fn rolled_back_write_does_not_render() {
    let mut f = fixture(1, 64 << 20);
    let id = f.ids[0];
    {
        let tx = f.conn.transaction().unwrap();
        let adj = ParametricAdjustments { exposure: 1.0, ..png_defaults() };
        crate::db::repo::save_adjustments(&tx, id, &adj).unwrap();
        tx.rollback().unwrap();
    }
    std::thread::sleep(Duration::from_millis(1500));
    assert!(f.ed.current(id).is_none());
    assert!(f.events.try_recv().is_err());
}

#[test]
fn disk_budget_evicts_least_recently_used() {
    // Measure one photo's files, then allow two and a half.
    let mut probe = fixture(1, 64 << 20);
    let pid = probe.ids[0];
    probe.edit(pid, 1.0);
    probe.next_for(pid);
    let one = probe.ed.bytes();
    assert!(one > 0);
    let budget = one * 2 + one / 2;

    let mut f = fixture(4, budget);
    let ids = f.ids.clone();
    for &id in &ids {
        f.edit(id, 1.0);
        f.next_for(id);
        assert!(f.ed.bytes() <= budget, "within budget");
    }
    let cached: Vec<ImageId> = ids.iter().copied().filter(|id| f.ed.current(*id).is_some()).collect();
    assert_eq!(cached, ids[2..].to_vec(), "oldest evicted first");
    assert_eq!(f.files(), 4);

    // An evicted photo that is listed again is re-rendered (self-healing).
    let entry = crate::db::repo::get_image(&f.conn, ids[0]).unwrap();
    assert!(entry.edited_preview.is_none());
    assert!(f.next_for(ids[0]).preview.is_some());
    assert!(f.ed.bytes() <= budget);

    // Removing images deletes their files.
    let before = f.files();
    f.cache.forget_images(&[ids[0]]);
    assert!(f.ed.current(ids[0]).is_none());
    assert_eq!(f.files(), before - 2);
}

#[test]
fn settled_render_is_reused_and_restart_rescans() {
    let mut f = fixture(1, 64 << 20);
    let id = f.ids[0];
    let adj = ParametricAdjustments { exposure: 1.0, ..png_defaults() };
    // A marker frame stands in for the Develop settled render of exactly these settings.
    let marker = Rgb { width: 1600, height: 1000, pixels: vec![7u8; 1600 * 1000 * 3] };
    f.ed.note_settled(id, &adj, marker);
    crate::develop::history::commit(&mut f.conn, id, &adj, "Exposure").unwrap();
    let urls = f.next_for(id).preview.unwrap();
    assert!(f.mean_of_thumb(id) < 12.0, "the settled frame was used, not a re-render");

    let index = scan(&f.ed.config().dir);
    let it = index.items.get(&id).unwrap();
    assert_eq!(preview_urls(id, it.hash), urls);
    assert_eq!(index.bytes, f.ed.bytes());
}
