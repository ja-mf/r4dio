use crate::signals::VisualSignals;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VizMode {
    TectonicTerrain,
    LiquidMarble,
    StormCells,
    NeonDrift,
    ShatterMap,
    TopographicRadar,
    PulseBulb,
    TripleBulbs,
}

const ALL_MODES: [VizMode; 8] = [
    VizMode::TectonicTerrain,
    VizMode::LiquidMarble,
    VizMode::StormCells,
    VizMode::NeonDrift,
    VizMode::ShatterMap,
    VizMode::TopographicRadar,
    VizMode::PulseBulb,
    VizMode::TripleBulbs,
];

impl VizMode {
    pub fn all() -> &'static [VizMode] {
        &ALL_MODES
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let idx = self.index();
        all[(idx + 1) % all.len()]
    }

    pub fn index(self) -> usize {
        Self::all().iter().position(|m| *m == self).unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TectonicTerrain => "Tectonic Terrain",
            Self::LiquidMarble => "Liquid Marble",
            Self::StormCells => "Storm Cells",
            Self::NeonDrift => "Neon Drift",
            Self::ShatterMap => "Shatter Map",
            Self::TopographicRadar => "Topographic Radar",
            Self::PulseBulb => "Pulse Bulb",
            Self::TripleBulbs => "Triple Bulbs (Low / Mid / High)",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FieldTuning {
    pub zoom: f32,
    pub motion: f32,
}

impl Default for FieldTuning {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            motion: 1.0,
        }
    }
}

pub struct VisualizationEngine {
    mode: VizMode,
    width: usize,
    height: usize,
    field: Vec<f32>,
    scratch: Vec<f32>,
    phase: f32,
    seed: u64,
    bulb_energy: f32,
    bulb_energy_lmh: [f32; 3],
    bulb_impulse: f32,
    bulb_impulse_lmh: [f32; 3],
}

impl VisualizationEngine {
    pub fn new(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let mut out = Self {
            mode: VizMode::TectonicTerrain,
            width,
            height,
            field: vec![0.0; width * height],
            scratch: vec![0.0; width * height],
            phase: 0.0,
            seed: 0xA81CD9B5,
            bulb_energy: 0.0,
            bulb_energy_lmh: [0.0; 3],
            bulb_impulse: 0.0,
            bulb_impulse_lmh: [0.0; 3],
        };
        out.seed_field();
        out
    }

    pub fn mode(&self) -> VizMode {
        self.mode
    }

    pub fn next_mode(&mut self) {
        self.mode = self.mode.next();
    }

    pub fn set_mode(&mut self, mode: VizMode) {
        self.mode = mode;
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.field.resize(width * height, 0.0);
        self.scratch.resize(width * height, 0.0);
        self.seed_field();
    }

    fn seed_field(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let n = hash01(x as u32, y as u32, self.seed);
                self.field[idx] = 0.3 + 0.4 * n;
            }
        }
    }

    pub fn update(&mut self, signals: &VisualSignals, dt: f32, tuning: FieldTuning) {
        self.phase += dt * (0.4 + tuning.motion * (0.2 + signals.mid * 1.7));
        match self.mode {
            VizMode::TectonicTerrain => self.update_tectonic(signals, tuning),
            VizMode::LiquidMarble => self.update_marble(signals, tuning),
            VizMode::StormCells => self.update_storm(signals, tuning),
            VizMode::NeonDrift => self.update_neon(signals, tuning),
            VizMode::ShatterMap => self.update_shatter(signals, tuning),
            VizMode::TopographicRadar => self.update_radar(signals, tuning),
            VizMode::PulseBulb => self.update_pulse_bulb(signals, dt, tuning),
            VizMode::TripleBulbs => self.update_triple_bulbs(signals, dt, tuning),
        }
        std::mem::swap(&mut self.field, &mut self.scratch);
    }

    pub fn field(&self) -> (&[f32], usize, usize) {
        (&self.field, self.width, self.height)
    }

    fn update_tectonic(&mut self, s: &VisualSignals, t: FieldTuning) {
        let zoom = t.zoom.clamp(0.35, 6.0);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let n = neighborhood4(&self.field, self.width, self.height, x, y);
                let nx = x as f32 / self.width as f32;
                let ny = y as f32 / self.height as f32;
                let ridge_x = ((nx * zoom * 9.0 + self.phase * (0.7 + 1.5 * s.low)).sin() * 0.5 + 0.5)
                    .powf(1.0 + 2.0 * s.low);
                let ridge_y = ((ny * zoom * 6.0 - self.phase * (0.5 + 1.3 * s.mid)).cos() * 0.5 + 0.5)
                    .powf(1.0 + 1.6 * s.low);
                let flow = (s.mid - 0.5) * 2.0 * t.motion;
                let adv = sample_bilinear(
                    &self.field,
                    self.width,
                    self.height,
                    x as f32 - flow * 2.0,
                    y as f32 + flow,
                );
                let grit = hash01(x as u32, y as u32, self.seed ^ self.phase.to_bits() as u64);
                self.scratch[idx] = (0.40 * self.field[idx]
                    + 0.22 * n
                    + 0.20 * (0.56 * ridge_x + 0.44 * ridge_y)
                    + 0.12 * adv
                    + 0.06 * (grit * s.high + s.transient * 0.8))
                    .clamp(0.0, 1.0);
            }
        }
    }

    fn update_marble(&mut self, s: &VisualSignals, t: FieldTuning) {
        let zoom = t.zoom.clamp(0.35, 6.0);
        let cx = self.width as f32 * 0.5;
        let cy = self.height as f32 * 0.5;
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let nx = (x as f32 - cx) / self.width as f32;
                let ny = (y as f32 - cy) / self.height as f32;
                let swirl = 0.35 + 0.8 * s.mid;
                let vx = -ny * swirl + (self.phase + ny * 8.0).sin() * 0.09;
                let vy = nx * swirl + (self.phase * 0.7 + nx * 7.0).cos() * 0.09;
                let carried = sample_bilinear(
                    &self.field,
                    self.width,
                    self.height,
                    x as f32 - vx * (1.8 + 3.2 * t.motion),
                    y as f32 - vy * (1.8 + 3.2 * t.motion),
                );
                let veins = ((nx * zoom * 20.0
                    + ny * zoom * 14.0
                    + carried * (3.2 + 1.5 * s.low)
                    + self.phase * (0.8 + s.low))
                    .sin()
                    * 0.5
                    + 0.5)
                    .powf(0.85 + 0.55 * s.low);
                let sparkle =
                    (((nx * 31.0).sin() * (ny * 27.0).cos()).abs() * s.high * 0.14).clamp(0.0, 1.0);
                self.scratch[idx] = ((0.68 - 0.22 * s.spectral_flux) * carried
                    + (0.32 + 0.22 * s.spectral_flux) * veins
                    + sparkle)
                    .clamp(0.0, 1.0);
            }
        }
    }

    fn update_storm(&mut self, s: &VisualSignals, t: FieldTuning) {
        let diffusion = (0.15 + 0.35 * s.mid).clamp(0.1, 0.7);
        let react = (0.10 + 0.85 * s.pulse + 0.40 * s.transient).clamp(0.0, 1.2);
        let decay = (0.12 + 0.25 * (1.0 - s.low)).clamp(0.05, 0.6);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let c = self.field[idx];
                let n = neighborhood4(&self.field, self.width, self.height, x, y);
                let lap = n - c;
                let trigger = hash01(x as u32, y as u32, self.seed ^ (self.phase * 1000.0) as u64);
                let lightning = if trigger > 0.995 - s.transient * 0.03 {
                    0.7 + 0.3 * s.high
                } else {
                    0.0
                };
                let mut v =
                    c + diffusion * lap + react * c * (1.0 - c) - decay * c + lightning * 0.08 * t.motion;
                v += s.high * 0.03 * (trigger - 0.5);
                self.scratch[idx] = v.clamp(0.0, 1.0);
            }
        }
    }

    fn update_neon(&mut self, s: &VisualSignals, t: FieldTuning) {
        let zoom = t.zoom.clamp(0.35, 6.0);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let nx = x as f32 / self.width as f32 - 0.5;
                let ny = y as f32 / self.height as f32 - 0.5;
                let wave = ((nx * zoom * 8.0 + self.phase * (0.6 + s.mid)).sin()
                    + (ny * zoom * 5.5 - self.phase * (0.5 + s.low)).cos())
                    * 0.25
                    + 0.5;
                let flow = sample_bilinear(
                    &self.field,
                    self.width,
                    self.height,
                    x as f32 + (self.phase + ny * 11.0).sin() * (1.2 + t.motion),
                    y as f32 + (self.phase * 0.8 + nx * 9.0).cos() * (1.2 + t.motion),
                );
                let smooth = neighborhood8(&self.field, self.width, self.height, x, y);
                self.scratch[idx] =
                    (0.36 * flow + 0.34 * smooth + 0.25 * wave + 0.05 * s.pulse).clamp(0.0, 1.0);
            }
        }
    }

    fn update_shatter(&mut self, s: &VisualSignals, t: FieldTuning) {
        let cx = self.width as f32 * (0.5 + (self.phase * 0.37).sin() * 0.18);
        let cy = self.height as f32 * (0.5 + (self.phase * 0.29).cos() * 0.18);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let r = (dx * dx + dy * dy).sqrt();
                let a = dy.atan2(dx);
                let rings = ((r * 0.12 * t.zoom + self.phase * (1.8 + 1.6 * s.low)).sin() * 0.5 + 0.5)
                    .powf(1.2 + 2.4 * s.transient);
                let cracks = ((a * (7.0 + 10.0 * s.high) + self.phase * (2.0 + 2.0 * t.motion)).sin()).abs();
                let crack_edge = (1.0 - cracks).powf(16.0 - 8.0 * s.high);
                let old = self.field[idx] * (0.82 - 0.35 * s.transient);
                self.scratch[idx] = (old + 0.28 * rings + 0.52 * crack_edge + 0.16 * s.pulse).clamp(0.0, 1.0);
            }
        }
    }

    fn update_radar(&mut self, s: &VisualSignals, t: FieldTuning) {
        let cx = self.width as f32 * 0.5;
        let cy = self.height as f32 * 0.5;
        let sweep = self.phase * (0.8 + t.motion * 0.8);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let r = (dx * dx + dy * dy).sqrt() / (self.width.min(self.height) as f32 * 0.5);
                let ang = dy.atan2(dx);
                let scan = (1.0 - angular_distance(ang, sweep).min(std::f32::consts::PI) / std::f32::consts::PI)
                    .powf(6.0 - 3.0 * s.mid);
                let contour = ((r * (18.0 + 12.0 * t.zoom) - self.phase * (1.0 + s.low)).sin() * 0.5 + 0.5)
                    .powf(0.9 + 1.8 * s.low);
                let return_echo = (0.35 + 0.65 * scan) * contour + 0.2 * s.high;
                self.scratch[idx] =
                    (self.field[idx] * (0.78 - 0.18 * s.pulse) + return_echo * 0.42 + s.transient * 0.1)
                        .clamp(0.0, 1.0);
            }
        }
    }

    fn update_pulse_bulb(&mut self, s: &VisualSignals, dt: f32, t: FieldTuning) {
        let base = (0.52 * s.rms_drive + 0.22 * db_to_unit(s.vu_db) + 0.12 * s.pulse).clamp(0.0, 1.0);
        let kick = (1.45 * s.low_delta + 0.62 * s.transient + 0.35 * s.punch + 0.18 * s.pulse).clamp(0.0, 1.0);
        if kick > 0.18 {
            let impulse_add = ((kick - 0.18) / 0.82).powf(0.72);
            self.bulb_impulse = (self.bulb_impulse + impulse_add * 0.95).clamp(0.0, 1.0);
        }
        let impulse_decay = 1.0 - (-dt / (0.070 + 0.18 * (1.0 - s.punch))).exp();
        self.bulb_impulse = (self.bulb_impulse * (1.0 - impulse_decay)).max(0.0);

        let target = (base + 0.85 * self.bulb_impulse).clamp(0.0, 1.0);
        let punch = s.punch.clamp(0.0, 1.0);
        let attack = 1.0 - (-dt / (0.007 - 0.003 * punch).max(0.003)).exp();
        let release = 1.0 - (-dt / (0.095 + 0.26 * punch + 0.12 * (1.0 - self.bulb_impulse))).exp();
        if target > self.bulb_energy {
            self.bulb_energy += attack * (target - self.bulb_energy);
        } else {
            self.bulb_energy += release * (target - self.bulb_energy);
        }
        self.render_single_bulb(self.bulb_energy, t.zoom);
    }

    fn update_triple_bulbs(&mut self, s: &VisualSignals, dt: f32, t: FieldTuning) {
        let drives = [
            (0.55 * s.low + 0.15 * s.rms_drive + 0.08 * db_to_unit(s.vu_db)).clamp(0.0, 1.0),
            (0.62 * s.mid + 0.12 * s.rms_drive + 0.08 * db_to_unit(s.vu_db)).clamp(0.0, 1.0),
            (0.58 * s.high + 0.10 * s.rms_drive + 0.06 * db_to_unit(s.vu_db)).clamp(0.0, 1.0),
        ];
        let kicks = [
            (1.55 * s.low_delta + 0.45 * s.transient + 0.12 * s.pulse).clamp(0.0, 1.0),
            (1.20 * s.mid_delta + 0.50 * s.transient + 0.16 * s.pulse).clamp(0.0, 1.0),
            (1.05 * s.high_delta + 0.66 * s.transient + 0.20 * s.pulse).clamp(0.0, 1.0),
        ];
        for i in 0..3 {
            let kick = kicks[i];
            if kick > 0.17 {
                let impulse_add = ((kick - 0.17) / 0.83).powf(0.74);
                self.bulb_impulse_lmh[i] = (self.bulb_impulse_lmh[i] + impulse_add * (0.90 + i as f32 * 0.05)).clamp(0.0, 1.0);
            }
            let imp_decay = 1.0 - (-dt / (0.072 + i as f32 * 0.018)).exp();
            self.bulb_impulse_lmh[i] = (self.bulb_impulse_lmh[i] * (1.0 - imp_decay)).max(0.0);

            let target = (drives[i] + 0.82 * self.bulb_impulse_lmh[i]).clamp(0.0, 1.0);
            let punch = (s.punch + [0.10, 0.08, 0.12][i]).clamp(0.0, 1.0);
            let attack = 1.0 - (-dt / (0.0065 + i as f32 * 0.0015 - 0.0025 * punch).max(0.003)).exp();
            let release = 1.0
                - (-dt / (0.11 + 0.20 * punch + i as f32 * 0.05 + 0.08 * (1.0 - self.bulb_impulse_lmh[i]))).exp();
            if target > self.bulb_energy_lmh[i] {
                self.bulb_energy_lmh[i] += attack * (target - self.bulb_energy_lmh[i]);
            } else {
                self.bulb_energy_lmh[i] += release * (target - self.bulb_energy_lmh[i]);
            }
        }
        self.render_triple_bulbs(self.bulb_energy_lmh, t.zoom);
    }

    fn render_single_bulb(&mut self, energy: f32, zoom: f32) {
        let cx = self.width as f32 * 0.5;
        let cy = self.height as f32 * 0.43;
        let base_r = (self.width.min(self.height) as f32
            * 0.24
            * zoom.clamp(0.55, 1.8)
            * (0.72 + 0.62 * energy))
            .max(4.0);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let glass = (1.0 - d / base_r).clamp(0.0, 1.0).powf(1.6);
                let halo = (1.0 - d / (base_r * 1.7)).clamp(0.0, 1.0).powf(2.8);
                let neck_dx = (x as f32 - cx).abs() / (base_r * 0.33);
                let neck_dy = ((y as f32 - (cy + base_r * 0.98)).abs()) / (base_r * 0.28);
                let neck = (1.0 - neck_dx.max(neck_dy)).clamp(0.0, 1.0);
                let filament = (1.0
                    - (((x as f32 - cx).abs() / (base_r * 0.34))
                        .max((y as f32 - (cy + base_r * 0.18)).abs() / (base_r * 0.12))))
                .clamp(0.0, 1.0)
                    * energy;
                self.scratch[idx] = (energy * (0.62 * glass + 0.26 * halo + 0.10 * neck) + 0.28 * filament).clamp(0.0, 1.0);
            }
        }
    }

    fn render_triple_bulbs(&mut self, energies: [f32; 3], zoom: f32) {
        let centers = [
            self.width as f32 * 0.20,
            self.width as f32 * 0.50,
            self.width as f32 * 0.80,
        ];
        let cy = self.height as f32 * 0.45;
        let base_r_base = (self.width.min(self.height) as f32 * 0.17 * zoom.clamp(0.55, 1.8)).max(3.0);

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let mut v = 0.0_f32;
                for (i, cx) in centers.iter().enumerate() {
                    let e = energies[i];
                    let base_r = base_r_base * (0.78 + 0.55 * e);
                    let dx = x as f32 - *cx;
                    let dy = y as f32 - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    let glass = (1.0 - d / base_r).clamp(0.0, 1.0).powf(1.8);
                    let halo = (1.0 - d / (base_r * 1.6)).clamp(0.0, 1.0).powf(2.6);
                    let neck_dx = (x as f32 - *cx).abs() / (base_r * 0.30);
                    let neck_dy = ((y as f32 - (cy + base_r * 0.96)).abs()) / (base_r * 0.25);
                    let neck = (1.0 - neck_dx.max(neck_dy)).clamp(0.0, 1.0);
                    v += e * (0.56 * glass + 0.25 * halo + 0.12 * neck);
                }
                self.scratch[idx] = v.clamp(0.0, 1.0);
            }
        }
    }
}

fn db_to_unit(db: f32) -> f32 {
    ((db + 72.0) / 72.0).clamp(0.0, 1.0).powf(0.82)
}

fn neighborhood4(field: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let up = ((y + h - 1) % h) * w + x;
    let down = ((y + 1) % h) * w + x;
    let left = y * w + ((x + w - 1) % w);
    let right = y * w + ((x + 1) % w);
    (field[up] + field[down] + field[left] + field[right]) * 0.25
}

fn neighborhood8(field: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for oy in [-1isize, 0, 1] {
        for ox in [-1isize, 0, 1] {
            if ox == 0 && oy == 0 {
                continue;
            }
            let xx = ((x as isize + ox).rem_euclid(w as isize)) as usize;
            let yy = ((y as isize + oy).rem_euclid(h as isize)) as usize;
            sum += field[yy * w + xx];
            n += 1.0;
        }
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

fn sample_bilinear(field: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let xf = x.rem_euclid(width as f32);
    let yf = y.rem_euclid(height as f32);
    let x0 = xf.floor() as usize % width;
    let y0 = yf.floor() as usize % height;
    let x1 = (x0 + 1) % width;
    let y1 = (y0 + 1) % height;
    let tx = xf - x0 as f32;
    let ty = yf - y0 as f32;
    let a = field[y0 * width + x0] * (1.0 - tx) + field[y0 * width + x1] * tx;
    let b = field[y1 * width + x0] * (1.0 - tx) + field[y1 * width + x1] * tx;
    a * (1.0 - ty) + b * ty
}

fn angular_distance(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % (2.0 * std::f32::consts::PI);
    if d > std::f32::consts::PI {
        d -= 2.0 * std::f32::consts::PI;
    } else if d < -std::f32::consts::PI {
        d += 2.0 * std::f32::consts::PI;
    }
    d.abs()
}

fn hash01(x: u32, y: u32, seed: u64) -> f32 {
    let mut v = seed ^ ((x as u64) << 32) ^ y as u64;
    v ^= v >> 33;
    v = v.wrapping_mul(0xff51afd7ed558ccd);
    v ^= v >> 33;
    v = v.wrapping_mul(0xc4ceb9fe1a85ec53);
    v ^= v >> 33;
    (v as u32 as f32) / (u32::MAX as f32)
}
