# r4dio v1 Refactor Plan

## Current State Assessment

### What Exists Today
- **Single crate, two binaries**: `radio-daemon` and `radio-tui` share a `shared/` lib module
- **Daemon** (1,653 LOC across 5 files): TCP + HTTP server, mpv controller, ICY metadata poller
- **TUI** (5,694 LOC across 3 files): monolithic `App` struct (~90 fields), one giant `main.rs` (3,593 lines) and `ui.rs` (2,101 lines)
- **ratatui 0.25 + crossterm 0.27** (5 major versions behind current 0.30)
- **No component model**: every panel is an ad-hoc draw function, focus is a manual match cascade, scrolling math is copy-pasted per pane
- **No reusable abstractions**: `ScrollableList`, `FocusRing`, `FilterableTable` do not exist -- each pane reinvents them
- **Daemon architecture** is reasonable but not event-driven state machine: uses `Arc<Mutex>` for mpv, `Arc<RwLock>` for state, ad-hoc polling loops
- **IPC** is length-prefixed JSON over TCP -- functional but no versioning, no delta/snapshot model, no typed errors

### Key Problems (from v1-iteration.md)
1. TUI is a monolithic blob -- crash-prone, hard to extend, duplicated logic everywhere
2. No component/widget reuse -- adding a new pane means 200+ lines of boilerplate
3. Manual focus/scroll/filter per pane -- 4 copies of nearly identical logic
4. Daemon lacks a single state-owner event loop -- multiple tasks mutate shared state through locks
5. mpv integration is request/response with polling, not property-observation-driven
6. No pending-intent UX -- pressing pause flips immediately instead of showing "pending" state
7. No protocol versioning or snapshot+delta model
8. No resilience state machines (mpv health, network health per station)

---

## Crate Ecosystem Evaluation

After cloning and inspecting all mentioned crates (see `~/gh/rust-stuff/`):

| Crate | Version | ratatui compat | Use? | Rationale |
|---|---|---|---|---|
| **tui-input** | latest | 0.30 | **YES** | Lightweight single-line input, perfect for filter/search/command. Full unicode, visual scroll, backend-agnostic |
| **tui-widgets** (tui-popup, tui-scrollview) | latest | 0.30 | **YES** | `tui-popup` for modal dialogs, `tui-scrollview` for scrollable info panels |
| **tui-tree-widget** | 0.24 | 0.30 | **MAYBE** | Only if we add hierarchical station grouping (country > genre > station). Skip for v1, revisit later |
| **ratkit** | 0.x | 0.29 | **NO** | Too opinionated (own event loop/coordinator), pinned to ratatui 0.29, API still evolving. Cherry-pick ideas instead |
| **tui-textarea** | 0.6 | 0.29 | **NO** | Overkill for our needs (multi-line editor), version behind. tui-input covers our cases |
| **ratatui-templates** (component pattern) | - | 0.30 | **REFERENCE** | Use the Component trait pattern and Action enum architecture as structural inspiration |

### Decided Stack
- **ratatui 0.30** + **crossterm 0.28** (upgrade from 0.25/0.27)
- **tui-input** for all text inputs (filter bar, command palette, URL input)
- **tui-popup** (from tui-widgets) for modal dialogs and help overlay
- **tui-scrollview** (from tui-widgets) for scrollable panels
- **Custom internal framework** (inspired by ratatui-templates component pattern): `Component` trait, `Action` enum, `FocusRing`, `ScrollableList<T>`, `PaneChrome`

---

## Architecture Overview

### Project Structure (workspace migration)

```
r4dio/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── radio-proto/        # Shared protocol types, config, station model
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── command.rs    # Command enum (versioned)
│   │       ├── broadcast.rs  # Broadcast/event types
│   │       ├── state.rs      # DaemonState, Station, PlaybackStatus
│   │       ├── config.rs     # Config struct + loader
│   │       ├── platform.rs   # Platform paths
│   │       └── codec.rs      # Length-prefixed JSON encode/decode
│   ├── radio-daemon/       # Daemon binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs       # Entry point, small
│   │       ├── daemon.rs     # State-owner event loop
│   │       ├── mpv/
│   │       │   ├── mod.rs    # MpvDriver (spawn, ipc, health FSM)
│   │       │   ├── ipc.rs    # JSON IPC reader/writer tasks
│   │       │   └── types.rs  # mpv-specific types
│   │       ├── metadata/
│   │       │   ├── mod.rs    # Metadata pipeline coordinator
│   │       │   ├── icy.rs    # ICY stream metadata
│   │       │   └── poller.rs # HTTP now-playing endpoints
│   │       ├── server/
│   │       │   ├── socket.rs # Unix/TCP IPC server
│   │       │   └── http.rs   # Axum REST API
│   │       └── persistence.rs # Async file writes
│   └── radio-tui/          # TUI binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs       # Entry + terminal setup
│           ├── app.rs        # App state + event loop
│           ├── action.rs     # Action enum (all user intents)
│           ├── event.rs      # Event handler (crossterm + daemon)
│           ├── keymap.rs     # Mode-aware key -> Action mapping
│           ├── focus.rs      # FocusRing manager
│           ├── theme.rs      # Color palette + style constants
│           ├── widgets/      # Reusable widget primitives
│           │   ├── mod.rs
│           │   ├── scrollable_list.rs   # Generic scrollable + filterable list
│           │   ├── pane_chrome.rs       # Bordered pane wrapper with focus styling
│           │   ├── status_bar.rs        # Bottom status bar
│           │   ├── filter_input.rs      # Filter bar (wraps tui-input)
│           │   ├── toast.rs             # Toast notification system
│           │   └── progress_bar.rs      # Playback progress
│           ├── components/   # Panel components (each implements Component)
│           │   ├── mod.rs
│           │   ├── header.rs            # Now-playing header
│           │   ├── station_list.rs      # Station list panel
│           │   ├── file_list.rs         # File browser panel
│           │   ├── icy_ticker.rs        # ICY metadata ticker
│           │   ├── songs_ticker.rs      # Songs history ticker
│           │   ├── nts_panel.rs         # NTS live info
│           │   ├── file_meta.rs         # File metadata/chapters
│           │   ├── log_panel.rs         # Log viewer
│           │   └── help_overlay.rs      # Help popup
│           ├── workspace.rs  # Workspace/tab management (Radio, Files)
│           └── connection.rs # Daemon connection + reconnect logic
├── stations.toml
└── ...
```

---

## Detailed Implementation Plan

### Phase 0: Preparation & Upgrade (foundation)

#### 0.1 Create workspace structure
- Convert single-crate to Cargo workspace with 3 crates: `radio-proto`, `radio-daemon`, `radio-tui`
- Move `src/shared/` into `crates/radio-proto/src/`
- Move `src/daemon/` into `crates/radio-daemon/src/`
- Move `src/tui/` into `crates/radio-tui/src/`
- Verify everything compiles and runs unchanged

#### 0.2 Upgrade dependencies
- ratatui `0.25` -> `0.30` (handle breaking API changes: `Frame<'_>` no longer generic over backend, `Table`/`List` API changes, new color/style API)
- crossterm `0.27` -> `0.28` (event API changes)
- reqwest `0.11` -> `0.12` (if needed for compatibility)
- Verify compilation, fix all deprecation warnings

---

### Phase 1: Daemon Hardening (event-driven state machine)

#### 1.1 Single state-owner event loop
**Current**: Multiple tasks hold `Arc<Mutex<MpvController>>` and `Arc<RwLock<DaemonState>>`, mutating through locks.
**Target**: Single `DaemonCore` task that owns all mutable state. All other tasks send typed `DaemonEvent` messages to it via an mpsc channel.

```rust
enum DaemonEvent {
    // From clients
    ClientCommand { client_id: u64, cmd: Command },
    ClientConnected { client_id: u64, tx: oneshot::Sender<DaemonState> },
    ClientDisconnected { client_id: u64 },
    // From mpv driver
    MpvPropertyChange { property: String, value: serde_json::Value },
    MpvEvent { event: MpvEvent },
    MpvHealthChanged { status: MpvHealth },
    // From metadata pipeline
    MetadataUpdate { station_idx: usize, source: MetadataSource, info: TrackInfo },
    // Internal
    Tick,
    PersistenceAck,
    ShutdownRequested,
}
```

The core loop:
```rust
loop {
    match event_rx.recv().await {
        DaemonEvent::ClientCommand { .. } => { /* apply intent, emit side effects */ }
        DaemonEvent::MpvPropertyChange { .. } => { /* update observed state */ }
        // ...
    }
    // After each event: broadcast state delta to clients
}
```

#### 1.2 mpv as a "device driver"
**Current**: `MpvController` does sync-style request/response with `ipc()`, reads lines looking for matching request_id, stores ICY title as side effect during read.
**Target**: Two dedicated tasks (reader + writer) with proper message routing.

- **Reader task**: Reads JSON lines from mpv socket, parses into `MpvMessage` (response with request_id, or event/property-change), forwards to `DaemonEvent` channel
- **Writer task**: Receives `MpvCommand` from a channel, serializes with request_id, writes to socket
- **Health FSM**: `MpvHealth { Starting, Running, Degraded(reason), Dead, Restarting }` with transitions emitted as events
- **Property observation**: On stream load, immediately observe: `pause`, `volume`, `mute`, `time-pos`, `duration`, `core-idle`, `icy-title`, `metadata`, `media-title`, `audio-params`, `demuxer-cache-duration`
- **Intent vs observed**: A "pause" command sets `intended_paused = true` and sends mpv command. State only changes to `paused = true` when mpv confirms via property-change event. The delta between intent and observed is exposed to clients.

#### 1.3 Metadata pipeline
**Current**: ICY poller runs independently, stores results in a HashMap on DaemonState. mpv ICY title extracted as side-effect during IPC reads.
**Target**: Unified metadata pipeline with source prioritization.

```rust
struct TrackInfo {
    title: Option<String>,
    artist: Option<String>,
    source: MetadataSource,  // MpvIcy, MpvMetadata, IcyStream, HttpNowPlaying
    timestamp: Instant,
    confidence: u8,          // 0-100
}
```

Merge policy: prefer mpv ICY (freshest, tied to actual stream) > ICY stream fetcher > HTTP now-playing endpoint. Debounce rapid changes (500ms window). Persist last effective track with provenance.

#### 1.4 Protocol versioning & snapshot+delta
**Current**: Full state sent on every update. No versioning.
**Target**: 
- Add `protocol_version: u32` to initial handshake
- Each state broadcast carries `rev: u64` (monotonically increasing)
- On connect: client receives full `Snapshot { rev, state }` 
- Subsequent updates: `Delta { rev, changes: Vec<StateChange> }` (or simplified: just send full state but with rev, and client can detect gaps to request resync)
- Typed error responses: `CommandResponse::Ok(payload) | CommandResponse::Error { code, message }`

*Pragmatic simplification*: For v1, keep sending full state snapshots but add `rev` numbering. True delta compression can come in v2. The important parts are: versioning, typed errors, and the rev number for gap detection.

#### 1.5 Health state machines
Add explicit health tracking:
- `MpvHealth`: Starting -> Running | Dead. Running -> Degraded | Dead. Dead -> Restarting -> Starting.
- `StationHealth` per station: Idle -> Connecting -> Connected | Failed(retries, backoff). 
- `ClientHealth`: Connected -> Idle(timeout) -> Disconnected. Track slow consumers.
- Expose health in `DaemonState` so TUI can render connection quality indicators.

#### 1.6 Persistence improvements
**Current**: Synchronous JSON/TOML file writes scattered through state manager.
**Target**: 
- Dedicated persistence task that receives `PersistRequest` messages
- Atomic writes (write to temp file, rename)
- Debounced: coalesce rapid changes (e.g., volume slider) into single write after 500ms quiet period
- Non-blocking: state owner emits persist requests as side effects, never waits for disk

---

### Phase 2: TUI Component Architecture (eliminate boilerplate)

#### 2.1 Component trait & Action system
Inspired by ratatui-templates component pattern:

```rust
pub trait Component {
    /// Handle a user action, return optional actions to propagate
    fn handle_action(&mut self, action: &Action) -> Vec<Action>;
    /// Handle raw key event (only when focused), return optional actions
    fn handle_key(&mut self, key: KeyEvent) -> Vec<Action>;
    /// Handle mouse event (within bounds), return optional actions
    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<Action>;
    /// Update from daemon state snapshot
    fn update_state(&mut self, state: &DaemonState);
    /// Render into the given area
    fn draw(&mut self, frame: &mut Frame, area: Rect, focused: bool);
    /// Whether this component is focusable
    fn focusable(&self) -> bool { true }
    /// Unique component ID
    fn id(&self) -> ComponentId;
}
```

```rust
pub enum Action {
    // Playback
    Play(usize), PlayFile(String), Stop, TogglePause, Next, Prev, Random,
    Volume(f32), SeekRelative(f64), SeekTo(f64),
    // Navigation
    FocusNext, FocusPrev, FocusPane(ComponentId),
    SelectUp(usize), SelectDown(usize), SelectFirst, SelectLast,
    JumpToCurrent,
    // Filter/search
    OpenFilter, CloseFilter, FilterChanged(String),
    // Workspace
    SwitchWorkspace(Workspace), // Radio, Files
    // UI
    ToggleLogs, ToggleHelp, ToggleKeybindings,
    CycleSort, CycleSortReverse,
    ToggleStar, CopyToClipboard,
    ToggleFullWidth,
    // NTS
    ToggleNts(usize),
    // Daemon
    SendCommand(Command),
    // System
    Quit, Resize(u16, u16), Tick, Render,
}
```

#### 2.2 FocusRing manager
**Current**: Manual `focus_pane` field + match statements scattered everywhere.
**Target**: Reusable focus manager.

```rust
pub struct FocusRing {
    items: Vec<ComponentId>,
    current: usize,
}

impl FocusRing {
    fn next(&mut self) -> ComponentId;
    fn prev(&mut self) -> ComponentId;
    fn set(&mut self, id: ComponentId);
    fn current(&self) -> ComponentId;
    fn set_items(&mut self, items: Vec<ComponentId>);  // reconfigure for workspace changes
}
```

The workspace (Radio vs Files) reconfigures the focus ring with different component sets:
- Radio: `[StationList, NtsPanel, IcyTicker, SongsTicker]`
- Files: `[FileList, FileMeta, SongsTicker, IcyTicker]`

#### 2.3 Reusable ScrollableList<T>
**Current**: Each pane has its own `selected_idx`, `view_start` calculation, scroll-into-view logic, filter state.
**Target**: Single generic widget used by all list-bearing panels.

```rust
pub struct ScrollableList<T> {
    items: Vec<T>,
    filtered_indices: Vec<usize>,
    selected: usize,
    scroll_offset: usize,
    filter: String,
    filter_fn: Box<dyn Fn(&T, &str) -> bool>,
    sort_fn: Option<Box<dyn Fn(&T, &T) -> Ordering>>,
}

impl<T> ScrollableList<T> {
    fn select_up(&mut self, n: usize);
    fn select_down(&mut self, n: usize);
    fn select_first(&mut self);
    fn select_last(&mut self);
    fn set_filter(&mut self, query: &str);
    fn set_sort(&mut self, sort: impl Fn(&T, &T) -> Ordering);
    fn selected_item(&self) -> Option<&T>;
    fn visible_items(&self, height: usize) -> &[(usize, &T)]; // (original_idx, item)
    fn handle_scroll(&mut self, direction: ScrollDirection, amount: usize);
    fn handle_click(&mut self, row: usize, area_height: usize);
    fn ensure_visible(&mut self, height: usize); // scroll selected into view
}
```

Used by: `StationList`, `FileList`, `IcyTicker`, `SongsTicker`, `LogPanel`.

#### 2.4 PaneChrome widget
**Current**: Each draw function manually creates a `Block` with borders/title/styling.
**Target**: Reusable pane wrapper.

```rust
pub struct PaneChrome<'a> {
    title: &'a str,
    number_key: Option<char>,    // e.g., '1' for "[1] stations"
    focused: bool,
    badge: Option<(&'a str, Color)>,  // e.g., ("LIVE", green) or ("ERR", red)
    collapsed: bool,
}

impl PaneChrome {
    fn render(&self, frame: &mut Frame, area: Rect, inner_fn: impl FnOnce(&mut Frame, Rect));
}
```

Standardizes: focus highlight color, border style, title format `[N] title`, collapse behavior (render only the 1-line header separator), badge positioning.

#### 2.5 FilterInput widget (wraps tui-input)
**Current**: Manual char-by-char input handling in main.rs key handler, per-pane filter strings.
**Target**: Reusable filter input component wrapping `tui-input`.

```rust
pub struct FilterInput {
    input: tui_input::Input,
    active: bool,
    placeholder: String,
}

impl FilterInput {
    fn activate(&mut self);
    fn deactivate(&mut self);
    fn handle_key(&mut self, key: KeyEvent) -> FilterAction; // Changed(text), Confirmed, Cancelled
    fn draw(&self, frame: &mut Frame, area: Rect);
    fn text(&self) -> &str;
    fn is_active(&self) -> bool;
}
```

#### 2.6 Toast notification system
**Current**: Errors shown as popup, logs in a bar -- no transient notifications.
**Target**: Toast stack for brief status messages.

```rust
pub struct ToastManager {
    toasts: VecDeque<Toast>,
    max_visible: usize,
}

struct Toast {
    message: String,
    severity: Severity, // Info, Success, Warning, Error
    expires: Instant,
}
```

Renders in top-right corner, auto-expires after 3s. Used for: "Station added to favorites", "Copied to clipboard", "Connection lost", "mpv restarting".

#### 2.7 Workspace/Tab system
**Current**: `left_mode: LeftPaneMode` (Stations/Files) with ad-hoc switching.
**Target**: Proper workspace model.

```rust
pub enum Workspace {
    Radio,  // Station list + ICY + Songs + NTS
    Files,  // File browser + File meta + Songs + ICY
}

pub struct WorkspaceManager {
    active: Workspace,
    radio_layout: PaneLayout,
    files_layout: PaneLayout,
}
```

Each workspace defines:
- Which components are visible
- Focus ring order
- Layout constraints (split ratios, which panes can collapse)
- Per-workspace state persistence (last selected item, scroll position, filter)

Switching workspace preserves state of the inactive workspace.

#### 2.8 Pending-intent UX
**Current**: UI immediately reflects user actions (press pause -> UI shows paused, even before mpv confirms).
**Target**: Three-state rendering for actions with latency.

```rust
pub enum IntentState<T> {
    Confirmed(T),           // mpv confirmed this value
    Pending { intended: T, confirmed: T, since: Instant }, // waiting for confirmation
    TimedOut { intended: T, confirmed: T },  // mpv didn't confirm after threshold
}
```

Visual treatment:
- Pending: dim/pulsing indicator (e.g., pause icon blinks)
- Timed out: warning color + "?" indicator
- Confirmed: solid, normal rendering

Apply to: play/pause, volume changes, seek operations, station switches.

---

### Phase 3: UI Polish & New Features

#### 3.1 Visual improvements
- **Collapsed borders**: Use ratatui's border-collapsing technique so adjacent panes share borders (lazygit style) instead of double borders
- **Focus styling**: Focused pane gets brighter border (`C_ACCENT`), unfocused gets `C_PANEL_BORDER` (muted). Title of focused pane is bold.
- **Status badges**: Each pane header can show contextual badges (mpv health, metadata source, connection status)
- **Better progress bar**: Unicode block characters for smooth progress (▏▎▍▌▋▊▉█) instead of ▓·

#### 3.2 Mouse improvements
**Current**: Click-to-select works but has issues; hover changes focus (can be annoying).
**Target**:
- Click to select items in lists (fix any click offset bugs)
- Click on pane header/body to focus that pane (not hover)
- Scroll wheel within pane boundaries scrolls that pane
- Click on workspace tabs to switch
- Drag to resize split panels (stretch goal)

#### 3.3 Collapsible panels
**Current**: `_` toggles full-width mode (hides right panel entirely).
**Target**: Per-pane collapse.
- Each pane can be collapsed to a single header line
- In files workspace, the accordion behavior (only one right pane expanded) is kept but improved
- Collapse animation: instant (no animation, just layout recalculation)
- Collapsed pane shows title + optional one-line summary (e.g., collapsed ICY shows last title)

#### 3.4 Command palette (stretch goal for v1)
- `/` or `:` opens a command palette overlay
- Fuzzy-match against all available actions
- Context-aware: shows different actions based on current focus/workspace
- Uses `tui-input` for the input field, `tui-popup` for the overlay
- Replaces the need to memorize all keybindings

#### 3.5 Log panel improvements
**Current**: Reads daemon log file from disk every 2 seconds.
**Target**: 
- Daemon pushes log entries over IPC (using a tracing layer that forwards to a ring buffer)
- TUI receives logs as `Broadcast::Log` messages -- no file reading needed
- Log panel: scrollable, filterable (by log level), copyable
- Log bar (collapsed): shows last entry with severity color

#### 3.6 Keybinding improvements
- Mode indicator in status bar: `NORMAL`, `FILTER`, `COMMAND`
- Keybinding footer respects current mode/context
- Help overlay is searchable
- Key bindings are configurable via TOML (stretch goal)

---

### Phase 4: Dependency & Build Improvements

#### 4.1 Dependency updates
```toml
# Workspace Cargo.toml [workspace.dependencies]
ratatui = "0.30"
crossterm = "0.28"
tui-input = "0.11"        # single-line input
tui-widgets = { version = "0.3", features = ["popup", "scrollview"] }
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
toml = "0.8"
dirs = "5.0"
axum = "0.7"
reqwest = { version = "0.12", features = ["json"] }
chrono = { version = "0.4", features = ["serde"] }
rand = "0.8"
unicode-width = "0.2"     # for proper text width calculations
```

#### 4.2 Workspace layout
```toml
# Root Cargo.toml
[workspace]
members = ["crates/radio-proto", "crates/radio-daemon", "crates/radio-tui"]
resolver = "2"

[workspace.dependencies]
# ... shared deps above
radio-proto = { path = "crates/radio-proto" }
```

---

## Implementation Order & Estimates

| Phase | Task | Priority | Effort | Dependencies |
|---|---|---|---|---|
| **0.1** | Workspace migration | P0 | Medium | None |
| **0.2** | Dependency upgrade (ratatui 0.30) | P0 | Medium | 0.1 |
| **2.1** | Component trait + Action enum | P0 | Medium | 0.2 |
| **2.2** | FocusRing manager | P0 | Small | 2.1 |
| **2.3** | ScrollableList<T> generic widget | P0 | Medium | 2.1 |
| **2.4** | PaneChrome widget | P0 | Small | 2.1 |
| **2.5** | FilterInput (tui-input integration) | P0 | Small | 2.1 |
| **2.7** | Workspace/tab system | P0 | Medium | 2.2, 2.4 |
| **P1 TUI** | Migrate station_list to Component | P0 | Medium | 2.1-2.5 |
| **P1 TUI** | Migrate file_list to Component | P0 | Medium | 2.3 |
| **P1 TUI** | Migrate icy_ticker to Component | P0 | Medium | 2.3 |
| **P1 TUI** | Migrate songs_ticker to Component | P0 | Small | 2.3 |
| **P1 TUI** | Migrate nts_panel to Component | P1 | Small | 2.4 |
| **P1 TUI** | Migrate file_meta to Component | P1 | Small | 2.4 |
| **P1 TUI** | Migrate header to Component | P0 | Small | 2.1 |
| **P1 TUI** | Migrate log_panel to Component | P1 | Small | 2.3 |
| **P1 TUI** | Migrate help overlay to Component | P1 | Small | - |
| **P1 TUI** | App.rs main loop rewrite | P0 | Large | All above |
| **1.1** | Daemon event loop (DaemonCore) | P1 | Large | 0.1 |
| **1.2** | mpv driver rewrite (reader/writer tasks) | P1 | Large | 1.1 |
| **1.3** | Metadata pipeline | P2 | Medium | 1.1 |
| **1.4** | Protocol versioning + rev numbers | P1 | Small | 0.1 |
| **1.5** | Health state machines | P2 | Medium | 1.1, 1.2 |
| **1.6** | Async persistence | P2 | Small | 1.1 |
| **2.6** | Toast notifications | P2 | Small | 2.1 |
| **2.8** | Pending-intent UX | P2 | Medium | 1.1, 1.2 |
| **3.1** | Visual polish (collapsed borders, badges) | P1 | Medium | 2.4 |
| **3.2** | Mouse improvements | P1 | Medium | 2.1 |
| **3.3** | Collapsible panels | P1 | Medium | 2.4, 2.7 |
| **3.4** | Command palette | P3 | Medium | 2.5 |
| **3.5** | Log panel via IPC | P2 | Medium | 1.1 |
| **3.6** | Keybinding improvements | P2 | Small | 2.1 |

---

## Execution Strategy

### Step-by-step build order (preserving a working app at each step):

1. **Workspace migration** (0.1): Move files, update Cargo.toml, verify `cargo build` works
2. **Dependency upgrade** (0.2): Bump ratatui/crossterm, fix all compile errors from API changes
3. **Introduce widget primitives** (2.3, 2.4, 2.5): Write `ScrollableList`, `PaneChrome`, `FilterInput` as new files -- no existing code changes yet
4. **Component trait + Action enum** (2.1, 2.2): Define the trait and FocusRing in new files
5. **Migrate one component at a time**: Start with `header` (simplest), then `station_list` (most complex, proves the pattern), then remaining panels. Each migration:
   - Create `components/X.rs` implementing `Component`
   - Move relevant fields from monolithic `App` into the component
   - Move relevant drawing code from `ui.rs` into `component.draw()`
   - Move relevant key handling from `main.rs` into `component.handle_key()`
   - Update `App` to use the component
   - Test that it still works
6. **Workspace/tab system** (2.7): Replace `LeftPaneMode` with proper workspace manager
7. **App.rs rewrite**: Once all components are migrated, rewrite the main loop to use the component dispatch pattern
8. **Daemon hardening** (1.1, 1.2): Refactor daemon internals -- this is independent of TUI changes since IPC is preserved
9. **Protocol improvements** (1.4): Add versioning to protocol -- backward compatible
10. **Polish pass** (3.1-3.6): Visual improvements, mouse fixes, collapsible panels

### Key principle: **Always compilable, always runnable**
Each step produces a working application. No big-bang rewrites. The monolithic code shrinks incrementally as components are extracted.

---

## Risk Assessment

| Risk | Mitigation |
|---|---|
| ratatui 0.30 has significant API breaks | Upgrade early (Phase 0), fix all issues before other changes |
| Component migration breaks UX | Migrate one panel at a time, visual regression check after each |
| tui-input incompatibility | It targets 0.30, should be clean. Fallback: keep manual input handling |
| Daemon refactor breaks IPC | Keep protocol wire format identical in Phase 1; old TUI can talk to new daemon |
| Scope creep | P0 items are the must-haves; P2/P3 are stretch goals. Ship when P0+P1 are done |

---

## Success Criteria for v1 Tag

**Must have (P0)**:
- [ ] Workspace structure with 3 crates
- [ ] ratatui 0.30 + crossterm 0.28
- [ ] Component trait with Action-based event routing
- [ ] All panels migrated to Component pattern
- [ ] FocusRing replacing manual focus management
- [ ] ScrollableList<T> used by all list panels
- [ ] PaneChrome used by all panels
- [ ] FilterInput using tui-input
- [ ] Workspace tabs (Radio/Files) with proper state preservation

**Should have (P1)**:
- [ ] Daemon event-driven state machine
- [ ] mpv reader/writer task separation
- [ ] Protocol versioning with rev numbers
- [ ] Collapsed borders (lazygit-style)
- [ ] Mouse click-to-focus (not hover)
- [ ] Collapsible panels
- [ ] Visual polish (focus styling, badges, progress bar)

**Nice to have (P2/P3)**:
- [ ] Metadata pipeline with source prioritization
- [ ] Health state machines
- [ ] Pending-intent UX
- [ ] Toast notifications
- [ ] Command palette
- [ ] Configurable keybindings
- [ ] Log panel via IPC instead of file reading
