use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    symbols::Marker,
    widgets::{Block, Chart, Dataset},
    Frame,
};

use crate::app_state::AppState;
use crate::scope::{oscilloscope::Oscilloscope, DataSet, DisplayMode, GraphConfig, Matrix};

/// Default number of PCM samples displayed per scope frame.
/// At 44100 Hz: 4096 samples ≈ 93 ms of audio.
pub const SCOPE_SAMPLES: usize = 4096;

pub struct ScopePanel {
    oscilloscope: Oscilloscope,
    graph_cfg: GraphConfig,
    matrix: Matrix,
}

impl Default for ScopePanel {
    fn default() -> Self {
        Self {
            oscilloscope: Oscilloscope::default(),
            graph_cfg: GraphConfig {
                // scale=1.0: full PCM range (-1..1) fits exactly in display.
                // User can zoom with Up/Down like scope-tui.
                samples: SCOPE_SAMPLES as u32,
                scale: 1.0,
                scatter: false,
                references: false,
                show_ui: false,
                marker_type: Marker::Braille,
                palette: vec![Color::Rgb(0, 200, 180)],
                labels_color: Color::DarkGray,
                axis_color: Color::Rgb(40, 40, 40),
            },
            matrix: vec![Vec::new()],
        }
    }
}

impl ScopePanel {
    /// Handle scope-tui-style keys.
    ///
    /// Up / Down          — scale ± 0.01 (× 10 with Shift)
    /// Left / Right       — samples ± 25  (× 10 with Shift)
    /// Esc                — reset to defaults
    pub fn handle_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let magnitude: f64 = if shift { 10.0 } else { 1.0 };

        match key.code {
            // ── Scale (Y zoom) ────────────────────────────────────────────────
            KeyCode::Up => {
                self.graph_cfg.scale = (self.graph_cfg.scale + 0.01 * magnitude)
                    .min(10.0)
                    .max(0.01);
            }
            KeyCode::Down => {
                self.graph_cfg.scale = (self.graph_cfg.scale - 0.01 * magnitude)
                    .min(10.0)
                    .max(0.01);
            }
            // ── Sample window (X zoom) ────────────────────────────────────────
            KeyCode::Right => {
                let inc = (25.0 * magnitude) as u32;
                self.graph_cfg.samples = self
                    .graph_cfg
                    .samples
                    .saturating_add(inc)
                    .min(SCOPE_SAMPLES as u32 * 4);
            }
            KeyCode::Left => {
                let dec = (25.0 * magnitude) as u32;
                self.graph_cfg.samples = self.graph_cfg.samples.saturating_sub(dec).max(64);
            }
            // ── Reset ─────────────────────────────────────────────────────────
            KeyCode::Esc => {
                self.graph_cfg.scale = 1.0;
                self.graph_cfg.samples = SCOPE_SAMPLES as u32;
            }
            _ => {}
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect, state: &AppState) {
        let n = self.graph_cfg.samples as usize;
        let ring = &state.pcm_ring;

        if self.matrix.is_empty() {
            self.matrix.push(Vec::new());
        }
        let channel = &mut self.matrix[0];
        channel.clear();
        channel.resize(n, 0.0_f64);

        let skip = ring.len().saturating_sub(n);
        let offset = n.saturating_sub(ring.len().min(n));
        for (i, &s) in ring.iter().skip(skip).enumerate() {
            channel[offset + i] = s as f64;
        }

        let mut all_datasets: Vec<DataSet> = Vec::new();
        if self.graph_cfg.references {
            all_datasets.extend(self.oscilloscope.references(&self.graph_cfg));
        }
        all_datasets.extend(self.oscilloscope.process(&self.graph_cfg, &self.matrix));

        let ratatui_datasets: Vec<Dataset> = all_datasets.iter().map(|ds| ds.into()).collect();

        let x_axis = self
            .oscilloscope
            .axis(&self.graph_cfg, crate::scope::Dimension::X);
        let y_axis = self
            .oscilloscope
            .axis(&self.graph_cfg, crate::scope::Dimension::Y);

        let block = Block::default().style(Style::default().bg(crate::theme::C_BG));

        let chart = Chart::new(ratatui_datasets)
            .block(block)
            .x_axis(x_axis)
            .y_axis(y_axis);

        frame.render_widget(chart, area);
    }
}
