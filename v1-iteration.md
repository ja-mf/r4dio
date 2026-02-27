# UI RECOMMENDATIONS

Yes. With ratatui alone you typically end up writing a lot of “glue” (focus management, keymaps, scroll state, pane layout, popups, async event integration). The usual way to avoid tons of bespoke code is to combine ratatui with a small set of higher-level crates that solve the repetitive parts, and to adopt an architecture that treats the UI as a state machine with composable components.

The most leverage comes from these layers:
	1.	Application architecture helpers (biggest reduction in custom code)

	•	TUI event loop integration with async: run a single UI loop that consumes (a) terminal input events and (b) daemon/state updates from channels. You want a crate or pattern that gives you a clean “tick + event + render” structure and hides raw crossterm plumbing.
	•	Component model: define each panel/tab as a component with (handle_event, update, render) and let a root router manage focus + routing. This is not a library requirement, but if you adopt an existing component framework you write dramatically less code.

Practical options that are widely used:
	•	crossterm as backend + a small “app framework” crate built around ratatui. There isn’t a single universally dominant “ratatui framework” the way Bubble Tea is for Go, but there are a few that meaningfully reduce boilerplate (routing, focus, key bindings, popups). The downside is churn: some are hobby projects. If you choose one, pick a maintained one with recent commits and examples.

	2.	Widgets that save real work (lists/tables/trees/forms)
ratatui’s built-in widgets cover basics, but ergonomic UX usually needs:

	•	Rich list/table widgets: sorting, filtering, column sizing, selection, incremental updates.
	•	Tree view: collapsible hierarchy (stations → categories, or logs).
	•	Input fields with editing, history, completion.
	•	Popup dialogs, toasts/notifications, command palette.

There are crates in the ecosystem that provide these, but quality varies. In practice, the “best” approach I’ve seen stick is:
	•	Use ratatui for layout + rendering primitives.
	•	Use tui-textarea (or similar) for text input/editing (this is a major time saver).
	•	Implement selection/scroll state yourself but standardize it into reusable “model” types (ScrollableList, FilterableTable, FocusRing). This sounds like custom code, but it’s the kind you write once and reuse everywhere; it’s much less than trying to custom-build everything ad hoc per panel.

	3.	UX patterns that reduce complexity more than any library choice
These are the patterns that keep the codebase small while still producing a “proper app” UX:

	•	Command palette (single entry point for actions): instead of bespoke keymaps per panel, unify actions behind a palette and show context-specific actions. Implementation cost is modest and it reduces complexity.
	•	Global input modes: Normal / Insert / Search / Command (vim-like). This simplifies key handling and focus logic.
	•	Central “focus ring” manager: define a linear order of focusable panes and let Tab/Shift-Tab move focus; each component exposes focusable sub-elements (list, input, buttons).
	•	Render-on-change + capped redraw: don’t redraw on every tick unless needed; redraw on state change or at a fixed rate for time/progress bars. This matters for responsiveness.

What I would pick for “best” today (minimize custom code, keep maintainable)
	•	ratatui + crossterm
	•	tui-textarea for inputs (search boxes, rename station, command palette)
	•	A small internal component framework (you write it) with:
	•	a shared AppState snapshot from the daemon
	•	an Action enum (play/pause, set station, volume, toggle panel, focus next, open tab, etc.)
	•	a Keymap layer mapping keys → Action based on mode/context
	•	a Focus manager
This is usually less code than adopting a third-party “framework” crate whose abstractions you’ll fight.

If you really want “less code than that,” the honest answer is: Go + Bubble Tea is currently the most batteries-included for terminal UX (focus, components, update/view loop) with minimal scaffolding. Rust can match it, but you pay more in architecture/setup unless you accept a more DIY framework.

A realistic “minimal-boilerplate” Rust stack for your case
	•	UI: ratatui + crossterm
	•	Input: tui-textarea
	•	Async integration: tokio + channels (watch/broadcast) feeding the UI loop
	•	Terminal sizing/unicode width: unicode-width, optionally a grapheme-aware crate if you care about complex scripts
	•	Logging panel: tracing + tracing-subscriber with a custom layer that forwards log records into a ring buffer displayed in a pane

If you tell me whether you want vim-like modal UX or “app-like” keybinds, I can propose a concrete component/action/focus layout that keeps total UI code surprisingly small while still supporting panes, collapses, tabs, scrolling lists, and popups.


tui-realm (crate name tuirealm): a higher-level framework on top of ratatui with reusable “components”, message/event/update loop ideas (Elm-ish) and a more structured app model than raw ratatui.  ￼

ratatui-toolkit: a collection of reusable higher-level TUI components for ratatui (e.g., resizable splits, tree views, markdown rendering, etc.).  ￼

ratatui_router: page-based navigation/routing helpers (derive macro to generate routes, page rendering, event handling, shared context).  ￼

tui-textarea: multiline text editor widget (useful for command palettes, search boxes, editable fields, log filtering).  ￼

tui-input: single-line input model/widget support (cursor management, editing helpers; backend-agnostic but commonly used with ratatui).  ￼

tui-tree-widget (and related forks like tui-tree-widget-table): collapsible tree widget + selection state (useful for hierarchical station lists / categories).  ￼

tui-markdown: markdown renderer widget for ratatui (handy for help panes / docs / changelog views).  ￼

tui-widgets (repo joshka/tui-widgets): a combined “useful widgets” collection intended to simplify using multiple widget crates together.  ￼

ratatui-form: form builder on ratatui (useful if you want structured settings dialogs and multi-field editors).  ￼

ratatui-code-editor: code editor widget with syntax highlighting via tree-sitter (overkill for many apps, but relevant if you want rich editable configs).  ￼

Discovery/indices (good for finding more widgets/frameworks)
	•	Ratatui “Third Party Widgets Showcase” page.  ￼
	•	“Awesome Ratatui” curated list of crates/apps.  ￼

Starter templates that bake in better structure (not a library, but reduces UX scaffolding work)
	•	ratatui/templates (simple + async templates).  ￼
	•	ratatui/crates-tui: an opinionated example/template with async, custom widgets, and action/key-chord mapping.  ￼

For “lazygit-style” panes (tiled layout, borders that visually merge, focus highlight, panels that can be collapsed/expanded and resized), ratatui can do it well, but you want one of the “split/pane” component crates plus a couple of widgets for inputs/trees.

What you want for collapsible panels is two separate features: (1) resizable splits (drag or key-controlled divider), and (2) layout constraints that can be toggled to “collapsed” (0/1 rows/cols, or hidden entirely) while preserving focus and state.

Libraries that directly help with that:
	•	ratatui-toolkit: provides higher-level components including draggable split panels and a Pane container; it explicitly lists “ResizableSplit / ResizableGrid” and “Pane” components, plus toasts/dialogs that help UX.  ￼
	•	ratatui-interact: includes a SplitPane (resizable split) and a TabView, plus other viewer components; useful if you want a prebuilt split-pane interaction model.  ￼
	•	Ratatui’s own “collapse borders” recipe: not about collapsing panels, but it’s exactly the trick you want for the “lazygit tiled panes that look connected” effect (overlapping borders rather than double borders).  ￼

A good, practical stack for a lazygit-like UI in Rust
	1.	Rendering + terminal backend

	•	ratatui + crossterm (standard baseline).

	2.	Pane layout + resizing + panel chrome
Pick one:

	•	ratatui-toolkit for resizable splits/grids + a consistent Pane wrapper + toast/dialog UX primitives.  ￼
or
	•	ratatui-interact if you mainly want SplitPane + TabView and you’re fine building your own “pane” styling conventions.  ￼

	3.	Collapsing/expanding panels (how to do it cleanly)
Even if you use a split-pane crate, you still need a “layout model” that supports collapse. The reliable pattern is:

	•	Maintain a layout tree (e.g., Split {dir, ratio, a, b}, Tabs {active, children}, Leaf {panel_id}).
	•	For “collapse panel X”, mutate the layout tree: replace that leaf with Hidden, or set its constraint to 0/Min(1) and move focus elsewhere.
	•	Rendering uses the layout tree to produce Rects; hidden nodes simply don’t render or get 0-area.

The split-pane crates help with resizing interaction; the “collapse” is usually a small amount of your own code because it’s app-specific (what collapses, where focus goes, how it re-expands).
	4.	Essential UX widgets

	•	Text input / command palette / search: tui-textarea is the usual accelerator (you don’t want to implement cursor/editing yourself). (You asked earlier; still the right choice.)
	•	Hierarchical station list / groups: a tree widget crate (e.g., tui-tree-widget) is typically how you get expand/collapse lists without writing it all yourself.
	•	Tables/lists: ratatui’s built-ins are fine, but you’ll want standardized “select + scroll + filter state” structs you reuse.

	5.	Visual cohesion like lazygit

	•	Use ratatui’s border-collapsing layout approach so adjacent panes share borders cleanly.  ￼
	•	Standardize focus styling: one focused pane gets a brighter border/title; others muted. This is not a library feature; it’s a convention you implement once in your “Pane wrapper” function.

If you want the shortest path with the least custom layout work: use ratatui-toolkit (ResizableSplit/ResizableGrid + Pane + toast/dialog) and implement a minimal layout tree just for collapse semantics + focus routing. That combination gets you very close to lazygit ergonomics without writing a full UI framework yourself.  ￼


# ARCHITECTURAL RECOMMENDATIONS:

Treat the system as three cooperating programs: a long-lived daemon that owns all “truth” and side effects, a TUI client that is purely a rendering/input shell over the daemon’s state, and an mpv subprocess that is fully controlled through JSON IPC and never directly touched by the TUI. The daemon is the only component allowed to (a) spawn/kill mpv, (b) connect to streams, (c) reconcile ICY/now-playing metadata, and (d) persist state. The TUI should be able to crash/restart at any time without affecting playback; conversely, mpv may crash/restart and the daemon should reconstitute playback to the last intended station deterministically.

In Rust, implement the daemon as an event-driven state machine with strict separation between command intent, observed reality, and derived presentation state. Concretely: define an internal CoreState that is only mutated by a single “state owner” task (one Tokio task, one mutable state), and feed it messages via channels. Every external stimulus becomes a typed event: user commands from clients, mpv events/responses, network metadata updates (ICY title changes, now-playing HTTP responses), timers/ticks, persistence acks, and health events. The state owner applies events in order and emits two things: (1) side-effect requests (spawn mpv, send mpv command, start HTTP fetch, write settings, etc.) and (2) a monotonic stream of state updates to clients. This avoids race conditions by construction: no shared mutable state across tasks, no locks in the hot path, and “ordering” is explicit in the event queue rather than implicit in scheduling.

Mpv integration should be modeled like a device driver. Spawn mpv with a dedicated IPC endpoint controlled by the daemon. Use JSON IPC and immediately put mpv into an “observed mode”: issue observe_property for every property you care about (pause, volume, mute, time-pos, duration if relevant, cache state, demuxer-cache-duration, metadata, media-title, filename, chapter if needed, audio-params, etc.). Also subscribe to general events if you rely on them (end-file, start-file, file-loaded, property-change, log-message). The daemon must treat mpv responses and property-change notifications as authoritative; a user command like “pause” becomes an intent that is only considered “applied” when the mpv property confirms it (or you time out and mark a fault). This is the key to “fully in sync” control: the daemon never assumes a command succeeded. For robustness, wrap mpv I/O in a dedicated task that does nothing but read JSON lines, parse them into typed messages, and forward to the state owner; writing commands should also be serialized through a single writer task so you never interleave or corrupt JSON, with request IDs tracked so you can correlate responses. When mpv dies or IPC breaks, emit a health event, transition state into a degraded mode, and have the state machine decide whether to restart mpv and reapply the last desired station/volume/pause state.

ICY/now-playing should be multi-source and reconciled explicitly. Build a “metadata pipeline” with pluggable providers per station: mpv-derived metadata (often enough), ICY metadata from the stream when supported, and station-specific now-playing endpoints (JSON/HTML) where necessary. Each provider emits a typed TrackInfo candidate with a source, timestamp, station id, and confidence/priority. The state machine runs a deterministic merge policy: prefer mpv/ICY if it changes frequently and is tied to the actual stream, fall back to now-playing if mpv metadata is absent or stale, and always apply monotonicity rules (ignore older updates, ignore thrashy oscillations via a short debounce). Persist the last “effective track” plus its provenance so the UI can show why it believes something. This matters because metadata is messy; you want the system to be predictably wrong rather than randomly wrong.

Client/daemon IPC should be streaming and versioned. Keep it local-first and resilient: on Unix, Unix domain sockets; on Windows, named pipes. Use a single bidirectional connection model where clients send commands (request/response, correlated by id) and the daemon pushes state updates (event stream). JSON is fine and debuggable; MessagePack/CBOR can come later if needed. The important part is schema/version discipline: include protocol version negotiation and a “full snapshot” message that the client can request on connect, followed by incremental patches (or deltas) with a monotonically increasing revision number. The simplest robust approach is “snapshot + deltas”: every state update carries rev, and the client detects gaps and requests a fresh snapshot. This makes reconnect and recovery trivial and prevents subtle drift when packets drop or clients lag. Also design multi-client semantics up front: commands are serialized by the daemon anyway, but you should decide how focus-like interactions behave (e.g., only one client can own the “interactive seek” at a time, others see it as state). In practice, treat everything as last-writer-wins on intents, but always render observed mpv truth.

Persistence should be asynchronous and non-blocking. Keep configuration (favorites, last station, per-station settings, volume default) in a small file (TOML/JSON) written atomically (write temp + rename). Keep “history/log” in a ring buffer in memory with optional append-only file if you want. The state machine should emit “persist requested” side effects on relevant changes, but not block UI responsiveness on disk. Similarly, structured logging should be tracing everywhere, with a custom Layer that forwards recent log lines into the daemon’s in-memory ring buffer so the TUI can show logs without scraping files.

On the TUI side, use ratatui with a thin, explicit component model; don’t try to make it a second brain. The TUI maintains only UI-local state (focused pane, scroll offsets, active tab, filter text, modal dialogs) plus the latest daemon snapshot. Every keystroke becomes an Action that either mutates UI-local state (e.g., move selection) or becomes a daemon command (e.g., play station, pause, set volume). The TUI never computes playback truth; it renders whatever the daemon says and, crucially, it renders “pending intents” as a distinct UX state (e.g., user pressed pause, daemon sent pause, mpv hasn’t confirmed yet → show a transient “pending” indicator rather than flipping immediately). This single UX decision eliminates the “UI lies” feeling under latency.

For heavy pane UX (collapsible/resizable panels, tabs, scrollable lists), pair ratatui with one pane/split helper and one text input helper. The most practical stack is ratatui + crossterm backend, plus ratatui-toolkit for resizable splits/panes (or similar split-pane crate), plus tui-textarea for any editable text field (search box, command palette, station URL input). Implement collapse as a feature of your layout model rather than a widget trick: represent layout as a tree (split nodes + leaf panes), and when a pane is collapsed, it becomes hidden and focus is re-routed deterministically (e.g., next visible pane). Use ratatui’s “collapsed borders” technique so adjacent panes visually merge like lazygit; then standardize a “PaneChrome” wrapper that draws title/border with focus styling and optional status badges (connected/disconnected, mpv healthy/unhealthy, metadata source). Rendering should be incremental in effect (draw on state changes), but it’s fine to keep a steady tick (e.g., 30 Hz) for spinners/progress, as long as the redraw loop is bounded and doesn’t allocate heavily.

The glue between daemon and TUI is a small, strict client library crate that both sides share for protocol types and codecs. Put protocol structs/enums in a separate crate (radio_proto) with serde derives, and keep the daemon and TUI depending on the same types to avoid divergence. Include error modeling in the protocol: every command response should be either Ok(payload) or a typed error (invalid station id, mpv unavailable, timed out waiting for confirmation, station unreachable). The TUI can then render errors consistently (toast/log pane) without guessing.

Finally, treat resiliency as a first-class behavior, not an afterthought. The daemon should have explicit health state machines: mpv state (Running/Starting/Dead/Restarting), network state per station (Connecting/Connected/Failed with exponential backoff), metadata providers (Active/Stale/Failed), and client connections (Connected/Idle/Slow consumer). Slow consumers matter: if a TUI can’t keep up with state deltas, either drop deltas and require resync or apply backpressure and disconnect with a clear reason. This prevents one stuck terminal from degrading the daemon. If you implement snapshot+delta with revision numbers, the strategy is straightforward: broadcast deltas on a bounded channel; if a client falls behind, it gets a “resync required” marker and must request a snapshot.

This architecture gives you robust sync because there is exactly one authority (daemon), exactly one owner of mutable truth (state task), and every other part is a client of that truth, including mpv. It also scales: you can add an HTTP API later (wrap the same command/state streams), add a tray UI, or add remote control, without changing the core invariants.
