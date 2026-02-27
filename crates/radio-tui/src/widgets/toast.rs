//! Toast notification system — transient status messages.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use crate::theme::{C_TOAST_ERROR, C_TOAST_INFO, C_TOAST_SUCCESS, C_TOAST_WARNING};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

struct Toast {
    message: String,
    severity: Severity,
    expires: Instant,
}

/// A persistent spinner toast that animates until resolved.
struct SpinnerToast {
    message: String,
    frame: usize,
}

const SPINNER_FRAMES: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

pub struct ToastManager {
    toasts: VecDeque<Toast>,
    spinner: Option<SpinnerToast>,
    max_visible: usize,
}

impl ToastManager {
    pub fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
            spinner: None,
            max_visible: 4,
        }
    }

    pub fn push(&mut self, message: impl Into<String>, severity: Severity, duration: Duration) {
        // Remove duplicates (same message)
        let msg = message.into();
        self.toasts.retain(|t| t.message != msg);
        self.toasts.push_back(Toast {
            message: msg,
            severity,
            expires: Instant::now() + duration,
        });
        // Cap queue
        while self.toasts.len() > self.max_visible * 2 {
            self.toasts.pop_front();
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(message, Severity::Info, Duration::from_secs(3));
    }

    pub fn success(&mut self, message: impl Into<String>) {
        self.push(message, Severity::Success, Duration::from_secs(3));
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.push(message, Severity::Warning, Duration::from_secs(4));
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(message, Severity::Error, Duration::from_secs(5));
    }

    /// Start or replace the persistent spinner toast.  The spinner animates
    /// on every `tick()` call and does not expire until `resolve_spinner` is
    /// called.
    pub fn spinner(&mut self, message: impl Into<String>) {
        self.spinner = Some(SpinnerToast {
            message: message.into(),
            frame: 0,
        });
    }

    /// Resolve the active spinner toast: dismiss it and push a normal expiring
    /// toast in its place.
    pub fn resolve_spinner(
        &mut self,
        severity: Severity,
        message: impl Into<String>,
        duration: Duration,
    ) {
        self.spinner = None;
        self.push(message, severity, duration);
    }

    /// Dismiss the active spinner without replacing it (e.g. on error paths
    /// that have no result to show).
    pub fn dismiss_spinner(&mut self) {
        self.spinner = None;
    }

    /// Remove expired toasts and advance the spinner frame. Call each tick.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|t| t.expires > now);
        if let Some(ref mut s) = self.spinner {
            s.frame = (s.frame + 1) % SPINNER_FRAMES.len();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty() && self.spinner.is_none()
    }

    /// Render toasts in the top-right corner of `area`.
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        if self.is_empty() {
            return;
        }
        let max_width = (area.width / 2).min(60).max(30);

        // Collect normal toasts (newest first) and optionally prepend spinner
        let mut y = area.y + 1;

        // Spinner always rendered first (topmost row)
        if let Some(ref s) = self.spinner {
            let icon = SPINNER_FRAMES[s.frame % SPINNER_FRAMES.len()];
            let msg_len = s.message.chars().count() as u16;
            let w = (msg_len + 4).min(max_width);
            let x = area.x + area.width.saturating_sub(w + 1);
            let toast_area = Rect {
                x,
                y,
                width: w,
                height: 1,
            };
            frame.render_widget(Clear, toast_area);
            let paragraph = Paragraph::new(Line::from(vec![Span::styled(
                format!(" {} {} ", icon, &s.message),
                Style::default()
                    .fg(C_TOAST_INFO)
                    .add_modifier(Modifier::BOLD),
            )]));
            frame.render_widget(paragraph, toast_area);
            y += 1;
            if y >= area.y + area.height {
                return;
            }
        }

        // Normal toasts below the spinner
        let visible: Vec<&Toast> = self.toasts.iter().rev().take(self.max_visible).collect();

        for toast in visible {
            let msg_len = toast.message.chars().count() as u16;
            let w = (msg_len + 4).min(max_width);
            let x = area.x + area.width.saturating_sub(w + 1);

            let color = match toast.severity {
                Severity::Info => C_TOAST_INFO,
                Severity::Success => C_TOAST_SUCCESS,
                Severity::Warning => C_TOAST_WARNING,
                Severity::Error => C_TOAST_ERROR,
            };

            let icon = match toast.severity {
                Severity::Info => "·",
                Severity::Success => "✓",
                Severity::Warning => "!",
                Severity::Error => "✗",
            };

            let toast_area = Rect {
                x,
                y,
                width: w,
                height: 1,
            };
            frame.render_widget(Clear, toast_area);
            let paragraph = Paragraph::new(Line::from(vec![Span::styled(
                format!(" {} {} ", icon, &toast.message),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )]));
            frame.render_widget(paragraph, toast_area);

            y += 1;
            if y >= area.y + area.height {
                break;
            }
        }
    }
}

impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}
