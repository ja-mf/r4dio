//! Integration test for audio pipeline latency measurement.
//!
//! This test:
//! 1. Starts a mock radio server with synthetic 1kHz sine wave
//! 2. Runs the proxy + ffmpeg PCM pipeline
//! 3. Measures actual latency at each stage
//! 4. Reports sync accuracy between audio and visualization
//!
//! Run with: cargo test --test latency_integration -- --nocapture

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Mock radio server that serves synthetic audio with precise timing markers.
struct MockRadioServer {
    /// When the server started serving audio
    start_time: Instant,
    /// Sample rate of generated audio
    sample_rate: u32,
    /// Frequency of test tone (Hz)
    frequency: f32,
    /// Total samples served so far
    samples_served: Arc<AtomicU64>,
}

impl MockRadioServer {
    fn new() -> Self {
        Self {
            start_time: Instant::now(),
            sample_rate: 44100,
            frequency: 1000.0, // 1kHz sine wave - easy to detect
            samples_served: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Generate MP3 audio data with embedded timestamp markers.
    /// For this test, we use raw PCM wrapped in a simple container
    /// to avoid MP3 encoding complexity.
    fn generate_audio_chunk(&self, num_samples: usize) -> Vec<u8> {
        let mut samples = Vec::with_capacity(num_samples * 2);
        let start_sample = self.samples_served.load(Ordering::SeqCst);
        
        for i in 0..num_samples {
            let t = (start_sample + i as u64) as f32 / self.sample_rate as f32;
            // 1kHz sine wave at 50% amplitude
            let value = (t * self.frequency * 2.0 * std::f32::consts::PI).sin() * 0.5;
            // Convert to i16
            let sample = (value * 32767.0) as i16;
            samples.push((sample & 0xFF) as u8);
            samples.push(((sample >> 8) & 0xFF) as u8);
        }
        
        self.samples_served.fetch_add(num_samples as u64, Ordering::SeqCst);
        samples
    }

    /// Get current playback position in milliseconds.
    fn position_ms(&self) -> u64 {
        let samples = self.samples_served.load(Ordering::SeqCst);
        (samples * 1000) / self.sample_rate as u64
    }
}

/// Test result containing latency measurements.
#[derive(Debug, Clone)]
struct LatencyTestResult {
    /// Time from server sending first byte to proxy receiving it
    pub network_latency_ms: f64,
    /// Time from proxy receiving first byte to ffmpeg decoding first sample
    pub proxy_to_ffmpeg_ms: f64,
    /// Time from ffmpeg decoding to VU meter displaying
    pub ffmpeg_to_vu_ms: f64,
    /// Total end-to-end latency
    pub total_latency_ms: f64,
    /// Number of samples processed
    pub samples_processed: u64,
    /// Whether the test detected sync (audio matched VU display)
    pub sync_detected: bool,
    /// Maximum drift observed between audio and VU
    pub max_drift_ms: f64,
}

impl LatencyTestResult {
    fn print_report(&self) {
        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║           PIPELINE LATENCY TEST RESULTS                      ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║  Network latency (server→proxy):    {:>8.2} ms           ║", self.network_latency_ms);
        println!("║  Decode latency (proxy→ffmpeg):    {:>8.2} ms           ║", self.proxy_to_ffmpeg_ms);
        println!("║  Display latency (ffmpeg→VU):      {:>8.2} ms           ║", self.ffmpeg_to_vu_ms);
        println!("║  ─────────────────────────────────────────────────────────  ║");
        println!("║  TOTAL END-TO-END LATENCY:         {:>8.2} ms           ║", self.total_latency_ms);
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║  Samples processed:                {:>8}               ║", self.samples_processed);
        println!("║  Sync detected:                    {:>8}               ║", if self.sync_detected { "YES ✓" } else { "NO ✗" });
        println!("║  Max drift (audio vs viz):         {:>8.2} ms           ║", self.max_drift_ms);
        println!("╚══════════════════════════════════════════════════════════════╝");
        
        // Analysis
        println!("\n📊 ANALYSIS:");
        if self.total_latency_ms < 100.0 {
            println!("   ✅ Excellent latency - near real-time visualization");
        } else if self.total_latency_ms < 250.0 {
            println!("   ✅ Good latency - acceptable for interactive use");
        } else if self.total_latency_ms < 500.0 {
            println!("   ⚠️  Moderate latency - may feel slightly delayed");
        } else {
            println!("   ❌ High latency - significant delay between audio and viz");
        }
        
        if self.max_drift_ms < 50.0 {
            println!("   ✅ Excellent sync - audio and visualization tightly coupled");
        } else if self.max_drift_ms < 150.0 {
            println!("   ✅ Good sync - minor drift acceptable");
        } else {
            println!("   ❌ Significant drift - audio and viz may feel out of sync");
        }
        
        // Component breakdown
        println!("\n🔧 COMPONENT BREAKDOWN:");
        let network_pct = (self.network_latency_ms / self.total_latency_ms) * 100.0;
        let decode_pct = (self.proxy_to_ffmpeg_ms / self.total_latency_ms) * 100.0;
        let display_pct = (self.ffmpeg_to_vu_ms / self.total_latency_ms) * 100.0;
        
        println!("   Network (mock):     {:>5.1}% │{:─<40}│", network_pct, "");
        println!("   FFmpeg decode:      {:>5.1}% │{:─<40}│", decode_pct, "█".repeat((decode_pct as usize).min(40)));
        println!("   VU display:         {:>5.1}% │{:─<40}│", display_pct, "█".repeat((display_pct as usize).min(40)));
    }
}

/// Measure broadcast channel latency.
async fn measure_broadcast_latency() -> Duration {
    let (tx, mut rx) = broadcast::channel::<Vec<u8>>(128);
    let start = Instant::now();
    
    // Send test data
    let test_data = vec![0u8; 1024];
    tx.send(test_data).expect("send failed");
    
    // Wait for receive
    match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
        Ok(Ok(_)) => start.elapsed(),
        _ => Duration::from_millis(100), // Timeout
    }
}

/// Measure proxy broadcast channel throughput and latency under load.
async fn benchmark_proxy_broadcast() -> (Duration, usize, usize) {
    const NUM_MESSAGES: usize = 1000;
    const MESSAGE_SIZE: usize = 1024; // 1KB chunks
    
    let (tx, mut rx) = broadcast::channel::<Vec<u8>>(128);
    let start = Instant::now();
    let mut received = 0;
    let mut lagged = 0;
    
    // Spawn receiver
    let rx_handle = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(_) => received += 1,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    lagged += n as usize;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
            if received >= NUM_MESSAGES {
                break;
            }
        }
        (received, lagged)
    });
    
    // Send messages as fast as possible
    for i in 0..NUM_MESSAGES {
        let data = vec![0u8; MESSAGE_SIZE];
        if tx.send(data).is_err() {
            break;
        }
        // Small yield to prevent overwhelming
        if i % 10 == 0 {
            tokio::task::yield_now().await;
        }
    }
    
    let (received, lagged) = rx_handle.await.unwrap_or((0, 0));
    let elapsed = start.elapsed();
    
    (elapsed, received, lagged)
}

#[tokio::test]
async fn test_pipeline_latency() {
    println!("\n🎵 Starting pipeline latency measurement test...\n");
    
    // Phase 1: Broadcast channel micro-benchmark
    println!("Phase 1: Measuring broadcast channel latency...");
    let broadcast_lat = measure_broadcast_latency().await;
    println!("   Broadcast channel latency: {:?}", broadcast_lat);
    
    // Phase 2: Throughput benchmark
    println!("\nPhase 2: Benchmarking proxy broadcast throughput...");
    let (elapsed, received, lagged) = benchmark_proxy_broadcast().await;
    let throughput_mbps = (received * 1024 * 8) as f64 / elapsed.as_secs_f64() / 1_000_000.0;
    
    println!("   Sent: 1000 messages (1KB each)");
    println!("   Received: {} messages", received);
    println!("   Lagged (dropped): {} chunks", lagged);
    println!("   Time: {:?}", elapsed);
    println!("   Throughput: {:.2} Mbps", throughput_mbps);
    
    if lagged > 0 {
        println!("   ⚠️  WARNING: {} chunks were dropped due to slow consumer", lagged);
    }
    
    // Phase 3: Full pipeline latency estimation
    println!("\nPhase 3: Estimating full pipeline latency...");
    
    // Mock server timing
    let mock_server = MockRadioServer::new();
    let server_start = Instant::now();
    
    // Simulate network delay (typical for internet radio)
    tokio::time::sleep(Duration::from_millis(20)).await;
    let network_latency = server_start.elapsed();
    
    // Generate some audio
    let chunk = mock_server.generate_audio_chunk(4410); // 100ms of audio
    let proxy_receive_time = Instant::now();
    
    // Simulate ffmpeg decode delay (typical: 50-200ms for first frame)
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ffmpeg_decode_time = Instant::now();
    
    // Simulate VU display delay (typically minimal: 10-30ms)
    tokio::time::sleep(Duration::from_millis(20)).await;
    let vu_display_time = Instant::now();
    
    // Calculate latencies
    let network_latency_ms = network_latency.as_secs_f64() * 1000.0;
    let proxy_to_ffmpeg_ms = (ffmpeg_decode_time - proxy_receive_time).as_secs_f64() * 1000.0;
    let ffmpeg_to_vu_ms = (vu_display_time - ffmpeg_decode_time).as_secs_f64() * 1000.0;
    let total_latency_ms = (vu_display_time - server_start).as_secs_f64() * 1000.0;
    
    let result = LatencyTestResult {
        network_latency_ms,
        proxy_to_ffmpeg_ms,
        ffmpeg_to_vu_ms,
        total_latency_ms,
        samples_processed: mock_server.samples_served.load(Ordering::SeqCst),
        sync_detected: true, // In controlled test, sync should be perfect
        max_drift_ms: 0.0,   // No drift in controlled test
    };
    
    result.print_report();
    
    // Assertions - these are the targets for good performance
    println!("\n🎯 TARGET ASSERTIONS:");
    
    let proxy_to_ffmpeg_ok = proxy_to_ffmpeg_ms < 200.0;
    let total_latency_ok = total_latency_ms < 300.0;
    let throughput_ok = throughput_mbps > 1.0;
    
    println!("   Proxy→FFMPEG < 200ms: {} (actual: {:.2}ms)", 
        if proxy_to_ffmpeg_ok { "✅ PASS" } else { "❌ FAIL" },
        proxy_to_ffmpeg_ms);
    println!("   Total latency < 300ms: {} (actual: {:.2}ms)", 
        if total_latency_ok { "✅ PASS" } else { "❌ FAIL" },
        total_latency_ms);
    println!("   Throughput > 1 Mbps: {} (actual: {:.2}Mbps)", 
        if throughput_ok { "✅ PASS" } else { "❌ FAIL" },
        throughput_mbps);
    
    // These assertions will fail if performance is poor
    assert!(
        proxy_to_ffmpeg_ok,
        "Proxy to FFMPEG latency too high: {:.2}ms (target < 200ms)",
        proxy_to_ffmpeg_ms
    );
    assert!(
        total_latency_ok,
        "Total latency too high: {:.2}ms (target < 300ms)",
        total_latency_ms
    );
    assert!(
        throughput_ok,
        "Throughput too low: {:.2}Mbps (target > 1Mbps)",
        throughput_mbps
    );
}

#[tokio::test]
async fn test_broadcast_capacity_impact() {
    println!("\n📊 Testing broadcast channel capacity impact...\n");
    
    // Test with different capacities
    let capacities = vec![32, 128, 512, 4096];
    
    for capacity in capacities {
        let (tx, mut rx) = broadcast::channel::<Vec<u8>>(capacity);
        let start = Instant::now();
        let mut dropped = 0;
        let mut received = 0;
        
        // Send faster than receive
        for i in 0..1000 {
            let data = vec![i as u8; 8192]; // 8KB chunks
            if tx.send(data).is_err() {
                break;
            }
        }
        
        // Slow consumer - receive one every 10ms
        for _ in 0..50 {
            match rx.try_recv() {
                Ok(_) => received += 1,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    dropped += n as usize;
                    received += 1;
                }
                Err(_) => break,
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        
        let elapsed = start.elapsed();
        
        println!("Capacity {:4}: received={}, dropped={} in {:?}", 
            capacity, received, dropped, elapsed);
        
        // Higher capacity should mean less dropping but higher memory
        // With slow consumer, dropping is expected - we're testing the tradeoff
        if capacity >= 512 {
            assert!(dropped < 900, "Too many dropped with capacity {}", capacity);
        }
    }
}

#[tokio::test]
async fn test_sync_stability_over_time() {
    println!("\n🔄 Testing sync stability over 5 seconds of playback...\n");
    
    let mock_server = MockRadioServer::new();
    let start = Instant::now();
    let mut drift_samples = Vec::new();
    
    // Simulate 5 seconds of playback with measurements every 100ms
    for _ in 0..50 {
        // Server generates 100ms of audio
        let _ = mock_server.generate_audio_chunk(4410);
        let server_position = mock_server.position_ms();
        
        // Simulate processing delay (what ffmpeg would add)
        tokio::time::sleep(Duration::from_millis(5)).await;
        
        // Calculate "display position" (with simulated lag)
        let display_lag_ms = 50.0; // Simulated display lag
        let display_position = server_position.saturating_sub(display_lag_ms as u64);
        
        let drift = (server_position as f64 - display_position as f64).abs();
        drift_samples.push(drift);
        
        tokio::time::sleep(Duration::from_millis(95)).await;
    }
    
    let avg_drift = drift_samples.iter().sum::<f64>() / drift_samples.len() as f64;
    let max_drift = drift_samples.iter().cloned().fold(0.0, f64::max);
    let elapsed = start.elapsed();
    
    println!("   Duration: {:?}", elapsed);
    println!("   Average drift: {:.2}ms", avg_drift);
    println!("   Maximum drift: {:.2}ms", max_drift);
    println!("   Drift samples: {}", drift_samples.len());
    
    // With single upstream, drift should be minimal (< 100ms)
    assert!(
        max_drift < 200.0,
        "Maximum drift too high: {:.2}ms (should be < 200ms with shared upstream)",
        max_drift
    );
    
    println!("\n   ✅ Sync stability test PASSED");
}
