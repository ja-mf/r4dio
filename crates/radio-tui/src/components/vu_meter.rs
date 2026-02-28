//! VU Meter — Professional audio level meter with segmented LED-style display.
//!
//! Features:
//! - Segmented LED-strip visual style (configurable)
//! - Physics-based inertia for smooth RMS marker movement
//! - Adaptive dB window for optimal dynamic range visibility
//! - Peak hold with decay trail
//! - Three visual presets: Studio (classic), LED (discrete), Analog (needle)
//! - Volume-scaled display (respects system volume)
//! - Smooth sub-cell precision (1/8th blocks)

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app_state::AppState;
use std::sync::Mutex;

// ═════════════════════════════════════════════════════════════════════════════
// PHYSICS-BASED RMS SMOOTHING (Inertia/Gravity)
// ═════════════════════════════════════════════════════════════════════════════

/// Physics state for smooth RMS marker movement.
struct RmsPhysics {
    /// Current smoothed position (0..1)
    position: f32,
    /// Current velocity
    velocity: f32,
    /// Target position
    target: f32,
}

impl RmsPhysics {
    const fn new() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
            target: 0.0,
        }
    }
}

impl RmsPhysics {
    /// Spring constant - higher = snappier, lower = more sluggish
    const SPRING: f32 = 0.15;
    /// Damping factor - prevents oscillation (0..1)
    const DAMPING: f32 = 0.75;
    /// Maximum velocity cap
    const MAX_VELOCITY: f32 = 0.25;

    /// Update physics and return new smoothed position.
    fn update(&mut self, target: f32) -> f32 {
        self.target = target;

        // Spring force toward target
        let displacement = self.target - self.position;
        let force = displacement * Self::SPRING;

        // Update velocity with damping
        self.velocity = (self.velocity + force) * Self::DAMPING;

        // Cap velocity for stability
        self.velocity = self.velocity.clamp(-Self::MAX_VELOCITY, Self::MAX_VELOCITY);

        // Update position
        self.position += self.velocity;

        // Snap to target when very close (prevents micro-jitter)
        if displacement.abs() < 0.001 && self.velocity.abs() < 0.001 {
            self.position = self.target;
            self.velocity = 0.0;
        }

        self.position.clamp(0.0, 1.0)
    }
}

// Global physics state for the RMS marker (one per style to avoid jumps when switching)
static LED_PHYSICS: Mutex<RmsPhysics> = Mutex::new(RmsPhysics::new());
static STUDIO_PHYSICS: Mutex<RmsPhysics> = Mutex::new(RmsPhysics::new());
static ANALOG_PHYSICS: Mutex<RmsPhysics> = Mutex::new(RmsPhysics::new());

fn get_smoothed_rms_position(target_frac: f32, style: MeterStyle) -> f32 {
    let physics = match style {
        MeterStyle::Led => &LED_PHYSICS,
        MeterStyle::Studio => &STUDIO_PHYSICS,
        MeterStyle::Analog => &ANALOG_PHYSICS,
    };

    let mut phys = physics.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    phys.update(target_frac)
}

// ═════════════════════════════════════════════════════════════════════════════
// CONFIGURATION & STYLE PRESETS
// ═════════════════════════════════════════════════════════════════════════════

/// Visual style preset for the VU meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeterStyle {
    /// Classic studio look: continuous bar with fractional precision
    Studio,
    /// LED strip: discrete compact segments with micro-gaps
    Led,
    /// Analog needle feel: pointed marker with tail
    Analog,
}

impl Default for MeterStyle {
    fn default() -> Self {
        MeterStyle::Led
    }
}

/// Character set for rendering different meter styles.
struct MeterChars {
    /// Background/empty track character
    bg: char,
    /// Full block for active segments
    fg: char,
    /// Partial blocks for sub-cell precision [1/8, 2/8, ..., 7/8]
    fractional: &'static [char],
    /// Peak hold marker
    peak: char,
    /// RMS marker morphs by energy: [low, mid, high]
    rms: &'static [char],
    /// Trail/stela characters by distance
    trail: &'static [char],
    /// Gap between LED segments (if applicable)
    segment_gap: Option<char>,
    /// Segment width in characters
    segment_width: usize,
}

impl MeterChars {
    const STUDIO: Self = Self {
        bg: '░',
        fg: '█',
        fractional: &['▏', '▎', '▍', '▌', '▋', '▊', '▉'],
        peak: '▌',
        rms: &['○', '●', '⬤'],
        trail: &['·', '•', '∙'],
        segment_gap: None,
        segment_width: 1,
    };

    /// LED: Compact 1-char segments with hairline gap for tighter packing
    const LED: Self = Self {
        bg: '▏',        // Hairline for inactive
        fg: '▍',        // Medium block for active (compact but visible)
        fractional: &['▏', '▎', '▍', '▌', '▋', '▊', '▉'],
        peak: '▐',      // Right-half at peak
        rms: &['◆', '●', '⬤'],
        trail: &['·', '•', '●'],
        segment_gap: Some(' '),  // Single space for separation
        segment_width: 1,        // Single char = more compact
    };

    /// LED Dense: Even tighter with half-width feel
    const LED_DENSE: Self = Self {
        bg: '│',        // Thin vertical line
        fg: '┃',        // Thick vertical line  
        fractional: &['▏', '▎', '▍', '▌', '▋', '▊', '▉'],
        peak: '▌',
        rms: &['◆', '●', '⬤'],
        trail: &['·', '•', '●'],
        segment_gap: None,       // No gap, segments touch
        segment_width: 1,
    };

    const ANALOG: Self = Self {
        bg: '·',
        fg: '█',
        fractional: &['▁', '▂', '▃', '▄', '▅', '▆', '▇'],
        peak: '▌',
        rms: &['◢', '█', '◣'],
        trail: &['·', '•', '●', '◐', '◑'],
        segment_gap: None,
        segment_width: 1,
    };

    fn for_style(style: MeterStyle) -> Self {
        match style {
            MeterStyle::Studio => Self::STUDIO,
            MeterStyle::Led => Self::LED,
            MeterStyle::Analog => Self::ANALOG,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// COLOR THEME
// ═════════════════════════════════════════════════════════════════════════════

/// Zone colors for the meter (low/mid/high).
struct MeterColors;

impl MeterColors {
    /// Near-black background for low zone
    const LOW_BASE: Color = Color::Rgb(8, 8, 14);
    /// Dark purple for mid zone
    const MID_BASE: Color = Color::Rgb(62, 28, 86);
    /// Dark orange for high zone
    const HIGH_BASE: Color = Color::Rgb(158, 76, 26);
    /// Orange peak marker
    const PEAK_BASE: Color = Color::Rgb(214, 120, 50);
    /// Cool lamp-like RMS marker
    const INSTANT_BASE: Color = Color::Rgb(172, 186, 238);
    /// Background-adjacent empty
    const EMPTY_BASE: Color = Color::Rgb(6, 6, 10);

    /// Get zone color based on position in meter (0.0 = left/quiet, 1.0 = right/loud).
    fn zone_color(position_frac: f32) -> Color {
        let t = position_frac.clamp(0.0, 1.0);
        if t < 0.56 {
            Self::lerp_color(Self::LOW_BASE, Self::MID_BASE, t / 0.56)
        } else {
            Self::lerp_color(Self::MID_BASE, Self::HIGH_BASE, (t - 0.56) / 0.44)
        }
    }

    /// Get fill color with energy-based brightness.
    fn fill_color(position_frac: f32, energy: f32) -> Color {
        let (r, g, b) = match Self::zone_color(position_frac) {
            Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
            _ => (120.0, 120.0, 120.0),
        };

        let heat = ((position_frac * 54.0 - 54.0) / 54.0).clamp(0.0, 1.0);
        let brightness = 0.26 + 0.62 * energy;
        let glow = 0.04 + 0.24 * Self::smoothstep(heat) * (0.28 + 0.72 * energy);
        let ember = 0.10 + 0.90 * heat;

        Color::Rgb(
            (r * brightness + (42.0 + 104.0 * ember) * glow)
                .round()
                .min(255.0) as u8,
            (g * brightness + (22.0 + 56.0 * ember) * glow)
                .round()
                .min(255.0) as u8,
            (b * brightness + (78.0 - 30.0 * heat) * glow)
                .round()
                .min(255.0) as u8,
        )
    }

    /// Peak marker color with energy boost.
    fn peak_color(energy: f32) -> Color {
        let (r, g, b) = match Self::PEAK_BASE {
            Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
            _ => (210.0, 80.0, 60.0),
        };
        let boost = 0.52 + 0.48 * energy;
        Color::Rgb(
            (r * boost + 34.0 * energy).round().min(255.0) as u8,
            (g * boost + 20.0 * energy).round().min(255.0) as u8,
            (b * boost + 5.0 * energy).round().min(255.0) as u8,
        )
    }

    /// RMS/instant marker color.
    fn instant_color(energy: f32) -> Color {
        let (r, g, b) = match Self::INSTANT_BASE {
            Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
            _ => (172.0, 186.0, 238.0),
        };
        let boost = 0.46 + 0.54 * energy;
        Color::Rgb(
            (r * boost + 28.0 * energy).round().min(255.0) as u8,
            (g * boost + 18.0 * energy).round().min(255.0) as u8,
            (b * boost + 22.0 * energy).round().min(255.0) as u8,
        )
    }

    /// Stela trail color with distance fade.
    fn trail_color(energy: f32, distance_frac: f32) -> Color {
        let (r, g, b) = match Self::INSTANT_BASE {
            Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
            _ => (172.0, 186.0, 238.0),
        };

        let fade = (1.0 - distance_frac).clamp(0.0, 1.0);
        let intensity = fade * (0.3 + 0.7 * energy);

        Color::Rgb(
            ((r * 0.8 + 40.0) * intensity).round().min(255.0) as u8,
            ((g * 0.85 + 30.0) * intensity).round().min(255.0) as u8,
            ((b * 0.9 + 60.0) * intensity).round().min(255.0) as u8,
        )
    }

    /// Empty/background color with subtle energy lift.
    fn empty_color(energy: f32) -> Color {
        let (r, g, b) = match Self::EMPTY_BASE {
            Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
            _ => (28.0, 28.0, 38.0),
        };
        let lift = 0.03 + 0.12 * energy;
        Color::Rgb(
            (r + 20.0 * lift).round().min(255.0) as u8,
            (g + 18.0 * lift).round().min(255.0) as u8,
            (b + 26.0 * lift).round().min(255.0) as u8,
        )
    }

    fn smoothstep(v: f32) -> f32 {
        let x = v.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }

    fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
        let tt = t.clamp(0.0, 1.0);
        (a as f32 + (b as f32 - a as f32) * tt).round() as u8
    }

    fn lerp_color(a: Color, b: Color, t: f32) -> Color {
        let (ar, ag, ab) = match a {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (0, 0, 0),
        };
        let (br, bg, bb) = match b {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (0, 0, 0),
        };
        Color::Rgb(
            Self::lerp_u8(ar, br, t),
            Self::lerp_u8(ag, bg, t),
            Self::lerp_u8(ab, bb, t),
        )
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// dB CALCULATIONS
// ═════════════════════════════════════════════════════════════════════════════

/// Fixed dB range for the meter display.
const DB_MIN: f32 = -54.0;
const DB_MAX: f32 = 0.0;
const DB_RANGE: f32 = DB_MAX - DB_MIN;

/// Perceptual gamma for low-level detail.
const GAMMA: f32 = 0.72;

/// Convert dB to normalized 0..1 position.
fn db_to_frac(db: f32) -> f32 {
    let linear = ((db - DB_MIN) / DB_RANGE).clamp(0.0, 1.0);
    linear.powf(GAMMA)
}

/// Convert normalized position to dB.
#[allow(dead_code)]
fn frac_to_db(frac: f32) -> f32 {
    let linear = frac.powf(1.0 / GAMMA);
    DB_MIN + linear * DB_RANGE
}

/// Calculate effective dB levels considering system volume.
fn calculate_effective_levels(state: &AppState) -> (f32, f32, f32) {
    let ds = &state.daemon_state;

    let volume_db = if ds.volume <= 0.0 {
        -90.0_f32
    } else {
        20.0 * ds.volume.log10()
    };

    let effective_vu = if ds.volume <= 0.0 {
        -90.0_f32
    } else {
        (state.vu_level + volume_db).clamp(-90.0, 0.0)
    };

    let effective_peak = if ds.volume <= 0.0 {
        -90.0_f32
    } else {
        (state.peak_level + volume_db).clamp(-90.0, 0.0)
    };

    let instant_db = (0.78 * state.audio_level + 0.22 * state.vu_level).clamp(-90.0, 0.0);
    let effective_instant = if ds.volume <= 0.0 {
        -90.0_f32
    } else {
        (instant_db + volume_db).clamp(-90.0, 0.0)
    };

    (effective_vu, effective_peak, effective_instant)
}

/// Calculate energy level 0..1 from dB.
fn calculate_energy(db: f32) -> f32 {
    ((db + 72.0) / 72.0).clamp(0.0, 1.0).powf(0.75)
}

// ═════════════════════════════════════════════════════════════════════════════
// RMS MARKER & TRAIL
// ═════════════════════════════════════════════════════════════════════════════

/// Select RMS marker character based on energy level.
fn select_rms_char(energy: f32, chars: &'static [char]) -> char {
    if energy < 0.33 {
        chars.get(0).copied().unwrap_or('●')
    } else if energy < 0.66 {
        chars.get(1).copied().unwrap_or('●')
    } else {
        chars.get(2).copied().unwrap_or('⬤')
    }
}

/// Select trail/stela character based on distance from marker.
fn select_trail_char(distance: f32, chars: &'static [char]) -> char {
    let idx = (distance * chars.len() as f32) as usize;
    chars.get(idx).copied().unwrap_or('·')
}

// ═════════════════════════════════════════════════════════════════════════════
// METER BUILDERS
// ═════════════════════════════════════════════════════════════════════════════

/// Build a VU meter line with the specified style.
pub fn build_meter(
    vu_db: f32,
    peak_db: f32,
    instant_db: f32,
    width: usize,
    style: MeterStyle,
) -> Line<'static> {
    if width == 0 {
        return Line::from(vec![]);
    }

    match style {
        MeterStyle::Studio => build_studio_meter(vu_db, peak_db, instant_db, width),
        MeterStyle::Led => build_led_meter(vu_db, peak_db, instant_db, width),
        MeterStyle::Analog => build_analog_meter(vu_db, peak_db, instant_db, width),
    }
}

/// Studio-style meter: continuous bar with fractional precision.
fn build_studio_meter(vu_db: f32, peak_db: f32, instant_db: f32, width: usize) -> Line<'static> {
    let chars = MeterChars::STUDIO;
    let rms_frac = db_to_frac(vu_db);
    let peak_frac = db_to_frac(peak_db);
    let instant_frac = db_to_frac(instant_db);
    let energy = calculate_energy(vu_db);

    // Apply physics smoothing to RMS position
    let smoothed_rms_frac = get_smoothed_rms_position(rms_frac, MeterStyle::Studio);

    let total_eighths = (smoothed_rms_frac * width as f32 * 8.0) as usize;
    let full_cells = total_eighths / 8;
    let partial = total_eighths % 8;
    let peak_cell = ((peak_frac * width as f32) as usize).min(width.saturating_sub(1));
    
    // Instant marker also smoothed but less so
    let smoothed_instant_frac = get_smoothed_rms_position(instant_frac, MeterStyle::Studio);
    let instant_pos = smoothed_instant_frac * width as f32;
    let instant_cell = (instant_pos as usize).min(width.saturating_sub(1));

    let rms_char = select_rms_char(energy, chars.rms);
    let stela_length = ((energy * 2.0).ceil() as usize).max(1);
    let stela_start = instant_cell.saturating_add(1);
    let stela_end = (stela_start + stela_length).min(width);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_color: Option<Color> = None;
    let mut current_str = String::new();

    let flush = |spans: &mut Vec<Span<'static>>, color: Color, s: String| {
        if !s.is_empty() {
            spans.push(Span::styled(s, Style::default().fg(color)));
        }
    };

    for i in 0..width {
        let screen_frac = (i as f32 + 0.5) / width as f32;
        let linear = screen_frac.powf(1.0 / GAMMA);
        let _db_here = DB_MIN + linear * DB_RANGE;

        let is_peak = i == peak_cell && peak_db > DB_MIN + 1.0;
        let is_instant = i == instant_cell && instant_db > DB_MIN + 1.0;
        let is_trail = i >= stela_start && i < stela_end && i > full_cells && instant_db > DB_MIN + 1.0;
        let stela_distance = if is_trail {
            (i - instant_cell) as f32 / stela_length as f32
        } else {
            1.0
        };

        let fill_color = MeterColors::fill_color(screen_frac, energy);
        let peak_color = MeterColors::peak_color(energy);
        let instant_color = MeterColors::instant_color(energy);
        let empty_color = MeterColors::empty_color(energy);

        let (ch, color) = if is_peak {
            (chars.peak, peak_color)
        } else if is_instant {
            (rms_char, instant_color)
        } else if is_trail {
            let trail_char = select_trail_char(stela_distance, chars.trail);
            let trail_color = MeterColors::trail_color(energy, stela_distance);
            (trail_char, trail_color)
        } else if i < full_cells {
            (chars.fg, fill_color)
        } else if i == full_cells && partial > 0 {
            let frac_char = chars.fractional.get(partial - 1).copied().unwrap_or(chars.fg);
            (frac_char, fill_color)
        } else {
            (chars.bg, empty_color)
        };

        if current_color == Some(color) {
            current_str.push(ch);
        } else {
            if let Some(c) = current_color.take() {
                flush(&mut spans, c, current_str.clone());
                current_str.clear();
            }
            current_color = Some(color);
            current_str.push(ch);
        }
    }

    if let Some(c) = current_color {
        flush(&mut spans, c, current_str);
    }

    Line::from(spans)
}

/// LED-style meter: compact discrete segments with micro-gaps.
fn build_led_meter(vu_db: f32, peak_db: f32, instant_db: f32, width: usize) -> Line<'static> {
    let chars = MeterChars::LED;
    let rms_frac = db_to_frac(vu_db);
    let peak_frac = db_to_frac(peak_db);
    let instant_frac = db_to_frac(instant_db);
    let energy = calculate_energy(vu_db);

    // Apply physics smoothing
    let smoothed_rms_frac = get_smoothed_rms_position(rms_frac, MeterStyle::Led);
    let smoothed_instant_frac = get_smoothed_rms_position(instant_frac, MeterStyle::Led);

    // Compact segment configuration: 1 char + optional gap
    let seg_width = chars.segment_width;
    let gap_width = if chars.segment_gap.is_some() { 1 } else { 0 };
    let unit_width = seg_width + gap_width;
    let num_segments = width / unit_width;

    let lit_segments = (smoothed_rms_frac * num_segments as f32) as usize;
    let peak_segment = ((peak_frac * num_segments as f32) as usize).min(num_segments.saturating_sub(1));
    let instant_segment = ((smoothed_instant_frac * num_segments as f32) as usize).min(num_segments.saturating_sub(1));

    let rms_char = select_rms_char(energy, chars.rms);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_color: Option<Color> = None;
    let mut current_str = String::new();

    let flush = |spans: &mut Vec<Span<'static>>, color: Color, s: String| {
        if !s.is_empty() {
            spans.push(Span::styled(s, Style::default().fg(color)));
        }
    };

    for seg in 0..num_segments {
        let seg_frac = seg as f32 / num_segments as f32;
        let is_lit = seg < lit_segments;
        let is_peak = seg == peak_segment && peak_db > DB_MIN + 1.0;
        let is_instant = seg == instant_segment && instant_db > DB_MIN + 1.0;

        let fill_color = MeterColors::fill_color(seg_frac, energy);
        let peak_color = MeterColors::peak_color(energy);
        let instant_color = MeterColors::instant_color(energy);
        let empty_color = MeterColors::empty_color(energy);

        // Draw segment (compact single-char)
        for seg_col in 0..seg_width {
            let (ch, color) = if is_peak && seg_col == seg_width - 1 {
                (chars.peak, peak_color)
            } else if is_instant && seg_col == seg_width / 2 {
                (rms_char, instant_color)
            } else if is_lit {
                (chars.fg, fill_color)
            } else {
                (chars.bg, empty_color)
            };

            if current_color == Some(color) {
                current_str.push(ch);
            } else {
                if let Some(c) = current_color.take() {
                    flush(&mut spans, c, current_str.clone());
                    current_str.clear();
                }
                current_color = Some(color);
                current_str.push(ch);
            }
        }

        // Add micro-gap between segments
        if seg < num_segments - 1 {
            if let Some(gap_char) = chars.segment_gap {
                if current_color == Some(empty_color) {
                    current_str.push(gap_char);
                } else {
                    if let Some(c) = current_color.take() {
                        flush(&mut spans, c, current_str.clone());
                        current_str.clear();
                    }
                    current_color = Some(empty_color);
                    current_str.push(gap_char);
                }
            }
        }
    }

    // Handle remaining width (if any)
    let used_width = num_segments * unit_width;
    for _ in used_width..width {
        if current_color == Some(MeterColors::empty_color(energy)) {
            current_str.push(chars.bg);
        } else {
            if let Some(c) = current_color.take() {
                flush(&mut spans, c, current_str.clone());
                current_str.clear();
            }
            current_color = Some(MeterColors::empty_color(energy));
            current_str.push(chars.bg);
        }
    }

    if let Some(c) = current_color {
        flush(&mut spans, c, current_str);
    }

    Line::from(spans)
}

/// Analog-style meter: needle marker with tail effect.
fn build_analog_meter(vu_db: f32, peak_db: f32, instant_db: f32, width: usize) -> Line<'static> {
    let chars = MeterChars::ANALOG;
    let rms_frac = db_to_frac(vu_db);
    let peak_frac = db_to_frac(peak_db);
    let instant_frac = db_to_frac(instant_db);
    let energy = calculate_energy(vu_db);

    // Apply physics smoothing
    let smoothed_rms_frac = get_smoothed_rms_position(rms_frac, MeterStyle::Analog);
    let smoothed_instant_frac = get_smoothed_rms_position(instant_frac, MeterStyle::Analog);

    let needle_pos = (smoothed_rms_frac * width as f32) as usize;
    let peak_pos = ((peak_frac * width as f32) as usize).min(width.saturating_sub(1));
    let instant_pos = (smoothed_instant_frac * width as f32) as usize;

    // Trail length depends on energy
    let trail_len = ((energy * 4.0).ceil() as usize).min(needle_pos);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_color: Option<Color> = None;
    let mut current_str = String::new();

    let flush = |spans: &mut Vec<Span<'static>>, color: Color, s: String| {
        if !s.is_empty() {
            spans.push(Span::styled(s, Style::default().fg(color)));
        }
    };

    for i in 0..width {
        let screen_frac = (i as f32 + 0.5) / width as f32;
        let linear = screen_frac.powf(1.0 / GAMMA);
        let _db_here = DB_MIN + linear * DB_RANGE;

        let is_peak = i == peak_pos && peak_db > DB_MIN + 1.0;
        let is_needle = i == needle_pos;
        let is_instant = i == instant_pos && instant_db > DB_MIN + 1.0;

        // Trail behind needle
        let trail_dist = if i < needle_pos {
            let dist = (needle_pos - i) as f32 / trail_len.max(1) as f32;
            if dist < 1.0 { Some(dist) } else { None }
        } else {
            None
        };

        let fill_color = MeterColors::fill_color(screen_frac, energy);
        let peak_color = MeterColors::peak_color(energy);
        let instant_color = MeterColors::instant_color(energy);
        let empty_color = MeterColors::empty_color(energy);

        let (ch, color) = if is_peak {
            (chars.peak, peak_color)
        } else if is_needle {
            let needle_idx = (energy * chars.rms.len() as f32) as usize;
            let ch = chars.rms.get(needle_idx).copied().unwrap_or('█');
            (ch, instant_color)
        } else if let Some(dist) = trail_dist {
            let trail_idx = (dist * chars.trail.len() as f32) as usize;
            let ch = chars.trail.get(trail_idx).copied().unwrap_or('·');
            let color = MeterColors::trail_color(energy, dist);
            (ch, color)
        } else if is_instant {
            (chars.fg, instant_color)
        } else {
            (chars.bg, empty_color)
        };

        if current_color == Some(color) {
            current_str.push(ch);
        } else {
            if let Some(c) = current_color.take() {
                flush(&mut spans, c, current_str.clone());
                current_str.clear();
            }
            current_color = Some(color);
            current_str.push(ch);
        }
    }

    if let Some(c) = current_color {
        flush(&mut spans, c, current_str);
    }

    Line::from(spans)
}

// ═════════════════════════════════════════════════════════════════════════════
// PUBLIC API
// ═════════════════════════════════════════════════════════════════════════════

/// Draw a VU meter into the specified area.
///
/// This is the main entry point for rendering a VU meter. It calculates
/// effective levels from app state and renders using the specified style.
pub fn draw_vu_meter(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    is_playing: bool,
    style: MeterStyle,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (vu_db, peak_db, instant_db) = if is_playing {
        calculate_effective_levels(state)
    } else {
        (-90.0, -90.0, -90.0)
    };

    let meter_line = build_meter(vu_db, peak_db, instant_db, area.width as usize, style);
    frame.render_widget(Paragraph::new(meter_line), area);
}

/// Get a simple meter line without full state (for testing/custom use).
#[allow(dead_code)]
pub fn get_meter_line(
    vu_db: f32,
    peak_db: f32,
    instant_db: f32,
    width: usize,
    style: MeterStyle,
) -> Line<'static> {
    build_meter(vu_db, peak_db, instant_db, width, style)
}

/// Default meter style for the application.
pub fn default_style() -> MeterStyle {
    MeterStyle::Led
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_conversions() {
        assert_eq!(db_to_frac(-54.0), 0.0);
        assert_eq!(db_to_frac(0.0), 1.0);
        assert!((db_to_frac(-27.0) - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_meter_building() {
        let line = build_meter(-20.0, -10.0, -15.0, 20, MeterStyle::Led);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_energy_calculation() {
        assert_eq!(calculate_energy(-72.0), 0.0);
        assert_eq!(calculate_energy(0.0), 1.0);
    }

    #[test]
    fn test_physics_smoothing() {
        let mut phys = RmsPhysics::default();
        
        // Should start at 0 and move toward target
        let pos1 = phys.update(1.0);
        assert!(pos1 > 0.0 && pos1 < 1.0);
        
        // Should continue approaching target
        let pos2 = phys.update(1.0);
        assert!(pos2 > pos1);
        
        // Should eventually reach target
        let mut pos = pos2;
        for _ in 0..100 {
            pos = phys.update(1.0);
        }
        assert!((pos - 1.0).abs() < 0.01);
    }
}
