//! NTS API client

use anyhow::{Context, Result};
use chrono::Datelike;
use serde::Deserialize;
use std::collections::HashMap;

/// NTS API episode response
#[derive(Debug, Deserialize)]
pub struct EpisodeApiData {
    pub name: String,
    #[serde(rename = "location_long")]
    pub location_long: Option<String>,
    pub broadcast: String,
    pub description: Option<String>,
    pub mixcloud: Option<String>,
    #[serde(rename = "audio_sources")]
    pub audio_sources: Option<Vec<AudioSource>>,
    pub media: Option<Media>,
    pub genres: Option<Vec<Genre>>,
    pub embeds: Option<Embeds>,
}

#[derive(Debug, Deserialize)]
pub struct AudioSource {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Media {
    #[serde(rename = "picture_large")]
    pub picture_large: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Genre {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct Embeds {
    pub tracklist: Option<TracklistEmbed>,
}

#[derive(Debug, Deserialize)]
pub struct TracklistEmbed {
    pub results: Vec<TrackResult>,
}

#[derive(Debug, Deserialize)]
pub struct TrackResult {
    pub title: String,
    pub artist: String,
}

/// Fetch episode data from NTS API
pub async fn fetch_episode(show_name: &str, episode_alias: &str) -> Result<EpisodeApiData> {
    let url = format!(
        "https://www.nts.live/api/v2/shows/{}/episodes/{}",
        show_name, episode_alias
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .context("Failed to fetch NTS API")?;

    if !response.status().is_success() {
        anyhow::bail!("NTS API returned status: {}", response.status());
    }

    let data: EpisodeApiData = response
        .json()
        .await
        .context("Failed to parse NTS API response")?;

    Ok(data)
}

/// Fetch episode HTML page for metadata extraction
pub async fn fetch_episode_html(url: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("Accept", "text/html")
        .send()
        .await
        .context("Failed to fetch NTS episode page")?;

    if !response.status().is_success() {
        anyhow::bail!("NTS page returned status: {}", response.status());
    }

    let html = response
        .text()
        .await
        .context("Failed to read NTS page HTML")?;

    Ok(html)
}

/// Fetch cover image
pub async fn fetch_image(url: &str) -> Result<(Vec<u8>, String)> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to fetch image")?;

    if !response.status().is_success() {
        anyhow::bail!("Image fetch returned status: {}", response.status());
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let data = response
        .bytes()
        .await
        .context("Failed to read image data")?
        .to_vec();

    Ok((data, content_type))
}

/// Mixcloud API search response
#[derive(Debug, Deserialize)]
struct MixcloudSearchResponse {
    data: Vec<MixcloudResult>,
}

#[derive(Debug, Deserialize)]
struct MixcloudResult {
    name: String,
    url: String,
    user: MixcloudUser,
}

#[derive(Debug, Deserialize)]
struct MixcloudUser {
    username: String,
}

/// Try to find Mixcloud URL via search API
pub async fn mixcloud_search(title: &str, date: &chrono::NaiveDate) -> Result<Option<String>> {
    use crate::nts_download::parser::get_ordinal_suffix;

    let day = date.day();
    let suffix = get_ordinal_suffix(day);
    let full_title = format!("{} - {}{} {}", title, day, suffix, date.format("%B %Y"));

    // Create search query
    let query = full_title
        .replace(['-', '/'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("+");

    let url = format!(
        "https://api.mixcloud.com/search/?q={}&type=cloudcast",
        query
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to search Mixcloud")?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let search: MixcloudSearchResponse = match response.json().await {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    // Find matching result from NTSRadio user
    for result in search.data {
        if result.user.username.to_lowercase() == "ntsradio" && result.name == full_title {
            return Ok(Some(result.url));
        }
    }

    Ok(None)
}

/// Resolve audio source: Mixcloud > Soundcloud > fallback
pub async fn resolve_audio_source(
    api_data: &EpisodeApiData,
    metadata: &crate::nts_download::EpisodeMetadata,
) -> Result<String> {
    // 1. Try explicit Mixcloud URL from API
    if let Some(mixcloud) = &api_data.mixcloud {
        if mixcloud.starts_with("https://mixcloud") {
            return Ok(mixcloud.clone());
        }
    }

    // 2. Try audio_sources
    if let Some(sources) = &api_data.audio_sources {
        if let Some(first) = sources.first() {
            let url = &first.url;
            if url.starts_with("https://mixcloud") || url.starts_with("https://soundcloud") {
                return Ok(url.clone());
            }
        }
    }

    // 3. Try Mixcloud search
    if let Some(url) = mixcloud_search(&metadata.title, &metadata.date).await? {
        return Ok(url);
    }

    anyhow::bail!("No audio source found for this episode")
}

/// Fetch all episodes of a show
pub async fn fetch_show_episodes(show_name: &str) -> Result<Vec<String>> {
    let mut episodes = Vec::new();
    let mut offset = 0;
    let mut total_count = None;

    let client = reqwest::Client::new();

    loop {
        let url = format!(
            "https://www.nts.live/api/v2/shows/{}/episodes?offset={}",
            show_name, offset
        );

        let response = client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch show episodes")?;

        if !response.status().is_success() {
            anyhow::bail!("Show API returned status: {}", response.status());
        }

        let data: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse show episodes response")?;

        // Get total count on first request
        if total_count.is_none() {
            total_count = data
                .get("metadata")
                .and_then(|m| m.get("resultset"))
                .and_then(|r| r.get("count"))
                .and_then(|c| c.as_i64());
        }

        let limit = data
            .get("metadata")
            .and_then(|m| m.get("resultset"))
            .and_then(|r| r.get("limit"))
            .and_then(|l| l.as_i64())
            .unwrap_or(20);

        if let Some(results) = data.get("results").and_then(|r| r.as_array()) {
            for ep in results {
                if let (Some(status), Some(alias)) = (
                    ep.get("status").and_then(|s| s.as_str()),
                    ep.get("episode_alias").and_then(|a| a.as_str()),
                ) {
                    if status == "published" {
                        episodes.push(format!(
                            "https://www.nts.live/shows/{}/episodes/{}",
                            show_name, alias
                        ));
                    }
                }
            }
        }

        offset += limit as usize;

        // Check if we've got all episodes
        if let Some(count) = total_count {
            if episodes.len() >= count as usize {
                break;
            }
        } else {
            break;
        }

        // Safety limit
        if offset > 10000 {
            break;
        }
    }

    Ok(episodes)
}
