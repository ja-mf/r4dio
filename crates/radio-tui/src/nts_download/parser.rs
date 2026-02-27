//! HTML parsing and metadata extraction

use anyhow::{Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use scraper::{Html, Selector};

use crate::nts_download::api::EpisodeApiData;
use crate::nts_download::{EpisodeMetadata, Track};

/// Parse NTS episode URL to extract show name and episode alias
pub fn parse_episode_url(url: &str) -> Result<(String, String)> {
    // Pattern: https://www.nts.live/shows/{show}/episodes/{episode}
    let re = Regex::new(r"nts\.live/shows/([^/]+)/episodes/([^/?]+)")?;

    if let Some(caps) = re.captures(url) {
        let show_name = caps.get(1).unwrap().as_str().to_string();
        let episode_alias = caps.get(2).unwrap().as_str().to_string();
        return Ok((show_name, episode_alias));
    }

    anyhow::bail!("Invalid NTS episode URL: {}", url)
}

/// Parse show URL to extract show name
pub fn parse_show_url(url: &str) -> Result<String> {
    // Pattern: https://www.nts.live/shows/{show}
    let re = Regex::new(r"nts\.live/shows/([^/]+)/?$")?;

    if let Some(caps) = re.captures(url) {
        return Ok(caps.get(1).unwrap().as_str().to_string());
    }

    anyhow::bail!("Invalid NTS show URL: {}", url)
}

/// Check if URL is an episode URL
pub fn is_episode_url(url: &str) -> bool {
    Regex::new(r"nts\.live/shows/[^/]+/episodes/")
        .map(|re| re.is_match(url))
        .unwrap_or(false)
}

/// Check if URL is a show URL
pub fn is_show_url(url: &str) -> bool {
    Regex::new(r"nts\.live/shows/[^/]+/?$")
        .map(|re| re.is_match(url) && !is_episode_url(url))
        .unwrap_or(false)
}

/// Make title safe for filesystem
pub fn safe_filename(title: &str) -> String {
    title.replace(['/', ':'], "-")
}

/// Get ordinal suffix for day number
pub fn get_ordinal_suffix(day: u32) -> &'static str {
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

/// Parse NTS data from HTML and API response
pub fn parse_nts_data(
    html: &str,
    api_data: &EpisodeApiData,
    source_url: &str,
) -> Result<EpisodeMetadata> {
    // Parse title
    let title = api_data.name.clone();
    let safe_title = safe_filename(&title);

    // Parse station/location
    let station = api_data
        .location_long
        .clone()
        .unwrap_or_else(|| "London".to_string());

    // Parse image URL
    let image_url = api_data
        .media
        .as_ref()
        .and_then(|m| m.picture_large.clone())
        .unwrap_or_default();

    // Parse date
    let date =
        parse_broadcast_date(&api_data.broadcast).context("Failed to parse broadcast date")?;

    // Parse genres
    let genres: Vec<String> = api_data
        .genres
        .as_ref()
        .map(|g| {
            g.iter()
                .filter_map(|genre| {
                    let v = genre.value.trim();
                    if v.is_empty() {
                        None
                    } else {
                        Some(v.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Parse tracklist from API
    let tracks = parse_tracklist(api_data);

    // Parse description
    let description = api_data.description.clone().unwrap_or_default();

    // Parse artists from HTML and title
    let (artists, parsed_artists) = parse_artists(&title, html)?;

    Ok(EpisodeMetadata {
        title,
        safe_title,
        date,
        artists,
        parsed_artists,
        station,
        genres,
        tracks,
        image_url,
        description,
        source_url: source_url.to_string(),
    })
}

/// Parse broadcast date string (ISO 8601)
fn parse_broadcast_date(date_str: &str) -> Result<NaiveDate> {
    // Try parsing full ISO 8601 datetime
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        return Ok(dt.naive_local().date());
    }

    // Try parsing just date portion
    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        return Ok(date);
    }

    anyhow::bail!("Cannot parse date: {}", date_str)
}

/// Parse tracklist from API data
fn parse_tracklist(api_data: &EpisodeApiData) -> Vec<Track> {
    api_data
        .embeds
        .as_ref()
        .and_then(|e| e.tracklist.as_ref())
        .map(|t| {
            t.results
                .iter()
                .map(|r| Track {
                    name: r.title.clone(),
                    artist: r.artist.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse artists from title and HTML
///
/// Extracts artists from:
/// 1. HTML bio-artists section ( BeautifulSoup equivalent: `.bio-artists a` )
/// 2. Title patterns like "w/ Artist", "with Artist", "Artist1 & Artist2"
pub fn parse_artists(title: &str, html: &str) -> Result<(Vec<String>, Vec<String>)> {
    let document = Html::parse_document(html);

    // Parse artists from HTML bio-artists section
    let mut artists = Vec::new();
    let selector = Selector::parse(".bio-artists a").ok();

    if let Some(sel) = selector {
        for element in document.select(&sel) {
            let text = element.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                artists.push(text);
            }
        }
    }

    // Parse artists from title patterns
    let parsed_artists = parse_artists_from_title(title);

    Ok((artists, parsed_artists))
}

/// Parse artists from title text
///
/// Handles patterns:
/// - "Show Title w/ Artist Name"
/// - "Show Title with Artist Name"
/// - "Show Title w/ Artist1 and Artist2"
/// - "Show Title w/ Artist1, Artist2 & Artist3"
fn parse_artists_from_title(title: &str) -> Vec<String> {
    let mut result = Vec::new();

    // Find the "w/" or "with" prefix and extract everything after it
    let after_prefix = if let Some(pos) = title.to_lowercase().find("w/") {
        &title[pos + 2..]
    } else if let Some(pos) = title.to_lowercase().find("with") {
        &title[pos + 4..]
    } else {
        return result;
    };

    // Trim leading whitespace
    let after_prefix = after_prefix.trim_start();

    // Find where the artist section ends (before " - ", " ~ ", or end of string)
    let artist_section = if let Some(end_pos) = after_prefix.find(" - ") {
        &after_prefix[..end_pos]
    } else if let Some(end_pos) = after_prefix.find(" ~ ") {
        &after_prefix[..end_pos]
    } else {
        after_prefix
    };

    // Split by separators: ",", "&", " and "
    let parts: Vec<&str> = artist_section
        .split(&[',', '&'])
        .flat_map(|p| p.split(" and "))
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    for part in parts {
        result.push(part.to_string());
    }

    result
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn test_parse_episode_url() {
        let url = "https://www.nts.live/shows/my-show/episodes/my-episode";
        let (show, ep) = parse_episode_url(url).unwrap();
        assert_eq!(show, "my-show");
        assert_eq!(ep, "my-episode");
    }

    #[test]
    fn test_safe_filename() {
        assert_eq!(
            safe_filename("Show: Name / Edition"),
            "Show- Name - Edition"
        );
        assert_eq!(safe_filename("Normal Name"), "Normal Name");
    }

    #[test]
    fn test_get_ordinal_suffix() {
        assert_eq!(get_ordinal_suffix(1), "st");
        assert_eq!(get_ordinal_suffix(2), "nd");
        assert_eq!(get_ordinal_suffix(3), "rd");
        assert_eq!(get_ordinal_suffix(4), "th");
        assert_eq!(get_ordinal_suffix(11), "th");
        assert_eq!(get_ordinal_suffix(12), "th");
        assert_eq!(get_ordinal_suffix(13), "th");
        assert_eq!(get_ordinal_suffix(21), "st");
        assert_eq!(get_ordinal_suffix(22), "nd");
    }

    #[test]
    fn test_parse_artists_from_title_simple() {
        let artists = parse_artists_from_title("Show Title w/ Artist Name");
        assert_eq!(artists, vec!["Artist Name"]);
    }

    #[test]
    fn test_parse_artists_from_title_with_multiple() {
        let artists = parse_artists_from_title("Show Title w/ Artist1 and Artist2");
        assert!(artists.contains(&"Artist1".to_string()));
        assert!(artists.contains(&"Artist2".to_string()));
    }

    #[test]
    fn test_parse_artists_from_title_with_comma() {
        let artists = parse_artists_from_title("Show Title w/ Artist1, Artist2 & Artist3");
        assert!(artists.contains(&"Artist1".to_string()));
        assert!(artists.contains(&"Artist2".to_string()));
        assert!(artists.contains(&"Artist3".to_string()));
    }
}
