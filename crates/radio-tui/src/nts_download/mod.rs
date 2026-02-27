//! NTS Radio show downloader - Rust port of nts_get
//! 
//! Downloads NTS episodes with metadata extraction and audio tagging.

use std::path::Path;

pub mod api;
pub mod parser;
pub mod download;
pub mod metadata;

#[cfg(test)]
mod tests;

use anyhow::Result;
use chrono::{Datelike, NaiveDate};

/// Parsed NTS episode metadata
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeMetadata {
    pub title: String,
    pub safe_title: String,
    pub date: NaiveDate,
    pub artists: Vec<String>,
    pub parsed_artists: Vec<String>,
    pub station: String,
    pub genres: Vec<String>,
    pub tracks: Vec<Track>,
    pub image_url: String,
    pub description: String,
    pub source_url: String,
}

/// Track from episode tracklist
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub name: String,
    pub artist: String,
}

impl EpisodeMetadata {
    /// Get formatted title: "Show Name - DDth Month YYYY"
    pub fn display_title(&self) -> String {
        let day = self.date.day();
        let suffix = get_ordinal_suffix(day);
        format!(
            "{} - {}{} {}",
            self.title,
            day,
            suffix,
            self.date.format("%B %Y")
        )
    }
    
    /// Get filename-safe base name
    pub fn file_base_name(&self) -> String {
        format!(
            "{} - {}-{:02}-{:02}",
            self.safe_title,
            self.date.year(),
            self.date.month(),
            self.date.day()
        )
    }
    
    /// Get all unique artists combined
    pub fn all_artists(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        
        for artist in self.artists.iter().chain(&self.parsed_artists) {
            let lower = artist.to_lowercase();
            if seen.insert(lower) {
                result.push(artist.clone());
            }
        }
        
        result
    }
    
    /// Format tracklist for lyrics tag
    pub fn format_tracklist(&self) -> String {
        if self.tracks.is_empty() {
            return String::new();
        }
        
        let lines: Vec<String> = self.tracks
            .iter()
            .map(|t| format!("{} by {}", t.name, t.artist))
            .collect();
        
        format!("Tracklist:\n{}", lines.join("\n"))
    }
}

fn get_ordinal_suffix(day: u32) -> &'static str {
    if (11..=13).contains(&(day % 100)) {
        "th"
    } else {
        match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    }
}

/// Download an NTS episode
/// 
/// # Arguments
/// * `url` - NTS episode URL (e.g., https://www.nts.live/shows/xyz/episodes/abc)
/// * `output_dir` - Directory to save the downloaded file
/// * `yt_dlp_path` - Path to yt-dlp binary
pub async fn download_episode(
    url: &str,
    output_dir: &Path,
    yt_dlp_path: &Path,
) -> Result<PathBuf> {
    // 1. Parse episode URL
    let (show_name, episode_alias) = parser::parse_episode_url(url)?;
    
    // 2. Fetch API data and HTML
    let api_data = api::fetch_episode(&show_name, &episode_alias).await?;
    let html = api::fetch_episode_html(url).await?;
    
    // 3. Parse metadata
    let metadata = parser::parse_nts_data(&html, &api_data, url)?;
    
    // 4. Determine download source (Mixcloud > Soundcloud)
    let audio_source = api::resolve_audio_source(&api_data, &metadata).await?;
    
    // 5. Download audio
    let download_path = download::download_audio(
        &audio_source,
        &metadata.file_base_name(),
        output_dir,
        yt_dlp_path,
    ).await?;
    
    // 6. Download cover image
    let image_data = if !metadata.image_url.is_empty() {
        Some(api::fetch_image(&metadata.image_url).await?)
    } else {
        None
    };
    
    // 7. Tag the file
    metadata::write_metadata(&download_path, &metadata, image_data).await?;
    
    Ok(download_path)
}

use std::path::PathBuf;
