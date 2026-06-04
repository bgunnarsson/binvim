//! Right-side terminal pane glue. Mirrors `terminal_glue.rs` for a
//! pane pinned to the right edge of the editor band — used by the
//! AI-assistant commands (`:claude`, `:codex`, `:opencode`). The
//! lifetime model differs from the bottom pane in one key way:
//! tabs here carry a stable `label` (the tool name) and re-running
//! the same command focuses the existing tab instead of spawning a
//! duplicate. The user mental model is "one Claude per project,"
//! not "one Claude per invocation."
//!
//! Both panes can be open at the same time. `App.terminal_focus`
//! tracks which one consumes keystrokes while `Mode::Terminal` is
//! active — see `app::input::handle_terminal_key` for the routing.

use std::cell::Cell;
use std::time::Instant;

use crate::mode::Mode;
use crate::terminal::Terminal;

/// One tab in the right-side terminal pane. The `label` doubles as
/// the tab header AND as the dedup key — re-running `:claude` while
/// a side terminal labelled "claude" already exists focuses that
/// tab instead of spawning another one.
pub struct SideTerminal {
    pub terminal: Terminal,
    pub label: String,
    /// When this tab's PTY was spawned. Drives the loading-splash
    /// minimum duration so fast-starting tools don't flash through
    /// the splash in a single frame.
    pub spawned_at: Instant,
    /// Most-recent byte arrival from the PTY, refreshed each drain
    /// that produced output. `Cell` so the render path (which holds
    /// `&App`) can update it while painting without an outer
    /// `&mut`. Combined with `spawned_at` it powers the
    /// "still settling?" check that gates the loading splash.
    pub last_byte_at: Cell<Instant>,
    /// Latched the first time the loading-splash decides we're past
    /// the loading window — once we've shown the real TUI we never
    /// go back to the splash, even if the tool briefly exits
    /// alt-screen or has a quiet stretch. Without this latch, every
    /// stutter in the tool's output would flash the splash on / off.
    pub loading_done: Cell<bool>,
    /// The `@<path>` prefix to write into the tool's input box once
    /// the loading splash has settled. Populated when
    /// `[ai] path_handoff = true` and the active buffer had a path
    /// at open-time. The user presses Enter manually to submit —
    /// we tried auto-submit (drip + discrete `\r`) but it never
    /// settled reliably across all three tools; each one
    /// classified the Enter slightly differently depending on
    /// timing. Pre-typing the path is the part that works
    /// universally, so we keep just that.
    pub pending_initial_input: Cell<Option<String>>,
    /// Captured the first time the loading splash flips off — the
    /// flush waits a per-tool quiet window AFTER this before
    /// writing, so the input field is fully wired up by then.
    pub loading_settled_at: Cell<Option<Instant>>,
}

/// Which terminal pane consumes keystrokes while `Mode::Terminal`
/// is active. Set whenever focus moves between panes; defaults to
/// `Bottom` so the existing `:terminal` flow behaves unchanged for
/// users who never open a side pane.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TerminalFocus {
    Bottom,
    Side,
}

/// Mouse-drag text selection scoped to a single side-terminal tab.
/// The host terminal's native Shift+drag selects across the whole
/// window with no awareness of binvim's panes — this struct + the
/// drag handling in `app/input.rs` lets the user drag inside the
/// side pane and have binvim select just within the embedded
/// terminal's grid, then copy to the system clipboard on release.
///
/// Coords are 0-based pane-local grid cells (row 0 = first row of
/// the terminal body, col 0 = first column of the grid). Selection
/// is stream-style — from `anchor` to `head` walks across rows like
/// a text-editor selection, not a rectangular block.
#[derive(Copy, Clone, Debug)]
pub struct SideSelection {
    /// The tab index this selection belongs to. The renderer + the
    /// "copy on release" path both gate on this matching the active
    /// tab so switching tabs mid-drag doesn't leak a selection into
    /// the wrong grid.
    pub tab_idx: usize,
    pub anchor: (usize, usize),
    pub head: (usize, usize),
    /// True while the left button is held down — false after release
    /// so subsequent renders still show the highlight but a fresh
    /// `Down` knows to clear-and-restart rather than extend.
    pub dragging: bool,
}

impl SideSelection {
    /// `(start, end)` in row-major order so callers can iterate
    /// without worrying which way the user dragged.
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// True when `(row, col)` falls inside the stream-style range.
    /// Matches a text editor's notion of selection: same-row → a
    /// column span; multi-row → from start to end-of-line, full
    /// middle rows, then start-of-line to end col.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (a, b) = self.ordered();
        if row < a.0 || row > b.0 {
            return false;
        }
        if a.0 == b.0 {
            col >= a.1 && col <= b.1
        } else if row == a.0 {
            col >= a.1
        } else if row == b.0 {
            col <= b.1
        } else {
            true
        }
    }
}

/// Pull text out of a grid via `visible_row` for the cells inside
/// `sel`, so selection text reflects whichever rows the renderer
/// actually painted (scrollback included) rather than always the live
/// tail. Both terminal panes can drag-select across scrolled-back
/// history, where indexing into the live cells would grab the wrong
/// text. Trailing whitespace is trimmed per row (TUIs pad rows out to
/// full width with blanks); multi-row selections join with `\n`.
pub fn extract_visible_selection_text(grid: &crate::terminal::Grid, sel: &SideSelection) -> String {
    let (start, end) = sel.ordered();
    let mut out = String::new();
    let mut first = true;
    for row in start.0..=end.0 {
        let row_cells = match grid.visible_row(row) {
            Some(r) => r,
            None => break,
        };
        let cols = row_cells.len();
        let (lo, hi) = if row == start.0 && row == end.0 {
            (start.1.min(cols), (end.1 + 1).min(cols))
        } else if row == start.0 {
            (start.1.min(cols), cols)
        } else if row == end.0 {
            (0, (end.1 + 1).min(cols))
        } else {
            (0, cols)
        };
        let line: String = row_cells[lo..hi].iter().map(|c| c.ch).collect();
        if !first {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        first = false;
    }
    out
}

/// Per-pane double-click + word-drag tracking, shared by the three
/// terminal-style panes (bottom `:terminal`, AI side pane, DAP
/// console). `last` is the time + pane-local `(row_or_line, col)` of
/// the previous left-click — a second click at the same cell within
/// `DOUBLE_CLICK_WINDOW` registers as a double-click. `word_drag`
/// holds the double-clicked word's `(row, start, end_exclusive)` so a
/// following drag grows the selection word-by-word rather than
/// char-by-char (mirrors the main editor's `word_drag_origin`).
#[derive(Default, Clone, Copy)]
pub struct PaneClickState {
    pub last: Option<(std::time::Instant, usize, usize)>,
    pub word_drag: Option<(usize, usize, usize)>,
}

/// Boundaries `(start, end_exclusive)` of the word straddling `col`
/// in `chars` — a word being a maximal run of non-whitespace. `None`
/// when `col` is past the end or sits on whitespace. Non-whitespace
/// runs (rather than the editor's alphanumeric-class words) are what
/// a terminal double-click is expected to grab: a whole path, flag,
/// or `foo.bar` token reads as one unit, which is the useful default
/// for the code / paths / logs these panes show.
pub fn word_bounds_in_line(chars: &[char], col: usize) -> Option<(usize, usize)> {
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    Some((start, end))
}

/// Inclusive `(lo, hi)` cell range a word-granular drag should cover.
/// `origin` is the double-clicked word `(row, start, end_exclusive)`;
/// `(drow, dcol)` the current drag point and `dword` the word under
/// it (`None` over whitespace). The origin word always stays
/// selected; the span grows by whole words toward the drag, anchored
/// on the far side of the origin from the drag direction — same model
/// as the editor's `word_drag_extend`, but in pane-local cell coords.
pub fn word_drag_span(
    origin: (usize, usize, usize),
    drow: usize,
    dcol: usize,
    dword: Option<(usize, usize)>,
) -> ((usize, usize), (usize, usize)) {
    let (orow, ostart, oend) = origin;
    let o_last = oend.saturating_sub(1).max(ostart);
    if (drow, dcol) < (orow, ostart) {
        // Backward drag — head snaps to the start of the drag word.
        let lo_col = dword.map(|w| w.0).unwrap_or(dcol);
        ((drow, lo_col), (orow, o_last))
    } else if (drow, dcol) > (orow, o_last) {
        // Forward drag — head snaps to the last cell of the drag word.
        let hi_col = dword.map(|w| w.1.saturating_sub(1)).unwrap_or(dcol);
        ((orow, ostart), (drow, hi_col))
    } else {
        // Still inside the origin word — hold the origin selection.
        ((orow, ostart), (orow, o_last))
    }
}

impl super::App {
    /// `(start, end_exclusive)` of the word under the active side
    /// terminal's body cell `(row, col)`, read through `visible_row`
    /// so it matches scrolled-back content. `None` over whitespace.
    pub(super) fn side_word_at(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        let t = self.active_side_terminal()?;
        let inner = t.grid();
        let cells = inner.handler.grid.visible_row(row)?;
        // '\0' is a wide glyph's trailing half — treat it as part of
        // the word (non-whitespace) so a CJK token isn't split. The
        // copied text comes from the real cells, so the placeholder
        // never leaks out.
        let chars: Vec<char> = cells
            .iter()
            .map(|c| if c.ch == '\0' { 'x' } else { c.ch })
            .collect();
        word_bounds_in_line(&chars, col)
    }

    /// Open (or focus) the right-side terminal pane and run `command`
    /// inside a freshly-spawned interactive shell tab labelled
    /// `label`. If a tab with the same label already exists we
    /// focus it instead of spawning a duplicate — re-running
    /// `:claude` is "give me the existing Claude," not "start a
    /// second one."
    ///
    /// **Why the shell wrapper.** Direct-spawning the AI tool as the
    /// PTY child gave a perfectly clean splash → UI handoff, but it
    /// skipped the user's rc files entirely. nvm-managed Node
    /// binaries (e.g. `codex`, which is a `#!/usr/bin/env node`
    /// shebang script) live on a PATH that only exists once
    /// `.zshrc` runs — without the shell, the exec fails outright
    /// with "spawn_command failed." Spawning `$SHELL` first means
    /// `.zshrc` runs, nvm injects its PATH, and the lookup succeeds.
    ///
    /// **Why `clear; exec`.** The shell prints its first prompt as
    /// soon as it's ready. We immediately write `clear; exec
    /// {command}\n` so the shell (a) wipes the prompt + echoed
    /// command line via `clear`'s `\e[2J\e[H`, and (b) replaces
    /// itself with the AI tool via `exec`. The intermediate
    /// prompt + echo never reaches the user because the loading
    /// splash sits on top of the pane until the tool is settled.
    pub(super) fn open_side_terminal(&mut self, label: &str, command: &str, with_handoff: bool) {
        // Every invocation spawns a fresh tab — `:claude` /
        // `<leader>jc` opening one tab and re-running to focus the
        // same instance was the old model. Now focus is its own
        // explicit binding (`<leader>jf`); the open bindings always
        // give you a clean session. Multiple tabs may share the
        // same label (`claude`, `claude`, `claude`); the user
        // disambiguates by tab position.
        // Flip the open flag first so side_pane_cols() returns the
        // right value when we read dimensions. Toggle back off if
        // the spawn fails AND no other side tab exists.
        let was_empty = self.side_terminals.is_empty();
        self.side_terminal_pane_open = true;
        let cols = self.side_pane_content_cols().max(8) as u16;
        let rows = (self.buffer_rows()).saturating_sub(1).max(4) as u16;
        // Launch the user's `$SHELL` as a login + interactive shell
        // so it sources `.zprofile` + `.zshrc` (and equivalents on
        // bash). That's where nvm / asdf / direnv / homebrew shims
        // live — without them, a plain `spawn(codex)` would fail to
        // resolve a `#!/usr/bin/env node` shebang script's
        // interpreter. `-c "exec {command}"` runs the launcher and
        // replaces the shell process with the AI tool, so the user
        // never sees a residual shell prompt.
        let shell = crate::terminal::default_shell();
        let launcher = format!("exec {command}");
        // Compute the `@<path> ` prefix on the spawn path only — the
        // re-focus branch above returns early so an ongoing
        // conversation never gets `@path` re-stuffed into it. Honour
        // [ai] path_handoff (default off); when on, anchor the path
        // on the cwd so generated `@src/foo.rs` references resolve in
        // the tool's eye against the same root binvim is editing
        // from. Falls back gracefully when the active buffer has no
        // path or the strip-prefix doesn't apply.
        let pending_input = if with_handoff {
            self.ai_path_handoff_prefix()
        } else {
            None
        };
        match Terminal::spawn_program(rows, cols, &shell, &["-l", "-i", "-c", &launcher]) {
            Ok(t) => {
                let now = Instant::now();
                self.side_terminals.push(SideTerminal {
                    terminal: t,
                    label: label.to_string(),
                    spawned_at: now,
                    last_byte_at: Cell::new(now),
                    loading_done: Cell::new(false),
                    pending_initial_input: Cell::new(pending_input),
                    loading_settled_at: Cell::new(None),
                });
                self.active_side_terminal_idx = self.side_terminals.len() - 1;
                self.terminal_focus = TerminalFocus::Side;
                self.mode = Mode::Terminal;
                self.adjust_viewport();
                self.status_msg.clear();
            }
            Err(e) => {
                if was_empty {
                    self.side_terminal_pane_open = false;
                }
                self.status_msg = format!("{label}: spawn failed: {e:#}");
            }
        }
    }

    /// Build the `@<rel-path>` payload to write into a freshly-
    /// spawned side terminal, or `None` when handoff is disabled,
    /// the active buffer has no path, or the path can't be
    /// project-relativised. Project-relative anchoring is by cwd
    /// (matches what the tools expect for their `@<path>`
    /// expansion); when the path lies outside cwd we fall through
    /// to the absolute form because that still resolves.
    ///
    /// No trailing newline / Enter — the user submits manually.
    /// Auto-submit was attempted and abandoned: each of the three
    /// tools classified our programmatic `\r` differently
    /// depending on timing, and no single tuning made all three
    /// submit reliably. Pre-typing the path is the part that
    /// works universally, so we keep just that.
    fn ai_path_handoff_prefix(&self) -> Option<String> {
        let path = self.buffer.path.as_ref()?;
        let cwd = std::env::current_dir().ok();
        let display = match cwd.as_ref().and_then(|c| path.strip_prefix(c).ok()) {
            Some(rel) => rel.display().to_string(),
            None => path.display().to_string(),
        };
        if display.is_empty() {
            return None;
        }
        Some(format!("@{display}"))
    }

    /// Per-frame flush — once the loading splash settles AND the
    /// per-tool quiet window has elapsed (so the input field is
    /// fully wired up), write the pending `@<path>` prefix into
    /// the tool's input box as a single chunk and clear the slot.
    /// The user then presses Enter to submit.
    ///
    /// We tried auto-submit (drip the path at typing cadence,
    /// follow with a discrete `\r`) and could not find a single
    /// timing that submitted reliably across Claude / Codex /
    /// opencode — each tool classified the trailing Enter
    /// differently depending on context (autocomplete capture,
    /// debounce window, paste-mode newline). Pre-typing the path
    /// is the part that works universally, so that's what we
    /// keep. Two-keypress flow (`:claude` → Enter) instead of
    /// one, but reliable on all three tools.
    pub(super) fn side_terminal_flush_pending_inputs(&self) {
        for s in &self.side_terminals {
            if side_terminal_loading(s) {
                continue;
            }
            // Anchor the wait window on splash-exit, not on spawn —
            // splash duration varies per tool, see `side_terminal_loading`.
            let settled_at = match s.loading_settled_at.get() {
                Some(t) => t,
                None => {
                    let now = Instant::now();
                    s.loading_settled_at.set(Some(now));
                    now
                }
            };
            let now = Instant::now();
            let since_settled = now.duration_since(settled_at);
            let since_byte = now.duration_since(s.last_byte_at.get());
            let (quiet_guard, max_wait) = handoff_tuning(&s.label);
            let ready = since_byte >= quiet_guard || since_settled >= max_wait;
            if !ready {
                continue;
            }
            // Atomic write of the whole `@<path>` payload. The
            // per-tool quiet guard already ensured the input field
            // is ready, so front-of-path truncation isn't a risk
            // here the way it was when we wrote at the splash
            // boundary.
            if let Some(prefix) = s.pending_initial_input.take() {
                let _ = s.terminal.write_bytes(prefix.as_bytes());
            }
        }
    }

    /// `<leader>jf` — drop focus into the side-pane and start
    /// routing keystrokes there. No-op (with a hint) when no side
    /// tab exists; mirrors `Action::TerminalFocus` for the bottom
    /// pane.
    pub(super) fn focus_side_terminal(&mut self) {
        if self.side_terminals.is_empty() {
            self.status_msg = "ai: no side pane (open with `<leader>jc` / `jx` / `jo`)".into();
            return;
        }
        self.side_terminal_pane_open = true;
        self.terminal_focus = TerminalFocus::Side;
        self.mode = Mode::Terminal;
        self.resize_all_side_terminals();
        self.adjust_viewport();
    }

    /// `<leader>jp` — hide / show the side pane without killing
    /// the PTYs. Mirrors `<leader>tp` for the bottom pane:
    /// - Visible → hide, drop focus to Normal. PTYs keep draining.
    /// - Hidden + tabs alive → show, refocus into `Mode::Terminal`.
    /// - No tabs → status hint (use `<leader>j{c,x,o}` to open).
    pub(super) fn toggle_side_terminal_pane(&mut self) {
        if self.side_terminal_pane_open {
            self.side_terminal_pane_open = false;
            if matches!(self.mode, Mode::Terminal)
                && matches!(self.terminal_focus, TerminalFocus::Side)
            {
                self.mode = Mode::Normal;
                self.terminal_focus = TerminalFocus::Bottom;
            }
            self.adjust_viewport();
            return;
        }
        if self.side_terminals.is_empty() {
            self.status_msg = "ai: no side pane (open with `<leader>jc` / `jx` / `jo`)".into();
            return;
        }
        self.side_terminal_pane_open = true;
        self.terminal_focus = TerminalFocus::Side;
        self.mode = Mode::Terminal;
        self.resize_all_side_terminals();
        self.adjust_viewport();
    }

    /// Active side-terminal handle, if any.
    pub fn active_side_terminal(&self) -> Option<&Terminal> {
        self.side_terminals
            .get(self.active_side_terminal_idx)
            .map(|s| &s.terminal)
    }

    /// `<leader>jq` (or `:q` while focused) — drop the active
    /// side-terminal tab. If it was the last one, hide the pane and
    /// snap focus back to the bottom pane so future `Mode::Terminal`
    /// keystrokes don't get routed into the void.
    pub(super) fn close_side_terminal(&mut self) {
        if self.side_terminals.is_empty() {
            return;
        }
        let idx = self
            .active_side_terminal_idx
            .min(self.side_terminals.len() - 1);
        self.side_terminals.remove(idx);
        // Drop any drag selection — the grid it was anchored to is
        // either gone (last tab closed) or now belongs to a different
        // tab (index shift), so the coords can't be trusted.
        self.side_terminal_selection = None;
        if self.side_terminals.is_empty() {
            self.active_side_terminal_idx = 0;
            self.side_terminal_pane_open = false;
            self.terminal_focus = TerminalFocus::Bottom;
            if matches!(self.mode, Mode::Terminal) {
                self.mode = Mode::Normal;
            }
        } else if self.active_side_terminal_idx >= self.side_terminals.len() {
            self.active_side_terminal_idx = self.side_terminals.len() - 1;
        }
        self.adjust_viewport();
    }

    /// Push the current pane body geometry to every side terminal's
    /// PTY (SIGWINCH). Same rationale as `resize_all_terminals`.
    /// Uses `side_pane_content_cols()` so the reserved border column
    /// isn't double-counted into the child's reported width.
    pub(super) fn resize_all_side_terminals(&mut self) {
        if self.side_terminals.is_empty() {
            return;
        }
        let cols = (self.side_pane_content_cols()).max(8) as u16;
        let rows = (self.buffer_rows()).saturating_sub(1).max(4) as u16;
        for s in &self.side_terminals {
            let _ = s.terminal.resize(rows, cols);
        }
        // Grid coords just shifted under any in-flight drag selection;
        // drop it rather than partially-highlight cells that no
        // longer exist where the user clicked.
        self.side_terminal_selection = None;
    }

    /// Drain pending PTY output into every side terminal's grid.
    /// Returns `(dirty, more)`: `dirty` is true if any bytes were
    /// processed so the caller can mark the frame dirty; `more` is true
    /// if any tab hit its per-tick drain budget with output still
    /// queued, so the caller can keep spinning to catch up. Background
    /// tabs drain too. Each tab stamps `last_byte_at` whenever its
    /// drain produces output, so the render-side loading-splash check
    /// can ask "have we been quiet long enough to drop the splash?"
    pub(super) fn side_terminal_drain_if_open(&self) -> (bool, bool) {
        let mut any = false;
        let mut more = false;
        let now = Instant::now();
        for s in &self.side_terminals {
            let (bytes, term_more) = s.terminal.drain();
            if bytes > 0 {
                s.last_byte_at.set(now);
                any = true;
            }
            more |= term_more;
        }
        (any, more)
    }

    /// Make sure every side terminal's PTY grid matches the rows /
    /// cols the pane currently has room for. Called once per main-
    /// loop tick. Cheaper than wiring `resize_all_side_terminals()`
    /// into every pane-toggle site (bottom terminal open/close,
    /// debug pane open/close, host resize, layout split changes
    /// that touch the bottom strip…) — and idempotent, so the
    /// per-tick check just no-ops when nothing changed. Without
    /// this, opening the bottom `:terminal` after a side tab is
    /// already up leaves the side tab's PTY at its old (taller)
    /// dimensions and the embedded tool keeps painting past where
    /// we draw it.
    pub(super) fn sync_side_terminal_geometry(&self) {
        if self.side_terminals.is_empty() {
            return;
        }
        let want_cols = (self.side_pane_content_cols()).max(8) as u16;
        let want_rows = (self.buffer_rows()).saturating_sub(1).max(4) as u16;
        for s in &self.side_terminals {
            let grid = s.terminal.grid();
            let cur_rows = grid.handler.grid.rows as u16;
            let cur_cols = grid.handler.grid.cols as u16;
            drop(grid);
            if cur_rows != want_rows || cur_cols != want_cols {
                let _ = s.terminal.resize(want_rows, want_cols);
            }
        }
    }
}

/// True while the side terminal `s` is still settling — show the
/// binvim loading splash instead of the partially-rendered TUI
/// frame. The gate is monotonic: once it flips off it stays off,
/// so a chatty tool whose output comes in bursts (claude / opencode
/// / codex all do) can't flash the splash back on at every quiet
/// gap.
///
/// Three signals collaborate to decide when to drop the splash:
///
///   - **Alt-screen entry.** True TUIs (vim, htop, claude,
///     opencode) emit `\e[?1049h` as the first thing they do to
///     declare "I'm taking over the screen now." The moment we see
///     it we know the next frame is the real UI.
///   - **Idle quiet.** Tools that don't use alt-screen — codex
///     prints its welcome and waits at a prompt without ever
///     entering alt-screen — settle when their output goes quiet.
///     We wait until the PTY has been silent for `IDLE_QUIET` AND
///     at least `IDLE_MIN_SPAWN` has elapsed (the floor stops a
///     tool that's instantly idle from popping straight through).
///   - **Hard cap.** Last-resort fallback for tools that neither
///     enter alt-screen nor ever go quiet — drop the splash so the
///     user isn't staring at the loader forever.
///
/// A `MIN_SPLASH` floor under all three keeps a fast-starting tool
/// Per-tool tuning for the path-handoff drip — returns
/// `(output_quiet_guard, max_post_splash_wait)`.
///
/// Each TUI we target boots and accepts input on its own timeline.
/// A single shared tuning makes one tool work while another loses
/// bytes or fails to submit, so the dispatch is tool-aware via the
/// tab label (which doubles as a stable identity — `:claude` tabs
/// are always labelled `"claude"`, etc.).
///
/// Empirical values, gathered by iterating with the three tools:
/// - **Claude** boots fast — alt-screen + a render of the welcome
///   pane, then idle. 300ms quiet is enough; longer waits seem to
///   put Claude into a state where the trailing `\r` no longer
///   registers as submit.
/// - **Codex** takes a bit longer to wire up its input field after
///   the splash render. 800ms quiet catches it cleanly.
/// - **opencode** is the slowest — its TUI does a lot of background
///   initialisation after rendering the splash, and we've seen
///   front-of-path truncation as late as ~700ms in. 1500ms quiet
///   gives a comfortable safety margin.
///
/// `max_post_splash_wait` is the fallback for tools with periodic
/// PTY redraws that keep `last_byte_at` updating (cursor blink,
/// status-line clocks) and would otherwise prevent the quiet guard
/// from ever tripping. Set ~1s above the quiet guard.
fn handoff_tuning(label: &str) -> (std::time::Duration, std::time::Duration) {
    use std::time::Duration;
    match label {
        "claude" => (Duration::from_millis(300), Duration::from_millis(1500)),
        "codex" => (Duration::from_millis(800), Duration::from_millis(2500)),
        "opencode" => (Duration::from_millis(1500), Duration::from_millis(3500)),
        // Unknown tool — split the difference.
        _ => (Duration::from_millis(800), Duration::from_millis(2500)),
    }
}

/// from popping through in one frame, which would itself look like
/// a flash.
pub fn side_terminal_loading(s: &SideTerminal) -> bool {
    use std::time::Duration;
    const MIN_SPLASH: Duration = Duration::from_millis(200);
    const IDLE_MIN_SPAWN: Duration = Duration::from_millis(350);
    const IDLE_QUIET: Duration = Duration::from_millis(200);
    const HARD_CAP: Duration = Duration::from_millis(3000);
    if s.loading_done.get() {
        return false;
    }
    let now = Instant::now();
    let since_spawn = now.duration_since(s.spawned_at);
    if since_spawn < MIN_SPLASH {
        return true;
    }
    let alt = s.terminal.alt_screen_active();
    let since_byte = now.duration_since(s.last_byte_at.get());
    let idle_settled = since_spawn >= IDLE_MIN_SPAWN && since_byte >= IDLE_QUIET;
    if alt || idle_settled || since_spawn >= HARD_CAP {
        s.loading_done.set(true);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{Cell, Grid};

    /// Build a `Grid` whose rows are filled left-aligned with the
    /// glyphs of each `&str`, padded with blanks out to `cols`. Mirrors
    /// what the vte handler would produce for a chunk of plain ASCII.
    fn grid_from_lines(lines: &[&str], cols: usize) -> Grid {
        let mut g = Grid::new(lines.len(), cols);
        for (r, line) in lines.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                if c >= cols {
                    break;
                }
                g.cells[r][c] = Cell {
                    ch,
                    ..Cell::default()
                };
            }
        }
        g
    }

    #[test]
    fn extract_visible_selection_text_single_row_trims_trailing_pad() {
        // "hello" on a 10-cell row — selection covers the whole row.
        // The 5 trailing blank cells should be trimmed.
        let g = grid_from_lines(&["hello"], 10);
        let sel = SideSelection {
            tab_idx: 0,
            anchor: (0, 0),
            head: (0, 9),
            dragging: false,
        };
        assert_eq!(extract_visible_selection_text(&g, &sel), "hello");
    }

    #[test]
    fn extract_visible_selection_text_multi_row_joins_with_newlines() {
        let g = grid_from_lines(&["abc", "def", "ghi"], 5);
        // Stream-style selection from (0, 1) to (2, 1) → "bc\ndef\ngh".
        let sel = SideSelection {
            tab_idx: 0,
            anchor: (0, 1),
            head: (2, 1),
            dragging: false,
        };
        assert_eq!(extract_visible_selection_text(&g, &sel), "bc\ndef\ngh");
    }

    #[test]
    fn extract_visible_selection_text_walks_scrollback_when_view_scrolled() {
        // Push three lines into scrollback (a, b, c), keep "d" in the
        // live grid. Then scroll the view all the way back. A
        // selection covering rows 0..2 of the visible window should
        // pull "a", "b", "c" from scrollback — the live "d" must NOT
        // sneak in.
        let mut g = grid_from_lines(&["d"], 5);
        g.scrollback
            .push(grid_from_lines(&["a"], 5).cells.remove(0));
        g.scrollback
            .push(grid_from_lines(&["b"], 5).cells.remove(0));
        g.scrollback
            .push(grid_from_lines(&["c"], 5).cells.remove(0));
        g.scroll_view_by(3);
        let sel = SideSelection {
            tab_idx: 0,
            anchor: (0, 0),
            head: (2, 4),
            dragging: false,
        };
        assert_eq!(extract_visible_selection_text(&g, &sel), "a\nb\nc");
    }

    #[test]
    fn word_bounds_picks_non_whitespace_run() {
        let chars: Vec<char> = "foo  bar.baz quux".chars().collect();
        // Inside "foo".
        assert_eq!(word_bounds_in_line(&chars, 1), Some((0, 3)));
        // "bar.baz" is one token — punctuation doesn't split it.
        assert_eq!(word_bounds_in_line(&chars, 9), Some((5, 12)));
        // On whitespace → no word.
        assert_eq!(word_bounds_in_line(&chars, 3), None);
        // Past the end → no word.
        assert_eq!(word_bounds_in_line(&chars, 99), None);
    }

    #[test]
    fn word_drag_span_grows_by_whole_words() {
        // Origin word "bar" at row 0, cols [4, 7). Drag forward into
        // "baz" (cols [8, 11)) → span covers bar..baz inclusive.
        let origin = (0, 4, 7);
        let (lo, hi) = word_drag_span(origin, 0, 9, Some((8, 11)));
        assert_eq!((lo, hi), ((0, 4), (0, 10)));
        // Drag backward into "foo" (cols [0, 3)) → anchor pins to the
        // end of the origin word, head snaps to the drag word's start.
        let (lo, hi) = word_drag_span(origin, 0, 1, Some((0, 3)));
        assert_eq!((lo, hi), ((0, 0), (0, 6)));
        // Drag still inside the origin word → origin selection held.
        let (lo, hi) = word_drag_span(origin, 0, 5, Some((4, 7)));
        assert_eq!((lo, hi), ((0, 4), (0, 6)));
        // Forward drag over whitespace (no word) → head follows the
        // raw drag column so the span doesn't snap back.
        let (lo, hi) = word_drag_span(origin, 1, 2, None);
        assert_eq!((lo, hi), ((0, 4), (1, 2)));
    }
}
