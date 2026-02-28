//! VU Meter — Professional audio level meter with segmented LED-style display.
//!
//! Features:
//! - Segmented LED-strip visual style (configurable)
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

// ═════════════════════════════════════════════════════════════════════════════
// CONFIGURATION & STYLE PRESETS
// ═════════════════════════════════════════════════════════════════════════════

/// Visual style preset for the VU meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeterStyle {
    /// Classic studio look: continuous bar with fractional precision
    Studio,
    /// LED strip: discrete segments with gaps
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
    };

    const LED: Self = Self {
        bg: '┆',
        fg: '▮',
        fractional: &['▁', '▂', '▃', '▄', '▅', '▆', '▇'],
        peak: '▐',
        rms: &['◆', '●', '⬤'],
        trail: &['·', '•', '●'],
        segment_gap: Some(' '),
    };

    const ANALOG: Self = Self {
        bg: '·',
        fg: '█',
        fractional: &['▁', '▂', '▃', '▄', '▅', '▆', '▇'],
        peak: '▌',
        rms: &['◢', '█', '◣'],
        trail: &['·', '•', '●', '◐', '◑'],
        segment_gap: None,
    };

    fn for_style(style: MeterStyle) -> Self {
        match style {
            MeterStyle::Studio => Self::STUDIO,
            MeterStyle::Led => Self::LED,
            MeterStyle::Analog => Self::ANALOG,
        }
    }
}

/// dB scale markers for professional reference points.
#[derive(Debug, Clone)]
struct ScaleMarkers;

impl ScaleMarkers {
    /// Get scale marker character and position for a given dB value.
    /// Returns (db_value, marker_char, label_opt).
    const MARKERS: [(f32, char, Option<&'static str>); 5] = [
        (-54.0, '│', Some("∞")),  // Silence floor
        (-36.0, '├', None),       // -36dB (12.5%)
        (-18.0, '┼', Some("18")), // -18dB reference (50%)
        (-6.0, '┤', Some("6")),   // -6dB (75%)
        (0.0, '│', Some("0")),    // 0dB clip point
    ];

    /// Check if a dB value is near a scale marker (within tolerance).
    fn is_near_marker(db: f32, tolerance_db: f32) -> Option<(char, Option<&'static str>)> {
        for (marker_db, ch, label) in Self::MARKERS {
            if (db - marker_db).abs() < tolerance_db {
                return Some((ch, label));
            }
        }
        None
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

    let total_eighths = (rms_frac * width as f32 * 8.0) as usize;
    let full_cells = total_eighths / 8;
    let partial = total_eighths % 8;
    let peak_cell = ((peak_frac * width as f32) as usize).min(width.saturating_sub(1));
    let instant_pos = instant_frac * width as f32;
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
        let db_here = DB_MIN + linear * DB_RANGE;

        let is_peak = i == peak_cell && peak_db > DB_MIN + 1.0;
        let is_instant = i == instant_cell && instant_db > DB_MIN + 1.0;
        let is_trail = i >= stela_start && i < stela_end && i > full_cells && instant_db > DB_MIN + 1.0;
        let stela_distance = if is_trail {
            (i - instant_cell) as f32 / stela_length as f32
        } else {
            1.0
        };

        let zone_color = MeterColors::zone_color(screen_frac);
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

/// LED-style meter: discrete segments with gaps.
fn build_led_meter(vu_db: f32, peak_db: f32, instant_db: f32, width: usize) -> Line<'static> {
    let chars = MeterChars::LED;
    let rms_frac = db_to_frac(vu_db);
    let peak_frac = db_to_frac(peak_db);
    let instant_frac = db_to_frac(instant_db);
    let energy = calculate_energy(vu_db);

    // LED segment configuration
    const SEGMENT_WIDTH: usize = 2;
    const SEGMENT_GAP: usize = 1;
    let total_segment_units = SEGMENT_WIDTH + SEGMENT_GAP;
    let num_segments = width / total_segment_units;
    let lit_segments = (rms_frac * num_segments as f32) as usize;
    let peak_segment = ((peak_frac * num_segments as f32) as usize).min(num_segments.saturating_sub(1));
    let instant_segment = ((instant_frac * num_segments as f32) as usize).min(num_segments.saturating_sub(1));

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

        let zone_color = MeterColors::zone_color(seg_frac);
        let fill_color = MeterColors::fill_color(seg_frac, energy);
        let peak_color = MeterColors::peak_color(energy);
        let instant_color = MeterColors::instant_color(energy);
        let empty_color = MeterColors::empty_color(energy);

        // Draw segment
        for seg_col in 0..SEGMENT_WIDTH {
            let (ch, color) = if is_peak && seg_col == SEGMENT_WIDTH - 1 {
                (chars.peak, peak_color)
            } else if is_instant && seg_col == SEGMENT_WIDTH / 2 {
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

        // Add gap between segments
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

    let needle_pos = (rms_frac * width as f32) as usize;
    let peak_pos = ((peak_frac * width as f32) as usize).min(width.saturating_sub(1));
    let instant_pos = (instant_frac * width as f32) as usize;

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
        let db_here = DB_MIN + linear * DB_RANGE;

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

        let zone_color = MeterColors::zone_color(screen_frac);
        let fill_color = MeterColors::fill_color(screen_frac, energy);
        let peak_color = MeterColors::peak_color(energy);
        let instant_color = MeterColors::instant_color(energy);
        let empty_color = MeterColors::empty_color(energy);

        let (ch, color) = if is_peak {
            (chars.peak, peak_color)
        } else if is_needle {
            // Needle head
            let needle_idx = (energy * chars.rms.len() as f32) as usize;
            let ch = chars.rms.get(needle_idx).copied().unwrap_or('█');
            (ch, instant_color)
        } else if let Some(dist) = trail_dist {
            // Trail fades with distance
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
        assert!((db_to_frac(-27.0) - 0.5).abs() < 0.1); // Approx midpoint
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
}
