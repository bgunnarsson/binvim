use crate::app::App;
use crate::mode::Mode;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::Write;

const TAB_WIDTH: usize = 4;

pub fn draw(out: &mut impl Write, app: &App) -> Result<()> {
    queue!(out, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    draw_buffer(out, app)?;
    draw_status_line(out, app)?;
    draw_command_line(out, app)?;
    place_cursor(out, app)?;
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
            queue!(
                out,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("{:>width$} ", line_idx + 1, width = gutter - 1)),
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
    let chars: Vec<char> = text.chars().collect();
    let mut visual_used = 0usize;
    for (col, c) in chars.iter().enumerate() {
        let display_w = if *c == '\t' { TAB_WIDTH } else { 1 };
        if visual_used + display_w > avail {
            break;
        }
        let in_sel = sel.map(|(s, e)| col >= s && col < e).unwrap_or(false);
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        if *c == '\t' {
            queue!(out, Print(" ".repeat(TAB_WIDTH)))?;
        } else {
            queue!(out, Print(c.to_string()))?;
        }
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reset))?;
        }
        visual_used += display_w;
    }
    // Empty line in visual mode: show a single highlighted space so the line is visible.
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

fn draw_status_line(out: &mut impl Write, app: &App) -> Result<()> {
    let row = (app.height as usize).saturating_sub(2) as u16;
    let name = app
        .buffer
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[No Name]".into());
    let dirty = if app.buffer.dirty { " [+]" } else { "" };
    let mode = app.mode.label();
    let pos = format!("{}:{}", app.cursor.line + 1, app.cursor.col + 1);
    let recording_tag = match app.recording_macro {
        Some(c) => format!("rec @{}  ", c),
        None => String::new(),
    };
    let left = format!(" {} | {}{}", mode, name, dirty);
    let right = format!("{}{} ", recording_tag, pos);
    let total = app.width as usize;
    let left_count = left.chars().count();
    let right_count = right.chars().count();
    let pad = total.saturating_sub(left_count + right_count);
    let line = format!("{}{}{}", left, " ".repeat(pad), right);
    queue!(
        out,
        MoveTo(0, row),
        SetAttribute(Attribute::Reverse),
        Print(line),
        SetAttribute(Attribute::Reset)
    )?;
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
