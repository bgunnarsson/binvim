//! The editor's central state machine. `App` owns every long-lived value
//! (buffer, cursor, mode, registers, LSP manager, …) and the main event
//! loop drives it through the modules listed below. All `App` methods
//! live in those submodules; `app.rs` itself just defines the struct,
//! `new`/`run`/`handle_event`, and the terminal raw-mode guard.
//!
//! Sub-module map:
//! - [`state`]: shared types (Register, BufferStash, …) + small helpers
//! - [`pair`]: bracket and HTML-tag matching, auto-pair logic
//! - [`view`]: viewport, scrolling, folds, highlight cache
//! - [`search`]: search, jumps, per-line range queries for the renderer
//! - [`registers`]: registers, macros, `.` repeat
//! - [`buffers`]: buffer switching, open/close, disk reload, recents
//! - [`save`]: save flow, formatter, .editorconfig on-save, git branch
//! - [`edit`]: primitive edits — insert, replace, surround, undo, …
//! - [`visual`]: visual-mode selection helpers
//! - [`dispatch`]: `apply_action` and the operator/motion glue
//! - [`input`]: key/mouse handlers and the `:`-command dispatch
//! - [`lsp_glue`]: LSP event handling and request dispatch
//! - [`picker_glue`]: generic picker open/handle and yazi shell-out
//! - [`health`]: `:health` command output

mod android_glue;
mod buffers;
mod cmdline_complete;
mod cmdline_history;
mod comment;
mod copilot;
mod dap_glue;
mod dispatch;
mod edit;
mod file_tree;
mod git_glue;
mod health;
mod input;
pub(crate) mod installer;
mod lazygit_glue;
mod lsp_glue;
mod multi_cursor;
mod package_glue;
mod pair;
mod picker_glue;
mod quickfix;
mod registers;
mod rename_preview;
mod save;
mod search;
mod side_terminal_glue;
mod spell_glue;
pub(crate) mod state;
mod task_glue;
mod terminal_glue;
mod test_glue;
mod view;
mod visual;
mod windows;

pub use health::{DiagnosticsCounts, HealthSnapshot};
pub use side_terminal_glue::{
    PaneClickState, SideSelection, SideTerminal, TerminalFocus, extract_visible_selection_text,
    side_terminal_loading, word_bounds_in_line, word_drag_span,
};
pub use state::{
    BufferStash, CompletionState, FindRecord, FoldRange, HOVER_MAX_HEIGHT, HoverCodeBlock,
    HoverLine, HoverState, LastEdit, Register, WhichKeyState, YankHighlight,
};

use state::WHICHKEY_DELAY;

/// How long a `status_msg` notification lingers before auto-dismissing.
/// The next keypress still clears it instantly (matches Vim's `:messages`
/// behaviour); the timeout is for the case where the user doesn't touch
/// the keyboard after a save / error / status update.
const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the `:health` dashboard re-snapshots its resource / LSP /
/// buffer counts. One-second cadence gives the user a live view (so
/// they can watch CPU drop after a busy moment) without paying for a
/// `ps` shell-out at the input poll's full speed.
const HEALTH_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

use anyhow::Result;
use crossterm::event::KeyEvent;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::config::Config;
use crate::dap::DapManager;
use crate::editorconfig::EditorConfig;
use crate::lang::HighlightCache;
use crate::lsp::{CodeActionItem, CodeLensItem, InlayHint, LspManager, SignatureHelp};
use crate::mode::Mode;
use crate::parser::PendingCmd;
use crate::picker::PickerState;
use crate::render;
use crate::undo::History;

use state::RecordingState;

/// Which content the debug pane is showing. Each variant is a tab
/// in the bar across the top of the pane; clicking switches active
/// tab, and per-tab scroll positions live on `App.dap_tab_scrolls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DapPaneTab {
    /// Call stack — one row per frame in the stopped thread.
    Frames,
    /// Locals tree of the top frame — structured values expand
    /// lazily, navigated with `j`/`k`/`Enter`/`Tab`.
    Locals,
    /// User-managed watch expressions, re-evaluated per stop.
    Watches,
    /// Every active breakpoint, grouped by file. Click a row to
    /// jump to the source line (future polish).
    Breakpoints,
    /// Streaming debuggee + adapter console output.
    Console,
}

impl DapPaneTab {
    /// Tabs in the order they're rendered in the bar. Console first
    /// — it's where the user's eye lands first when they hit `:debug`,
    /// because the launch's stdout / setBreakpoints chatter shows up
    /// there before any frames or locals exist. Locals / Breakpoints
    /// / Frames / Watches follow in roughly decreasing
    /// session-frequency order.
    pub fn all() -> [DapPaneTab; 5] {
        [
            DapPaneTab::Console,
            DapPaneTab::Locals,
            DapPaneTab::Breakpoints,
            DapPaneTab::Frames,
            DapPaneTab::Watches,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            DapPaneTab::Console => "Console",
            DapPaneTab::Locals => "Locals",
            DapPaneTab::Breakpoints => "Breakpoints",
            DapPaneTab::Frames => "Frames",
            DapPaneTab::Watches => "Watches",
        }
    }
}

/// Filter preset applied to the Console tab. Cycled with `f` while
/// focus is in the pane. `All` is the default and shows every line;
/// `Program` hides adapter chatter (`console`/`telemetry`/`important`
/// categories) so only the running program's stdout + stderr remain;
/// `Errors` shows only `stderr` so the user can isolate failures
/// without scrolling past stdout noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsoleFilter {
    #[default]
    All,
    Program,
    Errors,
}

impl ConsoleFilter {
    /// Cycle in the order shown in the tab label so the user can
    /// press `f` repeatedly and predict the next state.
    pub fn next(self) -> Self {
        match self {
            ConsoleFilter::All => ConsoleFilter::Program,
            ConsoleFilter::Program => ConsoleFilter::Errors,
            ConsoleFilter::Errors => ConsoleFilter::All,
        }
    }

    /// Short suffix rendered next to the Console tab label so the
    /// active filter is visible at a glance. Empty for `All` to
    /// keep the default state unchanged-looking.
    pub fn chip(self) -> &'static str {
        match self {
            ConsoleFilter::All => "",
            ConsoleFilter::Program => "prog",
            ConsoleFilter::Errors => "err",
        }
    }

    /// True if a line emitted under `category` should be visible
    /// under this filter. Categories follow the DAP spec naming —
    /// `stdout` / `stderr` / `console` / `telemetry` / `important` —
    /// plus whatever else the active adapter happens to emit.
    pub fn allows(self, category: &str) -> bool {
        match self {
            ConsoleFilter::All => true,
            ConsoleFilter::Program => matches!(category, "stdout" | "stderr"),
            ConsoleFilter::Errors => category == "stderr",
        }
    }
}

/// One mouse-drag selection inside the Debug Console. `(line, col)`
/// pairs index into the flattened console-line view (every newline
/// counts as a line break) so the renderer + clipboard copier
/// operate on the same coordinate space.
#[derive(Debug, Clone, Copy)]
pub struct DapConsoleSelection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl DapConsoleSelection {
    /// Normalised `(start, end)` so `start <= end` regardless of
    /// which direction the user dragged.
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

pub struct App {
    pub buffer: Buffer,
    /// The active window — its cursor, viewport, Visual anchor, and the
    /// buffer index it's pointing at. Inactive windows are stashed in
    /// `windows` and swapped in when focus moves. The split tree lives
    /// in `layout`; `active_window` names which leaf is the focused one.
    pub window: crate::window::Window,
    /// Stashed view state for every non-active window. Keyed by the
    /// `WindowId` that the layout tree references.
    pub windows: HashMap<crate::layout::WindowId, crate::window::Window>,
    pub layout: crate::layout::Layout,
    pub active_window: crate::layout::WindowId,
    pub mode: Mode,
    pub pending: PendingCmd,
    pub history: History,
    pub registers: HashMap<char, Register>,
    pub cmdline: String,
    /// Insert position inside `cmdline`, as a byte offset (`<=
    /// cmdline.len()`). Drives Left/Right/Home/End navigation in
    /// the floating cmdline popup and the painted cursor cell in
    /// `draw_floating_cmdline`. Reset to `cmdline.len()` whenever
    /// the cmdline is pre-filled (rename / file-tree rename) so the
    /// cursor lands at the end of the seed text; reset to 0 when
    /// the popup opens with an empty cmdline.
    pub cmdline_cursor: usize,
    /// Active Tab-completion cycle inside `Mode::Command`. `Some` while
    /// the user is rotating candidates with successive Tab / Shift-Tab
    /// presses; cleared on any other key so the next Tab recomputes
    /// against the freshly-edited cmdline. See `app/cmdline_complete.rs`.
    pub cmdline_completion: Option<cmdline_complete::CmdlineCompletion>,
    /// Ex-command history (`:`), oldest first. Persisted to the session
    /// file alongside the buffer list; `<Up>` / `<Down>` inside Command
    /// mode walks it via `history_walk_back` / `history_walk_forward`.
    pub cmd_history: Vec<String>,
    /// Search-query history (`/` / `?`), oldest first. Same shape and
    /// recall path as `cmd_history`; the two are kept independent so
    /// that searching doesn't pollute the ex-command list and vice versa.
    pub search_history: Vec<String>,
    /// Index into the active history while the user is cycling with
    /// Up/Down. `None` means "not currently cycling — `cmdline` is
    /// the freshly-typed draft." Reset whenever Command / Search mode
    /// is entered or exited.
    pub history_cursor: Option<usize>,
    /// Snapshot of `cmdline` taken on the first Up press so walking
    /// off the bottom of history restores the in-progress draft. Reset
    /// alongside `history_cursor`.
    pub history_draft: Option<String>,
    pub status_msg: String,
    /// Bundle indices whose "toolchain missing — press `<leader>i`" hint has
    /// already fired this session. Keyed by `install::BUNDLES` index so the
    /// first-run nudge shows at most once per language until binvim restarts.
    pub toolchain_prompted: std::collections::HashSet<usize>,
    /// When the current `status_msg` was first observed non-empty.
    /// Drives the 10-second auto-dismiss timer; cleared whenever the
    /// message itself is cleared (either by the next keypress, the
    /// timeout firing, or a fresh message replacing it).
    pub status_msg_at: Option<Instant>,
    pub width: u16,
    pub height: u16,
    pub should_quit: bool,
    pub last_find: Option<FindRecord>,
    /// `(query, backward)` — direction is the original search direction so `n`/`N` honour it.
    pub last_search: Option<(String, bool)>,
    /// True when `:noh` has temporarily silenced search highlight; auto-cleared on next search.
    pub search_hl_off: bool,
    pub last_edit: Option<LastEdit>,
    pub marks: HashMap<char, (usize, usize)>,
    pub jumplist: Vec<(usize, usize)>,
    pub jump_idx: usize,
    pub macros: HashMap<char, Vec<KeyEvent>>,
    pub recording_macro: Option<char>,
    pub macro_buffer: Vec<KeyEvent>,
    pub last_replayed_macro: Option<char>,
    /// Reentry counter for `replay_macro` — incremented on entry, decremented
    /// on exit. Guards against `qa@aq` followed by `@a` spinning until the
    /// editor wedges. Aborts past `MACRO_REPLAY_DEPTH_LIMIT`.
    pub(crate) macro_replay_depth: usize,
    /// `Some(idx)` when the visual cursor is parked on the lens phantom
    /// row above its content line, with `idx` selecting which lens segment
    /// (`▶ Run | Debug` → 0 / 1). `cursor.line` is unchanged so ENTER fires
    /// the chosen lens and edits still target the content line. `None` when
    /// the cursor sits on the actual buffer row. `h`/`l` walk the segments;
    /// `j` / any non-vertical motion / any action that isn't single-step
    /// Up/Down/Left/Right drops back to content. Self-heals at render time
    /// (and at action dispatch) if `cursor.line` no longer has a lens.
    pub phantom_lens_idx: Option<usize>,
    /// All buffers; `buffers[active]` is a placeholder while its real state lives on App fields.
    pub buffers: Vec<BufferStash>,
    /// Index into `buffers` of the buffer whose content is currently
    /// live on App's fields (`buffer`, `history`, `folds`, …). May
    /// differ from `active_tab` when a split's focused pane shows a
    /// different buffer than the tab's primary one.
    pub active: usize,
    /// Index into `buffers` of the buffer whose tab (layout +
    /// per-pane Windows) is currently loaded into App.layout /
    /// App.windows. Tab swaps via `H`/`L`/`:b` move this; per-window
    /// buffer changes via `:e` / picker do not. Initial value: 0
    /// (the seed buffer's tab is the first one in view).
    pub active_tab: usize,
    /// Buffer indices that the user has "promoted" to first-class
    /// tabs — visible in the tabline, reachable via `H`/`L`. A
    /// buffer is in this set after being the active tab at any
    /// point (via switch_tab, or via the single-window switch_to
    /// rule). Split-companion buffers — opened via `<C-w>v` + picker
    /// without ever being the active tab — stay out of the set and
    /// out of the tabline. Maintained alongside `buffers` so the
    /// indices stay valid through delete / buffer_only / phantom
    /// strip shifts.
    pub tabs: std::collections::HashSet<usize>,
    pub highlight_cache: Option<HighlightCache>,
    /// Per-line markdown render meta for the active buffer. Cached
    /// against `(path, buffer.version)` and consulted by the renderer
    /// only when the active buffer is markdown AND the editor is in
    /// Normal mode (`markdown_render_active`). Insert/Visual flip back
    /// to raw markdown source.
    pub markdown_meta: Option<crate::app::state::MarkdownMetaCache>,
    pub picker: Option<PickerState>,
    pub config: Config,
    pub editorconfig: EditorConfig,
    pub lsp: LspManager,
    /// Debug session manager — owns the user's breakpoint table and the
    /// currently-active DAP session (if any).
    pub dap: DapManager,
    /// Last buffer version we shipped to the LSP, keyed by path.
    pub last_sent_version: HashMap<PathBuf, u64>,
    /// Wall-clock of the last `did_change` flush. Drives the keystroke
    /// debounce so rapid typing doesn't flood the server with one update
    /// per character.
    pub last_lsp_sync_at: Instant,
    pub completion: Option<CompletionState>,
    pub hover: Option<HoverState>,
    /// Active signature-help popup, if the cursor is currently inside a
    /// function call the LSP knows about. Auto-dismisses on Esc / `)`.
    pub signature_help: Option<SignatureHelp>,
    pub whichkey: Option<WhichKeyState>,
    pub leader_pressed_at: Option<Instant>,
    pub git_branch: Option<String>,
    /// Working-tree diff against the index for the active buffer, parsed
    /// into per-line hunk markers. Painted as a coloured stripe at column
    /// 0 of the gutter. Refreshed on save, buffer switch, and `:Gdiff`.
    pub git_hunks: Vec<crate::git::GitHunk>,
    /// Whether `:Gblame` virtual text is currently rendered for the
    /// active buffer.
    pub blame_visible: bool,
    /// Paths of buffers with `:spell` toggled on. Membership = enabled;
    /// drives the spell-check refresh in `refresh_spell_cache` and the
    /// `]s` / `[s` / `z=` action handlers.
    pub spell_enabled: std::collections::HashSet<PathBuf>,
    /// Per-buffer misspelling cache, keyed by path. Tuple is
    /// `(buffer.version, ranges)` — the version field invalidates the
    /// cache automatically when the buffer is edited so the next
    /// navigation request triggers a recheck.
    pub spell_cache: HashMap<PathBuf, (u64, Vec<crate::spell::SpellRange>)>,
    /// Per-line blame metadata for the active buffer. Populated on first
    /// toggle of `blame_visible` and refreshed on save.
    pub blame: Vec<crate::git::BlameLine>,
    /// True when binvim was launched with no path — render the start page in
    /// place of the empty buffer until the user opens something.
    pub show_start_page: bool,
    /// When true, the buffer area is replaced by the `:health` dashboard
    /// instead of the active buffer's contents. Dismissed by Esc / `q`
    /// / `:q`. Drawn by `render::draw_health_page`; the data behind it
    /// is freshly sampled per frame (cheap — no LSP traffic, just one
    /// `ps` shell-out).
    pub show_health_page: bool,
    /// Wall clock of the last frame painted while `show_health_page` is
    /// up. Paired with `HEALTH_REFRESH_INTERVAL` in the event loop so
    /// the dashboard re-snapshots resources / LSP-pending counts on a
    /// fixed cadence rather than freezing at the open-time reading.
    pub health_last_refresh: Instant,
    /// Number of dashboard rows scrolled off the top while
    /// `show_health_page` is up. Clamped against
    /// `health_content_height` by the input handlers.
    pub health_scroll: usize,
    /// Total virtual rows the dashboard occupied on the last render.
    /// Used by the input handlers to clamp `health_scroll` so the user
    /// can't scroll past the bottom. `Cell` because `render::draw`
    /// borrows `App` immutably.
    pub health_content_height: std::cell::Cell<usize>,
    /// Bottom debug pane visibility. When open it steals rows from the
    /// editor area; height is computed from terminal size in `view.rs`.
    /// Starts closed; toggled by `:dappane` and forced open by `:debug`.
    pub debug_pane_open: bool,
    /// Which tab the debug pane is currently showing. Mouse clicks on
    /// the tab bar at the top of the pane switch this; the pane body
    /// is per-tab content.
    pub dap_pane_tab: DapPaneTab,
    /// Per-tab viewport scroll offsets. `Console`'s offset counts
    /// lines hidden below the bottom (0 keeps the latest line glued
    /// to the bottom); every other tab uses "lines hidden above"
    /// like a normal scroll viewport. Stored per-tab so switching
    /// doesn't lose the user's reading position.
    pub dap_tab_scrolls: HashMap<DapPaneTab, usize>,
    /// Per-tab horizontal scroll. Long rows (deep stack frames,
    /// wide log lines, expanded structured values) need a way to
    /// see content past the pane's right edge — same pattern as
    /// the editor's `view_left`.
    pub dap_tab_h_scrolls: HashMap<DapPaneTab, usize>,
    /// Index into the flat locals tree of the currently-selected row
    /// when the Locals tab has focus. Bounded against the live tree
    /// at access time — vrefs can shift across stops, so the
    /// renderer and key handler clamp before use.
    pub dap_pane_cursor: usize,
    /// Horizontal rects of the rendered tab headers — `(tab, x_start,
    /// x_end_exclusive)`. Populated by `draw_debug_pane` each frame
    /// so the mouse handler can hit-test clicks on the tab bar.
    /// `Cell` because `render::draw` only has `&App`.
    pub dap_tab_hitboxes: std::cell::Cell<Vec<(DapPaneTab, u16, u16)>>,
    /// Active mouse-drag selection inside the Console tab. `None`
    /// when nothing's selected. Anchor + head are
    /// `(line_idx_in_flattened_buffer, char_col)`; the renderer
    /// normalises ordering when painting. Cleared on next plain
    /// click; persists across drag-release so the user sees what
    /// they just copied.
    pub dap_console_selection: Option<DapConsoleSelection>,
    /// Active filter preset on the Console tab. Cycled with `f` when
    /// the pane has focus. Default `All` is identical to pre-filter
    /// behaviour; `Program` and `Errors` hide noisier categories.
    pub dap_console_filter: ConsoleFilter,
    /// Committed search query for the Console tab. `Some` after the
    /// user hits Enter on the `/`-prompt; `None` when no search is
    /// active. Substring match, case-sensitive — keeps the model
    /// simple and predictable for log digging.
    pub dap_console_search: Option<String>,
    /// Which match the cursor is "on" in the flattened post-filter
    /// console view. 0-indexed into `dap_console_match_lines`. `n`
    /// moves forward, `N` backward; both clamp at the ends rather
    /// than wrapping so the user can tell when they've run out.
    pub dap_console_match_idx: usize,
    /// Active yank flash, if any. Drained automatically by the main loop
    /// once its `expires_at` deadline passes.
    pub yank_highlight: Option<YankHighlight>,
    /// Pending code actions waiting for the user to pick from the picker.
    /// Indexed by `PickerPayload::CodeActionIdx`.
    pub pending_code_actions: Vec<CodeActionItem>,
    /// Staging area for the multi-lens picker — the lens commands
    /// anchored on the current line, indexed by the `CodeLensIdx`
    /// picker payload. Cleared when the picker accepts or cancels.
    pub pending_code_lens_commands: Vec<crate::lsp::LspCommand>,
    /// Debug-launch state captured between the project picker and the
    /// profile picker. When `launchSettings.json` for the chosen project
    /// has more than one runnable profile, we stash the project + the
    /// parsed profile list here and open the profile picker; on accept
    /// the index in `PickerPayload::DebugProfile` selects which profile
    /// goes into the LaunchContext.
    pub pending_debug_project: Option<std::path::PathBuf>,
    pub pending_debug_profiles: Vec<crate::dap::LaunchProfile>,
    /// Discovered tasks staged between picker open and accept. Indexed
    /// by `PickerPayload::TaskIdx`. Cleared on picker accept / cancel.
    pub pending_tasks: Vec<crate::task::Task>,
    /// Most recent task spawned this session. Drives `:tasklast` /
    /// `<leader>ml`. Kept across picker invocations so re-runs survive
    /// independent picker sessions; cleared on a clean shutdown when
    /// the session is saved.
    pub last_task: Option<crate::task::Task>,
    /// LSP rename preview state. `Some` while the user is reviewing
    /// a `WorkspaceEdit` from `textDocument/rename` and hasn't yet
    /// accepted (apply enabled rows) or cancelled. Drives the modal
    /// overlay; cleared on either resolution path.
    pub pending_rename_preview: Option<crate::app::state::RenamePreview>,
    /// Symbol position captured when a rename prompt opened — the LSP
    /// rename request needs the original `(line, col)` even after the
    /// prompt has stolen focus and the user has moved focus around.
    pub rename_anchor: Option<(PathBuf, usize, usize, String)>,
    /// C# / Razor find-references augment context. Stashed when the
    /// request is fired; consumed when the LSP reply arrives so a
    /// ripgrep pass across `.cshtml` / `.razor` can be merged into
    /// the picker. See `state::PendingRefAugment`.
    pub pending_ref_augment: Option<crate::app::state::PendingRefAugment>,
    /// Computed fold ranges for the active buffer (cached against `folds_version`).
    pub folds: Vec<FoldRange>,
    pub folds_version: u64,
    pub closed_folds: std::collections::HashSet<usize>,
    /// Most-recently-used files for the file picker. Persisted to
    /// `~/.cache/binvim/recents`.
    pub recents: Vec<PathBuf>,
    /// Latest inlay hints per buffer path, indexed by canonicalised path.
    /// Cleared on buffer reload, replaced on each `InlayHints` event.
    pub inlay_hints: HashMap<PathBuf, Vec<InlayHint>>,
    /// Buffer version we last asked inlay hints for, per path. Lets us
    /// skip the request when nothing changed since the previous response.
    pub last_inlay_request_version: HashMap<PathBuf, u64>,
    /// Wall clock of the last disk-mtime probe — drives the watch-and-reload
    /// loop without spamming syscalls.
    pub last_disk_check: Instant,
    /// Wall-clock + buffer position of the most recent left-click. A second
    /// click at the same `(line, col)` within `DOUBLE_CLICK_WINDOW` is
    /// treated as a double-click and selects the word under the cursor.
    pub last_click: Option<(Instant, usize, usize)>,
    /// `(timestamp, entry_idx)` of the most recent click inside the
    /// file-tree pane. A second click on the same entry inside
    /// `DOUBLE_CLICK_WINDOW` triggers open-or-expand; the first
    /// click just moves the tree cursor.
    pub last_tree_click: Option<(Instant, usize)>,
    /// Char-idx range `(start, end_exclusive)` of the word a double-click
    /// originated on. While `Some`, left-drag extends the visual selection
    /// in word increments anchored to this range, snapping to word
    /// boundaries instead of moving char-by-char. Cleared on next plain
    /// left-click or on exiting Visual.
    pub word_drag_origin: Option<(usize, usize)>,
    /// Char-index positions of secondary cursors that mirror Insert-mode
    /// edits at the primary cursor. Empty when only one cursor is active.
    /// Populated by `Ctrl-click` in Insert mode; cleared on Esc.
    pub additional_cursors: Vec<usize>,
    /// Active TextMate snippet session — populated when a multi-stop
    /// snippet completion expands; Tab cycles the cursor through its
    /// stops. `None` when no snippet is in flight.
    pub snippet_session: Option<crate::app::state::SnippetSession>,
    /// Quickfix list — `:cnext` / `:cprev` / `]q` / `[q` navigate it.
    /// Loaded from grep, LSP references, or diagnostics; `None` when
    /// nothing has been populated yet.
    pub quickfix: Option<crate::app::state::QuickfixState>,
    /// Additional Visual-char selection ranges (start, end exclusive).
    /// Populated by `Ctrl-N` while a Visual-char selection is active;
    /// `d`/`c`/`y` then operates on every range plus the primary one.
    /// Cleared on Esc / exiting Visual / collapse.
    pub additional_selections: Vec<(usize, usize)>,
    pub(crate) replaying_macro: bool,
    pub(crate) recording: Option<RecordingState>,
    pub(crate) replaying: bool,
    /// True when `App::new` restored buffers from a saved session on
    /// launch (no explicit file argument + a session file matched the
    /// cwd). Surfaced by the `:health` dashboard so the user can tell
    /// whether the buffer list was restored or seeded fresh.
    pub session_restored: bool,
    /// Active Copilot ghost suggestion — rendered as muted text after
    /// the cursor in Insert mode, accepted with `<Tab>`. Cleared on
    /// any non-Tab keystroke or when the cursor moves away.
    pub copilot_ghost: Option<CopilotGhost>,
    /// Wall clock of the last keystroke in Insert mode. Drives the
    /// idle-pause that fires `textDocument/inlineCompletion` against
    /// the Copilot LSP — we wait until ~250ms of typing-idle so we
    /// don't spam the server with requests for every character.
    pub last_keystroke_at: Instant,
    /// Buffer version we last asked Copilot for an inline completion
    /// for, keyed by path. Skips the request when nothing has changed
    /// since the previous response (after accept / reject the ghost
    /// is cleared, so a re-request will fire on the next idle).
    pub last_copilot_request_version: HashMap<PathBuf, u64>,
    /// Wall clock of the last `checkStatus` request fired while
    /// Copilot is in `PendingAuth`. Drives a 3-second poll so binvim
    /// picks up "user finished signing in" without needing a restart
    /// or a manual `:copilot reload`.
    pub last_copilot_status_poll: Instant,
    /// Per-buffer semantic-token cache from
    /// `textDocument/semanticTokens/full`. Refreshed once per buffer
    /// version when the server advertises the capability. The renderer
    /// layers these over the tree-sitter highlight cache so LSP-aware
    /// distinctions (mutable vs immutable, async functions, defaultLibrary
    /// symbols, ...) override the static-query colour where they apply.
    pub semantic_tokens: HashMap<PathBuf, SemanticTokensCache>,
    /// Buffer version we last asked semantic-tokens for, per path.
    /// Same dedupe strategy as inlay hints.
    pub last_semantic_tokens_request_version: HashMap<PathBuf, u64>,
    /// Per-buffer document-highlight cache, keyed by canonicalised path.
    /// Refreshed by the idle-pause path in `app/lsp_glue.rs` whenever the
    /// cursor lands on a new symbol. The renderer paints a subtle bg on
    /// every range whose anchor still matches the live cursor, so a
    /// stale cache (cursor moved off, response not back yet) doesn't
    /// flash wrong highlights.
    pub document_highlights: HashMap<PathBuf, DocumentHighlightCache>,
    /// Paths with an in-flight `textDocument/documentHighlight`
    /// request. Capped at one per path so a user navigating fast (or
    /// firing during a server's cold-start indexing window) can't
    /// queue up hundreds of requests that the server hasn't gotten to
    /// yet — intermediate positions get skipped, and the next idle
    /// render after the response fires for wherever the cursor has
    /// settled.
    pub document_highlight_in_flight: std::collections::HashSet<PathBuf>,
    /// Paths with an in-flight `textDocument/inlayHint` request. Same
    /// cap-to-one semantics as `document_highlight_in_flight`.
    pub inlay_hints_in_flight: std::collections::HashSet<PathBuf>,
    /// Paths with an in-flight `textDocument/semanticTokens/full`
    /// request. Same cap-to-one semantics as the others.
    pub semantic_tokens_in_flight: std::collections::HashSet<PathBuf>,
    /// Merged code-lens cache — the union of `lsp_only_code_lens` and
    /// `synth_only_code_lens` for the same buffer version. The
    /// renderer / `<leader>l` / click path / picker all read from
    /// here so they don't have to know whether a lens came from the
    /// LSP or from the in-tree tree-sitter synthesiser.
    /// `refresh_code_lens` is the only writer.
    pub code_lens: HashMap<PathBuf, CodeLensCache>,
    /// `textDocument/codeLens` answers from the LSP, kept around
    /// even when the merged cache is empty so the synth-side merge
    /// has something to layer on top of. `(version, lenses)`. Empty
    /// vec is a valid value (means "server responded with []").
    pub lsp_only_code_lens: HashMap<PathBuf, (u64, Vec<CodeLensItem>)>,
    /// Synthetic code lenses produced by the in-tree tree-sitter
    /// walk in `code_lens_synth.rs`. Same `(version, lenses)` shape.
    pub synth_only_code_lens: HashMap<PathBuf, (u64, Vec<CodeLensItem>)>,
    /// Buffer version we last ran the synth pass for, per path.
    /// Version-dedupe so a stable buffer doesn't re-walk the tree on
    /// every render tick.
    pub last_synth_lens_version: HashMap<PathBuf, u64>,
    /// Buffer version we last asked code-lens for, per path. Same
    /// version-dedup strategy as inlay hints / semantic tokens.
    pub last_code_lens_request_version: HashMap<PathBuf, u64>,
    /// Paths with an in-flight `textDocument/codeLens` request. Same
    /// cap-to-one semantics as the other per-buffer LSP request caches.
    pub code_lens_in_flight: std::collections::HashSet<PathBuf>,
    /// Wall-clock of the last `textDocument/codeLens` request, per
    /// path. Drives a slow retry when the LSP cache is empty: rust-
    /// analyzer commonly returns `[]` while it's still indexing, and
    /// without a retry the version-dedupe would otherwise pin us at
    /// "we already asked for this version" forever — the lens row
    /// would never appear unless the user edited the buffer.
    pub last_code_lens_request_at: HashMap<PathBuf, Instant>,
    /// `:terminal` pane terminals. Each entry is one PTY-backed
    /// shell + grid; the pane renders at the bottom of the editor
    /// when `terminal_pane_open` is true. Empty when no terminal
    /// has been spawned (or all have been closed). `<leader>tt`
    /// always appends a new entry — long-running processes like
    /// `pnpm dev` get their own tab so they can run in parallel.
    /// Tabs only render in the header when there are 2 or more —
    /// a single terminal hides the strip and uses that space for
    /// the hint text instead.
    pub terminals: Vec<crate::terminal::Terminal>,
    pub active_terminal_idx: usize,
    /// Hit-test rectangles for the terminal tab strip — same
    /// pattern as `dap_tab_hitboxes`. Each `(idx, x_start, x_end)`
    /// covers one tab label on the header row. Populated by
    /// `draw_terminal_pane` every frame, consumed by mouse-down
    /// inside the pane header.
    pub terminal_tab_hitboxes: std::cell::Cell<Vec<(usize, u16, u16)>>,
    pub terminal_pane_open: bool,
    /// Right-side terminal pane — dedicated to long-running AI
    /// assistants (`:claude`, `:codex`, `:opencode`). Each entry is
    /// one PTY-backed shell + grid plus a stable `label` we dedupe
    /// against so re-running `:claude` re-focuses the existing tab
    /// instead of spawning a duplicate. Sits on the right edge of
    /// the editor band (width ≈ 25 % of the host terminal), parallel
    /// to but independent of the bottom `terminals` pane: both panes
    /// can be open at the same time, and `terminal_focus` selects
    /// which one consumes keystrokes while `Mode::Terminal` is
    /// active.
    pub side_terminals: Vec<SideTerminal>,
    pub active_side_terminal_idx: usize,
    pub side_terminal_pane_open: bool,
    /// Hit-test rectangles for the side-pane tab strip. One entry
    /// per drawn tab — `(tab_index, x_start, x_end_exclusive)` in
    /// screen columns. Populated by `draw_side_terminal_pane` each
    /// frame the strip is visible, consumed by mouse-down in the
    /// header row to switch tabs.
    pub side_terminal_tab_hitboxes: std::cell::Cell<Vec<(usize, u16, u16)>>,
    /// Pane-scoped mouse-drag text selection in the active side
    /// terminal tab. Drives both the selection-highlight overlay
    /// (`render.rs::draw_side_terminal_pane`) and the clipboard
    /// copy on `Up`. Cleared on tab switch, pane close, resize, and
    /// any non-drag mouse-down that lands in the pane.
    pub side_terminal_selection: Option<SideSelection>,
    /// Same shape, but for the bottom `:terminal` pane. Reuses
    /// `SideSelection` because the data carrier is identical (just
    /// `(anchor, head, tab_idx, dragging)`); the `tab_idx` here keys
    /// into `terminals` rather than `side_terminals`. Cleared on the
    /// same lifecycle events.
    pub terminal_selection: Option<SideSelection>,
    /// Double-click + word-drag tracking for the bottom `:terminal`,
    /// AI side pane, and DAP console respectively. Each pane keeps its
    /// own so a double-click in one can't false-trigger off a click in
    /// another at a coincidentally-equal pane-local cell.
    pub term_click: PaneClickState,
    pub side_click: PaneClickState,
    pub dap_click: PaneClickState,
    /// Which terminal pane consumes keystrokes when `Mode::Terminal`
    /// is active. Set whenever focus moves between the bottom and
    /// side panes; resets to `Bottom` after `close_side_terminal`
    /// drops the last side tab.
    pub terminal_focus: TerminalFocus,
    /// Ring buffer of server-emitted `window/showMessage` /
    /// `window/logMessage` notifications. Newest at the tail. Bounded
    /// so a chatty server (jdtls warming up, OmniSharp resolving
    /// references) can't grow it without limit. Viewable via
    /// `:messages`; the loudest entries (showMessage Error/Warning)
    /// also flash through `status_msg`.
    pub lsp_messages: Vec<LspServerMessage>,
    /// `:messages` overlay toggle. When true, the buffer area is
    /// replaced with the scrollable list of `lsp_messages`. Dismissed
    /// by Esc / `q` / `:q`. Scroll position in `messages_scroll`.
    pub show_messages_page: bool,
    pub messages_scroll: usize,
    /// Total virtual rows the messages list occupied on the last
    /// render — used to clamp `messages_scroll`. `Cell` because
    /// `render::draw` borrows `App` immutably.
    pub messages_content_height: std::cell::Cell<usize>,
    /// `:reg` / `:registers` overlay toggle. Lists yank registers
    /// alongside recorded macro registers with a short preview.
    /// Dismissed the same way as `:messages`.
    pub show_registers_page: bool,
    pub registers_scroll: usize,
    pub registers_content_height: std::cell::Cell<usize>,
    /// Integrated test runner — owns the active run + the streaming
    /// output buffer. Drained per main-loop tick alongside `lsp` /
    /// `dap`; the resulting events go through `handle_test_events`.
    pub test: crate::test::TestManager,
    /// `<leader>p` package manager — owns the background-thread channel
    /// and the active install/search flow. Drained per tick like `test`;
    /// results advance the flow in `handle_package_events`.
    pub package: state::PackageState,
    /// `<leader>A` Android emulator manager — owns its own background-thread
    /// channel and the active create-AVD flow. Drained per tick like
    /// `package`; results advance the flow in `handle_android_events`.
    pub android: state::AndroidState,
    /// Debug-attach context captured between `AndroidEvent::DebugReady` (adb
    /// forward done) and the jdtls `JavaDebugSession` reply that hands back the
    /// DAP port — at which point we attach and clear this.
    pub pending_android_debug: Option<crate::android::DebugPrep>,
    /// `:testresults` / auto-opened-on-run overlay toggle. Same
    /// scrollable-overlay pattern as `:health` / `:messages`.
    pub show_test_results_page: bool,
    pub test_results_scroll: usize,
    pub test_results_content_height: std::cell::Cell<usize>,
    /// `:install` overlay toggle. When true, the buffer area is
    /// replaced with the installer's checkbox / plan view. Dismissed
    /// by `q` / `Esc` / `:q`, or by completing the run flow which
    /// calls `dismiss_install`.
    pub show_install_page: bool,
    /// State for the active installer overlay — `None` once it's
    /// dismissed. Holds the current stage (Bundles / NodeVersions /
    /// Plan), cursor + per-row check state, prior picks (so the
    /// `n`-on-Plan back-step preserves them), detected Node.js
    /// installations, and the built plan.
    pub installer: Option<installer::InstallerState>,
    /// Tail-follow mode for the results overlay — when true, the
    /// renderer pins the scroll position to the bottom each frame so
    /// new streaming events stay visible without the user having to
    /// `G`. User scrolling upward drops out of tail mode; scrolling
    /// back down to the bottom (or pressing `G`) re-engages it.
    /// Reset to true on every new run start.
    pub test_results_at_tail: bool,
    /// Active sidebar file-tree state. `Some` whenever the pane is
    /// open; `None` closes the pane. Gated on
    /// `config.file_explorer.tree` — `<leader>e` falls back to the
    /// yazi shell-out when the tree explorer is disabled.
    pub file_tree: Option<file_tree::FileTreeState>,
}

/// Decoded `textDocument/semanticTokens/full` tokens for one buffer,
/// plus a `(line, char_count)` lookup table that lets the renderer
/// translate the spec's UTF-16-code-unit `start_col` / `length` into
/// the char columns the rope pipeline operates on. We pre-bin tokens
/// by `line` so per-row lookups are constant-time during draw.
#[derive(Debug, Clone)]
pub struct SemanticTokensCache {
    pub buffer_version: u64,
    /// Tokens grouped by line index. Each row's vec is sorted by
    /// `start_col` ascending — the renderer assumes this when binary-
    /// searching for the token covering a given column.
    pub by_line: Vec<Vec<crate::lsp::SemanticToken>>,
}

/// Cached `textDocument/documentHighlight` ranges for one buffer.
/// `anchor_version` gates rendering — the highlights only paint while
/// the buffer version still matches and the live cursor falls inside one
/// of the ranges (see `line_document_highlights`), so a stale response
/// doesn't smear yesterday's highlights over today's code. The request's
/// `anchor_line`/`anchor_col` are echoed back for diagnostics but no
/// longer feed the gate.
#[derive(Debug, Clone)]
pub struct DocumentHighlightCache {
    #[allow(dead_code)]
    pub anchor_line: usize,
    #[allow(dead_code)]
    pub anchor_col: usize,
    pub anchor_version: u64,
    pub ranges: Vec<crate::lsp::DocumentHighlightRange>,
}

/// Cached `textDocument/codeLens` lenses for one buffer. `buffer_version`
/// is the version the request was anchored on — the renderer checks it
/// against the live buffer before painting and discards stale lenses
/// whose line indices no longer match. `anchor_lines` is a flat set of
/// the lines lenses are anchored to, so the per-row paint walk can
/// answer "does this line have a lens above it?" in O(1) instead of
/// scanning the full lens list per row.
#[derive(Debug, Clone)]
pub struct CodeLensCache {
    pub buffer_version: u64,
    pub lenses: Vec<CodeLensItem>,
    pub anchor_lines: std::collections::HashSet<usize>,
}

/// One captured `window/showMessage` or `window/logMessage` entry.
/// Stored in `App.lsp_messages` so the user can scroll back through
/// server-emitted notifications via `:messages` instead of losing
/// every line that scrolled out of `status_msg`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LspServerMessage {
    pub client_key: String,
    pub severity: crate::lsp::MessageSeverity,
    pub text: String,
    /// `true` when the server fired `window/showMessage` (loud);
    /// `false` for the quieter `window/logMessage`.
    pub is_show: bool,
    /// Capture time — kept for a future timestamp column in the
    /// overlay; currently unused at the renderer level.
    pub when: Instant,
}

/// Active Copilot ghost suggestion. `text` is the FULL replacement
/// content the server returned — it may already include whatever the
/// user has typed between `replace_start` and the cursor. On accept
/// the buffer span `[replace_start .. cursor]` is wiped and `text` is
/// inserted at `replace_start`, so the user's existing prefix isn't
/// duplicated. The ghost-render layer strips the common prefix off
/// `text` (against the live buffer text from `replace_start` to the
/// cursor) and shows only the divergent tail.
#[derive(Debug, Clone)]
pub struct CopilotGhost {
    pub text: String,
    /// Cursor position the request was anchored on. The ghost is
    /// dropped if the cursor has since moved off it.
    pub line: usize,
    pub col: usize,
    /// Start of the replacement range — usually `<= (line, col)`.
    /// Often the beginning of the current line.
    pub replace_start_line: usize,
    pub replace_start_col: usize,
    pub path: PathBuf,
}

/// How quickly a second left-click must arrive at the same buffer position
/// to register as a double-click. Tuned to typical OS double-click defaults.
pub const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);

impl App {
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        // Always load the saved session for this cwd if one exists —
        // we use it for cmdline / search history regardless of how the
        // editor was launched. Buffer restoration is gated separately
        // on `path.is_none()`, so `binvim foo.rs` doesn't pull every
        // previously-open buffer back in, but it DOES keep your `:` /
        // `/` recall warm. The session can be history-only (e.g. after
        // `<leader>bA` closed every buffer but we kept the file alive
        // to preserve recall) — in that case there are no buffers to
        // restore, so we want the start page, not the bare seed buffer.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let saved_session = crate::session::load_for_cwd(&cwd);
        let session_has_buffers = saved_session
            .as_ref()
            .map(|s| !s.buffers.is_empty())
            .unwrap_or(false);
        let restore_buffers = path.is_none() && session_has_buffers;
        let show_start_page = path.is_none() && !restore_buffers;
        let buffer = match path.as_ref() {
            Some(p) => Buffer::from_path(p.clone())?,
            None => Buffer::empty(),
        };
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let (layout, root_window) = crate::layout::Layout::new();
        let mut this = Self {
            buffer,
            window: crate::window::Window::default(),
            windows: HashMap::new(),
            layout,
            active_window: root_window,
            mode: Mode::Normal,
            pending: PendingCmd::default(),
            history: History::new(),
            registers: HashMap::new(),
            cmdline: String::new(),
            cmdline_cursor: 0,
            cmdline_completion: None,
            cmd_history: Vec::new(),
            search_history: Vec::new(),
            history_cursor: None,
            history_draft: None,
            status_msg: String::new(),
            toolchain_prompted: std::collections::HashSet::new(),
            status_msg_at: None,
            width: w,
            height: h,
            should_quit: false,
            last_find: None,
            last_search: None,
            search_hl_off: false,
            last_edit: None,
            marks: HashMap::new(),
            jumplist: Vec::new(),
            jump_idx: 0,
            macros: HashMap::new(),
            recording_macro: None,
            macro_buffer: Vec::new(),
            last_replayed_macro: None,
            macro_replay_depth: 0,
            phantom_lens_idx: None,
            buffers: vec![BufferStash::default()],
            active: 0,
            active_tab: 0,
            tabs: {
                let mut s = std::collections::HashSet::new();
                s.insert(0);
                s
            },
            highlight_cache: None,
            markdown_meta: None,
            picker: None,
            config: Config::load(),
            editorconfig: EditorConfig::default(),
            lsp: LspManager::new(),
            dap: DapManager::new(),
            last_sent_version: HashMap::new(),
            last_lsp_sync_at: Instant::now(),
            completion: None,
            hover: None,
            signature_help: None,
            whichkey: None,
            leader_pressed_at: None,
            git_branch: save::detect_git_branch(
                &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ),
            git_hunks: Vec::new(),
            blame_visible: false,
            spell_enabled: std::collections::HashSet::new(),
            spell_cache: HashMap::new(),
            blame: Vec::new(),
            show_start_page,
            show_health_page: false,
            health_last_refresh: Instant::now(),
            health_scroll: 0,
            health_content_height: std::cell::Cell::new(0),
            debug_pane_open: false,
            dap_pane_cursor: 0,
            dap_pane_tab: DapPaneTab::Console,
            dap_tab_scrolls: HashMap::new(),
            dap_tab_h_scrolls: HashMap::new(),
            dap_tab_hitboxes: std::cell::Cell::new(Vec::new()),
            dap_console_selection: None,
            dap_console_filter: ConsoleFilter::default(),
            dap_console_search: None,
            dap_console_match_idx: 0,
            yank_highlight: None,
            pending_code_actions: Vec::new(),
            pending_code_lens_commands: Vec::new(),
            pending_debug_project: None,
            pending_debug_profiles: Vec::new(),
            pending_tasks: Vec::new(),
            last_task: None,
            pending_rename_preview: None,
            rename_anchor: None,
            pending_ref_augment: None,
            folds: Vec::new(),
            folds_version: u64::MAX,
            closed_folds: std::collections::HashSet::new(),
            recents: buffers::load_recents(),
            inlay_hints: HashMap::new(),
            last_inlay_request_version: HashMap::new(),
            last_disk_check: Instant::now(),
            last_click: None,
            last_tree_click: None,
            word_drag_origin: None,
            additional_cursors: Vec::new(),
            snippet_session: None,
            quickfix: None,
            additional_selections: Vec::new(),
            replaying_macro: false,
            recording: None,
            replaying: false,
            session_restored: restore_buffers,
            copilot_ghost: None,
            last_keystroke_at: Instant::now(),
            last_copilot_request_version: HashMap::new(),
            last_copilot_status_poll: Instant::now(),
            semantic_tokens: HashMap::new(),
            last_semantic_tokens_request_version: HashMap::new(),
            terminals: Vec::new(),
            active_terminal_idx: 0,
            terminal_tab_hitboxes: std::cell::Cell::new(Vec::new()),
            terminal_pane_open: false,
            side_terminals: Vec::new(),
            active_side_terminal_idx: 0,
            side_terminal_pane_open: false,
            side_terminal_tab_hitboxes: std::cell::Cell::new(Vec::new()),
            side_terminal_selection: None,
            terminal_selection: None,
            term_click: PaneClickState::default(),
            side_click: PaneClickState::default(),
            dap_click: PaneClickState::default(),
            terminal_focus: TerminalFocus::Bottom,
            document_highlights: HashMap::new(),
            document_highlight_in_flight: std::collections::HashSet::new(),
            inlay_hints_in_flight: std::collections::HashSet::new(),
            semantic_tokens_in_flight: std::collections::HashSet::new(),
            code_lens: HashMap::new(),
            lsp_only_code_lens: HashMap::new(),
            synth_only_code_lens: HashMap::new(),
            last_synth_lens_version: HashMap::new(),
            last_code_lens_request_version: HashMap::new(),
            code_lens_in_flight: std::collections::HashSet::new(),
            last_code_lens_request_at: HashMap::new(),
            lsp_messages: Vec::new(),
            show_messages_page: false,
            messages_scroll: 0,
            messages_content_height: std::cell::Cell::new(0),
            show_registers_page: false,
            registers_scroll: 0,
            registers_content_height: std::cell::Cell::new(0),
            test: crate::test::TestManager::new(),
            package: state::PackageState::new(),
            android: state::AndroidState::new(),
            pending_android_debug: None,
            show_test_results_page: false,
            show_install_page: false,
            installer: None,
            test_results_scroll: 0,
            test_results_content_height: std::cell::Cell::new(0),
            test_results_at_tail: true,
            file_tree: None,
        };
        // Mirror the user's Copilot opt-in onto the LSP manager so
        // copilot-language-server is included in every spec lookup
        // from here forward. The flag has to be set *before* any
        // buffer is opened (and thus before `lsp_attach_active` runs)
        // so the initial didOpen carries the Copilot client too.
        this.lsp.copilot_enabled = this.config.copilot.enabled;
        // Hydrate from the saved session — histories always restore so
        // `:` / `/` recall stays warm regardless of launch mode; the
        // buffer set restores only when no path arg was given (so
        // `binvim foo.rs` opens just `foo.rs`, not the whole prior set).
        if let Some(s) = saved_session {
            this.cmd_history = s.cmd_history.clone();
            this.search_history = s.search_history.clone();
            this.macros = s
                .macros
                .iter()
                .map(|(name, keys)| (*name, keys.iter().map(|k| k.to_event()).collect()))
                .collect();
            if restore_buffers {
                this.hydrate_from_session(s);
            }
        }
        Ok(this)
    }

    pub fn run(&mut self) -> Result<()> {
        let _guard = TerminalGuard::enable()?;
        let mut stdout = io::stdout();
        // CLI-launched buffer (binvim huge.json) bypasses the
        // open_buffer path that ordinarily surfaces this hint, so fire
        // it here before lsp_attach_active short-circuits silently.
        if self.buffer.is_large() {
            self.status_msg = "large file — tree-sitter + LSP disabled".into();
        }
        self.lsp_attach_active();
        // Same first-run toolchain nudge open_buffer fires, for the
        // CLI-launched buffer that never went through it. No-op on the
        // start page / restored session (no path) and on large files.
        self.maybe_prompt_toolchain();
        self.refresh_editorconfig();
        self.refresh_git_hunks();
        let mut needs_render = true;
        // Set when a PTY drain hit its per-tick byte budget with output
        // still queued. Carried into the next iteration's poll budget so
        // we come straight back to finish draining instead of idling —
        // see the poll-budget block below.
        let mut pty_backlog = false;
        while !self.should_quit {
            if needs_render {
                self.maybe_reload_from_disk();
                self.adjust_viewport();
                self.ensure_highlights();
                self.ensure_markdown_meta();
                self.ensure_inactive_markdown_meta();
                self.ensure_folds();
                self.lsp_sync_active_debounced();
                self.lsp_request_inlay_hints_if_due();
                self.lsp_request_semantic_tokens_if_due();
                self.lsp_request_document_highlight_if_due();
                self.lsp_request_code_lens_if_due();
                self.synth_code_lens_if_due();
                self.copilot_maybe_request_inline();
                self.copilot_maybe_poll_status();
                render::draw(&mut stdout, self)?;
                stdout.flush()?;
                needs_render = false;
            }
            // Compute the poll budget — a pending leader-prefix shortens it so the
            // which-key popup appears promptly when the user pauses. The poll
            // wakes early on any input event, so a 100ms ceiling is fine even
            // when the LSP backlog is being drained in chunks.
            let mut poll_dur = match self.leader_pressed_at {
                Some(t) => {
                    let target = t + WHICHKEY_DELAY;
                    target
                        .checked_duration_since(Instant::now())
                        .unwrap_or(Duration::from_millis(0))
                        .min(Duration::from_millis(100))
                }
                None => Duration::from_millis(100),
            };
            // Side-pane loading splash animates a spinner — cap the
            // poll to one spinner step so the next frame fires when
            // the glyph actually changes (and not before). Matching
            // the renderer's 150ms-per-frame cadence keeps us from
            // burning a full clear+redraw cycle just to repaint the
            // same glyph, which is what the user perceived as
            // flicker.
            if self
                .side_terminals
                .iter()
                .any(side_terminal_glue::side_terminal_loading)
            {
                poll_dur = poll_dur.min(Duration::from_millis(150));
                needs_render = true;
            }
            // Active debug session — DAP stepping / breakpoint hits
            // cascade 4-5 request/response round-trips per user action
            // (stopped → threads → stackTrace → scopes → variables), and
            // each round-trip waits for the next poll wake-up to drain
            // the reader channel. Tightening the budget here turns
            // stepping from "noticeably slow" into "instant".
            if self.dap.is_active() {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // Active `:terminal` overlay — same reasoning as the
            // DAP case. PTY output arrives asynchronously; a long
            // poll budget delays the next render by that much, so
            // typing in the embedded shell feels laggy. 16ms is
            // ~60fps and well under the threshold of perception.
            if !self.terminals.is_empty() {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // Live AI side pane (`:claude` / `:codex` / `:opencode`) —
            // same reasoning as the `:terminal` overlay. The pane's PTY
            // echo and streaming output arrive asynchronously and are
            // drained by `side_terminal_drain_if_open`, not as crossterm
            // events, so a 100ms poll budget makes the echo of each
            // keystroke lag a full beat behind the keypress — typing
            // feels slower than the user types. Cap to 16ms once a pane
            // is past its loading splash (which keeps its own 150ms
            // spinner cadence above; capping during loading would burn a
            // full redraw per 16ms just to repaint the same glyph).
            if self
                .side_terminals
                .iter()
                .any(|s| !side_terminal_glue::side_terminal_loading(s))
            {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // Active test run — adapter output streams in
            // asynchronously. Same 16ms cap as the DAP / terminal
            // cases so the user watches tests tick by live rather
            // than waiting on the next keystroke.
            if self.test.is_running() {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // A package-manager op (search / restore / add) is in flight on
            // a background thread — tighten so its result is applied promptly
            // instead of waiting on the next keystroke.
            if self.package.busy {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // An Android SDK op (list / create / launch) is in flight on a
            // background thread — same tightening so its result lands promptly.
            if self.android.busy {
                poll_dur = poll_dur.min(Duration::from_millis(16));
            }
            // A live yank flash needs us to wake up at its deadline so the
            // highlight clears on time.
            if let Some(h) = self.yank_highlight.as_ref() {
                let until = h
                    .expires_at
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::from_millis(0));
                poll_dur = poll_dur.min(until);
            }
            // Pending debounced LSP sync — wake at the deadline so the
            // server sees the user's pause-burst flush even if no further
            // key arrives.
            if let Some(due) = self.lsp_sync_due_at() {
                let until = due
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::from_millis(0));
                poll_dur = poll_dur.min(until);
            }
            // Active notification — wake at the 10s deadline so the box
            // disappears on time even if the user never touches the
            // keyboard.
            if let Some(at) = self.status_msg_at {
                let until = (at + NOTIFICATION_TIMEOUT)
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::from_millis(0));
                poll_dur = poll_dur.min(until);
            }
            // Health dashboard refresh tick — wake at the next 1s mark so
            // the resource numbers actually update while the user looks
            // at them.
            if self.show_health_page {
                let until = (self.health_last_refresh + HEALTH_REFRESH_INTERVAL)
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::from_millis(0));
                poll_dur = poll_dur.min(until);
            }
            // Code-lens retry deadline — cap the poll so the loop
            // wakes within a beat of the retry being due, otherwise
            // an idle user wouldn't see lenses appear until rust-
            // analyzer fired some unrelated notification.
            let lens_retry_due = self.code_lens_retry_due();
            if lens_retry_due {
                poll_dur = poll_dur.min(Duration::from_millis(250));
            }
            // A PTY drain hit its byte budget last iteration with bytes
            // still queued. Don't idle on the poll timeout — come right
            // back to finish draining. poll(0) still returns instantly
            // when a key is pending, so input stays responsive while the
            // pane catches up across ticks instead of freezing for the
            // whole burst.
            if pty_backlog {
                poll_dur = Duration::from_millis(0);
            }
            if crossterm::event::poll(poll_dur)? {
                self.handle_event()?;
                needs_render = true;
                // Coalesce an input burst into a single render. Each event
                // otherwise forces a full-screen redraw + flush, so a fast
                // trackpad/wheel scroll over a pane — which the OS delivers
                // as a flood of ScrollUp/Down events — backs up dozens of
                // frames and the editor visibly lags catching up after the
                // gesture ends. Drain everything already queued (poll(0) is
                // instant) and draw once at the loop top. The cap stops a
                // continuous event stream from starving the PTY drains below.
                let mut batched = 0;
                while batched < 256 && crossterm::event::poll(Duration::from_millis(0))? {
                    self.handle_event()?;
                    batched += 1;
                }
            }
            // Poll timed out and a lens retry is due — force a render
            // tick so the `if_due` hook actually fires the request.
            if !needs_render && lens_retry_due {
                needs_render = true;
            }
            // Prefix timeout fired? Open the matching which-key popup.
            if let Some(t) = self.leader_pressed_at {
                if Instant::now() >= t + WHICHKEY_DELAY {
                    let popup = if self.pending.awaiting_leader {
                        Some(WhichKeyState {
                            title: "Leader".into(),
                            entries: state::leader_entries(),
                        })
                    } else if self.pending.awaiting_buffer_leader {
                        Some(WhichKeyState {
                            title: "Buffer".into(),
                            entries: state::buffer_prefix_entries(),
                        })
                    } else if self.pending.awaiting_debug_leader {
                        Some(WhichKeyState {
                            title: "Debug".into(),
                            entries: state::debug_prefix_entries(),
                        })
                    } else if self.pending.awaiting_hunk_leader {
                        Some(WhichKeyState {
                            title: "Hunk".into(),
                            entries: state::hunk_prefix_entries(),
                        })
                    } else if self.pending.awaiting_git_leader {
                        Some(WhichKeyState {
                            title: "Git".into(),
                            entries: state::git_prefix_entries(),
                        })
                    } else if self.pending.awaiting_task_leader {
                        Some(WhichKeyState {
                            title: "Task".into(),
                            entries: state::task_prefix_entries(),
                        })
                    } else if self.pending.awaiting_terminal_leader {
                        Some(WhichKeyState {
                            title: "Terminal".into(),
                            entries: state::terminal_prefix_entries(),
                        })
                    } else if self.pending.awaiting_test_leader {
                        Some(WhichKeyState {
                            title: "Test".into(),
                            entries: state::test_prefix_entries(),
                        })
                    } else if self.pending.awaiting_ai_leader {
                        Some(WhichKeyState {
                            title: "AI".into(),
                            entries: state::ai_prefix_entries(),
                        })
                    } else if self.pending.awaiting_package_leader {
                        Some(WhichKeyState {
                            title: "Package".into(),
                            entries: state::package_prefix_entries(),
                        })
                    } else if self.pending.awaiting_android_leader {
                        Some(WhichKeyState {
                            title: "Android".into(),
                            entries: state::android_prefix_entries(),
                        })
                    } else {
                        None
                    };
                    if let Some(p) = popup {
                        self.whichkey = Some(p);
                        needs_render = true;
                    }
                    self.leader_pressed_at = None;
                }
            }
            let (events, _more) = self.lsp.drain();
            if !events.is_empty() {
                self.handle_lsp_events(events);
                needs_render = true;
            }
            let (dap_events, dap_progress) = self.dap.drain();
            let had_dap_events = !dap_events.is_empty();
            if had_dap_events {
                self.handle_dap_events(dap_events);
            }
            // Silent protocol replies (stackTrace / scopes / variables)
            // mutate visible session state without producing a user-facing
            // event — render anyway so the pane reflects the new frames
            // and locals immediately instead of on the next keypress.
            if had_dap_events || dap_progress {
                needs_render = true;
            }
            let (test_events, test_progress) = self.test.drain();
            let had_test_events = !test_events.is_empty();
            if had_test_events {
                self.handle_test_events(test_events);
            }
            if had_test_events || test_progress {
                needs_render = true;
            }
            // Package-manager results (installed list / versions / search
            // hits / add outcome) come back on a background channel; the
            // debounce tick fires the registry search once typing settles.
            if self.handle_package_events() {
                needs_render = true;
            }
            if self.pkg_search_tick() {
                needs_render = true;
            }
            // Android SDK results (AVD list / device list / system images /
            // create outcome) come back on their own background channel.
            if self.handle_android_events() {
                needs_render = true;
            }
            // Drain PTY output → grid mutations once per loop. Any
            // bytes processed = something visible has changed. Both
            // the bottom and side panes drain every tick — background
            // tabs included, so a long-running `pnpm dev` or `claude`
            // session keeps absorbing output while focus is elsewhere.
            let (term_dirty, term_more) = self.terminal_drain_if_open();
            if term_dirty {
                needs_render = true;
            }
            pty_backlog = term_more;
            // Watch labelled (task) tabs for exit — first time we
            // notice the child is gone, scrape the visible grid +
            // scrollback for `path:line:col` errors and feed them
            // into the quickfix list. Cheap on quiet frames
            // (try_wait is non-blocking, and the per-tab `exited`
            // latch short-circuits once we've already scraped).
            if self.task_poll_exits_and_scrape() {
                needs_render = true;
            }
            // Catch any pane-geometry change (bottom terminal /
            // debug pane toggle, host resize, etc.) by reconciling
            // the side terminals' PTY size with what `buffer_rows()`
            // currently allows. Cheap no-op when nothing changed.
            self.sync_side_terminal_geometry();
            let (side_dirty, side_more) = self.side_terminal_drain_if_open();
            if side_dirty {
                needs_render = true;
            }
            pty_backlog |= side_more;
            // After draining, give any side terminal whose loading
            // splash just flipped off the chance to write its
            // pending `@<path>` prefix into the freshly-ready
            // input field. Cheap no-op when [ai] path_handoff is
            // off — the pending slot would be `None` in that case.
            self.side_terminal_flush_pending_inputs();
            // Drop the yank flash once its deadline has passed so the next
            // render paints the buffer cleanly.
            if let Some(h) = self.yank_highlight.as_ref() {
                if Instant::now() >= h.expires_at {
                    self.yank_highlight = None;
                    needs_render = true;
                }
            }
            // Notification timer bookkeeping. Sync first so any message
            // set during the iteration (save confirm, LSP error, …) gets
            // a freshly-anchored expiry; then drop the message if its
            // 10 s have elapsed.
            if self.status_msg.is_empty() {
                self.status_msg_at = None;
            } else if self.status_msg_at.is_none() {
                self.status_msg_at = Some(Instant::now());
                needs_render = true;
            }
            if let Some(at) = self.status_msg_at {
                if Instant::now() >= at + NOTIFICATION_TIMEOUT {
                    self.status_msg.clear();
                    self.status_msg_at = None;
                    needs_render = true;
                }
            }
            // Debounced LSP sync due — request a render so it fires.
            if let Some(due) = self.lsp_sync_due_at() {
                if Instant::now() >= due {
                    needs_render = true;
                }
            }
            // Health dashboard refresh cadence — flip needs_render once
            // per `HEALTH_REFRESH_INTERVAL` so the dashboard's resource
            // numbers reflect current state instead of freezing at the
            // open-time snapshot.
            if self.show_health_page
                && Instant::now() >= self.health_last_refresh + HEALTH_REFRESH_INTERVAL
            {
                needs_render = true;
                self.health_last_refresh = Instant::now();
            }
        }
        // Clean shutdown — persist the session so the next launch in this
        // cwd can restore it. Best-effort: errors don't block exit.
        // We DELETE the file only when there's truly nothing worth
        // restoring (no buffers AND no histories). Buffers-empty-but-
        // history-non-empty still saves: the `<leader>bA` flow shouldn't
        // wipe `:` / `/` recall, and `hydrate_from_session` already
        // tolerates a session whose tracked files have all been deleted.
        let session = self.build_session();
        if !session.buffers.is_empty()
            || !session.cmd_history.is_empty()
            || !session.search_history.is_empty()
            || !session.macros.is_empty()
        {
            let _ = crate::session::save(&session);
        } else {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let _ = crate::session::clear_for_cwd(&cwd);
        }
        Ok(())
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enable() -> Result<Self> {
        use crossterm::{
            event::{
                EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
                PushKeyboardEnhancementFlags,
            },
            execute,
            terminal::{EnterAlternateScreen, enable_raw_mode},
        };
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // EnableBracketedPaste makes the host terminal deliver a Cmd-V
        // paste as a single `Event::Paste(text)` instead of a flood of
        // synthetic keystrokes. Without it a multi-line paste types out
        // char-by-char and every newline submits the AI panes mid-paste.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        // Kitty keyboard protocol — terminals that support it now report
        // SUPER/META as a modifier (so Cmd-Backspace etc. arrive with the
        // right `KeyModifiers`). Non-supporting terminals silently ignore
        // the CSI sequence, so this is safe to push unconditionally.
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        );
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        use crossterm::{
            cursor::{SetCursorStyle, Show},
            event::{DisableBracketedPaste, DisableMouseCapture, PopKeyboardEnhancementFlags},
            execute,
            terminal::{LeaveAlternateScreen, disable_raw_mode},
        };
        let mut stdout = io::stdout();
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = execute!(
            stdout,
            DisableBracketedPaste,
            DisableMouseCapture,
            SetCursorStyle::DefaultUserShape,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_filter_cycles_in_order() {
        // `f` cycles forward; three presses returns to the start.
        let f = ConsoleFilter::All;
        let f = f.next();
        assert_eq!(f, ConsoleFilter::Program);
        let f = f.next();
        assert_eq!(f, ConsoleFilter::Errors);
        let f = f.next();
        assert_eq!(f, ConsoleFilter::All);
    }

    #[test]
    fn console_filter_program_hides_adapter_categories() {
        let f = ConsoleFilter::Program;
        assert!(f.allows("stdout"));
        assert!(f.allows("stderr"));
        // Adapter chatter — netcoredbg banners, dlv hints, dotnet
        // build summaries — all land in `console` and must hide
        // under the Program filter.
        assert!(!f.allows("console"));
        assert!(!f.allows("telemetry"));
        assert!(!f.allows("important"));
    }

    #[test]
    fn console_filter_errors_only_stderr() {
        let f = ConsoleFilter::Errors;
        assert!(!f.allows("stdout"));
        assert!(f.allows("stderr"));
        assert!(!f.allows("console"));
    }

    #[test]
    fn console_filter_all_passes_everything() {
        let f = ConsoleFilter::All;
        assert!(f.allows("stdout"));
        assert!(f.allows("stderr"));
        assert!(f.allows("console"));
        assert!(f.allows("telemetry"));
        // Unknown / adapter-specific categories must not be silently
        // dropped under the default filter.
        assert!(f.allows("custom-adapter-channel"));
    }

    #[test]
    fn console_filter_chip_distinguishes_active_state() {
        // The label-side chip is empty for the default so the tab
        // strip looks identical to pre-filter behaviour, but
        // populated for any active preset so the user can tell.
        assert_eq!(ConsoleFilter::All.chip(), "");
        assert_ne!(ConsoleFilter::Program.chip(), "");
        assert_ne!(ConsoleFilter::Errors.chip(), "");
    }
}
