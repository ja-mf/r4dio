//! Audio metadata tagging using lofty

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};
use std::path::Path;

use crate::nts_download::EpisodeMetadata;

/// Write metadata to audio file
/// 
/// Maps NTS episode metadata to standard audio tags:
/// - Title: "Show Name - DDth Month YYYY"
/// - Album: "NTS"
/// - Artist: Combined artists from bio and title
/// - Year: Broadcast date (ISO format)
/// - Genre: Semicolon-separated genres
/// - Lyrics: Tracklist
/// - Comment: Description + station + URL
/// - Album Art: Cover image
pub async fn write_metadata(
    file_path: &Path,
    metadata: &EpisodeMetadata,
    image_data: Option<(Vec<u8>, String)>,
) -> Result<()> {
    // Use blocking task for file I/O
    let path = file_path.to_path_buf();
    let meta = metadata.clone();
    let img = image_data;
    
    tokio::task::spawn_blocking(move || {
        write_metadata_blocking(&path, &meta, img)
    }).await
    .context("Metadata writing task failed")??;
    
    Ok(())
}

fn write_metadata_blocking(
    file_path: &Path,
    metadata: &EpisodeMetadata,
    image_data: Option<(Vec<u8>, String)>,
) -> Result<()> {
    // Open file and read existing tags
    let tagged_file = Probe::open(file_path)?
        .read()
        .context("Failed to read audio file")?;
    
    // Get primary tag type for this format
    let tag_type = guess_tag_type(file_path)?;
    
    // Create or modify tag
    let mut tag = tagged_file.primary_tag()
        .map(|t| t.clone())
        .unwrap_or_else(|| Tag::new(tag_type));
    
    // Set title
    tag.insert_text(ItemKey::TrackTitle, metadata.display_title());
    
    // Set album
    tag.insert_text(ItemKey::AlbumTitle, "NTS".to_string());
    
    // Set artist (combined)
    let artists = metadata.all_artists();
    if !artists.is_empty() {
        tag.insert_text(ItemKey::TrackArtist, artists.join("; "));
    }
    
    // Set year/date
    let date_str = metadata.date.to_string();
    tag.insert_text(ItemKey::Year, date_str.clone());
    
    // Set genre
    if !metadata.genres.is_empty() {
        tag.insert_text(ItemKey::Genre, metadata.genres.join("; "));
    }
    
    // Set tracklist as lyrics
    let tracklist = metadata.format_tracklist();
    if !tracklist.is_empty() {
        tag.insert_text(ItemKey::Lyrics, tracklist);
    }
    
    // Set comment
    let comment = format_comment(metadata);
    tag.insert_text(ItemKey::Comment, comment);
    
    // Set compilation flag
    tag.insert_text(ItemKey::FlagCompilation, "1".to_string());
    
    // Add cover art
    if let Some((data, content_type)) = image_data {
        let mime_type = guess_mime_type(&content_type);
        let picture = Picture::new_unchecked(
            PictureType::CoverFront,
            Some(mime_type),
            None,
            data,
        );
        tag.push_picture(picture);
    }
    
    // Save tag back to file
    tag.save_to_path(file_path, WriteOptions::default())
        .context("Failed to save metadata to file")?;
    
    Ok(())
}

/// Guess the appropriate tag type for file extension
fn guess_tag_type(file_path: &Path) -> Result<TagType> {
    let ext = file_path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    
    match ext.as_str() {
        "mp3" => Ok(TagType::Id3v2),
        "m4a" | "mp4" | "aac" => Ok(TagType::Mp4Ilst),
        "flac" => Ok(TagType::VorbisComments),
        "ogg" | "opus" => Ok(TagType::VorbisComments),
        "wav" => Ok(TagType::RiffInfo),
        "wma" => Ok(TagType::Ape),
        _ => anyhow::bail!("Unsupported audio format: {}", ext),
    }
}

/// Guess MIME type from content-type string
fn guess_mime_type(content_type: &str) -> MimeType {
    if content_type.contains("jpeg") || content_type.contains("jpg") {
        MimeType::Jpeg
    } else if content_type.contains("png") {
        MimeType::Png
    } else if content_type.contains("gif") {
        MimeType::Gif
    } else if content_type.contains("bmp") {
        MimeType::Bmp
    } else if content_type.contains("tiff") {
        MimeType::Tiff
    } else {
        MimeType::Jpeg // Default
    }
}

/// Format comment field
fn format_comment(metadata: &EpisodeMetadata) -> String {
    let mut parts = Vec::new();
    
    if !metadata.description.is_empty() {
        parts.push(metadata.description.clone());
    }
    
    parts.push(format!("Station Location: {}", metadata.station));
    parts.push(metadata.source_url.clone());
    
    parts.join("\n")
}

/// Read metadata from audio file (for verification)
pub fn read_metadata(file_path: &Path) -> Result<ReadMetadata> {
    let tagged_file = Probe::open(file_path)?
        .read()
        .context("Failed to read audio file")?;
    
    let tag = tagged_file.primary_tag()
        .context("No metadata tag found")?;
    
    let get_text = |key: &ItemKey| -> Option<String> {
        tag.get_string(key).map(|s| s.to_string())
    };
    
    Ok(ReadMetadata {
        title: get_text(&ItemKey::TrackTitle),
        artist: get_text(&ItemKey::TrackArtist),
        album: get_text(&ItemKey::AlbumTitle),
        year: get_text(&ItemKey::Year),
        genre: get_text(&ItemKey::Genre),
        comment: get_text(&ItemKey::Comment),
        lyrics: get_text(&ItemKey::Lyrics),
        has_picture: !tag.pictures().is_empty(),
    })
}

/// Metadata read from file (for verification)
#[derive(Debug, Clone)]
pub struct ReadMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub lyrics: Option<String>,
    pub has_picture: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_guess_mime_type() {
        assert_eq!(guess_mime_type("image/jpeg"), MimeType::Jpeg);
        assert_eq!(guess_mime_type("image/png"), MimeType::Png);
        assert_eq!(guess_mime_type("image/gif"), MimeType::Gif);
        assert_eq!(guess_mime_type("application/octet-stream"), MimeType::Jpeg);
    }

    #[test]
    fn test_format_comment() {
        let meta = EpisodeMetadata {
            title: "Test Show".to_string(),
            safe_title: "Test Show".to_string(),
            date: chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            artists: vec!["Artist 1".to_string()],
            parsed_artists: vec![],
            station: "London".to_string(),
            genres: vec!["Electronic".to_string()],
            tracks: vec![],
            image_url: "".to_string(),
            description: "Test description".to_string(),
            source_url: "https://nts.live/shows/test".to_string(),
        };
        
        let comment = format_comment(&meta);
        assert!(comment.contains("Test description"));
        assert!(comment.contains("Station Location: London"));
        assert!(comment.contains("https://nts.live/shows/test"));
    }
}
