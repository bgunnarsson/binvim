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
        self.dap_right_scroll = 0;
        // Park the left column on the last couple of frames so the
        // separator and first locals are visible by default — without
        // hiding the user-relevant frame context above.
        let frames_len = self
            .dap
            .session
            .as_ref()
            .map(|s| s.frames.len())
            .unwrap_or(0);
        self.dap_left_scroll = frames_len.saturating_sub(2);
        self.mode = Mode::DebugPane;
        self.status_msg =
            "pane: j/k navigate · ^Y/^E scroll · J/K log · Enter expand · Esc exits".into();
    }

    pub(super) fn dap_exit_pane_focus(&mut self) {
        if self.mode == Mode::DebugPane {
            self.mode = Mode::Normal;
            self.status_msg.clear();
        }
    }

    /// Key dispatch for `Mode::DebugPane`. Returns `true` if the key was
    /// consumed (the caller skips the normal-mode dispatch in that case).
    pub(super) fn handle_debug_pane_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let locals_len = self
            .dap
            .session
            .as_ref()
            .map(|s| flat_locals_view(s).len())
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc => {
                self.dap_exit_pane_focus();
                true
            }
            // Ctrl-Y / Ctrl-E: scroll the left column without moving the
            // selection — Vim-convention free scroll for peeking at
            // frames above the locals.
            KeyCode::Char('y') if ctrl => {
                self.dap_left_scroll = self.dap_left_scroll.saturating_sub(1);
                true
            }
            KeyCode::Char('e') if ctrl => {
                self.dap_left_scroll = self
                    .dap_left_scroll
                    .saturating_add(1)
                    .min(self.dap_left_scroll_max());
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if locals_len > 0 {
                    self.dap_pane_cursor = (self.dap_pane_cursor + 1).min(locals_len - 1);
                    self.dap_follow_selection_in_left_column();
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.dap_pane_cursor = self.dap_pane_cursor.saturating_sub(1);
                self.dap_follow_selection_in_left_column();
                true
            }
            KeyCode::Char('g') => {
                self.dap_pane_cursor = 0;
                self.dap_follow_selection_in_left_column();
                true
            }
            KeyCode::Char('G') => {
                self.dap_pane_cursor = locals_len.saturating_sub(1);
                self.dap_follow_selection_in_left_column();
                true
            }
            // Right column scrolling — capital J/K so lowercase stays
            // bound to locals navigation. J pages toward the latest log
            // line; K pages back into older history.
            KeyCode::Char('J') => {
                self.dap_right_scroll = self.dap_right_scroll.saturating_sub(1);
                true
            }
            KeyCode::Char('K') => {
                self.dap_right_scroll = self
                    .dap_right_scroll
                    .saturating_add(1)
                    .min(self.dap_right_scroll_max());
                true
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char(' ') => {
                self.dap_pane_toggle_at_cursor();
                self.dap_follow_selection_in_left_column();
                true
            }
            // Stepping bindings while focus is in the pane — saves an
            // Esc + <leader>d{c,n,i,O} round-trip during inspection.
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

    /// Maximum valid value for `dap_left_scroll`. The total left-column
    /// row count (frames + optional separator + locals tree) minus the
    /// number of body rows the pane currently has.
    pub(super) fn dap_left_scroll_max(&self) -> usize {
        let Some(session) = self.dap.session.as_ref() else {
            return 0;
        };
        let flat = flat_locals_view(session);
        let total = if flat.is_empty() {
            session.frames.len()
        } else {
            session.frames.len() + 1 + flat.len()
        };
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        total.saturating_sub(body_rows)
    }

    /// Maximum valid value for `dap_right_scroll`. Counts every output
    /// line currently in the buffer; the buffer is bounded by
    /// `OUTPUT_LOG_CAP` so this is cheap.
    pub(super) fn dap_right_scroll_max(&self) -> usize {
        let total_lines: usize = self
            .dap
            .output_buffer
            .iter()
            .map(|o| o.output.lines().count().max(1))
            .sum();
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        total_lines.saturating_sub(body_rows)
    }

    /// Adjust `dap_left_scroll` so the currently-selected local is in
    /// the visible viewport. Called after every selection-moving key.
    fn dap_follow_selection_in_left_column(&mut self) {
        let Some(session) = self.dap.session.as_ref() else {
            return;
        };
        let frames_len = session.frames.len();
        let flat_len = flat_locals_view(session).len();
        if flat_len == 0 {
            return;
        }
        let cursor = self.dap_pane_cursor.min(flat_len - 1);
        // Locals occupy rows `[frames_len + 1, frames_len + 1 + flat_len)`
        // — the `+1` accounts for the "── Locals ──" separator row.
        let selected_abs = frames_len + 1 + cursor;
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        if body_rows == 0 {
            return;
        }
        if selected_abs < self.dap_left_scroll {
            self.dap_left_scroll = selected_abs;
        }
        let last_visible = self.dap_left_scroll + body_rows;
        if selected_abs >= last_visible {
            self.dap_left_scroll = selected_abs + 1 - body_rows;
        }
        let max = self.dap_left_scroll_max();
        if self.dap_left_scroll > max {
            self.dap_left_scroll = max;
        }
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
                    self.dap_right_scroll = 0;
                    self.dap_left_scroll = 0;
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
