// Oscilloscope display mode — ported from scope-tui.
// Renders audio samples as a waveform using ratatui Chart.

use ratatui::{
    style::Style,
    text::Span,
    widgets::{Axis, GraphType},
};

use super::{DataSet, Dimension, DisplayMode, GraphConfig, Matrix};

pub struct Oscilloscope {
    pub peaks: bool,
}

impl Default for Oscilloscope {
    fn default() -> Self {
        Self { peaks: false }
    }
}

impl DisplayMode for Oscilloscope {
    fn axis(&self, cfg: &GraphConfig, dimension: Dimension) -> Axis<'_> {
        let (name, bounds) = match dimension {
            Dimension::X => ("", [0.0, cfg.samples as f64]),
            Dimension::Y => ("", [-cfg.scale, cfg.scale]),
        };
        let mut a = Axis::default();
        if cfg.show_ui {
            a = a.title(Span::styled(name, Style::default().fg(cfg.labels_color)));
        }
        a.style(Style::default().fg(cfg.axis_color)).bounds(bounds)
    }

    fn references(&self, cfg: &GraphConfig) -> Vec<DataSet> {
        vec![DataSet::new(
            None,
            vec![(0.0, 0.0), (cfg.samples as f64, 0.0)],
            cfg.marker_type,
            GraphType::Line,
            cfg.axis_color,
        )]
    }

    fn process(&mut self, cfg: &GraphConfig, data: &Matrix) -> Vec<DataSet> {
        let mut out = Vec::new();

        for (n, channel) in data.iter().enumerate().rev() {
            let (mut min, mut max) = (0.0_f64, 0.0_f64);
            let mut pts = Vec::with_capacity(channel.len());
            for (i, &s) in channel.iter().enumerate() {
                if s < min {
                    min = s;
                }
                if s > max {
                    max = s;
                }
                pts.push((i as f64, s));
            }

            if self.peaks {
                out.push(DataSet::new(
                    None,
                    vec![(0.0, min), (0.0, max)],
                    cfg.marker_type,
                    GraphType::Scatter,
                    cfg.palette(n),
                ));
            }

            out.push(DataSet::new(
                None,
                pts,
                cfg.marker_type,
                if cfg.scatter {
                    GraphType::Scatter
                } else {
                    GraphType::Line
                },
                cfg.palette(n),
            ));
        }

        out
    }
}
