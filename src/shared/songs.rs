//! Song database and recognition for Windows
//! 
//! This module provides:
//! - SQLite-based song database (songs.vds)
//! - Song recognition using AcoustID/fingerprinting
//! - ICY metadata tracking
//! - Windows-compatible implementation

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};

/// Song entry in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongEntry {
    pub id: Option<i64>,
    pub timestamp: DateTime<Local>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub radio_station: String,
    pub show_name: Option<String>,
    pub url: Option<String>,
    pub icy_raw: Option<String>,
    pub fingerprint: Option<String>,
    pub recognized: bool,
    pub extra: Option<String>,
}

impl SongEntry {
    pub fn new(
        title: String,
        artist: String,
        radio_station: String,
    ) -> Self {
        Self {
            id: None,
            timestamp: Local::now(),
            title,
            artist,
            album: None,
            genre: None,
            year: None,
            radio_station,
            show_name: None,
            url: None,
            icy_raw: None,
            fingerprint: None,
            recognized: false,
            extra: None,
        }
    }

    /// Parse ICY metadata to extract song info
    pub fn from_icy(icy: &str, station: &str) -> Option<Self> {
        // Common formats:
        // "Artist - Title"
        // "Artist - Title - Album"
        // "Title by Artist"
        
        let icy = icy.trim();
        if icy.is_empty() {
            return None;
        }

        // Try "Artist - Title" format
        if let Some(sep_pos) = icy.find(" - ") {
            let artist = icy[..sep_pos].trim().to_string();
            let rest = &icy[sep_pos + 3..];
            
            // Check if there's an album
            if let Some(album_sep) = rest.find(" - ") {
                let title = rest[..album_sep].trim().to_string();
                let album = Some(rest[album_sep + 3..].trim().to_string());
                
                let mut entry = Self::new(title, artist, station.to_string());
                entry.album = album;
                entry.icy_raw = Some(icy.to_string());
                return Some(entry);
            } else {
                let title = rest.trim().to_string();
                let mut entry = Self::new(title, artist, station.to_string());
                entry.icy_raw = Some(icy.to_string());
                return Some(entry);
            }
        }

        // Try "Title by Artist" format
        if let Some(sep_pos) = icy.to_lowercase().find(" by ") {
            let title = icy[..sep_pos].trim().to_string();
            let artist = icy[sep_pos + 4..].trim().to_string();
            let mut entry = Self::new(title, artist, station.to_string());
            entry.icy_raw = Some(icy.to_string());
            return Some(entry);
        }

        // If we can't parse it, store the whole thing as title
        let mut entry = Self::new(icy.to_string(), "Unknown".to_string(), station.to_string());
        entry.icy_raw = Some(icy.to_string());
        Some(entry)
    }

    /// Format for display
    pub fn display_title(&self) -> String {
        if self.artist.is_empty() || self.artist == "Unknown" {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// Song database manager
pub struct SongDatabase {
    db_path: PathBuf,
}

impl SongDatabase {
    pub fn new(path: PathBuf) -> Self {
        Self { db_path: path }
    }

    /// Get default database path
    pub fn default_path() -> PathBuf {
        #[cfg(windows)]
        {
            // On Windows, use portable path if config folder exists
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    let portable_db = exe_dir.join("songs.vds");
                    if portable_db.exists() || exe_dir.join("config").exists() {
                        return portable_db;
                    }
                }
            }
        }
        
        // Default: use data directory
        crate::shared::platform::data_dir().join("songs.vds")
    }

    /// Initialize the database
    pub async fn init(&self) -> anyhow::Result<()> {
        // For now, use CSV format for simplicity
        // In future, this could be SQLite
        
        if !self.db_path.exists() {
            info!("Creating new song database at: {:?}", self.db_path);
            
            // Create parent directory if needed
            if let Some(parent) = self.db_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            
            // Create empty database with header
            let header = "timestamp|title|artist|album|genre|year|radio_station|show_name|url|icy_raw|fingerprint|recognized|extra\n";
            tokio::fs::write(&self.db_path, header).await?;
        }
        
        Ok(())
    }

    /// Add a song to the database
    pub async fn add_song(&self, song: &SongEntry) -> anyhow::Result<()> {
        let line = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            song.timestamp.format("%Y-%m-%d %H:%M:%S"),
            escape_field(&song.title),
            escape_field(&song.artist),
            escape_field(song.album.as_deref().unwrap_or("")),
            escape_field(song.genre.as_deref().unwrap_or("")),
            song.year.map(|y| y.to_string()).unwrap_or_default(),
            escape_field(&song.radio_station),
            escape_field(song.show_name.as_deref().unwrap_or("")),
            escape_field(song.url.as_deref().unwrap_or("")),
            escape_field(song.icy_raw.as_deref().unwrap_or("")),
            escape_field(song.fingerprint.as_deref().unwrap_or("")),
            song.recognized,
            escape_field(song.extra.as_deref().unwrap_or(""))
        );
        
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.db_path)
            .await?
            .write_all(line.as_bytes())
            .await?;
        
        info!("Added song to database: {}", song.display_title());
        Ok(())
    }

    /// Get recent songs
    pub async fn get_recent(&self,
        limit: usize,
    ) -> anyhow::Result<Vec<SongEntry>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&self.db_path).await?;
        let lines: Vec<&str> = content.lines().collect();
        
        // Skip header, take last 'limit' entries
        let start = if lines.len() > limit + 1 {
            lines.len() - limit
        } else {
            1 // Skip header
        };
        
        let mut songs = Vec::new();
        for line in &lines[start..] {
            if let Some(song) = parse_line(line) {
                songs.push(song);
            }
        }
        
        Ok(songs)
    }

    /// Search songs
    pub async fn search(&self, query: &str) -> anyhow::Result<Vec<SongEntry>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&self.db_path).await?;
        let query_lower = query.to_lowercase();
        
        let mut songs = Vec::new();
        for line in content.lines().skip(1) { // Skip header
            if let Some(song) = parse_line(line) {
                let search_text = format!(
                    "{} {} {}",
                    song.title.to_lowercase(),
                    song.artist.to_lowercase(),
                    song.album.as_deref().unwrap_or("").to_lowercase()
                );
                if search_text.contains(&query_lower) {
                    songs.push(song);
                }
            }
        }
        
        Ok(songs)
    }
}

/// Escape field for CSV-like format
fn escape_field(s: &str) -> String {
    s.replace('|', "\\|")
     .replace('\n', " ")
     .replace('\r', "")
}

/// Parse a line from the database
fn parse_line(line: &str) -> Option<SongEntry> {
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() < 5 {
        return None;
    }

    let timestamp = chrono::NaiveDateTime::parse_from_str(parts[0], "%Y-%m-%d %H:%M:%S")
        .ok()
        .and_then(|dt| dt.and_local_timezone(Local).earliest())
        .unwrap_or_else(Local::now);

    Some(SongEntry {
        id: None,
        timestamp,
        title: unescape_field(parts[1]),
        artist: unescape_field(parts[2]),
        album: parts.get(3).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        genre: parts.get(4).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        year: parts.get(5).and_then(|s| s.parse().ok()),
        radio_station: unescape_field(parts[6]),
        show_name: parts.get(7).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        url: parts.get(8).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        icy_raw: parts.get(9).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        fingerprint: parts.get(10).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
        recognized: parts.get(11).map(|s| *s == "true").unwrap_or(false),
        extra: parts.get(12).map(|s| unescape_field(s)).filter(|s| !s.is_empty()),
    })
}

fn unescape_field(s: &str) -> String {
    s.replace("\\|", "|")
}

/// Song recognizer interface
pub struct SongRecognizer;

impl SongRecognizer {
    /// Recognize a song using available methods
    /// 
    /// For now, this uses ICY metadata. In the future, this could:
    /// - Record audio sample using ffmpeg
    /// - Generate fingerprint using chromaprint
    /// - Query AcoustID API
    pub async fn recognize(
        _audio_sample: Option<&[ u8]>,
        icy_metadata: Option<&str>,
        station: &str,
    ) -> Option<SongEntry> {
        // First, try to parse ICY metadata
        if let Some(icy) = icy_metadata {
            if let Some(entry) = SongEntry::from_icy(icy, station) {
                return Some(entry);
            }
        }

        // TODO: Implement audio fingerprinting
        // 1. Record 10-20 second sample using ffmpeg
        // 2. Generate fingerprint using chromaprint (fpcalc)
        // 3. Query AcoustID API
        // 4. Return enriched metadata

        None
    }

    /// Check if audio recognition is available on this platform
    pub fn is_available() -> bool {
        // For now, only ICY metadata is available
        // Full audio recognition requires ffmpeg + chromaprint
        false
    }
}

use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_icy_artist_title() {
        let entry = SongEntry::from_icy("The Beatles - Hey Jude", "Test Radio").unwrap();
        assert_eq!(entry.artist, "The Beatles");
        assert_eq!(entry.title, "Hey Jude");
    }

    #[test]
    fn test_parse_icy_with_album() {
        let entry = SongEntry::from_icy("Pink Floyd - Shine On You Crazy Diamond - Wish You Were Here", "Test Radio").unwrap();
        assert_eq!(entry.artist, "Pink Floyd");
        assert_eq!(entry.title, "Shine On You Crazy Diamond");
        assert_eq!(entry.album, Some("Wish You Were Here".to_string()));
    }
}
