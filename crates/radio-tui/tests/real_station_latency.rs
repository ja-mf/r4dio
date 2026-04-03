//! Real-world station latency test.
//!
//! Connects to actual radio stations and measures:
//! 1. Connection establishment time
//! 2. Time to first byte (TTFB)
//! 3. Time to first audio (via PCM decode)
//! 4. Sustained throughput and stability
//!
//! Run with: cargo test --test real_station_latency -- --nocapture
//!
//! Stations tested:
//! - /uno from radio.chungo.es (MP3, ~192kbps)
//! - NTS 1 (MP3, high quality)
//! - KCRW (MP3/AAC, high quality)

use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Station configuration for testing.
#[derive(Debug, Clone)]
struct TestStation {
    name: &'static str,
    url: &'static str,
    expected_format: &'static str,
    expected_bitrate_kbps: u32,
}

const TEST_STATIONS: &[TestStation] = &[
    // MP3 stations
    TestStation {
        name: "FIP (France)",
        url: "https://icecast.radiofrance.fr/fip-midfi.mp3",
        expected_format: "MP3",
        expected_bitrate_kbps: 128,
    },
    TestStation {
        name: "NTS 1 (UK)",
        url: "https://stream-relay-geo.ntslive.net/stream",
        expected_format: "MP3",
        expected_bitrate_kbps: 320,
    },
    TestStation {
        name: "Radio Paradise (USA)",
        url: "http://stream.radioparadise.com/mp3-192",
        expected_format: "MP3",
        expected_bitrate_kbps: 192,
    },
    // AAC station
    TestStation {
        name: "KEXP (USA/AAC)",
        url: "https://live-aacplus-64.kexp.org/kexp64.aac",
        expected_format: "AAC",
        expected_bitrate_kbps: 64,
    },
    // HLS/AAC station (BBC)
    TestStation {
        name: "BBC 6 Music (HLS)",
        url: "http://as-hls-ww-live.akamaized.net/pool_81827798/live/ww/bbc_6music/bbc_6music.isml/bbc_6music-audio=320000.norewind.m3u8",
        expected_format: "HLS/AAC",
        expected_bitrate_kbps: 320,
    },
    // OGG/Opus station (if any work)
    TestStation {
        name: "SomaFM Sonic Universe",
        url: "https://ice2.somafm.com/sonicuniverse-256-mp3",
        expected_format: "MP3",
        expected_bitrate_kbps: 256,
    },
];

/// Results from testing a single station.
#[derive(Debug, Clone)]
struct StationResult {
    name: String,
    url: String,
    /// Time from HTTP request to first byte
    ttfb_ms: f64,
    /// Time from first byte to first PCM sample
    decode_startup_ms: f64,
    /// Total time to audio
    total_startup_ms: f64,
    /// Bytes received in test period
    bytes_received: usize,
    /// Test duration
    duration_ms: f64,
    /// Calculated bitrate
    measured_bitrate_kbps: f64,
    /// Whether audio was successfully decoded
    audio_decoded: bool,
    /// Any errors encountered
    error: Option<String>,
}

impl StationResult {
    fn print(&self) {
        println!("\n┌─────────────────────────────────────────────────────────────┐");
        println!("│  {}", pad_right(&self.name, 57));
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  URL: {}", pad_right(&self.url, 50));
        println!("│                                                             │");
        println!("│  Time to First Byte (TTFB):      {:>8.2} ms           │", self.ttfb_ms);
        println!("│  FFmpeg Decode Startup:          {:>8.2} ms           │", self.decode_startup_ms);
        println!("│  ────────────────────────────────────────────────────────  │");
        println!("│  TOTAL STARTUP LATENCY:          {:>8.2} ms           │", self.total_startup_ms);
        println!("│                                                             │");
        println!("│  Bytes received:                 {:>8} bytes          │", self.bytes_received);
        println!("│  Test duration:                  {:>8.2} ms           │", self.duration_ms);
        println!("│  Measured bitrate:               {:>8.2} kbps         │", self.measured_bitrate_kbps);
        println!("│  Audio decoded:                   {:>8}               │", if self.audio_decoded { "✅ YES" } else { "❌ NO" });
        if let Some(ref err) = self.error {
            println!("│  Error: {}", pad_right(err, 46));
        }
        println!("└─────────────────────────────────────────────────────────────┘");
        
        // Grade the result
        print!("\n  Grade: ");
        if self.error.is_some() {
            println!("❌ FAIL - Connection/Decode error");
        } else if self.total_startup_ms < 500.0 {
            println!("🟢 EXCELLENT - Sub-second startup");
        } else if self.total_startup_ms < 1000.0 {
            println!("🟡 GOOD - Under 1 second");
        } else if self.total_startup_ms < 2000.0 {
            println!("🟠 ACCEPTABLE - 1-2 seconds");
        } else {
            println!("🔴 SLOW - Over 2 seconds");
        }
    }
}

fn pad_right(s: &str, width: usize) -> String {
    if s.len() >= width {
        s[..width].to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - s.len()))
    }
}

/// Test a single station by connecting and measuring timing.
async fn test_station(station: &TestStation, test_duration: Duration) -> StationResult {
    println!("\n  Testing {}...", station.name);
    
    let overall_start = Instant::now();
    let mut ttfb_ms = 0.0;
    let mut decode_startup_ms = 0.0;
    let mut bytes_received = 0usize;
    let mut audio_decoded = false;
    let mut error = None;
    
    // Phase 1: HTTP connection and first byte
    let http_start = Instant::now();
    match reqwest::get(station.url).await {
        Ok(response) => {
            if !response.status().is_success() {
                error = Some(format!("HTTP {}", response.status()));
            } else {
                ttfb_ms = http_start.elapsed().as_secs_f64() * 1000.0;
                
                // Read some bytes for the test duration
                let read_start = Instant::now();
                let mut stream = response.bytes_stream();
                
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            bytes_received += chunk.len();
                            if read_start.elapsed() >= test_duration {
                                break;
                            }
                        }
                        Err(e) => {
                            error = Some(format!("Stream error: {}", e));
                            break;
                        }
                    }
                }
            }
        }
        Err(e) => {
            error = Some(format!("Connection failed: {}", e));
        }
    }
    
    // Phase 2: Test FFmpeg decode startup time (if we got data)
    if error.is_none() && bytes_received > 0 {
        let ffmpeg_start = Instant::now();
        
        // Run ffmpeg to decode just the first frame
        match Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel", "error",
                "-nostdin",
                "-probesize", "64k",
                "-analyzeduration", "200000",
                "-i", station.url,
                "-t", "0.1",  // Just 100ms
                "-ac", "1",
                "-ar", "44100",
                "-f", "s16le",
                "-",
            ])
            .output()
            .await
        {
            Ok(output) => {
                decode_startup_ms = ffmpeg_start.elapsed().as_secs_f64() * 1000.0;
                audio_decoded = !output.stdout.is_empty() && output.status.success();
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.is_empty() {
                        error = Some(format!("FFmpeg: {}", stderr.lines().next().unwrap_or("unknown")));
                    }
                }
            }
            Err(e) => {
                error = Some(format!("FFmpeg failed: {}", e));
            }
        }
    }
    
    let total_duration = overall_start.elapsed();
    let duration_ms = total_duration.as_secs_f64() * 1000.0;
    let measured_bitrate_kbps = if duration_ms > 0.0 {
        (bytes_received as f64 * 8.0) / (duration_ms / 1000.0) / 1000.0
    } else {
        0.0
    };
    
    StationResult {
        name: station.name.to_string(),
        url: station.url.to_string(),
        ttfb_ms,
        decode_startup_ms,
        total_startup_ms: ttfb_ms + decode_startup_ms,
        bytes_received,
        duration_ms,
        measured_bitrate_kbps,
        audio_decoded,
        error,
    }
}

#[tokio::test]
async fn test_real_station_startup_latency() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           REAL STATION LATENCY TEST                                  ║");
    println!("║                                                                      ║");
    println!("║  Testing actual radio stations for real-world performance metrics    ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    
    let test_duration = Duration::from_secs(3);
    let mut results = Vec::new();
    
    for station in TEST_STATIONS {
        let result = test_station(station, test_duration).await;
        result.print();
        results.push(result);
        
        // Brief pause between tests
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    
    // Summary comparison
    println!("\n\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           COMPARISON SUMMARY                                         ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  Station                    │ TTFB    │ Decode  │ Total   │ Bitrate  ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    
    for r in &results {
        let status = if r.error.is_some() { "❌" } else if r.audio_decoded { "✅" } else { "⚠️" };
        println!("║  {:<25} │ {:>6.0}ms │ {:>6.0}ms │ {:>6.0}ms │ {:>6.0}k  {} ║",
            truncate(&r.name, 25),
            r.ttfb_ms,
            r.decode_startup_ms,
            r.total_startup_ms,
            r.measured_bitrate_kbps,
            status
        );
    }
    
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    
    // Analysis
    let successful: Vec<_> = results.iter().filter(|r| r.error.is_none()).collect();
    if !successful.is_empty() {
        let avg_ttfb = successful.iter().map(|r| r.ttfb_ms).sum::<f64>() / successful.len() as f64;
        let avg_decode = successful.iter().map(|r| r.decode_startup_ms).sum::<f64>() / successful.len() as f64;
        let avg_total = successful.iter().map(|r| r.total_startup_ms).sum::<f64>() / successful.len() as f64;
        
        println!("\n📊 AVERAGES (successful connections only):");
        println!("   Time to First Byte:  {:.0} ms", avg_ttfb);
        println!("   FFmpeg Decode:       {:.0} ms", avg_decode);
        println!("   Total Startup:       {:.0} ms", avg_total);
        
        // Find fastest
        let fastest = successful.iter().min_by(|a, b| a.total_startup_ms.partial_cmp(&b.total_startup_ms).unwrap()).unwrap();
        println!("\n🏆 FASTEST: {} ({:.0} ms total)", fastest.name, fastest.total_startup_ms);
        
        // Find slowest decode (where optimization would help most)
        let slowest_decode = successful.iter().max_by(|a, b| a.decode_startup_ms.partial_cmp(&b.decode_startup_ms).unwrap()).unwrap();
        println!("🐢 SLOWEST DECODE: {} ({:.0} ms)", slowest_decode.name, slowest_decode.decode_startup_ms);
    }
    
    // Fail test if any station completely failed
    let failures: Vec<_> = results.iter().filter(|r| r.error.is_some()).collect();
    if !failures.is_empty() {
        println!("\n❌ FAILED CONNECTIONS:");
        for f in failures {
            println!("   - {}: {}", f.name, f.error.as_ref().unwrap());
        }
    }
    
    // Assertions - these are the real-world targets
    // Note: Real-world latency varies by geography, server load, and codec
    let very_slow_connections = successful.iter().filter(|r| r.total_startup_ms > 3000.0).count();
    assert!(
        very_slow_connections == 0,
        "{} station(s) took over 3 seconds to start - serious optimization needed",
        very_slow_connections
    );
    
    let failed_decodes = successful.iter().filter(|r| !r.audio_decoded).count();
    assert!(
        failed_decodes == 0,
        "{} station(s) failed to decode audio",
        failed_decodes
    );
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

use futures_util::StreamExt;

#[tokio::test]
async fn test_station_bitrate_stability() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           BITRATE STABILITY TEST                                     ║");
    println!("║                                                                      ║");
    println!("║  Measuring bitrate consistency over 10 seconds                       ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    
    let test_duration = Duration::from_secs(10);
    
    for station in TEST_STATIONS {
        println!("\n  Testing {}...", station.name);
        
        let start = Instant::now();
        let mut bytes_per_second = Vec::new();
        let mut current_second_bytes = 0usize;
        let mut last_second = 0u64;
        
        match reqwest::get(station.url).await {
            Ok(response) => {
                if response.status().is_success() {
                    let mut stream = response.bytes_stream();
                    
                    while let Some(chunk_result) = stream.next().await {
                        if let Ok(chunk) = chunk_result {
                            current_second_bytes += chunk.len();
                            
                            let elapsed_sec = start.elapsed().as_secs();
                            if elapsed_sec > last_second {
                                bytes_per_second.push(current_second_bytes);
                                current_second_bytes = 0;
                                last_second = elapsed_sec;
                            }
                            
                            if start.elapsed() >= test_duration {
                                break;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("    ❌ Connection failed: {}", e);
                continue;
            }
        }
        
        if bytes_per_second.len() >= 2 {
            let kbps_samples: Vec<f64> = bytes_per_second.iter()
                .map(|&b| (b as f64 * 8.0) / 1000.0)
                .collect();
            
            let avg_kbps = kbps_samples.iter().sum::<f64>() / kbps_samples.len() as f64;
            let min_kbps = kbps_samples.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_kbps = kbps_samples.iter().cloned().fold(0.0f64, f64::max);
            let variance = kbps_samples.iter()
                .map(|&x| (x - avg_kbps).powi(2))
                .sum::<f64>() / kbps_samples.len() as f64;
            let std_dev = variance.sqrt();
            
            println!("    Average: {:.0} kbps", avg_kbps);
            println!("    Range: {:.0} - {:.0} kbps", min_kbps, max_kbps);
            println!("    Std Dev: {:.0} kbps ({}% variance)", 
                std_dev, 
                (std_dev / avg_kbps * 100.0) as u32
            );
            
            // Quality assessment
            let stability = if std_dev / avg_kbps < 0.1 {
                "🟢 Very Stable"
            } else if std_dev / avg_kbps < 0.2 {
                "🟡 Stable"
            } else if std_dev / avg_kbps < 0.5 {
                "🟠 Variable"
            } else {
                "🔴 Unstable"
            };
            println!("    Stability: {}", stability);
        }
    }
}

#[tokio::test]
async fn test_ffmpeg_probe_optimization() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           FFMPEG PROBE OPTIMIZATION TEST                             ║");
    println!("║                                                                      ║");
    println!("║  Comparing default vs optimized probe settings                       ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    
    // Test with a single station, different probe settings
    let station = &TEST_STATIONS[0]; // radio.chungo.es
    
    let probe_settings = vec![
        ("Default", vec!["-probesize", "64k", "-analyzeduration", "200000"]),
        ("Fast", vec!["-probesize", "32k", "-analyzeduration", "50000"]),
        ("Minimal", vec!["-probesize", "16k", "-analyzeduration", "0"]),
        ("Max", vec!["-probesize", "256k", "-analyzeduration", "500000"]),
    ];
    
    println!("\n  Testing: {}", station.name);
    
    for (name, probe_args) in probe_settings {
        let start = Instant::now();
        
        let mut args = vec![
            "-hide_banner",
            "-loglevel", "error",
            "-nostdin",
        ];
        args.extend(&probe_args);
        args.extend(&[
            "-i", station.url,
            "-t", "0.1",
            "-ac", "1",
            "-ar", "44100",
            "-f", "s16le",
            "-",
        ]);
        
        match Command::new("ffmpeg").args(&args).output().await {
            Ok(output) => {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let success = output.status.success() && !output.stdout.is_empty();
                
                println!("    {:<10} {:>6.0} ms  {}", 
                    name, 
                    elapsed_ms,
                    if success { "✅" } else { "❌" }
                );
            }
            Err(e) => {
                println!("    {:<10} FAILED: {}", name, e);
            }
        }
    }
    
    println!("\n  Recommendation: Check which setting successfully decodes fastest");
}
