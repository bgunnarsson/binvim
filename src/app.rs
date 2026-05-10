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
mod dispatch;
mod edit;
mod health;
mod input;
mod lsp_glue;
mod pair;
mod picker_glue;
mod registers;
mod save;
mod search;
mod state;
mod view;
mod visual;

pub use state::{
    BufferStash, CompletionState, FindRecord, FoldRange, HoverCodeBlock, HoverLine, HoverState,
    LastEdit, Register, WhichKeyState, YankHighlight, HOVER_MAX_HEIGHT,
};

use state::WHICHKEY_DELAY;

use anyhow::Result;
use crossterm::event::KeyEvent;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::config::Config;
use crate::cursor::Cursor;
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
    pub cursor: Cursor,
    pub mode: Mode,
    pub pending: PendingCmd,
    pub history: History,
    pub registers: HashMap<char, Register>,
    pub cmdline: String,
    pub status_msg: String,
    pub view_top: usize,
    /// Visual columns hidden off the left edge of the buffer area.
    pub view_left: usize,
    pub width: u16,
    pub height: u16,
    pub should_quit: bool,
    pub visual_anchor: Option<Cursor>,
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
    pub active: usize,
    pub highlight_cache: Option<HighlightCache>,
    pub picker: Option<PickerState>,
    pub config: Config,
    pub editorconfig: EditorConfig,
    pub lsp: LspManager,
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
    /// True when binvim was launched with no path — render the start page in
    /// place of the empty buffer until the user opens something.
    pub show_start_page: bool,
    /// Active yank flash, if any. Drained automatically by the main loop
    /// once its `expires_at` deadline passes.
    pub yank_highlight: Option<YankHighlight>,
    /// Pending code actions waiting for the user to pick from the picker.
    /// Indexed by `PickerPayload::CodeActionIdx`.
    pub pending_code_actions: Vec<CodeActionItem>,
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
    /// Char-index positions of secondary cursors that mirror Insert-mode
    /// edits at the primary cursor. Empty when only one cursor is active.
    /// Populated by `Ctrl-click` in Insert mode; cleared on Esc.
    pub additional_cursors: Vec<usize>,
    pub(crate) replaying_macro: bool,
    pub(crate) recording: Option<RecordingState>,
    pub(crate) replaying: bool,
}

/// How quickly a second left-click must arrive at the same buffer position
/// to register as a double-click. Tuned to typical OS double-click defaults.
pub const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);

impl App {
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        // No path arg + a saved session for this cwd → restore the session
        // instead of opening the empty start page. The session module does
        // its own existence checks for each tracked buffer.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let restored_session = if path.is_none() {
            crate::session::load_for_cwd(&cwd)
        } else {
            None
        };
        let show_start_page = path.is_none() && restored_session.is_none();
        let buffer = match path.as_ref() {
            Some(p) => Buffer::from_path(p.clone())?,
            None => Buffer::empty(),
        };
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let mut this = Self {
            buffer,
            cursor: Cursor::default(),
            mode: Mode::Normal,
            pending: PendingCmd::default(),
            history: History::new(),
            registers: HashMap::new(),
            cmdline: String::new(),
            status_msg: String::new(),
            view_top: 0,
            view_left: 0,
            width: w,
            height: h,
            should_quit: false,
            visual_anchor: None,
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
            highlight_cache: None,
            picker: None,
            config: Config::load(),
            editorconfig: EditorConfig::default(),
            lsp: LspManager::new(),
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
            show_start_page,
            yank_highlight: None,
            pending_code_actions: Vec::new(),
            rename_anchor: None,
            folds: Vec::new(),
            folds_version: u64::MAX,
            closed_folds: std::collections::HashSet::new(),
            recents: buffers::load_recents(),
            inlay_hints: HashMap::new(),
            last_inlay_request_version: HashMap::new(),
            last_disk_check: Instant::now(),
            last_click: None,
            additional_cursors: Vec::new(),
            replaying_macro: false,
            recording: None,
            replaying: false,
        };
        // Hydrate from the saved session — open every still-extant buffer,
        // restore each one's cursor + viewport, and land on the previously
        // active buffer.
        if let Some(s) = restored_session {
            this.hydrate_from_session(s);
        }
        Ok(this)
    }

    pub fn run(&mut self) -> Result<()> {
        let _guard = TerminalGuard::enable()?;
        let mut stdout = io::stdout();
        self.lsp_attach_active();
        self.refresh_editorconfig();
        let mut needs_render = true;
        while !self.should_quit {
            if needs_render {
                self.maybe_reload_from_disk();
                self.adjust_viewport();
                self.ensure_highlights();
                self.ensure_folds();
                self.lsp_sync_active_debounced();
                self.lsp_request_inlay_hints_if_due();
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
            // Drop the yank flash once its deadline has passed so the next
            // render paints the buffer cleanly.
            if let Some(h) = self.yank_highlight.as_ref() {
                if Instant::now() >= h.expires_at {
                    self.yank_highlight = None;
                    needs_render = true;
                }
            }
            // Debounced LSP sync due — request a render so it fires.
            if let Some(due) = self.lsp_sync_due_at() {
                if Instant::now() >= due {
                    needs_render = true;
                }
            }
        }
        // Clean shutdown — persist the session so the next launch in this
        // cwd can restore it. Best-effort: errors don't block exit.
        let session = self.build_session();
        if !session.buffers.is_empty() {
            let _ = crate::session::save(&session);
        }
        Ok(())
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enable() -> Result<Self> {
        use crossterm::{
            event::EnableMouseCapture,
            execute,
            terminal::{enable_raw_mode, EnterAlternateScreen},
        };
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        use crossterm::{
            cursor::{SetCursorStyle, Show},
            event::DisableMouseCapture,
            execute,
            terminal::{disable_raw_mode, LeaveAlternateScreen},
        };
        let mut stdout = io::stdout();
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
