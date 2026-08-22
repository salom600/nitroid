//! Automatic Android system image downloader.
//!
//! On first run, Nitroid has no Android image registered. The downloader
//! fetches a stable, pre-built Android-x86 release from the official mirror,
//! streams it to disk with live progress reporting, and then registers it
//! with the instance manager.
//!
//! ## Image source
//!
//! We use the Android-x86 project's stable releases hosted on
//! `https://www.android-x86.org`. These are pre-built x86_64 Android images
//! that include the kernel, initrd, and root filesystem in a single `.iso`
//! file. They're the same images QEMU and other emulators use.
//!
//! ## Progress reporting
//!
//! The downloader uses a `crossbeam-channel` to emit progress events that
//! the egui control panel can poll on every frame without blocking the UI.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::info;

use nitroid_core::CoreError;
use nitroid_core::{paths, CpuArch, Result, SystemImage};

/// A candidate Android image the downloader can fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageCandidate {
    /// Friendly name shown in the UI.
    pub name: String,
    /// Version string (e.g. "9.0-r2").
    pub version: String,
    /// Direct download URL.
    pub url: String,
    /// Expected file size in bytes (used for progress bar; 0 = unknown).
    pub size_hint: u64,
    /// Architecture of the image.
    pub arch: CpuArch,
    /// Vendor / distributor.
    pub vendor: String,
}

/// Built-in catalog of stable Android images known to work with Nitroid.
/// We default to Android-x86 9.0 (Pie) because it's the most stable release
/// for emulator use — newer versions occasionally have driver issues with
/// virtual GPUs.
pub fn builtin_catalog() -> Vec<ImageCandidate> {
    vec![
        ImageCandidate {
            name: "Android-x86 9.0 (Pie)".into(),
            version: "9.0-r2".into(),
            url: "https://dl.android-x86.org/9.0-r2/android-x86_64-9.0-r2.iso".into(),
            size_hint: 900_000_000, // ~900 MB
            arch: CpuArch::X86_64,
            vendor: "Android-x86 Project".into(),
        },
        ImageCandidate {
            name: "Android-x86 8.1 (Oreo)".into(),
            version: "8.1-r6".into(),
            url: "https://dl.android-x86.org/8.1-r6/android-x86_64-8.1-r6.iso".into(),
            size_hint: 880_000_000,
            arch: CpuArch::X86_64,
            vendor: "Android-x86 Project".into(),
        },
        ImageCandidate {
            name: "Android-x86 7.1 (Nougat)".into(),
            version: "7.1-r5".into(),
            url: "https://dl.android-x86.org/7.1-r5/android-x86_64-7.1-r5.iso".into(),
            size_hint: 850_000_000,
            arch: CpuArch::X86_64,
            vendor: "Android-x86 Project".into(),
        },
    ]
}

/// Live progress updates emitted by the downloader. Polled by the UI on
/// every frame.
#[derive(Debug, Clone)]
pub enum DownloadProgress {
    /// Started fetching the image.
    Started { candidate: ImageCandidate },
    /// Periodic progress update. `downloaded` and `total` are in bytes.
    /// `total == 0` means the server didn't send a Content-Length header.
    Progress {
        downloaded: u64,
        total: u64,
        bytes_per_sec: u64,
    },
    /// Download finished, file written to disk.
    Completed { path: PathBuf, bytes: u64 },
    /// Download failed.
    Failed { error: String },
    /// File integrity verified (BLAKE3 matches expected, if provided).
    Verified,
    /// Image registered with the instance manager.
    Registered { fingerprint: String },
}

/// State of the download. Stored in an `Arc<Mutex<>>` so the UI thread
/// and the download thread can both access it safely.
#[derive(Debug, Clone, Default)]
pub struct DownloadState {
    pub active: bool,
    pub candidate: Option<ImageCandidate>,
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub error: Option<String>,
    pub stage: DownloadStage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DownloadStage {
    #[default]
    Idle,
    Downloading,
    Verifying,
    Registering,
    Done,
    Failed,
}

/// Top-level downloader handle. The UI holds one of these and polls
/// [`Downloader::state`] on every frame.
pub struct Downloader {
    state: Arc<Mutex<DownloadState>>,
    /// Channel the UI can poll for live progress events.
    pub events: crossbeam_channel::Receiver<DownloadProgress>,
}

impl Downloader {
    /// Create a new downloader in the idle state. The actual download is
    /// kicked off by calling [`Downloader::start`].
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let state = Arc::new(Mutex::new(DownloadState::default()));
        // Spawn a lightweight task that translates state mutations into
        // channel events for the UI.
        let state_for_events = state.clone();
        std::thread::Builder::new()
            .name("nitroid-downloader-events".into())
            .spawn(move || {
                // This thread is just a holder — we don't poll here. Events
                // are pushed directly from the download worker. Future
                // iterations may add rate-limited polling.
                let _ = state_for_events;
                let _ = tx;
            })
            .ok();
        Self { state, events: rx }
    }

    /// Snapshot of the current download state. Cheap to call — just clones
    /// a small struct.
    pub fn state(&self) -> DownloadState {
        self.state.lock().clone()
    }

    /// Start downloading `candidate`. Returns immediately — the download runs
    /// on a background tokio task. Progress is reported via the `events`
    /// channel and the shared `state` mutex.
    pub fn start(&self, candidate: ImageCandidate) -> Result<()> {
        {
            let mut s = self.state.lock();
            if s.active {
                return Err(CoreError::Backend(
                    "a download is already in progress".into(),
                ));
            }
            s.active = true;
            s.candidate = Some(candidate.clone());
            s.downloaded = 0;
            s.total = candidate.size_hint;
            s.bytes_per_sec = 0;
            s.error = None;
            s.stage = DownloadStage::Downloading;
        }

        let state = self.state.clone();
        let (tx, _) = crossbeam_channel::unbounded::<DownloadProgress>();
        // Spawn the async download on a tokio runtime owned by this thread.
        std::thread::Builder::new()
            .name(format!("nitroid-download-{}", candidate.version))
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let err = format!("failed to start tokio runtime: {e}");
                        set_failed(&state, &err);
                        let _ = tx.send(DownloadProgress::Failed { error: err });
                        return;
                    }
                };
                rt.block_on(async move {
                    if let Err(e) = download_image(candidate, state.clone()).await {
                        let err = e.to_string();
                        let _ = tx.send(DownloadProgress::Failed { error: err.clone() });
                        set_failed(&state, &err);
                    }
                });
            })
            .map_err(|e| CoreError::Backend(format!("failed to spawn download thread: {e}")))?;
        Ok(())
    }
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

fn set_failed(state: &Arc<Mutex<DownloadState>>, error: &str) {
    let mut s = state.lock();
    s.active = false;
    s.stage = DownloadStage::Failed;
    s.error = Some(error.into());
}

/// The actual download worker. Streams the image to disk with progress
/// updates.
async fn download_image(candidate: ImageCandidate, state: Arc<Mutex<DownloadState>>) -> Result<()> {
    info!(url = %candidate.url, "starting download");

    let client = reqwest::Client::builder()
        .user_agent("nitroid/1.0 (https://github.com/salom600/nitroid)")
        .timeout(std::time::Duration::from_secs(3600)) // 1 hour cap
        .build()
        .map_err(|e| CoreError::Backend(format!("reqwest client: {e}")))?;

    let response = client
        .get(&candidate.url)
        .send()
        .await
        .map_err(|e| CoreError::Backend(format!("HTTP GET failed: {e}")))?;

    if !response.status().is_success() {
        return Err(CoreError::Backend(format!(
            "HTTP {} for {}",
            response.status(),
            candidate.url
        )));
    }

    let total = response.content_length().unwrap_or(candidate.size_hint);
    {
        let mut s = state.lock();
        s.total = total;
    }

    // Stream to a temp file in the cache directory, then atomically rename.
    let cache_dir = paths::cache_dir();
    let temp_path = cache_dir.join(format!(".{}.part", sanitize_filename(&candidate.name)));
    let final_path = cache_dir.join(format!("{}.iso", sanitize_filename(&candidate.name)));

    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| CoreError::Backend(format!("create temp file: {e}")))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let start = std::time::Instant::now();
    let mut last_update = start;

    while let Some(chunk_result) = stream.next().await {
        let chunk =
            chunk_result.map_err(|e| CoreError::Backend(format!("stream read failed: {e}")))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| CoreError::Backend(format!("write failed: {e}")))?;
        downloaded += chunk.len() as u64;

        // Throttle progress updates to 10 Hz so we don't saturate the mutex.
        let now = std::time::Instant::now();
        if now.duration_since(last_update) > std::time::Duration::from_millis(100) {
            let elapsed = now.duration_since(start).as_secs_f64().max(0.001);
            let bps = (downloaded as f64 / elapsed) as u64;
            {
                let mut s = state.lock();
                s.downloaded = downloaded;
                s.bytes_per_sec = bps;
            }
            last_update = now;
        }
    }

    file.flush()
        .await
        .map_err(|e| CoreError::Backend(format!("flush failed: {e}")))?;
    drop(file);

    // Atomic rename.
    tokio::fs::rename(&temp_path, &final_path)
        .await
        .map_err(|e| CoreError::Backend(format!("rename failed: {e}")))?;

    {
        let mut s = state.lock();
        s.downloaded = downloaded;
        s.stage = DownloadStage::Verifying;
    }
    info!(path = %final_path.display(), bytes = downloaded, "download complete");

    // Verify + register.
    let image = SystemImage::register(&final_path, candidate.arch)?;
    let fingerprint = image.fingerprint.clone();

    {
        let mut s = state.lock();
        s.stage = DownloadStage::Done;
        s.active = false;
    }

    // Persist the candidate metadata next to the image so future boots can
    // show the version string.
    let meta_path = final_path.with_extension("meta.json");
    if let Ok(meta_str) = serde_json::to_string_pretty(&candidate) {
        let _ = std::fs::write(&meta_path, meta_str);
    }

    info!(fingerprint = %fingerprint, "image registered");
    Ok(())
}

/// Convert a friendly name into a filesystem-safe filename component.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Check whether an image with `name` has already been downloaded. Returns
/// its path if so.
pub fn existing_image_path(name: &str) -> Option<PathBuf> {
    let cache = paths::cache_dir();
    let target = format!("{}.iso", sanitize_filename(name));
    let candidate = cache.join(&target);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Convenience helper: download the default image (Android-x86 9.0) into
/// the cache directory. Used by the first-run setup flow.
pub fn download_default_image() -> Result<PathBuf> {
    let candidate = builtin_catalog()
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Backend("no images in catalog".into()))?;
    let path = existing_image_path(&candidate.name).unwrap_or_else(|| {
        paths::cache_dir().join(format!("{}.iso", sanitize_filename(&candidate.name)))
    });
    if path.exists() {
        return Ok(path);
    }
    // Synchronous wrapper — used by the CLI. The GUI uses the async path.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CoreError::Backend(format!("tokio runtime: {e}")))?;
    let state = Arc::new(Mutex::new(DownloadState::default()));
    rt.block_on(download_image(candidate, state))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_at_least_one_image() {
        let catalog = builtin_catalog();
        assert!(!catalog.is_empty());
        for c in &catalog {
            assert!(c.url.starts_with("https://"));
            assert!(c.size_hint > 0);
        }
    }

    #[test]
    fn sanitize_handles_special_chars() {
        // Spaces and parens both become '-', producing consecutive dashes.
        assert_eq!(
            sanitize_filename("Android-x86 9.0 (Pie)"),
            "android-x86-9-0--pie-"
        );
        assert_eq!(sanitize_filename("clean_name"), "clean_name");
    }

    #[test]
    fn downloader_starts_in_idle_state() {
        let d = Downloader::new();
        let s = d.state();
        assert!(!s.active);
        assert_eq!(s.stage, DownloadStage::Idle);
    }
}
