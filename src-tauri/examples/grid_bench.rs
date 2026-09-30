//! Grid query benchmark on a synthetic catalog (Phase 8 perf pass).
//!
//! ```text
//! cargo run --release --example grid_bench -- [images] [--keep PATH] [--plans]
//! ```
//! Builds a catalog with `images` (default 50 000) rows spread over 20 folders: ready
//! thumbnails, quality scores on 90%, ~1.5 auto tags per image, bursts of 3-8 frames
//! (40% of frames), ratings/picks/labels, edits on 10%. Then times (warm, median of 15 runs)
//! the Library's hot queries: `list_image_ids` (the grid's id list), `get_images` of a
//! 120-id chunk (the grid's row fetch), `list_images` pages, and `get_filter_counts`,
//! each for the whole catalog and one folder under several filters/sorts.
//! `--plans` prints `EXPLAIN QUERY PLAN` of the main statements (the filter-bar indexes of
//! migration 0011 should appear: `idx_images_folder_pick_rating`, `idx_image_tags_live`,
//! `idx_images_missing`). 1% of the images are flagged missing (IPC v13).

use std::time::Instant;

use rusqlite::Connection;
use sieve_lib::db::{self, repo};
use sieve_lib::ipc::types::{CullTag, ImageQuery, ImageSort, PickFlag, TagMatch};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut out = f(); // warm-up
    let mut t = Vec::with_capacity(runs);
    for _ in 0..runs {
        let s = Instant::now();
        out = f();
        t.push(s.elapsed().as_secs_f64() * 1000.0);
    }
    (median(t), out)
}

fn populate(conn: &Connection, n: i64) {
    let folders = 20;
    let started = Instant::now();
    conn.execute_batch("BEGIN").unwrap();
    for f in 1..=folders {
        conn.execute(
            "INSERT INTO folders (id, path, added_at) VALUES (?1, ?2, 0)",
            rusqlite::params![f, format!("/shoot/{f}")],
        )
        .unwrap();
    }
    // Bursts: ids 1..=n/8, each covering a run of consecutive images.
    conn.execute_batch(&format!(
        "WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO images (id, folder_id, path, file_name, format, camera_make, camera_model, sensor_layout,
                             captured_at_ms, iso, width, height, orientation, file_size, file_mtime_ms,
                             rating, pick, color_label, imported_at)
         SELECT x, 1 + (x - 1) * {folders} / {n}, '/shoot/' || x || '/DSC' || printf('%05d', x) || '.ARW',
                'DSC' || printf('%05d', x) || '.ARW', 'arw', 'sony', 'ILCE-7M4', 'bayer',
                CASE WHEN x % 97 = 0 THEN NULL ELSE 1700000000000 + x * 700 END, 400, 7008, 4672, 1,
                25000000, 0, (x * 7) % 6,
                CASE x % 11 WHEN 0 THEN 'pick' WHEN 1 THEN 'reject' ELSE 'unflagged' END,
                CASE x % 13 WHEN 0 THEN 'red' WHEN 1 THEN 'green' ELSE NULL END, 0
         FROM s;
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO thumbnails (image_id, status, path, preview_path, width, height, extracted_at)
         SELECT x, 'ready', '/cache/' || x || '_512.jpg', '/cache/' || x || '_2048.jpg', 512, 341, 0 FROM s;
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO quality_scores (image_id, overall, face_sharpness, global_sharpness, eyes_open, composition,
                                     face_count, clipped_highlights_pct, clipped_shadows_pct, mean_luma,
                                     model_version, analyzed_at, suggested_rating, suggested_pick)
         SELECT x, ((x * 7919) % 1000) / 1000.0, 0.5, 0.5, 0.9, 0.5, 1, 0.1, 0.1, 0.45, 'v', 0, x % 6,
                CASE x % 9 WHEN 0 THEN 'reject' ELSE 'unflagged' END
         FROM s WHERE x % 10 <> 0;
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO image_tags (image_id, tag, source, confidence, suppressed)
         SELECT x, CASE (x / 3) % 6 WHEN 0 THEN 'blink' WHEN 1 THEN 'missed_focus' WHEN 2 THEN 'motion_blur'
                              WHEN 3 THEN 'underexposed' WHEN 4 THEN 'overexposed' ELSE 'creative_blur' END,
                'auto', 0.8, CASE WHEN x % 50 = 0 THEN 1 ELSE 0 END
         FROM s WHERE x % 3 <> 0;
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO image_tags (image_id, tag, source, confidence, suppressed)
         SELECT x, 'duplicate_burst', 'auto', 1.0, 0 FROM s WHERE x % 5 IN (1, 2);
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n} / 8)
         INSERT INTO burst_groups (id, started_at_ms, ended_at_ms, keeper_image_id)
         SELECT x, 0, 0, (x - 1) * 8 + 1 FROM s;
         UPDATE images SET burst_group_id = (id - 1) / 8 + 1 WHERE (id - 1) % 8 < 3 + (id / 8) % 3;
         WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {n})
         INSERT INTO adjustments (image_id, params_json, process_version, updated_at)
         SELECT x, '{{\"exposure\":0.5}}', 1, 0 FROM s WHERE x % 10 = 0;
         UPDATE images SET xmp_dirty = 0;
         UPDATE images SET missing_since_ms = 1700000000000 WHERE id % 100 = 0;"
    ))
    .unwrap();
    conn.execute_batch("COMMIT; ANALYZE;").unwrap();
    eprintln!("populated {n} images in {:.1} s", started.elapsed().as_secs_f64());
}

fn plans(conn: &Connection) {
    let q = |sql: &str| {
        println!("-- {}", sql.split_whitespace().collect::<Vec<_>>().join(" "));
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(3)).unwrap();
        for r in rows {
            println!("   {}", r.unwrap());
        }
    };
    q("SELECT i.id FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id
       ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id");
    q("SELECT i.id FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id WHERE i.folder_id = 3
       ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id");
    q("SELECT i.id FROM images i LEFT JOIN quality_scores q ON q.image_id = i.id
       WHERE EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = i.id AND it.suppressed = 0 AND it.tag IN ('blink'))
       ORDER BY i.captured_at_ms IS NULL, i.captured_at_ms, i.file_name, i.id");
    q("SELECT i.id, i.captured_at_ms, i.file_name, i.rating, NULL FROM images i
       WHERE i.id NOT IN (SELECT image_id FROM image_tags WHERE suppressed = 0 AND tag IN ('blink', 'missed_focus'))");
    q("SELECT pick, rating, COUNT(*) FROM images GROUP BY folder_id, pick, rating");
    q("SELECT pick, rating, COUNT(*) FROM images WHERE folder_id = 3 GROUP BY pick, rating");
    q("SELECT tag, COUNT(*) FROM image_tags WHERE suppressed = 0 GROUP BY tag ORDER BY tag");
    q("SELECT tag, COUNT(*) FROM image_tags
       WHERE suppressed = 0 AND image_id IN (SELECT id FROM images WHERE folder_id = 3) GROUP BY tag ORDER BY tag");
    q("SELECT COUNT(DISTINCT i.burst_group_id),
              COALESCE(SUM(b.keeper_image_id IS NOT NULL AND b.keeper_image_id <> i.id), 0)
       FROM images i JOIN burst_groups b ON b.id = i.burst_group_id");
    q("SELECT COUNT(*) FROM images WHERE missing_since_ms IS NOT NULL");
    q("SELECT COUNT(*) FROM images WHERE missing_since_ms IS NOT NULL AND folder_id = 3");
}

fn main() {
    let mut n: i64 = 50_000;
    let mut keep: Option<String> = None;
    let mut show_plans = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--keep" => keep = args.next(),
            "--plans" => show_plans = true,
            v => n = v.parse().expect("image count"),
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = keep.map(std::path::PathBuf::from).unwrap_or_else(|| dir.path().join("grid.sqlite"));
    let fresh = !path.exists();
    let conn = db::open(&path).unwrap();
    if fresh {
        populate(&conn, n);
    }
    if show_plans {
        plans(&conn);
    }
    let runs = 15;
    let base = ImageQuery::default();
    let folder = Some(3);
    let cases: Vec<(&str, ImageQuery)> = vec![
        ("all, capture time", base.clone()),
        ("all, capture time desc", ImageQuery { sort_descending: true, ..base.clone() }),
        ("all, file name", ImageQuery { sort: ImageSort::FileName, ..base.clone() }),
        ("all, quality", ImageQuery { sort: ImageSort::Quality, ..base.clone() }),
        ("all, rating", ImageQuery { sort: ImageSort::Rating, ..base.clone() }),
        ("folder", ImageQuery { folder_id: folder, ..base.clone() }),
        ("all, picks", ImageQuery { picks: vec![PickFlag::Pick], ..base.clone() }),
        ("all, >=3 stars", ImageQuery { min_rating: Some(3), ..base.clone() }),
        ("all, tag blink", ImageQuery { include_tags: vec![CullTag::Blink], ..base.clone() }),
        (
            "all, tags all(blink,dup)",
            ImageQuery {
                include_tags: vec![CullTag::Blink, CullTag::DuplicateBurst],
                tag_match: TagMatch::All,
                ..base.clone()
            },
        ),
        (
            "all, exclude blink+focus",
            ImageQuery { exclude_tags: vec![CullTag::Blink, CullTag::MissedFocus], ..base.clone() },
        ),
        ("all, collapse bursts", ImageQuery { collapse_bursts: true, ..base.clone() }),
        ("all, missing", ImageQuery { missing_only: true, ..base.clone() }),
        (
            "folder, collapse, >=1 star",
            ImageQuery { folder_id: folder, collapse_bursts: true, min_rating: Some(1), ..base.clone() },
        ),
    ];
    println!("{:<28} {:>10} {:>9} {:>12} {:>12}", "query", "ids ms", "ids", "page0 ms", "page@40k ms");
    let mut worst: f64 = 0.0;
    for (name, q) in &cases {
        let (ids_ms, ids) = time(runs, || repo::list_image_ids(&conn, q).unwrap());
        let (p0, _) = time(runs, || repo::list_images(&conn, &ImageQuery { offset: 0, ..q.clone() }).unwrap());
        let deep = (ids.len() as u32).saturating_sub(200).min(40_000);
        let (pd, _) = time(runs, || repo::list_images(&conn, &ImageQuery { offset: deep, ..q.clone() }).unwrap());
        worst = worst.max(ids_ms).max(p0).max(pd);
        println!("{name:<28} {ids_ms:>10.2} {:>9} {p0:>12.2} {pd:>12.2}", ids.len());
    }
    let all_ids = repo::list_image_ids(&conn, &base).unwrap();
    let mid = all_ids.len() / 2;
    let chunk: Vec<i64> = all_ids[mid..mid + 120].to_vec();
    let (chunk_ms, _) = time(runs, || repo::get_images(&conn, &chunk).unwrap());
    println!("get_images(120 ids)          {chunk_ms:>10.2}");
    let (fc_all, c) = time(runs, || repo::filter_counts(&conn, None).unwrap());
    let (fc_folder, _) = time(runs, || repo::filter_counts(&conn, folder).unwrap());
    println!("get_filter_counts(all)       {fc_all:>10.2}   total {}", c.total);
    println!("get_filter_counts(folder)    {fc_folder:>10.2}");
    let (cs, _) = time(runs, || repo::catalog_state(&conn, "x", "y").unwrap());
    println!("get_catalog_state            {cs:>10.2}");
    worst = worst.max(chunk_ms).max(fc_all).max(fc_folder).max(cs);
    println!("worst median: {worst:.2} ms (target < 20 ms)");

    // Crash-safety costs at this size: startup integrity check + backup, and per-commit
    // latency of single-row transactions per `synchronous` mode (ingest commits per image).
    let size_mb = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / (1 << 20) as f64;
    let t = Instant::now();
    let ok = db::integrity(&path).unwrap();
    println!("quick_check ({size_mb:.0} MB): {:.0} ms ({ok:?})", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let b = db::backup(&path).unwrap();
    println!("backup (VACUUM INTO): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let _ = std::fs::remove_file(b);
    for (mode, fullfsync) in [("NORMAL", false), ("FULL", false), ("FULL", true)] {
        conn.pragma_update(None, "synchronous", mode).unwrap();
        conn.pragma_update(None, "fullfsync", fullfsync).unwrap();
        let t = Instant::now();
        for id in 1..=500 {
            conn.execute("UPDATE images SET rating = (rating + 1) % 6 WHERE id = ?1", [id]).unwrap();
        }
        println!(
            "synchronous={mode}{}: {:.3} ms/commit",
            if fullfsync { " + fullfsync" } else { "" },
            t.elapsed().as_secs_f64() * 1000.0 / 500.0
        );
    }
}
