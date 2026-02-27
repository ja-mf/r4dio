use radio_proto::protocol::Station;
use radio_proto::state::parse_stations_from_toml_str;
use rand::seq::SliceRandom;
use reqwest::header::HeaderValue;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Semaphore};

#[derive(Debug, Clone)]
enum PollKind {
    IcyTitle,
    ConnectedNoTitle,
    NonIcy,
    Timeout,
    Error,
}

#[derive(Debug, Clone)]
struct PollOutcome {
    station: String,
    url: String,
    kind: PollKind,
    detail: String,
    bytes_read: usize,
    elapsed: Duration,
}

#[derive(Debug, Clone)]
struct ProbeConfig {
    concurrency: usize,
    connect_timeout: Duration,
    request_timeout: Duration,
    metadata_read_timeout: Duration,
    icy_blocks: usize,
}

#[tokio::test]
#[ignore = "network diagnostic harness; run explicitly with --ignored --nocapture"]
async fn poll_non_nts_stations_for_icy_diagnostics() {
    let cfg = ProbeConfig {
        concurrency: env_usize("STATION_POLL_CONCURRENCY", 5).max(1),
        connect_timeout: Duration::from_millis(env_u64("STATION_POLL_CONNECT_TIMEOUT_MS", 4000)),
        request_timeout: Duration::from_millis(env_u64("STATION_POLL_REQUEST_TIMEOUT_MS", 20000)),
        metadata_read_timeout: Duration::from_millis(env_u64(
            "STATION_POLL_METADATA_TIMEOUT_MS",
            10000,
        )),
        icy_blocks: env_usize("STATION_POLL_ICY_BLOCKS", 4).max(1),
    };

    let stations_path = workspace_root().join("stations.toml");
    let content = std::fs::read_to_string(&stations_path).expect("failed to read stations.toml");
    let mut stations =
        parse_stations_from_toml_str(&content).expect("failed to parse stations TOML");
    stations.retain(is_non_nts_station);

    let only_match = env_csv_tokens("STATION_POLL_ONLY_MATCH");
    if !only_match.is_empty() {
        stations.retain(|s| station_matches_any_token(s, &only_match));
    }

    let max_stations = env_usize("STATION_POLL_MAX_STATIONS", 0);
    if env_bool("STATION_POLL_SHUFFLE", true) {
        let mut rng = rand::thread_rng();
        stations.shuffle(&mut rng);
    }
    if max_stations > 0 && stations.len() > max_stations {
        stations.truncate(max_stations);
    }

    assert!(
        !stations.is_empty(),
        "expected at least one non-NTS station in stations.toml"
    );

    let client = reqwest::Client::builder()
        .user_agent("r4dio-station-poll-diagnostic/0.1")
        .connect_timeout(cfg.connect_timeout)
        .timeout(cfg.request_timeout)
        .build()
        .expect("failed to build reqwest client");

    println!(
        "polling non-NTS stations: total={} concurrency={} connect={}ms req={}ms meta={}ms blocks={} filter={}",
        stations.len(),
        cfg.concurrency,
        cfg.connect_timeout.as_millis(),
        cfg.request_timeout.as_millis(),
        cfg.metadata_read_timeout.as_millis(),
        cfg.icy_blocks,
        if only_match.is_empty() {
            "(none)".to_string()
        } else {
            only_match.join(",")
        }
    );

    let run_start = Instant::now();
    let sem = std::sync::Arc::new(Semaphore::new(cfg.concurrency));
    let (tx, mut rx) = mpsc::channel::<PollOutcome>(stations.len().max(16));

    for station in stations.clone() {
        let txc = tx.clone();
        let c = client.clone();
        let cfgc = cfg.clone();
        let semc = sem.clone();
        tokio::spawn(async move {
            let _permit = semc.acquire_owned().await.expect("semaphore closed");
            let outcome = probe_station(c, station, cfgc).await;
            let _ = txc.send(outcome).await;
        });
    }
    drop(tx);

    let total = stations.len();
    let mut done = 0usize;
    let mut outcomes = Vec::with_capacity(total);

    while let Some(o) = rx.recv().await {
        done += 1;
        println!(
            "[{}/{}] {:<16} {:>5}ms {:>6}B  {}  {}",
            done,
            total,
            kind_label(&o.kind),
            o.elapsed.as_millis(),
            o.bytes_read,
            o.station,
            o.detail
        );
        outcomes.push(o);
    }

    let elapsed = run_start.elapsed();
    assert_eq!(done, total, "did not receive all outcomes");

    let title_count = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::IcyTitle))
        .count();
    let connected_no_title = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::ConnectedNoTitle))
        .count();
    let non_icy_count = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::NonIcy))
        .count();
    let timeout_count = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::Timeout))
        .count();
    let error_count = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::Error))
        .count();

    let mut lat_ms: Vec<u128> = outcomes.iter().map(|o| o.elapsed.as_millis()).collect();
    lat_ms.sort_unstable();
    let p50 = percentile_ms(&lat_ms, 0.50);
    let p90 = percentile_ms(&lat_ms, 0.90);
    let p99 = percentile_ms(&lat_ms, 0.99);

    let total_bytes: usize = outcomes.iter().map(|o| o.bytes_read).sum();
    let meta_count = title_count + connected_no_title;

    println!("--- summary ---");
    println!("total: {}", total);
    println!("icy_title: {}", title_count);
    println!("connected_no_title: {}", connected_no_title);
    println!("non_icy: {}", non_icy_count);
    println!("timeout: {}", timeout_count);
    println!("error: {}", error_count);
    println!("elapsed_seconds: {:.2}", elapsed.as_secs_f64());
    println!(
        "avg_ms_per_station: {:.1}",
        elapsed.as_millis() as f64 / total as f64
    );
    println!("latency_ms p50={} p90={} p99={}", p50, p90, p99);
    println!("total_bytes_read: {}", total_bytes);
    if meta_count > 0 {
        println!(
            "avg_bytes_for_icy_attempt: {:.1}",
            total_bytes as f64 / meta_count as f64
        );
    }

    let timeout_examples: Vec<_> = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::Timeout))
        .take(8)
        .collect();
    if !timeout_examples.is_empty() {
        println!("timeouts (sample):");
        for o in timeout_examples {
            println!("  - {} ({})", o.station, o.url);
        }
    }

    let error_examples: Vec<_> = outcomes
        .iter()
        .filter(|o| matches!(o.kind, PollKind::Error))
        .take(8)
        .collect();
    if !error_examples.is_empty() {
        println!("errors (sample):");
        for o in error_examples {
            println!("  - {}: {}", o.station, o.detail);
        }
    }
}

async fn probe_station(client: reqwest::Client, station: Station, cfg: ProbeConfig) -> PollOutcome {
    let started = Instant::now();
    let mut url = station.url.clone();
    let mut detail = String::new();

    // One-level playlist resolution for m3u/pls links.
    if is_playlist_url(&url) {
        match fetch_playlist_target(&client, &url).await {
            Ok(Some(next)) => {
                detail = format!("playlist->{}", next);
                url = next;
            }
            Ok(None) => {
                return PollOutcome {
                    station: station.name,
                    url,
                    kind: PollKind::NonIcy,
                    detail: "playlist with no playable target".to_string(),
                    bytes_read: 0,
                    elapsed: started.elapsed(),
                };
            }
            Err(e) => {
                return classify_failure(station.name, url, started.elapsed(), e.to_string());
            }
        }
    }

    let mut req = client
        .get(&url)
        .header("Icy-MetaData", HeaderValue::from_static("1"));
    if looks_hls_url(&url) {
        req = req.header(
            "Accept",
            HeaderValue::from_static("application/vnd.apple.mpegurl,*/*"),
        );
    }

    let mut resp = match req.send().await {
        Ok(r) => match r.error_for_status() {
            Ok(ok) => ok,
            Err(e) => {
                return classify_failure(station.name, url, started.elapsed(), e.to_string());
            }
        },
        Err(e) => {
            return classify_failure(station.name, url, started.elapsed(), e.to_string());
        }
    };

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.contains("mpegurl") || looks_hls_url(&url) {
        return PollOutcome {
            station: station.name,
            url,
            kind: PollKind::NonIcy,
            detail: if detail.is_empty() {
                "hls/playlist stream".to_string()
            } else {
                format!("{} hls/playlist stream", detail)
            },
            bytes_read: 0,
            elapsed: started.elapsed(),
        };
    }

    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());

    if let Some(metaint) = metaint {
        if !(1..=256_000).contains(&metaint) {
            return PollOutcome {
                station: station.name,
                url,
                kind: PollKind::NonIcy,
                detail: format!("invalid icy-metaint={}", metaint),
                bytes_read: 0,
                elapsed: started.elapsed(),
            };
        }

        match read_icy_streamtitle(
            &mut resp,
            metaint,
            cfg.metadata_read_timeout,
            cfg.icy_blocks,
        )
        .await
        {
            Ok((Some(title), bytes_read)) => PollOutcome {
                station: station.name,
                url,
                kind: PollKind::IcyTitle,
                detail: format!("title={}", title),
                bytes_read,
                elapsed: started.elapsed(),
            },
            Ok((None, bytes_read)) => PollOutcome {
                station: station.name,
                url,
                kind: PollKind::ConnectedNoTitle,
                detail: if detail.is_empty() {
                    "icy metadata empty".to_string()
                } else {
                    format!("{} icy metadata empty", detail)
                },
                bytes_read,
                elapsed: started.elapsed(),
            },
            Err(e) => classify_failure(station.name, url, started.elapsed(), e),
        }
    } else {
        PollOutcome {
            station: station.name,
            url,
            kind: PollKind::NonIcy,
            detail: if detail.is_empty() {
                "no icy-metaint header".to_string()
            } else {
                format!("{} no icy-metaint header", detail)
            },
            bytes_read: 0,
            elapsed: started.elapsed(),
        }
    }
}

fn classify_failure(station: String, url: String, elapsed: Duration, err: String) -> PollOutcome {
    let kind = if err.to_ascii_lowercase().contains("timed out") {
        PollKind::Timeout
    } else {
        PollKind::Error
    };
    PollOutcome {
        station,
        url,
        kind,
        detail: err,
        bytes_read: 0,
        elapsed,
    }
}

async fn read_icy_streamtitle(
    resp: &mut reqwest::Response,
    metaint: usize,
    timeout_total: Duration,
    max_blocks: usize,
) -> Result<(Option<String>, usize), String> {
    let deadline = Instant::now() + timeout_total;
    let mut buf: Vec<u8> = Vec::with_capacity((metaint + 1024).min(128 * 1024));
    let mut cursor = 0usize;

    for _ in 0..max_blocks {
        let required_for_len = cursor + metaint + 1;
        while buf.len() < required_for_len {
            let now = Instant::now();
            if now >= deadline {
                return Err("timed out waiting for ICY metadata length byte".to_string());
            }
            let remain = deadline.saturating_duration_since(now);
            let next = tokio::time::timeout(remain, resp.chunk())
                .await
                .map_err(|_| "timed out waiting for stream chunk".to_string())?
                .map_err(|e| e.to_string())?;

            match next {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => return Err("stream ended before ICY metadata block".to_string()),
            }
        }

        let len_byte = buf[cursor + metaint] as usize;
        let meta_len = len_byte * 16;
        if meta_len > 16 * 255 {
            return Err(format!("metadata block too large: {}", meta_len));
        }

        let block_total = metaint + 1 + meta_len;
        let required_total = cursor + block_total;
        while buf.len() < required_total {
            let now = Instant::now();
            if now >= deadline {
                return Err("timed out waiting for ICY metadata payload".to_string());
            }
            let remain = deadline.saturating_duration_since(now);
            let next = tokio::time::timeout(remain, resp.chunk())
                .await
                .map_err(|_| "timed out waiting for stream chunk".to_string())?
                .map_err(|e| e.to_string())?;
            match next {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => return Err("stream ended before ICY metadata payload".to_string()),
            }
        }

        if meta_len > 0 {
            let meta_start = cursor + metaint + 1;
            let meta_end = meta_start + meta_len;
            let meta = &buf[meta_start..meta_end];
            if let Some(title) = parse_stream_title(meta) {
                return Ok((Some(title), required_total));
            }
        }

        cursor = required_total;
    }

    Ok((None, cursor))
}

fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(meta)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }

    if let Some(start) = text.find("StreamTitle='") {
        let rest = &text[start + "StreamTitle='".len()..];
        if let Some(end) = rest.find("';") {
            let title = rest[..end].trim();
            return if title.is_empty() {
                None
            } else {
                Some(title.to_string())
            };
        }
    }

    if let Some(start) = text.find("StreamTitle=\"") {
        let rest = &text[start + "StreamTitle=\"".len()..];
        if let Some(end) = rest.find("\";") {
            let title = rest[..end].trim();
            return if title.is_empty() {
                None
            } else {
                Some(title.to_string())
            };
        }
    }

    None
}

async fn fetch_playlist_target(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<String>, String> {
    let body = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    if url.to_ascii_lowercase().ends_with(".pls") {
        for line in body.lines() {
            let l = line.trim();
            if l.to_ascii_lowercase().starts_with("file") {
                if let Some((_, v)) = l.split_once('=') {
                    let resolved = resolve_relative_url(url, v.trim())?;
                    return Ok(Some(resolved));
                }
            }
        }
        return Ok(None);
    }

    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let resolved = resolve_relative_url(url, l)?;
        return Ok(Some(resolved));
    }
    Ok(None)
}

fn resolve_relative_url(base: &str, candidate: &str) -> Result<String, String> {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return Ok(candidate.to_string());
    }
    let base_url = reqwest::Url::parse(base).map_err(|e| e.to_string())?;
    base_url
        .join(candidate)
        .map(|u| u.to_string())
        .map_err(|e| e.to_string())
}

fn is_non_nts_station(st: &Station) -> bool {
    if st.name.eq_ignore_ascii_case("NTS 1") || st.name.eq_ignore_ascii_case("NTS 2") {
        return false;
    }
    if !st.mixtape_url.trim().is_empty() {
        return false;
    }
    st.url.starts_with("http://") || st.url.starts_with("https://")
}

fn is_playlist_url(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.ends_with(".m3u") || l.ends_with(".m3u8") || l.ends_with(".pls")
}

fn looks_hls_url(url: &str) -> bool {
    url.to_ascii_lowercase().contains(".m3u8")
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(crate_dir.as_path())
        .to_path_buf()
}

fn kind_label(kind: &PollKind) -> &'static str {
    match kind {
        PollKind::IcyTitle => "icy-title",
        PollKind::ConnectedNoTitle => "no-title",
        PollKind::NonIcy => "non-icy",
        PollKind::Timeout => "timeout",
        PollKind::Error => "error",
    }
}

fn percentile_ms(data: &[u128], p: f64) -> u128 {
    if data.is_empty() {
        return 0;
    }
    let pos = ((data.len() - 1) as f64 * p).round() as usize;
    data[pos.min(data.len() - 1)]
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let l = v.trim().to_ascii_lowercase();
            !(l == "0" || l == "false" || l == "no" || l == "off")
        }
        Err(_) => default,
    }
}

fn env_csv_tokens(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn station_matches_any_token(st: &Station, tokens: &[String]) -> bool {
    let hay_name = st.name.to_ascii_lowercase();
    let hay_url = st.url.to_ascii_lowercase();
    tokens
        .iter()
        .any(|t| hay_name.contains(t) || hay_url.contains(t))
}
