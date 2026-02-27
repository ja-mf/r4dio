//! yt-dlp wrapper for downloading audio

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Download progress information
#[derive(Debug, Clone)]
pub enum DownloadProgress {
    Starting,
    Downloading {
        percent: f32,
        speed: String,
        eta: String,
    },
    Converting,
    Complete(PathBuf),
    Error(String),
}

/// Download audio from a URL using yt-dlp
///
/// # Arguments
/// * `url` - Audio source URL (Mixcloud, Soundcloud, etc.)
/// * `base_name` - Base filename without extension
/// * `output_dir` - Directory to save the file
/// * `yt_dlp_path` - Path to yt-dlp binary
pub async fn download_audio(
    url: &str,
    base_name: &str,
    output_dir: &Path,
    yt_dlp_path: &Path,
) -> Result<PathBuf> {
    let output_template = format!("{}/{}.%(ext)s", output_dir.display(), base_name);

    info!("Starting download from {} to {}", url, output_template);

    let mut cmd = Command::new(yt_dlp_path);
    cmd.arg("--no-progress")
        .arg("--newline")
        .arg("-o")
        .arg(&output_template)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn yt-dlp")?;

    // Read output for logging
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("yt-dlp: {}", line);
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();

        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                warn!("yt-dlp stderr: {}", line);
            }
        });
    }

    let status = child.wait().await.context("Failed to wait for yt-dlp")?;

    if !status.success() {
        anyhow::bail!("yt-dlp exited with status: {:?}", status.code());
    }

    // Find the downloaded file
    let downloaded_file = find_downloaded_file(output_dir, base_name)
        .await
        .context("Could not find downloaded file")?;

    info!("Download complete: {}", downloaded_file.display());

    Ok(downloaded_file)
}

/// Parse yt-dlp progress output line
fn parse_progress_line(line: &str) -> Option<DownloadProgress> {
    // Example: [download]  45.3% of ~50.12MiB at  2.56MiB/s ETA 00:12
    if line.contains("[download]") && line.contains('%') {
        let parts: Vec<&str> = line.split_whitespace().collect();

        // Find percentage
        for (i, part) in parts.iter().enumerate() {
            if part.ends_with('%') {
                if let Ok(percent) = part.trim_end_matches('%').parse::<f32>() {
                    let speed = parts.get(i + 2).map(|s| s.to_string()).unwrap_or_default();
                    let eta = parts.get(i + 4).map(|s| s.to_string()).unwrap_or_default();

                    return Some(DownloadProgress::Downloading {
                        percent: percent / 100.0,
                        speed,
                        eta,
                    });
                }
            }
        }
    }

    if line.contains("[ExtractAudio]") || line.contains("[FFmpeg]") {
        return Some(DownloadProgress::Converting);
    }

    None
}

/// Find the downloaded file by base name
async fn find_downloaded_file(output_dir: &Path, base_name: &str) -> Result<PathBuf> {
    let mut entries = tokio::fs::read_dir(output_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
            if name == base_name {
                return Ok(path);
            }
        }
    }

    anyhow::bail!("Downloaded file not found for: {}", base_name)
}

/// Convert webm/opus to ogg using ffmpeg
///
/// This is needed because some Mixcloud downloads come as webm/opus
/// which has limited player support.
pub async fn convert_to_ogg(input_path: &Path, output_path: &Path) -> Result<()> {
    let ffmpeg_bin =
        radio_proto::platform::find_ffmpeg_binary().unwrap_or_else(|| PathBuf::from("ffmpeg"));

    info!(
        "Converting {} to {}",
        input_path.display(),
        output_path.display()
    );

    let status = Command::new(ffmpeg_bin)
        .arg("-i")
        .arg(input_path)
        .arg("-acodec")
        .arg("copy")
        .arg("-y")
        .arg(output_path)
        .status()
        .await
        .context("Failed to spawn ffmpeg")?;

    if !status.success() {
        anyhow::bail!("ffmpeg conversion failed");
    }

    // Remove original file
    tokio::fs::remove_file(input_path)
        .await
        .context("Failed to remove original file after conversion")?;

    info!("Conversion complete: {}", output_path.display());

    Ok(())
}

/// Find yt-dlp binary
///
/// Searches in order:
/// 1. YT_DLP_PATH environment variable
/// 2. Beside current executable
/// 3. PATH
pub fn find_yt_dlp() -> Option<PathBuf> {
    // 1. Environment variable
    if let Ok(path) = std::env::var("YT_DLP_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Beside executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let names = yt_dlp_binary_names();
            for name in &names {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    // 3. PATH
    if let Ok(path) = std::env::var("PATH") {
        #[cfg(unix)]
        let separator = ':';
        #[cfg(windows)]
        let separator = ';';

        for dir in path.split(separator) {
            for name in yt_dlp_binary_names() {
                let candidate = PathBuf::from(dir).join(&name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn yt_dlp_binary_names() -> Vec<String> {
    #[cfg(windows)]
    return vec!["yt-dlp.exe".to_string(), "yt-dlp".to_string()];

    #[cfg(not(windows))]
    return vec![
        "yt-dlp".to_string(),
        "yt-dlp_macos".to_string(),
        "yt-dlp_linux".to_string(),
    ];
}

/// Re-export platform-based yt-dlp finder (preferred)
pub fn find_yt_dlp_platform() -> Option<PathBuf> {
    radio_proto::platform::find_yt_dlp_binary()
}
