#![allow(dead_code)]

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const DEFAULT_BOOTSTRAP_URL: &str = "https://www.nts.live/infinite-mixtapes/slow-focus";
pub const MIXTAPES_API_URL: &str = "https://www.nts.live/api/v2/mixtapes";

#[derive(Debug, Clone)]
pub struct FirebaseConfig {
    pub project_id: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct EpisodeInfo {
    pub title: Option<String>,
    pub url: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AliasResult {
    pub alias: String,
    pub elapsed: Duration,
    pub data: EpisodeInfo,
}

#[derive(Debug, Clone)]
pub struct ScanSummary {
    pub total: usize,
    pub with_url: usize,
    pub title_only: usize,
    pub not_announced: usize,
    pub errors: usize,
    pub elapsed: Duration,
    pub rows: Vec<AliasResult>,
}

pub fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent("r4dio-nts-mixtape-test/0.1")
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build reqwest client")
}

pub fn parse_alias_from_url(mixtape_url: &str) -> Result<String> {
    let parsed =
        reqwest::Url::parse(mixtape_url).with_context(|| format!("invalid URL: {mixtape_url}"))?;

    let segments: Vec<_> = parsed
        .path_segments()
        .ok_or_else(|| anyhow!("URL has no path segments"))?
        .collect();

    if segments.len() >= 2 && segments[0] == "infinite-mixtapes" && !segments[1].is_empty() {
        return Ok(segments[1].to_string());
    }

    bail!("URL must look like https://www.nts.live/infinite-mixtapes/<alias>")
}

pub async fn fetch_mixtape_aliases(client: &Client) -> Result<Vec<String>> {
    let payload: Value = client
        .get(MIXTAPES_API_URL)
        .header("Accept", "application/json")
        .send()
        .await
        .context("request /api/v2/mixtapes failed")?
        .error_for_status()
        .context("/api/v2/mixtapes returned non-2xx")?
        .json()
        .await
        .context("failed to parse /api/v2/mixtapes JSON")?;

    let mut aliases = Vec::new();
    if let Some(results) = payload.get("results").and_then(Value::as_array) {
        for item in results {
            if let Some(alias) = item.get("mixtape_alias").and_then(Value::as_str) {
                aliases.push(alias.to_string());
            }
        }
    }

    aliases.sort();
    aliases.dedup();
    if aliases.is_empty() {
        bail!("/api/v2/mixtapes returned no aliases")
    }
    Ok(aliases)
}

pub async fn fetch_bootstrap_firebase(
    client: &Client,
    bootstrap_url: &str,
) -> Result<FirebaseConfig> {
    let html = client
        .get(bootstrap_url)
        .send()
        .await
        .with_context(|| format!("request bootstrap page failed: {bootstrap_url}"))?
        .error_for_status()
        .with_context(|| format!("bootstrap page non-2xx: {bootstrap_url}"))?
        .text()
        .await
        .context("failed to read bootstrap HTML")?;

    let bundle_url = extract_bundle_url(&html).context("failed to extract app bundle URL")?;
    let js = client
        .get(&bundle_url)
        .send()
        .await
        .with_context(|| format!("request bundle failed: {bundle_url}"))?
        .error_for_status()
        .with_context(|| format!("bundle non-2xx: {bundle_url}"))?
        .text()
        .await
        .context("failed to read bundle JS")?;

    extract_firebase_from_js(&js).context("failed to extract firebase config from JS")
}

pub async fn fetch_latest_episode_for_alias(
    client: &Client,
    firebase: &FirebaseConfig,
    alias: &str,
) -> Result<EpisodeInfo> {
    let endpoint = format!(
        "https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents:runQuery?key={}",
        firebase.project_id, firebase.api_key
    );

    let payload = json!({
        "structuredQuery": {
            "from": [{ "collectionId": "mixtape_titles" }],
            "where": {
                "fieldFilter": {
                    "field": { "fieldPath": "mixtape_alias" },
                    "op": "EQUAL",
                    "value": { "stringValue": alias }
                }
            },
            "orderBy": [{ "field": { "fieldPath": "started_at" }, "direction": "DESCENDING" }],
            "limit": 1
        }
    });

    let rows: Value = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("firestore query failed for alias={alias}"))?
        .error_for_status()
        .with_context(|| format!("firestore non-2xx for alias={alias}"))?
        .json()
        .await
        .with_context(|| format!("invalid firestore JSON for alias={alias}"))?;

    Ok(parse_episode_from_rows(&rows))
}

pub async fn probe_single_url(
    client: &Client,
    mixtape_url: &str,
) -> Result<(String, EpisodeInfo, Duration)> {
    let started = Instant::now();
    let alias = parse_alias_from_url(mixtape_url)?;
    let firebase = fetch_bootstrap_firebase(client, mixtape_url).await?;
    let info = fetch_latest_episode_for_alias(client, &firebase, &alias).await?;
    Ok((alias, info, started.elapsed()))
}

pub async fn scan_all_aliases(
    client: &Client,
    delay_between: Duration,
    max_aliases: Option<usize>,
    bootstrap_url: &str,
) -> Result<ScanSummary> {
    let run_started = Instant::now();
    let mut aliases = fetch_mixtape_aliases(client).await?;
    if let Some(max) = max_aliases {
        aliases.truncate(max);
    }
    let firebase = fetch_bootstrap_firebase(client, bootstrap_url).await?;

    let mut with_url = 0usize;
    let mut title_only = 0usize;
    let mut not_announced = 0usize;
    let mut errors = 0usize;
    let mut rows = Vec::new();

    let total = aliases.len();
    for (idx, alias) in aliases.iter().enumerate() {
        let started = Instant::now();
        match fetch_latest_episode_for_alias(client, &firebase, alias).await {
            Ok(data) => {
                if data.title.is_some() && data.url.is_some() {
                    with_url += 1;
                } else if data.title.is_some() {
                    title_only += 1;
                } else {
                    not_announced += 1;
                }

                let elapsed = started.elapsed();
                println!(
                    "[{}/{}] {:<24} {:<13} {:>4}ms  {}",
                    idx + 1,
                    total,
                    alias,
                    status_label(&data),
                    elapsed.as_millis(),
                    detail_label(&data)
                );
                rows.push(AliasResult {
                    alias: alias.clone(),
                    elapsed,
                    data,
                });
            }
            Err(err) => {
                errors += 1;
                let elapsed = started.elapsed();
                println!(
                    "[{}/{}] {:<24} error         {:>4}ms  {}",
                    idx + 1,
                    total,
                    alias,
                    elapsed.as_millis(),
                    err
                );
                rows.push(AliasResult {
                    alias: alias.clone(),
                    elapsed,
                    data: EpisodeInfo {
                        title: None,
                        url: None,
                        started_at: None,
                    },
                });
            }
        }

        if idx + 1 < total && !delay_between.is_zero() {
            tokio::time::sleep(delay_between).await;
        }
    }

    Ok(ScanSummary {
        total,
        with_url,
        title_only,
        not_announced,
        errors,
        elapsed: run_started.elapsed(),
        rows,
    })
}

fn extract_bundle_url(html: &str) -> Result<String> {
    let mut cursor = 0usize;
    while let Some(pos) = html[cursor..].find("src=\"") {
        let start = cursor + pos + 5;
        let rest = &html[start..];
        let Some(end_rel) = rest.find('"') else {
            break;
        };
        let src = &rest[..end_rel];
        if src.contains("/js/app.min.") && src.ends_with(".js") {
            if src.starts_with("http") {
                return Ok(src.to_string());
            }
            return Ok(format!("https://www.nts.live{src}"));
        }
        cursor = start + end_rel + 1;
    }
    bail!("no /js/app.min.*.js script source found")
}

fn extract_firebase_from_js(js: &str) -> Result<FirebaseConfig> {
    let marker = "projectId:\"nts-ios-app\"";
    let proj_pos = js
        .find(marker)
        .ok_or_else(|| anyhow!("projectId nts-ios-app marker not found"))?;

    let scan_start = proj_pos.saturating_sub(700);
    let scan_end = (proj_pos + 700).min(js.len());
    let window = &js[scan_start..scan_end];

    let key_prefix = "apiKey:\"";
    let key_pos = window
        .find(key_prefix)
        .ok_or_else(|| anyhow!("apiKey marker near projectId not found"))?;
    let key_start = key_pos + key_prefix.len();
    let key_tail = &window[key_start..];
    let key_end = key_tail
        .find('"')
        .ok_or_else(|| anyhow!("apiKey closing quote not found"))?;
    let api_key = key_tail[..key_end].to_string();

    if !api_key.starts_with("AIza") {
        bail!("extracted api_key does not look like a Firebase key")
    }

    Ok(FirebaseConfig {
        project_id: "nts-ios-app".to_string(),
        api_key,
    })
}

fn parse_episode_from_rows(rows: &Value) -> EpisodeInfo {
    let Some(arr) = rows.as_array() else {
        return EpisodeInfo {
            title: None,
            url: None,
            started_at: None,
        };
    };

    let Some(doc) = arr
        .first()
        .and_then(|row| row.get("document"))
        .and_then(Value::as_object)
    else {
        return EpisodeInfo {
            title: None,
            url: None,
            started_at: None,
        };
    };

    let fields = doc
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let title = field_string(&fields, "title");
    let show_alias = field_string(&fields, "show_alias");
    let episode_alias = field_string(&fields, "episode_alias");
    let started_at = field_string(&fields, "started_at");

    let url = match show_alias {
        Some(show) => match episode_alias {
            Some(ep) => Some(format!("https://www.nts.live/shows/{show}/episodes/{ep}")),
            None => Some(format!("https://www.nts.live/shows/{show}")),
        },
        None => None,
    };

    EpisodeInfo {
        title,
        url,
        started_at,
    }
}

fn field_string(fields: &serde_json::Map<String, Value>, field_name: &str) -> Option<String> {
    let value = fields.get(field_name)?.as_object()?;
    if let Some(s) = value.get("stringValue").and_then(Value::as_str) {
        let s = s.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(s) = value.get("timestampValue").and_then(Value::as_str) {
        let s = s.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(s) = value.get("integerValue").and_then(Value::as_str) {
        let s = s.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    None
}

fn status_label(data: &EpisodeInfo) -> &'static str {
    if data.title.is_some() && data.url.is_some() {
        "title+url"
    } else if data.title.is_some() {
        "title-only"
    } else {
        "not-announced"
    }
}

fn detail_label(data: &EpisodeInfo) -> String {
    match (&data.title, &data.url) {
        (Some(title), Some(url)) => format!("{title} | {url}"),
        (Some(title), None) => title.clone(),
        (None, _) => "not announced".to_string(),
    }
}
