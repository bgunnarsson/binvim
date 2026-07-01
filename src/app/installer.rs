//! `:install` / `:update` — in-editor wrapper around `binvim::install`.
//!
//! Both commands share this overlay; `InstallerKind` selects the plan
//! builder and wording. `:install` installs what's missing; `:update`
//! upgrades tools already on `$PATH` (and leaves the rest for `:install`).
//!
//! Full-screen overlay that mirrors the `binvim-install` CLI:
//!   1. **Bundles** — multi-select checkbox of every language /
//!      Copilot / editor-tool bundle. In `:update` mode a binvim
//!      self-update row is prepended at index 0 (see `binvim_offset`).
//!   2. **NodeVersions** — only if the plan has any `npm install -g`
//!      step. Skipped automatically when exactly one Node.js
//!      installation is detected; errors out if none.
//!   3. **Plan** — render the deduped install plan + per-tool
//!      `[LSP]/[FMT]/[DAP]/[TOOL]` chip, with npm steps annotated
//!      with the target Node versions. `y` confirms and the editor
//!      suspends lazygit-style to let the installers print to the
//!      real terminal; `n` / `Esc` cancels.
//!
//! Mirrors `src/bin/binvim-install.rs` keystroke-for-keystroke
//! (`j/k`, `Space`, `a`/`n`, `Enter`, `q`/`Esc`) so users moving
//! between the CLI and the in-editor flow don't have to relearn.

use std::collections::HashSet;

use crate::lang::Lang;
use crate::mode::Mode;
use crate::picker::{PickerKind, PickerPayload, PickerState};
use binvim::install::{
    BUNDLES, BinvimUpdate, Choice, NodeVersion, PlanItem, Role, Tool, build_plan,
    build_update_plan, bundle_index_by_name, detect_binvim_update, detect_managers,
    discover_node_versions, missing_core_tools, on_path, plan_needs_node, run_binvim_update,
    run_plan,
};

/// Map a detected [`Lang`] to the index of its `BUNDLES` entry, so the
/// first-run flow can preselect exactly the toolchain the buffer under the
/// cursor needs. The name table lives here (not in the shared `install`
/// library) because `Lang` is editor-only; resolution is by bundle name so
/// reordering the catalog can't mis-point the preselection.
///
/// Languages with no auto-installable bundle (JSON — biome formats it but no
/// JSON LSP ships in a bundle; XML / `.editorconfig` / `.gitignore` — no
/// server at all) return `None`: the caller stays quiet.
fn bundle_for_lang(lang: Lang) -> Option<usize> {
    let name = match lang {
        Lang::Rust => "Rust",
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => "TypeScript / JavaScript",
        Lang::Go => "Go",
        Lang::Python => "Python",
        Lang::C | Lang::Cpp => "C / C++",
        Lang::CSharp => "C#",
        Lang::Razor => "Razor / .cshtml",
        Lang::Bash => "Bash / Shell",
        Lang::Yaml => "YAML",
        Lang::Lua => "Lua",
        Lang::Svelte => "Svelte",
        Lang::Markdown => "Markdown",
        Lang::Toml => "TOML",
        Lang::Ruby => "Ruby",
        Lang::Php => "PHP",
        Lang::Java => "Java",
        Lang::Zig => "Zig",
        Lang::Nix => "Nix",
        Lang::Elixir => "Elixir",
        Lang::Kotlin => "Kotlin",
        Lang::Dockerfile => "Docker",
        Lang::Sql => "SQL",
        Lang::Css | Lang::Scss => "CSS / SCSS / Less",
        Lang::Html => "HTML",
        Lang::Json | Lang::Xml | Lang::EditorConfig | Lang::GitIgnore => return None,
    };
    bundle_index_by_name(name)
}

/// State machine for the overlay. `App.installer` is `Some` while the
/// overlay is up and `None` once it's dismissed; this enum tracks the
/// current sub-screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerStage {
    Bundles,
    NodeVersions,
    Plan,
}

/// Whether the overlay is running `:install` (install what's missing) or
/// `:update` (upgrade what's already on `$PATH`). The two share the entire
/// three-stage flow; only the plan builder, the wording, and the run banner
/// differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerKind {
    Install,
    Update,
}

impl InstallerKind {
    /// Verb used in subtitles / help / banners ("install" vs "update").
    fn verb(self) -> &'static str {
        match self {
            InstallerKind::Install => "install",
            InstallerKind::Update => "update",
        }
    }
}

pub struct InstallerState {
    pub stage: InstallerStage,
    /// Cursor position within the currently visible list.
    pub cursor: usize,
    /// Per-row check state for the active stage's list. Reset when
    /// transitioning between Bundles and NodeVersions.
    pub checked: Vec<bool>,
    /// Bundle indices the user picked at the Bundles stage. Carried
    /// through to plan-build time so the user can navigate back
    /// without redoing the work.
    pub bundle_picks: Vec<usize>,
    /// Node.js installations found on the system. Empty until the
    /// NodeVersions stage opens (or stays empty when the plan has
    /// no npm steps).
    pub detected_nodes: Vec<NodeVersion>,
    /// Indices into `detected_nodes` the user has picked. Auto-set
    /// to `[0]` (the newest version) when the stage opens.
    pub node_picks: Vec<usize>,
    /// The built plan, populated at the transition into the Plan
    /// stage and rendered there.
    pub plan: Vec<PlanItem>,
    /// Whether this overlay installs (`:install`) or updates (`:update`).
    pub kind: InstallerKind,
    /// `:update` only — how the running binvim binary can update itself,
    /// shown as the first checkbox in the bundle list. `None` in `:install`
    /// mode (so no binvim row is rendered).
    pub binvim_update: Option<BinvimUpdate>,
    /// Whether the user ticked the binvim self-update row.
    pub binvim_selected: bool,
    /// Catalog tool `bin`s found on `$PATH`, probed once when the overlay
    /// opens (probing per render frame would be far too many syscalls). Used
    /// to mark already-installed tools green in the bundle picker.
    pub installed: HashSet<&'static str>,
    /// Subtitle text the renderer paints under the banner. Stage-
    /// specific (e.g. counts of items, hint about npm).
    pub subtitle: String,
    /// Scroll offset (in plan rows) for the Plan stage — the Bundles /
    /// NodeVersions stages scroll their cursor instead, so this only
    /// applies once the plan is on screen.
    pub plan_scroll: usize,
    /// Largest valid `plan_scroll`, stashed by the renderer each frame
    /// (it knows the viewport height after laying out the banner / help)
    /// so the input handler can clamp without re-measuring.
    pub plan_max_scroll: std::cell::Cell<usize>,
}

impl InstallerState {
    fn new(kind: InstallerKind) -> Self {
        // Only `:update` self-updates binvim; `:install` leaves it out.
        let binvim_update = match kind {
            InstallerKind::Update => Some(detect_binvim_update()),
            InstallerKind::Install => None,
        };
        let row_count = BUNDLES.len() + binvim_update.is_some() as usize;
        // Dedupe bins across bundles before probing so a shared tool
        // (prettier, lldb-dap, …) is only checked once.
        let installed: HashSet<&'static str> = BUNDLES
            .iter()
            .flat_map(|b| b.tools.iter().map(|t| t.bin))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|bin| on_path(bin))
            .collect();
        Self {
            stage: InstallerStage::Bundles,
            cursor: 0,
            checked: vec![false; row_count],
            bundle_picks: Vec::new(),
            detected_nodes: Vec::new(),
            node_picks: Vec::new(),
            plan: Vec::new(),
            kind,
            binvim_update,
            binvim_selected: false,
            installed,
            subtitle: bundles_subtitle(kind),
            plan_scroll: 0,
            plan_max_scroll: std::cell::Cell::new(0),
        }
    }

    /// `1` when the bundle list has a leading binvim self-update row (update
    /// mode), `0` otherwise. The bundle picker's `checked` / `cursor` indices
    /// are shifted by this much relative to `BUNDLES`.
    pub fn binvim_offset(&self) -> usize {
        self.binvim_update.is_some() as usize
    }

    /// Resolve the picked Node.js installations into a borrowed slice
    /// of `NodeVersion`s suitable for `install::run_plan`.
    pub fn node_versions(&self) -> Vec<NodeVersion> {
        self.node_picks
            .iter()
            .filter_map(|&i| self.detected_nodes.get(i).cloned())
            .collect()
    }
}

fn bundles_subtitle(kind: InstallerKind) -> String {
    let tail = match kind {
        InstallerKind::Install => "pick the languages you want set up",
        InstallerKind::Update => "pick the installed languages to update",
    };
    format!(
        "{} — {tail}  ({} bundles · green = already installed)",
        kind.verb(),
        BUNDLES.len()
    )
}

fn node_subtitle(kind: InstallerKind, detected: usize) -> String {
    format!(
        "npm packages — pick which Node.js installations to {} for  ({detected} detected)",
        kind.verb()
    )
}

impl super::App {
    /// `:install` entry point. Pops the overlay; subsequent keystrokes
    /// route through `handle_installer_key`.
    pub(super) fn cmd_install(&mut self) {
        self.open_installer(InstallerKind::Install);
    }

    /// `:update` entry point — same overlay as `:install`, but the plan only
    /// upgrades tools already on `$PATH` (see `install::build_update_plan`).
    pub(super) fn cmd_update(&mut self) {
        self.open_installer(InstallerKind::Update);
    }

    fn open_installer(&mut self, kind: InstallerKind) {
        // Other full-screen overlays would otherwise paint over us.
        self.show_health_page = false;
        self.show_messages_page = false;
        self.show_registers_page = false;
        self.show_test_results_page = false;
        self.show_start_page = false;
        self.completion = None;
        self.hover = None;
        self.signature_help = None;
        self.whichkey = None;

        self.installer = Some(InstallerState::new(kind));
        self.show_install_page = true;
        self.mode = Mode::Installer;
    }

    /// Close the overlay and return to Normal mode without running
    /// anything. Used by `q`/`Esc` and by post-run resume.
    pub(super) fn dismiss_install(&mut self) {
        self.installer = None;
        self.show_install_page = false;
        if matches!(self.mode, Mode::Installer) {
            self.mode = Mode::Normal;
        }
    }

    /// First-run nudge: called after a buffer attaches its LSP. If the active
    /// file's language is missing its primary LSP or formatter and we haven't
    /// already said so this session, pop a small picker listing what's missing
    /// (accept → `:install` preselected, `Esc` → dismiss). A popup rather than
    /// a `status_msg` so a competing notification — Copilot sign-in, an LSP
    /// message — can't paint over it. Gated behind `[install] prompt_on_open`
    /// (default on), skipped for large files (which never attach a server), and
    /// never opened over an existing overlay or a non-Normal context.
    pub(super) fn maybe_prompt_toolchain(&mut self) {
        if !self.config.install.prompt_on_open || self.buffer.is_large() {
            return;
        }
        // Don't hijack an active picker/overlay or an in-progress edit.
        if self.picker.is_some() || !matches!(self.mode, Mode::Normal) {
            return;
        }
        let Some(path) = self.buffer.path.as_deref() else {
            return;
        };
        let Some(lang) = crate::lang::Lang::detect(path) else {
            return;
        };
        let Some(bundle_idx) = bundle_for_lang(lang) else {
            return;
        };
        // Once per language per session — don't re-nag on every buffer switch.
        if self.toolchain_prompted.contains(&bundle_idx) {
            return;
        }
        let missing = missing_core_tools(bundle_idx);
        if missing.is_empty() {
            return;
        }
        self.toolchain_prompted.insert(bundle_idx);
        self.open_toolchain_picker(bundle_idx, &missing);
    }

    /// Build + show the missing-toolchain picker for one bundle. Each row is a
    /// missing tool (label + role); they all route to the same bundle, so
    /// accepting any one opens `:install` preselected to that language's
    /// bundle. The first row is the primary "install everything missing for
    /// this language" action (same as `:install` on the bundle); the rows below
    /// itemise what that covers. Every row routes to the same bundle install —
    /// the installer is language-granular — so picking a specific tool still
    /// sets up the whole language, with the review overlay as the confirm step.
    fn open_toolchain_picker(&mut self, bundle_idx: usize, missing: &[&'static Tool]) {
        let name = BUNDLES
            .get(bundle_idx)
            .map(|b| b.name)
            .unwrap_or("this language");
        // Row 0: the "install all" action. Rows 1..N: the individual missing
        // tools, for context. All carry the same bundle payload.
        let mut items: Vec<(String, PickerPayload)> = Vec::with_capacity(missing.len() + 1);
        items.push((
            format!("Install all missing dependencies ({})", missing.len()),
            PickerPayload::InstallToolchain { bundle_idx },
        ));
        for t in missing {
            let role = match t.role {
                Role::Lsp => "LSP",
                Role::Formatter => "formatter",
                Role::Dap => "debugger",
                Role::Tool => "tool",
            };
            items.push((
                format!("{}  ·  {role}", t.label),
                PickerPayload::InstallToolchain { bundle_idx },
            ));
        }
        let title = format!(
            "Set up {name} — {} not installed (Enter to install · Esc to skip)",
            missing.len()
        );
        let mut state = PickerState::new(PickerKind::InstallToolchain, title, items);
        // Accent the "install all" row so it reads as the primary action even
        // when the cursor moves down onto an itemised tool.
        state.marked = Some(0);
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    /// `<leader>i` — open `:install` preselected to the current buffer's
    /// language bundle so the user lands on exactly the toolchain they need,
    /// reviews it, and confirms. Falls back to the plain installer when the
    /// buffer has no language-specific bundle (JSON, XML, a `[No Name]`
    /// scratch buffer).
    pub(super) fn install_toolchain_for_current(&mut self) {
        match self
            .buffer
            .path
            .as_deref()
            .and_then(crate::lang::Lang::detect)
            .and_then(bundle_for_lang)
        {
            Some(idx) => self.open_installer_for_bundle(idx),
            None => self.open_installer(InstallerKind::Install),
        }
    }

    /// Open the installer overlay with one bundle preselected (its row checked
    /// and the cursor parked on it). Shared by `<leader>i` and the accept path
    /// of the missing-toolchain picker.
    pub(super) fn open_installer_for_bundle(&mut self, bundle_idx: usize) {
        self.open_installer(InstallerKind::Install);
        if let Some(state) = self.installer.as_mut() {
            let row = bundle_idx + state.binvim_offset();
            if let Some(slot) = state.checked.get_mut(row) {
                *slot = true;
                state.cursor = row;
            }
        }
    }

    pub(super) fn handle_installer_key(&mut self, k: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        let Some(state) = self.installer.as_mut() else {
            // Defensive: somehow in Installer mode without state.
            self.dismiss_install();
            return;
        };

        // Universal dismiss + Ctrl-C.
        match (k.code, k.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
                self.dismiss_install();
                return;
            }
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.dismiss_install();
                return;
            }
            _ => {}
        }

        match state.stage {
            InstallerStage::Bundles | InstallerStage::NodeVersions => {
                handle_picker_keys(state, k);
                // Enter: move to the next stage.
                if matches!(k.code, KeyCode::Enter) {
                    let stage = state.stage;
                    match stage {
                        InstallerStage::Bundles => self.advance_from_bundles(),
                        InstallerStage::NodeVersions => self.advance_from_nodes(),
                        _ => {}
                    }
                }
            }
            InstallerStage::Plan => match (k.code, k.modifiers) {
                (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _) => self.run_install_plan(),
                (KeyCode::Char('n'), m) | (KeyCode::Char('N'), m)
                    if !m.contains(KeyModifiers::CONTROL) =>
                {
                    // Go back to the previous stage (Node picker if it
                    // was visited, else bundle picker).
                    self.installer_go_back();
                }
                (KeyCode::Backspace, _) => self.installer_go_back(),
                // The plan can outgrow the viewport — scroll it. j/k by a
                // row, Ctrl-d/u by half a page, PgDn/PgUp / Ctrl-f/b by a
                // page, g/G to the ends.
                (KeyCode::Char('j'), _) | (KeyCode::Down, _) => self.installer_plan_scroll_by(1),
                (KeyCode::Char('k'), _) | (KeyCode::Up, _) => self.installer_plan_scroll_by(-1),
                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let step = (self.buffer_rows() / 2).max(1) as isize;
                    self.installer_plan_scroll_by(step);
                }
                (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let step = (self.buffer_rows() / 2).max(1) as isize;
                    self.installer_plan_scroll_by(-step);
                }
                (KeyCode::Char('f'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                    self.installer_plan_scroll_by(step);
                }
                (KeyCode::Char('b'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                    self.installer_plan_scroll_by(-step);
                }
                (KeyCode::PageDown, _) => {
                    let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                    self.installer_plan_scroll_by(step);
                }
                (KeyCode::PageUp, _) => {
                    let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                    self.installer_plan_scroll_by(-step);
                }
                (KeyCode::Char('g'), _) | (KeyCode::Home, _) => {
                    if let Some(s) = self.installer.as_mut() {
                        s.plan_scroll = 0;
                    }
                }
                (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
                    if let Some(s) = self.installer.as_mut() {
                        s.plan_scroll = s.plan_max_scroll.get();
                    }
                }
                _ => {}
            },
        }
    }

    /// Mouse-wheel scroll for the overlay. On the Plan stage it scrolls
    /// the plan; on the checkbox stages it walks the cursor (which the
    /// renderer keeps visible), so the wheel feels the same everywhere.
    pub(super) fn installer_scroll_by(&mut self, delta: isize) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        match state.stage {
            InstallerStage::Plan => {
                let max = state.plan_max_scroll.get();
                let next = (state.plan_scroll as isize + delta).max(0) as usize;
                state.plan_scroll = next.min(max);
            }
            InstallerStage::Bundles | InstallerStage::NodeVersions => {
                let last = state.checked.len().saturating_sub(1);
                let next = (state.cursor as isize + delta).max(0) as usize;
                state.cursor = next.min(last);
            }
        }
    }

    /// Move the Plan-stage viewport by `delta` rows, clamping to
    /// `[0, plan_max_scroll]` (the renderer stashes the max each frame).
    /// Negative deltas scroll up.
    fn installer_plan_scroll_by(&mut self, delta: isize) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        let max = state.plan_max_scroll.get();
        let next = (state.plan_scroll as isize + delta).max(0) as usize;
        state.plan_scroll = next.min(max);
    }

    fn advance_from_bundles(&mut self) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        // In update mode index 0 is the binvim self-update row; bundle rows
        // start at `offset`, so map checked indices back onto BUNDLES.
        let offset = state.binvim_offset();
        let binvim_selected = offset == 1 && state.checked.first().copied().unwrap_or(false);
        let picks: Vec<usize> = state
            .checked
            .iter()
            .enumerate()
            .skip(offset)
            .filter_map(|(i, &c)| if c { Some(i - offset) } else { None })
            .collect();
        if picks.is_empty() && !binvim_selected {
            self.status_msg = if offset == 1 {
                "Nothing selected — pick binvim or at least one bundle.".into()
            } else {
                "Nothing selected — pick at least one bundle.".into()
            };
            return;
        }
        state.bundle_picks = picks.clone();
        state.binvim_selected = binvim_selected;
        let kind = state.kind;

        let managers = detect_managers();
        let plan = match kind {
            InstallerKind::Install => build_plan(&picks, &managers),
            InstallerKind::Update => build_update_plan(&picks, &managers),
        };
        let needs_npm = plan_needs_node(&plan);

        if needs_npm {
            let detected = discover_node_versions();
            if detected.is_empty() {
                self.status_msg =
                    "No Node.js installation found. Install Node.js (nvm, fnm, brew, …) first."
                        .into();
                self.dismiss_install();
                return;
            }
            if detected.len() == 1 {
                // Single Node — skip the picker, go straight to plan.
                let state = self.installer.as_mut().unwrap();
                state.detected_nodes = detected;
                state.node_picks = vec![0];
                state.plan = plan;
                self.enter_plan_stage();
                return;
            }
            // Two or more — prompt.
            let state = self.installer.as_mut().unwrap();
            state.detected_nodes = detected;
            state.node_picks.clear();
            state.checked = vec![false; state.detected_nodes.len()];
            // Default-check the newest (already sorted first).
            if !state.checked.is_empty() {
                state.checked[0] = true;
            }
            state.cursor = 0;
            state.stage = InstallerStage::NodeVersions;
            state.subtitle = node_subtitle(kind, state.detected_nodes.len());
            state.plan = plan;
        } else {
            let state = self.installer.as_mut().unwrap();
            state.plan = plan;
            self.enter_plan_stage();
        }
    }

    fn advance_from_nodes(&mut self) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        let picks: Vec<usize> = state
            .checked
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| if c { Some(i) } else { None })
            .collect();
        if picks.is_empty() {
            self.status_msg = "Pick at least one Node version (npm installs need a target).".into();
            return;
        }
        state.node_picks = picks;
        self.enter_plan_stage();
    }

    fn enter_plan_stage(&mut self) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        state.stage = InstallerStage::Plan;
        state.cursor = 0;
        state.plan_scroll = 0;
        let count = state.plan.len() + state.binvim_selected as usize;
        state.subtitle = format!(
            "plan — {count} items · press y to {}, n to go back, q to cancel",
            state.kind.verb()
        );
    }

    fn installer_go_back(&mut self) {
        let Some(state) = self.installer.as_mut() else {
            return;
        };
        if !state.detected_nodes.is_empty() && state.detected_nodes.len() > 1 {
            // Return to the Node picker, preserving prior selections.
            state.stage = InstallerStage::NodeVersions;
            state.checked = vec![false; state.detected_nodes.len()];
            for &i in &state.node_picks {
                if let Some(slot) = state.checked.get_mut(i) {
                    *slot = true;
                }
            }
            state.cursor = 0;
            state.subtitle = node_subtitle(state.kind, state.detected_nodes.len());
        } else {
            // No Node stage to return to — go all the way back to bundles.
            state.stage = InstallerStage::Bundles;
            let offset = state.binvim_offset();
            state.checked = vec![false; BUNDLES.len() + offset];
            if offset == 1 {
                state.checked[0] = state.binvim_selected;
            }
            for &i in &state.bundle_picks {
                if let Some(slot) = state.checked.get_mut(i + offset) {
                    *slot = true;
                }
            }
            state.cursor = 0;
            state.subtitle = bundles_subtitle(state.kind);
        }
    }

    /// Suspend the editor lazygit-style, run the plan with inherited
    /// stdio so install output streams to the host terminal, prompt
    /// to return, and resume. Mirrors `lazygit_glue::cmd_lazygit`.
    fn run_install_plan(&mut self) {
        use crossterm::{
            cursor::{Hide, Show},
            event::{
                DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
                PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
            },
            execute,
            terminal::{
                EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
            },
        };

        // Pull the plan + node versions + binvim self-update off `installer`
        // so we can mutate `self` freely during the run.
        let (plan, node_versions, kind, binvim) = {
            let state = match self.installer.as_mut() {
                Some(s) => s,
                None => return,
            };
            let binvim = if state.binvim_selected {
                state.binvim_update.clone()
            } else {
                None
            };
            (
                std::mem::take(&mut state.plan),
                state.node_versions(),
                state.kind,
                binvim,
            )
        };
        let verb = kind.verb();

        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();

        // Banner line so the user knows where the output is coming from,
        // then run.
        println!();
        println!("─── binvim-{verb} (in-editor) ───");

        // binvim self-update runs first — a replaced binary only takes effect
        // on the next launch, so it shouldn't gate the toolchain updates.
        let binvim_note = binvim.as_ref().map(|u| {
            if u.is_manual() {
                println!("\n↑ binvim — update it yourself:");
                println!("  {}", u.display());
                format!("binvim: manual ({})", u.method())
            } else {
                match run_binvim_update(u) {
                    Ok(()) => format!("binvim updated via {}", u.method()),
                    Err(e) => format!("binvim update FAILED ({e})"),
                }
            }
        });

        println!();
        let summary = run_plan(&plan, &node_versions);
        let done = match kind {
            InstallerKind::Install => "installed",
            InstallerKind::Update => "updated",
        };
        println!();
        println!("─── Summary ───");
        if let Some(note) = &binvim_note {
            println!("  {note}");
        }
        println!("  {} {done}", summary.installed);
        if summary.skipped > 0 {
            println!("  {} already present", summary.skipped);
        }
        if summary.not_installed > 0 {
            println!(
                "  {} not installed (run :install to add)",
                summary.not_installed
            );
        }
        if summary.manual > 0 {
            println!("  {} manual (see above)", summary.manual);
        }
        if !summary.failed.is_empty() {
            println!("  {} failed:", summary.failed.len());
            for (label, why) in &summary.failed {
                println!("    {label}: {why}");
            }
        }
        if summary.installed > 0 {
            println!();
            println!("Some installers extend $PATH (cargo, go, dotnet tool, gem, pipx).");
            println!("Open a fresh shell or re-source your rc file before relying on them.");
        }
        println!();
        print!("Press Enter to return to binvim... ");
        use std::io::Write;
        let _ = stdout.flush();
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);

        // Reclaim the terminal — same incantation lazygit_glue uses.
        let _ = enable_raw_mode();
        let _ = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide);
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        );

        // Status line summary in the editor, then dismiss the overlay.
        let tools_msg = if summary.failed.is_empty() {
            match kind {
                InstallerKind::Install => format!(
                    "install: {} installed, {} already present, {} manual",
                    summary.installed, summary.skipped, summary.manual
                ),
                InstallerKind::Update => format!(
                    "update: {} updated, {} not installed, {} manual",
                    summary.installed, summary.not_installed, summary.manual
                ),
            }
        } else {
            format!(
                "{verb}: {} {done}, {} failed — see :messages for the rerun command",
                summary.installed,
                summary.failed.len()
            )
        };
        self.status_msg = match &binvim_note {
            Some(note) => format!("{note} · {tools_msg}"),
            None => tools_msg,
        };
        self.dismiss_install();
    }
}

/// Shared key-handling for the Bundles + NodeVersions stages — both
/// drive the same checkbox list. `Enter` is *not* handled here; the
/// caller routes it stage-by-stage so it can advance / build the plan.
fn handle_picker_keys(state: &mut InstallerState, k: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    match k.code {
        KeyCode::Char('j') | KeyCode::Down if state.cursor + 1 < state.checked.len() => {
            state.cursor += 1;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.cursor = state.cursor.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => state.cursor = 0,
        KeyCode::Char('G') | KeyCode::End => {
            state.cursor = state.checked.len().saturating_sub(1);
        }
        KeyCode::Char(' ') => {
            if let Some(slot) = state.checked.get_mut(state.cursor) {
                *slot = !*slot;
            }
        }
        KeyCode::Char('a') => state.checked.iter_mut().for_each(|c| *c = true),
        KeyCode::Char('n') => state.checked.iter_mut().for_each(|c| *c = false),
        _ => {}
    }
}

// Renderer-side helpers — small enough not to warrant their own module.
// Each returns owned `PickerRow`s because the renderer borrows immutably and
// we'd otherwise be juggling lifetimes against `App.installer`.

/// One row in a checkbox picker: a name plus a summary the renderer paints
/// to its right.
pub struct PickerRow {
    pub name: String,
    pub summary: PickerSummary,
}

pub enum PickerSummary {
    /// A single uncoloured string (Node picker rows, binvim self-update row).
    Plain(String),
    /// A bundle's tool list, each flagged with whether it's already on
    /// `$PATH`, so the renderer can paint installed tools green.
    Tools(Vec<ToolStatus>),
}

pub struct ToolStatus {
    pub label: String,
    pub installed: bool,
}

pub fn bundle_picker_rows(state: &InstallerState) -> Vec<PickerRow> {
    let mut rows = Vec::new();
    // `:update` puts a binvim self-update row at the top of the list.
    if let Some(u) = &state.binvim_update {
        let summary = if u.is_manual() {
            format!("self-update ({}) — {}", u.method(), u.display())
        } else {
            format!("self-update via {} — {}", u.method(), u.display())
        };
        rows.push(PickerRow {
            name: "binvim".to_string(),
            summary: PickerSummary::Plain(summary),
        });
    }
    for b in BUNDLES {
        let tools = b
            .tools
            .iter()
            .map(|t| ToolStatus {
                label: t.label.to_string(),
                installed: state.installed.contains(t.bin),
            })
            .collect();
        rows.push(PickerRow {
            name: b.name.to_string(),
            summary: PickerSummary::Tools(tools),
        });
    }
    rows
}

pub fn node_picker_rows(state: &InstallerState) -> Vec<PickerRow> {
    state
        .detected_nodes
        .iter()
        .map(|v| PickerRow {
            name: v.label.clone(),
            summary: PickerSummary::Plain(v.npm_path.display().to_string()),
        })
        .collect()
}

/// Plan items as `(symbol, label, role_tag, detail, color_tag)` rows
/// so the renderer doesn't need to import `install::Choice`.
pub fn plan_rows(state: &InstallerState) -> Vec<PlanRow> {
    use binvim::install::Installer as I;
    let node_targets = state
        .node_picks
        .iter()
        .filter_map(|&i| state.detected_nodes.get(i).map(|v| v.label.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut rows = Vec::new();
    // binvim self-update sits at the top of the plan, mirroring its row in
    // the bundle list.
    if state.binvim_selected {
        if let Some(u) = &state.binvim_update {
            let (glyph, color, detail) = if u.is_manual() {
                (
                    " ! ",
                    PlanRowColor::Yellow,
                    format!("manual: {}", u.display()),
                )
            } else {
                (
                    " ↑ ",
                    PlanRowColor::Teal,
                    format!("{}   (self-update via {})", u.display(), u.method()),
                )
            };
            rows.push(PlanRow {
                glyph,
                color,
                label: "binvim".to_string(),
                role: "BIN",
                detail,
                target: String::new(),
            });
        }
    }
    for item in &state.plan {
        let used = item.used_by.join(", ");
        match &item.chosen {
            Choice::Already => rows.push(PlanRow {
                glyph: " ✓ ",
                color: PlanRowColor::Green,
                label: item.tool.label.to_string(),
                role: item.tool.role.tag(),
                detail: format!("already on PATH ({used})"),
                target: String::new(),
            }),
            Choice::Install(inst) => {
                let mut target = String::new();
                if matches!(inst, I::Npm(_)) && !node_targets.is_empty() {
                    target = format!("targeting: {node_targets}");
                }
                rows.push(PlanRow {
                    glyph: " → ",
                    color: PlanRowColor::Teal,
                    label: item.tool.label.to_string(),
                    role: item.tool.role.tag(),
                    detail: format!("{}   ({used})", inst.display()),
                    target,
                });
            }
            Choice::Update(inst) => {
                let mut target = String::new();
                if matches!(inst, I::Npm(_)) && !node_targets.is_empty() {
                    target = format!("targeting: {node_targets}");
                }
                rows.push(PlanRow {
                    glyph: " ↑ ",
                    color: PlanRowColor::Teal,
                    label: item.tool.label.to_string(),
                    role: item.tool.role.tag(),
                    detail: format!("{}   ({used})", inst.upgrade_display()),
                    target,
                });
            }
            Choice::NotInstalled => rows.push(PlanRow {
                glyph: " · ",
                color: PlanRowColor::Subtle,
                label: item.tool.label.to_string(),
                role: item.tool.role.tag(),
                detail: format!("not installed — run :install to add it ({used})"),
                target: String::new(),
            }),
            Choice::Manual(msg) => rows.push(PlanRow {
                glyph: " ! ",
                color: PlanRowColor::Yellow,
                label: item.tool.label.to_string(),
                role: item.tool.role.tag(),
                detail: format!("manual install: {msg}"),
                target: String::new(),
            }),
            Choice::NoManager(opts) => rows.push(PlanRow {
                glyph: " ✗ ",
                color: PlanRowColor::Red,
                label: item.tool.label.to_string(),
                role: item.tool.role.tag(),
                detail: format!("no installer available, tried: {}", opts.join(" | ")),
                target: String::new(),
            }),
        }
    }
    rows
}

pub struct PlanRow {
    pub glyph: &'static str,
    pub color: PlanRowColor,
    pub label: String,
    pub role: &'static str,
    pub detail: String,
    pub target: String,
}

#[derive(Copy, Clone)]
pub enum PlanRowColor {
    Green,
    Teal,
    Yellow,
    Red,
    /// Muted — used for `:update`'s "not installed" rows.
    Subtle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_for_lang_points_at_the_named_bundle() {
        // Spot-check the non-obvious many-to-one mappings and confirm the
        // resolved index really is the bundle we expect.
        let cases = [
            (Lang::Rust, "Rust"),
            (Lang::Tsx, "TypeScript / JavaScript"),
            (Lang::JavaScript, "TypeScript / JavaScript"),
            (Lang::C, "C / C++"),
            (Lang::Cpp, "C / C++"),
            (Lang::CSharp, "C#"),
            (Lang::Scss, "CSS / SCSS / Less"),
            (Lang::Css, "CSS / SCSS / Less"),
            (Lang::Dockerfile, "Docker"),
        ];
        for (lang, name) in cases {
            let idx = bundle_for_lang(lang).expect("lang should map to a bundle");
            assert_eq!(BUNDLES[idx].name, name, "{lang:?} mis-mapped");
        }
    }

    #[test]
    fn bundle_for_lang_stays_quiet_for_bundleless_langs() {
        // JSON has no LSP bundle (biome formats it via the TS bundle, but we
        // don't want to nag "typescript-language-server missing" on a .json
        // file); XML / editorconfig / gitignore have no server at all.
        for lang in [Lang::Json, Lang::Xml, Lang::EditorConfig, Lang::GitIgnore] {
            assert_eq!(bundle_for_lang(lang), None, "{lang:?} should be bundleless");
        }
    }
}
