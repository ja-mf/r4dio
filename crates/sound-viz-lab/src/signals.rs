use std::{f32::consts::PI, sync::Arc};

use rustfft::{num_complex::Complex, Fft, FftPlanner};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct VisualSignals {
    pub rms_db: f32,
    pub instant_peak_db: f32,
    pub vu_db: f32,
    pub peak_db: f32,
    pub mean_db: f32,
    pub spread_db: f32,
    pub rms_drive: f32,
    pub punch: f32,

    pub envelope_fast: f32,
    pub envelope_slow: f32,
    pub crest: f32,
    pub zcr: f32,

    pub low: f32,
    pub mid: f32,
    pub high: f32,
    pub low_delta: f32,
    pub mid_delta: f32,
    pub high_delta: f32,
    pub spectral_flux: f32,

    pub pulse: f32,
    pub transient: f32,
    pub confidence: f32,
}

impl Default for VisualSignals {
    fn default() -> Self {
        Self {
            rms_db: -90.0,
            instant_peak_db: -90.0,
            vu_db: -90.0,
            peak_db: -90.0,
            mean_db: -18.0,
            spread_db: 6.0,
            rms_drive: 0.0,
            punch: 0.0,
            envelope_fast: 0.0,
            envelope_slow: 0.0,
            crest: 0.0,
            zcr: 0.0,
            low: 0.0,
            mid: 0.0,
            high: 0.0,
            low_delta: 0.0,
            mid_delta: 0.0,
            high_delta: 0.0,
            spectral_flux: 0.0,
            pulse: 0.0,
            transient: 0.0,
            confidence: 0.0,
        }
    }
}

pub struct VisualSignalCore {
    sample_rate: f32,

    // Ballistics / trackers
    vu_db: f32,
    peak_db: f32,
    mean_db: f32,
    spread_db: f32,
    env_fast: f32,
    env_slow: f32,

    // Spectral tracking
    fft_size: usize,
    fft: Arc<dyn Fft<f32>>,
    fft_input: Vec<Complex<f32>>,
    fft_history: Vec<f32>,
    window: Vec<f32>,
    prev_mag: Vec<f32>,
    prev_low: f32,
    prev_mid: f32,
    prev_high: f32,
    flux_ema: f32,

    // Rhythmic-ish modulators
    transient_ema: f32,
    pulse: f32,
    rms_drive: f32,

    latest: VisualSignals,
}

impl VisualSignalCore {
    pub fn new(sample_rate: f32) -> Self {
        let fft_size = 1024;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / fft_size as f32).cos())
            .collect();

        Self {
            sample_rate: sample_rate.max(8_000.0),
            vu_db: -90.0,
            peak_db: -90.0,
            mean_db: -18.0,
            spread_db: 6.0,
            env_fast: 0.0,
            env_slow: 0.0,
            fft_size,
            fft,
            fft_input: vec![Complex::new(0.0, 0.0); fft_size],
            fft_history: Vec::with_capacity(fft_size),
            window,
            prev_mag: vec![0.0; fft_size / 2],
            prev_low: 0.0,
            prev_mid: 0.0,
            prev_high: 0.0,
            flux_ema: 0.0,
            transient_ema: 0.0,
            pulse: 0.0,
            rms_drive: 0.0,
            latest: VisualSignals::default(),
        }
    }

    pub fn latest(&self) -> &VisualSignals {
        &self.latest
    }

    pub fn update_from_pcm_chunk(&mut self, pcm: &[f32]) -> VisualSignals {
        if pcm.is_empty() {
            return self.latest.clone();
        }

        let n = pcm.len() as f32;
        let dt = (n / self.sample_rate).clamp(1.0 / 300.0, 0.5);

        let mut sum_sq = 0.0_f32;
        let mut peak_lin = 0.0_f32;
        let mut abs_mean = 0.0_f32;
        let mut zc = 0usize;
        let mut prev = pcm[0];

        for &s in pcm {
            let a = s.abs();
            peak_lin = peak_lin.max(a);
            sum_sq += s * s;
            abs_mean += a;
        }
        for &s in pcm.iter().skip(1) {
            if (s >= 0.0) != (prev >= 0.0) {
                zc += 1;
            }
            prev = s;
        }

        let rms = (sum_sq / n.max(1.0)).sqrt().max(1e-8);
        let rms_db = (20.0 * rms.log10()).clamp(-90.0, 0.0);
        let instant_peak_db = (20.0 * peak_lin.max(1e-8).log10()).clamp(-90.0, 0.0);
        abs_mean /= n.max(1.0);
        let zcr = zc as f32 / (pcm.len().saturating_sub(1).max(1) as f32);
        let punch = ((instant_peak_db - rms_db) / 24.0).clamp(0.0, 1.0);

        // VU body: fast attack, medium release (in dB domain).
        let attack = 1.0 - (-dt / 0.045).exp();
        let release = 1.0 - (-dt / 0.24).exp();
        if rms_db > self.vu_db {
            self.vu_db += attack * (rms_db - self.vu_db);
        } else {
            self.vu_db += release * (rms_db - self.vu_db);
        }

        // Peak with quick fall toward body.
        if rms_db > self.peak_db {
            self.peak_db = rms_db;
        } else {
            self.peak_db = (self.peak_db - dt * 28.0).max(self.vu_db).max(-90.0);
        }

        // Long-term mean/spread (same spirit as r4dio meter trackers).
        let alpha_mean = (1.0 - (-dt / 4.0).exp()).min(0.2);
        self.mean_db += alpha_mean * (rms_db - self.mean_db);
        let alpha_spread = (1.0 - (-dt / 8.0).exp()).min(0.15);
        let deviation = (rms_db - self.mean_db).abs();
        self.spread_db += alpha_spread * (deviation - self.spread_db);
        self.spread_db = self.spread_db.max(2.0);

        // Envelopes (linear domain).
        let env_fast_a = 1.0 - (-dt / 0.05).exp();
        let env_slow_a = 1.0 - (-dt / 0.35).exp();
        self.env_fast += env_fast_a * (abs_mean - self.env_fast);
        self.env_slow += env_slow_a * (abs_mean - self.env_slow);

        // Crest factor normalized 0..1.
        let crest_raw = (peak_lin / rms).clamp(1.0, 8.0);
        let crest = ((crest_raw - 1.0) / 7.0).clamp(0.0, 1.0);

        // Keep a small history window for FFT.
        self.fft_history.extend_from_slice(pcm);
        if self.fft_history.len() > self.fft_size {
            let drop = self.fft_history.len() - self.fft_size;
            self.fft_history.drain(0..drop);
        }

        let (low, mid, high, flux_now) = self.compute_spectrum();
        let low_delta = (low - self.prev_low).abs();
        let mid_delta = (mid - self.prev_mid).abs();
        let high_delta = (high - self.prev_high).abs();
        self.prev_low = low;
        self.prev_mid = mid;
        self.prev_high = high;

        let flux_alpha = 1.0 - (-dt / 0.12).exp();
        self.flux_ema += flux_alpha * (flux_now - self.flux_ema);

        let transient_raw = (0.55 * high_delta
            + 0.30 * self.flux_ema
            + 0.15 * (self.env_fast - self.env_slow).max(0.0))
            .clamp(0.0, 1.0);

        let trans_attack = 1.0 - (-dt / 0.03).exp();
        let trans_release = 1.0 - (-dt / 0.2).exp();
        if transient_raw > self.transient_ema {
            self.transient_ema += trans_attack * (transient_raw - self.transient_ema);
        } else {
            self.transient_ema += trans_release * (transient_raw - self.transient_ema);
        }

        self.pulse = (self.pulse - dt * (0.6 + self.pulse * 1.2)).max(0.0);
        if self.transient_ema > 0.32 {
            self.pulse = (self.pulse + self.transient_ema * 0.85).min(1.0);
        }

        let signal_conf = ((rms_db + 72.0) / 72.0).clamp(0.0, 1.0);
        let activity_conf = ((self.spread_db - 2.0) / 18.0).clamp(0.0, 1.0);
        let flux_conf = (self.flux_ema * 1.8).clamp(0.0, 1.0);
        let confidence = (0.50 * signal_conf + 0.30 * activity_conf + 0.20 * flux_conf).clamp(0.0, 1.0);
        let target_drive = (((rms_db + 72.0) / 72.0) + 0.42 * punch + 0.26 * self.transient_ema).clamp(0.0, 1.0);
        let att_tau = (0.016 - 0.008 * punch).max(0.004);
        let rel_tau = 0.25 + 0.95 * punch + 0.3 * self.transient_ema;
        let drive_attack = 1.0 - (-dt / att_tau).exp();
        let drive_release = 1.0 - (-dt / rel_tau).exp();
        if target_drive > self.rms_drive {
            self.rms_drive += drive_attack * (target_drive - self.rms_drive);
        } else {
            self.rms_drive += drive_release * (target_drive - self.rms_drive);
        }
        self.rms_drive = self.rms_drive.clamp(0.0, 1.0);

        self.latest = VisualSignals {
            rms_db,
            instant_peak_db,
            vu_db: self.vu_db,
            peak_db: self.peak_db,
            mean_db: self.mean_db,
            spread_db: self.spread_db,
            rms_drive: self.rms_drive,
            punch,
            envelope_fast: self.env_fast.clamp(0.0, 1.0),
            envelope_slow: self.env_slow.clamp(0.0, 1.0),
            crest,
            zcr: zcr.clamp(0.0, 1.0),
            low: low.clamp(0.0, 1.0),
            mid: mid.clamp(0.0, 1.0),
            high: high.clamp(0.0, 1.0),
            low_delta: low_delta.clamp(0.0, 1.0),
            mid_delta: mid_delta.clamp(0.0, 1.0),
            high_delta: high_delta.clamp(0.0, 1.0),
            spectral_flux: self.flux_ema.clamp(0.0, 1.0),
            pulse: self.pulse.clamp(0.0, 1.0),
            transient: self.transient_ema.clamp(0.0, 1.0),
            confidence,
        };

        self.latest.clone()
    }

    fn compute_spectrum(&mut self) -> (f32, f32, f32, f32) {
        let hist = if self.fft_history.len() > self.fft_size {
            &self.fft_history[self.fft_history.len() - self.fft_size..]
        } else {
            &self.fft_history
        };
        let pad = self.fft_size.saturating_sub(hist.len());

        for v in &mut self.fft_input {
            *v = Complex::new(0.0, 0.0);
        }
        for (i, &s) in hist.iter().enumerate() {
            let idx = pad + i;
            self.fft_input[idx] = Complex::new(s * self.window[idx], 0.0);
        }

        self.fft.process(&mut self.fft_input);

        let half = self.fft_size / 2;
        if self.prev_mag.len() != half {
            self.prev_mag.resize(half, 0.0);
        }

        let mut low = 0.0_f32;
        let mut mid = 0.0_f32;
        let mut high = 0.0_f32;
        let mut low_wsum = 0.0_f32;
        let mut mid_wsum = 0.0_f32;
        let mut high_wsum = 0.0_f32;
        let mut total = 0.0_f32;
        let mut flux = 0.0_f32;

        // Soft crossover points chosen for better low/mid/high separation in music-reactive visuals.
        const F_LOW: f32 = 220.0;
        const F_HIGH: f32 = 2_200.0;

        for bin in 1..half {
            let mag = self.fft_input[bin].norm();
            let freq = bin as f32 * self.sample_rate / self.fft_size as f32;
            total += mag;
            let w_low = 1.0 / (1.0 + (freq / F_LOW).powi(2));
            let w_high = if freq < 1.0 {
                0.0
            } else {
                1.0 / (1.0 + (F_HIGH / freq).powi(2))
            };
            let w_mid = ((1.0 - w_low) * (1.0 - w_high)).max(0.0);

            low += mag * w_low;
            mid += mag * w_mid;
            high += mag * w_high;
            low_wsum += w_low;
            mid_wsum += w_mid;
            high_wsum += w_high;

            let prev = self.prev_mag[bin];
            if mag > prev {
                flux += mag - prev;
            }
            self.prev_mag[bin] = mag;
        }

        if total > 1e-8 && low_wsum > 0.0 && mid_wsum > 0.0 && high_wsum > 0.0 {
            low = (low / low_wsum) / (total / half as f32 + 1e-8);
            mid = (mid / mid_wsum) / (total / half as f32 + 1e-8);
            high = (high / high_wsum) / (total / half as f32 + 1e-8);
            let band_total = low + mid + high;
            if band_total > 1e-8 {
                low /= band_total;
                mid /= band_total;
                high /= band_total;
            }
        }

        let flux_norm = if total > 1e-8 {
            (flux / total).clamp(0.0, 1.0)
        } else {
            0.0
        };

        (low, mid, high, flux_norm)
    }
}
