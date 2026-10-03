//! Background-activity reporting (IPC v18): emits [`ActivityEvent`]s for the frontend's
//! corner indicator, throttled to at most 10 progress events per second per activity.
//!
//! Two ways to report:
//! - **Channels** for workers whose progress already flows through a sink (ingest, analysis,
//!   export, auto-sync): [`Activities::progress`] starts the activity on its first call for a
//!   channel name and updates it afterwards; [`Activities::finish`] ends it (no-op when the
//!   channel never started).
//! - **Handles** for command-scoped work: [`Activities::start`] returns an [`ActivityHandle`];
//!   call `progress`, then `finish` / `fail` / `cancel`. A handle dropped without ending
//!   (early `?` return, panic) reports `error` ("Stopped unexpectedly").
//!
//! The managed instance lives in Tauri state (`lib.rs`); workers get it from an `AppHandle`
//! with [`activities`]. Tests build one with [`Activities::new`] and a collecting closure.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime};
use tauri_specta::Event;

use super::events::{ActivityEvent, ActivityKind, ActivityState};

/// Minimum gap between two `running` events of one activity (10 Hz).
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

type EmitFn = dyn Fn(&ActivityEvent) + Send + Sync;

struct Running {
    event: ActivityEvent,
    last_emit: Instant,
}

struct Inner {
    emit: Box<EmitFn>,
    next_id: AtomicU32,
    next_handle: AtomicU32,
    channels: Mutex<HashMap<String, Running>>,
}

/// Activity reporter. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Activities {
    inner: Arc<Inner>,
}

/// The app's managed [`Activities`], if set up (absent in unit tests that use an `AppHandle`
/// sink without the full app).
pub fn activities<R: Runtime>(app: &AppHandle<R>) -> Option<Activities> {
    app.try_state::<Activities>().map(|s| s.inner().clone())
}

impl Activities {
    /// Reporter calling `emit` for every event (already throttled).
    pub fn new(emit: impl Fn(&ActivityEvent) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                emit: Box::new(emit),
                next_id: AtomicU32::new(1),
                next_handle: AtomicU32::new(1),
                channels: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Reporter emitting `ActivityEvent` Tauri events on `app`.
    pub fn for_app<R: Runtime>(app: &AppHandle<R>) -> Self {
        let app = app.clone();
        Self::new(move |e| {
            let _ = e.clone().emit(&app);
        })
    }

    fn channels(&self) -> MutexGuard<'_, HashMap<String, Running>> {
        self.inner.channels.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts (first call for `channel`) or updates the activity on `channel`. The start is
    /// always emitted; updates at most every [`MIN_INTERVAL`] (the latest values are kept and
    /// carried by the next emitted event, incl. the terminal one).
    pub fn progress(&self, channel: &str, kind: ActivityKind, label: &str, done: u32, total: Option<u32>) {
        let mut channels = self.channels();
        let now = Instant::now();
        let emit = match channels.get_mut(channel) {
            Some(r) => {
                r.event.done = done;
                r.event.total = total;
                if r.event.label != label {
                    r.event.label = label.to_owned();
                }
                if now.duration_since(r.last_emit) >= MIN_INTERVAL {
                    r.last_emit = now;
                    Some(r.event.clone())
                } else {
                    None
                }
            }
            None => {
                let event = ActivityEvent {
                    id: self.inner.next_id.fetch_add(1, Ordering::Relaxed),
                    kind,
                    label: label.to_owned(),
                    done,
                    total,
                    state: ActivityState::Running,
                    message: None,
                };
                channels.insert(channel.to_owned(), Running { event: event.clone(), last_emit: now });
                Some(event)
            }
        };
        drop(channels);
        if let Some(e) = emit {
            (self.inner.emit)(&e);
        }
    }

    /// Ends the activity on `channel` with `state` (never throttled). Keeps the last reported
    /// `done` / `total`. No-op when nothing runs on `channel`.
    pub fn finish(&self, channel: &str, state: ActivityState, message: Option<String>) {
        let Some(running) = self.channels().remove(channel) else { return };
        let event = ActivityEvent { state, message, ..running.event };
        (self.inner.emit)(&event);
    }

    /// An activity runs on `channel`.
    pub fn is_running(&self, channel: &str) -> bool {
        self.channels().contains_key(channel)
    }

    /// Starts a command-scoped activity (emitted at once) on a channel of its own.
    pub fn start(&self, kind: ActivityKind, label: impl Into<String>, total: Option<u32>) -> ActivityHandle {
        // Unique channel name per handle (worker channels never start with "handle-").
        let channel = format!("handle-{}", self.inner.next_handle.fetch_add(1, Ordering::Relaxed));
        let label = label.into();
        self.progress(&channel, kind, &label, 0, total);
        ActivityHandle { hub: self.clone(), channel, kind, label, total, ended: false }
    }
}

/// A command-scoped activity ([`Activities::start`]).
pub struct ActivityHandle {
    hub: Activities,
    channel: String,
    kind: ActivityKind,
    label: String,
    total: Option<u32>,
    ended: bool,
}

impl ActivityHandle {
    /// Reports `done` of the total given at start (throttled).
    pub fn progress(&self, done: u32) {
        self.hub.progress(&self.channel, self.kind, &self.label, done, self.total);
    }

    /// Reports `done` of a new `total` (throttled).
    pub fn progress_of(&mut self, done: u32, total: Option<u32>) {
        self.total = total;
        self.progress(done);
    }

    fn end(mut self, state: ActivityState, message: Option<String>) {
        self.ended = true;
        self.hub.finish(&self.channel, state, message);
    }

    /// Ended normally (`message`: summary, e.g. "Saved 120 photos; 2 failed").
    pub fn finish(self, message: Option<String>) {
        self.end(ActivityState::Finished, message);
    }

    /// Ended by an error.
    pub fn fail(self, message: impl Into<String>) {
        self.end(ActivityState::Error, Some(message.into()));
    }

    /// Stopped by the user.
    pub fn cancel(self, message: Option<String>) {
        self.end(ActivityState::Cancelled, message);
    }
}

impl Drop for ActivityHandle {
    fn drop(&mut self) {
        if !self.ended {
            self.hub.finish(&self.channel, ActivityState::Error, Some("Stopped unexpectedly".to_owned()));
        }
    }
}

/// Terminal message of an XMP save: "Saved metadata for 12 photos" (+ "; 3 sidecars could not
/// be written").
pub fn xmp_message(saved: usize, failed: usize) -> String {
    let saved = format!("Saved metadata for {}", photos(saved as u32));
    match failed {
        0 => saved,
        1 => format!("{saved}; 1 sidecar could not be written"),
        f => format!("{saved}; {f} sidecars could not be written"),
    }
}

/// `"1 photo"` / `"12 photos"` for activity labels and messages.
pub fn photos(n: u32) -> String {
    if n == 1 {
        "1 photo".to_owned()
    } else {
        format!("{n} photos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collecting() -> (Activities, Arc<Mutex<Vec<ActivityEvent>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        (Activities::new(move |e| sink.lock().unwrap().push(e.clone())), log)
    }

    #[test]
    fn channel_starts_throttles_and_finishes() {
        let (a, log) = collecting();
        for done in 0..=1000 {
            a.progress("import", ActivityKind::Import, "Importing photos", done, Some(1000));
        }
        assert!(a.is_running("import"));
        a.finish("import", ActivityState::Finished, Some("1000 photos imported".into()));
        assert!(!a.is_running("import"));
        let log = log.lock().unwrap();
        assert_eq!(log[0].state, ActivityState::Running);
        assert_eq!(log[0].done, 0);
        // A tight loop takes well under a second: throttled to a handful of events.
        assert!(log.len() <= 4, "throttled: {} events", log.len());
        let last = log.last().unwrap();
        assert_eq!((last.state, last.done, last.total), (ActivityState::Finished, 1000, Some(1000)));
        assert_eq!(last.message.as_deref(), Some("1000 photos imported"));
        assert!(log.iter().all(|e| e.id == log[0].id));
        // Finishing a channel that is not running is a no-op.
        drop(log);
        a.finish("import", ActivityState::Finished, None);
    }

    #[test]
    fn progress_after_the_interval_is_emitted() {
        let (a, log) = collecting();
        a.progress("x", ActivityKind::Other, "Working", 0, None);
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(10));
        a.progress("x", ActivityKind::Other, "Working", 5, None);
        assert_eq!(log.lock().unwrap().iter().map(|e| e.done).collect::<Vec<_>>(), [0, 5]);
    }

    #[test]
    fn handles_have_unique_ids_and_report_errors_when_dropped() {
        let (a, log) = collecting();
        let h1 = a.start(ActivityKind::XmpSave, "Saving metadata to XMP", Some(3));
        let h2 = a.start(ActivityKind::Export, "Exporting 2 photos", Some(2));
        h1.progress(3);
        h1.finish(Some("Saved 3 photos".into()));
        drop(h2);
        let log = log.lock().unwrap();
        let ids: std::collections::HashSet<u32> = log.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 2);
        let end1 = log.iter().find(|e| e.kind == ActivityKind::XmpSave && e.state != ActivityState::Running).unwrap();
        assert_eq!((end1.state, end1.done), (ActivityState::Finished, 3));
        let end2 = log.iter().find(|e| e.kind == ActivityKind::Export && e.state != ActivityState::Running).unwrap();
        assert_eq!(end2.state, ActivityState::Error);
        assert_eq!(photos(1), "1 photo");
    }
}
