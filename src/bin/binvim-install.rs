//! `binvim-install` — interactive installer for the LSPs, formatters, and DAP
//! adapters binvim drives. The main binary is intentionally feature-detecting
//! at runtime (any missing tool is silently skipped); this helper exists so a
//! fresh install doesn't have to read the README to know what to install.
//!
//! Catalog + runner live in `binvim::install` so the in-editor `:install`
//! overlay drives the exact same plan. This file owns only the CLI's
//! crossterm picker UI and orchestration.

use std::io::{IsTerminal, Write, stdout};
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use binvim::install::{
    BUNDLES, Choice, Installer, NodeVersion, PlanItem, Summary, build_plan, bundle_summary,
    detect_managers, discover_node_versions, plan_needs_node, run_plan,
};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use crossterm::{execute, queue};

// ─── banner + palette ──────────────────────────────────────────────────────

const BANNER: &[&str] = &[
    "██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗",
    "██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║",
    "██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║",
    "██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    "██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    "╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];

// Catppuccin Mocha — keep in sync with the editor's palette.
const MAUVE: Color = Color::Rgb {
    r: 203,
    g: 166,
    b: 247,
};
const TEAL: Color = Color::Rgb {
    r: 148,
    g: 226,
    b: 213,
};
const GREEN: Color = Color::Rgb {
    r: 166,
    g: 227,
    b: 161,
};
const RED: Color = Color::Rgb {
    r: 243,
    g: 139,
    b: 168,
};
const YELLOW: Color = Color::Rgb {
    r: 249,
    g: 226,
    b: 175,
};
const SUBTLE: Color = Color::Rgb {
    r: 108,
    g: 112,
    b: 134,
};
const ACCENT: Color = Color::Rgb {
    r: 250,
    g: 179,
    b: 135,
};

// ─── checkbox UI ───────────────────────────────────────────────────────────

struct PickerState {
    cursor: usize,
    checked: Vec<bool>,
}

/// Generic multi-select checkbox prompt — drives both the language bundle
/// picker and the Node-version picker. Each `(name, summary)` pair becomes
/// one row; `default_checked` are pre-selected indices so `Enter` alone
/// picks a sensible default for the Node picker (newest version).
fn run_multi_select(
    subtitle: &str,
    items: &[(String, String)],
    default_checked: &[usize],
) -> Result<Option<Vec<usize>>> {
    if !stdout().is_terminal() {
        return Err(anyhow!(
            "binvim-install needs a TTY for the checkbox UI — run it directly in a terminal."
        ));
    }
    if items.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let mut state = PickerState {
        cursor: 0,
        checked: vec![false; items.len()],
    };
    for &i in default_checked {
        if let Some(slot) = state.checked.get_mut(i) {
            *slot = true;
        }
    }
    let name_width = items
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0)
        .max(20);

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, Hide)?;

    let result = (|| -> Result<Option<Vec<usize>>> {
        loop {
            render_list(&mut out, &state, items, subtitle, name_width)?;
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = event::read()?
            else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            match (code, modifiers) {
                (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return Ok(None),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                    if state.cursor + 1 < items.len() {
                        state.cursor += 1;
                    }
                }
                (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                    state.cursor = state.cursor.saturating_sub(1);
                }
                (KeyCode::Char('g'), _) | (KeyCode::Home, _) => state.cursor = 0,
                (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
                    state.cursor = items.len().saturating_sub(1);
                }
                (KeyCode::Char(' '), _) => {
                    let c = &mut state.checked[state.cursor];
                    *c = !*c;
                }
                (KeyCode::Char('a'), _) => state.checked.iter_mut().for_each(|c| *c = true),
                (KeyCode::Char('n'), _) => state.checked.iter_mut().for_each(|c| *c = false),
                (KeyCode::Enter, _) => {
                    let picks: Vec<usize> = state
                        .checked
                        .iter()
                        .enumerate()
                        .filter_map(|(i, &c)| if c { Some(i) } else { None })
                        .collect();
                    return Ok(Some(picks));
                }
                _ => {}
            }
        }
    })();

    execute!(out, Show)?;
    disable_raw_mode()?;
    println!();
    result
}

fn render_list(
    out: &mut impl Write,
    state: &PickerState,
    items: &[(String, String)],
    subtitle: &str,
    name_width: usize,
) -> Result<()> {
    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;

    queue!(
        out,
        SetForegroundColor(MAUVE),
        SetAttribute(Attribute::Bold)
    )?;
    for (i, line) in BANNER.iter().enumerate() {
        queue!(out, MoveTo(0, i as u16), Print(line))?;
    }
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(SUBTLE)
    )?;
    queue!(
        out,
        MoveTo(0, BANNER.len() as u16),
        Print(format!("  {subtitle}"))
    )?;
    queue!(out, ResetColor)?;

    let help_row = (BANNER.len() + 2) as u16;
    queue!(out, MoveTo(0, help_row), SetForegroundColor(SUBTLE))?;
    queue!(
        out,
        Print("  j/k move · space toggle · a all · n none · Enter confirm · q quit")
    )?;
    queue!(out, ResetColor)?;

    let list_top = help_row + 2;
    for (i, (name, summary)) in items.iter().enumerate() {
        let row = list_top + i as u16;
        let active = i == state.cursor;
        let checked = state.checked[i];
        queue!(out, MoveTo(0, row))?;
        if active {
            queue!(
                out,
                SetForegroundColor(ACCENT),
                SetAttribute(Attribute::Bold),
                Print("▸ ")
            )?;
        } else {
            queue!(out, Print("  "))?;
        }
        let mark = if checked { "[x]" } else { "[ ]" };
        let mark_color = if checked { GREEN } else { SUBTLE };
        queue!(out, SetForegroundColor(mark_color), Print(mark), Print(" "))?;
        if active {
            queue!(
                out,
                SetForegroundColor(ACCENT),
                SetAttribute(Attribute::Bold)
            )?;
        } else {
            queue!(out, ResetColor)?;
        }
        queue!(out, Print(format!("{:<width$}", name, width = name_width)))?;
        queue!(
            out,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(SUBTLE)
        )?;
        queue!(out, Print(format!("  {summary}")))?;
        queue!(out, ResetColor)?;
    }

    out.flush()?;
    Ok(())
}

fn pick_bundles() -> Result<Option<Vec<usize>>> {
    let items: Vec<(String, String)> = BUNDLES
        .iter()
        .map(|b| (b.name.to_string(), bundle_summary(b)))
        .collect();
    run_multi_select("install — pick the languages you want set up", &items, &[])
}

fn pick_node_versions(versions: &[NodeVersion]) -> Result<Option<Vec<usize>>> {
    let items: Vec<(String, String)> = versions
        .iter()
        .map(|v| (v.label.clone(), v.npm_path.display().to_string()))
        .collect();
    run_multi_select(
        "npm packages — pick which Node.js installations to install for",
        &items,
        &[0],
    )
}

// ─── plan + summary printers ───────────────────────────────────────────────

fn print_plan(plan: &[PlanItem], node_versions: &[NodeVersion]) {
    println!();
    println!("Plan:");
    println!();
    for item in plan {
        let used = item.used_by.join(", ");
        match &item.chosen {
            Choice::Already => {
                let_color(GREEN, " ✓ ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!("  [{}] — already on PATH ({})", item.tool.role.tag(), used),
                );
                println!();
            }
            Choice::Install(inst) => {
                let_color(TEAL, " → ");
                print!("{}", item.tool.label);
                let_color(SUBTLE, &format!("  [{}]  ", item.tool.role.tag()));
                print!("{}", inst.display());
                let_color(SUBTLE, &format!("   ({used})"));
                println!();
                if matches!(inst, Installer::Npm(_)) {
                    let targets = node_versions
                        .iter()
                        .map(|v| v.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let_color(SUBTLE, &format!("     targeting: {targets}\n"));
                }
            }
            Choice::Manual(msg) => {
                let_color(YELLOW, " ! ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!("  [{}] — manual install:", item.tool.role.tag()),
                );
                println!();
                let_color(SUBTLE, &format!("     {msg}"));
                println!();
            }
            Choice::NoManager(opts) => {
                let_color(RED, " ✗ ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!(
                        "  [{}] — no installer available, tried:",
                        item.tool.role.tag()
                    ),
                );
                println!();
                for o in opts {
                    let_color(SUBTLE, &format!("     {o}"));
                    println!();
                }
            }
        }
    }
    println!();
}

fn let_color(c: Color, s: &str) {
    let mut out = stdout();
    let _ = execute!(out, SetForegroundColor(c), Print(s), ResetColor);
}

fn confirm_proceed() -> Result<bool> {
    print!("Proceed with installs? [y/N] ");
    stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

fn print_summary(s: &Summary) {
    println!();
    println!("─────────────────────────────");
    println!("Summary:");
    let_color(GREEN, &format!("  {} installed\n", s.installed));
    let_color(SUBTLE, &format!("  {} already present\n", s.skipped));
    if s.manual > 0 {
        let_color(YELLOW, &format!("  {} manual\n", s.manual));
    }
    if !s.failed.is_empty() {
        let_color(RED, &format!("  {} failed\n", s.failed.len()));
        for (label, why) in &s.failed {
            let_color(RED, &format!("    {label}: {why}\n"));
        }
    }
    println!();
    if s.installed > 0 {
        let_color(
            SUBTLE,
            "Some installers extend $PATH (cargo, go, dotnet tool, gem, pipx).\n",
        );
        let_color(
            SUBTLE,
            "Open a fresh shell or re-source your rc file before launching binvim.\n",
        );
    }
}

fn print_managers(managers: &std::collections::BTreeSet<&'static str>) {
    let_color(SUBTLE, "Detected package managers: ");
    if managers.is_empty() {
        let_color(RED, "none on $PATH\n");
        return;
    }
    let_color(
        TEAL,
        &format!(
            "{}\n",
            managers.iter().copied().collect::<Vec<_>>().join(", ")
        ),
    );
}

// ─── entry ─────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let picks = match pick_bundles()? {
        Some(p) => p,
        None => {
            println!("Cancelled.");
            return Ok(());
        }
    };
    if picks.is_empty() {
        println!("Nothing selected.");
        return Ok(());
    }

    let managers = detect_managers();
    print_managers(&managers);

    let plan = build_plan(&picks, &managers);

    let needs_npm = plan_needs_node(&plan);
    let node_versions = if needs_npm {
        select_node_versions()?
    } else {
        Vec::new()
    };
    if needs_npm && node_versions.is_empty() {
        return Ok(());
    }

    print_plan(&plan, &node_versions);

    if !confirm_proceed()? {
        println!("Aborted.");
        return Ok(());
    }

    let summary = run_plan(&plan, &node_versions);
    print_summary(&summary);
    Ok(())
}

/// Discover Node.js installations, prompt the user when there's more than
/// one, and return the picked set. Empty `Vec` signals "abort the run" —
/// either no Node was found at all, or the user cancelled / picked zero
/// versions. The caller prints "Cancelled." / "Aborted." as appropriate.
fn select_node_versions() -> Result<Vec<NodeVersion>> {
    let detected = discover_node_versions();
    if detected.is_empty() {
        let_color(
            RED,
            "No Node.js installation found. Install Node.js (nvm, fnm, brew, apt, …) and re-run.\n",
        );
        return Ok(Vec::new());
    }
    if detected.len() == 1 {
        let_color(
            SUBTLE,
            &format!("Using Node {} for npm installs.\n", detected[0].label),
        );
        return Ok(vec![detected[0].clone()]);
    }
    let_color(
        SUBTLE,
        &format!(
            "Detected {} Node.js installations — pick which to install npm packages for.\n",
            detected.len()
        ),
    );
    match pick_node_versions(&detected)? {
        None => {
            println!("Cancelled.");
            Ok(Vec::new())
        }
        Some(indices) if indices.is_empty() => {
            println!("No Node version selected — aborting (npm installs need a target).");
            Ok(Vec::new())
        }
        Some(indices) => Ok(indices.into_iter().map(|i| detected[i].clone()).collect()),
    }
}
