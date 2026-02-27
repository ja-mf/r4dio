//! Download manager for NTS shows
//!
//! Handles async downloading of NTS episodes with progress tracking.

use crate::nts_download::{self, EpisodeMetadata};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Download status for a show
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    /// Not downloaded
    NotDownloaded,
    /// Currently downloading with progress (0.0 - 1.0)
    Downloading(f32),
    /// Downloaded and available at path
    Downloaded(PathBuf),
    /// Download failed with error message
    Failed(String),
}

/// Download manager handles NTS show downloads
pub struct DownloadManager {
    /// Map of NTS URLs to download status
    pub statuses: HashMap<String, DownloadStatus>,
    /// Default download directory
    pub download_dir: PathBuf,
    /// Channel for download progress updates
    progress_tx: mpsc::Sender<DownloadProgress>,
    progress_rx: mpsc::Receiver<DownloadProgress>,
}

/// Progress update from download task
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub url: String,
    pub status: DownloadStatus,
}

impl DownloadManager {
    pub fn new(download_dir: PathBuf) -> Self {
        let (progress_tx, progress_rx) = mpsc::channel(100);
        Self {
            statuses: HashMap::new(),
            download_dir,
            progress_tx,
            progress_rx,
        }
    }

    /// Check if a show is downloaded (scan download directory)
    pub fn scan_downloaded_shows(&mut self) {
        if !self.download_dir.exists() {
            return;
        }

        let entries = match std::fs::read_dir(&self.download_dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read download directory: {}", e);
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                // Extract base name without extension
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Try to match against known downloads
                    // Format: "{safe_title} - YYYY-MM-DD"
                    for (url, status) in self.statuses.iter_mut() {
                        if let DownloadStatus::NotDownloaded = status {
                            // Check if this file matches the URL
                            // We'll update this when we have the metadata
                        }
                    }
                }
            }
        }
    }

    /// Start downloading an NTS show
    pub async fn start_download(&mut self, url: String) -> Result<(), String> {
        // Check if already downloading or downloaded
        match self.statuses.get(&url) {
            Some(DownloadStatus::Downloading(_)) => {
                return Err("Already downloading".to_string());
            }
            Some(DownloadStatus::Downloaded(_)) => {
                return Err("Already downloaded".to_string());
            }
            _ => {}
        }

        // Find yt-dlp
        let yt_dlp_path = nts_download::download::find_yt_dlp_platform()
            .or_else(nts_download::download::find_yt_dlp)
            .ok_or("yt-dlp not found")?;

        info!("Starting download of {} using yt-dlp at {:?}", url, yt_dlp_path);

        // Set initial status
        self.statuses
            .insert(url.clone(), DownloadStatus::Downloading(0.0));

        // Create download directory if needed
        if !self.download_dir.exists() {
            if let Err(e) = tokio::fs::create_dir_all(&self.download_dir).await {
                self.statuses
                    .insert(url.clone(), DownloadStatus::Failed(e.to_string()));
                return Err(format!("Failed to create download directory: {}", e));
            }
        }

        let download_dir = self.download_dir.clone();
        let progress_tx = self.progress_tx.clone();
        let url_clone = url.clone();

        // Spawn download task
        tokio::spawn(async move {
            let result =
                Self::do_download(&url_clone, &download_dir, &yt_dlp_path, progress_tx.clone()).await;

            let status = match result {
                Ok(path) => {
                    info!("Download complete: {:?}", path);
                    DownloadStatus::Downloaded(path)
                }
                Err(e) => {
                    error!("Download failed: {}", e);
                    DownloadStatus::Failed(e)
                }
            };

            let _ = progress_tx
                .send(DownloadProgress {
                    url: url_clone,
                    status,
                })
                .await;
        });

        Ok(())
    }

    /// Internal download implementation
    async fn do_download(
        url: &str,
        download_dir: &Path,
        yt_dlp_path: &Path,
        _progress_tx: mpsc::Sender<DownloadProgress>,
    ) -> Result<PathBuf, String> {
        // Use the nts_download module
        nts_download::download_episode(url, download_dir, yt_dlp_path)
            .await
            .map_err(|e| e.to_string())
    }

    /// Process pending progress updates
    pub fn update_statuses(&mut self) {
        while let Ok(progress) = self.progress_rx.try_recv() {
            self.statuses.insert(progress.url, progress.status);
        }
    }

    /// Get status for a URL
    pub fn get_status(&self, url: &str) -> DownloadStatus {
        self.statuses
            .get(url)
            .cloned()
            .unwrap_or(DownloadStatus::NotDownloaded)
    }

    /// Check if file exists for a given metadata
    pub fn check_downloaded(&self, metadata: &EpisodeMetadata) -> Option<PathBuf> {
        let base_name = metadata.file_base_name();
        
        // Check for common audio extensions
        for ext in &["opus", "m4a", "mp3", "ogg", "flac"] {
            let path = self.download_dir.join(format!("{}.{}", base_name, ext));
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Mark a URL as downloaded (for loading from persistence)
    pub fn mark_downloaded(&mut self, url: String, path: PathBuf) {
        self.statuses.insert(url, DownloadStatus::Downloaded(path));
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        let download_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("radio-downloads");
        Self::new(download_dir)
    }
}
