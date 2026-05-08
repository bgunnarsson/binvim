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
    draw_notification(out, app)?;
    if matches!(app.mode, Mode::Command | Mode::Search { .. }) {
        draw_floating_cmdline(out, app)?;
    }
    if app.mode == Mode::Picker {
        draw_picker(out, app)?;
    }
    if app.completion.is_some() {
        draw_completion_popup(out, app)?;
    }
    if app.hover.is_some() {
        draw_hover_popup(out, app)?;
    }
    if app.whichkey.is_some() {
        draw_whichkey(out, app)?;
    }
    place_cursor(out, app)?;
    queue!(out, EndSynchronizedUpdate)?;
    Ok(())
}

fn draw_whichkey(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(wk) = app.whichkey.as_ref() else { return Ok(()); };
    if wk.entries.is_empty() {
        return Ok(());
    }

    // Width budget: every row prints exactly `popup_w` chars.
    //   top    = '╭' + '─'... + '╮'
    //   body   = '│' + ' ' + key (right-pad to key_w) + ' → ' + label + trail + '│'
    //   footer = '│' + " ESC close " + pad + '│'
    //   bottom = '╰' + '─'... + '╯'
    // Define `content_w = popup_w - 2` — chars strictly between the side borders.
    // The body row needs at least: 1 (left pad) + key_w + 3 (" → ") + label_w + 0 trail = key_w + label_w + 4.
    let key_w = wk.entries.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(1);
    let label_w = wk.entries.iter().map(|(_, l)| l.chars().count()).max().unwrap_or(1);
    let entry_min = key_w + label_w + 4;
    let title_min = wk.title.chars().count() + 4; // some breathing space around the title
    let footer_min = " ESC close ".chars().count();
    let mut content_w = entry_min.max(title_min).max(footer_min);
    let total_w = app.width as usize;
    if content_w + 2 > total_w.saturating_sub(4) {
        content_w = total_w.saturating_sub(6);
    }
    let popup_w = content_w + 2;
    let popup_h = wk.entries.len() + 3; // top + N entries + footer + bottom

    let total_h = app.height as usize;
    let popup_h = popup_h.min(total_h.saturating_sub(2));
    let max_entries = popup_h.saturating_sub(3);

    let left = total_w.saturating_sub(popup_w) / 2;
    let top = total_h.saturating_sub(popup_h) / 2;

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let border = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }; // Surface2
    let title_fg = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let key_fg = Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 }; // Mauve
    let label_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let arrow_fg = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }; // Overlay0
    let hint_fg = Color::Rgb { r: 0x9e, g: 0xa3, b: 0xb6 };

    // ── Top border ───────────────────────────────────────────────────────
    let title_text = format!(" {} ", wk.title);
    let title_len = title_text.chars().count();
    let pre = content_w.saturating_sub(title_len) / 2;
    let post = content_w.saturating_sub(title_len + pre);
    queue!(
        out,
        MoveTo(left as u16, top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print("─".repeat(pre)),
        SetForegroundColor(title_fg),
        SetAttribute(Attribute::Bold),
        Print(&title_text),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(post)),
        Print('╮'),
    )?;

    // ── Entry rows ───────────────────────────────────────────────────────
    for (i, (key, label)) in wk.entries.iter().take(max_entries).enumerate() {
        let key_chars = key.chars().count();
        let key_pad = key_w.saturating_sub(key_chars);
        let label_max = content_w.saturating_sub(key_w + 4);
        let label_trunc: String = label.chars().take(label_max).collect();
        let label_chars = label_trunc.chars().count();
        let trail = content_w.saturating_sub(key_w + 4 + label_chars);
        queue!(
            out,
            MoveTo(left as u16, (top + 1 + i) as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
            // Inside-the-borders content: 1 + key_w + 3 + label + trail = content_w.
            Print(' '),
            Print(" ".repeat(key_pad)),
            SetForegroundColor(key_fg),
            Print(key),
            SetForegroundColor(arrow_fg),
            Print(" → "),
            SetForegroundColor(label_fg),
            Print(&label_trunc),
            Print(" ".repeat(trail)),
            SetForegroundColor(border),
            Print('│'),
        )?;
    }

    // ── Footer hint row ──────────────────────────────────────────────────
    let footer_row = top + 1 + max_entries.min(wk.entries.len());
    let hint = " ESC close ";
    let hint_chars = hint.chars().count();
    let hint_pad = content_w.saturating_sub(hint_chars);
    queue!(
        out,
        MoveTo(left as u16, footer_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        SetForegroundColor(hint_fg),
        Print(hint),
        Print(" ".repeat(hint_pad)),
        SetForegroundColor(border),
        Print('│'),
    )?;

    // ── Bottom border ────────────────────────────────────────────────────
    queue!(
        out,
        MoveTo(left as u16, (footer_row + 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(content_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

fn draw_hover_popup(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(hover) = app.hover.as_ref() else { return Ok(()); };
    if hover.lines.is_empty() {
        return Ok(());
    }

    // Width: lines were word-wrapped at hover.wrap_width, so use that.
    let widest_actual = hover.lines.iter().map(|l| l.chars().count()).max().unwrap_or(20);
    let content_w = widest_actual.min(hover.wrap_width).max(20);
    let popup_w = content_w + 2;

    // Height: cap at HOVER_MAX_HEIGHT, also cap at half the screen.
    let total_h = app.height as usize;
    let max_visible = crate::app::HOVER_MAX_HEIGHT
        .min(total_h.saturating_sub(4).max(4));
    let visible = hover.lines.len().min(max_visible);
    let popup_h = visible + 2;

    // Position: prefer below cursor; flip above if overflow.
    let buffer_rows = app.buffer_rows();
    let cursor_row = app.cursor.line.saturating_sub(app.view_top);
    let mut top_row = cursor_row + 1;
    if top_row + popup_h > buffer_rows {
        top_row = cursor_row.saturating_sub(popup_h);
    }
    let gutter = app.gutter_width();
    let mut left_col = gutter + app.cursor.col;
    if left_col + popup_w > app.width as usize {
        left_col = (app.width as usize).saturating_sub(popup_w);
    }

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let border = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }; // Surface2
    let text_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let title_fg = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let arrow_fg = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }; // Overlay0

    // Top border with title (and a "start-end/total" scroll indicator on the right).
    let total = hover.lines.len();
    let scroll_label = if total > visible {
        let first = hover.scroll + 1;
        let last = (hover.scroll + visible).min(total);
        format!(" {}-{}/{} ", first, last, total)
    } else {
        String::new()
    };
    let title = " hover ";
    let title_w = title.chars().count();
    let scroll_w = scroll_label.chars().count();
    let dashes_total = content_w.saturating_sub(title_w + scroll_w);
    let pre = dashes_total / 2;
    let post = dashes_total - pre;
    queue!(
        out,
        MoveTo(left_col as u16, top_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print("─".repeat(pre)),
        SetForegroundColor(title_fg),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(post)),
        SetForegroundColor(arrow_fg),
        Print(&scroll_label),
        SetForegroundColor(border),
        Print('╮'),
    )?;

    // Body — show `visible` lines starting at `scroll`.
    for i in 0..visible {
        let idx = hover.scroll + i;
        let line = hover.lines.get(idx).map(|s| s.as_str()).unwrap_or("");
        let truncated: String = line.chars().take(content_w).collect();
        let pad = content_w.saturating_sub(truncated.chars().count());
        queue!(
            out,
            MoveTo(left_col as u16, (top_row + 1 + i) as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
            SetForegroundColor(text_fg),
            Print(&truncated),
            Print(" ".repeat(pad)),
            SetForegroundColor(border),
            Print('│'),
        )?;
    }

    // Bottom border.
    queue!(
        out,
        MoveTo(left_col as u16, (top_row + 1 + visible) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(content_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

/// Classify a status message by content into a Catppuccin severity colour. We
/// avoid threading a level enum through every callsite by reading the prefix
/// patterns at render time.
fn notification_color(msg: &str) -> Color {
    let lower = msg.to_lowercase();
    // Error: "error: <foo>" or vim-style E37 / E89 / E492…
    let vim_error = msg
        .strip_prefix('E')
        .map(|rest| {
            let digits: usize = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            digits >= 1 && rest[..digits].len() <= 4 && rest[digits..].starts_with(':')
        })
        .unwrap_or(false);
    if lower.starts_with("error:") || vim_error {
        return Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }; // Red
    }
    // Success: file write, substitution count, range yank / delete summaries.
    if lower.contains("written")
        || lower.contains("substitution")
        || lower.ends_with(" yanked")
        || lower.ends_with(" deleted")
        || lower.starts_with("recorded ")
        || lower.starts_with("kept buffer")
    {
        return Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 }; // Green
    }
    // Warning: not-found, no-such, edge-of-history.
    if lower.contains("not found")
        || lower.contains("no write")
        || lower.contains("no previous")
        || lower.contains("already at")
        || lower.contains("only one")
        || lower.contains("empty ")
        || lower.contains("not on path")
        || lower.contains("no word")
        || lower.contains("no files")
        || lower.contains("buffer closed")
    {
        return Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }; // Yellow
    }
    Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa } // Blue — info default
}

fn draw_notification(out: &mut impl Write, app: &App) -> Result<()> {
    // Cmdline and search modes get the centred box; their floating widget covers any notification.
    if matches!(app.mode, Mode::Command | Mode::Search { .. }) {
        return Ok(());
    }
    if app.status_msg.is_empty() {
        return Ok(());
    }
    let msg = truncate_oneline(&app.status_msg);
    if msg.is_empty() {
        return Ok(());
    }
    let level = notification_color(&msg);

    let max_inner = (app.width as usize).saturating_sub(8).max(20);
    let inner: String = msg.chars().take(max_inner).collect();
    let inner_w = inner.chars().count() + 2; // padding inside borders
    let box_w = inner_w + 2;
    let total_w = app.width as usize;
    let left = total_w.saturating_sub(box_w + 1);
    let top = 0usize;

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle

    queue!(
        out,
        MoveTo(left as u16, top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(level),
        Print('╭'),
        Print("─".repeat(inner_w)),
        Print('╮'),
    )?;
    let text_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Catppuccin Text
    queue!(
        out,
        MoveTo(left as u16, (top + 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(level),
        Print('│'),
        SetForegroundColor(text_fg),
        Print(format!(" {} ", inner)),
        SetForegroundColor(level),
        Print('│'),
    )?;
    queue!(
        out,
        MoveTo(left as u16, (top + 2) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(level),
        Print('╰'),
        Print("─".repeat(inner_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

fn truncate_oneline(s: &str) -> String {
    let one = s.lines().next().unwrap_or("").to_string();
    one
}

/// Layout for the floating command line — returns (left_col, top_row, width).
fn cmdline_box_layout(app: &App) -> (usize, usize, usize) {
    let total_w = app.width as usize;
    let total_h = app.height as usize;
    let box_w = total_w.saturating_sub(20).min(60).max(24);
    let left = total_w.saturating_sub(box_w) / 2;
    let top = (total_h * 4 / 10).max(2);
    (left, top, box_w)
}

/// Mode → (title, prompt char). Prompt is `>` for `:` and shows direction for search.
fn cmdline_chrome(mode: Mode) -> (&'static str, char) {
    match mode {
        Mode::Command => ("binvim", '>'),
        Mode::Search { backward: false } => ("Search", '/'),
        Mode::Search { backward: true } => ("Search", '?'),
        _ => ("", ' '),
    }
}

fn draw_floating_cmdline(out: &mut impl Write, app: &App) -> Result<()> {
    let (left, top, box_w) = cmdline_box_layout(app);
    let (title, prompt) = cmdline_chrome(app.mode);
    let inner_w = box_w.saturating_sub(2);

    let border = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }; // Surface2
    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let title_fg = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let prompt_fg = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa }; // Blue
    let text_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text

    // Top border with centred title.
    let title_text = format!(" {} ", title);
    let title_w = title_text.chars().count();
    let left_pad = inner_w.saturating_sub(title_w) / 2;
    let right_pad = inner_w.saturating_sub(title_w + left_pad);

    queue!(
        out,
        MoveTo(left as u16, top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print("─".repeat(left_pad)),
        SetForegroundColor(title_fg),
        SetAttribute(Attribute::Bold),
        Print(&title_text),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(right_pad)),
        Print('╮'),
    )?;

    // Input row.
    let input: String = app.cmdline.chars().take(inner_w.saturating_sub(4)).collect();
    let pad = inner_w
        .saturating_sub(3)
        .saturating_sub(input.chars().count());
    queue!(
        out,
        MoveTo(left as u16, (top + 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        SetForegroundColor(prompt_fg),
        SetAttribute(Attribute::Bold),
        Print(format!(" {} ", prompt)),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(text_fg),
        Print(&input),
        Print(" ".repeat(pad)),
        SetForegroundColor(border),
        Print('│'),
    )?;

    // Bottom border.
    queue!(
        out,
        MoveTo(left as u16, (top + 2) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(inner_w)),
        Print('╯'),
        ResetColor,
    )?;

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
                SetBackgroundColor(Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }), // Surface1
                SetForegroundColor(Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe })  // Lavender
            )?;
        } else {
            queue!(
                out,
                SetBackgroundColor(Color::Rgb { r: 0x31, g: 0x32, b: 0x44 }), // Surface0
                SetForegroundColor(Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 })  // Text
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
        SetForegroundColor(Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }), // Peach
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
                SetBackgroundColor(Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }), // Surface1
                SetForegroundColor(Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe })  // Lavender
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
                Severity::Error => ('!', Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }), // Red
                Severity::Warning => ('?', Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }), // Yellow
                Severity::Info => ('i', Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa }), // Blue
                Severity::Hint => ('h', Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }), // Sky
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
                SetForegroundColor(Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }), // Overlay0
                Print(format!("{:>width$} ", line_idx + 1, width = gutter - 2)),
                ResetColor
            )?;
            draw_line_with_selection(out, app, line_idx, avail)?;
        } else {
            queue!(
                out,
                SetForegroundColor(Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }), // Surface1
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
    let dim = app.has_modal_overlay();
    let dim_color = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }; // Overlay0
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
                SetBackgroundColor(Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }), // Yellow
                SetForegroundColor(Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e })  // Base
            )?;
        } else if dim {
            // Modal mode: drop syntax colour, render everything muted.
            queue!(out, SetForegroundColor(dim_color))?;
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
        } else if in_search || syntax_color.is_some() || dim {
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

    // Error Lens-style inline diagnostic at the end of the line.
    if !dim {
        if let Some(diag) = app.line_diagnostics(line_idx).first() {
            let remaining = avail.saturating_sub(visual_used);
            if remaining > 4 {
                let (icon, color) = match diag.severity {
                    Severity::Error => ('●', Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }), // Red
                    Severity::Warning => ('▲', Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }), // Yellow
                    Severity::Info => ('●', Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa }), // Blue
                    Severity::Hint => ('●', Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }), // Sky
                };
                // Spacing: "  ● " (2 leading + icon + space) → 4 cols.
                let prefix_w = 4usize;
                let text_room = remaining.saturating_sub(prefix_w);
                let mut msg: String = diag.message.lines().next().unwrap_or("").to_string();
                if msg.chars().count() > text_room {
                    msg = msg.chars().take(text_room).collect();
                }
                queue!(
                    out,
                    SetForegroundColor(color),
                    SetAttribute(Attribute::Italic),
                    Print(format!("  {} {}", icon, msg)),
                    SetAttribute(Attribute::NoItalic),
                    ResetColor
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

// Catppuccin Mocha mode block colours (matches the syntax-highlighting palette).
fn mode_color(mode: Mode) -> Color {
    match mode {
        Mode::Normal => Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }, // Lavender
        Mode::Insert => Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 }, // Green
        Mode::Visual(VisualKind::Char) => Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 }, // Mauve
        Mode::Visual(VisualKind::Line) => Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 }, // Mauve
        Mode::Command => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        Mode::Search { .. } => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        Mode::Picker => Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }, // Sky
    }
}

fn lang_icon(lang: Lang) -> char {
    match lang {
        Lang::Rust => '\u{e7a8}',
        Lang::TypeScript | Lang::Tsx => '\u{e628}',
        Lang::JavaScript => '\u{e60c}',
        Lang::Json => '\u{e60b}',
        Lang::Go => '\u{e627}',
        Lang::Html => '\u{e60e}',
        Lang::Css => '\u{e749}',
        Lang::Markdown => '\u{e609}',
        Lang::CSharp => '\u{f81a}',
    }
}

fn lang_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rust",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::JavaScript => "javascript",
        Lang::Json => "json",
        Lang::Go => "go",
        Lang::Html => "html",
        Lang::Css => "css",
        Lang::Markdown => "markdown",
        Lang::CSharp => "csharp",
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
    let row = (app.height as usize).saturating_sub(1) as u16;
    let total = app.width as usize;

    let mode_bg = mode_color(app.mode);
    let mode_fg = Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e }; // Base
    let branch_bg = Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }; // Surface1
    let branch_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let path_bg = Color::Rgb { r: 0x31, g: 0x32, b: 0x44 }; // Surface0
    let path_fg = Color::Rgb { r: 0xa6, g: 0xad, b: 0xc8 }; // Subtext0
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


fn place_cursor(out: &mut impl Write, app: &App) -> Result<()> {
    let style = match app.mode {
        Mode::Insert => SetCursorStyle::SteadyBar,
        _ => SetCursorStyle::SteadyBlock,
    };
    queue!(out, style, Show)?;
    if matches!(app.mode, Mode::Command | Mode::Search { .. }) {
        let (left, top, _) = cmdline_box_layout(app);
        // Box layout:  │ <prompt> <input>   │  → cursor at left + 4 + len(input)
        let col = left + 4 + app.cmdline.chars().count();
        queue!(
            out,
            SetCursorStyle::SteadyBar,
            MoveTo(col as u16, (top + 1) as u16)
        )?;
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
