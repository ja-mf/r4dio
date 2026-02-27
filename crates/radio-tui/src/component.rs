//! Component trait — the interface every UI panel implements.
//!
//! Design principles:
//! - Components are self-contained: they own their state and render themselves.
//! - Components receive `AppState` (read-only) for data they don't own.
//! - Components produce `Vec<Action>` — they never mutate shared state directly.
//! - The App event-loop dispatches those actions to the appropriate targets.

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::action::{Action, ComponentId};
use crate::app_state::AppState;

/// The trait every focusable panel implements.
pub trait Component {
    /// Which component is this?
    fn id(&self) -> ComponentId;

    /// Handle a key event. Returns actions to be dispatched.
    /// Only called when this component has focus (or for global keys).
    fn handle_key(&mut self, key: KeyEvent, state: &AppState) -> Vec<Action>;

    /// Handle a mouse event. Returns actions to be dispatched.
    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, state: &AppState) -> Vec<Action>;

    /// Called each tick (~100ms). For time-based updates, expiry checks, etc.
    fn tick(&mut self, _state: &AppState) -> Vec<Action> {
        Vec::new()
    }

    /// Receive an action dispatched by the App.
    /// Components can react to actions even when not focused.
    fn on_action(&mut self, action: &Action, state: &AppState) -> Vec<Action>;

    /// Render the component into `area`.
    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool, state: &AppState);

    /// The minimum height required to render meaningfully.
    fn min_height(&self) -> u16 {
        3
    }

    /// One-line summary shown in the collapsed header strip.
    /// Return `None` to show just the title with no extra info.
    fn collapse_summary(&self, _state: &AppState) -> Option<String> {
        None
    }
}
