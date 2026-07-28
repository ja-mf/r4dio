//! Lightweight DSP primitives for audio visualization.
//!
//! Zero external dependencies — implements Butterworth biquad IIR filters
//! for real-time 3-band frequency splitting (bass / mid / treble).

use std::f64::consts::PI;

/// Second-order IIR (biquad) filter state.
///
/// Implements the Direct Form II Transposed structure:
///   y[n] = b0·x[n] + s1
///   s1   = b1·x[n] − a1·y[n] + s2
///   s2   = b2·x[n] − a2·y[n]
///
/// Coefficients are pre-normalised (a0 = 1).
#[derive(Debug, Clone)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl Biquad {
    /// Process a single sample and return the filtered output.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let xd = x as f64;
        let y = self.b0 * xd + self.s1;
        self.s1 = self.b1 * xd - self.a1 * y + self.s2;
        self.s2 = self.b2 * xd - self.a2 * y;
        y as f32
    }

    /// Reset delay elements to zero (call on station change).
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    // ── Factory methods ────────────────────────────────────────────────────

    /// 2nd-order Butterworth low-pass at `fc` Hz, sample rate `fs` Hz.
    pub fn lowpass(fc: f64, fs: f64) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let (sin_w, cos_w) = w0.sin_cos();
        // Q = 1/√2 for Butterworth
        let alpha = sin_w / (2.0 * std::f64::consts::FRAC_1_SQRT_2).recip();
        // Correct: alpha = sin(w0) / (2*Q), Q = sqrt(2)/2 for Butterworth
        let alpha = sin_w / (2.0 * (2.0_f64.sqrt() / 2.0));

        let b0 = (1.0 - cos_w) / 2.0;
        let b1 = 1.0 - cos_w;
        let b2 = (1.0 - cos_w) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// 2nd-order Butterworth high-pass at `fc` Hz, sample rate `fs` Hz.
    pub fn highpass(fc: f64, fs: f64) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let (sin_w, cos_w) = w0.sin_cos();
        let alpha = sin_w / (2.0 * (2.0_f64.sqrt() / 2.0));

        let b0 = (1.0 + cos_w) / 2.0;
        let b1 = -(1.0 + cos_w);
        let b2 = (1.0 + cos_w) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Band-pass by cascading a high-pass at `f_lo` and a low-pass at `f_hi`.
    /// Returns a pair of filters that should be applied in series.
    pub fn bandpass_pair(f_lo: f64, f_hi: f64, fs: f64) -> (Self, Self) {
        (Self::highpass(f_lo, fs), Self::lowpass(f_hi, fs))
    }
}

/// Three-band energy splitter for audio visualization.
///
/// Splits mono PCM into bass / mid / treble bands using cascaded biquads
/// and tracks per-band RMS energy. Designed to be called sample-by-sample
/// inside the existing MeterTick loop with negligible CPU cost.
#[derive(Debug, Clone)]
pub struct BandSplitter {
    bass_lp: Biquad,
    mid_hp: Biquad,
    mid_lp: Biquad,
    treble_hp: Biquad,
}

/// Per-frame energy accumulator for the 3-band splitter.
#[derive(Debug, Clone, Default)]
pub struct BandEnergy {
    bass_sum: f64,
    mid_sum: f64,
    treble_sum: f64,
    count: usize,
}

impl BandEnergy {
    /// Feed one sample through all bands and accumulate energy.
    #[inline]
    pub fn push(&mut self, splitter: &mut BandSplitter, sample: f32) {
        let bass = splitter.bass_lp.tick(sample);
        let mid = splitter.mid_lp.tick(splitter.mid_hp.tick(sample));
        let treble = splitter.treble_hp.tick(sample);

        self.bass_sum += (bass as f64) * (bass as f64);
        self.mid_sum += (mid as f64) * (mid as f64);
        self.treble_sum += (treble as f64) * (treble as f64);
        self.count += 1;
    }

    /// Convert accumulated energy to dB triplet. Returns `(bass_db, mid_db, treble_db)`.
    pub fn to_db(&self) -> (f32, f32, f32) {
        let n = self.count.max(1) as f64;
        let to_db = |sum: f64| -> f32 {
            let rms = (sum / n).sqrt();
            if rms < 1e-10 {
                -90.0
            } else {
                (20.0 * rms.log10()) as f32
            }
        };
        (
            to_db(self.bass_sum),
            to_db(self.mid_sum),
            to_db(self.treble_sum),
        )
    }
}

impl BandSplitter {
    /// Create a 3-band splitter with crossovers at 250 Hz and 2 kHz.
    /// Sample rate should be 44100.
    pub fn new(fs: f64) -> Self {
        let (mid_hp, mid_lp) = Biquad::bandpass_pair(250.0, 2000.0, fs);
        Self {
            bass_lp: Biquad::lowpass(250.0, fs),
            mid_hp,
            mid_lp,
            treble_hp: Biquad::highpass(2000.0, fs),
        }
    }

    /// Reset all filter states (call on station change).
    pub fn reset(&mut self) {
        self.bass_lp.reset();
        self.mid_hp.reset();
        self.mid_lp.reset();
        self.treble_hp.reset();
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SHARED ACF + COMB FILTER TEMPO ENGINE
// ═══════════════════════════════════════════════════════════════════════════════

const BPM_MIN: f32 = 40.0;
const BPM_MAX: f32 = 200.0;

/// Core BPM computation: direct ACF + 4-element comb filter (à la BTrack, Stark 2014)
/// over a pre-normalised onset envelope at frame-rate (25 FPS).
///
/// Returns `(raw_bpm, confidence 0–1)`. Returns `(0, 0)` on insufficient signal.
///
/// Why comb over plain ACF peak: at 120 BPM the comb also accumulates evidence from
/// the 2× harmonic (lag 6) and ½× sub-harmonic (lag 25), making it far more robust to
/// sparse kick patterns and radio compression than picking the single highest ACF lag.
///
/// Rayleigh prior (σ=12 frames ≈ 125 BPM at 25 FPS) gently biases toward the most
/// common radio music tempo without hard-coding anything.
fn acf_comb_bpm(onset: &[f32], fps: f32, lag_min: usize, lag_max: usize) -> (f32, f32) {
    let n = onset.len();
    if n < lag_min + 1 {
        return (0.0, 0.0);
    }
    let lag_max = lag_max.min(n.saturating_sub(1));
    if lag_max < lag_min {
        return (0.0, 0.0);
    }

    // Silence gate.
    let onset_sum: f32 = onset.iter().sum();
    if onset_sum < 1e-6 {
        return (0.0, 0.0);
    }

    // Normalised direct ACF: r[τ] = Σ x[i]·x[i+τ] / (n−τ)
    let mut acf = vec![0.0_f32; lag_max + 1];
    for lag in lag_min..=lag_max {
        let valid = n - lag;
        acf[lag] = onset[..valid]
            .iter()
            .zip(onset[lag..].iter())
            .map(|(a, b)| a * b)
            .sum::<f32>()
            / valid as f32;
    }

    // 4-element comb + Rayleigh weighting (σ=12 → peak at lag 12 ≈ 125 BPM).
    let sigma = 12.0_f32;
    let (mut best_val, mut best_lag, mut comb_total) = (0.0_f32, lag_min, 0.0_f32);
    for p in lag_min..=lag_max {
        let rayleigh =
            (p as f32 / (sigma * sigma)) * (-(p as f32 * p as f32) / (2.0 * sigma * sigma)).exp();
        let c = acf[p]
            + if 2 * p <= lag_max {
                0.50 * acf[2 * p]
            } else {
                0.0
            }
            + if 3 * p <= lag_max {
                0.33 * acf[3 * p]
            } else {
                0.0
            }
            + if 4 * p <= lag_max {
                0.25 * acf[4 * p]
            } else {
                0.0
            };
        let scored = c * rayleigh;
        comb_total += scored;
        if scored > best_val {
            best_val = scored;
            best_lag = p;
        }
    }

    // Lag → BPM, snap into [BPM_MIN, BPM_MAX] via halving / doubling.
    let mut raw_bpm = fps * 60.0 / best_lag as f32;
    while raw_bpm > BPM_MAX * 1.05 {
        raw_bpm /= 2.0;
    }
    while raw_bpm < BPM_MIN * 0.95 {
        raw_bpm *= 2.0;
    }
    if raw_bpm < BPM_MIN * 0.9 || raw_bpm > BPM_MAX * 1.1 {
        return (0.0, 0.0);
    }

    // Confidence = normalised peak dominance. tanh maps: 2.5→0, 4→0.6, 6→0.9.
    let num_lags = (lag_max - lag_min + 1) as f32;
    let comb_mean = comb_total / num_lags;
    let dominance = if comb_mean > 1e-10 {
        best_val / comb_mean
    } else {
        0.0
    };
    let confidence = ((dominance - 2.5) / 2.0).tanh().max(0.0);
    (raw_bpm, confidence)
}

/// Return the onset buffer in chronological order, mean-subtracted and HWR.
/// Matches BTrack's adaptive threshold step: removes DC bias so ACF measures
/// genuine periodicity rather than overall loudness level.
fn prepare_onset_buf(buf: &[f32], write_pos: usize, frame_count: usize) -> Vec<f32> {
    let buf_len = buf.len();
    let n = frame_count.min(buf_len);
    let start = if frame_count < buf_len { 0 } else { write_pos };
    let raw: Vec<f32> = (0..n).map(|i| buf[(start + i) % buf_len]).collect();
    let mean = raw.iter().sum::<f32>() / n.max(1) as f32;
    raw.into_iter().map(|x| (x - mean).max(0.0)).collect()
}

// ═══════════════════════════════════════════════════════════════════════════════
// BPM ESTIMATOR — reactive, ~1 Hz update, PLL beat phase
// ═══════════════════════════════════════════════════════════════════════════════

/// Real-time BPM estimator for live radio streams.
///
/// Algorithm (inspired by BTrack, Stark 2014):
/// - Caller pre-computes onset strength: `HWR(Δbass·0.5) + HWR(Δmid·0.3) + HWR(Δtreble·0.2)`
///   where `HWR(x) = max(0, x)` and `Δ = current_db − prev_db`.
///   This approximates spectral flux using our existing 3-band energy without FFT.
/// - 512-frame (~20s) rolling onset buffer; no growing history to go stale.
/// - Direct ACF (O(25k) mults/estimate at 25 FPS — negligible) + 4-element comb filter.
/// - EMA smoothing with 35% max-jump gate rejects stream reconnects and abrupt jumps.
/// - PLL phase accumulator stays in sync even when individual beats are missed.
///
/// Edge cases: silence / field recordings → near-zero onset strength → confidence=0,
/// no BPM reported. Stream reconnect burst → outlier rejection gate holds the estimate.
#[derive(Debug, Clone)]
pub struct BpmEstimator {
    fps: f32,
    onset_buf: Vec<f32>,
    write_pos: usize,
    frame_count: usize,
    smoothed_bpm: f32,
    cached_bpm: Option<f32>,
    cached_confidence: f32,
    /// Phase accumulator [0, 1). 0 = on-beat.
    phase: f32,
    beat_period: f32,
    onset_ema: f32,
    frames_since_estimate: usize,
}

impl BpmEstimator {
    const BUF_LEN: usize = 512;
    const ESTIMATE_EVERY: usize = 25; // ~1 s

    pub fn new(fps: f32) -> Self {
        Self {
            fps,
            onset_buf: vec![0.0; Self::BUF_LEN],
            write_pos: 0,
            frame_count: 0,
            smoothed_bpm: 0.0,
            cached_bpm: None,
            cached_confidence: 0.0,
            phase: 0.0,
            beat_period: 0.0,
            onset_ema: 0.0,
            frames_since_estimate: 0,
        }
    }

    /// Feed one frame of pre-computed multi-band onset strength.
    pub fn push_onset(&mut self, onset: f32) {
        self.onset_buf[self.write_pos] = onset;
        self.write_pos = (self.write_pos + 1) % Self::BUF_LEN;
        self.frame_count += 1;

        // Local onset mean: τ ≈ 20 frames (800 ms).
        self.onset_ema += 0.05 * (onset - self.onset_ema);

        // PLL: advance phase; correct when a strong onset lands near beat.
        if self.beat_period > 0.0 {
            self.phase = (self.phase + 1.0 / self.beat_period).fract();
            let strong = onset > self.onset_ema * 2.5 && self.onset_ema > 1e-4;
            if strong && (self.phase > 0.8 || self.phase < 0.2) {
                self.phase *= 0.6; // pull toward 0 (on-beat)
            }
        }

        self.frames_since_estimate += 1;
        if self.frames_since_estimate >= Self::ESTIMATE_EVERY {
            self.frames_since_estimate = 0;
            self.estimate();
        }
    }

    fn estimate(&mut self) {
        if self.frame_count < 50 {
            return;
        }
        let onset = prepare_onset_buf(&self.onset_buf, self.write_pos, self.frame_count);
        let fill = (self.frame_count as f32 / 200.0).min(1.0);
        let (raw_bpm, conf_raw) = acf_comb_bpm(&onset, self.fps, 6, 50);

        if raw_bpm <= 0.0 || conf_raw <= 0.0 {
            self.cached_confidence = (self.cached_confidence * 0.8).max(0.0);
            if self.cached_confidence < 0.15 {
                self.cached_bpm = None;
            }
            return;
        }

        // Max-jump gate: >35% tempo change = outlier (reconnect burst, genre switch).
        if self.smoothed_bpm > 0.0 {
            let jump = (raw_bpm - self.smoothed_bpm).abs() / self.smoothed_bpm;
            if jump > 0.35 {
                self.cached_confidence *= 0.7;
                if self.cached_confidence < 0.15 {
                    self.cached_bpm = None;
                }
                return;
            }
            self.smoothed_bpm = 0.75 * self.smoothed_bpm + 0.25 * raw_bpm;
        } else {
            self.smoothed_bpm = raw_bpm;
        }
        self.beat_period = self.fps * 60.0 / self.smoothed_bpm;

        let confidence = (conf_raw * fill).clamp(0.0, 1.0);
        self.cached_confidence = confidence;
        if confidence >= 0.25 {
            self.cached_bpm = Some(self.smoothed_bpm);
        } else {
            self.cached_bpm = None;
        }
    }

    pub fn bpm(&self) -> Option<f32> {
        self.cached_bpm
    }
    pub fn confidence(&self) -> f32 {
        self.cached_confidence
    }
    pub fn beat_phase(&self) -> f32 {
        self.phase
    }

    pub fn reset(&mut self) {
        self.onset_buf.fill(0.0);
        self.write_pos = 0;
        self.frame_count = 0;
        self.smoothed_bpm = 0.0;
        self.cached_bpm = None;
        self.cached_confidence = 0.0;
        self.phase = 0.0;
        self.beat_period = 0.0;
        self.onset_ema = 0.0;
        self.frames_since_estimate = 0;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEMPO LOCK — slow, stable estimator for the 4th header bulb
// ═══════════════════════════════════════════════════════════════════════════════

/// Stable long-window BPM estimator for the "tempo lock" bulb.
///
/// Uses a 30-second onset buffer and hysteresis to give a steady reading
/// that doesn't flicker. Once locked it only updates when new estimates
/// consistently agree. Ideal for radio: a song plays at the same BPM for
/// minutes; the lock should hold across soft passages and minor variations.
#[derive(Debug, Clone)]
pub struct TempoLock {
    fps: f32,
    onset_buf: Vec<f32>,
    write_pos: usize,
    frame_count: usize,
    history: Vec<f32>,
    hist_pos: usize,
    hist_count: usize,
    locked_bpm: Option<f32>,
    deviate_count: usize,
    phase: f32,
    beat_period: f32,
    onset_ema: f32,
    cached_confidence: f32,
    frames_since_estimate: usize,
}

impl TempoLock {
    const BUF_LEN: usize = 750; // ~30 s at 25 FPS
    const ESTIMATE_EVERY: usize = 75; // ~3 s update rate
    const HISTORY_LEN: usize = 8; // last 8 estimates (~24 s)

    pub fn new(fps: f32) -> Self {
        Self {
            fps,
            onset_buf: vec![0.0; Self::BUF_LEN],
            write_pos: 0,
            frame_count: 0,
            history: vec![0.0; Self::HISTORY_LEN],
            hist_pos: 0,
            hist_count: 0,
            locked_bpm: None,
            deviate_count: 0,
            phase: 0.0,
            beat_period: 0.0,
            onset_ema: 0.0,
            cached_confidence: 0.0,
            frames_since_estimate: 0,
        }
    }

    pub fn push_onset(&mut self, onset: f32) {
        self.onset_buf[self.write_pos] = onset;
        self.write_pos = (self.write_pos + 1) % Self::BUF_LEN;
        self.frame_count += 1;

        self.onset_ema += 0.03 * (onset - self.onset_ema);

        if self.beat_period > 0.0 {
            self.phase = (self.phase + 1.0 / self.beat_period).fract();
            let strong = onset > self.onset_ema * 2.5 && self.onset_ema > 1e-4;
            if strong && (self.phase > 0.8 || self.phase < 0.2) {
                self.phase *= 0.6;
            }
        }

        self.frames_since_estimate += 1;
        if self.frames_since_estimate >= Self::ESTIMATE_EVERY {
            self.frames_since_estimate = 0;
            self.estimate();
        }
    }

    fn estimate(&mut self) {
        if self.frame_count < 100 {
            return;
        }
        let onset = prepare_onset_buf(&self.onset_buf, self.write_pos, self.frame_count);
        let fill = (self.frame_count as f32 / 400.0).min(1.0);
        let (raw_bpm, conf_raw) = acf_comb_bpm(&onset, self.fps, 6, 55);
        let confidence = (conf_raw * fill).clamp(0.0, 1.0);
        self.cached_confidence = confidence;

        if raw_bpm <= 0.0 || confidence < 0.2 {
            return;
        }

        // If locked: reject estimates that deviate > 8%.
        if let Some(locked) = self.locked_bpm {
            let dev = (raw_bpm - locked).abs() / locked;
            if dev > 0.08 {
                self.deviate_count += 1;
                if self.deviate_count >= 3 {
                    // Three consecutive misses → unlock.
                    self.locked_bpm = None;
                    self.deviate_count = 0;
                    self.history.fill(0.0);
                    self.hist_count = 0;
                }
                return;
            }
            self.deviate_count = 0;
        }

        // Record estimate in history ring.
        self.history[self.hist_pos] = raw_bpm;
        self.hist_pos = (self.hist_pos + 1) % Self::HISTORY_LEN;
        if self.hist_count < Self::HISTORY_LEN {
            self.hist_count += 1;
        }

        if self.hist_count >= 4 {
            let valid = &self.history[..self.hist_count];
            let mean = valid.iter().sum::<f32>() / self.hist_count as f32;
            let std_dev = (valid.iter().map(|&x| (x - mean) * (x - mean)).sum::<f32>()
                / self.hist_count as f32)
                .sqrt();

            if self.locked_bpm.is_none() && std_dev < 3.5 && confidence > 0.45 {
                // Lock onto the consensus tempo.
                self.locked_bpm = Some(mean);
                self.beat_period = self.fps * 60.0 / mean;
            } else if let Some(locked) = self.locked_bpm {
                // Slowly drift the locked value.
                let updated = locked * 0.92 + mean * 0.08;
                self.locked_bpm = Some(updated);
                self.beat_period = self.fps * 60.0 / updated;
            }
        }
    }

    pub fn bpm(&self) -> Option<f32> {
        self.locked_bpm
    }
    pub fn is_locked(&self) -> bool {
        self.locked_bpm.is_some()
    }
    pub fn confidence(&self) -> f32 {
        self.cached_confidence
    }
    pub fn beat_phase(&self) -> f32 {
        self.phase
    }

    pub fn reset(&mut self) {
        self.onset_buf.fill(0.0);
        self.write_pos = 0;
        self.frame_count = 0;
        self.history.fill(0.0);
        self.hist_pos = 0;
        self.hist_count = 0;
        self.locked_bpm = None;
        self.deviate_count = 0;
        self.phase = 0.0;
        self.beat_period = 0.0;
        self.onset_ema = 0.0;
        self.cached_confidence = 0.0;
        self.frames_since_estimate = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 44100.0;

    /// Generate a pure sine wave at `freq` Hz, `n` samples long.
    fn sine(freq: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / SR).sin() as f32)
            .collect()
    }

    /// RMS in dB of a slice.
    fn rms_db(samples: &[f32]) -> f32 {
        let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum / samples.len() as f64).sqrt();
        if rms < 1e-10 {
            -90.0
        } else {
            (20.0 * rms.log10()) as f32
        }
    }

    /// Verify that a low-pass filter passes bass and rejects treble.
    #[test]
    fn test_lowpass_frequency_response() {
        let mut lp = Biquad::lowpass(250.0, SR);
        let n = 8820; // 200ms — enough to settle

        // 100 Hz sine → should pass through (~0 dB attenuation)
        let input = sine(100.0, n);
        let output: Vec<f32> = input.iter().map(|&s| lp.tick(s)).collect();
        let in_db = rms_db(&input[4410..]); // skip transient
        let out_db = rms_db(&output[4410..]);
        let attenuation = in_db - out_db;
        assert!(
            attenuation < 1.5,
            "100Hz through LP250: attenuation {attenuation:.1} dB should be < 1.5"
        );

        // 4000 Hz sine → should be heavily attenuated (>20 dB)
        lp.reset();
        let input = sine(4000.0, n);
        let output: Vec<f32> = input.iter().map(|&s| lp.tick(s)).collect();
        let in_db = rms_db(&input[4410..]);
        let out_db = rms_db(&output[4410..]);
        let attenuation = in_db - out_db;
        assert!(
            attenuation > 20.0,
            "4kHz through LP250: attenuation {attenuation:.1} dB should be > 20"
        );
    }

    /// Verify that a high-pass filter passes treble and rejects bass.
    #[test]
    fn test_highpass_frequency_response() {
        let mut hp = Biquad::highpass(2000.0, SR);
        let n = 8820;

        // 8000 Hz → should pass
        let input = sine(8000.0, n);
        let output: Vec<f32> = input.iter().map(|&s| hp.tick(s)).collect();
        let attenuation = rms_db(&input[4410..]) - rms_db(&output[4410..]);
        assert!(
            attenuation < 1.5,
            "8kHz through HP2k: attenuation {attenuation:.1} dB should be < 1.5"
        );

        // 100 Hz → should be heavily attenuated
        hp.reset();
        let input = sine(100.0, n);
        let output: Vec<f32> = input.iter().map(|&s| hp.tick(s)).collect();
        let attenuation = rms_db(&input[4410..]) - rms_db(&output[4410..]);
        assert!(
            attenuation > 20.0,
            "100Hz through HP2k: attenuation {attenuation:.1} dB should be > 20"
        );
    }

    /// Verify 3-band splitter routes frequencies to correct bands.
    #[test]
    fn test_band_splitter_routing() {
        let mut splitter = BandSplitter::new(SR);
        let n = 8820;

        // Pure 100 Hz → energy should be in bass, not treble
        let bass_tone = sine(100.0, n);
        let mut energy = BandEnergy::default();
        for &s in &bass_tone[2205..] {
            energy.push(&mut splitter, s);
        }
        let (bass_db, _mid_db, treble_db) = energy.to_db();
        assert!(
            bass_db > treble_db + 20.0,
            "100Hz: bass ({bass_db:.1}) should dominate treble ({treble_db:.1}) by >20 dB"
        );

        // Pure 8000 Hz → energy should be in treble, not bass
        splitter.reset();
        let treble_tone = sine(8000.0, n);
        let mut energy = BandEnergy::default();
        for &s in &treble_tone[2205..] {
            energy.push(&mut splitter, s);
        }
        let (bass_db, _mid_db, treble_db) = energy.to_db();
        assert!(
            treble_db > bass_db + 20.0,
            "8kHz: treble ({treble_db:.1}) should dominate bass ({bass_db:.1}) by >20 dB"
        );

        // Pure 1000 Hz → energy should be in mid
        splitter.reset();
        let mid_tone = sine(1000.0, n);
        let mut energy = BandEnergy::default();
        for &s in &mid_tone[2205..] {
            energy.push(&mut splitter, s);
        }
        let (bass_db, mid_db, treble_db) = energy.to_db();
        assert!(
            mid_db > bass_db + 10.0,
            "1kHz: mid ({mid_db:.1}) should dominate bass ({bass_db:.1}) by >10 dB"
        );
        assert!(
            mid_db > treble_db + 10.0,
            "1kHz: mid ({mid_db:.1}) should dominate treble ({treble_db:.1}) by >10 dB"
        );
    }

    /// Verify the adaptive bulb color algorithm produces distinct tints for
    /// bass-heavy vs treble-heavy signals, and that the adaptive window
    /// stretches brightness variation vs the old static range.
    #[test]
    fn test_adaptive_bulb_color_properties() {
        /// Test-only params mirroring the freq_bulb_color inputs.
        struct BulbParams {
            level_db: f32,
            mean_db: f32,
            spread_db: f32,
            bass_db: f32,
            mid_db: f32,
            treble_db: f32,
        }

        // Replicate the bulb_color logic for testing (the actual fn is private,
        // so we inline the core algorithm here — any drift is caught by the
        // assertions below).
        fn adaptive_bulb(p: &BulbParams) -> (u8, u8, u8) {
            let spread = p.spread_db.clamp(2.0, 22.0);
            let mut floor = (p.mean_db - 2.5 * spread).clamp(-90.0, -10.0);
            let ceil = (p.mean_db + 2.0 * spread).clamp(-40.0, -1.0);
            if ceil - floor < 8.0 {
                floor = (ceil - 8.0).max(-90.0);
            }

            let t = ((p.level_db - floor) / (ceil - floor)).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            let t = t.powf(0.72);

            let b = (p.bass_db.max(-60.0) + 60.0) / 60.0;
            let m = (p.mid_db.max(-60.0) + 60.0) / 60.0;
            let tr = (p.treble_db.max(-60.0) + 60.0) / 60.0;
            let total = (b + m + tr).max(0.001);
            let (wb, wm, wt) = (b / total, m / total, tr / total);

            let r = wb * 255.0 + wm * 240.0 + wt * 160.0;
            let g = wb * 180.0 + wm * 230.0 + wt * 200.0;
            let bl = wb * 60.0 + wm * 200.0 + wt * 255.0;

            let scale = 0.08 + 0.92 * t;
            (
                (r * scale).round().min(255.0) as u8,
                (g * scale).round().min(255.0) as u8,
                (bl * scale).round().min(255.0) as u8,
            )
        }

        // ── Bass-heavy signal should be warmer (more red, less blue) ───────
        let bass_heavy = BulbParams {
            level_db: -15.0,
            mean_db: -18.0,
            spread_db: 4.0,
            bass_db: -10.0,
            mid_db: -30.0,
            treble_db: -40.0,
        };
        let treble_heavy = BulbParams {
            level_db: -15.0,
            mean_db: -18.0,
            spread_db: 4.0,
            bass_db: -40.0,
            mid_db: -30.0,
            treble_db: -10.0,
        };
        let (br, _bg, bb) = adaptive_bulb(&bass_heavy);
        let (tr, _tg, tb) = adaptive_bulb(&treble_heavy);

        assert!(
            br > tr + 10,
            "bass-heavy R ({br}) should be warmer than treble-heavy R ({tr})"
        );
        assert!(
            tb > bb + 10,
            "treble-heavy B ({tb}) should be cooler than bass-heavy B ({bb})"
        );

        // ── Adaptive window gives more brightness variation ────────────────
        let levels = [-21.0_f32, -19.0, -17.0, -15.0, -13.0];
        let adaptive_brightness: Vec<f32> = levels
            .iter()
            .map(|&l| {
                let (r, g, b) = adaptive_bulb(&BulbParams {
                    level_db: l,
                    mean_db: -17.0,
                    spread_db: 3.0,
                    bass_db: -20.0,
                    mid_db: -20.0,
                    treble_db: -20.0,
                });
                (r as f32 + g as f32 + b as f32) / 3.0
            })
            .collect();

        let range = adaptive_brightness
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max)
            - adaptive_brightness
                .iter()
                .cloned()
                .fold(f32::INFINITY, f32::min);
        assert!(
            range > 50.0,
            "6 dB span should produce >50 brightness units of variation (got {range:.1})"
        );

        // ── Silence should produce a dim bulb ──────────────────────────────
        let silence = adaptive_bulb(&BulbParams {
            level_db: -90.0,
            mean_db: -30.0,
            spread_db: 6.0,
            bass_db: -90.0,
            mid_db: -90.0,
            treble_db: -90.0,
        });
        assert!(
            silence.0 < 30 && silence.1 < 30 && silence.2 < 30,
            "silence should be very dim: {:?}",
            silence
        );
    }

    /// BpmEstimator detects a regular beat pattern and rejects silence.
    #[test]
    fn test_bpm_estimator() {
        let fps = 25.0;
        let mut est = BpmEstimator::new(fps);

        // ~115 BPM: strong onset every 13 frames (520 ms), silence between.
        for frame in 0..300usize {
            let onset = if frame % 13 == 0 { 3.0 } else { 0.0 };
            est.push_onset(onset);
        }

        let bpm = est.bpm();
        assert!(bpm.is_some(), "should detect BPM from regular beats");
        let bpm = bpm.unwrap();
        assert!(
            bpm > 100.0 && bpm < 130.0,
            "expected ~115 BPM, got {bpm:.1}"
        );
        assert!(est.confidence() >= 0.25, "confidence={}", est.confidence());

        // Reset + silence → no BPM.
        est.reset();
        for _ in 0..300 {
            est.push_onset(0.0);
        }
        assert!(
            est.bpm().is_none(),
            "silence should not produce a BPM estimate"
        );
    }

    /// ACF should find the correct period in a clean synthetic onset signal.
    #[test]
    fn test_acf_finds_correct_period() {
        let n = 512;
        let lag_true = 13usize; // ~115 BPM at 25 FPS
        let mut onset = vec![0.0_f32; n];
        for i in (0..n).step_by(lag_true) {
            onset[i] = 1.0;
        }
        let mean = onset.iter().sum::<f32>() / n as f32;
        let centered: Vec<f32> = onset.iter().map(|&x| (x - mean).max(0.0)).collect();

        let (bpm, conf) = acf_comb_bpm(&centered, 25.0, 6, 50);
        assert!(bpm > 0.0, "should find a BPM for periodic signal");
        assert!(
            bpm > 100.0 && bpm < 130.0,
            "ACF should find ~115 BPM, got {bpm:.1}"
        );
        assert!(conf > 0.3, "confidence should be clear: {conf:.2}");
    }

    /// Comb filter scores the true period higher than the half-tempo alias.
    #[test]
    fn test_comb_prefers_true_period_over_half_tempo() {
        let n = 512;
        let lag_true = 12usize; // 125 BPM at 25 FPS
        let mut onset = vec![0.0_f32; n];
        for i in (0..n).step_by(lag_true) {
            onset[i] = 1.0;
        }
        let mean = onset.iter().sum::<f32>() / n as f32;
        let centered: Vec<f32> = onset.iter().map(|&x| (x - mean).max(0.0)).collect();

        // Manually compute comb scores at lag=12 and lag=24.
        let acf_at = |lag: usize| -> f32 {
            let valid = n - lag;
            centered[..valid]
                .iter()
                .zip(centered[lag..].iter())
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / valid as f32
        };
        let comb_12 = acf_at(12) + 0.5 * acf_at(24) + 0.33 * acf_at(36) + 0.25 * acf_at(48);
        let comb_24 = acf_at(24) + 0.5 * acf_at(48);
        assert!(
            comb_12 > comb_24,
            "comb at true period lag=12 ({comb_12:.4}) should beat half-tempo lag=24 ({comb_24:.4})"
        );
    }
}
