//! `:install` / `:update` — in-editor wrapper around `binvim::install`.
//!
//! Both commands share this overlay; `InstallerKind` selects the plan
//! builder and wording. `:install` installs what's missing; `:update`
//! upgrades tools already on `$PATH` (and leaves the rest for `:install`).
//!
//! Full-screen overlay that mirrors the `binvim-install` CLI:
//!   1. **Bundles** — multi-select checkbox of every language /
//!      Copilot / editor-tool bundle.
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

use crate::mode::Mode;
use binvim::install::{
    BUNDLES, Choice, NodeVersion, PlanItem, build_plan, build_update_plan, bundle_summary,
    detect_managers, discover_node_versions, plan_needs_node, run_plan,
};

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
    /// Subtitle text the renderer paints under the banner. Stage-
    /// specific (e.g. counts of items, hint about npm).
    pub subtitle: String,
}

impl InstallerState {
    fn new(kind: InstallerKind) -> Self {
        Self {
            stage: InstallerStage::Bundles,
            cursor: 0,
            checked: vec![false; BUNDLES.len()],
            bundle_picks: Vec::new(),
            detected_nodes: Vec::new(),
            node_picks: Vec::new(),
            plan: Vec::new(),
            kind,
            subtitle: bundles_subtitle(kind),
        }
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
    format!("{} — {tail}  ({} bundles)", kind.verb(), BUNDLES.len())
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
            InstallerStage::Plan => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.run_install_plan(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Backspace => {
                    // Go back to the previous stage (Node picker if it
                    // was visited, else bundle picker).
                    self.installer_go_back();
                }
                _ => {}
            },
        }
    }

    fn advance_from_bundles(&mut self) {
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
            self.status_msg = "Nothing selected — pick at least one bundle.".into();
            return;
        }
        state.bundle_picks = picks.clone();
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
        state.subtitle = format!(
            "plan — {} items · press y to {}, n to go back, q to cancel",
            state.plan.len(),
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
            state.checked = vec![false; BUNDLES.len()];
            for &i in &state.bundle_picks {
                if let Some(slot) = state.checked.get_mut(i) {
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

        // Pull the plan + node versions off `installer` so we can
        // mutate `self` freely during the run.
        let (plan, node_versions, kind) = {
            let state = match self.installer.as_mut() {
                Some(s) => s,
                None => return,
            };
            (
                std::mem::take(&mut state.plan),
                state.node_versions(),
                state.kind,
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
        println!();
        let summary = run_plan(&plan, &node_versions);
        let done = match kind {
            InstallerKind::Install => "installed",
            InstallerKind::Update => "updated",
        };
        println!();
        println!("─── Summary ───");
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
        self.status_msg = if summary.failed.is_empty() {
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
        self.dismiss_install();
    }
}

/// Shared key-handling for the Bundles + NodeVersions stages — both
/// drive the same checkbox list. `Enter` is *not* handled here; the
/// caller routes it stage-by-stage so it can advance / build the plan.
fn handle_picker_keys(state: &mut InstallerState, k: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if state.cursor + 1 < state.checked.len() {
                state.cursor += 1;
            }
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

/// Renderer-side helpers — small enough not to warrant their own
/// module. Each returns owned `Vec<(name, summary)>` because the
/// renderer borrows immutably and we'd otherwise be juggling lifetimes
/// against `App.installer`.

pub fn bundle_picker_items() -> Vec<(String, String)> {
    BUNDLES
        .iter()
        .map(|b| (b.name.to_string(), bundle_summary(b)))
        .collect()
}

pub fn node_picker_items(state: &InstallerState) -> Vec<(String, String)> {
    state
        .detected_nodes
        .iter()
        .map(|v| (v.label.clone(), v.npm_path.display().to_string()))
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
