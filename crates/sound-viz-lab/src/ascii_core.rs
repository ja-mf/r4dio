use std::collections::HashMap;

use ratatui::style::Color;

use crate::{signals::VisualSignals, visuals::VizMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DitherMode {
    Off,
    Ordered,
    Noise,
}

impl DitherMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Ordered,
            Self::Ordered => Self::Noise,
            Self::Noise => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ordered => "ordered",
            Self::Noise => "noise",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderTuning {
    pub zoom: f32,
    pub global_contrast: f32,
    pub directional_contrast: f32,
    pub dither_strength: f32,
    pub dither_mode: DitherMode,
}

impl Default for RenderTuning {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            global_contrast: 1.4,
            directional_contrast: 1.35,
            dither_strength: 0.12,
            dither_mode: DitherMode::Ordered,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AsciiCell {
    pub ch: char,
    pub color: Color,
}

#[derive(Debug, Clone)]
pub struct AsciiFrame {
    pub rows: Vec<Vec<AsciiCell>>,
}

#[derive(Debug, Clone, Copy)]
struct Glyph {
    ch: char,
    shape: [f32; 6],
}

pub struct AsciiRenderCore {
    glyphs: Vec<Glyph>,
    cache: HashMap<u32, usize>,
    frame: u64,
}

impl AsciiRenderCore {
    pub fn new() -> Self {
        let mut glyphs = vec![
            Glyph { ch: ' ', shape: [0.00, 0.00, 0.00, 0.00, 0.00, 0.00] },
            Glyph { ch: '.', shape: [0.00, 0.00, 0.00, 0.00, 0.18, 0.18] },
            Glyph { ch: ':', shape: [0.14, 0.14, 0.00, 0.00, 0.14, 0.14] },
            Glyph { ch: '-', shape: [0.05, 0.05, 0.62, 0.62, 0.05, 0.05] },
            Glyph { ch: '_', shape: [0.02, 0.02, 0.08, 0.08, 0.86, 0.86] },
            Glyph { ch: '=', shape: [0.18, 0.18, 0.66, 0.66, 0.25, 0.25] },
            Glyph { ch: '+', shape: [0.25, 0.25, 0.82, 0.82, 0.25, 0.25] },
            Glyph { ch: '*', shape: [0.52, 0.52, 0.74, 0.74, 0.52, 0.52] },
            Glyph { ch: '/', shape: [0.06, 0.82, 0.22, 0.58, 0.84, 0.08] },
            Glyph { ch: '\\', shape: [0.82, 0.06, 0.58, 0.22, 0.08, 0.84] },
            Glyph { ch: '|', shape: [0.28, 0.28, 0.88, 0.88, 0.28, 0.28] },
            Glyph { ch: '!', shape: [0.20, 0.20, 0.78, 0.78, 0.30, 0.30] },
            Glyph { ch: 'L', shape: [0.78, 0.10, 0.74, 0.10, 0.86, 0.86] },
            Glyph { ch: 'J', shape: [0.10, 0.78, 0.10, 0.74, 0.86, 0.86] },
            Glyph { ch: 'r', shape: [0.62, 0.38, 0.68, 0.56, 0.26, 0.18] },
            Glyph { ch: 'n', shape: [0.62, 0.56, 0.68, 0.62, 0.34, 0.28] },
            Glyph { ch: 'b', shape: [0.64, 0.44, 0.72, 0.66, 0.62, 0.48] },
            Glyph { ch: 'd', shape: [0.44, 0.64, 0.66, 0.72, 0.48, 0.62] },
            Glyph { ch: '#', shape: [0.72, 0.72, 0.92, 0.92, 0.72, 0.72] },
            Glyph { ch: '%', shape: [0.78, 0.42, 0.46, 0.72, 0.56, 0.82] },
            Glyph { ch: '@', shape: [0.84, 0.80, 0.92, 0.90, 0.80, 0.78] },
        ];
        normalize_glyph_vectors(&mut glyphs);

        Self {
            glyphs,
            cache: HashMap::new(),
            frame: 0,
        }
    }

    pub fn render(
        &mut self,
        field: &[f32],
        width: usize,
        height: usize,
        tuning: RenderTuning,
        signals: &VisualSignals,
        mode: VizMode,
    ) -> AsciiFrame {
        if width == 0 || height == 0 || field.len() != width * height {
            return AsciiFrame {
                rows: vec![vec![]],
            };
        }

        if self.cache.len() > 120_000 {
            self.cache.clear();
        }

        self.frame = self.frame.wrapping_add(1);
        let mut rows = Vec::with_capacity(height);
        let cx = (width as f32 - 1.0) * 0.5;
        let cy = (height as f32 - 1.0) * 0.5;
        let zoom = tuning.zoom.clamp(0.35, 6.0);

        for y in 0..height {
            let mut row = Vec::with_capacity(width);
            for x in 0..width {
                let base_x = ((x as f32 - cx) / zoom) + cx;
                let base_y = ((y as f32 - cy) / zoom) + cy;

                let mut internal = [0.0_f32; 6];
                for (i, (ox, oy)) in INTERNAL_OFFSETS.iter().enumerate() {
                    let mut v = sample_field(field, width, height, base_x + ox, base_y + oy);
                    v = self.apply_dither(v, x, y, tuning);
                    internal[i] = v.clamp(0.0, 1.0);
                }

                let mut external = [0.0_f32; 10];
                for (i, (ox, oy)) in EXTERNAL_OFFSETS.iter().enumerate() {
                    let mut v = sample_field(field, width, height, base_x + ox, base_y + oy);
                    v = self.apply_dither(v, x, y, tuning);
                    external[i] = v.clamp(0.0, 1.0);
                }

                apply_global_contrast(&mut internal, tuning.global_contrast.max(1.0));
                apply_directional_contrast(
                    &mut internal,
                    &external,
                    tuning.directional_contrast.max(1.0),
                );

                let key = quantize_vec6(&internal);
                let glyph_idx = if let Some(idx) = self.cache.get(&key).copied() {
                    idx
                } else {
                    let idx = nearest_glyph_index(&self.glyphs, &internal);
                    self.cache.insert(key, idx);
                    idx
                };

                let edge = ((internal[0] - internal[5]).abs()
                    + (internal[1] - internal[4]).abs()
                    + (internal[2] - internal[3]).abs())
                    / 3.0;
                let avg = internal.iter().sum::<f32>() / 6.0;
                let base_value = sample_field(field, width, height, base_x, base_y);
                let color = colorize(
                    mode,
                    base_value,
                    avg,
                    edge,
                    width,
                    x,
                    y,
                    self.frame,
                    signals,
                );

                row.push(AsciiCell {
                    ch: self.glyphs[glyph_idx].ch,
                    color,
                });
            }
            rows.push(row);
        }

        AsciiFrame { rows }
    }

    fn apply_dither(&self, value: f32, x: usize, y: usize, tuning: RenderTuning) -> f32 {
        let strength = tuning.dither_strength.clamp(0.0, 1.0) * 0.35;
        if strength <= 0.0 {
            return value;
        }

        let centered = match tuning.dither_mode {
            DitherMode::Off => 0.0,
            DitherMode::Ordered => {
                let threshold = (BAYER_4X4[y % 4][x % 4] as f32 + 0.5) / 16.0;
                threshold - 0.5
            }
            DitherMode::Noise => {
                hash01(x as u32, y as u32, self.frame.wrapping_mul(0x9E37_79B9)) - 0.5
            }
        };

        (value + centered * strength).clamp(0.0, 1.0)
    }
}

const INTERNAL_OFFSETS: [(f32, f32); 6] = [
    (-0.30, -0.40),
    (0.30, -0.48),
    (-0.30, 0.00),
    (0.30, 0.00),
    (-0.30, 0.48),
    (0.30, 0.40),
];

const EXTERNAL_OFFSETS: [(f32, f32); 10] = [
    (-0.60, -1.10),
    (0.00, -1.20),
    (0.60, -1.10),
    (-1.10, -0.45),
    (1.10, -0.45),
    (-1.10, 0.45),
    (1.10, 0.45),
    (-0.60, 1.10),
    (0.00, 1.20),
    (0.60, 1.10),
];

const AFFECTING_EXTERNAL_INDICES: [&[usize]; 6] = [
    &[0, 1, 2, 4],
    &[0, 1, 3, 5],
    &[2, 4, 6],
    &[3, 5, 7],
    &[4, 6, 8, 9],
    &[5, 7, 8, 9],
];

const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

fn normalize_glyph_vectors(glyphs: &mut [Glyph]) {
    let mut max = [1e-6_f32; 6];
    for glyph in glyphs.iter() {
        for (i, &v) in glyph.shape.iter().enumerate() {
            if v > max[i] {
                max[i] = v;
            }
        }
    }
    for glyph in glyphs.iter_mut() {
        for (i, value) in glyph.shape.iter_mut().enumerate() {
            *value /= max[i];
        }
    }
}

fn apply_global_contrast(v: &mut [f32; 6], exponent: f32) {
    let max_value = v
        .iter()
        .copied()
        .fold(0.0_f32, |acc, x| if x > acc { x } else { acc });
    if max_value <= 1e-8 {
        return;
    }
    for value in v.iter_mut() {
        let n = (*value / max_value).clamp(0.0, 1.0);
        *value = n.powf(exponent) * max_value;
    }
}

fn apply_directional_contrast(v: &mut [f32; 6], external: &[f32; 10], exponent: f32) {
    for (i, value) in v.iter_mut().enumerate() {
        let mut max_value = *value;
        for &ext_idx in AFFECTING_EXTERNAL_INDICES[i] {
            max_value = max_value.max(external[ext_idx]);
        }
        if max_value <= 1e-8 {
            continue;
        }
        let n = (*value / max_value).clamp(0.0, 1.0);
        *value = n.powf(exponent) * max_value;
    }
}

fn quantize_vec6(v: &[f32; 6]) -> u32 {
    const BITS: u32 = 4;
    const RANGE: f32 = (1_u32 << BITS) as f32;

    let mut key = 0_u32;
    for value in v {
        let q = (value.clamp(0.0, 1.0) * RANGE).floor().min(RANGE - 1.0) as u32;
        key = (key << BITS) | q;
    }
    key
}

fn nearest_glyph_index(glyphs: &[Glyph], input: &[f32; 6]) -> usize {
    let mut best_idx = 0usize;
    let mut best_dist = f32::INFINITY;
    for (idx, glyph) in glyphs.iter().enumerate() {
        let mut dist = 0.0_f32;
        for i in 0..6 {
            let d = glyph.shape[i] - input[i];
            dist += d * d;
        }
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx;
        }
    }
    best_idx
}

#[allow(clippy::too_many_arguments)]
fn colorize(
    mode: VizMode,
    base_value: f32,
    avg: f32,
    edge: f32,
    width: usize,
    x: usize,
    y: usize,
    frame: u64,
    s: &VisualSignals,
) -> Color {
    if mode == VizMode::TripleBulbs {
        let x_phase = if width <= 1 {
            0.0
        } else {
            x as f32 / (width - 1) as f32
        };
        let idx = if x_phase < 0.33 {
            0
        } else if x_phase < 0.66 {
            1
        } else {
            2
        };
        let base = match idx {
            0 => [255, 120, 70],  // low
            1 => [255, 228, 120], // mid
            _ => [120, 210, 255], // high
        };
        let drive = match idx {
            0 => (0.72 * s.low + 0.20 * s.low_delta + 0.08 * s.pulse).clamp(0.0, 1.0),
            1 => (0.70 * s.mid + 0.18 * s.mid_delta + 0.12 * s.transient).clamp(0.0, 1.0),
            _ => (0.66 * s.high + 0.22 * s.high_delta + 0.12 * s.transient).clamp(0.0, 1.0),
        };
        let lum = (0.25 + 1.55 * (0.6 * drive + 0.4 * base_value)).clamp(0.15, 2.1);
        return Color::Rgb(scale_u8(base[0], lum), scale_u8(base[1], lum), scale_u8(base[2], lum));
    }

    let texture = (0.7 * avg + 0.3 * edge).clamp(0.0, 1.0);
    let shimmer = (((x as f32 * 0.06 + frame as f32 * 0.02).sin()
        + (y as f32 * 0.04 - frame as f32 * 0.015).cos())
        * 0.5
        + 0.5)
        * s.mid
        * 0.12;
    let t = (0.72 * base_value + 0.22 * texture + shimmer).clamp(0.0, 1.0);

    let palette = palette_for_mode(mode);
    let mut rgb = gradient_color(palette, t);

    let energy = ((s.low + s.mid + s.high) / 3.0).clamp(0.0, 1.0);
    let gain = (0.65 + 0.45 * energy + 0.35 * s.pulse).clamp(0.55, 1.75);
    rgb = [scale_u8(rgb[0], gain), scale_u8(rgb[1], gain), scale_u8(rgb[2], gain)];

    let sparkle_gate = hash01(x as u32, y as u32, frame.wrapping_mul(0xA24B1CFD));
    if sparkle_gate > 0.985 - s.high * 0.02 {
        rgb = [
            rgb[0].saturating_add((35.0 * (0.4 + s.transient)).round() as u8),
            rgb[1].saturating_add((25.0 * (0.3 + s.high)).round() as u8),
            rgb[2].saturating_add((40.0 * (0.3 + s.pulse)).round() as u8),
        ];
    }

    Color::Rgb(rgb[0], rgb[1], rgb[2])
}

fn palette_for_mode(mode: VizMode) -> &'static [(f32, [u8; 3])] {
    match mode {
        VizMode::TectonicTerrain => &[
            (0.00, [12, 18, 28]),
            (0.30, [32, 64, 78]),
            (0.58, [84, 126, 116]),
            (1.00, [210, 190, 138]),
        ],
        VizMode::LiquidMarble => &[
            (0.00, [18, 12, 34]),
            (0.34, [54, 34, 98]),
            (0.62, [98, 76, 158]),
            (1.00, [198, 228, 248]),
        ],
        VizMode::StormCells => &[
            (0.00, [6, 10, 18]),
            (0.25, [22, 46, 72]),
            (0.62, [52, 114, 188]),
            (1.00, [214, 236, 255]),
        ],
        VizMode::NeonDrift => &[
            (0.00, [10, 8, 22]),
            (0.28, [32, 14, 78]),
            (0.55, [90, 34, 156]),
            (1.00, [56, 228, 194]),
        ],
        VizMode::ShatterMap => &[
            (0.00, [14, 8, 8]),
            (0.30, [68, 22, 30]),
            (0.65, [154, 58, 78]),
            (1.00, [244, 188, 164]),
        ],
        VizMode::TopographicRadar => &[
            (0.00, [6, 18, 14]),
            (0.34, [18, 74, 52]),
            (0.68, [42, 152, 98]),
            (1.00, [186, 255, 214]),
        ],
        VizMode::PulseBulb => &[
            (0.00, [20, 10, 4]),
            (0.34, [120, 62, 18]),
            (0.68, [242, 154, 66]),
            (1.00, [255, 245, 190]),
        ],
        VizMode::TripleBulbs => &[
            (0.00, [8, 8, 12]),
            (0.40, [90, 76, 118]),
            (0.75, [178, 162, 220]),
            (1.00, [240, 236, 255]),
        ],
    }
}

fn gradient_color(stops: &[(f32, [u8; 3])], t: f32) -> [u8; 3] {
    if stops.is_empty() {
        return [255, 255, 255];
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for pair in stops.windows(2) {
        let (a_t, a) = pair[0];
        let (b_t, b) = pair[1];
        if t <= b_t {
            let u = ((t - a_t) / (b_t - a_t).max(1e-6)).clamp(0.0, 1.0);
            return [
                lerp_u8(a[0], b[0], u),
                lerp_u8(a[1], b[1], u),
                lerp_u8(a[2], b[2], u),
            ];
        }
    }
    stops[stops.len() - 1].1
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn scale_u8(v: u8, gain: f32) -> u8 {
    (v as f32 * gain).round().clamp(0.0, 255.0) as u8
}

fn sample_field(field: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
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

fn hash01(x: u32, y: u32, seed: u64) -> f32 {
    let mut v = seed ^ ((x as u64) << 32) ^ y as u64;
    v ^= v >> 33;
    v = v.wrapping_mul(0xff51afd7ed558ccd);
    v ^= v >> 33;
    v = v.wrapping_mul(0xc4ceb9fe1a85ec53);
    v ^= v >> 33;
    (v as u32 as f32) / (u32::MAX as f32)
}
