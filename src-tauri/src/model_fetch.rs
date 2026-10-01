//! Model installation for the packaged app (Rust port of `scripts/fetch-models.sh`).
//!
//! The release bundle ships the culling models (~27 MB) in `Contents/Resources/models`.
//! The AI-mask segmentation models (~560 MB) are downloaded on first use into
//! `<app_data_dir>/models`, which is also the directory the segmenter reads: the bundled face
//! models are symlinked into it by [`link_bundled`] (people / person-part masks need them).
//! Those links are absolute paths into the bundle, so [`link_bundled`] re-validates them on
//! every release start (dangling / stale links are repointed or removed; regular files kept).
//!
//! Downloads go through the system `curl` (TLS, proxies, redirects, resume) into `<name>.part`,
//! are verified against the pinned SHA-256 in `models/checksums.sha256` (compiled in), and only
//! then renamed into place, so a model file is either absent or verified. Memory stays flat:
//! hashing streams in 1 MiB chunks.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::ipc::error::{AppError, AppResult};
use crate::ipc::events::{ModelDownloadFinished, ModelDownloadProgress};
use crate::ipc::types::{ModelDownloadStatus, ModelFileStatus, ModelGroupStatus, MODEL_GROUP_SEGMENTATION};

/// Pinned SHA-256 of every model (shared with `scripts/fetch-models.sh`).
const CHECKSUMS: &str = include_str!("../models/checksums.sha256");

/// Culling models bundled into the app (`build.rs` `BUNDLED_MODELS`).
pub const BUNDLED: &[&str] =
    &["det_10g.onnx", "2d106det.onnx", "open_closed_eye.onnx", "face_landmarks_detector_1x3x256x256.onnx"];

const HF: &str = "https://huggingface.co";

/// Segmentation models downloaded on first use: (file name, URL, size in bytes).
/// URLs are the pinned revisions in `scripts/fetch-models.sh` (a unit test cross-checks).
const SEGMENTATION: &[(&str, &str, u64)] = &[
    (
        "birefnet_lite.onnx",
        "onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
        224_005_088,
    ),
    ("skyseg.onnx", "JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx", 175_997_079),
    (
        "yolox_m.onnx",
        "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_m.onnx",
        101_259_744,
    ),
    (
        "efficientsam_ti_encoder.onnx",
        "yunyangx/EfficientSAM/resolve/1cf49585c39567bfc49e991ab8eb31f491ad4877/efficientsam_ti_encoder.onnx",
        24_799_761,
    ),
    (
        "efficientsam_ti_decoder.onnx",
        "yunyangx/EfficientSAM/resolve/1cf49585c39567bfc49e991ab8eb31f491ad4877/efficientsam_ti_decoder.onnx",
        16_565_728,
    ),
    (
        "selfie_multiclass_256x256.onnx",
        "senty-au/selfie_multiclass_256x256-ONNX/resolve/6db8421a7150ac20558f2c24675078eb3a1a04d0/onnx/model.onnx",
        16_454_560,
    ),
];

/// One file to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub name: String,
    pub url: String,
    pub sha256: String,
    /// Expected size (progress total; also a cheap pre-check before hashing).
    pub size: u64,
}

/// Progress of [`fetch`], reported about every 200 ms while downloading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchProgress<'a> {
    pub name: &'a str,
    /// 0-based index of the current file among the files being fetched.
    pub file_index: usize,
    pub file_count: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug)]
pub enum FetchError {
    Io(io::Error),
    /// `curl` exited unsuccessfully (network error, HTTP error, ...).
    Download {
        name: String,
        detail: String,
    },
    Checksum {
        name: String,
        expected: String,
        actual: String,
    },
    Cancelled,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Io(e) => write!(f, "model download: {e}"),
            FetchError::Download { name, detail } => write!(f, "downloading {name} failed: {detail}"),
            FetchError::Checksum { name, expected, actual } => {
                write!(f, "{name}: SHA-256 mismatch (expected {expected}, got {actual})")
            }
            FetchError::Cancelled => write!(f, "model download cancelled"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> Self {
        FetchError::Io(e)
    }
}

/// Pinned SHA-256 of `name` from `models/checksums.sha256`.
pub fn checksum(name: &str) -> Option<&'static str> {
    CHECKSUMS.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (sum, file) = (it.next()?, it.next()?);
        (file.trim_start_matches('*') == name).then_some(sum)
    })
}

/// The AI-mask models, in download order.
pub fn segmentation_downloads() -> Vec<Download> {
    SEGMENTATION
        .iter()
        .map(|&(name, url, size)| Download {
            name: name.to_owned(),
            url: if url.starts_with("https://") { url.to_owned() } else { format!("{HF}/{url}") },
            sha256: checksum(name).expect("every model has a pinned checksum").to_owned(),
            size,
        })
        .collect()
}

/// Downloads not yet installed in `dir` (cheap check: present with the expected size;
/// installed files were hash-verified before being moved into place).
pub fn missing(dir: &Path, downloads: &[Download]) -> Vec<Download> {
    downloads.iter().filter(|d| fs::metadata(dir.join(&d.name)).map_or(true, |m| m.len() != d.size)).cloned().collect()
}

/// Streaming SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What [`link_bundled`] does (or, via [`plan_bundled_links`], would do) for one bundled model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Already a symlink to the current bundled model.
    Keep,
    /// No entry yet: link it.
    Create,
    /// A symlink to somewhere else (an older app location, a cleaned `target/` bundle):
    /// repoint it at the current bundled model.
    Repoint { from: PathBuf },
    /// A dangling symlink and no bundled model to point it at: remove it so nothing mistakes
    /// it for an installed model (it is relinked on the next start with resources present).
    RemoveDangling { from: PathBuf },
    /// A regular file (a downloaded / hand-installed model): never touched.
    KeepRegular,
    /// Nothing usable either way (no entry, no bundled model), or a working symlink elsewhere
    /// while the bundled model is missing (kept: better than no model).
    Skip,
}

/// Read-only: the action [`link_bundled`] would take for each [`BUNDLED`] model.
pub fn plan_bundled_links(bundled_dir: &Path, dir: &Path) -> Vec<(&'static str, LinkAction)> {
    BUNDLED
        .iter()
        .map(|&name| {
            let target = bundled_dir.join(name);
            let link = dir.join(name);
            let have_target = target.is_file();
            let action = match fs::symlink_metadata(&link) {
                Ok(m) if m.file_type().is_symlink() => {
                    let from = fs::read_link(&link).unwrap_or_default();
                    // `is_file` follows the link: false for a dangling one.
                    let resolves = link.is_file();
                    if from == target && resolves {
                        LinkAction::Keep
                    } else if have_target {
                        LinkAction::Repoint { from }
                    } else if !resolves {
                        LinkAction::RemoveDangling { from }
                    } else {
                        LinkAction::Skip
                    }
                }
                Ok(_) => LinkAction::KeepRegular,
                Err(_) if have_target => LinkAction::Create,
                Err(_) => LinkAction::Skip,
            };
            (name, action)
        })
        .collect()
}

/// Symlinks the bundled culling models into `dir` (the writable models dir the segmenter
/// reads), repairing links left dangling or stale by a moved app / repo or a cleaned `target/`
/// (links are absolute paths into the bundle, so this runs at every release start).
/// Regular files in `dir` (downloaded models) are never touched. Every model is attempted;
/// the first error is returned. Repairs are logged to stderr.
pub fn link_bundled(bundled_dir: &Path, dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut first_err = None;
    for (name, action) in plan_bundled_links(bundled_dir, dir) {
        let target = bundled_dir.join(name);
        let link = dir.join(name);
        let result = match &action {
            LinkAction::Keep | LinkAction::KeepRegular | LinkAction::Skip => Ok(()),
            LinkAction::Create => std::os::unix::fs::symlink(&target, &link),
            LinkAction::Repoint { from } => {
                eprintln!("models: repointing {} ({} -> {})", link.display(), from.display(), target.display());
                fs::remove_file(&link).and_then(|()| std::os::unix::fs::symlink(&target, &link))
            }
            LinkAction::RemoveDangling { from } => {
                eprintln!(
                    "models: removing dangling {} -> {} (bundled model missing at {})",
                    link.display(),
                    from.display(),
                    target.display()
                );
                fs::remove_file(&link)
            }
        };
        if let Err(e) = result {
            eprintln!("models: {name}: {action:?} failed: {e}");
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

fn curl() -> PathBuf {
    let system = Path::new("/usr/bin/curl");
    if system.is_file() {
        system.to_path_buf()
    } else {
        PathBuf::from("curl")
    }
}

fn spawn_curl(url: &str, part: &Path, resume: bool) -> io::Result<Child> {
    let mut cmd = Command::new(curl());
    cmd.args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--retry-delay", "2"]);
    if resume {
        cmd.args(["--continue-at", "-"]);
    }
    cmd.arg("--output").arg(part).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    cmd.spawn()
}

/// Downloads `downloads` into `dir` (created if needed), skipping files already installed with
/// a matching checksum. Partial `.part` files are resumed. `cancel` is polled while downloading;
/// a cancelled or failed file leaves no final file behind (the `.part` is kept for resume
/// unless its checksum failed).
pub fn fetch(
    dir: &Path,
    downloads: &[Download],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(FetchProgress<'_>),
) -> Result<(), FetchError> {
    fs::create_dir_all(dir)?;
    let bytes_total: u64 = downloads.iter().map(|d| d.size).sum();
    let mut bytes_before = 0u64;
    for (file_index, d) in downloads.iter().enumerate() {
        let dest = dir.join(&d.name);
        let base = bytes_before;
        let mut report = |done: u64| {
            on_progress(FetchProgress {
                name: &d.name,
                file_index,
                file_count: downloads.len(),
                bytes_done: base + done.min(d.size),
                bytes_total,
            })
        };
        if !(dest.is_file() && sha256_file(&dest)? == d.sha256) {
            let part = dir.join(format!("{}.part", d.name));
            download_one(d, &part, cancel, &mut report)?;
            let actual = sha256_file(&part)?;
            if actual != d.sha256 {
                let _ = fs::remove_file(&part);
                return Err(FetchError::Checksum { name: d.name.clone(), expected: d.sha256.clone(), actual });
            }
            fs::rename(&part, &dest)?;
        }
        report(d.size);
        bytes_before += d.size;
    }
    Ok(())
}

fn download_one(
    d: &Download,
    part: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), FetchError> {
    // A complete-sized `.part` from an interrupted run only needs verifying.
    let existing = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if existing == d.size {
        return Ok(());
    }
    if existing > d.size {
        fs::remove_file(part)?;
    }
    let mut resume = existing > 0 && existing < d.size;
    loop {
        let mut child = spawn_curl(&d.url, part, resume)?;
        let status = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FetchError::Cancelled);
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            progress(fs::metadata(part).map(|m| m.len()).unwrap_or(0));
            std::thread::sleep(Duration::from_millis(200));
        };
        if status.success() {
            return Ok(());
        }
        let mut detail = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut detail);
        }
        // The server refused the range request: start over once from byte 0.
        if resume && status.code() == Some(33) {
            fs::remove_file(part)?;
            resume = false;
            continue;
        }
        return Err(FetchError::Download { name: d.name.clone(), detail: detail.trim().to_owned() });
    }
}

// ---------------------------------------------------------------------------
// In-app downloads (IPC v12: `model_downloads_status`, `download_models`, `cancel_model_download`)
// ---------------------------------------------------------------------------

/// A downloadable set of models (`ModelGroupStatus.id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGroup {
    pub id: String,
    pub label: String,
    pub downloads: Vec<Download>,
}

/// The groups the app can download, in display order.
pub fn model_groups() -> Vec<ModelGroup> {
    vec![ModelGroup {
        id: MODEL_GROUP_SEGMENTATION.to_owned(),
        label: "AI masking models".to_owned(),
        downloads: segmentation_downloads(),
    }]
}

/// Minimum spacing of `ModelDownloadProgress` events for the same file.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Default)]
struct DownloadState {
    /// Group id in flight.
    downloading: Option<String>,
}

/// Managed state: at most one background download at a time into the segmenter's models
/// directory (`<app_data_dir>/models` in release, `src-tauri/models` in dev, or
/// `SIEVE_MODELS`). The segmenter checks model files on every call, so installed models are
/// usable without a restart. Cheap to clone.
#[derive(Clone)]
pub struct ModelDownloads {
    dir: PathBuf,
    groups: Arc<Vec<ModelGroup>>,
    state: Arc<Mutex<DownloadState>>,
    cancel: Arc<AtomicBool>,
}

impl ModelDownloads {
    /// No I/O.
    pub fn new(dir: PathBuf) -> Self {
        Self::with_groups(dir, model_groups())
    }

    /// Explicit groups (tests).
    pub fn with_groups(dir: PathBuf, groups: Vec<ModelGroup>) -> Self {
        Self {
            dir,
            groups: Arc::new(groups),
            state: Arc::new(Mutex::new(DownloadState::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DownloadState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Installed files per group (size check only; cheap) + the download in flight.
    pub fn status(&self) -> ModelDownloadStatus {
        let groups = self
            .groups
            .iter()
            .map(|g| {
                let missing: Vec<String> = missing(&self.dir, &g.downloads).into_iter().map(|d| d.name).collect();
                let files: Vec<ModelFileStatus> = g
                    .downloads
                    .iter()
                    .map(|d| ModelFileStatus {
                        name: d.name.clone(),
                        installed: !missing.contains(&d.name),
                        bytes: d.size,
                    })
                    .collect();
                ModelGroupStatus {
                    id: g.id.clone(),
                    label: g.label.clone(),
                    installed: missing.is_empty(),
                    bytes_total: g.downloads.iter().map(|d| d.size).sum(),
                    files,
                }
            })
            .collect();
        ModelDownloadStatus { groups, downloading: self.lock().downloading.clone() }
    }

    /// Starts downloading `group` on a background thread and returns immediately.
    /// `on_progress` gets throttled progress; `on_finished` is called exactly once at the end
    /// (after the in-flight marker is cleared, so `status()` is already idle).
    /// Errors: `invalid_argument` for an unknown group or while another download is running.
    pub fn start(
        &self,
        group: &str,
        mut on_progress: impl FnMut(ModelDownloadProgress) + Send + 'static,
        on_finished: impl FnOnce(ModelDownloadFinished) + Send + 'static,
    ) -> AppResult<()> {
        let g = self
            .groups
            .iter()
            .find(|g| g.id == group)
            .cloned()
            .ok_or_else(|| AppError::invalid(format!("unknown model group `{group}`")))?;
        {
            let mut st = self.lock();
            if let Some(running) = &st.downloading {
                return Err(AppError::invalid(format!("the {running} models are already downloading")));
            }
            st.downloading = Some(g.id.clone());
            self.cancel.store(false, Ordering::SeqCst);
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("model-download".into()).spawn(move || {
            let mut last: Option<(usize, u64, std::time::Instant)> = None;
            let result = fetch(&this.dir, &g.downloads, &this.cancel, |p| {
                let now = std::time::Instant::now();
                let emit = match last {
                    None => true,
                    Some((file, done, at)) => {
                        // New file, file completed, or enough time passed with new bytes.
                        p.file_index != file
                            || (p.bytes_done != done
                                && (now.duration_since(at) >= PROGRESS_INTERVAL
                                    || p.bytes_done == file_end(&g.downloads, p.file_index)))
                    }
                };
                if emit {
                    last = Some((p.file_index, p.bytes_done, now));
                    on_progress(ModelDownloadProgress {
                        group: g.id.clone(),
                        name: p.name.to_owned(),
                        file_index: p.file_index as u32,
                        file_count: p.file_count as u32,
                        bytes_done: p.bytes_done,
                        bytes_total: p.bytes_total,
                    });
                }
            });
            this.lock().downloading = None;
            let cancelled = matches!(result, Err(FetchError::Cancelled));
            on_finished(ModelDownloadFinished {
                group: g.id.clone(),
                ok: result.is_ok(),
                cancelled,
                error: result.err().map(|e| e.to_string()),
            });
        });
        if let Err(e) = spawned {
            self.lock().downloading = None;
            return Err(AppError::internal(format!("starting the model download: {e}")));
        }
        Ok(())
    }

    /// Stops the download in flight (no-op when idle); its `ModelDownloadFinished` follows
    /// with `cancelled: true`. The partial file is kept and resumed by the next download.
    pub fn cancel(&self) {
        if self.lock().downloading.is_some() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Cumulative group bytes at the end of file `index`.
fn file_end(downloads: &[Download], index: usize) -> u64 {
    downloads.iter().take(index + 1).map(|d| d.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(dir: &Path, name: &str, body: &[u8]) -> Download {
        let src = dir.join(format!("src-{name}"));
        fs::write(&src, body).unwrap();
        Download {
            name: name.to_owned(),
            url: format!("file://{}", src.display()),
            sha256: sha256_file(&src).unwrap(),
            size: body.len() as u64,
        }
    }

    #[test]
    fn every_model_has_a_checksum_and_urls_match_the_script() {
        let script = include_str!("../../scripts/fetch-models.sh");
        for name in BUNDLED {
            assert!(checksum(name).is_some_and(|s| s.len() == 64), "{name}");
        }
        let downloads = segmentation_downloads();
        assert_eq!(downloads.len(), 6);
        for d in &downloads {
            assert_eq!(d.sha256.len(), 64, "{}", d.name);
            assert!(script.contains(&format!("\"{}|", d.name)), "{} listed in fetch-models.sh", d.name);
            // Pinned revisions / release tags and file paths must agree with the script.
            for token in d.url.split('/').skip(3).filter(|t| t.len() >= 12) {
                assert!(script.contains(token), "{}: `{token}` not in fetch-models.sh", d.name);
            }
        }
        assert_eq!(checksum("nope.onnx"), None);
    }

    #[test]
    fn fetch_verifies_installs_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("models");
        let a = local(tmp.path(), "a.onnx", &vec![7u8; 300_000]);
        let b = local(tmp.path(), "b.onnx", b"small model");
        let all = vec![a.clone(), b.clone()];
        assert_eq!(missing(&dest, &all).len(), 2);

        let mut last = None;
        fetch(&dest, &all, &AtomicBool::new(false), |p| last = Some((p.bytes_done, p.bytes_total, p.file_index)))
            .unwrap();
        assert_eq!(last, Some((a.size + b.size, a.size + b.size, 1)));
        assert_eq!(fs::read(dest.join("b.onnx")).unwrap(), b"small model");
        assert!(missing(&dest, &all).is_empty());
        assert!(!dest.join("a.onnx.part").exists());

        // Already installed: skipped even if the source disappears.
        fs::remove_file(tmp.path().join("src-a.onnx")).unwrap();
        fetch(&dest, &all[..1], &AtomicBool::new(false), |_| {}).unwrap();
    }

    #[test]
    fn checksum_mismatch_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("models");
        let mut bad = local(tmp.path(), "bad.onnx", b"tampered");
        bad.sha256 = "0".repeat(64);
        let err = fetch(&dest, &[bad], &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(matches!(err, FetchError::Checksum { .. }), "{err}");
        assert!(!dest.join("bad.onnx").exists());
        assert!(!dest.join("bad.onnx.part").exists());
    }

    #[test]
    fn download_failure_and_cancel_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("models");
        let gone = Download {
            name: "gone.onnx".into(),
            url: format!("file://{}/does-not-exist", tmp.path().display()),
            sha256: "0".repeat(64),
            size: 10,
        };
        let err = fetch(&dest, std::slice::from_ref(&gone), &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(matches!(err, FetchError::Download { .. }), "{err}");
        assert!(!dest.join("gone.onnx").exists());

        let ok = local(tmp.path(), "c.onnx", b"cancel me");
        let err = fetch(&dest, &[ok], &AtomicBool::new(true), |_| {}).unwrap_err();
        assert!(matches!(err, FetchError::Cancelled));
        assert!(!dest.join("c.onnx").exists());
    }

    #[test]
    fn interrupted_part_is_resumed() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("models");
        fs::create_dir_all(&dest).unwrap();
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let d = local(tmp.path(), "r.onnx", &body);
        fs::write(dest.join("r.onnx.part"), &body[..50_000]).unwrap();
        fetch(&dest, &[d], &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(fs::read(dest.join("r.onnx")).unwrap(), body);
    }

    // ---- ModelDownloads (IPC v12) ----

    use std::sync::mpsc;

    fn group(id: &str, downloads: Vec<Download>) -> ModelGroup {
        ModelGroup { id: id.to_owned(), label: format!("{id} models"), downloads }
    }

    /// Starts `group` and waits for its `ModelDownloadFinished`, collecting progress.
    fn run(dl: &ModelDownloads, id: &str) -> (Vec<ModelDownloadProgress>, ModelDownloadFinished) {
        let (ptx, prx) = mpsc::channel();
        let (ftx, frx) = mpsc::channel();
        dl.start(id, move |p| ptx.send(p).unwrap(), move |f| ftx.send(f).unwrap()).unwrap();
        let finished = frx.recv_timeout(Duration::from_secs(30)).expect("finished event");
        (prx.try_iter().collect(), finished)
    }

    /// An HTTP endpoint that accepts connections and never answers (a stalled download).
    fn stalled_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for s in listener.incoming() {
                held.push(s);
            }
        });
        format!("http://{addr}/model.onnx")
    }

    #[test]
    fn default_groups() {
        let groups = model_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, MODEL_GROUP_SEGMENTATION);
        assert_eq!(groups[0].downloads, segmentation_downloads());
        let st = ModelDownloads::new(PathBuf::from("/nonexistent-models")).status();
        assert!(!st.groups[0].installed && st.downloading.is_none());
        assert_eq!(st.groups[0].bytes_total, SEGMENTATION.iter().map(|m| m.2).sum::<u64>());
    }

    #[test]
    fn status_with_and_without_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        let a = local(tmp.path(), "a.onnx", b"model a");
        let b = local(tmp.path(), "b.onnx", b"model bb");
        let dl = ModelDownloads::with_groups(dir.clone(), vec![group("seg", vec![a.clone(), b.clone()])]);

        let st = dl.status();
        assert_eq!(st.downloading, None);
        let g = &st.groups[0];
        assert_eq!((g.id.as_str(), g.installed, g.bytes_total), ("seg", false, a.size + b.size));
        assert!(g.files.iter().all(|f| !f.installed));
        assert_eq!(g.files[1], ModelFileStatus { name: "b.onnx".into(), installed: false, bytes: b.size });

        // One file present; a wrong-sized file does not count.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.onnx"), b"model a").unwrap();
        fs::write(dir.join("b.onnx"), b"short").unwrap();
        let g = dl.status().groups.remove(0);
        assert!(!g.installed && g.files[0].installed && !g.files[1].installed);

        fs::remove_file(dir.join("b.onnx")).unwrap();
        let (progress, finished) = run(&dl, "seg");
        assert_eq!(finished, ModelDownloadFinished { group: "seg".into(), ok: true, cancelled: false, error: None });
        let last = progress.last().unwrap();
        assert_eq!((last.file_index, last.file_count, last.bytes_done, last.bytes_total), (1, 2, 15, 15));
        assert!(progress.windows(2).all(|w| w[0].bytes_done <= w[1].bytes_done), "{progress:?}");
        let st = dl.status();
        assert!(st.groups[0].installed && st.downloading.is_none());
    }

    #[test]
    fn unknown_group_is_rejected() {
        let dl = ModelDownloads::with_groups(PathBuf::from("/nonexistent"), vec![]);
        let err = dl.start("nope", |_| {}, |_| {}).unwrap_err();
        assert_eq!(err.kind, crate::ipc::error::ErrorKind::InvalidArgument);
        dl.cancel(); // idle: no-op
    }

    #[test]
    fn cancel_stops_the_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        let stalled = Download { name: "s.onnx".into(), url: stalled_url(), sha256: "0".repeat(64), size: 1_000 };
        let dl = ModelDownloads::with_groups(dir.clone(), vec![group("seg", vec![stalled])]);
        let (ftx, frx) = mpsc::channel();
        dl.start("seg", |_| {}, move |f| ftx.send(f).unwrap()).unwrap();
        assert_eq!(dl.status().downloading.as_deref(), Some("seg"));
        // One download at a time.
        let busy = dl.start("seg", |_| {}, |_| {}).unwrap_err();
        assert_eq!(busy.kind, crate::ipc::error::ErrorKind::InvalidArgument);

        std::thread::sleep(Duration::from_millis(300));
        dl.cancel();
        let f = frx.recv_timeout(Duration::from_secs(10)).expect("finished after cancel");
        assert!(!f.ok && f.cancelled, "{f:?}");
        assert_eq!(f.error.as_deref(), Some("model download cancelled"));
        assert_eq!(dl.status().downloading, None);
        assert!(!dir.join("s.onnx").exists());
    }

    #[test]
    fn checksum_failure_surfaces_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        let good = local(tmp.path(), "good.onnx", b"fine");
        let mut bad = local(tmp.path(), "bad.onnx", b"tampered");
        bad.sha256 = "0".repeat(64);
        let dl = ModelDownloads::with_groups(dir.clone(), vec![group("seg", vec![good, bad])]);
        let (_, f) = run(&dl, "seg");
        assert!(!f.ok && !f.cancelled);
        let err = f.error.unwrap();
        assert!(err.contains("bad.onnx") && err.contains("SHA-256 mismatch"), "{err}");
        let g = dl.status().groups.remove(0);
        assert!(!g.installed && g.files[0].installed && !g.files[1].installed);
        assert!(!dir.join("bad.onnx").exists());
    }

    #[test]
    fn mask_capabilities_follow_installed_files_without_restart() {
        use crate::develop::masks::{MaskCache, MaskCacheConfig};
        use crate::ipc::types::AiTargetKind;
        use crate::ml::masking::{Segmenter, SegmenterConfig};

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        let cache = MaskCache::new(MaskCacheConfig {
            catalog_path: tmp.path().join("catalog.sqlite"),
            cache_dir: tmp.path().join("cache"),
        });
        let segmenter = Segmenter::new(
            SegmenterConfig { models_dir: dir.clone(), catalog_path: tmp.path().join("catalog.sqlite") },
            cache,
        );
        let subject = |s: &Segmenter| {
            s.capabilities().ai.into_iter().find(|c| c.kind == AiTargetKind::Subject).map(|c| c.available)
        };
        assert_eq!(subject(&segmenter), Some(false));
        let reason = segmenter.capabilities().ai.into_iter().find(|c| c.kind == AiTargetKind::Subject).unwrap().reason;
        assert_eq!(reason.as_deref(), Some(crate::ml::masking::MODELS_NOT_INSTALLED), "points at the download button");
        // What a finished download leaves behind: the verified file under its final name.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("birefnet_lite.onnx"), b"stand-in").unwrap();
        assert_eq!(subject(&segmenter), Some(true));
    }

    fn bundle_with_models(root: &Path) -> PathBuf {
        let bundle = root.join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        for name in BUNDLED {
            fs::write(bundle.join(name), name.as_bytes()).unwrap();
        }
        bundle
    }

    #[test]
    fn link_bundled_symlinks_and_repairs() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = bundle_with_models(tmp.path());
        let data = tmp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        // Dangling link from a moved repo / cleaned `target/`.
        std::os::unix::fs::symlink(tmp.path().join("old/det_10g.onnx"), data.join("det_10g.onnx")).unwrap();
        // Working link to another (older) app location.
        let other = tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("2d106det.onnx"), b"old").unwrap();
        std::os::unix::fs::symlink(other.join("2d106det.onnx"), data.join("2d106det.onnx")).unwrap();

        let plan = plan_bundled_links(&bundle, &data);
        assert_eq!(plan[0], ("det_10g.onnx", LinkAction::Repoint { from: tmp.path().join("old/det_10g.onnx") }));
        assert_eq!(plan[1], ("2d106det.onnx", LinkAction::Repoint { from: other.join("2d106det.onnx") }));
        assert_eq!(plan[2], ("open_closed_eye.onnx", LinkAction::Create));
        // Planning is read-only.
        assert!(data.join("det_10g.onnx").symlink_metadata().is_ok() && !data.join("det_10g.onnx").exists());

        link_bundled(&bundle, &data).unwrap();
        link_bundled(&bundle, &data).unwrap();
        for name in BUNDLED {
            assert_eq!(fs::read(data.join(name)).unwrap(), name.as_bytes());
            assert_eq!(fs::read_link(data.join(name)).unwrap(), bundle.join(name));
        }
        assert!(plan_bundled_links(&bundle, &data).iter().all(|(_, a)| *a == LinkAction::Keep));
    }

    #[test]
    fn link_bundled_never_touches_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = bundle_with_models(tmp.path());
        let data = tmp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("det_10g.onnx"), b"user copy").unwrap();
        fs::write(data.join("birefnet_lite.onnx"), b"downloaded").unwrap();
        assert_eq!(plan_bundled_links(&bundle, &data)[0].1, LinkAction::KeepRegular);
        link_bundled(&bundle, &data).unwrap();
        assert!(!data.join("det_10g.onnx").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(data.join("det_10g.onnx")).unwrap(), b"user copy");
        assert_eq!(fs::read(data.join("birefnet_lite.onnx")).unwrap(), b"downloaded");

        // Same with no bundle at all.
        link_bundled(&tmp.path().join("no-bundle"), &data).unwrap();
        assert_eq!(fs::read(data.join("det_10g.onnx")).unwrap(), b"user copy");
        assert_eq!(fs::read(data.join("birefnet_lite.onnx")).unwrap(), b"downloaded");
    }

    #[test]
    fn link_bundled_with_missing_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("bundle"); // not created yet
        let data = tmp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        // Dangling: removed (nothing to repoint it at), even when it names the current target.
        let gone = tmp.path().join("old/det_10g.onnx");
        std::os::unix::fs::symlink(&gone, data.join("det_10g.onnx")).unwrap();
        std::os::unix::fs::symlink(bundle.join("2d106det.onnx"), data.join("2d106det.onnx")).unwrap();
        // Working link elsewhere: kept (better than no model).
        let other = tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("open_closed_eye.onnx"), b"eye").unwrap();
        std::os::unix::fs::symlink(other.join("open_closed_eye.onnx"), data.join("open_closed_eye.onnx")).unwrap();

        let plan = plan_bundled_links(&bundle, &data);
        assert_eq!(plan[0].1, LinkAction::RemoveDangling { from: gone });
        assert_eq!(plan[1].1, LinkAction::RemoveDangling { from: bundle.join("2d106det.onnx") });
        assert_eq!(plan[2].1, LinkAction::Skip);
        assert_eq!(plan[3].1, LinkAction::Skip);

        link_bundled(&bundle, &data).unwrap();
        assert!(data.join("det_10g.onnx").symlink_metadata().is_err());
        assert!(data.join("2d106det.onnx").symlink_metadata().is_err());
        assert_eq!(fs::read(data.join("open_closed_eye.onnx")).unwrap(), b"eye");
        assert!(data.join(BUNDLED[3]).symlink_metadata().is_err());

        // Resources appear (next start from a real bundle): everything linked.
        let bundle = bundle_with_models(tmp.path());
        link_bundled(&bundle, &data).unwrap();
        for name in BUNDLED {
            assert_eq!(fs::read_link(data.join(name)).unwrap(), bundle.join(name));
        }
    }

    #[test]
    fn dangling_links_are_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        let a = local(tmp.path(), "a.onnx", b"model a");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("gone/a.onnx"), dir.join("a.onnx")).unwrap();
        let dl = ModelDownloads::with_groups(dir.clone(), vec![group("seg", vec![a.clone()])]);
        let g = dl.status().groups.remove(0);
        assert!(!g.installed && !g.files[0].installed);
        // A link that resolves to the right file counts.
        fs::remove_file(dir.join("a.onnx")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("src-a.onnx"), dir.join("a.onnx")).unwrap();
        assert!(dl.status().groups[0].installed);
    }

    /// Read-only dry run against the real app-data dir (never modifies it):
    /// `SIEVE_RESOURCE_MODELS=<app>/Contents/Resources/models cargo test real_app_data_dry_run -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_app_data_dry_run() {
        let home = std::env::var_os("HOME").unwrap();
        let data = Path::new(&home).join("Library/Application Support/com.sieve.app/models");
        let bundled = std::env::var_os("SIEVE_RESOURCE_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Applications/Sieve.app/Contents/Resources/models"));
        println!("resource models: {} (exists: {})", bundled.display(), bundled.is_dir());
        for (name, action) in plan_bundled_links(&bundled, &data) {
            println!("{name}: {action:?}");
        }
    }
}
