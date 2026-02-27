//! Action enum — all user-initiated intents and internal events.

use radio_proto::protocol::Command;

/// Unique identifier for a focusable component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentId {
    StationList,
    FileList,
    IcyTicker,
    SongsTicker,
    NtsPanel,
    FileMeta,
    LogPanel,
    HelpOverlay,
    ScopePanel,
}

/// Context for star operations — identifies which item type is being starred.
#[derive(Debug, Clone)]
pub enum StarContext {
    Station(String), // station name
    File(String),    // file path
}

/// All actions that can flow through the system.
/// Components produce Actions; the App dispatches them.
#[derive(Debug, Clone)]
pub enum Action {
    // ── Playback ─────────────────────────────────────────────────────────────
    Play(usize),                 // play station by index
    PlayFile(String),            // play local file by path
    PlayFileAt(String, f64),     // play file starting at position
    PlayFilePaused(String, f64), // play file paused at position
    Stop,
    TogglePause,
    Next,
    Prev,
    Random,
    RandomBack,
    Volume(f32),
    SeekRelative(f64),
    SeekTo(f64),
    Mute, // toggle mute (save/restore volume)

    // ── Navigation ───────────────────────────────────────────────────────────
    FocusNext,
    FocusPrev,
    FocusPane(ComponentId),
    SelectUp(usize),
    SelectDown(usize),
    SelectFirst,
    SelectLast,
    ScrollUp(usize),
    ScrollDown(usize),
    JumpToCurrent,

    // ── Filter/search ────────────────────────────────────────────────────────
    OpenFilter,
    CloseFilter,
    FilterChanged(String),
    ClearFilter,

    // ── Workspace ────────────────────────────────────────────────────────────
    SwitchWorkspace(Workspace),
    ToggleFullWidth,
    ToggleRightMaximized,

    // ── Sorting ──────────────────────────────────────────────────────────────
    CycleSort,
    CycleSortReverse,

    // ── Stars/ratings ────────────────────────────────────────────────────────
    ToggleStar,
    SetStar(u8, StarContext),

    // ── NTS ──────────────────────────────────────────────────────────────────
    ToggleNts(usize), // channel 0 or 1
    /// Hovering over an NTS station row: Some(0|1) = which channel, None = left NTS row.
    HoverNts(Option<usize>),

    // ── Scope ─────────────────────────────────────────────────────────────────
    ToggleScope,

    // ── Song recognition ─────────────────────────────────────────────────────
    /// Trigger song recognition (vibra + icy + nts pipeline).
    RecognizeSong,

    // ── UI toggles ───────────────────────────────────────────────────────────
    ToggleLogs,
    ToggleHelp,
    ToggleKeys,
    ToggleCollapse,          // collapse/expand the currently focused pane
    CopyToClipboard(String), // text to copy
    Download,

    // ── System ───────────────────────────────────────────────────────────────
    SendCommand(Command),
    Quit,
    Tick,
    Render,
    Resize(u16, u16),
    Noop,
}

/// Which workspace (tab) is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Radio, // station list on the left
    Files, // file browser on the left
}
