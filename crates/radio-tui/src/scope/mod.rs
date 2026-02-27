// Scope display module — ported from scope-tui (https://github.com/alemi/scope-tui)
// Adapted for use in r4dio: stripped to oscilloscope-only, no crate dependency.

pub mod oscilloscope;

use ratatui::{
    style::{Color, Style},
    symbols::Marker,
    widgets::{Axis, Dataset, GraphType},
};

pub type Matrix = Vec<Vec<f64>>;

pub enum Dimension {
    X,
    Y,
}

#[derive(Debug, Clone)]
pub struct GraphConfig {
    pub samples: u32,
    pub scale: f64,
    pub scatter: bool,
    pub references: bool,
    pub show_ui: bool,
    pub marker_type: Marker,
    pub palette: Vec<Color>,
    pub labels_color: Color,
    pub axis_color: Color,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            samples: 2048,
            scale: 1.0,
            scatter: false,
            references: true,
            show_ui: false,
            marker_type: Marker::Braille,
            palette: vec![Color::Cyan],
            labels_color: Color::DarkGray,
            axis_color: Color::DarkGray,
        }
    }
}

impl GraphConfig {
    pub fn palette(&self, index: usize) -> Color {
        *self.palette.get(index % self.palette.len()).unwrap_or(&Color::White)
    }
}

pub trait DisplayMode {
    fn axis(&self, cfg: &GraphConfig, dimension: Dimension) -> Axis<'_>;
    fn process(&mut self, cfg: &GraphConfig, data: &Matrix) -> Vec<DataSet>;
    fn references(&self, _cfg: &GraphConfig) -> Vec<DataSet> { vec![] }
}

pub struct DataSet {
    pub name: Option<String>,
    pub data: Vec<(f64, f64)>,
    pub marker_type: Marker,
    pub graph_type: GraphType,
    pub color: Color,
}

impl DataSet {
    pub fn new(
        name: Option<String>,
        data: Vec<(f64, f64)>,
        marker_type: Marker,
        graph_type: GraphType,
        color: Color,
    ) -> Self {
        Self { name, data, marker_type, graph_type, color }
    }
}

impl<'a> From<&'a DataSet> for Dataset<'a> {
    fn from(ds: &'a DataSet) -> Dataset<'a> {
        let mut out = Dataset::default();
        if let Some(name) = &ds.name {
            out = out.name(name.clone());
        }
        out.marker(ds.marker_type)
            .graph_type(ds.graph_type)
            .style(Style::default().fg(ds.color))
            .data(&ds.data)
    }
}
