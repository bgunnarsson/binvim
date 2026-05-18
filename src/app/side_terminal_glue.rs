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
    /// Bytes to write to the PTY once the tool's input field is
    /// ready. Populated when `[ai] path_handoff = true` and the
    /// active buffer had a path at open-time; the actual write
    /// happens on the first frame `loading_done` flips true, so the
    /// `@<path>` text lands in the tool's input box (post-splash)
    /// rather than getting eaten by startup chatter (pre-splash).
    /// `Cell` so the per-frame writer can clear it from a `&App`
    /// render path without an outer `&mut`.
    pub pending_initial_input: Cell<Option<String>>,
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

impl super::App {
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
    pub(super) fn open_side_terminal(&mut self, label: &str, command: &str) {
        // Re-focus an existing tab with the same label.
        if let Some(idx) = self.side_terminals.iter().position(|t| t.label == label) {
            self.side_terminal_pane_open = true;
            self.active_side_terminal_idx = idx;
            self.terminal_focus = TerminalFocus::Side;
            self.mode = Mode::Terminal;
            self.resize_all_side_terminals();
            self.adjust_viewport();
            return;
        }
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
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let launcher = format!("exec {command}");
        // Compute the `@<path> ` prefix on the spawn path only — the
        // re-focus branch above returns early so an ongoing
        // conversation never gets `@path` re-stuffed into it. Honour
        // [ai] path_handoff (default off); when on, anchor the path
        // on the cwd so generated `@src/foo.rs` references resolve in
        // the tool's eye against the same root binvim is editing
        // from. Falls back gracefully when the active buffer has no
        // path or the strip-prefix doesn't apply.
        let pending_input = self.ai_path_handoff_prefix();
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

    /// Build the `@<rel-path> ` string to inject into a freshly-
    /// spawned side terminal, or `None` when handoff is disabled,
    /// the active buffer has no path, or the path can't be
    /// project-relativised. Project-relative anchoring is by cwd
    /// (matches what the tools expect for their `@<path>`
    /// expansion); when the path lies outside cwd we fall through
    /// to the absolute form because that still resolves.
    fn ai_path_handoff_prefix(&self) -> Option<String> {
        if !self.config.ai.path_handoff {
            return None;
        }
        let path = self.buffer.path.as_ref()?;
        let cwd = std::env::current_dir().ok();
        let display = match cwd.as_ref().and_then(|c| path.strip_prefix(c).ok()) {
            Some(rel) => rel.display().to_string(),
            None => path.display().to_string(),
        };
        if display.is_empty() {
            return None;
        }
        Some(format!("@{display} "))
    }

    /// Per-frame flush: if any side terminal has finished its
    /// loading splash AND still carries a `pending_initial_input`,
    /// write the prefix to the PTY and clear the slot. Called from
    /// the main loop after `side_terminal_drain_if_open` so the
    /// write happens on the same tick the splash flips off (the
    /// tool's input field is ready by then, so the bytes land in
    /// the prompt rather than getting eaten by startup chatter).
    pub(super) fn side_terminal_flush_pending_inputs(&self) {
        for s in &self.side_terminals {
            if side_terminal_loading(s) {
                continue;
            }
            let Some(prefix) = s.pending_initial_input.take() else {
                continue;
            };
            let _ = s.terminal.write_bytes(prefix.as_bytes());
        }
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
    pub(super) fn resize_all_side_terminals(&self) {
        if self.side_terminals.is_empty() {
            return;
        }
        let cols = (self.side_pane_content_cols()).max(8) as u16;
        let rows = (self.buffer_rows()).saturating_sub(1).max(4) as u16;
        for s in &self.side_terminals {
            let _ = s.terminal.resize(rows, cols);
        }
    }

    /// Drain pending PTY output into every side terminal's grid.
    /// Returns `true` if any bytes were processed so the caller can
    /// mark the frame dirty. Background tabs drain too. Each tab
    /// stamps `last_byte_at` whenever its drain produces output, so
    /// the render-side loading-splash check can ask "have we been
    /// quiet long enough to drop the splash?"
    pub(super) fn side_terminal_drain_if_open(&self) -> bool {
        let mut any = false;
        let now = Instant::now();
        for s in &self.side_terminals {
            if s.terminal.drain() > 0 {
                s.last_byte_at.set(now);
                any = true;
            }
        }
        any
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
