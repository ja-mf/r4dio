//! Header component — 2-row top bar.
//!
//! Row 1: now-playing station/file, ICY/show title, location, health badge.
//! Row 2: VU meter (left half) | seek bar + position (right half, file only).
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
    components::vu_meter::{self, MeterStyle},
    intent::RenderHint,
    theme::{
        C_ACCENT, C_BADGE_ERR, C_BADGE_PENDING, C_CONNECTING, C_MUTED, C_NETWORK, C_PLAYING, C_TAG,
    },
};

// ═══════════════════════════════════════════════════════════════════════════════
// TITLE LAMP EFFECT (Row 1 title glow)
// ═══════════════════════════════════════════════════════════════════════════════

/// Calculate adaptive title lamp brightness from audio levels.
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

/// Calculate title bulb color from lamp level.
fn title_bulb_color(state: &AppState) -> Color {
    let level = title_lamp_level(state);
    let scale = 0.01 + 0.99 * level;

    Color::Rgb(
        (8.0 + 232.0 * scale).round().min(255.0) as u8,
        (8.0 + 232.0 * scale).round().min(255.0) as u8,
        (12.0 + 244.0 * scale).round().min(255.0) as u8,
    )
}

/// Frequency-tinted bulb: bass → warm amber, mid → neutral, treble → cool blue.
/// Uses the adaptive dB window so the full brightness range tracks the station.
fn freq_bulb_color(state: &AppState) -> Color {
    let spread = state.meter_spread_db.clamp(2.0, 22.0);
    let mut floor = (state.meter_mean_db - 2.5 * spread).clamp(-90.0, -10.0);
    let ceil = (state.meter_mean_db + 2.0 * spread).clamp(-40.0, -1.0);
    if ceil - floor < 8.0 {
        floor = (ceil - 8.0).max(-90.0);
    }

    let level = state.audio_level.max(-90.0);
    let t = ((level - floor) / (ceil - floor)).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t); // smoothstep
    let t = t.powf(0.72);

    let b = (state.bass_db.max(-60.0) + 60.0) / 60.0;
    let m = (state.mid_db.max(-60.0) + 60.0) / 60.0;
    let tr = (state.treble_db.max(-60.0) + 60.0) / 60.0;
    let total = (b + m + tr).max(0.001);
    let (wb, wm, wt) = (b / total, m / total, tr / total);

    let r_base = wb * 255.0 + wm * 240.0 + wt * 160.0;
    let g_base = wb * 180.0 + wm * 230.0 + wt * 200.0;
    let b_base = wb * 60.0 + wm * 200.0 + wt * 255.0;

    let scale = 0.08 + 0.92 * t;
    Color::Rgb(
        (r_base * scale).round().min(255.0) as u8,
        (g_base * scale).round().min(255.0) as u8,
        (b_base * scale).round().min(255.0) as u8,
    )
}

/// BPM-pulsing bulb: pulses with beat phase when a tempo is detected.
/// Warm red pulse on beat, fades to dim between beats.
/// Falls back to a dim neutral dot when no BPM is detected.
fn bpm_bulb_color(state: &AppState) -> Color {
    let bpm = state.bpm_estimator.bpm();
    let confidence = state.bpm_estimator.confidence();

    if bpm.is_none() || confidence < 0.3 {
        // No reliable BPM — dim neutral dot.
        return Color::Rgb(40, 40, 45);
    }

    let phase = state.bpm_estimator.beat_phase();
    // Sharp pulse: bright on beat (phase=0), fast decay.
    // Use a raised-cosine envelope for a natural "thump" feel.
    let pulse = if phase < 0.3 {
        // Attack + sustain: cos curve from 1.0 → 0.0 over first 30% of beat
        let t = phase / 0.3;
        0.5 * (1.0 + (std::f32::consts::PI * t).cos())
    } else {
        0.0
    };

    // Blend pulse intensity with confidence.
    let intensity = pulse * confidence;
    let dim = 0.12; // floor brightness
    let scale = dim + (1.0 - dim) * intensity;

    // Warm red/orange pulse color.
    Color::Rgb(
        (255.0 * scale).round().min(255.0) as u8,
        (120.0 * scale).round().min(255.0) as u8,
        (40.0 * scale).round().min(255.0) as u8,
    )
}

/// Stable tempo-lock bulb: dim cyan when hunting, bright teal pulse when locked.
/// Visually distinct from the warm-red fast BPM bulb — indicates long-term lock.
fn tempo_lock_bulb_color(state: &AppState) -> Color {
    let locked = state.tempo_lock.is_locked();
    if !locked {
        return Color::Rgb(15, 45, 55); // dim dark cyan — hunting
    }
    let phase = state.tempo_lock.beat_phase();
    let pulse = if phase < 0.25 {
        let t = phase / 0.25;
        0.5 * (1.0 + (std::f32::consts::PI * t).cos())
    } else {
        0.0
    };
    let dim = 0.18_f32;
    let scale = dim + (1.0 - dim) * pulse;
    Color::Rgb(
        (50.0 * scale).round().min(255.0) as u8,
        (220.0 * scale).round().min(255.0) as u8,
        (235.0 * scale).round().min(255.0) as u8,
    )
}


/// Calculate title text color from lamp level.
fn title_text_color(state: &AppState) -> Color {
    let level = title_lamp_level(state);
    let scale = 0.02 + 0.98 * level;
    Color::Rgb(
        (6.0 + 210.0 * scale).round().min(255.0) as u8,
        (7.0 + 198.0 * scale).round().min(255.0) as u8,
        (12.0 + 236.0 * scale).round().min(255.0) as u8,
    )
}

fn smoothstep01(v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

// ═══════════════════════════════════════════════════════════════════════════════
// COMPONENT
// ═══════════════════════════════════════════════════════════════════════════════

pub struct Header {
    /// VU meter visual style.
    meter_style: MeterStyle,
}

impl Header {
    pub fn new() -> Self {
        Self {
            meter_style: MeterStyle::Analog, // Analog (ballistic needle) as default
        }
    }

    /// Set the VU meter style.
    pub fn with_meter_style(mut self, style: MeterStyle) -> Self {
        self.meter_style = style;
        self
    }

    /// Get the current VU meter style.
    pub fn meter_style(&self) -> MeterStyle {
        self.meter_style
    }

    /// Cycle to the next VU meter style.
    pub fn cycle_meter_style(&mut self) -> MeterStyle {
        self.meter_style = match self.meter_style {
            MeterStyle::Studio => MeterStyle::Led,
            MeterStyle::Led => MeterStyle::Analog,
            MeterStyle::Analog => MeterStyle::Studio,
        };
        self.meter_style
    }
}

impl Default for Header {
    fn default() -> Self {
        Self::new()
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

    fn on_action(&mut self, action: &Action, _state: &AppState) -> Vec<Action> {
        match action {
            Action::CycleVuMeterStyle => {
                self.cycle_meter_style();
                vec![]
            }
            _ => vec![],
        }
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

        // Row 2: meter | seek bar
        draw_row2(frame, rows[1], state, self.meter_style);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// ROW 1: Now Playing Info
// ═══════════════════════════════════════════════════════════════════════════════

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
        build_file_row(state, path, health_span)
    } else if let Some(idx) = ds.current_station {
        build_station_row(state, idx, health_span)
    } else {
        idle_line()
    }
}

fn build_file_row(
    state: &AppState,
    path: &str,
    health_span: Option<Span<'static>>,
) -> Line<'static> {
    let ds = &state.daemon_state;

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

    let file_name = std::path::Path::new(path)
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
        Span::styled(
            "●",
            Style::default()
                .fg(freq_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "●",
            Style::default()
                .fg(bpm_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "●",
            Style::default()
                .fg(tempo_lock_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if let Some(hs) = health_span {
        spans.push(hs);
    }
    Line::from(spans)
}

fn build_station_row(
    state: &AppState,
    idx: usize,
    health_span: Option<Span<'static>>,
) -> Line<'static> {
    let ds = &state.daemon_state;

    let Some(station) = ds.stations.get(idx) else {
        return idle_line();
    };

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
        Span::styled(
            "●",
            Style::default()
                .fg(freq_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "●",
            Style::default()
                .fg(bpm_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "●",
            Style::default()
                .fg(tempo_lock_bulb_color(state))
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // BPM label (only when confident).
    if let Some(bpm) = state.bpm_estimator.bpm() {
        if state.bpm_estimator.confidence() >= 0.3 {
            spans.push(Span::styled(
                format!(" {:.0}", bpm),
                Style::default().fg(C_MUTED),
            ));
        }
    }

    // Locked tempo label (only when TempoLock is locked).
    if state.tempo_lock.is_locked() {
        if let Some(locked_bpm) = state.tempo_lock.bpm() {
            spans.push(Span::styled(
                format!(" /{:.0}", locked_bpm),
                Style::default().fg(Color::Rgb(35, 140, 155)),
            ));
        }
    }
    if !station.city.is_empty() {
        spans.push(Span::styled(
            format!("  {}", station.city),
            Style::default().fg(C_MUTED),
        ));
    }

    // Show title: prefer NTS show name, then ICY from multiple sources
    // ICY fallback chain (most recent first):
    // 1. daemon_state.icy_title — live value from daemon
    // 2. last_known_icy — sticky value that survives transient None states
    // 3. icy_history — most recent entry for this station in session
    // 4. station_poll_titles — auto-poll cache
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
    let show_text = show_text
        // Tier 1: Live ICY from daemon
        .or_else(|| {
            state
                .daemon_state
                .icy_title
                .clone()
                .filter(|s| !s.is_empty())
        })
        // Tier 2: Sticky last_known_icy if it matches current station
        .or_else(|| {
            state
                .last_known_icy
                .as_ref()
                .filter(|(st, _)| st == &station.name)
                .map(|(_, title)| title.clone())
        })
        // Tier 3: Most recent ICY history entry for this station
        .or_else(|| {
            state
                .icy_history
                .iter()
                .rev()
                .find(|e| e.station.as_deref() == Some(&station.name))
                .map(|e| e.raw.clone())
        })
        // Tier 4: Auto-poll cache
        .or_else(|| {
            state
                .station_poll_titles
                .get(&station.name)
                .cloned()
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
}

fn idle_line() -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled("■  nothing playing", Style::default().fg(C_MUTED)),
    ])
}

// ═══════════════════════════════════════════════════════════════════════════════
// ROW 2: VU Meter + Seek Bar
// ═══════════════════════════════════════════════════════════════════════════════

fn draw_row2(frame: &mut Frame, area: Rect, state: &AppState, meter_style: MeterStyle) {
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

    // Draw VU meter using the new component
    vu_meter::draw_vu_meter(frame, meter_area, state, is_playing, meter_style);

    // Draw seek bar (file only)
    if let Some(seek_area) = seek_area {
        if let Some(pos) = ds.time_pos_secs {
            draw_seek_bar(frame, seek_area, pos, ds.duration_secs.unwrap_or(0.0));
        }
    }
}

fn draw_seek_bar(frame: &mut Frame, area: Rect, pos: f64, duration: f64) {
    let w = area.width as usize;
    let label = format!(
        " {}/{}",
        fmt_clock(pos),
        if duration > 0.0 {
            fmt_clock(duration)
        } else {
            "--:--".to_string()
        }
    );
    let label_w = label.len();
    let bar_w = w.saturating_sub(label_w + 1);
    let bar = smooth_bar(pos, duration, bar_w);

    let seek_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(bar, Style::default().fg(C_PLAYING)),
        Span::styled(label, Style::default().fg(C_TAG)),
    ]);
    frame.render_widget(Paragraph::new(seek_line), area);
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
