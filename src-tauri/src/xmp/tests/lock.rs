//! Phase 8c "database is locked" on Save: an explicit save of a whole shoot while the
//! auto-sync pass and UI rating writes run against the same catalog.

use std::sync::atomic::AtomicBool;

use super::*;

const IMAGES: usize = 2_400;

#[test]
fn explicit_save_during_auto_sync_and_rating_writes_is_not_locked() {
    let f = Fixture::new(IMAGES);
    let mut conn = f.conn();
    repo::set_xmp_auto_sync(&conn, true).unwrap();
    repo::set_rating(&mut conn, &f.ids, 3).unwrap();
    assert_eq!(store::dirty_ids(&conn).unwrap().len(), IMAGES);

    // UI stand-in: keyboard ratings on its own connection, as the command mutex's does.
    let stop = Arc::new(AtomicBool::new(false));
    let ui = {
        let stop = stop.clone();
        let catalog = f.sync.config().catalog_path.clone();
        let ids = f.ids.clone();
        std::thread::spawn(move || {
            let mut conn = db::open(&catalog).unwrap();
            let (mut ok, mut errors) = (0usize, Vec::new());
            let mut i = 0usize;
            while !stop.load(Ordering::SeqCst) {
                let id = ids[(i * 7919) % ids.len()];
                match repo::set_rating(&mut conn, &[id], (i % 6) as u8) {
                    Ok(()) => ok += 1,
                    Err(e) => errors.push(e.message),
                }
                i += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            (ok, errors)
        })
    };

    // Auto pass starts first (debounce 50 ms), then Cmd+S.
    let (s, rx) = sink();
    f.sync.notify_with(s);
    std::thread::sleep(Duration::from_millis(120));
    let started = Instant::now();
    let report = f.sync.write_images(&f.ids);
    let elapsed = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    let (ui_ok, ui_errors) = ui.join().unwrap();
    wait_idle_for(&f.sync, Duration::from_secs(120));
    let auto_failures: Vec<XmpWriteFailed> = rx.try_iter().filter_map(Result::err).collect();

    let report = report.unwrap_or_else(|e| panic!("explicit save failed: {e:?}"));
    println!(
        "explicit save of {IMAGES}: {} written, {} failed in {elapsed:?}; ui writes ok {ui_ok}, failed {}; \
         auto-pass failures {}",
        report.succeeded,
        report.failed.len(),
        ui_errors.len(),
        auto_failures.len()
    );
    if let Some(first) = report.failed.first() {
        println!("first explicit failure: {}", first.reason);
    }
    assert!(report.failed.is_empty(), "{} sidecars failed, first: {:?}", report.failed.len(), report.failed.first());
    assert_eq!(report.succeeded as usize, IMAGES);
    assert!(ui_errors.is_empty(), "UI writes failed: {:?}", &ui_errors[..ui_errors.len().min(3)]);
    assert!(auto_failures.is_empty(), "auto pass failures: {:?}", &auto_failures[..auto_failures.len().min(3)]);
    assert!(ui_ok > 0);
    // Every sidecar exists; the last UI ratings are dirty or written, never lost.
    assert!((0..IMAGES).all(|i| f.sidecar(i).exists()));
}

fn wait_idle_for(sync: &XmpSync, limit: Duration) {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        if !lock_ignore_poison(&sync.state.0).alive {
            return;
        }
    }
    panic!("auto-sync worker did not finish");
}

/// A catalog that stays locked past the busy timeout fails the save once, with one clear
/// error, instead of one "database is locked" per sidecar.
#[test]
fn catalog_lock_beyond_timeout_is_one_error() {
    let f = Fixture::new(5);
    let mut conn = f.conn();
    repo::set_rating(&mut conn, &f.ids, 2).unwrap();
    let blocker = f.conn();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let err = f.sync.write_images(&f.ids).unwrap_err();
    blocker.execute_batch("ROLLBACK").unwrap();
    println!("{err:?}");
    assert_eq!(err.kind, crate::ipc::error::ErrorKind::Database);
    assert!(err.message.contains("stopped after 0 of 5 photos") && err.message.contains("locked"), "{}", err.message);
    // Nothing was marked failed per image; the images stay dirty and a retry succeeds.
    assert!(f.ids.iter().all(|&id| f.state(id).0 && f.state(id).3.is_none()));
    assert_eq!(f.sync.write_images(&f.ids).unwrap().succeeded, 5);
}
