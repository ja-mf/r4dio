use crate::{search_matches, App, FocusPane, LeftPaneMode, NtsChannel, NtsShow, SortOrder};
use chrono::Datelike;
use radio_tui::shared::protocol::PlaybackStatus;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

// ── Color palette ─────────────────────────────────────────────────────────────
const C_ACCENT: Color = Color::Rgb(255, 95, 95);
const C_PLAYING: Color = Color::Rgb(80, 200, 120);
const C_CONNECTING: Color = Color::Rgb(255, 184, 80);
const C_MUTED: Color = Color::Rgb(72, 72, 88);
const C_SEPARATOR: Color = Color::Rgb(40, 40, 52);
const C_SECONDARY: Color = Color::Rgb(115, 115, 138);
const C_PRIMARY: Color = Color::Rgb(210, 210, 225);
const C_SELECTION_BG: Color = Color::Rgb(28, 28, 40);
const C_PANEL_BORDER: Color = Color::Rgb(40, 40, 52);
const C_FILTER_BG: Color = Color::Rgb(20, 20, 32);
const C_FILTER_FG: Color = Color::Rgb(255, 200, 80);
const C_TAG: Color = Color::Rgb(80, 140, 200);
const C_LOCATION: Color = Color::Rgb(100, 160, 130);
const C_NETWORK: Color = Color::Rgb(180, 120, 220);

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.size();

    // ── Outer vertical layout ────────────────────────────────────────────────
    let keys_h = if app.show_keys { 1 } else { 0 };
    let sep_h = if app.show_keys { 1 } else { 0 };
    let filter_h: u16 = if app.filter_active { 1 } else { 0 };
    let log_h: u16 = if app.show_logs { 8 } else { 1 };

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),        // header (now playing + vol)
            Constraint::Length(1),        // separator
            Constraint::Length(filter_h), // filter bar (hidden when inactive)
            Constraint::Min(3),           // body
            Constraint::Length(1),        // log separator
            Constraint::Length(log_h),    // log bar/panel
            Constraint::Length(sep_h),    // separator (hidden with keys)
            Constraint::Length(keys_h),   // keybindings
        ])
        .split(area);

    draw_header(f, app, outer[0]);
    draw_separator(f, outer[1]);
    if app.filter_active {
        draw_filter_bar(f, app, outer[2]);
    }
    draw_body(f, app, outer[3]);
    draw_separator(f, outer[4]);
    if app.show_logs {
        draw_logs(f, app, outer[5]);
    } else {
        draw_log_bar(f, app, outer[5]);
    }
    if app.show_keys {
        draw_separator(f, outer[6]);
        draw_keybindings(f, outer[7]);
    }

    // Overlays
    if app.show_help {
        draw_help_overlay(f, area);
    }
    if let Some(ref error) = app.error_message {
        draw_error_popup(f, error, area);
    }
}

// ── Header (now playing) ──────────────────────────────────────────────────────

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let hchunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(8)])
        .split(area);

    // Left: now playing — status icon + source + status text
    let left = if let Some(path) = app.state.current_file.as_ref() {
        let looks_paused = app.state.current_file.is_some()
            && app.state.playback_status == PlaybackStatus::Connecting
            && app.state.time_pos_secs.is_some();
        let (icon, icon_color, status_text) = if looks_paused {
            ("⏸", C_CONNECTING, "paused")
        } else {
            match &app.state.playback_status {
                PlaybackStatus::Playing => ("▶", C_PLAYING, ""),
                PlaybackStatus::Connecting => ("◔", C_CONNECTING, "loading"),
                PlaybackStatus::Error => ("⛔", C_ACCENT, "error"),
                PlaybackStatus::Idle => ("■", C_MUTED, "stopped"),
            }
        };
        let file_name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::raw(" "),
            Span::styled("🗎 ", Style::default().fg(C_MUTED)),
            Span::styled(
                file_name,
                Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
            ),
        ];
        if !status_text.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(status_text, Style::default().fg(icon_color)));
        }
        if matches!(
            app.state.playback_status,
            PlaybackStatus::Playing | PlaybackStatus::Connecting
        ) {
            let pos = app.state.time_pos_secs.unwrap_or(0.0).max(0.0);
            let dur = app.state.duration_secs.unwrap_or(0.0).max(0.0);
            let bar_w = 14usize;
            let fill = if dur > 0.0 {
                ((pos / dur).clamp(0.0, 1.0) * (bar_w as f64)).round() as usize
            } else {
                0
            };
            let mut bar_fill = String::new();
            let mut bar_empty = String::new();
            for i in 0..bar_w {
                if i < fill {
                    bar_fill.push('▓');
                } else {
                    bar_empty.push('·');
                }
            }
            let dur_txt = if dur > 0.0 {
                fmt_clock_secs(dur)
            } else {
                "--:--".to_string()
            };
            spans.push(Span::styled("  🗎 ", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(bar_fill, Style::default().fg(C_PLAYING)));
            spans.push(Span::styled(bar_empty, Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                format!(" {}/{}", fmt_clock_secs(pos), dur_txt),
                Style::default().fg(C_TAG),
            ));
        }
        if !app.filter.is_empty() && !app.filter_active {
            spans.push(Span::styled("  /", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                app.filter.as_str(),
                Style::default().fg(C_FILTER_FG),
            ));
        }
        if app.sort_order != SortOrder::Default && !app.filter_active {
            spans.push(Span::styled("  sort:", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                app.sort_order.label_for_mode(app.left_mode),
                Style::default().fg(C_NETWORK),
            ));
        }
        Line::from(spans)
    } else if let Some(idx) = app.state.current_station {
        if let Some(station) = app.state.stations.get(idx) {
            let (icon, icon_color, status_text) = match &app.state.playback_status {
                PlaybackStatus::Playing => ("▶", C_PLAYING, ""),
                PlaybackStatus::Connecting => ("◔", C_CONNECTING, "loading"),
                PlaybackStatus::Error => ("⛔", C_ACCENT, "error"),
                PlaybackStatus::Idle => ("■", C_MUTED, "stopped"),
            };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw(" "),
                Span::styled("📻 ", Style::default().fg(C_MUTED)),
                Span::styled(
                    &station.name,
                    Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
                ),
            ];
            if !status_text.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(status_text, Style::default().fg(icon_color)));
            }
            if let Some(loc) = build_radio_city_time(app) {
                spans.push(Span::styled(
                    format!("  {}", loc.city_time),
                    Style::default().fg(C_MUTED),
                ));
                if let Some(show) = loc.show_name {
                    spans.push(Span::styled("  ", Style::default()));
                    spans.push(Span::styled(show, Style::default().fg(C_NETWORK)));
                }
            }
            // Show filter / sort hints in header when bar is hidden
            if !app.filter.is_empty() && !app.filter_active {
                spans.push(Span::styled("  /", Style::default().fg(C_MUTED)));
                spans.push(Span::styled(
                    app.filter.as_str(),
                    Style::default().fg(C_FILTER_FG),
                ));
            }
            if app.sort_order != SortOrder::Default && !app.filter_active {
                spans.push(Span::styled("  sort:", Style::default().fg(C_MUTED)));
                spans.push(Span::styled(
                    app.sort_order.label_for_mode(app.left_mode),
                    Style::default().fg(C_NETWORK),
                ));
            }
            Line::from(spans)
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled("■  nothing playing", Style::default().fg(C_MUTED)),
            ])
        }
    } else {
        let mut spans = vec![
            Span::raw(" "),
            Span::styled("■  nothing playing", Style::default().fg(C_MUTED)),
        ];
        if !app.filter.is_empty() && !app.filter_active {
            spans.push(Span::styled("  /", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                app.filter.as_str(),
                Style::default().fg(C_FILTER_FG),
            ));
        }
        if app.sort_order != SortOrder::Default && !app.filter_active {
            spans.push(Span::styled("  sort:", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                app.sort_order.label_for_mode(app.left_mode),
                Style::default().fg(C_NETWORK),
            ));
        }
        Line::from(spans)
    };

    f.render_widget(Paragraph::new(left), hchunks[0]);

    // Right: connection + volume percent
    let vol_pct = (app.state.volume * 100.0).round() as u32;
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            format!("{:>3}%", vol_pct),
            Style::default().fg(C_SECONDARY),
        )]))
        .alignment(Alignment::Right),
        hchunks[1],
    );
}

/// Build header clock: local time only.
fn build_clock_string(app: &App) -> String {
    let now = chrono::Local::now();
    let _ = app;
    now.format("%d/%m %H:%M").to_string()
}

/// Returns CITY@HH:MM when current station timezone differs from local timezone.
struct HeaderLocation {
    city_time: String,
    show_name: Option<String>,
}

fn build_radio_city_time(app: &App) -> Option<HeaderLocation> {
    let now = chrono::Local::now();
    let idx = app.state.current_station?;
    let s = app.state.stations.get(idx)?;
    if s.city.is_empty() {
        return None;
    }
    let city_name = s.city.clone();
    // For NTS stations, get show name from fetched data
    let show_name = if s.name == "NTS 1" {
        app.nts_ch1
            .as_ref()
            .map(|ch| ch.now.broadcast_title.clone())
    } else if s.name == "NTS 2" {
        app.nts_ch2
            .as_ref()
            .map(|ch| ch.now.broadcast_title.clone())
    } else {
        None
    };

    let tz_name = get_timezone_for_city(&city_name)?;
    let radio_offset = parse_timezone_offset(&tz_name, &now)?;
    let _ = radio_offset;
    let local_hm = now.format("%H:%M").to_string();
    let radio_time = calculate_time_in_timezone(&now, &tz_name);
    if radio_time == local_hm {
        None
    } else {
        Some(HeaderLocation {
            city_time: format!("{}@{}", city_name, radio_time),
            show_name,
        })
    }
}

/// Calculate current time in a given IANA timezone
fn calculate_time_in_timezone(
    local_now: &chrono::DateTime<chrono::Local>,
    tz_name: &str,
) -> String {
    // Parse the timezone offset from common formats
    // tz_name is in format "Europe/London", "America/New_York", etc.
    match parse_timezone_offset(tz_name, local_now) {
        Some(offset_hours) => {
            let utc_now = local_now.with_timezone(&chrono::Utc);
            let radio_now = utc_now + chrono::Duration::hours(offset_hours);
            radio_now.format("%H:%M").to_string()
        }
        None => tz_name.to_string(), // Fallback to showing timezone name
    }
}

/// Parse timezone offset from IANA timezone name
/// Returns offset in hours from UTC, or None if unknown
fn parse_timezone_offset(
    tz_name: &str,
    reference_time: &chrono::DateTime<chrono::Local>,
) -> Option<i64> {
    // Check if it's DST period (Northern Hemisphere: March-October, Southern: opposite)
    let month = reference_time.month();
    let is_northern_summer = month >= 3 && month <= 10;

    match tz_name {
        // Europe
        "Europe/London" => Some(if is_northern_summer { 1 } else { 0 }),
        "Europe/Amsterdam" | "Europe/Paris" | "Europe/Berlin" | "Europe/Stockholm"
        | "Europe/Oslo" | "Europe/Copenhagen" | "Europe/Helsinki" | "Europe/Vienna"
        | "Europe/Zurich" | "Europe/Rome" | "Europe/Madrid" | "Europe/Lisbon" | "Europe/Dublin"
        | "Europe/Brussels" | "Europe/Prague" | "Europe/Warsaw" | "Europe/Budapest"
        | "Europe/Bucharest" | "Europe/Athens" => Some(if is_northern_summer { 2 } else { 1 }),
        "Europe/Moscow" | "Europe/Istanbul" => Some(3),

        // Americas
        "America/New_York" | "America/Toronto" | "America/Montreal" => {
            Some(if is_northern_summer { -4 } else { -5 })
        }
        "America/Chicago" => Some(if is_northern_summer { -5 } else { -6 }),
        "America/Denver" | "America/Edmonton" => Some(if is_northern_summer { -6 } else { -7 }),
        "America/Los_Angeles" | "America/Vancouver" => {
            Some(if is_northern_summer { -7 } else { -8 })
        }
        "America/Anchorage" => Some(if is_northern_summer { -8 } else { -9 }),
        "America/Mexico_City" => Some(if is_northern_summer { -5 } else { -6 }),
        "America/Sao_Paulo" => Some(if !is_northern_summer { -2 } else { -3 }), // Southern
        "America/Argentina/Buenos_Aires" => Some(-3),

        // Asia
        "Asia/Tokyo" | "Asia/Seoul" => Some(9),
        "Asia/Shanghai" | "Asia/Hong_Kong" | "Asia/Singapore" | "Asia/Taipei" => Some(8),
        "Asia/Bangkok" | "Asia/Jakarta" => Some(7),
        "Asia/Kolkata" | "Asia/Dubai" => Some(5),
        "Asia/Jerusalem" | "Asia/Beirut" => Some(if is_northern_summer { 3 } else { 2 }),
        "Asia/Manila" => Some(8),

        // Pacific
        "Australia/Sydney" | "Australia/Melbourne" | "Australia/Canberra" => {
            Some(if !is_northern_summer { 11 } else { 10 }) // Southern
        }
        "Australia/Brisbane" => Some(10),
        "Australia/Adelaide" => Some(if !is_northern_summer { 10 } else { 9 }),
        "Australia/Perth" => Some(8),
        "Australia/Darwin" => Some(9),
        "Australia/Hobart" => Some(if !is_northern_summer { 11 } else { 10 }),
        "Pacific/Auckland" | "Pacific/Wellington" => {
            Some(if !is_northern_summer { 13 } else { 12 })
        }
        "Pacific/Honolulu" => Some(-10),

        // Africa
        "Africa/Cairo" | "Africa/Johannesburg" => Some(2),
        "Africa/Casablanca" => Some(1),
        "Africa/Lagos" | "Africa/Accra" => Some(1),
        "Africa/Nairobi" | "Africa/Addis_Ababa" => Some(3),

        // Atlantic
        "Atlantic/Reykjavik" => Some(0),

        _ => None,
    }
}

/// Get timezone name for a city.
/// First checks hardcoded common cities, then falls back to API lookup.
fn get_timezone_for_city(city: &str) -> Option<String> {
    // Hardcoded mapping for common cities to avoid API calls
    let city_lower = city.to_lowercase();
    let hardcoded = match city_lower.as_str() {
        "london" | "ldn" => Some("Europe/London"),
        "amsterdam" | "ams" => Some("Europe/Amsterdam"),
        "paris" => Some("Europe/Paris"),
        "berlin" => Some("Europe/Berlin"),
        "new york" | "nyc" => Some("America/New_York"),
        "los angeles" | "la" | "lax" => Some("America/Los_Angeles"),
        "chicago" => Some("America/Chicago"),
        "tokyo" => Some("Asia/Tokyo"),
        "sydney" => Some("Australia/Sydney"),
        "melbourne" => Some("Australia/Melbourne"),
        "shanghai" => Some("Asia/Shanghai"),
        "hong kong" => Some("Asia/Hong_Kong"),
        "singapore" => Some("Asia/Singapore"),
        "mumbai" => Some("Asia/Kolkata"),
        "dubai" => Some("Asia/Dubai"),
        "moscow" => Some("Europe/Moscow"),
        "istanbul" => Some("Europe/Istanbul"),
        "cairo" => Some("Africa/Cairo"),
        "johannesburg" => Some("Africa/Johannesburg"),
        "são paulo" | "sao paulo" => Some("America/Sao_Paulo"),
        "rio de janeiro" | "rio" => Some("America/Sao_Paulo"),
        "buenos aires" => Some("America/Argentina/Buenos_Aires"),
        "mexico city" => Some("America/Mexico_City"),
        "vancouver" => Some("America/Vancouver"),
        "toronto" => Some("America/Toronto"),
        "montreal" => Some("America/Toronto"),
        "calgary" => Some("America/Edmonton"),
        "edmonton" => Some("America/Edmonton"),
        "seoul" => Some("Asia/Seoul"),
        "bangkok" => Some("Asia/Bangkok"),
        "jakarta" => Some("Asia/Jakarta"),
        "manila" => Some("Asia/Manila"),
        "taipei" => Some("Asia/Taipei"),
        "auckland" => Some("Pacific/Auckland"),
        "wellington" => Some("Pacific/Auckland"),
        "honolulu" => Some("Pacific/Honolulu"),
        "anchorage" => Some("America/Anchorage"),
        "reykjavik" => Some("Atlantic/Reykjavik"),
        "stockholm" => Some("Europe/Stockholm"),
        "oslo" => Some("Europe/Oslo"),
        "copenhagen" => Some("Europe/Copenhagen"),
        "helsinki" => Some("Europe/Helsinki"),
        "vienna" => Some("Europe/Vienna"),
        "zurich" => Some("Europe/Zurich"),
        "milan" => Some("Europe/Rome"),
        "rome" => Some("Europe/Rome"),
        "madrid" => Some("Europe/Madrid"),
        "barcelona" => Some("Europe/Madrid"),
        "lisbon" => Some("Europe/Lisbon"),
        "dublin" => Some("Europe/Dublin"),
        "brussels" => Some("Europe/Brussels"),
        "geneva" => Some("Europe/Zurich"),
        "prague" => Some("Europe/Prague"),
        "warsaw" => Some("Europe/Warsaw"),
        "budapest" => Some("Europe/Budapest"),
        "bucharest" => Some("Europe/Bucharest"),
        "athens" => Some("Europe/Athens"),
        "tel aviv" => Some("Asia/Jerusalem"),
        "jerusalem" => Some("Asia/Jerusalem"),
        "beirut" => Some("Asia/Beirut"),
        "casablanca" => Some("Africa/Casablanca"),
        "lagos" => Some("Africa/Lagos"),
        "nairobi" => Some("Africa/Nairobi"),
        "accra" => Some("Africa/Accra"),
        "addis ababa" => Some("Africa/Addis_Ababa"),
        "cape town" => Some("Africa/Johannesburg"),
        "durban" => Some("Africa/Johannesburg"),
        "perth" => Some("Australia/Perth"),
        "adelaide" => Some("Australia/Adelaide"),
        "brisbane" => Some("Australia/Brisbane"),
        "darwin" => Some("Australia/Darwin"),
        "hobart" => Some("Australia/Hobart"),
        "canberra" => Some("Australia/Canberra"),
        _ => None,
    };

    hardcoded.map(|s| s.to_string())
}

// ── Filter bar ────────────────────────────────────────────────────────────────

fn draw_filter_bar(f: &mut Frame, app: &App, area: Rect) {
    let (match_count, total, sort_str) = match app.filter_target {
        FocusPane::Left => {
            let counts = match app.left_mode {
                LeftPaneMode::Stations => (app.filtered_indices.len(), app.state.stations.len()),
                LeftPaneMode::Files => (app.file_filtered_indices.len(), app.files.len()),
            };
            let sort = if app.left_mode == LeftPaneMode::Stations
                && app.sort_order != SortOrder::Default
            {
                format!(" sort:{} ", app.sort_order.label_for_mode(app.left_mode))
            } else {
                String::new()
            };
            (counts.0, counts.1, sort)
        }
        FocusPane::Icy => {
            let q = app.filter_icy.clone();
            let total = app.icy_history.len();
            let count = if q.trim().is_empty() {
                total
            } else {
                app.icy_history
                    .iter()
                    .filter(|e| {
                        let mut t = e.display.clone();
                        if let Some(st) = e.station.as_deref() {
                            t.push_str(&format!(" {}", st));
                        }
                        search_matches(&q, &t)
                    })
                    .count()
            };
            (count, total, String::new())
        }
        FocusPane::Songs => {
            let q = app.filter_songs.clone();
            let total = app.songs_history.len();
            let count = if q.trim().is_empty() {
                total
            } else {
                app.songs_history
                    .iter()
                    .filter(|e| {
                        let mut t = e.display.clone();
                        if let Some(st) = e.station.as_deref() {
                            t.push_str(&format!(" {}", st));
                        }
                        if let Some(show) = e.show.as_deref() {
                            t.push_str(&format!(" {}", show));
                        }
                        if let Some(comment) = e.comment.as_deref() {
                            t.push_str(&format!(" {}", comment));
                        }
                        search_matches(&q, &t)
                    })
                    .count()
            };
            (count, total, String::new())
        }
        FocusPane::Meta => (0, 0, String::new()),
        FocusPane::Nts => (0, 0, String::new()),
    };

    let count_str = if app.filter.is_empty() {
        if total > 0 {
            format!(" {}/{} ", total, total)
        } else {
            String::new()
        }
    } else {
        if total > 0 {
            format!(" {}/{} ", match_count, total)
        } else {
            String::new()
        }
    };
    let right_str = format!("{}{}", sort_str, count_str);

    let used = 3 + app.filter.len() + 1 + right_str.len(); // "/ " + query + cursor + right
    let padding = (area.width as usize).saturating_sub(used);

    let line = Line::from(vec![
        Span::styled(" / ", Style::default().fg(C_MUTED).bg(C_FILTER_BG)),
        Span::styled(
            app.filter.as_str(),
            Style::default()
                .fg(C_FILTER_FG)
                .bg(C_FILTER_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("█", Style::default().fg(C_FILTER_FG).bg(C_FILTER_BG)), // cursor
        Span::styled(" ".repeat(padding), Style::default().bg(C_FILTER_BG)),
        Span::styled(sort_str, Style::default().fg(C_NETWORK).bg(C_FILTER_BG)),
        Span::styled(count_str, Style::default().fg(C_MUTED).bg(C_FILTER_BG)),
    ]);

    f.render_widget(Paragraph::new(line), area);
}

// ── Separator ─────────────────────────────────────────────────────────────────

fn draw_separator(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(C_SEPARATOR)),
        area,
    );
}

fn draw_labeled_separator(f: &mut Frame, area: Rect, label: &str) {
    draw_labeled_separator_color(f, area, label, C_MUTED);
}

fn draw_labeled_separator_color(f: &mut Frame, area: Rect, label: &str, label_color: Color) {
    let label_len = label.len();
    let line_len = (area.width as usize).saturating_sub(label_len + 1);
    let line = Line::from(vec![
        Span::styled("─".repeat(1), Style::default().fg(C_SEPARATOR)),
        Span::styled(label.to_string(), Style::default().fg(label_color)),
        Span::styled("─".repeat(line_len), Style::default().fg(C_SEPARATOR)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ── Body: left (station list) + right (info panel) ───────────────────────────

fn draw_body(f: &mut Frame, app: &mut App, area: Rect) {
    let (left_pct, div_w, right_pct) =
        if app.left_mode == LeftPaneMode::Files && app.files_left_full_width {
            (100, 0, 0)
        } else {
            (55, 1, 45)
        };
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_pct),  // station list / file list
            Constraint::Length(div_w),         // divider
            Constraint::Percentage(right_pct), // info panel
        ])
        .split(area);

    app.left_pane_rect = panels[0];
    app.upper_right_rect = Rect::default();
    app.middle_right_rect = Rect::default();
    app.lower_right_rect = Rect::default();

    draw_left_panel(f, app, panels[0]);
    if panels[1].width > 0 {
        draw_vertical_divider(f, panels[1]);
    }
    if panels[2].width > 0 {
        draw_info_panel(f, app, panels[2]);
    }
}

fn draw_vertical_divider(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = (0..area.height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(C_PANEL_BORDER))))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

// ── Station list (left panel) ─────────────────────────────────────────────────

fn draw_left_panel(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let label = match app.left_mode {
        LeftPaneMode::Stations => " [1] stations ",
        LeftPaneMode::Files => " [1] files ",
    };
    let hdr = Rect { height: 1, ..area };
    let content = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let focused = app.focus_pane == FocusPane::Left;
    draw_labeled_separator_color(f, hdr, label, if focused { C_PRIMARY } else { C_MUTED });
    match app.left_mode {
        LeftPaneMode::Stations => draw_station_list(f, app, content),
        LeftPaneMode::Files => draw_file_list(f, app, content),
    }
}

fn draw_station_list(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(Clear, area);
    if app.state.stations.is_empty() {
        let msg = if app.connected {
            "  no stations loaded"
        } else {
            "  connecting to daemon…"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(C_MUTED))),
            area,
        );
        return;
    }

    if app.filtered_indices.is_empty() && !app.filter.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  no stations match filter",
                Style::default().fg(C_MUTED),
            )),
            area,
        );
        return;
    }

    // Networks that appear more than once — only those get the network label shown.
    let mut network_count: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for s in &app.state.stations {
        if !s.network.is_empty() {
            *network_count.entry(s.network.as_str()).or_insert(0) += 1;
        }
    }

    let filtering = app.filter_active || !app.filter.is_empty();

    let total = app.filtered_indices.len();
    let visible = (area.height as usize).max(1);
    let selected_pos = app
        .filtered_indices
        .iter()
        .position(|&i| i == app.selected_idx)
        .unwrap_or(0)
        .min(total.saturating_sub(1));
    let start = if total > visible {
        selected_pos
            .saturating_sub(visible / 2)
            .min(total - visible)
    } else {
        0
    };
    app.station_view_start = start;

    let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .skip(start)
        .take(visible)
        .map(|&idx| {
            let station = &app.state.stations[idx];
            let is_current = Some(idx) == app.state.current_station;
            let is_selected = idx == app.selected_idx;

            let (icon, icon_color) = if is_current {
                match &app.state.playback_status {
                    PlaybackStatus::Playing => ("▶", C_PLAYING),
                    PlaybackStatus::Connecting => ("⋯", C_CONNECTING),
                    PlaybackStatus::Error => ("✗", C_ACCENT),
                    PlaybackStatus::Idle => ("■", C_MUTED),
                }
            } else {
                (" ", C_MUTED)
            };

            let name_color = if is_current {
                match &app.state.playback_status {
                    PlaybackStatus::Playing => C_PLAYING,
                    PlaybackStatus::Connecting => C_CONNECTING,
                    PlaybackStatus::Error => C_ACCENT,
                    PlaybackStatus::Idle => C_PRIMARY,
                }
            } else if is_selected {
                C_PRIMARY
            } else {
                C_SECONDARY
            };

            let name_style = if is_current || is_selected {
                Style::default().fg(name_color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(name_color)
            };

            let item_style = if is_selected {
                Style::default().bg(C_SELECTION_BG)
            } else {
                Style::default()
            };

            // ── Determine which metadata fields to show ────────────────────
            // Network: only when the network has more than one station.
            let show_network = !station.network.is_empty()
                && network_count
                    .get(station.network.as_str())
                    .copied()
                    .unwrap_or(0)
                    > 1;

            // Location: just city, never display country
            let location = if station.city.is_empty() {
                String::new()
            } else {
                station.city.clone()
            };

            // ── Build the single line ──────────────────────────────────────
            // Stars (leftmost, before icon)
            let stars = app
                .station_stars
                .get(&station.name)
                .copied()
                .unwrap_or(0)
                .min(3);
            let star_prefix = if stars > 0 {
                format!("{} ", "*".repeat(stars as usize))
            } else {
                "  ".to_string()
            };
            let mut spans: Vec<Span> = vec![
                Span::styled(star_prefix, Style::default().fg(C_TAG)),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw("  "),
            ];

            // Network badge (multi-station networks only)
            if show_network {
                spans.push(Span::styled(
                    station.network.as_str(),
                    Style::default().fg(C_NETWORK),
                ));
                spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
            }

            spans.push(Span::styled(station.name.as_str(), name_style));

            // Location
            if !location.is_empty() {
                spans.push(Span::styled("  ", Style::default()));
                spans.push(Span::styled(location, Style::default().fg(C_LOCATION)));
            }

            // Tags — shown only on the selected row while filtering
            if filtering && is_selected && !station.tags.is_empty() {
                spans.push(Span::styled("  ", Style::default()));
                for (i, tag) in station.tags.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
                    }
                    spans.push(Span::styled(tag.as_str(), Style::default().fg(C_TAG)));
                }
            }

            ListItem::new(Line::from(spans)).style(item_style)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::default())
        .highlight_symbol("");

    app.list_state
        .select(Some(selected_pos.saturating_sub(start)));

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn format_file_size(size: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let s = size as f64;
    if s >= GB {
        format!("{:.1}G", s / GB)
    } else if s >= MB {
        format!("{:.1}M", s / MB)
    } else if s >= KB {
        format!("{:.1}K", s / KB)
    } else {
        format!("{}B", size)
    }
}

fn draw_file_list(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(Clear, area);
    if app.files.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  no playable files in ~/nts-downloads",
                Style::default().fg(C_MUTED),
            )),
            area,
        );
        return;
    }

    if app.file_filtered_indices.is_empty() && !app.filter.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  no files match filter",
                Style::default().fg(C_MUTED),
            )),
            area,
        );
        return;
    }

    let total = app.file_filtered_indices.len();
    let visible = (area.height as usize).max(1);
    let selected_pos = app
        .file_filtered_indices
        .iter()
        .position(|&i| i == app.file_selected)
        .unwrap_or(0)
        .min(total.saturating_sub(1));
    let start = if total > visible {
        selected_pos
            .saturating_sub(visible / 2)
            .min(total - visible)
    } else {
        0
    };
    app.file_view_start = start;

    let items: Vec<ListItem> = app
        .file_filtered_indices
        .iter()
        .skip(start)
        .take(visible)
        .map(|&idx| {
            let file = &app.files[idx];
            let is_selected = idx == app.file_selected;
            let is_current = app
                .state
                .current_file
                .as_ref()
                .map(|p| p == &file.path.to_string_lossy())
                .unwrap_or(false);
            let icon = if is_current {
                match app.state.playback_status {
                    PlaybackStatus::Playing => "▶",
                    PlaybackStatus::Connecting => "⋯",
                    PlaybackStatus::Error => "✗",
                    PlaybackStatus::Idle => "■",
                }
            } else {
                " "
            };
            let color = if is_current {
                C_PLAYING
            } else if is_selected {
                C_PRIMARY
            } else {
                C_SECONDARY
            };
            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(icon, Style::default().fg(C_MUTED)),
                Span::raw("  "),
                Span::styled(
                    {
                        let k = file.path.to_string_lossy().to_string();
                        let s = app.file_stars.get(&k).copied().unwrap_or(0).min(3);
                        if s > 0 {
                            format!("{} ", "*".repeat(s as usize))
                        } else {
                            String::new()
                        }
                    },
                    Style::default().fg(C_TAG),
                ),
                Span::styled(file.name.as_str(), Style::default().fg(color)),
                {
                    let key = file.path.to_string_lossy().to_string();
                    let g = if let Some(meta) = app.file_metadata_cache.get(&key) {
                        (
                            meta.genre.as_deref().unwrap_or("-").to_string(),
                            meta.duration_secs
                                .map(fmt_clock_secs)
                                .unwrap_or_else(|| "--:--".to_string()),
                        )
                    } else {
                        ("-".to_string(), "--:--".to_string())
                    }
                    .0;
                    Span::styled(format!("  {}", g), Style::default().fg(C_LOCATION))
                },
                {
                    let key = file.path.to_string_lossy().to_string();
                    let d = app
                        .file_metadata_cache
                        .get(&key)
                        .and_then(|m| m.duration_secs)
                        .map(fmt_clock_secs)
                        .unwrap_or_else(|| "--:--".to_string());
                    Span::styled(format!("  {}", d), Style::default().fg(C_SECONDARY))
                },
            ]);
            let item_style = if is_selected {
                Style::default().bg(C_SELECTION_BG)
            } else {
                Style::default()
            };
            ListItem::new(line).style(item_style)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::default())
        .highlight_symbol("");
    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(selected_pos.saturating_sub(start)));
    f.render_stateful_widget(list, area, &mut list_state);
}

// ── Info panel (right panel) ──────────────────────────────────────────────────
// The right panel shows:
//   - ICY ticker (top half)
//   - labeled separator " songs "
//   - Songs.csv ticker (bottom half)
// If show_logs is on, a logs section is carved from the bottom.

fn draw_info_panel(f: &mut Frame, app: &mut App, area: Rect) {
    // Pad 1 column from the divider
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(1),
        ..area
    };

    if inner.height == 0 {
        return;
    }

    if app.left_mode == LeftPaneMode::Files {
        // Vertical accordion with 3 right panes in files mode:
        // 2=meta, 3=songs, 4=icy. Only the selected right pane is expanded.
        if inner.height < 4 {
            app.upper_right_rect = inner;
            app.middle_right_rect = inner;
            app.lower_right_rect = inner;
            draw_file_meta_panel(f, app, inner);
            return;
        }

        let expanded = match app.focus_pane {
            FocusPane::Meta | FocusPane::Songs | FocusPane::Icy => app.focus_pane,
            _ => match app.last_focus_pane {
                FocusPane::Meta | FocusPane::Songs | FocusPane::Icy => app.last_focus_pane,
                _ => FocusPane::Meta,
            },
        };

        let h_meta = Rect {
            y: inner.y,
            height: 1,
            ..inner
        };
        let h_songs = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        let h_icy = Rect {
            y: inner.y + 2,
            height: 1,
            ..inner
        };
        let content = Rect {
            y: inner.y + 3,
            height: inner.height.saturating_sub(3),
            ..inner
        };

        draw_labeled_separator_color(
            f,
            h_meta,
            " [2] file meta ",
            if expanded == FocusPane::Meta {
                C_PRIMARY
            } else {
                C_MUTED
            },
        );
        draw_labeled_separator_color(
            f,
            h_songs,
            " [3] songs ",
            if expanded == FocusPane::Songs {
                C_PRIMARY
            } else {
                C_MUTED
            },
        );
        draw_labeled_separator_color(
            f,
            h_icy,
            " [4] icy ",
            if expanded == FocusPane::Icy {
                C_PRIMARY
            } else {
                C_MUTED
            },
        );

        app.upper_right_rect = if expanded == FocusPane::Meta {
            Rect {
                y: h_meta.y,
                height: 1 + content.height,
                ..h_meta
            }
        } else {
            h_meta
        };
        app.middle_right_rect = if expanded == FocusPane::Songs {
            Rect {
                y: h_songs.y,
                height: 1 + content.height,
                ..h_songs
            }
        } else {
            h_songs
        };
        app.lower_right_rect = if expanded == FocusPane::Icy {
            Rect {
                y: h_icy.y,
                height: 1 + content.height,
                ..h_icy
            }
        } else {
            h_icy
        };

        if content.height > 0 {
            match expanded {
                FocusPane::Meta => draw_file_meta_panel(f, app, content),
                FocusPane::Songs => draw_songs_ticker(f, app, content),
                FocusPane::Icy => draw_icy_ticker(f, app, content),
                FocusPane::Left | FocusPane::Nts => draw_file_meta_panel(f, app, content),
            }
        }
        return;
    }

    // Stations mode: optionally show NTS panel above the tickers.
    if (app.show_nts_ch1 || app.show_nts_ch2) && app.left_mode == LeftPaneMode::Stations {
        let ch = if app.show_nts_ch1 { 1u8 } else { 2u8 };
        let (data, err) = if app.show_nts_ch1 {
            (app.nts_ch1.as_ref(), app.nts_ch1_error.as_deref())
        } else {
            (app.nts_ch2.as_ref(), app.nts_ch2_error.as_deref())
        };

        const TICKER_MIN: u16 = 8; // 1 sep + 3 icy + 1 sep + 3 songs
        if inner.height > TICKER_MIN + 4 {
            let ticker_h = TICKER_MIN;
            let nts_h = inner.height - ticker_h - 1; // -1 for the dividing separator
            let nts_area = Rect {
                height: nts_h,
                ..inner
            };
            let div_area = Rect {
                y: inner.y + nts_h,
                height: 1,
                ..inner
            };
            let ticker_area = Rect {
                y: inner.y + nts_h + 1,
                height: ticker_h,
                ..inner
            };
            app.upper_right_rect = nts_area;
            app.lower_right_rect = ticker_area;
            let nts_focused = app.focus_pane == FocusPane::Nts;
            draw_nts_panel(f, ch, data, err, nts_area, app.nts_scroll, nts_focused);
            draw_labeled_separator(f, div_area, " icy / songs ");
            draw_tickers_compact(f, app, ticker_area, 3);
        } else {
            app.upper_right_rect = inner;
            app.lower_right_rect = inner;
            let nts_focused = app.focus_pane == FocusPane::Nts;
            draw_nts_panel(f, ch, data, err, inner, app.nts_scroll, nts_focused);
        }
        return;
    }

    app.upper_right_rect = inner;
    app.lower_right_rect = inner;
    draw_tickers(f, app, inner);
}

// ── NTS live panel ────────────────────────────────────────────────────────────

fn fmt_nts_time_range(show: &NtsShow) -> String {
    let start = show.start.format("%H:%M").to_string();
    let end = show.end.format("%H:%M").to_string();
    format!("{} – {}", start, end)
}

fn draw_nts_panel(
    f: &mut Frame,
    channel: u8,
    data: Option<&NtsChannel>,
    error: Option<&str>,
    area: Rect,
    scroll: usize,
    focused: bool,
) {
    // Header separator — highlighted when this pane is focused
    let title = format!(" [4] NTS Channel {} ", channel);
    let header_color = if focused { C_PRIMARY } else { C_MUTED };
    draw_labeled_separator_color(f, Rect { height: 1, ..area }, &title, header_color);

    let content_area = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };

    if content_area.height == 0 {
        return;
    }

    // Error or loading state
    if let Some(err) = error {
        if data.is_none() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("  error: {}", err),
                    Style::default().fg(C_ACCENT),
                ))),
                content_area,
            );
            return;
        }
    }

    let Some(ch) = data else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  fetching NTS data…",
                Style::default().fg(C_MUTED),
            ))),
            content_area,
        );
        return;
    };

    let now = &ch.now;
    let mut lines: Vec<Line> = Vec::new();

    // ── Current show ──────────────────────────────────────────────────────────
    // Live/Replay indicator + title
    let live_span = if now.is_replay {
        Span::styled("(R) ", Style::default().fg(C_MUTED))
    } else {
        Span::styled("● ", Style::default().fg(C_ACCENT))
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        live_span,
        Span::styled(
            now.broadcast_title.as_str(),
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Time · Location (long only) — on one line, separated by "  ·  "
    {
        let time_str = fmt_nts_time_range(now);
        let mut spans = vec![
            Span::raw("   "),
            Span::styled(time_str, Style::default().fg(C_SECONDARY)),
        ];
        if !now.location_long.is_empty() {
            spans.push(Span::styled("  ·  ", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                now.location_long.as_str(),
                Style::default().fg(C_LOCATION),
            ));
        } else if !now.location_short.is_empty() {
            spans.push(Span::styled("  ·  ", Style::default().fg(C_MUTED)));
            spans.push(Span::styled(
                now.location_short.as_str(),
                Style::default().fg(C_LOCATION),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Genres
    if !now.genres.is_empty() {
        let genre_str = now.genres.join(" · ");
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(genre_str, Style::default().fg(C_TAG)),
        ]));
    }

    // Moods — word-wrapped onto the same indent, can span multiple lines
    if !now.moods.is_empty() {
        let wrap_width = (area.width as usize).saturating_sub(4).max(10);
        let mood_str = now.moods.join(" · ");
        for mood_line in word_wrap(&mood_str, wrap_width) {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(mood_line, Style::default().fg(C_MUTED)),
            ]));
        }
    }

    // Description (wrapped manually into multiple lines)
    if !now.description.is_empty() {
        lines.push(Line::from(""));
        // Word-wrap to panel width (leave 3 cols indent)
        let wrap_width = (area.width as usize).saturating_sub(4).max(10);
        for desc_line in word_wrap(&now.description, wrap_width) {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(desc_line, Style::default().fg(C_PRIMARY)),
            ]));
        }
    }

    // ── Upcoming schedule ─────────────────────────────────────────────────────
    if !ch.upcoming.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " upcoming",
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        )));

        for show in ch.upcoming.iter().take(8) {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(fmt_nts_time_range(show), Style::default().fg(C_SECONDARY)),
                Span::raw("  "),
                Span::styled(
                    show.broadcast_title.as_str(),
                    Style::default().fg(C_PRIMARY),
                ),
                if !show.location_short.is_empty() {
                    Span::styled(
                        format!("  {}", show.location_short),
                        Style::default().fg(C_LOCATION),
                    )
                } else {
                    Span::raw("")
                },
            ]));
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0)),
        content_area,
    );
}

/// Naive word-wrap: splits text into lines of at most `width` chars.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        if paragraph.is_empty() {
            lines.push(String::new());
        }
    }
    lines
}

fn fmt_clock_secs(v: f64) -> String {
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

fn draw_file_meta_panel(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    f.render_widget(Clear, area);
    let mut lines: Vec<Line> = Vec::new();
    let file_hdr = if app.focus_pane == FocusPane::Meta {
        C_PRIMARY
    } else {
        C_MUTED
    };
    lines.push(Line::from(Span::styled(
        " file",
        Style::default().fg(file_hdr).add_modifier(Modifier::BOLD),
    )));

    if let Some(file) = app.files.get(app.file_selected) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(file.name.as_str(), Style::default().fg(C_PRIMARY)),
        ]));

        if let Some(meta) = app.selected_file_metadata.as_ref() {
            if let Some(genre) = meta.genre.as_deref() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(genre, Style::default().fg(C_LOCATION)),
                ]));
            }
            let mut parts: Vec<String> = Vec::new();
            parts.push(format_file_size(file.size_bytes));
            parts.push(
                meta.duration_secs
                    .map(fmt_clock_secs)
                    .unwrap_or_else(|| "--:--".to_string()),
            );
            parts.push(meta.codec.clone().unwrap_or_else(|| "-".to_string()));
            if let Some(br) = meta.bitrate_kbps {
                parts.push(format!("{}k", br));
            }
            if let Some(sr) = meta.sample_rate_hz {
                parts.push(format!("{}Hz", sr));
            }
            if let Some(ch) = meta.channels {
                parts.push(format!("{}ch", ch));
            }
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(parts.join("  ·  "), Style::default().fg(C_SECONDARY)),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " tracklist",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )));

            if !meta.tracklist.is_empty() {
                for item in meta.tracklist.iter().take(160) {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(item.as_str(), Style::default().fg(C_PRIMARY)),
                    ]));
                }
            } else if !meta.chapters.is_empty() {
                for ch in meta.chapters.iter().take(160) {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!(
                                "{}-{}",
                                fmt_clock_secs(ch.start_secs),
                                fmt_clock_secs(ch.end_secs)
                            ),
                            Style::default().fg(C_MUTED),
                        ),
                        Span::raw("  "),
                        Span::styled(ch.title.as_str(), Style::default().fg(C_PRIMARY)),
                    ]));
                }
            } else {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("(no tracklist)", Style::default().fg(C_MUTED)),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  no file selected",
            Style::default().fg(C_MUTED),
        )));
    }

    let q = app.filter_meta.clone();
    let filtered_lines: Vec<Line> = if q.is_empty() {
        lines
    } else {
        lines
            .into_iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|s| search_matches(&q, &s.content.to_string()))
            })
            .collect()
    };
    let visible: Vec<Line> = filtered_lines.into_iter().skip(app.meta_scroll).collect();
    f.render_widget(Paragraph::new(visible).wrap(Wrap { trim: false }), area);
}

/// Draw the ICY ticker and songs.csv ticker, split vertically.
fn draw_tickers(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }

    let sep_h = if area.height >= 3 { 1 } else { 0 };
    let icy_h = if app.files_right_maximized && app.focus_pane == FocusPane::Icy {
        area.height.saturating_sub(sep_h).saturating_sub(3).max(1)
    } else if app.files_right_maximized && app.focus_pane == FocusPane::Songs {
        (area.height / 3).max(1)
    } else {
        (area.height / 2).max(1)
    };
    let songs_h = area.height.saturating_sub(icy_h + sep_h).max(0);

    let icy_area = Rect {
        height: icy_h,
        ..area
    };
    let sep_area = Rect {
        y: area.y + icy_h,
        height: sep_h,
        ..area
    };
    let songs_area = Rect {
        y: area.y + icy_h + sep_h,
        height: songs_h,
        ..area
    };

    app.upper_right_rect = icy_area;
    app.lower_right_rect = songs_area;
    draw_icy_ticker(f, app, icy_area);
    if sep_h > 0 {
        let c = if app.focus_pane == FocusPane::Songs {
            C_PRIMARY
        } else {
            C_MUTED
        };
        draw_labeled_separator_color(f, sep_area, " [3] songs ", c);
    }
    if songs_h > 0 {
        draw_songs_ticker(f, app, songs_area);
    }
}

/// Compact ticker strip: shows at most `max_lines` rows per section.
/// Used below the NTS panel to keep a small preview of icy/songs.
fn draw_tickers_compact(f: &mut Frame, app: &mut App, area: Rect, max_lines: u16) {
    if area.height == 0 {
        return;
    }
    let icy_h = max_lines.min(area.height);
    let remaining = area.height.saturating_sub(icy_h);
    let sep_h = if remaining > 0 { 1u16 } else { 0 };
    let songs_h = remaining.saturating_sub(sep_h);

    let icy_area = Rect {
        height: icy_h,
        ..area
    };
    let sep_area = Rect {
        y: area.y + icy_h,
        height: sep_h,
        ..area
    };
    let songs_area = Rect {
        y: area.y + icy_h + sep_h,
        height: songs_h,
        ..area
    };

    app.upper_right_rect = icy_area;
    app.lower_right_rect = songs_area;
    draw_icy_ticker(f, app, icy_area);
    if sep_h > 0 {
        let c = if app.focus_pane == FocusPane::Songs {
            C_PRIMARY
        } else {
            C_MUTED
        };
        draw_labeled_separator_color(f, sep_area, " [3] songs ", c);
    }
    if songs_h > 0 {
        draw_songs_ticker(f, app, songs_area);
    }
}

fn draw_icy_ticker(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }
    f.render_widget(Clear, area);

    // [2] icy header separator
    let hdr = Rect { height: 1, ..area };
    let focused = app.focus_pane == FocusPane::Icy;
    draw_labeled_separator_color(
        f,
        hdr,
        " [2] icy ",
        if focused { C_PRIMARY } else { C_MUTED },
    );
    let area = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    if area.height == 0 {
        return;
    }

    let q = app.filter_icy.clone();
    let visible_indices: Vec<usize> = (0..app.icy_history.len())
        .rev()
        .filter(|&i| {
            let e = &app.icy_history[i];
            let mut text = e.display.clone();
            if let Some(st) = e.station.as_deref() {
                text.push(' ');
                text.push_str(st);
            }
            search_matches(&q, &text)
        })
        .collect();

    if visible_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no icy metadata matches filter",
                Style::default().fg(C_MUTED),
            ))),
            area,
        );
        return;
    }

    let total = visible_indices.len();
    let max = (area.height as usize).max(1);
    let selected_row = app.icy_selected.min(total.saturating_sub(1));
    let start = if total > max {
        selected_row.saturating_sub(max / 2).min(total - max)
    } else {
        0
    };
    app.icy_view_start = start;
    let lines: Vec<Line> = visible_indices
        .iter()
        .skip(start)
        .take(max)
        .enumerate()
        .map(|(local_i, &idx)| {
            let entry = &app.icy_history[idx];
            let i = start + local_i;
            let style = if i == selected_row {
                if app.focus_pane == FocusPane::Icy {
                    Style::default()
                        .fg(C_PRIMARY)
                        .bg(C_SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_PRIMARY)
                }
            } else if i == 0 {
                Style::default().fg(C_PRIMARY)
            } else {
                Style::default().fg(C_SECONDARY)
            };
            let mut spans = vec![
                Span::styled(
                    if i == selected_row {
                        "  ▸  "
                    } else if i == 0 {
                        "  ♪  "
                    } else {
                        "     "
                    },
                    Style::default().fg(C_MUTED),
                ),
                Span::styled(entry.display.as_str(), style),
            ];
            if let Some(st) = entry.station.as_deref() {
                spans.push(Span::styled(
                    format!("  {}", st),
                    Style::default().fg(C_MUTED),
                ));
            }
            Line::from(spans)
        })
        .collect();

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_songs_ticker(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }
    f.render_widget(Clear, area);

    let q = app.filter_songs.clone();
    let visible_indices: Vec<usize> = (0..app.songs_history.len())
        .rev()
        .filter(|&i| {
            let e = &app.songs_history[i];
            let mut text = e.display.clone();
            if let Some(st) = e.station.as_deref() {
                text.push(' ');
                text.push_str(st);
            }
            if let Some(show) = e.show.as_deref() {
                text.push(' ');
                text.push_str(show);
            }
            if let Some(comment) = e.comment.as_deref() {
                text.push(' ');
                text.push_str(comment);
            }
            search_matches(&q, &text)
        })
        .collect();

    if visible_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no songs match filter",
                Style::default().fg(C_MUTED),
            ))),
            area,
        );
        return;
    }

    let total = visible_indices.len();
    let max = (area.height as usize).max(1);
    let selected_row = app.songs_selected.min(total.saturating_sub(1));
    let start = if total > max {
        selected_row.saturating_sub(max / 2).min(total - max)
    } else {
        0
    };
    app.songs_view_start = start;
    let lines: Vec<Line> = visible_indices
        .iter()
        .skip(start)
        .take(max)
        .enumerate()
        .map(|(local_i, &idx)| {
            let entry = &app.songs_history[idx];
            let i = start + local_i;
            let style = if i == selected_row {
                if app.focus_pane == FocusPane::Songs {
                    Style::default()
                        .fg(C_PRIMARY)
                        .bg(C_SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_PRIMARY)
                }
            } else if i == 0 {
                Style::default().fg(C_PRIMARY)
            } else {
                Style::default().fg(C_SECONDARY)
            };
            let mut spans = vec![
                Span::styled(
                    if i == selected_row {
                        "  ▸  "
                    } else if i == 0 {
                        "  ♫  "
                    } else {
                        "     "
                    },
                    Style::default().fg(C_MUTED),
                ),
                Span::styled(entry.display.as_str(), style),
            ];
            if let Some(ref st) = entry.station {
                spans.push(Span::styled(
                    format!("  {}", st),
                    Style::default().fg(C_MUTED),
                ));
                if let Some(ref show) = entry.show {
                    spans.push(Span::styled(" · ", Style::default().fg(C_MUTED)));
                    spans.push(Span::styled(
                        show.as_str(),
                        Style::default().fg(C_SECONDARY),
                    ));
                }
            }
            Line::from(spans)
        })
        .collect();

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_log_bar(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    f.render_widget(Clear, area);
    let last = app
        .logs
        .last()
        .cloned()
        .or_else(|| app.log_file_lines.first().cloned())
        .unwrap_or_else(|| "(no log)".to_string());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" log ", Style::default().fg(C_MUTED)),
            Span::styled(compact_log_line(&last), Style::default().fg(C_SECONDARY)),
        ])),
        area,
    );
}

fn draw_logs(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    // Solid black background fill
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );
    let lines: Vec<Line> = if app.log_file_lines.is_empty() {
        vec![Line::from(Span::styled(
            "  no log entries yet",
            Style::default().fg(C_MUTED),
        ))]
    } else {
        // log_file_lines is already newest-first (reversed on read)
        app.log_file_lines
            .iter()
            .skip(app.log_scroll)
            .take(area.height as usize)
            .map(|msg| {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        compact_log_line(msg),
                        Style::default().fg(C_MUTED).bg(Color::Black),
                    ),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(Color::Black))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn compact_log_line(raw: &str) -> String {
    let clean = strip_ansi(raw).trim().to_string();
    let mut rest = clean.as_str();
    let mut head: Vec<String> = Vec::new();

    if let Some((tok, rem)) = split_first_token(rest) {
        if let Some(ts) = compact_timestamp(tok) {
            head.push(ts);
            rest = rem.trim_start();
        }
    }

    if let Some((tok, rem)) = split_first_token(rest) {
        let upper = tok.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
        ) {
            head.push(upper);
            rest = rem.trim_start();
        }
    }

    if let Some((left, msg)) = rest.split_once(": ") {
        if !left.is_empty()
            && left.len() <= 48
            && left
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
        {
            rest = msg.trim_start();
        }
    }

    if head.is_empty() {
        rest.to_string()
    } else if rest.is_empty() {
        head.join(" ")
    } else {
        format!("{} {}", head.join(" "), rest)
    }
}

fn compact_timestamp(token: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(token).ok()?;
    let local = dt.with_timezone(&chrono::Local);
    let fmt = if local.date_naive() == chrono::Local::now().date_naive() {
        "%H:%M:%S"
    } else {
        "%m-%d %H:%M"
    };
    Some(local.format(fmt).to_string())
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.splitn(2, char::is_whitespace);
    let first = parts.next()?.trim();
    if first.is_empty() {
        return None;
    }
    Some((first, parts.next().unwrap_or("")))
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ('@'..='~').contains(&ch) {
                in_escape = false;
            }
            continue;
        }
        if ch == '\u{1b}' {
            in_escape = true;
            continue;
        }
        out.push(ch);
    }
    out
}

// ── Keybindings footer ────────────────────────────────────────────────────────

fn draw_keybindings(f: &mut Frame, area: Rect) {
    let pairs: &[(&str, &str)] = &[
        ("↑↓", "navigate"),
        ("tab", "pane"),
        ("`", "last pane"),
        ("f", "radio/files"),
        ("_", "max tab"),
        ("/", "filter"),
        ("s", "sort"),
        ("enter", "play"),
        ("y", "copy entry"),
        ("spc", "stop"),
        (",/.", "seek"),
        ("R", "undo random"),
        ("m", "mute"),
        ("*", "stars"),
        ("r", "shuffle"),
        ("←→", "vol"),
        ("l", "logs"),
        ("1/2/3", "focus panes"),
        ("!/@", "nts ch1/ch2"),
        ("h", "hide bar"),
        ("?", "help"),
        ("q", "quit"),
    ];

    let mut spans = vec![Span::raw("  ")];
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default()));
        }
        spans.push(Span::styled(
            *key,
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", label),
            Style::default().fg(C_MUTED),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Help overlay ──────────────────────────────────────────────────────────────

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 24, area);

    let help_lines: Vec<Line> = vec![
        Line::from(Span::styled(
            " keyboard shortcuts",
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        help_row("↑ / ↓", "navigate station list"),
        help_row("pg up / pg dn", "jump 10 stations"),
        help_row("home / end", "jump to first / last"),
        help_row("enter", "play selected station"),
        help_row("space", "stop playback"),
        help_row(", / .", "seek file backward / forward"),
        help_row("R", "return to previous file/time"),
        help_row("m", "mute / unmute"),
        help_row("*", "cycle stars on selected item"),
        help_row("n", "next station"),
        help_row("p", "previous station"),
        help_row("r", "random station"),
        help_row("← / →", "volume down / up"),
        help_row("+ / -", "volume up / down"),
        Line::from(""),
        help_row("tab", "cycle focused pane"),
        help_row("`", "jump to previous pane"),
        help_row("f", "toggle radio/files workspace"),
        help_row("1/2/3", "focus left / upper-right / lower-right"),
        help_row("_", "toggle focused right-tab height"),
        help_row("y", "copy selected entry in pane 2/3"),
        help_row("d (songs pane)", "download via nts_get URL"),
        Line::from(""),
        help_row("/", "filter stations"),
        help_row("esc (in filter)", "clear filter"),
        help_row("enter (in filter)", "confirm filter"),
        Line::from(""),
        help_row("s", "cycle sort modes for current workspace"),
        help_row("S", "cycle sort modes backward"),
        Line::from(""),
        help_row("l", "toggle log panel"),
        help_row("pg up/down + home/end", "scroll log panel (when open)"),
        help_row("!", "toggle NTS Channel 1"),
        help_row("@", "toggle NTS Channel 2"),
        help_row("h", "toggle keybindings bar"),
        help_row("?", "toggle this help"),
        help_row("q", "quit"),
        Line::from(""),
        Line::from(Span::styled(
            " press ? to close",
            Style::default().fg(C_MUTED),
        )),
    ];

    let widget = Paragraph::new(help_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(C_PANEL_BORDER))
                .style(Style::default().bg(Color::Rgb(18, 18, 26))),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(Clear, popup);
    f.render_widget(widget, popup);
}

fn help_row<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{:<14}", key),
            Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc, Style::default().fg(C_SECONDARY)),
    ])
}

// ── Error popup ───────────────────────────────────────────────────────────────

fn draw_error_popup(f: &mut Frame, msg: &str, area: Rect) {
    let popup = centered_rect(60, 5, area);
    let widget = Paragraph::new(msg)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(C_ACCENT))
                .title(" error "),
        )
        .style(Style::default().fg(C_ACCENT))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });

    f.render_widget(Clear, popup);
    f.render_widget(widget, popup);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}
