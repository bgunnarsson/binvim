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

mod buffers;
mod cmdline_history;
mod comment;
mod copilot;
mod dap_glue;
mod dispatch;
mod edit;
mod git_glue;
mod health;
mod input;
mod lsp_glue;
mod multi_cursor;
mod pair;
mod picker_glue;
mod quickfix;
mod registers;
mod save;
mod search;
pub(crate) mod state;
mod terminal_glue;
mod view;
mod visual;
mod windows;

pub use health::{DiagnosticsCounts, HealthSnapshot};
pub use state::{
    BufferStash, CompletionState, FindRecord, FoldRange, HoverCodeBlock, HoverLine, HoverState,
    LastEdit, Register, WhichKeyState, YankHighlight, HOVER_MAX_HEIGHT,
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
use crate::lsp::{CodeActionItem, InlayHint, LspManager, SignatureHelp};
use crate::mode::Mode;
use crate::parser::PendingCmd;
use crate::picker::PickerState;
use crate::render;
use crate::undo::History;

use state::RecordingState;

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
    /// Index into the flat locals tree of the currently-selected row when
    /// `Mode::DebugPane` has focus. Bounded against the live tree at
    /// access time — vrefs can shift across stops, so the renderer and
    /// key handler clamp before use.
    pub dap_pane_cursor: usize,
    /// Top of the left column's viewport (frames + separator + locals).
    /// Driven by `j`/`k` (auto-follow the selection) and `Ctrl-Y`/`Ctrl-E`
    /// (free scroll without moving selection).
    pub dap_left_scroll: usize,
    /// Number of "latest" console-output rows hidden below the right
    /// column's viewport. `0` keeps the latest line glued to the bottom;
    /// `J`/`K` scrolls into the older history.
    pub dap_right_scroll: usize,
    /// Active yank flash, if any. Drained automatically by the main loop
    /// once its `expires_at` deadline passes.
    pub yank_highlight: Option<YankHighlight>,
    /// Pending code actions waiting for the user to pick from the picker.
    /// Indexed by `PickerPayload::CodeActionIdx`.
    pub pending_code_actions: Vec<CodeActionItem>,
    /// Debug-launch state captured between the project picker and the
    /// profile picker. When `launchSettings.json` for the chosen project
    /// has more than one runnable profile, we stash the project + the
    /// parsed profile list here and open the profile picker; on accept
    /// the index in `PickerPayload::DebugProfile` selects which profile
    /// goes into the LaunchContext.
    pub pending_debug_project: Option<std::path::PathBuf>,
    pub pending_debug_profiles: Vec<crate::dap::LaunchProfile>,
    /// Symbol position captured when a rename prompt opened — the LSP
    /// rename request needs the original `(line, col)` even after the
    /// prompt has stolen focus and the user has moved focus around.
    pub rename_anchor: Option<(PathBuf, usize, usize, String)>,
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
    /// Active `:terminal` pane, if any. The PTY child + grid live
    /// inside this `Terminal`; the pane renders at the bottom of
    /// the editor area when `terminal_pane_open` is true. None
    /// when the terminal has been closed (`:q` in
    /// `Mode::TerminalNormal`) — opening again via `:terminal`
    /// re-spawns a fresh shell.
    pub terminal: Option<crate::terminal::Terminal>,
    pub terminal_pane_open: bool,
    /// View offset into the terminal grid's scrollback when the
    /// user is reading back history. 0 = bottom of scrollback (the
    /// live grid is fully visible); larger values shift the view
    /// up into older rows. Driven by `Ctrl-Y` / `Ctrl-E` in
    /// `Mode::TerminalNormal`.
    pub terminal_scroll: usize,
    /// Vim-style selection inside the terminal grid. When Some,
    /// the user has entered `v` in `Mode::TerminalNormal` and the
    /// anchor + `terminal_cursor` define a region to be yanked
    /// with `y`. Coordinates are (row, col) in the visible grid.
    pub terminal_visual_anchor: Option<(usize, usize)>,
    /// Reading-cursor position in `Mode::TerminalNormal`. Driven
    /// by `h/j/k/l`/word motions; rendered as an inverted block.
    pub terminal_cursor: (usize, usize),
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
/// The anchor (cursor position + buffer version when the request fired)
/// gates rendering — the highlights only paint while the live cursor
/// + buffer version still match, so a stale response doesn't smear
/// yesterday's highlights over today's code.
#[derive(Debug, Clone)]
pub struct DocumentHighlightCache {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub anchor_version: u64,
    pub ranges: Vec<crate::lsp::DocumentHighlightRange>,
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
            cmd_history: Vec::new(),
            search_history: Vec::new(),
            history_cursor: None,
            history_draft: None,
            status_msg: String::new(),
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
            blame: Vec::new(),
            show_start_page,
            show_health_page: false,
            health_last_refresh: Instant::now(),
            health_scroll: 0,
            health_content_height: std::cell::Cell::new(0),
            debug_pane_open: false,
            dap_pane_cursor: 0,
            dap_left_scroll: 0,
            dap_right_scroll: 0,
            yank_highlight: None,
            pending_code_actions: Vec::new(),
            pending_debug_project: None,
            pending_debug_profiles: Vec::new(),
            rename_anchor: None,
            folds: Vec::new(),
            folds_version: u64::MAX,
            closed_folds: std::collections::HashSet::new(),
            recents: buffers::load_recents(),
            inlay_hints: HashMap::new(),
            last_inlay_request_version: HashMap::new(),
            last_disk_check: Instant::now(),
            last_click: None,
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
            terminal: None,
            terminal_pane_open: false,
            terminal_scroll: 0,
            terminal_visual_anchor: None,
            terminal_cursor: (0, 0),
            document_highlights: HashMap::new(),
            document_highlight_in_flight: std::collections::HashSet::new(),
            inlay_hints_in_flight: std::collections::HashSet::new(),
            semantic_tokens_in_flight: std::collections::HashSet::new(),
            lsp_messages: Vec::new(),
            show_messages_page: false,
            messages_scroll: 0,
            messages_content_height: std::cell::Cell::new(0),
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
            if restore_buffers {
                this.hydrate_from_session(s);
            }
        }
        Ok(this)
    }

    pub fn run(&mut self) -> Result<()> {
        let _guard = TerminalGuard::enable()?;
        let mut stdout = io::stdout();
        self.lsp_attach_active();
        self.refresh_editorconfig();
        self.refresh_git_hunks();
        let mut needs_render = true;
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
            if self.terminal.is_some() {
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
            if crossterm::event::poll(poll_dur)? {
                self.handle_event()?;
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
                    } else if self.pending.awaiting_terminal_leader {
                        Some(WhichKeyState {
                            title: "Terminal".into(),
                            entries: state::terminal_prefix_entries(),
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
            // Drain PTY output → grid mutations once per loop. Any
            // bytes processed = something visible has changed.
            if self.terminal_drain_if_open() {
                needs_render = true;
            }
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
                EnableMouseCapture, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
            },
            execute,
            terminal::{enable_raw_mode, EnterAlternateScreen},
        };
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
            event::{DisableMouseCapture, PopKeyboardEnhancementFlags},
            execute,
            terminal::{disable_raw_mode, LeaveAlternateScreen},
        };
        let mut stdout = io::stdout();
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = execute!(
            stdout,
            DisableMouseCapture,
            SetCursorStyle::DefaultUserShape,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
