//! WorkspaceManager — manages Radio/Files tab switching and per-workspace layout.
//!
//! Tracks:
//! - Which workspace is active (Radio = station list, Files = file browser)
//! - Whether the NTS panel is visible (per-workspace)
//! - Whether the right pane is maximized (per-workspace)
//! - The FocusRing for the currently active workspace
//! - Per-pane collapsed state

use std::collections::HashSet;

use crate::action::{ComponentId, Workspace};
use crate::focus::FocusRing;

/// Layout mode for the right column in station/radio workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPane {
    /// ICY ticker (upper) + Songs ticker (lower)
    Tickers,
    /// NTS Channel 1 panel (full right column)
    Nts1,
    /// NTS Channel 2 panel (full right column)
    Nts2,
    /// Oscilloscope waveform display
    Scope,
}

pub struct WorkspaceManager {
    pub workspace: Workspace,

    // ── Radio workspace state ─────────────────────────────────────────────────
    pub radio_right_pane: RightPane,
    pub radio_right_maximized: bool, // true = right pane takes more vertical space

    // ── Files workspace state ─────────────────────────────────────────────────
    pub files_right_maximized: bool,
    /// Which right pane has focus in files mode (FileMeta=0, IcyTicker=1, SongsTicker=2)
    pub files_right_focus: u8,

    // ── Shared UI ─────────────────────────────────────────────────────────────
    pub show_log_panel: bool,
    pub show_help: bool,
    pub show_keys_bar: bool, // footer keybindings bar

    // ── Collapsed panes ───────────────────────────────────────────────────────
    /// Set of components that are currently collapsed to a header-only line.
    pub collapsed: HashSet<ComponentId>,

    // ── Focus ring ────────────────────────────────────────────────────────────
    pub focus: FocusRing,
}

impl WorkspaceManager {
    pub fn new() -> Self {
        let mut wm = Self {
            workspace: Workspace::Radio,
            radio_right_pane: RightPane::Tickers,
            radio_right_maximized: false,
            files_right_maximized: false,
            files_right_focus: 0,
            show_log_panel: false,
            show_help: false,
            show_keys_bar: true,
            collapsed: HashSet::new(),
            focus: FocusRing::new(Vec::new()),
        };
        wm.rebuild_focus_ring();
        wm
    }

    /// Rebuild the FocusRing for the current workspace.
    ///
    /// `nts_hover` — the current `AppState::nts_hover_channel` value; when
    /// `Some(_)` the NTS overlay is visible and should be included in the
    /// Radio/Tickers focus ring so Tab can reach it.
    pub fn rebuild_focus_ring_with(&mut self, nts_hover: Option<usize>) {
        let items = match self.workspace {
            Workspace::Radio => match self.radio_right_pane {
                RightPane::Tickers => {
                    if nts_hover.is_some() {
                        // Overlay is visible: StationList → IcyTicker → SongsTicker → NtsPanel(overlay)
                        vec![
                            ComponentId::StationList,
                            ComponentId::IcyTicker,
                            ComponentId::SongsTicker,
                            ComponentId::NtsPanel,
                        ]
                    } else {
                        // NtsPanel not visible but keep it at position 3 so key '4' is consistent
                        vec![
                            ComponentId::StationList,
                            ComponentId::IcyTicker,
                            ComponentId::SongsTicker,
                            ComponentId::NtsPanel,
                        ]
                    }
                }
                RightPane::Nts1 | RightPane::Nts2 => {
                    vec![
                        ComponentId::StationList,
                        ComponentId::IcyTicker,
                        ComponentId::SongsTicker,
                        ComponentId::NtsPanel,
                    ]
                }
                RightPane::Scope => {
                    vec![ComponentId::StationList, ComponentId::ScopePanel]
                }
            },
            Workspace::Files => vec![
                ComponentId::FileList,
                ComponentId::FileMeta,
                ComponentId::IcyTicker,
                ComponentId::SongsTicker,
            ],
        };
        self.focus.set_items(items);
    }

    /// Rebuild focus ring without hover context (uses no-overlay default).
    /// Prefer `rebuild_focus_ring_with` when hover state is available.
    pub fn rebuild_focus_ring(&mut self) {
        self.rebuild_focus_ring_with(None);
    }

    /// Switch to the other workspace.
    pub fn toggle_workspace(&mut self) {
        self.workspace = match self.workspace {
            Workspace::Radio => Workspace::Files,
            Workspace::Files => Workspace::Radio,
        };
        self.rebuild_focus_ring();
    }

    /// Set workspace explicitly.
    pub fn set_workspace(&mut self, ws: Workspace) {
        if self.workspace != ws {
            self.workspace = ws;
            self.rebuild_focus_ring();
        }
    }

    /// Toggle scope oscilloscope panel.
    pub fn toggle_scope(&mut self) {
        if self.workspace == Workspace::Radio {
            if self.radio_right_pane == RightPane::Scope {
                self.radio_right_pane = RightPane::Tickers;
            } else {
                self.radio_right_pane = RightPane::Scope;
            }
            self.rebuild_focus_ring();
        }
    }

    /// Toggle NTS channel 1 panel.
    pub fn toggle_nts1(&mut self) {
        if self.workspace == Workspace::Radio {
            if self.radio_right_pane == RightPane::Nts1 {
                self.radio_right_pane = RightPane::Tickers;
            } else {
                self.radio_right_pane = RightPane::Nts1;
            }
            self.rebuild_focus_ring();
        }
    }

    /// Toggle NTS channel 2 panel.
    pub fn toggle_nts2(&mut self) {
        if self.workspace == Workspace::Radio {
            if self.radio_right_pane == RightPane::Nts2 {
                self.radio_right_pane = RightPane::Tickers;
            } else {
                self.radio_right_pane = RightPane::Nts2;
            }
            self.rebuild_focus_ring();
        }
    }

    /// Toggle whether the right pane is maximized in the current workspace.
    pub fn toggle_right_maximized(&mut self) {
        match self.workspace {
            Workspace::Radio => self.radio_right_maximized = !self.radio_right_maximized,
            Workspace::Files => self.files_right_maximized = !self.files_right_maximized,
        }
    }

    /// Convenience: current focused component.
    pub fn focused(&self) -> Option<ComponentId> {
        self.focus.current()
    }

    /// Focus next in ring.
    pub fn focus_next(&mut self) -> Option<ComponentId> {
        self.focus.next()
    }

    /// Focus prev in ring.
    pub fn focus_prev(&mut self) -> Option<ComponentId> {
        self.focus.prev()
    }

    /// Focus a specific component.
    pub fn focus_set(&mut self, id: ComponentId) {
        self.focus.set(id);
    }

    /// Focus the Nth pane (0-indexed) in the current ring. Returns the focused id.
    pub fn focus_nth(&mut self, n: usize) -> Option<ComponentId> {
        self.focus.set_by_position(n)
    }

    // ── Collapse helpers ──────────────────────────────────────────────────────

    /// Toggle the collapsed state of the given component.
    pub fn toggle_collapse(&mut self, id: ComponentId) {
        if self.collapsed.contains(&id) {
            self.collapsed.remove(&id);
        } else {
            self.collapsed.insert(id);
        }
    }

    /// Whether the given component is currently collapsed.
    pub fn is_collapsed(&self, id: ComponentId) -> bool {
        self.collapsed.contains(&id)
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}
