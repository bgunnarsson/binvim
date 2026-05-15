//! Copilot ghost-completion glue. The LSP wiring lives in
//! `src/lsp/specs.rs` + `src/lsp/manager.rs`; this module sits between
//! that wire surface and the editor: maps `checkStatus` / `signIn`
//! replies onto `LspManager.copilot_status`, debounces the idle-pause
//! that fires `textDocument/inlineCompletion`, and handles `<Tab>`
//! accept of an active ghost suggestion.

use std::time::{Duration, Instant};

use crate::lsp::CopilotStatus;

/// How long Insert-mode typing must idle before we ask Copilot for an
/// inline suggestion. Tuned to roughly match what users perceive as
/// "stopped typing" — short enough to feel responsive, long enough to
/// not fire on every keystroke. Mirrors the GitHub Copilot defaults.
const COPILOT_IDLE_MS: u64 = 250;

/// How often to re-fire `checkStatus` while the user is mid-sign-in
/// (status is `PendingAuth`). Three seconds matches the cadence the
/// official GitHub Copilot plugins use for the same purpose — fast
/// enough that "I just entered the code" feels live, slow enough to
/// not flood the language server with status pings.
const COPILOT_POLL_MS: u64 = 3000;

impl super::App {
    /// Map a Copilot `checkStatus` / `signIn` response onto our local
    /// `CopilotStatus` enum and surface the result via the status
    /// message. Handles the device-flow `PendingAuth:{uri}:{code}`
    /// encoding the signIn handler packs into `kind`.
    pub(super) fn apply_copilot_status(&mut self, kind: String, user: Option<String>) {
        if let Some(rest) = kind.strip_prefix("PendingAuth:") {
            // Format is `PendingAuth:<uri>:<code>` — split from the
            // right since the URI itself may contain colons.
            let mut parts = rest.rsplitn(2, ':');
            let code = parts.next().unwrap_or("").to_string();
            let uri = parts.next().unwrap_or("").to_string();
            self.lsp.copilot_status = CopilotStatus::PendingAuth {
                verification_uri: uri.clone(),
                user_code: code.clone(),
            };
            self.status_msg = format!(
                "Copilot: visit {uri} and enter code {code}"
            );
            return;
        }
        match kind.as_str() {
            "OK" | "AlreadySignedIn" => {
                let display = user.clone().unwrap_or_else(|| "github user".into());
                self.lsp.copilot_status = CopilotStatus::SignedIn { user: display };
                self.status_msg = format!(
                    "Copilot: signed in as {}",
                    user.unwrap_or_else(|| "github user".into())
                );
            }
            "NotSignedIn" => {
                self.lsp.copilot_status = CopilotStatus::SignedOut;
                self.status_msg =
                    "Copilot: not signed in — run `:copilot signin` to authenticate".into();
                // Auto-kick the device-flow request so the user gets a
                // URL+code in the next message instead of needing to
                // discover the `:copilot signin` command first.
                self.lsp.request_copilot_sign_in();
            }
            "NotAuthorized" => {
                self.lsp.copilot_status =
                    CopilotStatus::Error("account not authorized for Copilot".into());
                self.status_msg = "Copilot: account not authorized".into();
            }
            "NoTelemetryConsent" => {
                self.lsp.copilot_status =
                    CopilotStatus::Error("telemetry consent required".into());
                self.status_msg = "Copilot: telemetry consent required".into();
            }
            other => {
                self.lsp.copilot_status = CopilotStatus::Error(other.into());
                self.status_msg = format!("Copilot: {other}");
            }
        }
    }

    /// Per-frame poll from the main loop. If Copilot is signed in, the
    /// user is in Insert mode on a real file, and the typing-idle
    /// window has passed without an active ghost, fire an inline-
    /// completion request anchored on the current cursor.
    pub(super) fn copilot_maybe_request_inline(&mut self) {
        if !matches!(self.lsp.copilot_status, CopilotStatus::SignedIn { .. }) {
            return;
        }
        if !matches!(self.mode, crate::mode::Mode::Insert) {
            return;
        }
        if self.copilot_ghost.is_some() {
            return;
        }
        let Some(path) = self.buffer.path.clone() else { return };
        if self.last_keystroke_at.elapsed() < Duration::from_millis(COPILOT_IDLE_MS) {
            return;
        }
        // Skip if we already asked for this exact (path, version,
        // cursor) — otherwise a slow server reply that returned
        // NotFound would re-fire every frame.
        let key_version = self.buffer.version;
        if self.last_copilot_request_version.get(&path).copied() == Some(key_version) {
            return;
        }
        self.last_copilot_request_version
            .insert(path.clone(), key_version);
        self.lsp.request_copilot_inline(
            &path,
            self.window.cursor.line,
            self.window.cursor.col,
            key_version,
        );
    }

    /// True when there's a live ghost anchored at the current cursor
    /// position in the current buffer. The renderer + the Tab accept
    /// path both consult this to decide whether to paint / consume.
    pub fn copilot_ghost_active(&self) -> bool {
        let Some(g) = self.copilot_ghost.as_ref() else {
            return false;
        };
        if !matches!(self.mode, crate::mode::Mode::Insert) {
            return false;
        }
        Some(g.path.as_path()) == self.buffer.path.as_deref()
            && g.line == self.window.cursor.line
            && g.col == self.window.cursor.col
    }

    /// `<Tab>` in Insert mode with an active ghost — insert the
    /// suggestion at the cursor and clear the ghost. Returns true if
    /// the ghost was consumed; the Tab handler in `input.rs` uses
    /// this to decide whether to fall through to snippet / literal-
    /// tab behaviour.
    pub fn copilot_accept_ghost(&mut self) -> bool {
        if !self.copilot_ghost_active() {
            return false;
        }
        let Some(ghost) = self.copilot_ghost.take() else {
            return false;
        };
        // Record the undo step before mutating so a single `u` undoes
        // the whole acceptance, matching how Insert-mode chunks land
        // in the history elsewhere.
        self.history
            .record(&self.buffer.rope, self.window.cursor);
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        self.buffer.insert_str(line, col, &ghost.text);
        // Advance the cursor to the end of the inserted text. The
        // text may span multiple lines — count the newlines and
        // measure the tail-line's length to land at the right col.
        let lines: Vec<&str> = ghost.text.split('\n').collect();
        let new_line = line + lines.len() - 1;
        let new_col = if lines.len() == 1 {
            col + lines[0].chars().count()
        } else {
            lines.last().map(|s| s.chars().count()).unwrap_or(0)
        };
        self.window.cursor.line = new_line;
        self.window.cursor.col = new_col;
        self.window.cursor.want_col = new_col;
        true
    }

    /// Any non-Tab keystroke in Insert mode invalidates a pending
    /// ghost — drop it so the next idle-pause can fire a fresh
    /// request anchored on the new cursor.
    pub fn copilot_invalidate_ghost(&mut self) {
        self.copilot_ghost = None;
        // Reset the per-buffer "last asked at version" so the next
        // idle pause re-fires the request.
        if let Some(path) = self.buffer.path.as_ref() {
            self.last_copilot_request_version.remove(path);
        }
    }

    /// While the user is in the middle of a device-flow sign-in
    /// (status = PendingAuth), poll `checkStatus` every few seconds
    /// so we notice "user just authenticated in the browser" without
    /// needing a manual `:copilot reload` or full editor restart.
    /// Called from the render-tick alongside the inline-completion
    /// poll; no-op for any other status.
    pub(super) fn copilot_maybe_poll_status(&mut self) {
        if !matches!(self.lsp.copilot_status, CopilotStatus::PendingAuth { .. }) {
            return;
        }
        if self.last_copilot_status_poll.elapsed() < Duration::from_millis(COPILOT_POLL_MS) {
            return;
        }
        self.last_copilot_status_poll = Instant::now();
        self.lsp.request_copilot_check_status();
    }

    /// `:copilot` — report current Copilot status in the status line.
    /// Useful for "did the language server actually start" /
    /// "am I signed in" debugging without opening `:health`.
    pub(super) fn copilot_show_status(&mut self) {
        if !self.lsp.copilot_enabled {
            self.status_msg = "Copilot: disabled (set [copilot] enabled = true in config)".into();
            return;
        }
        self.status_msg = match &self.lsp.copilot_status {
            CopilotStatus::NotStarted => {
                "Copilot: not started (open a file to attach the LSP)".into()
            }
            CopilotStatus::Checking => "Copilot: checking sign-in status…".into(),
            CopilotStatus::SignedIn { user } => format!("Copilot: signed in as {user}"),
            CopilotStatus::SignedOut => "Copilot: signed out".into(),
            CopilotStatus::PendingAuth {
                verification_uri,
                user_code,
            } => format!(
                "Copilot: pending auth — visit {verification_uri} and enter {user_code}"
            ),
            CopilotStatus::Error(msg) => format!("Copilot: error — {msg}"),
        };
    }

    /// `:copilot signin` — re-fire the device-flow sign-in request.
    /// Useful if the initial signIn failed (e.g. network blip on
    /// launch) or the user cancelled the previous prompt.
    pub(super) fn copilot_signin(&mut self) {
        if !self.lsp.copilot_enabled {
            self.status_msg = "Copilot: disabled (set [copilot] enabled = true in config)".into();
            return;
        }
        if !self.lsp.request_copilot_sign_in() {
            self.status_msg =
                "Copilot: language server not running (open any file to attach it)".into();
            return;
        }
        self.status_msg = "Copilot: signIn request sent…".into();
    }

    /// `:copilot reload` — re-fire `checkStatus`. The common case is
    /// "I just finished signing in via the browser; pick up my new
    /// state". The auto-poll handles this on a 3s loop, but the
    /// command gives the user a knob if they're impatient or the
    /// poll is paused (mode != PendingAuth).
    pub(super) fn copilot_reload(&mut self) {
        if !self.lsp.copilot_enabled {
            self.status_msg = "Copilot: disabled (set [copilot] enabled = true in config)".into();
            return;
        }
        if !self.lsp.request_copilot_check_status() {
            self.status_msg =
                "Copilot: language server not running (open any file to attach it)".into();
            return;
        }
        self.last_copilot_status_poll = Instant::now();
        self.status_msg = "Copilot: refreshing status…".into();
    }

    /// `:copilot signout` — call the language server's `signOut`
    /// method and drop the local sign-in cache. The user has to
    /// re-authenticate (via `:copilot signin`) to get suggestions
    /// again.
    pub(super) fn copilot_signout(&mut self) {
        if !self.lsp.copilot_enabled {
            self.status_msg = "Copilot: disabled (set [copilot] enabled = true in config)".into();
            return;
        }
        if !self.lsp.request_copilot_sign_out() {
            self.status_msg =
                "Copilot: language server not running (open any file to attach it)".into();
            return;
        }
        self.lsp.copilot_status = CopilotStatus::SignedOut;
        self.status_msg = "Copilot: signed out".into();
    }
}
