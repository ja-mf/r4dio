mod ascii_core;
mod input;
mod signals;
mod visuals;

use std::{io, time::Duration, time::Instant};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};

use crate::{
    ascii_core::{AsciiFrame, AsciiRenderCore, RenderTuning},
    input::{AudioInput, InputConfig},
    signals::VisualSignalCore,
    visuals::{FieldTuning, VisualizationEngine},
};

#[derive(Debug, Parser)]
#[command(name = "sound-viz-lab", version, about = "Audio-reactive ASCII visualization lab")]
struct Args {
    /// List available CPAL input devices and quit.
    #[arg(long, default_value_t = false)]
    list_devices: bool,

    /// Exact CPAL input device name to use (e.g. BlackHole 2ch on macOS).
    #[arg(long)]
    device: Option<String>,

    /// Preferred input channels.
    #[arg(short, long, default_value_t = 2)]
    channels: usize,

    /// Preferred sample rate (Hz).
    #[arg(short = 'r', long, default_value_t = 48_000)]
    sample_rate: u32,

    /// Preferred callback buffer size (frames).
    #[arg(short, long, default_value_t = 1024)]
    buffer: u32,

    /// CPAL callback timeout in seconds.
    #[arg(long, default_value_t = 5)]
    timeout_secs: u64,

    /// Target render FPS.
    #[arg(long, default_value_t = 25)]
    fps: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.list_devices {
        input::list_input_devices()?;
        return Ok(());
    }

    let cfg = InputConfig {
        device: args.device.clone(),
        channels: args.channels,
        sample_rate: args.sample_rate,
        buffer: args.buffer,
        timeout_secs: args.timeout_secs,
    };
    let mut input = AudioInput::new(&cfg)?;

    eprintln!(
        "sound-viz-lab: using '{}' (device={}ch, active={}ch @ {}Hz)",
        input.device_name, input.channels, input.active_channels, input.sample_rate
    );
    eprintln!("controls: Tab switch viz · arrows tune · [ ] motion · +/- fps · d dither · q quit");

    let mut signal_core = VisualSignalCore::new(input.sample_rate as f32);
    let mut field_tuning = FieldTuning::default();
    let mut render_tuning = RenderTuning::default();
    let mut renderer = AsciiRenderCore::new();
    let mut visuals = VisualizationEngine::new(120, 40);
    let mut mode_dialog = ModeDialogState::default();

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let run_result = run_loop(
        &mut terminal,
        &mut input,
        &mut signal_core,
        &mut visuals,
        &mut renderer,
        &mut field_tuning,
        &mut render_tuning,
        args.fps.max(5),
        &mut mode_dialog,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    run_result
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    input: &mut AudioInput,
    signal_core: &mut VisualSignalCore,
    visuals: &mut VisualizationEngine,
    renderer: &mut AsciiRenderCore,
    field_tuning: &mut FieldTuning,
    render_tuning: &mut RenderTuning,
    initial_fps: u32,
    mode_dialog: &mut ModeDialogState,
) -> Result<()> {
    let mut fps = initial_fps.clamp(5, 120);
    let mut should_quit = false;

    while !should_quit {
        let frame_start = Instant::now();
        let dt = 1.0 / fps as f32;
        let frame_time = Duration::from_secs_f32(dt);

        for chunk in input.drain_pcm_chunks() {
            signal_core.update_from_pcm_chunk(chunk.0.as_ref());
        }

        let size = terminal.size()?;
        let field_w = size.width.saturating_sub(2).max(1) as usize;
        let field_h = size.height.saturating_sub(4).max(1) as usize;
        visuals.resize(field_w, field_h);
        visuals.update(signal_core.latest(), dt, *field_tuning);

        render_tuning.zoom = field_tuning.zoom;
        let dither_audio_boost = (0.35 + signal_core.latest().high * 0.65).clamp(0.0, 1.0);
        let mut tuned = *render_tuning;
        tuned.dither_strength = (render_tuning.dither_strength * dither_audio_boost).clamp(0.0, 1.0);

        let (field, w, h) = visuals.field();
        let frame = renderer.render(field, w, h, tuned, signal_core.latest(), visuals.mode());

        terminal.draw(|f| {
            draw_ui(
                f,
                &frame,
                visuals.mode().label(),
                signal_core.latest(),
                input.device_name.as_str(),
                *field_tuning,
                *render_tuning,
                fps,
                mode_dialog,
            );
        })?;

        loop {
            let elapsed = frame_start.elapsed();
            if elapsed >= frame_time {
                break;
            }
            let wait = frame_time - elapsed;
            if !event::poll(wait)? {
                break;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(
                        key.code,
                        key.modifiers,
                        &mut should_quit,
                        &mut fps,
                        visuals,
                        field_tuning,
                        render_tuning,
                        mode_dialog,
                    );
                }
            }
        }

        while event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(
                        key.code,
                        key.modifiers,
                        &mut should_quit,
                        &mut fps,
                        visuals,
                        field_tuning,
                        render_tuning,
                        mode_dialog,
                    );
                }
            }
        }
    }

    Ok(())
}

fn draw_ui(
    frame: &mut ratatui::Frame,
    ascii_frame: &AsciiFrame,
    mode_label: &str,
    sig: &signals::VisualSignals,
    device_name: &str,
    field_tuning: FieldTuning,
    render_tuning: RenderTuning,
    fps: u32,
    mode_dialog: &ModeDialogState,
) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(frame.area());

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" sound-viz-lab · {mode_label} "));
    let inner = block.inner(areas[0]);
    frame.render_widget(block, areas[0]);

    let text_lines: Vec<Line> = ascii_frame
        .rows
        .iter()
        .map(|row| {
            if row.is_empty() {
                return Line::raw("");
            }
            let mut spans = Vec::new();
            let mut run_color = row[0].color;
            let mut run = String::new();
            for cell in row {
                if cell.color == run_color {
                    run.push(cell.ch);
                } else {
                    spans.push(Span::styled(run, Style::default().fg(run_color)));
                    run = String::new();
                    run.push(cell.ch);
                    run_color = cell.color;
                }
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, Style::default().fg(run_color)));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(text_lines), inner);

    let controls = Line::from(vec![
        Span::styled("Tab", Style::default().fg(Color::Yellow)),
        Span::raw(" switch  "),
        Span::styled("l", Style::default().fg(Color::Yellow)),
        Span::raw(" list  "),
        Span::styled("↑↓", Style::default().fg(Color::Yellow)),
        Span::raw(" zoom  "),
        Span::styled("←→", Style::default().fg(Color::Yellow)),
        Span::raw(" contrast  "),
        Span::styled("[ ]", Style::default().fg(Color::Yellow)),
        Span::raw(" motion  "),
        Span::styled("+/-", Style::default().fg(Color::Yellow)),
        Span::raw(" fps  "),
        Span::styled("d", Style::default().fg(Color::Yellow)),
        Span::raw(" dither  "),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]);

    let metrics = format!(
        "dev:{}  fps:{}  zoom:{:.2}  global:{:.2}  dir:{:.2}  motion:{:.2}  dither:{}  rms:{:.1}  low:{:.2} mid:{:.2} high:{:.2}  flux:{:.2} pulse:{:.2} tr:{:.2}",
        device_name,
        fps,
        field_tuning.zoom,
        render_tuning.global_contrast,
        render_tuning.directional_contrast,
        field_tuning.motion,
        render_tuning.dither_mode.label(),
        sig.rms_db,
        sig.low,
        sig.mid,
        sig.high,
        sig.spectral_flux,
        sig.pulse,
        sig.transient
    );
    let info = Line::from(Span::styled(metrics, Style::default().fg(Color::Gray)));

    frame.render_widget(Paragraph::new(vec![controls, info]), areas[1]);
    if mode_dialog.open {
        draw_mode_dialog(frame, mode_dialog);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    should_quit: &mut bool,
    fps: &mut u32,
    visuals: &mut VisualizationEngine,
    field_tuning: &mut FieldTuning,
    render_tuning: &mut RenderTuning,
    mode_dialog: &mut ModeDialogState,
) {
    if mode_dialog.open {
        match code {
            KeyCode::Esc | KeyCode::Char('l') => mode_dialog.open = false,
            KeyCode::Up => {
                if mode_dialog.selected == 0 {
                    mode_dialog.selected = visuals::VizMode::all().len().saturating_sub(1);
                } else {
                    mode_dialog.selected -= 1;
                }
            }
            KeyCode::Down => {
                mode_dialog.selected = (mode_dialog.selected + 1) % visuals::VizMode::all().len();
            }
            KeyCode::Enter => {
                if let Some(mode) = visuals::VizMode::all().get(mode_dialog.selected).copied() {
                    visuals.set_mode(mode);
                }
                mode_dialog.open = false;
            }
            _ => {}
        }
        return;
    }

    let coarse = modifiers.contains(KeyModifiers::SHIFT);
    let zoom_step = if coarse { 0.35 } else { 0.08 };
    let contrast_step = if coarse { 0.30 } else { 0.08 };
    let motion_step = if coarse { 0.25 } else { 0.05 };

    match code {
        KeyCode::Char('q') | KeyCode::Esc => *should_quit = true,
        KeyCode::Tab | KeyCode::BackTab => visuals.next_mode(),
        KeyCode::Char('l') => {
            mode_dialog.open = true;
            mode_dialog.selected = visuals.mode().index();
        }
        KeyCode::Up => field_tuning.zoom = (field_tuning.zoom + zoom_step).clamp(0.35, 6.0),
        KeyCode::Down => field_tuning.zoom = (field_tuning.zoom - zoom_step).clamp(0.35, 6.0),
        KeyCode::Right => {
            render_tuning.global_contrast = (render_tuning.global_contrast + contrast_step).clamp(1.0, 4.5);
            render_tuning.directional_contrast =
                (render_tuning.directional_contrast + contrast_step).clamp(1.0, 4.5);
        }
        KeyCode::Left => {
            render_tuning.global_contrast = (render_tuning.global_contrast - contrast_step).clamp(1.0, 4.5);
            render_tuning.directional_contrast =
                (render_tuning.directional_contrast - contrast_step).clamp(1.0, 4.5);
        }
        KeyCode::Char(']') => field_tuning.motion = (field_tuning.motion + motion_step).clamp(0.1, 3.0),
        KeyCode::Char('[') => field_tuning.motion = (field_tuning.motion - motion_step).clamp(0.1, 3.0),
        KeyCode::Char('d') => render_tuning.dither_mode = render_tuning.dither_mode.next(),
        KeyCode::Char('+') | KeyCode::Char('=') => *fps = (*fps + if coarse { 10 } else { 1 }).clamp(5, 120),
        KeyCode::Char('-') => *fps = fps.saturating_sub(if coarse { 10 } else { 1 }).clamp(5, 120),
        KeyCode::Char('r') => {
            *field_tuning = FieldTuning::default();
            *render_tuning = RenderTuning::default();
        }
        _ => {}
    }
}

#[derive(Debug, Default)]
struct ModeDialogState {
    open: bool,
    selected: usize,
}

fn draw_mode_dialog(frame: &mut ratatui::Frame, dialog: &ModeDialogState) {
    let area = centered_rect(50, 60, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Select Visualization (Enter) ")
        .style(Style::default().bg(Color::Rgb(12, 12, 16)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (idx, mode) in visuals::VizMode::all().iter().enumerate() {
        let sel = idx == dialog.selected;
        let prefix = if sel { "❯ " } else { "  " };
        let style = if sel {
            Style::default().fg(Color::Black).bg(Color::Rgb(180, 200, 255))
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{}", mode.label()), style)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ move • Enter select • Esc/L close",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
