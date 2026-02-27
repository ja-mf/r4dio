//! Header component — 2-row top bar.
//!
//! Row 1: now-playing station/file, ICY/show title, location, health badge.
//! Row 2: VU meter (left half) | seek bar + position (right half, file only) | volume %.
//!
//! Not focusable; draws to a 2-row area.

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use radio_proto::protocol::{MpvHealth, PlaybackStatus};

use crate::{
    action::{Action, ComponentId},
    app_state::AppState,
    component::Component,
    intent::RenderHint,
    theme::{
        C_ACCENT, C_BADGE_ERR, C_BADGE_PENDING, C_CONNECTING, C_MUTED, C_NETWORK, C_PLAYING,
        C_SECONDARY, C_TAG,
    },
};

// ── VU meter colours ──────────────────────────────────────────────────────────

const C_METER_LOW: Color = Color::Rgb(8, 8, 14); // near-black
const C_METER_MID: Color = Color::Rgb(62, 28, 86); // dark purple
const C_METER_HIGH: Color = Color::Rgb(158, 76, 26); // dark orange
const C_METER_PEAK: Color = Color::Rgb(214, 120, 50); // orange peak marker
const C_METER_INSTANT: Color = Color::Rgb(172, 186, 238); // cool lamp-like RMS marker
const C_METER_EMPTY: Color = Color::Rgb(6, 6, 10); // background-adjacent

fn title_lamp_level(state: &AppState) -> f32 {
    // Adaptive dB window from long-term signal statistics.
    let spread = state.meter_spread_db.clamp(2.0, 22.0);
    let mut floor = (state.meter_mean_db - spread * 3.0).clamp(-90.0, -20.0);
    let ceil = (state.meter_mean_db + spread * 1.8).clamp(-28.0, -1.0);
    if ceil - floor < 14.0 {
        floor = (ceil - 14.0).max(-90.0);
    }

    // Mostly low-passed body (vu_level), with a touch of fast and peak energy.
    let fast_db = state.audio_level.max(-90.0);
    let smooth_db = state.vu_level.max(-90.0);
    let peak_db = state.peak_level.max(smooth_db);
    let body_db = (0.14 * fast_db + 0.72 * smooth_db + 0.14 * peak_db).max(-90.0);

    let mut t = ((body_db - floor) / (ceil - floor)).clamp(0.0, 1.0);
    let adaptive_gain = (6.0 / spread).clamp(0.85, 1.45);
    t = ((t - 0.5) * adaptive_gain + 0.5).clamp(0.0, 1.0);
    smoothstep01(t).powf(0.82)
}

fn title_bulb_color(state: &AppState) -> Color {
    let level = title_lamp_level(state);
    let scale = 0.01 + 0.99 * level;

    Color::Rgb(
        (8.0 + 232.0 * scale).round().min(255.0) as u8,
        (8.0 + 232.0 * scale).round().min(255.0) as u8,
        (12.0 + 244.0 * scale).round().min(255.0) as u8,
    )
}

fn smoothstep01(v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn title_text_color(state: &AppState) -> Color {
    let level = title_lamp_level(state);
    let scale = 0.02 + 0.98 * level;
    Color::Rgb(
        (6.0 + 210.0 * scale).round().min(255.0) as u8,
        (7.0 + 198.0 * scale).round().min(255.0) as u8,
        (12.0 + 236.0 * scale).round().min(255.0) as u8,
    )
}

pub struct Header;

impl Header {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Header {
    fn id(&self) -> ComponentId {
        ComponentId::StationList
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &AppState) -> Vec<Action> {
        vec![]
    }

    fn handle_mouse(&mut self, _event: MouseEvent, _area: Rect, _state: &AppState) -> Vec<Action> {
        vec![]
    }

    fn on_action(&mut self, _action: &Action, _state: &AppState) -> Vec<Action> {
        vec![]
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, _focused: bool, state: &AppState) {
        if area.height < 2 {
            // Fallback: single-row, just draw row1 in whatever space we have
            frame.render_widget(Clear, area);
            let line = build_row1(state);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area);

        // Row 1: now-playing info
        frame.render_widget(Clear, rows[0]);
        frame.render_widget(Paragraph::new(build_row1(state)), rows[0]);

        // Row 2: meter | seek bar | volume
        draw_row2(frame, rows[1], state);
    }
}

// ── Row 1: station / file / show / location ───────────────────────────────────

fn build_row1(state: &AppState) -> Line<'static> {
    let ds = &state.daemon_state;

    let health_span: Option<Span<'static>> = match &ds.mpv_health {
        MpvHealth::Degraded(reason) => Some(Span::styled(
            format!(" [DEGD: {}]", reason),
            Style::default().fg(C_BADGE_ERR),
        )),
        MpvHealth::Dead => Some(Span::styled(
            " [mpv DEAD]".to_string(),
            Style::default()
                .fg(C_BADGE_ERR)
                .add_modifier(Modifier::BOLD),
        )),
        MpvHealth::Restarting => Some(Span::styled(
            " [mpv restarting…]".to_string(),
            Style::default().fg(C_BADGE_PENDING),
        )),
        MpvHealth::Starting => Some(Span::styled(
            " [mpv starting…]".to_string(),
            Style::default().fg(C_BADGE_PENDING),
        )),
        _ => None,
    };

    if let Some(path) = ds.current_file.as_ref() {
        // ── File mode ─────────────────────────────────────────────────────────
        let looks_paused =
            ds.time_pos_secs.is_some() && ds.playback_status == PlaybackStatus::Connecting;

        let (base_icon, base_icon_color): (&str, Color) = if looks_paused {
            ("⏸", C_CONNECTING)
        } else {
            match ds.playback_status {
                PlaybackStatus::Playing => ("▶", C_PLAYING),
                PlaybackStatus::Paused => ("⏸", C_CONNECTING),
                PlaybackStatus::Connecting => ("◔", C_CONNECTING),
                PlaybackStatus::Error => ("⛔", C_ACCENT),
                PlaybackStatus::Idle => ("■", C_MUTED),
            }
        };

        let (icon, icon_color) = match state.pause_hint {
            RenderHint::PendingHidden => (" ", base_icon_color),
            RenderHint::PendingVisible => (base_icon, C_BADGE_PENDING),
            RenderHint::TimedOut => ("?", C_BADGE_ERR),
            RenderHint::Normal => (base_icon, base_icon_color),
        };

        let file_name = std::path::Path::new(path.as_str())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();

        let mut spans: Vec<Span> = vec![
            Span::raw(" "),
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::raw(" "),
            Span::styled("📼 ", Style::default().fg(C_MUTED)),
            Span::styled(
                file_name,
                Style::default()
                    .fg(title_text_color(state))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "●",
                Style::default()
                    .fg(title_bulb_color(state))
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if let Some(hs) = health_span {
            spans.push(hs);
        }
        Line::from(spans)
    } else if let Some(idx) = ds.current_station {
        // ── Station mode ──────────────────────────────────────────────────────
        if let Some(station) = ds.stations.get(idx) {
            let (base_icon, base_icon_color): (&str, Color) = match ds.playback_status {
                PlaybackStatus::Playing => ("▶", C_PLAYING),
                PlaybackStatus::Paused => ("⏸", C_CONNECTING),
                PlaybackStatus::Connecting => ("◔", C_CONNECTING),
                PlaybackStatus::Error => ("⛔", C_ACCENT),
                PlaybackStatus::Idle => ("■", C_MUTED),
            };

            let pause_or_station_hint = if state.pause_hint != RenderHint::Normal {
                state.pause_hint
            } else {
                state.station_hint
            };
            let (icon, icon_color) = match pause_or_station_hint {
                RenderHint::PendingHidden => (" ", base_icon_color),
                RenderHint::PendingVisible => (base_icon, C_BADGE_PENDING),
                RenderHint::TimedOut => ("?", C_BADGE_ERR),
                RenderHint::Normal => (base_icon, base_icon_color),
            };

            let mut spans: Vec<Span> = vec![
                Span::raw(" "),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw(" "),
                Span::styled("📻 ", Style::default().fg(C_MUTED)),
                Span::styled(
                    station.name.clone(),
                    Style::default()
                        .fg(title_text_color(state))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    "●",
                    Style::default()
                        .fg(title_bulb_color(state))
                        .add_modifier(Modifier::BOLD),
                ),
            ];

            // City (no timezone, just label)
            if !station.city.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", station.city),
                    Style::default().fg(C_MUTED),
                ));
            }

            // Show title: prefer NTS show name, then ICY
            let show_text: Option<String> = match station.name.as_str() {
                "NTS 1" => state
                    .nts_ch1
                    .as_ref()
                    .map(|ch| ch.now.broadcast_title.clone()),
                "NTS 2" => state
                    .nts_ch2
                    .as_ref()
                    .map(|ch| ch.now.broadcast_title.clone()),
                _ => None,
            };
            let show_text = show_text.or_else(|| {
                state
                    .daemon_state
                    .icy_title
                    .clone()
                    .filter(|s| !s.is_empty())
            });
            if let Some(text) = show_text {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(text, Style::default().fg(C_NETWORK)));
            }

            if let Some(hs) = health_span {
                spans.push(hs);
            }
            Line::from(spans)
        } else {
            idle_line()
        }
    } else {
        idle_line()
    }
}

// ── Row 2: VU meter | seek bar ───────────────────────────────────────────────
// Volume % is no longer shown here; it was replaced by the oscilloscope in the
// header right half.  The VU meter is rendered on a fixed dBFS scale and
// scaled by current volume so muted -> flat bar.

fn draw_row2(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width < 10 {
        return;
    }

    let ds = &state.daemon_state;
    let is_playing = matches!(
        ds.playback_status,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Connecting
    );
    let has_file = ds.current_file.is_some();
    let has_seek = has_file && ds.time_pos_secs.is_some();

    // Scale meter by volume: volume=0 -> silence, volume=1 -> full level.
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

    let (meter_area, seek_area) = if has_seek && area.width >= 20 {
        let meter_w = (area.width * 4) / 10;
        let seek_w = area.width - meter_w;
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(meter_w), Constraint::Length(seek_w)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    // ── VU meter ──────────────────────────────────────────────────────────────
    let meter_line = if is_playing {
        build_meter(
            effective_vu,
            effective_peak,
            effective_instant,
            meter_area.width as usize,
        )
    } else {
        build_meter(-90.0, -90.0, -90.0, meter_area.width as usize)
    };
    frame.render_widget(Paragraph::new(meter_line), meter_area);

    // ── Seek bar (file only) ──────────────────────────────────────────────────
    if let Some(seek_area) = seek_area {
        if let Some(pos) = ds.time_pos_secs {
            let dur = ds.duration_secs.unwrap_or(0.0).max(0.0);
            let w = seek_area.width as usize;
            let label = format!(
                " {}/{}",
                fmt_clock(pos),
                if dur > 0.0 {
                    fmt_clock(dur)
                } else {
                    "--:--".to_string()
                }
            );
            let label_w = label.len();
            let bar_w = w.saturating_sub(label_w + 1);
            let bar = smooth_bar(pos, dur, bar_w);

            let seek_line = Line::from(vec![
                Span::raw(" "),
                Span::styled(bar, Style::default().fg(C_PLAYING)),
                Span::styled(label, Style::default().fg(C_TAG)),
            ]);
            frame.render_widget(Paragraph::new(seek_line), seek_area);
        }
    }
}

// ── VU meter bar builder ──────────────────────────────────────────────────────

/// Build a coloured VU-meter line on a fixed dBFS scale.
/// Includes a peak marker and a smooth near-instant RMS marker.
fn build_meter(vu_db: f32, peak_db: f32, instant_db: f32, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from(vec![]);
    }

    const DB_MIN: f32 = -54.0;
    const DB_MAX: f32 = 0.0;
    const DB_RANGE: f32 = DB_MAX - DB_MIN;

    const BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    const MARKERS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

    // Perceptual warp; near-linear feel with a bit more detail at low levels.
    const GAMMA: f32 = 0.72;

    // Map dB value -> 0..1 position with fixed scale + gamma.
    let db_to_frac = |db: f32| -> f32 {
        let linear = ((db - DB_MIN) / DB_RANGE).clamp(0.0, 1.0);
        linear.powf(GAMMA)
    };

    let rms_frac = db_to_frac(vu_db);
    let peak_frac = db_to_frac(peak_db);
    let instant_frac = db_to_frac(instant_db);
    let energy = ((vu_db + 72.0) / 72.0).clamp(0.0, 1.0).powf(0.75);

    let total_eighths = (rms_frac * width as f32 * 8.0) as usize;
    let full_cells = total_eighths / 8;
    let partial = total_eighths % 8;
    let peak_cell = ((peak_frac * width as f32) as usize).min(width.saturating_sub(1));
    let instant_pos = instant_frac * width as f32;
    let instant_cell = (instant_pos as usize).min(width.saturating_sub(1));
    let instant_marker = MARKERS[((instant_pos.fract() * 7.0).round() as usize).min(7)];

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

        let zone_color = meter_zone_color(db_here, DB_MIN, DB_MAX);
        let fill_color = meter_fill_color(zone_color, db_here, energy);
        let peak_color = meter_peak_color(energy);
        let instant_color = meter_instant_color(energy);
        let empty_color = meter_empty_color(energy);

        let (ch, color) = if is_peak {
            ('▌', peak_color)
        } else if is_instant {
            (instant_marker, instant_color)
        } else if i < full_cells {
            ('█', fill_color)
        } else if i == full_cells && partial > 0 {
            (BLOCKS[partial], fill_color)
        } else {
            (' ', empty_color)
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

// ── helpers ───────────────────────────────────────────────────────────────────

fn meter_fill_color(base: Color, db_here: f32, energy: f32) -> Color {
    let (r, g, b) = match base {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        _ => (120.0, 120.0, 120.0),
    };
    let heat = ((db_here + 54.0) / 54.0).clamp(0.0, 1.0);
    let brightness = 0.26 + 0.62 * energy;
    let glow = 0.04 + 0.24 * smoothstep01(heat) * (0.28 + 0.72 * energy);
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

fn meter_peak_color(energy: f32) -> Color {
    let (r, g, b) = match C_METER_PEAK {
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

fn meter_instant_color(energy: f32) -> Color {
    let (r, g, b) = match C_METER_INSTANT {
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

fn meter_empty_color(energy: f32) -> Color {
    let (r, g, b) = match C_METER_EMPTY {
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

fn meter_zone_color(db_here: f32, db_min: f32, db_max: f32) -> Color {
    let t = ((db_here - db_min) / (db_max - db_min)).clamp(0.0, 1.0);
    if t < 0.56 {
        lerp_color(C_METER_LOW, C_METER_MID, t / 0.56)
    } else {
        lerp_color(C_METER_MID, C_METER_HIGH, (t - 0.56) / 0.44)
    }
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
        lerp_u8(ar, br, t),
        lerp_u8(ag, bg, t),
        lerp_u8(ab, bb, t),
    )
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let tt = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * tt).round() as u8
}

fn idle_line() -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled("■  nothing playing", Style::default().fg(C_MUTED)),
    ])
}

/// Build a smooth sub-block progress bar string of `width` cells.
fn smooth_bar(pos: f64, dur: f64, width: usize) -> String {
    const BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    if width == 0 {
        return String::new();
    }
    let progress = if dur > 0.0 {
        (pos / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let eighths = (progress * width as f64 * 8.0) as usize;
    let full = eighths / 8;
    let partial = eighths % 8;
    let mut bar = String::with_capacity(width + 2);
    for _ in 0..full {
        bar.push('█');
    }
    if full < width {
        bar.push(BLOCKS[partial]);
        for _ in (full + 1)..width {
            bar.push('·');
        }
    }
    bar
}

fn fmt_clock(v: f64) -> String {
    let total = v.max(0.0).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
