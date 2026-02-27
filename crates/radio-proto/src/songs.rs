//! Song recognition and VDS (Vibra Data Store) persistence.
//!
//! ## New async pipeline (fire-and-forget, patch-in-place)
//!
//! On `i` press:
//!   1. Generate a `job_id` (hash of timestamp+station).
//!   2. Write an empty VDS row immediately with job_id, timestamp, station, icy_info.
//!   3. Spawn three concurrent tasks that each `patch_vds_by_job_id` when done:
//!      a. vibra  — ffmpeg captures 10 s raw PCM from stream, pipes to vibra (1 attempt)
//!      b. ICY    — already available, patched immediately
//!      c. NTS    — async API call (NTS 1/2 only)
//!
//! ## VDS schema (tab-separated)
//!
//!   job_id  timestamp  station  icy_info  nts_show  nts_tag  nts_url  vibra_rec
//!
//! All fields except job_id and timestamp may be empty strings.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};

// ── Public types ──────────────────────────────────────────────────────────────

/// Which data source provided a particular field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecognitionSource {
    Vibra,
    Icy,
    Nts,
}

impl RecognitionSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Vibra => "vibra",
            Self::Icy   => "icy",
            Self::Nts   => "nts",
        }
    }
}

/// One row in the VDS. Each source column is populated independently as data arrives.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecognitionResult {
    /// Unique job identifier (hex hash). Used to patch-in-place after initial write.
    pub job_id: String,
    pub timestamp: Option<DateTime<Local>>,
    pub station: Option<String>,
    /// Raw ICY title as-received from the stream.
    pub icy_info: Option<String>,
    /// NTS show broadcast title.
    pub nts_show: Option<String>,
    /// NTS tag / genre summary.
    pub nts_tag: Option<String>,
    /// NTS show URL.
    pub nts_url: Option<String>,
    /// Best track info from vibra: "Artist – Title" or just "Title".
    pub vibra_rec: Option<String>,
}

impl RecognitionResult {
    /// Display string for the songs pane.
    /// Priority: vibra_rec > icy_info > "?"
    pub fn display(&self) -> String {
        if let Some(v) = &self.vibra_rec {
            if !v.is_empty() {
                return v.clone();
            }
        }
        if let Some(i) = &self.icy_info {
            if !i.is_empty() {
                return i.clone();
            }
        }
        "?".to_string()
    }

    /// Which sources have contributed so far (for badge display).
    pub fn sources(&self) -> Vec<RecognitionSource> {
        let mut v = Vec::new();
        if self.vibra_rec.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            v.push(RecognitionSource::Vibra);
        }
        if self.nts_show.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            v.push(RecognitionSource::Nts);
        }
        if self.icy_info.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            v.push(RecognitionSource::Icy);
        }
        v
    }

    pub fn source_label(&self) -> String {
        self.sources().iter().map(|s| s.label()).collect::<Vec<_>>().join("+")
    }
}

// ── job_id generation ─────────────────────────────────────────────────────────

/// Generate a short job ID: first 12 hex chars of a simple hash of timestamp+station.
pub fn make_job_id(ts: &DateTime<Local>, station: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    ts.timestamp_nanos_opt().unwrap_or(ts.timestamp()).hash(&mut h);
    if let Some(s) = station { s.hash(&mut h); }
    format!("{:016x}", h.finish())
}

// ── Vibra recognition (ffmpeg capture) ───────────────────────────────────────

/// Capture 10 s of `stream_url` via ffmpeg (raw PCM to stdout), pipe to vibra
/// for fingerprinting.  One attempt, no retries.
///
/// Returns the raw vibra JSON on success.
pub async fn recognize_via_vibra(stream_url: &str) -> Option<serde_json::Value> {
    let vibra = crate::platform::find_vibra_binary()?;
    let ffmpeg = crate::platform::find_ffmpeg_binary()?;

    info!("[vibra] Starting capture: url={}", stream_url);
    match try_vibra_ffmpeg(&vibra, &ffmpeg, stream_url).await {
        Ok(json) => {
            info!("[vibra] Recognition succeeded");
            Some(json)
        }
        Err(e) => {
            warn!("[vibra] Recognition failed: {}", e);
            None
        }
    }
}

async fn try_vibra_ffmpeg(
    vibra: &PathBuf,
    ffmpeg: &PathBuf,
    stream_url: &str,
) -> anyhow::Result<serde_json::Value> {
    // Spawn ffmpeg: connect to stream, decode to raw PCM (s16le, 44100 Hz, stereo) on stdout.
    // pipe:1 routes the output to stdout. -t 10 records exactly 10 seconds.
    info!(
        "[vibra] Spawning ffmpeg: {} -i {} -t 10 -vn -ar 44100 -ac 2 -f s16le pipe:1",
        ffmpeg.display(),
        stream_url
    );

    let mut ffmpeg_proc = std::process::Command::new(ffmpeg)
        .args([
            "-i", stream_url,
            "-t", "10",
            "-vn",
            "-ar", "44100",
            "-ac", "2",
            "-f", "s16le",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let ffmpeg_pid = ffmpeg_proc.id();
    info!("[vibra] ffmpeg spawned with PID: {:?}", ffmpeg_pid);

    let ffmpeg_stdout = ffmpeg_proc.stdout.take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg stdout unavailable"))?;

    // Run vibra in a blocking thread: reads raw PCM from stdin (ffmpeg stdout), outputs JSON.
    // ffmpeg outputs 44100 Hz stereo 16-bit signed little-endian (s16le).
    let vibra_path = vibra.clone();
    info!(
        "[vibra] Spawning vibra: {} --recognize --seconds 10 --rate 44100 --channels 2 --bits 16",
        vibra_path.display()
    );

    let output = tokio::task::spawn_blocking(move || {
        let result = std::process::Command::new(&vibra_path)
            .args([
                "--recognize",
                "--seconds", "10",
                "--rate", "44100",
                "--channels", "2",
                "--bits", "16",
            ])
            .stdin(std::process::Stdio::from(ffmpeg_stdout))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();
        if let Ok(ref out) = result {
            info!("[vibra] vibra exited with status: {:?}", out.status.code());
        }
        result
    }).await??;

    // Wait for ffmpeg to finish (should already be done after 10s + vibra read)
    let ffmpeg_exit = ffmpeg_proc.wait()?;
    debug!("[vibra] ffmpeg exited: {:?}", ffmpeg_exit.code());

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("vibra exited {}: {}", output.status, stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        anyhow::bail!("vibra produced no output");
    }

    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| anyhow::anyhow!("vibra JSON parse: {} (raw: {})", e, stdout.trim()))?;

    if json.get("track").is_none() {
        anyhow::bail!("vibra: no track in response: {}", stdout.trim());
    }

    Ok(json)
}

/// Parse vibra JSON → "Artist – Title" string (or just "Title").
pub fn vibra_rec_string(json: &serde_json::Value) -> Option<String> {
    let track = &json["track"];
    let title  = track["title"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())?;
    let artist = track["subtitle"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Some(match artist {
        Some(a) => format!("{} \u{2013} {}", a, title),
        None    => title,
    })
}

// ── NTS recognition ───────────────────────────────────────────────────────────

/// Query the NTS live API for channel `ch` (0 = NTS 1, 1 = NTS 2).
/// Returns (show_title, tag_summary, show_url) on success.
pub async fn recognize_via_nts(ch: usize) -> Option<(String, Option<String>, Option<String>)> {
    let url = "https://www.nts.live/api/v2/live";
    info!("[nts] Querying live API for channel {}", ch);
    let resp = reqwest::get(url).await.map_err(|e| warn!("[nts] request error: {}", e)).ok()?;
    let json: serde_json::Value = resp.json().await.map_err(|e| warn!("[nts] JSON error: {}", e)).ok()?;

    let channel = &json["results"][ch];
    if channel.is_null() {
        warn!("[nts] Channel {} is null in response", ch);
        return None;
    }

    let now = &channel["now"];
    let show_title = now["broadcast_title"].as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    // Tag summary: first few genres joined
    let tags = now["embeds"]["details"]["genres"].as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g["value"].as_str())
                .take(3)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty());

    let show_url = now["embeds"]["details"]["slug"].as_str()
        .map(|slug| format!("https://www.nts.live/shows/{}", slug));

    info!("[nts] Got show: {:?}, tags: {:?}, url: {:?}", show_title, tags, show_url);
    Some((show_title, tags, show_url))
}

// ── ICY parsing ───────────────────────────────────────────────────────────────

/// Parse "Artist - Title" or "Title" from an ICY string.
pub fn parse_icy(icy: &str) -> (Option<String>, Option<String>) {
    let s = icy.trim();
    if let Some(pos) = s.find(" - ") {
        let artist = s[..pos].trim().to_string();
        let title  = s[pos + 3..].trim().to_string();
        (
            Some(title).filter(|t| !t.is_empty()),
            Some(artist).filter(|a| !a.is_empty()),
        )
    } else {
        (Some(s.to_string()).filter(|t| !t.is_empty()), None)
    }
}

// ── VDS persistence ───────────────────────────────────────────────────────────

const VDS_HEADER: &str = "job_id\ttimestamp\tstation\ticy_info\tnts_show\tnts_tag\tnts_url\tvibra_rec\n";

/// Write an initial (possibly partial) VDS row.
/// Called immediately on `i` press with whatever is known at that moment.
pub async fn append_to_vds(path: &PathBuf, result: &RecognitionResult) -> anyhow::Result<()> {
    let exists = path.exists();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    use tokio::io::AsyncWriteExt;
    if !exists {
        f.write_all(VDS_HEADER.as_bytes()).await?;
    }
    f.write_all(encode_row(result).as_bytes()).await?;
    info!("[vds] Wrote initial row job_id={}", result.job_id);
    Ok(())
}

/// Patch an existing VDS row identified by `job_id`, updating only the
/// columns that are `Some` in `patch`. Other columns are left unchanged.
pub async fn patch_vds_by_job_id(path: &PathBuf, job_id: &str, patch: VdsPatch) -> anyhow::Result<()> {
    let content = tokio::fs::read_to_string(path).await
        .unwrap_or_default();

    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut found = false;

    for line in lines.iter_mut() {
        // Skip header
        if line.starts_with("job_id\t") {
            continue;
        }
        let cols: Vec<&str> = line.splitn(9, '\t').collect();
        if cols.first().map(|c| *c == job_id).unwrap_or(false) {
            found = true;
            // Parse the existing row
            let get = |i: usize| cols.get(i).copied().unwrap_or("").to_string();
            let mut r = RecognitionResult {
                job_id:    get(0),
                timestamp: parse_ts(&get(1)),
                station:   nn(&get(2)),
                icy_info:  nn(&get(3)),
                nts_show:  nn(&get(4)),
                nts_tag:   nn(&get(5)),
                nts_url:   nn(&get(6)),
                vibra_rec: nn(&get(7)),
            };
            // Apply patch
            if let Some(v) = patch.icy_info   { r.icy_info  = Some(v); }
            if let Some(v) = patch.nts_show   { r.nts_show  = Some(v); }
            if let Some(v) = patch.nts_tag    { r.nts_tag   = Some(v); }
            if let Some(v) = patch.nts_url    { r.nts_url   = Some(v); }
            if let Some(v) = patch.vibra_rec  { r.vibra_rec = Some(v); }
            *line = encode_row(&r).trim_end_matches('\n').to_string();
            info!("[vds] Patched job_id={}: icy={:?} nts={:?} vibra={:?}",
                job_id, r.icy_info, r.nts_show, r.vibra_rec);
            break;
        }
    }

    if !found {
        warn!("[vds] patch_vds_by_job_id: job_id={} not found in file", job_id);
        return Ok(());
    }

    let new_content = lines.join("\n") + "\n";
    tokio::fs::write(path, new_content).await?;
    Ok(())
}

/// Fields that can be patched after initial write.
#[derive(Debug, Default)]
pub struct VdsPatch {
    pub icy_info:  Option<String>,
    pub nts_show:  Option<String>,
    pub nts_tag:   Option<String>,
    pub nts_url:   Option<String>,
    pub vibra_rec: Option<String>,
}

fn encode_row(r: &RecognitionResult) -> String {
    let ts = r.timestamp
        .as_ref()
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        vds_esc(&r.job_id),
        ts,
        vds_esc(r.station.as_deref().unwrap_or("")),
        vds_esc(r.icy_info.as_deref().unwrap_or("")),
        vds_esc(r.nts_show.as_deref().unwrap_or("")),
        vds_esc(r.nts_tag.as_deref().unwrap_or("")),
        vds_esc(r.nts_url.as_deref().unwrap_or("")),
        vds_esc(r.vibra_rec.as_deref().unwrap_or("")),
    )
}

fn vds_esc(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ").replace('\r', "")
}

fn parse_ts(s: &str) -> Option<DateTime<Local>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .and_then(|dt| dt.and_local_timezone(Local).earliest())
}

fn nn(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// Load the last `limit` rows from songs.vds.
pub fn load_vds(path: &PathBuf, limit: usize) -> Vec<RecognitionResult> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.starts_with("job_id\t")).unwrap_or(false) {
        lines.remove(0);
    }
    let start = lines.len().saturating_sub(limit);
    lines[start..].iter().filter_map(|line| parse_vds_row(line)).collect()
}

fn parse_vds_row(line: &str) -> Option<RecognitionResult> {
    let cols: Vec<&str> = line.splitn(9, '\t').collect();
    if cols.len() < 3 { return None; }
    Some(RecognitionResult {
        job_id:    cols[0].trim().to_string(),
        timestamp: cols.get(1).and_then(|s| parse_ts(s.trim())),
        station:   cols.get(2).and_then(|s| nn(s)),
        icy_info:  cols.get(3).and_then(|s| nn(s)),
        nts_show:  cols.get(4).and_then(|s| nn(s)),
        nts_tag:   cols.get(5).and_then(|s| nn(s)),
        nts_url:   cols.get(6).and_then(|s| nn(s)),
        vibra_rec: cols.get(7).and_then(|s| nn(s)),
    })
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_icy_artist_title() {
        let (title, artist) = parse_icy("The Beatles - Hey Jude");
        assert_eq!(title.as_deref(), Some("Hey Jude"));
        assert_eq!(artist.as_deref(), Some("The Beatles"));
    }

    #[test]
    fn test_parse_icy_no_sep() {
        let (title, artist) = parse_icy("Some Show Name");
        assert_eq!(title.as_deref(), Some("Some Show Name"));
        assert!(artist.is_none());
    }

    #[test]
    fn test_vibra_rec_string() {
        let json = serde_json::json!({
            "track": { "title": "Hey Jude", "subtitle": "The Beatles" }
        });
        assert_eq!(vibra_rec_string(&json).as_deref(), Some("The Beatles \u{2013} Hey Jude"));
    }

    #[test]
    fn test_display_priority() {
        let mut r = RecognitionResult::default();
        r.job_id = "abc".into();
        assert_eq!(r.display(), "?");

        r.icy_info = Some("ICY Title".into());
        assert_eq!(r.display(), "ICY Title");

        r.vibra_rec = Some("Vibra Artist \u{2013} Vibra Title".into());
        assert_eq!(r.display(), "Vibra Artist \u{2013} Vibra Title");
    }

    #[test]
    fn test_sources() {
        let r = RecognitionResult {
            job_id: "x".into(),
            vibra_rec: Some("A \u{2013} B".into()),
            icy_info: Some("raw".into()),
            nts_show: None,
            ..Default::default()
        };
        let srcs = r.sources();
        assert!(srcs.contains(&RecognitionSource::Vibra));
        assert!(srcs.contains(&RecognitionSource::Icy));
        assert!(!srcs.contains(&RecognitionSource::Nts));
    }

    #[test]
    fn test_vds_encode_decode_roundtrip() {
        let r = RecognitionResult {
            job_id:    "deadbeef".into(),
            timestamp: None,
            station:   Some("NTS 1".into()),
            icy_info:  Some("Artist - Track".into()),
            nts_show:  Some("Morning Show".into()),
            nts_tag:   Some("Jazz, Soul".into()),
            nts_url:   Some("https://www.nts.live/shows/morning".into()),
            vibra_rec: Some("Artist \u{2013} Track".into()),
        };
        let encoded = encode_row(&r);
        let decoded = parse_vds_row(encoded.trim()).unwrap();
        assert_eq!(decoded.job_id, "deadbeef");
        assert_eq!(decoded.station.as_deref(), Some("NTS 1"));
        assert_eq!(decoded.icy_info.as_deref(), Some("Artist - Track"));
        assert_eq!(decoded.vibra_rec.as_deref(), Some("Artist \u{2013} Track"));
    }
}
