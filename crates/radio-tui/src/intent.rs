//! Pending-intent tracking for actions with confirmation latency.
//!
//! When the user presses a key (e.g. pause), we send a command to the daemon
//! and wait for confirmation via a state broadcast.  During that window the UI
//! should show a "pending" indicator rather than immediately flipping state.
//!
//! # States
//! ```
//!  Confirmed(T)          — daemon confirmed; render normally
//!  Pending { ... }       — command sent, no confirmation yet; render dimmed/pulsing
//!  TimedOut { ... }      — waited too long; render with warning colour + "?"
//! ```
//!
//! The `IntentTracker` wraps one `IntentState<T>` and handles all transitions.

use std::time::{Duration, Instant};

/// Timeout before a pending intent becomes `TimedOut`.
pub const INTENT_TIMEOUT: Duration = Duration::from_millis(3000);

/// Three-state wrapper for a value that may be waiting for confirmation.
#[derive(Debug, Clone)]
pub enum IntentState<T: Clone + PartialEq> {
    /// Daemon has confirmed this value.
    Confirmed(T),
    /// Command sent; waiting for daemon to echo back `intended`.
    Pending {
        intended: T,
        confirmed: T,
        since: Instant,
    },
    /// Daemon didn't confirm within `INTENT_TIMEOUT`.
    TimedOut { intended: T, confirmed: T },
}

impl<T: Clone + PartialEq> IntentState<T> {
    /// Create a new confirmed state.
    pub fn new(value: T) -> Self {
        Self::Confirmed(value)
    }

    /// The value the user intended (most relevant for display).
    pub fn intended(&self) -> &T {
        match self {
            Self::Confirmed(v) => v,
            Self::Pending { intended, .. } => intended,
            Self::TimedOut { intended, .. } => intended,
        }
    }

    /// The last confirmed value from the daemon.
    pub fn confirmed(&self) -> &T {
        match self {
            Self::Confirmed(v) => v,
            Self::Pending { confirmed, .. } => confirmed,
            Self::TimedOut { confirmed, .. } => confirmed,
        }
    }

    /// True if currently waiting for confirmation.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    /// True if the intent timed out without confirmation.
    pub fn is_timed_out(&self) -> bool {
        matches!(self, Self::TimedOut { .. })
    }

    /// True when in normal confirmed state.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }

    /// Register a user intent (command was sent to daemon).
    /// Transitions to `Pending` unless `intended == current confirmed`.
    pub fn set_intent(&mut self, intended: T) {
        let confirmed = self.confirmed().clone();
        if intended == confirmed {
            // Already matches — no need to wait
            *self = Self::Confirmed(intended);
        } else {
            *self = Self::Pending {
                intended,
                confirmed,
                since: Instant::now(),
            };
        }
    }

    /// Called every tick to check for timeout.  Returns `true` if state changed.
    pub fn tick(&mut self) -> bool {
        if let Self::Pending {
            intended,
            confirmed,
            since,
        } = self
        {
            if since.elapsed() >= INTENT_TIMEOUT {
                *self = Self::TimedOut {
                    intended: intended.clone(),
                    confirmed: confirmed.clone(),
                };
                return true;
            }
        }
        false
    }

    /// Called when the daemon broadcasts a new confirmed value.
    /// Transitions back to `Confirmed` if the value matches the intent.
    /// Returns `true` if the state changed.
    pub fn on_confirmed(&mut self, value: T) -> bool {
        match self {
            Self::Pending { intended, .. } => {
                if value == *intended {
                    *self = Self::Confirmed(value);
                    return true;
                }
                // Different value came in — update confirmed but stay pending
                if let Self::Pending { confirmed, .. } = self {
                    *confirmed = value;
                }
                false
            }
            Self::TimedOut { intended, .. } => {
                // Accept whatever the daemon says
                let matches = value == *intended;
                *self = Self::Confirmed(value);
                matches
            }
            Self::Confirmed(v) => {
                if *v != value {
                    *self = Self::Confirmed(value);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Visual modifier for rendering: returns `(is_pending, is_timed_out)`.
    pub fn render_state(&self) -> RenderHint {
        match self {
            Self::Confirmed(_) => RenderHint::Normal,
            Self::Pending { since, .. } => {
                // Pulse on/off every 400ms
                let pulsing = (since.elapsed().as_millis() / 400) % 2 == 0;
                if pulsing {
                    RenderHint::PendingVisible
                } else {
                    RenderHint::PendingHidden
                }
            }
            Self::TimedOut { .. } => RenderHint::TimedOut,
        }
    }
}

/// How to render a value that may be pending confirmation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderHint {
    /// Render normally.
    Normal,
    /// Pending, show icon (pulse-on frame).
    PendingVisible,
    /// Pending, hide icon (pulse-off frame).
    PendingHidden,
    /// Timed out — render with warning colour and "?" suffix.
    TimedOut,
}
