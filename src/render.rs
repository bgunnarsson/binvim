use crate::app::App;
use crate::lang::Lang;
use crate::lsp::Severity;
use crate::mode::{Mode, VisualKind};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use std::io::Write;

const TAB_WIDTH: usize = 4;

pub fn draw(out: &mut impl Write, app: &App) -> Result<()> {
    queue!(out, BeginSynchronizedUpdate, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    draw_buffer(out, app)?;
    draw_status_line(out, app)?;
    draw_command_line(out, app)?;
    if app.mode == Mode::Picker {
        draw_picker(out, app)?;
    }
    if app.completion.is_some() {
        draw_completion_popup(out, app)?;
    }
    place_cursor(out, app)?;
    queue!(out, EndSynchronizedUpdate)?;
    Ok(())
}

fn draw_completion_popup(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(c) = app.completion.as_ref() else { return Ok(()); };
    if c.items.is_empty() {
        return Ok(());
    }
    let max_h = (app.height as usize).saturating_sub(2);
    let popup_h = c.items.len().min(10).min(max_h.saturating_sub(2));
    if popup_h == 0 {
        return Ok(());
    }
    // Compute popup width from labels (cap at 60).
    let max_label = c
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(8);
    let max_kind = c
        .items
        .iter()
        .filter_map(|i| i.kind.as_ref().map(|k| k.chars().count()))
        .max()
        .unwrap_or(0);
    let popup_w = (max_label + max_kind + 4).min(60).min((app.width as usize).saturating_sub(4));

    // Scroll window so the selected item is visible.
    let start = if c.selected >= popup_h {
        c.selected + 1 - popup_h
    } else {
        0
    };

    // Anchor at cursor position in the buffer area.
    let gutter = app.gutter_width();
    let cursor_row = app.cursor.line.saturating_sub(app.view_top);
    let cursor_col = gutter + app.cursor.col;
    // Below the cursor unless that would overflow; otherwise above.
    let buffer_rows = app.buffer_rows();
    let mut top_row = cursor_row + 1;
    if top_row + popup_h > buffer_rows {
        top_row = cursor_row.saturating_sub(popup_h);
    }
    let mut left_col = cursor_col;
    if left_col + popup_w > app.width as usize {
        left_col = (app.width as usize).saturating_sub(popup_w);
    }

    for row in 0..popup_h {
        let pos = start + row;
        if pos >= c.items.len() {
            break;
        }
        let item = &c.items[pos];
        let selected = pos == c.selected;
        let y = (top_row + row) as u16;
        queue!(out, MoveTo(left_col as u16, y))?;
        if selected {
            queue!(
                out,
                SetBackgroundColor(Color::DarkBlue),
                SetForegroundColor(Color::White)
            )?;
        } else {
            queue!(
                out,
                SetBackgroundColor(Color::DarkGrey),
                SetForegroundColor(Color::White)
            )?;
        }
        let kind = item.kind.as_deref().unwrap_or("");
        let label_max = popup_w.saturating_sub(kind.chars().count() + 3);
        let label_trunc: String = item.label.chars().take(label_max).collect();
        let pad = popup_w
            .saturating_sub(label_trunc.chars().count() + kind.chars().count() + 3);
        queue!(
            out,
            Print(format!(
                " {}{} {} ",
                label_trunc,
                " ".repeat(pad),
                kind
            ))
        )?;
        queue!(out, ResetColor)?;
    }
    Ok(())
}

fn picker_layout(app: &App) -> (usize, usize, usize) {
    let h = app.height as usize;
    let picker_h = (h * 2 / 5).clamp(6, 20);
    let bottom_chrome = 2; // status line + cmdline
    let top_row = h.saturating_sub(picker_h + bottom_chrome);
    (top_row, picker_h, h.saturating_sub(bottom_chrome))
}

fn draw_picker(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(picker) = app.picker.as_ref() else { return Ok(()); };
    let (top_row, picker_h, end_row) = picker_layout(app);
    let w = app.width as usize;

    // Title row.
    let title = format!(
        " {}  {}/{} ",
        picker.title,
        if picker.filtered.is_empty() { 0 } else { picker.selected + 1 },
        picker.filtered.len()
    );
    let pad = w.saturating_sub(title.chars().count());
    queue!(
        out,
        MoveTo(0, top_row as u16),
        Clear(ClearType::CurrentLine),
        SetAttribute(Attribute::Reverse),
        Print(title),
        Print(" ".repeat(pad)),
        SetAttribute(Attribute::Reset)
    )?;

    // Input row.
    let input_row = top_row + 1;
    queue!(
        out,
        MoveTo(0, input_row as u16),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        Print("> "),
        ResetColor,
        Print(&picker.input)
    )?;

    // List rows.
    let list_top = top_row + 2;
    let list_h = (end_row.saturating_sub(list_top)).min(picker_h.saturating_sub(2));
    let start = if picker.selected >= list_h {
        picker.selected + 1 - list_h
    } else {
        0
    };
    for row in 0..list_h {
        let y = list_top + row;
        queue!(out, MoveTo(0, y as u16), Clear(ClearType::CurrentLine))?;
        let pos = start + row;
        if pos >= picker.filtered.len() {
            continue;
        }
        let item_idx = picker.filtered[pos];
        let display = &picker.items[item_idx];
        let selected = pos == picker.selected;
        if selected {
            queue!(
                out,
                SetBackgroundColor(Color::DarkBlue),
                SetForegroundColor(Color::White)
            )?;
        }
        let max_w = w.saturating_sub(2);
        let truncated: String = display.chars().take(max_w).collect();
        let pad = max_w.saturating_sub(truncated.chars().count());
        queue!(
            out,
            Print(format!(" {}{}", truncated, " ".repeat(pad)))
        )?;
        if selected {
            queue!(out, ResetColor)?;
        }
    }
    Ok(())
}

fn draw_buffer(out: &mut impl Write, app: &App) -> Result<()> {
    let rows = app.buffer_rows();
    let gutter = app.gutter_width();
    let avail = (app.width as usize).saturating_sub(gutter);
    for row in 0..rows {
        let line_idx = app.view_top + row;
        queue!(out, MoveTo(0, row as u16))?;
        if line_idx < app.buffer.line_count() {
            // Diagnostic sign column.
            let sign = app.worst_diagnostic(line_idx).map(|s| match s {
                Severity::Error => ('!', Color::Red),
                Severity::Warning => ('?', Color::Yellow),
                Severity::Info => ('i', Color::Blue),
                Severity::Hint => ('h', Color::DarkBlue),
            });
            if let Some((ch, color)) = sign {
                queue!(
                    out,
                    SetForegroundColor(color),
                    Print(ch.to_string()),
                    ResetColor
                )?;
            } else {
                queue!(out, Print(" "))?;
            }
            queue!(
                out,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("{:>width$} ", line_idx + 1, width = gutter - 2)),
                ResetColor
            )?;
            draw_line_with_selection(out, app, line_idx, avail)?;
        } else {
            queue!(
                out,
                SetForegroundColor(Color::DarkBlue),
                Print("~"),
                ResetColor
            )?;
        }
    }
    Ok(())
}

fn draw_line_with_selection(
    out: &mut impl Write,
    app: &App,
    line_idx: usize,
    avail: usize,
) -> Result<()> {
    let slice = app.buffer.rope.line(line_idx);
    let mut text: String = slice.chars().collect();
    if text.ends_with('\n') {
        text.pop();
    }
    let sel = app.line_selection(line_idx);
    let search_matches = app.line_search_matches(line_idx);
    let line_byte_start = app.buffer.rope.line_to_byte(line_idx);
    let chars: Vec<char> = text.chars().collect();
    let mut visual_used = 0usize;
    let mut byte_off = line_byte_start;
    for (col, c) in chars.iter().enumerate() {
        let display_w = if *c == '\t' { TAB_WIDTH } else { 1 };
        if visual_used + display_w > avail {
            break;
        }
        let in_sel = sel.map(|(s, e)| col >= s && col < e).unwrap_or(false);
        let in_search = !in_sel && search_matches.iter().any(|(s, e)| col >= *s && col < *e);
        let syntax_color = app
            .highlight_cache
            .as_ref()
            .and_then(|cache| cache.byte_colors.get(byte_off).copied())
            .flatten();
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        } else if in_search {
            queue!(
                out,
                SetBackgroundColor(Color::Yellow),
                SetForegroundColor(Color::Black)
            )?;
        } else if let Some(fg) = syntax_color {
            queue!(out, SetForegroundColor(fg))?;
        }
        if *c == '\t' {
            queue!(out, Print(" ".repeat(TAB_WIDTH)))?;
        } else {
            queue!(out, Print(c.to_string()))?;
        }
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reset))?;
        } else if in_search || syntax_color.is_some() {
            queue!(out, ResetColor)?;
        }
        visual_used += display_w;
        byte_off += c.len_utf8();
    }
    if chars.is_empty() {
        if let Some((s, e)) = sel {
            if s < e {
                queue!(
                    out,
                    SetAttribute(Attribute::Reverse),
                    Print(" "),
                    SetAttribute(Attribute::Reset)
                )?;
            }
        }
    }
    Ok(())
}

// Powerline-style status line: mode block | branch | path | …filler… | language tag.
// Nerd Font glyphs are used directly (user has Nerd Font in their terminal).
const PL_RIGHT: char = '\u{e0b0}'; // right-pointing arrow (filled)
const PL_LEFT: char = '\u{e0b2}'; // left-pointing arrow (filled)
const NF_BRANCH: char = '\u{e0a0}';

fn mode_color(mode: Mode) -> Color {
    match mode {
        Mode::Normal => Color::Rgb { r: 135, g: 175, b: 240 },
        Mode::Insert => Color::Rgb { r: 165, g: 220, b: 130 },
        Mode::Visual(VisualKind::Char) => Color::Rgb { r: 200, g: 130, b: 220 },
        Mode::Visual(VisualKind::Line) => Color::Rgb { r: 200, g: 130, b: 220 },
        Mode::Command => Color::Rgb { r: 240, g: 200, b: 110 },
        Mode::Search { .. } => Color::Rgb { r: 240, g: 200, b: 110 },
        Mode::Picker => Color::Rgb { r: 130, g: 220, b: 220 },
    }
}

fn lang_icon(lang: Lang) -> char {
    match lang {
        Lang::Rust => '\u{e7a8}',
        Lang::TypeScript | Lang::Tsx => '\u{e628}',
        Lang::Json => '\u{e60b}',
        Lang::Go => '\u{e627}',
        Lang::Markdown => '\u{e609}',
    }
}

fn lang_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rust",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::Json => "json",
        Lang::Go => "go",
        Lang::Markdown => "markdown",
    }
}

fn truncate_left(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max || max == 0 {
        return s.chars().take(max).collect();
    }
    if max == 1 {
        return "…".to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let start = count - (max - 1);
    let mut out = String::from("…");
    out.extend(chars[start..].iter());
    out
}

fn draw_status_line(out: &mut impl Write, app: &App) -> Result<()> {
    let row = (app.height as usize).saturating_sub(2) as u16;
    let total = app.width as usize;

    let mode_bg = mode_color(app.mode);
    let mode_fg = Color::Rgb { r: 20, g: 22, b: 32 };
    let branch_bg = Color::Rgb { r: 54, g: 60, b: 86 };
    let branch_fg = Color::Rgb { r: 200, g: 200, b: 220 };
    let path_bg = Color::Rgb { r: 36, g: 40, b: 56 };
    let path_fg = Color::Rgb { r: 170, g: 175, b: 195 };
    let right_bg = branch_bg;
    let right_fg = branch_fg;

    queue!(out, MoveTo(0, row), Clear(ClearType::CurrentLine))?;

    // Build segment strings first so we can size everything.
    let recording = app
        .recording_macro
        .map(|c| format!(" @{c}"))
        .unwrap_or_default();
    let mode_text = format!(" {}{} ", app.mode.label(), recording);
    let branch_text = app
        .git_branch
        .as_deref()
        .map(|b| format!(" {} {} ", NF_BRANCH, b))
        .unwrap_or_default();
    let dirty = if app.buffer.dirty { " " } else { " " };
    let lang = app.buffer.path.as_deref().and_then(Lang::detect);
    let right_text = match lang {
        Some(l) => format!(" {} {} ", lang_icon(l), lang_name(l)),
        None => String::new(),
    };

    // Width budget. Powerline arrows take 1 column each.
    let mode_w = mode_text.chars().count();
    let mode_arrow_w = 1;
    let branch_w = branch_text.chars().count();
    let branch_arrow_w = if branch_text.is_empty() { 0 } else { 1 };
    let right_w = right_text.chars().count();
    let right_arrow_w = if right_text.is_empty() { 0 } else { 1 };

    let path_used =
        mode_w + mode_arrow_w + branch_w + branch_arrow_w + right_arrow_w + right_w;
    let path_room = total
        .saturating_sub(path_used)
        .saturating_sub(2 + dirty.chars().count()); // surrounding spaces + dirty marker

    let path_str = match app.buffer.path.as_ref() {
        Some(p) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let display = match p.strip_prefix(&cwd) {
                Ok(rel) => rel.display().to_string(),
                Err(_) => p.display().to_string(),
            };
            truncate_left(&display, path_room.max(1))
        }
        None => "[No Name]".into(),
    };

    // === Mode segment ===
    queue!(
        out,
        SetBackgroundColor(mode_bg),
        SetForegroundColor(mode_fg),
        SetAttribute(Attribute::Bold),
        Print(&mode_text),
        SetAttribute(Attribute::Reset),
    )?;

    // mode → branch transition (or → path if no branch)
    if !branch_text.is_empty() {
        queue!(
            out,
            SetBackgroundColor(branch_bg),
            SetForegroundColor(mode_bg),
            Print(PL_RIGHT.to_string()),
            SetBackgroundColor(branch_bg),
            SetForegroundColor(branch_fg),
            Print(&branch_text),
            SetBackgroundColor(path_bg),
            SetForegroundColor(branch_bg),
            Print(PL_RIGHT.to_string()),
        )?;
    } else {
        queue!(
            out,
            SetBackgroundColor(path_bg),
            SetForegroundColor(mode_bg),
            Print(PL_RIGHT.to_string()),
        )?;
    }

    // === Path segment ===
    queue!(
        out,
        SetBackgroundColor(path_bg),
        SetForegroundColor(path_fg),
        Print(format!(" {}{} ", path_str, dirty)),
    )?;

    // Fill the middle with the path background.
    let drawn = mode_w
        + mode_arrow_w
        + (if branch_text.is_empty() { 0 } else { branch_w + branch_arrow_w })
        + 2
        + path_str.chars().count()
        + dirty.chars().count();
    let fill = total.saturating_sub(drawn + right_arrow_w + right_w);
    queue!(
        out,
        SetBackgroundColor(path_bg),
        Print(" ".repeat(fill)),
    )?;

    // === Right segment (language) ===
    if !right_text.is_empty() {
        queue!(
            out,
            SetBackgroundColor(right_bg),
            SetForegroundColor(path_bg),
            Print(PL_LEFT.to_string()),
            SetBackgroundColor(right_bg),
            SetForegroundColor(right_fg),
            Print(&right_text),
        )?;
    }

    queue!(out, ResetColor)?;
    Ok(())
}

fn draw_command_line(out: &mut impl Write, app: &App) -> Result<()> {
    let row = (app.height as usize).saturating_sub(1) as u16;
    queue!(out, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    match app.mode {
        Mode::Command => {
            queue!(out, Print(format!(":{}", app.cmdline)))?;
        }
        Mode::Search { backward } => {
            let prefix = if backward { '?' } else { '/' };
            queue!(out, Print(format!("{}{}", prefix, app.cmdline)))?;
        }
        _ => {
            if !app.status_msg.is_empty() {
                queue!(out, Print(&app.status_msg))?;
            } else if let Some(diag) = app.line_diagnostics(app.cursor.line).first() {
                let color = match diag.severity {
                    Severity::Error => Color::Red,
                    Severity::Warning => Color::Yellow,
                    Severity::Info => Color::Blue,
                    Severity::Hint => Color::DarkBlue,
                };
                let max = (app.width as usize).saturating_sub(2);
                let mut msg: String = diag.message.lines().next().unwrap_or("").to_string();
                if msg.chars().count() > max {
                    msg = msg.chars().take(max).collect();
                }
                queue!(
                    out,
                    SetForegroundColor(color),
                    Print(msg),
                    ResetColor
                )?;
            }
        }
    }
    Ok(())
}

fn place_cursor(out: &mut impl Write, app: &App) -> Result<()> {
    let style = match app.mode {
        Mode::Insert => SetCursorStyle::SteadyBar,
        _ => SetCursorStyle::SteadyBlock,
    };
    queue!(out, style, Show)?;
    if matches!(app.mode, Mode::Command | Mode::Search { .. }) {
        let row = (app.height as usize).saturating_sub(1) as u16;
        let col = (app.cmdline.chars().count() + 1) as u16;
        queue!(out, MoveTo(col, row))?;
        return Ok(());
    }
    if app.mode == Mode::Picker {
        if let Some(picker) = app.picker.as_ref() {
            let (top_row, _, _) = picker_layout(app);
            let input_row = (top_row + 1) as u16;
            let col = (picker.input.chars().count() + 2) as u16;
            queue!(out, SetCursorStyle::SteadyBar, MoveTo(col, input_row))?;
            return Ok(());
        }
    }
    let gutter = app.gutter_width();
    let row = app.cursor.line.saturating_sub(app.view_top) as u16;
    let line = app.buffer.rope.line(app.cursor.line);
    let mut visual = 0usize;
    for (i, c) in line.chars().enumerate() {
        if i >= app.cursor.col {
            break;
        }
        if c == '\t' {
            visual += TAB_WIDTH;
        } else {
            visual += 1;
        }
    }
    let col = (gutter + visual) as u16;
    queue!(out, MoveTo(col, row))?;
    Ok(())
}
