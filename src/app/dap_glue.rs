//! DAP event handling and `:debug` / `:dap*` ex-command dispatch.
//!
//! Mirrors `app/lsp_glue.rs`: the main loop drains events off the manager
//! and we react here. Editor-side concerns (opening a buffer at the
//! stopped frame, surfacing status messages, opening the bottom pane on
//! session start) live here, not in `dap/manager.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::DebugSubCmd;
use crate::dap::{adapter_for_workspace, flat_locals_view, DapEvent, SessionState, StepKind};
use crate::mode::Mode;

impl super::App {
    pub(super) fn dispatch_debug(&mut self, sub: DebugSubCmd) {
        match sub {
            DebugSubCmd::Start => self.dap_start_session(),
            DebugSubCmd::Stop => self.dap_stop_session(),
            DebugSubCmd::Break => self.dap_toggle_breakpoint(),
            DebugSubCmd::ClearBreakpointsInFile => self.dap_clear_breakpoints_in_file(),
            DebugSubCmd::Continue => self.dap_step(StepKind::Continue),
            DebugSubCmd::Next => self.dap_step(StepKind::Next),
            DebugSubCmd::StepIn => self.dap_step(StepKind::StepIn),
            DebugSubCmd::StepOut => self.dap_step(StepKind::StepOut),
            DebugSubCmd::PaneToggle => {
                self.debug_pane_open = !self.debug_pane_open;
                self.adjust_viewport();
                self.status_msg = if self.debug_pane_open {
                    "debug pane: open".into()
                } else {
                    "debug pane: closed".into()
                };
            }
            DebugSubCmd::FocusPane => self.dap_enter_pane_focus(),
        }
    }

    pub(super) fn dispatch_debug_watch(&mut self, sub: crate::command::DebugWatchCmd) {
        match sub {
            crate::command::DebugWatchCmd::Add(expr) => {
                let display = expr.clone();
                if self.dap.add_watch(expr) {
                    let n = self.dap.watches.len();
                    self.status_msg = format!("watch [{n}]: {display}");
                    // Auto-open the pane so the user sees their
                    // watch appear without an extra :dappane.
                    if !self.debug_pane_open {
                        self.debug_pane_open = true;
                        self.adjust_viewport();
                    }
                } else {
                    self.status_msg = format!("watch already present: {display}");
                }
            }
            crate::command::DebugWatchCmd::Remove(idx) => match idx {
                None => {
                    let n = self.dap.watches.len();
                    self.dap.watches.clear();
                    self.status_msg = match n {
                        0 => "no watches to clear".into(),
                        1 => "cleared 1 watch".into(),
                        n => format!("cleared {n} watches"),
                    };
                }
                Some(one_based) => {
                    let zero_based = one_based - 1;
                    match self.dap.remove_watch(zero_based) {
                        Some(expr) => {
                            self.status_msg = format!("removed watch: {expr}");
                        }
                        None => {
                            self.status_msg = format!(
                                "watch [{one_based}] out of range (have {})",
                                self.dap.watches.len()
                            );
                        }
                    }
                }
            },
        }
    }

    /// `:dapwatches` — dump the watch list to the status line.
    /// Cheap inline format for the no-stop / pre-eval case where
    /// the user hasn't run the session yet but wants to confirm
    /// what they've queued up. Real listing lives in the debug
    /// pane once values are flowing.
    pub(super) fn dispatch_debug_watches_show(&mut self) {
        if self.dap.watches.is_empty() {
            self.status_msg = "no watches (add via `:dapwatch <expr>`)".into();
            return;
        }
        let summary: Vec<String> = self
            .dap
            .watches
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let n = i + 1;
                match &w.result {
                    Some(r) if r.error => format!("[{n}] {} = ERR", w.expr),
                    Some(r) => format!("[{n}] {} = {}", w.expr, r.value),
                    None => format!("[{n}] {} = …", w.expr),
                }
            })
            .collect();
        self.status_msg = summary.join("  ·  ");
    }

    fn dap_enter_pane_focus(&mut self) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        if !self.debug_pane_open {
            self.debug_pane_open = true;
            self.adjust_viewport();
        }
        self.dap_pane_cursor = 0;
        self.mode = Mode::DebugPane;
        self.status_msg =
            "pane: gt/gT switch tab · j/k move · Enter expand · Esc exits".into();
    }

    /// Mouse dispatch for the debug pane. Returns `true` if the
    /// click landed in pane rows (so the caller skips the editor
    /// mouse handling). Three regions:
    ///   1. Header chip row — focus the pane (no further action).
    ///   2. Tab bar row — click hit-tests against the per-tab
    ///      rectangles recorded in `dap_tab_hitboxes`. Switch
    ///      tabs + focus on a hit.
    ///   3. Body — focus the pane and (for clickable tabs) move
    ///      `dap_pane_cursor` to the clicked row.
    /// Scroll wheel inside the pane pages the active tab.
    pub(super) fn handle_debug_pane_mouse_event(
        &mut self,
        ev: &crossterm::event::MouseEvent,
        row: usize,
        col: usize,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let pane_rows = self.debug_pane_rows();
        if pane_rows == 0 {
            return false;
        }
        let pane_top = self.debug_pane_top();
        let pane_bottom = pane_top + pane_rows;
        if row < pane_top || row >= pane_bottom {
            return false;
        }
        let header_row = pane_top;
        let tab_row = pane_top + 1;
        let body_top = pane_top + 2;

        match ev.kind {
            MouseEventKind::ScrollUp => {
                self.dap_tab_scroll_by(-3);
                if !matches!(self.mode, Mode::DebugPane) {
                    self.mode = Mode::DebugPane;
                }
                return true;
            }
            MouseEventKind::ScrollDown => {
                self.dap_tab_scroll_by(3);
                if !matches!(self.mode, Mode::DebugPane) {
                    self.mode = Mode::DebugPane;
                }
                return true;
            }
            _ => {}
        }

        // Pull focus on any click anywhere in the pane. Subsequent
        // hit-testing further refines what the click did.
        let is_down = matches!(
            ev.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Middle | MouseButton::Right)
        );
        if !is_down {
            return true; // consumed but no action
        }
        if !matches!(self.mode, Mode::DebugPane) {
            self.mode = Mode::DebugPane;
        }
        if row == header_row {
            return true;
        }
        if row == tab_row {
            let hits = self.dap_tab_hitboxes.take();
            let mut clicked: Option<crate::app::DapPaneTab> = None;
            for (tab, x_start, x_end) in &hits {
                if (col as u16) >= *x_start && (col as u16) < *x_end {
                    clicked = Some(*tab);
                    break;
                }
            }
            self.dap_tab_hitboxes.set(hits);
            if let Some(tab) = clicked {
                self.dap_set_tab(tab);
            }
            return true;
        }
        // Body click — move the per-tab cursor to the clicked row.
        let body_row = row - body_top;
        let total = self.dap_active_tab_row_count_pub();
        let scroll = self.dap_tab_scroll(self.dap_pane_tab);
        let idx = scroll + body_row;
        if idx < total {
            self.dap_pane_cursor = idx;
            // Locals: clicking a expandable row also toggles it.
            // Frames: clicking jumps to the frame's source (future
            // polish — for now just selection).
            if matches!(self.dap_pane_tab, crate::app::DapPaneTab::Locals) {
                self.dap_pane_toggle_at_cursor();
            }
        }
        true
    }

    /// Public alias so the mouse dispatcher can read the row count
    /// without going through the private helper.
    pub(super) fn dap_active_tab_row_count_pub(&self) -> usize {
        self.dap_active_tab_row_count()
    }

    pub(super) fn dap_exit_pane_focus(&mut self) {
        if self.mode == Mode::DebugPane {
            self.mode = Mode::Normal;
            self.status_msg.clear();
        }
    }

    /// Switch to the previous / next tab in the bar. Wraps around so
    /// `gT` from Frames lands on Console.
    pub(super) fn dap_cycle_tab(&mut self, forward: bool) {
        let tabs = crate::app::DapPaneTab::all();
        let cur_idx = tabs.iter().position(|t| *t == self.dap_pane_tab).unwrap_or(0);
        let next = if forward {
            (cur_idx + 1) % tabs.len()
        } else {
            (cur_idx + tabs.len() - 1) % tabs.len()
        };
        self.dap_pane_tab = tabs[next];
        self.dap_pane_cursor = 0;
    }

    pub(super) fn dap_set_tab(&mut self, tab: crate::app::DapPaneTab) {
        if self.dap_pane_tab != tab {
            self.dap_pane_tab = tab;
            self.dap_pane_cursor = 0;
        }
    }

    /// Key dispatch for `Mode::DebugPane`. Returns `true` if the key was
    /// consumed (the caller skips the normal-mode dispatch in that case).
    pub(super) fn handle_debug_pane_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let body_len = self.dap_active_tab_row_count();
        match key.code {
            KeyCode::Esc => {
                self.dap_exit_pane_focus();
                true
            }
            // `gt` / `gT` (matching Vim's tab cycling) — but we only see
            // one keypress at a time here, so use Tab / BackTab as
            // the in-pane cycle keys + Right/Left as aliases.
            KeyCode::Tab => {
                self.dap_cycle_tab(true);
                true
            }
            KeyCode::BackTab => {
                self.dap_cycle_tab(false);
                true
            }
            KeyCode::Right => {
                self.dap_cycle_tab(true);
                true
            }
            KeyCode::Left => {
                self.dap_cycle_tab(false);
                true
            }
            // Number row → jump to tab by index (1-based, matches the
            // labels' visible order).
            KeyCode::Char(d) if d.is_ascii_digit() => {
                let idx = (d as u8 - b'1') as usize;
                let tabs = crate::app::DapPaneTab::all();
                if idx < tabs.len() {
                    self.dap_set_tab(tabs[idx]);
                }
                true
            }
            KeyCode::Char('y') if ctrl => {
                self.dap_tab_scroll_by(-1);
                true
            }
            KeyCode::Char('e') if ctrl => {
                self.dap_tab_scroll_by(1);
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if body_len > 0 {
                    self.dap_pane_cursor = (self.dap_pane_cursor + 1).min(body_len - 1);
                    self.dap_follow_selection();
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.dap_pane_cursor = self.dap_pane_cursor.saturating_sub(1);
                self.dap_follow_selection();
                true
            }
            KeyCode::Char('g') => {
                self.dap_pane_cursor = 0;
                self.dap_follow_selection();
                true
            }
            KeyCode::Char('G') => {
                self.dap_pane_cursor = body_len.saturating_sub(1);
                self.dap_follow_selection();
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if matches!(self.dap_pane_tab, crate::app::DapPaneTab::Locals) {
                    self.dap_pane_toggle_at_cursor();
                    self.dap_follow_selection();
                }
                true
            }
            // Stepping bindings while focus is in the pane.
            KeyCode::Char('c') => {
                self.dap.step(StepKind::Continue);
                true
            }
            KeyCode::Char('n') => {
                self.dap.step(StepKind::Next);
                true
            }
            KeyCode::Char('i') => {
                self.dap.step(StepKind::StepIn);
                true
            }
            KeyCode::Char('O') => {
                self.dap.step(StepKind::StepOut);
                true
            }
            // Ex-command escape hatch so the user can `:dapstop`/etc from
            // inside the pane without having to bounce through Normal.
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.cmdline.clear();
                self.history_reset();
                true
            }
            _ => false,
        }
    }

    /// Number of selectable rows in the currently-active tab. Used
    /// by the key handler to clamp `dap_pane_cursor`.
    fn dap_active_tab_row_count(&self) -> usize {
        match self.dap_pane_tab {
            crate::app::DapPaneTab::Frames => self
                .dap
                .session
                .as_ref()
                .map(|s| s.frames.len())
                .unwrap_or(0),
            crate::app::DapPaneTab::Locals => self
                .dap
                .session
                .as_ref()
                .map(flat_locals_view)
                .map(|v| v.len())
                .unwrap_or(0),
            crate::app::DapPaneTab::Watches => self.dap.watches.len(),
            crate::app::DapPaneTab::Breakpoints => self.dap.breakpoints.values().map(|v| v.len()).sum(),
            crate::app::DapPaneTab::Console => self
                .dap
                .output_buffer
                .iter()
                .map(|o| o.output.lines().count().max(1))
                .sum(),
        }
    }

    pub fn dap_tab_scroll(&self, tab: crate::app::DapPaneTab) -> usize {
        self.dap_tab_scrolls.get(&tab).copied().unwrap_or(0)
    }

    /// Adjust the active tab's scroll offset. For `Console` the
    /// offset counts lines hidden BELOW the bottom (so 0 sticks to
    /// the latest line, matching the old right-column behaviour);
    /// every other tab uses lines hidden ABOVE the top.
    pub(super) fn dap_tab_scroll_by(&mut self, delta: i32) {
        let tab = self.dap_pane_tab;
        let body_rows = self.dap_body_rows();
        let total = self.dap_active_tab_row_count();
        let max = total.saturating_sub(body_rows);
        let cur = self.dap_tab_scroll(tab) as i32;
        let next = (cur + delta).clamp(0, max as i32) as usize;
        self.dap_tab_scrolls.insert(tab, next);
    }

    /// Rows available for tab body — pane minus the header chip
    /// (debug status) and the tab bar.
    pub(super) fn dap_body_rows(&self) -> usize {
        self.debug_pane_rows().saturating_sub(2)
    }

    /// Adjust scroll so the cursor row in the active tab stays
    /// visible. Called from the key handler after a j/k/g/G.
    fn dap_follow_selection(&mut self) {
        let body_rows = self.dap_body_rows();
        if body_rows == 0 {
            return;
        }
        let cursor = self.dap_pane_cursor;
        let tab = self.dap_pane_tab;
        let mut scroll = self.dap_tab_scroll(tab);
        if cursor < scroll {
            scroll = cursor;
        }
        let last_visible = scroll + body_rows;
        if cursor >= last_visible {
            scroll = cursor + 1 - body_rows;
        }
        let total = self.dap_active_tab_row_count();
        let max = total.saturating_sub(body_rows);
        if scroll > max {
            scroll = max;
        }
        self.dap_tab_scrolls.insert(tab, scroll);
    }

    /// Visual Studio / Rider-style debug shortcut keys, dispatched
    /// regardless of editor mode so the muscle memory works during
    /// editing too. Returns `true` if the key was consumed.
    ///
    /// - `F5`        — start session (or continue if already paused)
    /// - `Shift+F5`  — stop session
    /// - `F9`        — toggle breakpoint at cursor
    /// - `F10`       — step over (next)
    /// - `F11`       — step into
    /// - `Shift+F11` — step out
    pub(super) fn try_handle_debug_function_key(&mut self, k: &KeyEvent) -> bool {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let modless = k
            .modifiers
            .difference(KeyModifiers::SHIFT)
            .is_empty();
        if !modless {
            return false;
        }
        match k.code {
            KeyCode::F(5) if shift => {
                self.dispatch_debug(DebugSubCmd::Stop);
                true
            }
            KeyCode::F(5) => {
                // F5 doubles as "start if there's no session yet" so the
                // user doesn't have to remember a separate Start binding.
                if self.dap.is_active() {
                    self.dispatch_debug(DebugSubCmd::Continue);
                } else {
                    self.dispatch_debug(DebugSubCmd::Start);
                }
                true
            }
            KeyCode::F(9) => {
                self.dispatch_debug(DebugSubCmd::Break);
                true
            }
            KeyCode::F(10) => {
                self.dispatch_debug(DebugSubCmd::Next);
                true
            }
            KeyCode::F(11) if shift => {
                self.dispatch_debug(DebugSubCmd::StepOut);
                true
            }
            KeyCode::F(11) => {
                self.dispatch_debug(DebugSubCmd::StepIn);
                true
            }
            _ => false,
        }
    }

    fn dap_pane_toggle_at_cursor(&mut self) {
        // Pull out the vref to toggle in a tight `&self` scope so the
        // ensuing `toggle_expanded` call can take `&mut self.dap`.
        let vref = {
            let Some(session) = self.dap.session.as_ref() else {
                return;
            };
            let flat = flat_locals_view(session);
            if flat.is_empty() {
                return;
            }
            let idx = self.dap_pane_cursor.min(flat.len() - 1);
            let row = &flat[idx];
            if !row.expandable {
                return;
            }
            row.var.variables_reference
        };
        self.dap.toggle_expanded(vref);
    }

    fn dap_start_session(&mut self) {
        // If a session is already alive (typically: paused on an
        // unhandled exception or a stale breakpoint), tear it down
        // first. Same effect as `<leader>dq` + `<leader>ds`, just
        // collapsed into the single keystroke — common workflow when
        // a transient first-run error (warm-up race, external service
        // not ready, …) wants a quick retry.
        //
        // The blocking variant waits up to 1.5s for the previous
        // adapter's debuggee to actually exit so its listening port is
        // released before the new launch tries to bind it. Manual dq+ds
        // worked because of the human pause between keystrokes; this
        // reproduces that pause programmatically.
        if self.dap.is_active() {
            self.status_msg = "debug: stopping previous session…".into();
            let _ =
                self.dap.stop_session_blocking(std::time::Duration::from_millis(1500));
        }
        // Start from the active buffer's directory when it's path-backed
        // (typical Normal-mode launch), otherwise the workspace cwd.
        // adapter_for_workspace walks up looking for any spec's root
        // markers, so a buffer deep inside the tree still finds the
        // right adapter regardless of what file is open.
        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let Some((adapter, root)) = crate::dap::adapter_for_workspace(&start_dir) else {
            self.status_msg = format!(
                "debug: no adapter for {} (need *.csproj/.sln, Cargo.toml, go.mod, or pyproject.toml/setup.py/requirements.txt/Pipfile)",
                start_dir.display()
            );
            return;
        };
        match adapter.key {
            "dotnet" => self.dap_resolve_dotnet(),
            "go" => self.dap_resolve_go(&root),
            "python" => self.dap_resolve_python(&root),
            "lldb" => self.dap_resolve_rust(&root),
            other => {
                self.status_msg = format!("debug: no resolver for adapter '{other}'");
            }
        }
    }

    /// .NET resolver — preserves the original two-stage project + profile
    /// flow. Splits the workspace-root + project-discovery off the
    /// generic dispatcher so the .sln/.git widening (which is unique to
    /// .NET) doesn't leak into other adapters.
    fn dap_resolve_dotnet(&mut self) {
        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let workspace_root = crate::dap::find_dotnet_workspace_root(&start_dir);
        let projects = crate::dap::find_dotnet_projects(&workspace_root);
        match projects.len() {
            0 => {
                self.status_msg = format!(
                    "debug: no .csproj/.fsproj/.vbproj under {}",
                    workspace_root.display()
                );
            }
            1 => {
                let project = projects.into_iter().next().unwrap();
                self.dap_start_session_with_project(project);
            }
            _ => self.open_debug_project_picker(projects),
        }
    }

    /// Go resolver — find every `package main` directory under the
    /// workspace root. Auto-pick when there's exactly one (the common
    /// single-binary case) so the user doesn't sit through a picker
    /// for no choice.
    fn dap_resolve_go(&mut self, workspace_root: &std::path::Path) {
        let mains = crate::dap::find_go_main_dirs(workspace_root);
        // Prefer the buffer's containing directory when it's one of the
        // main packages — saves a picker step when the user has the
        // file they want to debug already open.
        if let Some(buf_path) = self.buffer.path.as_ref() {
            if let Some(buf_dir) = buf_path.parent() {
                let canon_buf = buf_dir.canonicalize().unwrap_or_else(|_| buf_dir.to_path_buf());
                if mains.iter().any(|d| {
                    d.canonicalize().unwrap_or_else(|_| d.clone()) == canon_buf
                }) {
                    self.dap_launch_simple_target("go", canon_buf, None);
                    return;
                }
            }
        }
        match mains.len() {
            0 => {
                self.status_msg = format!(
                    "debug: no `package main` under {}",
                    workspace_root.display()
                );
            }
            1 => {
                let dir = mains.into_iter().next().unwrap();
                self.dap_launch_simple_target("go", dir, None);
            }
            _ => self.open_debug_target_picker(
                "go",
                "Go package",
                workspace_root,
                mains.into_iter().map(|p| (p, None)).collect(),
            ),
        }
    }

    /// Python resolver — prefer the active buffer if it's a `.py`,
    /// otherwise fall back to common entry-script names at the
    /// workspace root. Opens a picker when several candidates tie.
    fn dap_resolve_python(&mut self, workspace_root: &std::path::Path) {
        if let Some(buf_path) = self.buffer.path.clone() {
            if buf_path.extension().and_then(|s| s.to_str()) == Some("py") {
                self.dap_launch_simple_target("python", buf_path, None);
                return;
            }
        }
        let scripts = crate::dap::find_python_entry_scripts(workspace_root);
        match scripts.len() {
            0 => {
                self.status_msg = format!(
                    "debug: open a .py buffer or add main.py/manage.py/app.py at {}",
                    workspace_root.display()
                );
            }
            1 => {
                let script = scripts.into_iter().next().unwrap();
                self.dap_launch_simple_target("python", script, None);
            }
            _ => self.open_debug_target_picker(
                "python",
                "Python entry script",
                workspace_root,
                scripts.into_iter().map(|p| (p, None)).collect(),
            ),
        }
    }

    /// Rust resolver — parse `Cargo.toml` (and workspace members) for
    /// `[[bin]]` / `src/main.rs` / `src/bin/*.rs` targets. Each candidate
    /// carries the manifest path and the bin name so the prelaunch can
    /// invoke `cargo build --bin <name>` and `build_launch_args` can
    /// locate `target/debug/<name>`.
    fn dap_resolve_rust(&mut self, workspace_root: &std::path::Path) {
        let bins = crate::dap::find_rust_bin_targets(workspace_root);
        match bins.len() {
            0 => {
                self.status_msg = format!(
                    "debug: no Rust bin targets under {} — only library crates?",
                    workspace_root.display()
                );
            }
            1 => {
                let bin = bins.into_iter().next().unwrap();
                self.dap_launch_simple_target(
                    "lldb",
                    bin.manifest_path,
                    Some(bin.bin_name),
                );
            }
            _ => {
                let items: Vec<(std::path::PathBuf, Option<String>)> = bins
                    .into_iter()
                    .map(|b| (b.manifest_path, Some(b.bin_name)))
                    .collect();
                self.open_debug_target_picker(
                    "lldb",
                    "Rust bin target",
                    workspace_root,
                    items,
                );
            }
        }
    }

    /// Open a picker for adapters whose launch target is a single path
    /// (Go package dir, Python script, Rust manifest+bin). The display
    /// label is the path relative to the workspace root, with the bin
    /// name (Rust) appended when present so users can disambiguate
    /// multiple bins from the same crate.
    fn open_debug_target_picker(
        &mut self,
        adapter_key: &str,
        title: &str,
        workspace_root: &std::path::Path,
        targets: Vec<(std::path::PathBuf, Option<String>)>,
    ) {
        use crate::picker::{PickerKind, PickerPayload, PickerState};
        let canon_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let items: Vec<(String, PickerPayload)> = targets
            .into_iter()
            .map(|(path, name)| {
                let rel = path
                    .strip_prefix(&canon_root)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let label = match &name {
                    Some(n) => format!("{n} ({rel})"),
                    None => rel,
                };
                (
                    label,
                    PickerPayload::DebugTarget {
                        adapter_key: adapter_key.to_string(),
                        path,
                        name,
                    },
                )
            })
            .collect();
        let picker = PickerState::new(PickerKind::DebugTarget, title.into(), items);
        self.picker = Some(picker);
        self.mode = Mode::Picker;
    }

    /// Single-stage launch path for the Go / Python / Rust adapters.
    /// Builds a `LaunchContext` from the picked target and kicks off
    /// the session — no second-stage profile picker.
    pub(super) fn dap_start_target(
        &mut self,
        adapter_key: &str,
        path: std::path::PathBuf,
        name: Option<String>,
    ) {
        self.dap_launch_simple_target(adapter_key, path, name);
    }

    /// Shared launch routine for non-.NET adapters. `path` is whatever
    /// the adapter's `build_launch_args` expects in `project_path`:
    /// package dir (Go), script file (Python), manifest path (Rust).
    fn dap_launch_simple_target(
        &mut self,
        adapter_key: &str,
        path: std::path::PathBuf,
        name: Option<String>,
    ) {
        // Re-resolve the adapter from a path under the target. For
        // .NET-style flows we'd consult the stashed pending adapter,
        // but here the picked path is enough — the registry's marker
        // walk will land us on the same adapter.
        let probe = if path.is_dir() {
            path.clone()
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone())
        };
        let Some((adapter, _)) = crate::dap::adapter_for_workspace(&probe) else {
            self.status_msg =
                format!("debug: lost adapter '{adapter_key}' after picker close");
            return;
        };
        if adapter.key != adapter_key {
            self.status_msg = format!(
                "debug: adapter mismatch ({} vs {}) — workspace markers changed?",
                adapter.key, adapter_key
            );
            return;
        }
        // For Rust the prelaunch + launch want the manifest dir as cwd.
        // For Go the package dir IS the cwd. For Python the script's
        // parent dir is the cwd. Compute root from `path`'s shape.
        let root = if adapter_key == "lldb" {
            // path is the manifest file — manifest dir is the cwd.
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone())
        } else if path.is_dir() {
            path.clone()
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone())
        };
        let ctx = crate::dap::LaunchContext {
            root,
            project_path: Some(path.clone()),
            target_name: name.clone(),
            application_urls: Vec::new(),
            env: Default::default(),
        };
        let label = match &name {
            Some(n) => format!("{} ({n})", adapter.key),
            None => {
                let stem = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if stem.is_empty() {
                    adapter.key.to_string()
                } else {
                    format!("{} ({stem})", adapter.key)
                }
            }
        };
        self.status_msg = format!("debug: {label}");
        self.debug_pane_open = true;
        self.adjust_viewport();
        match self.dap.start_session(adapter, ctx) {
            Ok(()) => {
                self.status_msg = "debug: session starting".into();
            }
            Err(e) => {
                self.status_msg = format!("debug: {e}");
            }
        }
    }

    /// Open the project picker — one row per discovered `.csproj`,
    /// displayed as the path relative to the workspace root so the user
    /// can tell `Vettvangur.Site` from `Vettvangur.Core` at a glance.
    fn open_debug_project_picker(&mut self, projects: Vec<std::path::PathBuf>) {
        use crate::picker::{PickerKind, PickerPayload, PickerState};
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let canon_cwd = cwd.canonicalize().unwrap_or(cwd);
        let items: Vec<(String, PickerPayload)> = projects
            .into_iter()
            .map(|p| {
                let display = p
                    .strip_prefix(&canon_cwd)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| p.display().to_string());
                (display, PickerPayload::DebugProject(p))
            })
            .collect();
        let picker = PickerState::new(PickerKind::DebugProject, "Debug project".into(), items);
        self.picker = Some(picker);
        self.mode = Mode::Picker;
    }

    /// Continue the launch flow once a project has been picked (or auto-
    /// selected when there's only one). Reads
    /// `Properties/launchSettings.json` next to the project:
    ///
    /// - 0 runnable profiles → start without overrides (framework default
    ///   port, no extra env).
    /// - 1 runnable profile → use it directly.
    /// - >1 runnable profiles → stash the project + profile list on the
    ///   App and open the profile picker. The accept path routes back
    ///   through `dap_start_session_with_profile`.
    pub(super) fn dap_start_session_with_project(&mut self, project: std::path::PathBuf) {
        let project_dir = match project.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                self.status_msg = "debug: project path has no parent".into();
                return;
            }
        };
        let profiles = crate::dap::load_launch_profiles(&project_dir);
        match profiles.len() {
            0 => self.dap_start_session_with_profile(project, None),
            1 => {
                let profile = profiles.into_iter().next();
                self.dap_start_session_with_profile(project, profile);
            }
            _ => self.open_debug_profile_picker(project, profiles),
        }
    }

    /// Open the profile picker — one row per `commandName: "Project"`
    /// profile found in `Properties/launchSettings.json`. Each row
    /// displays the profile name and the first application URL so the
    /// user can tell `Umbraco.Web.UI (https://localhost:44317)` from
    /// `FaroeShip (https://localhost:44318)` at a glance.
    fn open_debug_profile_picker(
        &mut self,
        project: std::path::PathBuf,
        profiles: Vec<crate::dap::LaunchProfile>,
    ) {
        use crate::picker::{PickerKind, PickerPayload, PickerState};
        let items: Vec<(String, PickerPayload)> = profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let url_hint = if p.application_urls.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", p.application_urls.join(", "))
                };
                (format!("{}{}", p.name, url_hint), PickerPayload::DebugProfile(i))
            })
            .collect();
        // Stash the project + profile list so the picker accept path can
        // resolve which profile the index refers to. Cleared on accept,
        // on Esc cancel via picker_glue, and on next picker open.
        self.pending_debug_project = Some(project);
        self.pending_debug_profiles = profiles;
        let picker = PickerState::new(PickerKind::DebugProfile, "Launch profile".into(), items);
        self.picker = Some(picker);
        self.mode = Mode::Picker;
    }

    /// Final stage of the launch flow. `profile` is `None` when the
    /// project has no `commandName: "Project"` entries — we still start,
    /// just without applicationUrl / env overrides (framework defaults).
    pub(super) fn dap_start_session_with_profile(
        &mut self,
        project: std::path::PathBuf,
        profile: Option<crate::dap::LaunchProfile>,
    ) {
        let project_dir = match project.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                self.status_msg = "debug: project path has no parent".into();
                return;
            }
        };
        let Some((adapter, _)) = adapter_for_workspace(&project_dir) else {
            self.status_msg =
                "debug: no adapter found for this workspace (need a *.csproj/*.sln)".into();
            return;
        };
        let (application_urls, env, profile_label) = match profile {
            Some(p) => (p.application_urls, p.env, p.name),
            None => (Vec::new(), Default::default(), String::new()),
        };
        let ctx = crate::dap::LaunchContext {
            root: project_dir.clone(),
            project_path: Some(project.clone()),
            target_name: None,
            application_urls,
            env,
        };
        let project_label = project
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| "project");
        self.status_msg = if profile_label.is_empty() {
            format!("debug: {} ({})", adapter.key, project_label)
        } else {
            format!(
                "debug: {} ({} · {})",
                adapter.key, project_label, profile_label
            )
        };
        self.debug_pane_open = true;
        self.adjust_viewport();
        // Clear pending state so a subsequent <leader>ds doesn't see
        // stale data from this run.
        self.pending_debug_project = None;
        self.pending_debug_profiles.clear();
        match self.dap.start_session(adapter, ctx) {
            Ok(()) => {
                self.status_msg = "debug: session starting".into();
            }
            Err(e) => {
                self.status_msg = format!("debug: {e}");
            }
        }
    }

    fn dap_stop_session(&mut self) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        self.dap.stop_session();
        self.status_msg = "debug: session terminated".into();
        if self.mode == Mode::DebugPane {
            self.mode = Mode::Normal;
        }
        // Close the bottom pane on stop — the session is gone, there's
        // nothing useful to look at, and reclaiming the rows snaps the
        // editor back to its usual height.
        if self.debug_pane_open {
            self.debug_pane_open = false;
            self.adjust_viewport();
        }
    }

    fn dap_toggle_breakpoint(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "debug: buffer has no path".into();
            return;
        };
        let abs = path.canonicalize().unwrap_or(path);
        // Cursor.line is 0-based; DAP / the user-visible line number is 1-based.
        let line = self.window.cursor.line + 1;
        // Toggle the breakpoint silently — the gutter dot is the
        // user-visible confirmation, so a status-line notification is
        // redundant noise on every press.
        let _ = self.dap.toggle_breakpoint(&abs, line);
    }

    fn dap_clear_breakpoints_in_file(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "debug: buffer has no path".into();
            return;
        };
        let abs = path.canonicalize().unwrap_or(path);
        let n = self.dap.clear_breakpoints_in_file(&abs);
        self.status_msg = match n {
            0 => "debug: no breakpoints in this file".into(),
            1 => "debug: cleared 1 breakpoint".into(),
            n => format!("debug: cleared {n} breakpoints"),
        };
    }

    fn dap_step(&mut self, kind: StepKind) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        let stopped = self
            .dap
            .session
            .as_ref()
            .map(|s| matches!(s.state, SessionState::Stopped { .. }))
            .unwrap_or(false);
        if !stopped {
            self.status_msg = "debug: program is not paused".into();
            return;
        }
        self.dap.step(kind);
    }

    pub(super) fn handle_dap_events(&mut self, events: Vec<DapEvent>) {
        for ev in events {
            match ev {
                DapEvent::Initialized => {}
                DapEvent::Stopped {
                    thread_id, reason, ..
                } => {
                    self.status_msg = format!("debug: stopped — {} (thread {})", reason, thread_id);
                    // Reset the pane scroll positions so the user sees
                    // the new stop's frame + locals from a sensible
                    // starting position rather than wherever the previous
                    // stop's viewport happened to land.
                    self.dap_pane_cursor = 0;
                    self.dap_tab_scrolls.clear();
                    self.dap_jump_to_top_frame();
                }
                DapEvent::Continued { .. } => {
                    self.status_msg = "debug: running".into();
                }
                DapEvent::Output(_) => {}
                DapEvent::Thread { reason, thread_id } => {
                    if reason == "exited" {
                        self.status_msg = format!("debug: thread {} exited", thread_id);
                    }
                }
                DapEvent::Breakpoint { .. } => {}
                DapEvent::Exited { exit_code } => {
                    self.status_msg = format!("debug: debuggee exited ({})", exit_code);
                }
                DapEvent::Terminated => {
                    self.status_msg = "debug: session ended".into();
                    self.dap.session = None;
                    if self.mode == Mode::DebugPane {
                        self.mode = Mode::Normal;
                    }
                    if self.debug_pane_open {
                        self.debug_pane_open = false;
                        self.adjust_viewport();
                    }
                }
                DapEvent::AdapterError(msg) => {
                    self.status_msg = format!("debug error: {msg}");
                }
            }
        }
    }

    /// On a `stopped` event the manager has already requested the stack
    /// trace; by the time the renderer's next frame runs the top frame
    /// has typically arrived. Open the source file (if needed) and put
    /// the cursor on the frame's line so the user immediately sees where
    /// execution paused.
    fn dap_jump_to_top_frame(&mut self) {
        let Some(session) = self.dap.session.as_ref() else {
            return;
        };
        let Some(top) = session.frames.first() else {
            return;
        };
        let Some(path) = top.source.clone() else {
            return;
        };
        // DAP frames carry 1-based line numbers; binvim's cursor is 0-based.
        let line = top.line.saturating_sub(1);
        let col = top.column.saturating_sub(1);
        let already_open = self
            .buffer
            .path
            .as_ref()
            .map(|p| p == &path || p.canonicalize().ok() == path.canonicalize().ok())
            .unwrap_or(false);
        if !already_open {
            self.push_jump();
            if let Err(e) = self.open_buffer(path) {
                self.status_msg = format!("debug: cannot open frame source: {e}");
                return;
            }
        }
        self.window.cursor.line = line;
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
        self.clamp_cursor_normal();
        self.adjust_viewport();
    }
}
