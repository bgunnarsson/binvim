//! `:lazygit` / `<leader>gg` — full-screen lazygit takeover.
//!
//! Yazi-style suspend: we release the host terminal entirely (pop the
//! kitty keyboard protocol, disable mouse capture, leave the alt
//! screen, drop raw mode), spawn `lazygit` as a foreground child with
//! stdio inherited, and block until it exits. Then we reclaim the
//! terminal (re-enable raw mode, re-enter alt screen, re-arm mouse
//! capture + keyboard enhancement flags) and refresh the working-tree
//! diff for every open buffer so staged / committed / checked-out
//! changes show up in the gutter immediately.
//!
//! Deliberately not a PTY-embedded pane. Lazygit wants the whole
//! screen (its UI hard-codes panel widths against terminal cols, and
//! the bottom `:terminal` pane caps out at 20 rows) and the takeover
//! model gives clean exit detection for free — when the blocking
//! `status()` call returns, lazygit is done. No try_wait polling, no
//! tab management, no SIGWINCH plumbing.

use std::io;
use std::path::PathBuf;
use std::process::Command;

impl super::App {
    /// `:lazygit` / `<leader>gg` entry point. Suspends the editor,
    /// runs lazygit, and refreshes git gutters on exit. No-op (with a
    /// status-line hint) if the binary isn't on PATH.
    pub(super) fn cmd_lazygit(&mut self) {
        use crossterm::{
            cursor::{Hide, Show},
            event::{
                DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
                PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
            },
            execute,
            terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
        };

        // lazygit walks up from its own cwd to find the .git root, so
        // running from the editor's cwd is sufficient. Prefer the
        // active buffer's directory when set so opening a file from a
        // sibling repo lands lazygit on that repo rather than wherever
        // binvim was invoked from.
        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let mut stdout = io::stdout();
        // Hand the terminal over: pop kitty keyboard protocol, drop
        // mouse capture, leave the alt screen, drop raw mode. Same
        // sequence the yazi shell-out uses — see `open_yazi` for the
        // rationale on each step.
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();

        let status = Command::new("lazygit").current_dir(&start_dir).status();

        // Reclaim the terminal. Mouse capture must be re-armed
        // explicitly — clicks would otherwise stop reaching the editor
        // after lazygit exits.
        let _ = enable_raw_mode();
        let _ = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide);
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        );

        match status {
            Err(_) => {
                self.status_msg = "lazygit not on PATH".into();
                return;
            }
            Ok(_) => {
                self.status_msg.clear();
            }
        }

        // Working tree may have moved under every open buffer — stages,
        // commits, checkouts, stashes can all change which lines show
        // as added / modified / deleted. Refresh every buffer's hunks
        // and the active buffer's branch label.
        self.refresh_all_git_hunks();
        self.refresh_git_branch();
    }

    /// Recompute working-tree diff hunks for the active buffer plus
    /// every stashed buffer. Used after operations that may have moved
    /// the index or worktree out from under us (lazygit exit today;
    /// future external-git refresh tickers could share this).
    pub(super) fn refresh_all_git_hunks(&mut self) {
        // Active buffer first — `refresh_git_hunks` already exists and
        // reads `self.buffer.path` directly.
        self.refresh_git_hunks();
        // Each stashed buffer: recompute against its path if it has
        // one. Mirrors the active-buffer logic in `save::refresh_git_hunks`
        // but reads from the stash since the live `self.buffer`
        // isn't the one whose hunks we're updating.
        for stash in &mut self.buffers {
            stash.git_hunks = match stash.buffer.path.as_ref() {
                Some(p) => crate::git::diff_against_worktree(p).unwrap_or_default(),
                None => Vec::new(),
            };
        }
    }
}
