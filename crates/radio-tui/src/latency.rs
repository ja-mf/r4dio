//! Latency measurement and telemetry for audio pipeline debugging.
//!
//! This module provides utilities to measure and detect drift between
//! the audio output (mpv) and visualization (VU meter/scope) paths.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Global telemetry instance for testing/debugging.
/// Use `set_global_telemetry()` to enable, then any component can record.
static GLOBAL_TELEMETRY: OnceLock<PipelineTelemetry> = OnceLock::new();

/// Enable global telemetry (call once at startup for testing).
pub fn enable_global_telemetry() {
    let _ = GLOBAL_TELEMETRY.set(PipelineTelemetry::new());
    info!("Global latency telemetry enabled");
}

/// Get global telemetry if enabled.
pub fn global_telemetry() -> Option<&'static PipelineTelemetry> {
    GLOBAL_TELEMETRY.get()
}

/// Record proxy first byte in global telemetry (no-op if not enabled).
pub fn record_proxy_first_byte() {
    if let Some(t) = global_telemetry() {
        t.record_proxy_first_byte();
    }
}

/// Record ffmpeg first sample in global telemetry (no-op if not enabled).
pub fn record_ffmpeg_first_sample() {
    if let Some(t) = global_telemetry() {
        t.record_ffmpeg_first_sample();
    }
}

/// Record VU first display in global telemetry (no-op if not enabled).
pub fn record_vu_first_display() {
    if let Some(t) = global_telemetry() {
        t.record_vu_first_display();
    }
}

/// Get report from global telemetry.
pub fn global_report() -> Option<LatencyReport> {
    global_telemetry().map(|t| t.report())
}

/// Telemetry collector for pipeline latency measurements.
#[derive(Debug, Clone)]
pub struct PipelineTelemetry {
    /// When proxy received first byte from upstream
    proxy_first_byte: Arc<AtomicU64>,
    /// When ffmpeg decoded first PCM sample
    ffmpeg_first_sample: Arc<AtomicU64>,
    /// When VU meter first displayed audio
    vu_first_display: Arc<AtomicU64>,
    /// Sample counter for drift detection
    samples_processed: Arc<AtomicU64>,
}

impl Default for PipelineTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineTelemetry {
    pub fn new() -> Self {
        Self {
            proxy_first_byte: Arc::new(AtomicU64::new(0)),
            ffmpeg_first_sample: Arc::new(AtomicU64::new(0)),
            vu_first_display: Arc::new(AtomicU64::new(0)),
            samples_processed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Reset all counters (call when starting new playback).
    pub fn reset(&self) {
        self.proxy_first_byte.store(0, Ordering::SeqCst);
        self.ffmpeg_first_sample.store(0, Ordering::SeqCst);
        self.vu_first_display.store(0, Ordering::SeqCst);
        self.samples_processed.store(0, Ordering::SeqCst);
    }

    /// Record that proxy received first byte.
    pub fn record_proxy_first_byte(&self) {
        let now = Instant::now().elapsed().as_micros() as u64;
        self.proxy_first_byte.store(now, Ordering::SeqCst);
    }

    /// Record that ffmpeg decoded first sample.
    pub fn record_ffmpeg_first_sample(&self) {
        let now = Instant::now().elapsed().as_micros() as u64;
        self.ffmpeg_first_sample.store(now, Ordering::SeqCst);
    }

    /// Record that VU displayed first audio.
    pub fn record_vu_first_display(&self) {
        let now = Instant::now().elapsed().as_micros() as u64;
        self.vu_first_display.store(now, Ordering::SeqCst);
    }

    /// Increment sample counter.
    pub fn increment_samples(&self, count: u64) {
        self.samples_processed.fetch_add(count, Ordering::Relaxed);
    }

    /// Get current latency report.
    pub fn report(&self) -> LatencyReport {
        let proxy = self.proxy_first_byte.load(Ordering::SeqCst);
        let ffmpeg = self.ffmpeg_first_sample.load(Ordering::SeqCst);
        let vu = self.vu_first_display.load(Ordering::SeqCst);
        let samples = self.samples_processed.load(Ordering::Relaxed);

        LatencyReport {
            proxy_to_ffmpeg_us: if proxy > 0 && ffmpeg > 0 {
                Some(ffmpeg - proxy)
            } else {
                None
            },
            proxy_to_vu_us: if proxy > 0 && vu > 0 {
                Some(vu - proxy)
            } else {
                None
            },
            ffmpeg_to_vu_us: if ffmpeg > 0 && vu > 0 {
                Some(vu - ffmpeg)
            } else {
                None
            },
            total_samples: samples,
        }
    }
}

/// Latency measurements in microseconds.
#[derive(Debug, Clone, Copy)]
pub struct LatencyReport {
    /// Time from proxy receiving data to ffmpeg decoding it.
    pub proxy_to_ffmpeg_us: Option<u64>,
    /// Time from proxy receiving data to VU displaying it.
    pub proxy_to_vu_us: Option<u64>,
    /// Time from ffmpeg decoding to VU displaying.
    pub ffmpeg_to_vu_us: Option<u64>,
    /// Total samples processed through PCM path.
    pub total_samples: u64,
}

impl LatencyReport {
    /// Format for human reading.
    pub fn format(&self) -> String {
        format!(
            "Latency: proxy→ffmpeg={:?}, proxy→VU={:?}, ffmpeg→VU={:?}, samples={}",
            self.proxy_to_ffmpeg_us.map(|us| format!("{}us", us)),
            self.proxy_to_vu_us.map(|us| format!("{}us", us)),
            self.ffmpeg_to_vu_us.map(|us| format!("{}us", us)),
            self.total_samples
        )
    }
}

/// Monitors broadcast channel for lag detection.
pub struct BroadcastLagMonitor {
    telemetry: PipelineTelemetry,
    last_lag: Arc<AtomicU64>,
    lag_count: Arc<AtomicU64>,
}

impl BroadcastLagMonitor {
    pub fn new(telemetry: PipelineTelemetry) -> Self {
        Self {
            telemetry,
            last_lag: Arc::new(AtomicU64::new(0)),
            lag_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record a Lagged error from broadcast receiver.
    pub fn record_lag(&self, skipped: u64) {
        self.last_lag.store(skipped, Ordering::SeqCst);
        self.lag_count.fetch_add(1, Ordering::SeqCst);
        warn!("Broadcast lag detected: skipped {} chunks", skipped);
    }

    /// Get lag statistics.
    pub fn lag_stats(&self) -> LagStats {
        LagStats {
            last_lag_chunks: self.last_lag.load(Ordering::SeqCst),
            total_lag_events: self.lag_count.load(Ordering::SeqCst),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LagStats {
    pub last_lag_chunks: u64,
    pub total_lag_events: u64,
}

/// Test signal generator for latency calibration.
pub struct TestSignal {
    sample_rate: u32,
    frequency: f32,
    amplitude: f32,
}

impl TestSignal {
    /// Create a 1kHz sine wave test tone.
    pub fn sine_1khz() -> Self {
        Self {
            sample_rate: 44100,
            frequency: 1000.0,
            amplitude: 0.5,
        }
    }

    /// Generate samples for given duration.
    pub fn generate(&self, duration_ms: u64) -> Vec<i16> {
        let num_samples = (self.sample_rate as u64 * duration_ms / 1000) as usize;
        let mut samples = Vec::with_capacity(num_samples);
        
        for i in 0..num_samples {
            let t = i as f32 / self.sample_rate as f32;
            let value = (t * self.frequency * 2.0 * std::f32::consts::PI).sin();
            samples.push((value * self.amplitude * 32767.0) as i16);
        }
        
        samples
    }

    /// Generate impulse (single peak) for precise timing measurement.
    pub fn impulse() -> Vec<i16> {
        let mut samples = vec![0i16; 44100]; // 1 second of silence
        samples[0] = 32767; // Single peak at start
        samples
    }
}

/// Measures round-trip latency through a channel.
pub async fn measure_broadcast_latency<T: Clone + Send + 'static>(
    tx: &broadcast::Sender<T>,
    mut rx: broadcast::Receiver<T>,
) -> Option<Duration> {
    let start = Instant::now();
    let test_value = unsafe { std::mem::zeroed::<T>() };
    
    // Send test value
    if tx.send(test_value).is_err() {
        return None;
    }
    
    // Wait for receive with timeout
    match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
        Ok(Ok(_)) => Some(start.elapsed()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_report_formatting() {
        let report = LatencyReport {
            proxy_to_ffmpeg_us: Some(5000),
            proxy_to_vu_us: Some(15000),
            ffmpeg_to_vu_us: Some(10000),
            total_samples: 44100,
        };
        
        let formatted = report.format();
        assert!(formatted.contains("5000us"));
        assert!(formatted.contains("15000us"));
        assert!(formatted.contains("44100"));
    }

    #[test]
    fn test_telemetry_reset() {
        let telemetry = PipelineTelemetry::new();
        telemetry.record_proxy_first_byte();
        telemetry.increment_samples(100);
        
        telemetry.reset();
        
        let report = telemetry.report();
        assert_eq!(report.total_samples, 0);
    }

    #[test]
    fn test_test_signal_generation() {
        let signal = TestSignal::sine_1khz();
        let samples = signal.generate(100); // 100ms
        
        // 44100 Hz * 0.1s = 4410 samples
        assert_eq!(samples.len(), 4410);
        
        // Check amplitude is reasonable
        let max = samples.iter().map(|s| s.abs()).max().unwrap();
        assert!(max > 1000); // Should have significant amplitude
    }

    #[test]
    fn test_impulse_signal() {
        let impulse = TestSignal::impulse();
        assert_eq!(impulse[0], 32767);
        assert_eq!(impulse[1], 0);
        assert_eq!(impulse.len(), 44100);
    }
}
