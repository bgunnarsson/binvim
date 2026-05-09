use crate::app::App;
use crate::lang::Lang;
use crate::lsp::Severity;
use crate::mode::{Mode, VisualKind};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor, SetUnderlineColor},
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use std::io::Write;

pub const TAB_WIDTH: usize = 4;

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
    if app.signature_help.is_some() {
        draw_signature_popup(out, app)?;
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

fn draw_signature_popup(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(sig) = app.signature_help.as_ref() else { return Ok(()); };
    if sig.label.is_empty() {
        return Ok(());
    }
    let chars: Vec<char> = sig.label.chars().collect();
    let label_w = chars.len();
    let total_w = app.width as usize;
    let max_inner = total_w.saturating_sub(8).max(20);
    let inner_w = label_w.min(max_inner);
    if inner_w == 0 {
        return Ok(());
    }
    let popup_w = inner_w + 2;
    let popup_h = 3usize; // top + label + bottom

    let buffer_rows = app.buffer_rows();
    let cursor_row = app.cursor.line.saturating_sub(app.view_top);
    // Prefer above the cursor — call sites have the popup above so it
    // doesn't cover the line you're typing into.
    let mut top_row = cursor_row.saturating_sub(popup_h);
    if cursor_row < popup_h {
        top_row = (cursor_row + 1).min(buffer_rows.saturating_sub(popup_h));
    }
    let gutter = app.gutter_width();
    let cursor_visual = app.cursor_visual_col().saturating_sub(app.view_left);
    let mut left_col = gutter + cursor_visual;
    if left_col + popup_w > total_w {
        left_col = total_w.saturating_sub(popup_w);
    }

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let border = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }; // Surface2
    let text_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let active_fg = Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e }; // Base
    let active_bg = Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }; // Yellow

    queue!(
        out,
        MoveTo(left_col as u16, top_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print("─".repeat(inner_w)),
        Print('╮'),
        MoveTo(left_col as u16, (top_row + 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
    )?;

    // Render the label, highlighting the active parameter range if known.
    // Truncate to inner_w characters.
    let visible = chars.iter().take(inner_w).copied().collect::<Vec<_>>();
    let active = sig.active_param;
    for (i, ch) in visible.iter().enumerate() {
        let in_active = active.map(|(s, e)| i >= s && i < e).unwrap_or(false);
        if in_active {
            queue!(
                out,
                SetBackgroundColor(active_bg),
                SetForegroundColor(active_fg),
                Print(ch.to_string()),
                SetBackgroundColor(bg),
            )?;
        } else {
            queue!(out, SetForegroundColor(text_fg), Print(ch.to_string()))?;
        }
    }
    let pad = inner_w.saturating_sub(visible.len());
    if pad > 0 {
        queue!(out, Print(" ".repeat(pad)))?;
    }
    queue!(
        out,
        SetForegroundColor(border),
        Print('│'),
        MoveTo(left_col as u16, (top_row + 2) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(inner_w)),
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

/// Maximum content rows in a notification box before extra wrapped lines
/// get truncated with an ellipsis. Six is enough to read a typical path or
/// error message without letting a stack trace eat the whole screen.
const NOTIFICATION_MAX_ROWS: usize = 6;

fn draw_notification(out: &mut impl Write, app: &App) -> Result<()> {
    // Cmdline and search modes get the centred box; their floating widget covers any notification.
    if matches!(app.mode, Mode::Command | Mode::Search { .. }) {
        return Ok(());
    }
    if app.status_msg.is_empty() {
        return Ok(());
    }
    let level = notification_color(&app.status_msg);

    // Cap the notification at half the terminal width so long messages
    // (file paths, stack traces) don't span the whole screen. The box adds
    // 4 chrome columns: 2 borders + 2 padding spaces.
    let total_w = app.width as usize;
    let half_inner = (total_w / 2).saturating_sub(4);
    let term_inner = total_w.saturating_sub(8);
    let max_inner = half_inner.min(term_inner).max(20);

    let mut wrapped = wrap_notification(&app.status_msg, max_inner);
    if wrapped.is_empty() {
        return Ok(());
    }
    if wrapped.len() > NOTIFICATION_MAX_ROWS {
        wrapped.truncate(NOTIFICATION_MAX_ROWS);
        let last = wrapped.last_mut().unwrap();
        let kept = last.chars().count().saturating_sub(1);
        let mut s: String = last.chars().take(kept).collect();
        s.push('…');
        *last = s;
    }
    let inner_w = wrapped
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        + 2; // padding inside borders
    let box_w = inner_w + 2;
    let left = total_w.saturating_sub(box_w + 1);
    let top = 0usize;

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let text_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Catppuccin Text

    // Top border
    queue!(
        out,
        MoveTo(left as u16, top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(level),
        Print('╭'),
        Print("─".repeat(inner_w)),
        Print('╮'),
    )?;

    // Content rows
    for (i, line) in wrapped.iter().enumerate() {
        let line_chars = line.chars().count();
        let pad = (inner_w.saturating_sub(2)).saturating_sub(line_chars);
        queue!(
            out,
            MoveTo(left as u16, (top + 1 + i) as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(level),
            Print('│'),
            SetForegroundColor(text_fg),
            Print(format!(" {} ", line)),
            Print(" ".repeat(pad)),
            SetForegroundColor(level),
            Print('│'),
        )?;
    }

    // Bottom border
    queue!(
        out,
        MoveTo(left as u16, (top + 1 + wrapped.len()) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(level),
        Print('╰'),
        Print("─".repeat(inner_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

/// Break `msg` into rows of at most `width` chars. Honours embedded newlines
/// — each `\n`-separated segment wraps independently. Wrapping is purely
/// character-based: paths and stack traces have few spaces to break on, and
/// preserving structure visually matters more than typographic word wrap.
fn wrap_notification(msg: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    if width == 0 {
        return out;
    }
    for raw in msg.lines() {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let chars: Vec<char> = raw.chars().collect();
        let mut idx = 0;
        while idx < chars.len() {
            let end = (idx + width).min(chars.len());
            out.push(chars[idx..end].iter().collect());
            idx = end;
        }
    }
    out
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

const START_LOGO: &[&str] = &[
    "██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗",
    "██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║",
    "██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║",
    "██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    "██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    "╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];

fn draw_start_page(out: &mut impl Write, app: &App) -> Result<()> {
    let rows = app.buffer_rows();
    let total_w = app.width as usize;
    // Blank every row so leftover content from a prior frame can't bleed through.
    for row in 0..rows {
        queue!(out, MoveTo(0, row as u16), Clear(ClearType::CurrentLine))?;
    }
    // User config wins; fall back to the baked-in logo when no override is set.
    let configured: Vec<&str> = app
        .config
        .start_page
        .lines
        .iter()
        .map(|s| s.as_str())
        .collect();
    let lines: &[&str] = if configured.is_empty() {
        START_LOGO
    } else {
        &configured
    };
    let block_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    if block_w == 0 || block_w > total_w {
        return Ok(());
    }
    let block_h = lines.len();
    if block_h > rows {
        return Ok(());
    }
    let top = (rows.saturating_sub(block_h)) / 2;
    let blue = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa };
    for (i, line) in lines.iter().enumerate() {
        let line_w = line.chars().count();
        let left = (total_w.saturating_sub(line_w)) / 2;
        queue!(
            out,
            MoveTo(left as u16, (top + i) as u16),
            SetForegroundColor(blue),
            Print(line),
            ResetColor,
        )?;
    }
    Ok(())
}

fn draw_buffer(out: &mut impl Write, app: &App) -> Result<()> {
    if app.show_start_page {
        return draw_start_page(out, app);
    }
    let rows = app.buffer_rows();
    let gutter = app.gutter_width();
    let avail = (app.width as usize).saturating_sub(gutter);
    for row in 0..rows {
        let line_idx = app.view_top + row;
        // Clear the row before drawing — guards against terminal-side wrap
        // from the previous row's render leaking onto this one.
        queue!(out, MoveTo(0, row as u16), Clear(ClearType::CurrentLine))?;
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
    let yank_flash = app.line_yank_highlight(line_idx);
    let match_pair = app.line_match_pair(line_idx);
    let line_byte_start = app.buffer.rope.line_to_byte(line_idx);
    let chars: Vec<char> = text.chars().collect();
    let view_left = app.view_left;
    // Visual column from the start of the line — tracks where each char
    // would land if `view_left == 0`. Subtract `view_left` to get the
    // on-screen column.
    let mut line_visual_pos = 0usize;
    // Visual columns actually written to the terminal in this pass.
    let mut visual_used = 0usize;
    let mut byte_off = line_byte_start;
    let dim = app.has_modal_overlay();
    let dim_color = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }; // Overlay0
    // `:set list` equivalent — render every space as `·`, every tab as
    // `→` + filler, every non-breaking space as `⎵`, and the end-of-line
    // as `¬`. All in the muted overlay colour. Configurable via
    // `[whitespace]` in config.toml; on by default.
    let show_hidden = app.config.whitespace.show;
    // Precompute per-column severity from the LSP's diagnostic ranges so we
    // can paint an undercurl directly under the offending tokens.
    let line_diags = app.line_diagnostics(line_idx);
    let mut diag_at: Vec<Option<Severity>> = vec![None; chars.len()];
    for d in &line_diags {
        let start = d.col.min(chars.len());
        let end = if d.end_line == d.line {
            d.end_col.min(chars.len())
        } else {
            chars.len()
        };
        // Empty ranges (start == end) come from LSPs as a hint that the error
        // sits at one position; mark a single column so the undercurl shows.
        let span = if end > start { end } else { (start + 1).min(chars.len()) };
        for slot in &mut diag_at[start..span] {
            *slot = Some(merge_severity(*slot, d.severity));
        }
    }
    let mut clipped_right = false;
    for (col, c) in chars.iter().enumerate() {
        let display_w = if *c == '\t' { TAB_WIDTH } else { 1 };
        let char_visual_end = line_visual_pos + display_w;
        // Entirely off the left edge — advance trackers, render nothing.
        if char_visual_end <= view_left {
            line_visual_pos = char_visual_end;
            byte_off += c.len_utf8();
            continue;
        }
        // Visible window for this char. `visible_left == line_visual_pos`
        // when the char isn't clipped on the left; otherwise it's the
        // viewport edge mid-tab.
        let visible_left = line_visual_pos.max(view_left);
        let visible_right = char_visual_end.min(view_left + avail);
        if visible_right <= visible_left {
            clipped_right = true;
            break;
        }
        let visible_w = visible_right - visible_left;
        if visual_used + visible_w > avail {
            clipped_right = true;
            break;
        }
        let in_sel = sel.map(|(s, e)| col >= s && col < e).unwrap_or(false);
        let in_search = !in_sel && search_matches.iter().any(|(s, e)| col >= *s && col < *e);
        let in_yank_flash = !in_sel
            && !in_search
            && yank_flash.map(|(s, e)| col >= s && col < e).unwrap_or(false);
        let in_match_pair = !in_sel
            && !in_search
            && !in_yank_flash
            && match_pair.iter().any(|(s, e)| col >= *s && col < *e);
        let syntax_color = app
            .highlight_cache
            .as_ref()
            .and_then(|cache| cache.byte_colors.get(byte_off).copied())
            .flatten();
        let diag_severity = if !in_sel && !in_search && !dim {
            diag_at.get(col).copied().flatten()
        } else {
            None
        };
        let render_hidden = show_hidden && (*c == '\t' || *c == ' ' || *c == '\u{00A0}');
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        } else if in_search {
            queue!(
                out,
                SetBackgroundColor(Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }), // Yellow
                SetForegroundColor(Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e })  // Base
            )?;
        } else if in_yank_flash {
            // Distinct Peach flash — different from search Yellow so the two
            // never visually collide on shared text.
            queue!(
                out,
                SetBackgroundColor(Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }), // Peach
                SetForegroundColor(Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e })  // Base
            )?;
        } else if in_match_pair {
            // Subtle Surface2 background so the syntax-coloured foreground
            // still shows through, plus Bold so the bracket/tag pops.
            queue!(
                out,
                SetBackgroundColor(Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }), // Surface2
                SetAttribute(Attribute::Bold)
            )?;
        } else if render_hidden {
            // Whitespace marker overrides whatever syntax colour the
            // highlight cache would have used — these glyphs need to read
            // as chrome, not code.
            queue!(out, SetForegroundColor(dim_color))?;
        } else if dim {
            // Modal mode: drop syntax colour, render everything muted.
            queue!(out, SetForegroundColor(dim_color))?;
        } else if let Some(fg) = syntax_color {
            queue!(out, SetForegroundColor(fg))?;
        }
        if let Some(sev) = diag_severity {
            let underline = severity_color(sev);
            queue!(
                out,
                SetUnderlineColor(underline),
                SetAttribute(Attribute::Undercurled)
            )?;
        }
        if *c == '\t' {
            // For tabs that straddle `view_left`, the leading `→` glyph
            // gets clipped — only render space-fill for the visible width.
            let arrow_visible = show_hidden && line_visual_pos >= view_left;
            if arrow_visible {
                let fill = visible_w.saturating_sub(1);
                queue!(out, Print('→'), Print(" ".repeat(fill)))?;
            } else {
                queue!(out, Print(" ".repeat(visible_w)))?;
            }
        } else if *c == ' ' && show_hidden {
            queue!(out, Print('·'))?;
        } else if *c == '\u{00A0}' && show_hidden {
            queue!(out, Print('⎵'))?;
        } else {
            queue!(out, Print(c.to_string()))?;
        }
        if diag_severity.is_some() {
            queue!(out, SetAttribute(Attribute::NoUnderline))?;
        }
        if in_sel {
            queue!(out, SetAttribute(Attribute::Reset))?;
        } else if in_match_pair {
            // Tear down the bold + bg in one shot.
            queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        } else if in_search || in_yank_flash || syntax_color.is_some() || dim || render_hidden {
            queue!(out, ResetColor)?;
        }
        visual_used += visible_w;
        line_visual_pos = char_visual_end;
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

    // EOL marker — sits at the column right after the last char so the user
    // can see where lines actually end (vs. trailing whitespace). Only when
    // the entire line content fit; if we clipped right we're already at the
    // edge, and the marker wouldn't be at the line's actual end anyway.
    if show_hidden && !clipped_right && visual_used + 1 <= avail {
        queue!(
            out,
            SetForegroundColor(dim_color),
            Print('¬'),
            ResetColor
        )?;
    }

    // Error Lens-style inline diagnostic at the end of the line. We carefully
    // measure the prefix and message in display columns (not chars), and leave
    // one column of slack so any width-miscount can't push past the row edge
    // and force the terminal to wrap onto the next row — which would clobber
    // the next line's render with the diagnostic's tail.
    if !dim {
        if let Some(diag) = app.line_diagnostics(line_idx).first() {
            use unicode_width::UnicodeWidthChar;
            let remaining = avail.saturating_sub(visual_used);
            let icon = match diag.severity {
                Severity::Warning => '▲',
                _ => '●',
            };
            let color = severity_color(diag.severity);
            // "  <icon> " — leading 2 spaces, icon (1 or 2 cols), trailing space.
            let icon_w = UnicodeWidthChar::width(icon).unwrap_or(1);
            let prefix_w = 2 + icon_w + 1;
            let safety = 1usize; // never write the very last column
            if remaining > prefix_w + safety {
                let text_budget = remaining - prefix_w - safety;
                let raw = diag.message.lines().next().unwrap_or("");
                let mut used = 0usize;
                let mut msg = String::new();
                for c in raw.chars() {
                    let w = UnicodeWidthChar::width(c).unwrap_or(0);
                    if used + w > text_budget {
                        break;
                    }
                    msg.push(c);
                    used += w;
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

/// Catppuccin Mocha colour assignment per LSP severity. Used for both the
/// undercurl on the offending range and the inline Error Lens icon so the
/// two visuals match on the same line.
fn severity_color(sev: Severity) -> Color {
    match sev {
        Severity::Error => Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 },   // Red
        Severity::Warning => Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }, // Yellow
        Severity::Info => Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa },    // Blue
        Severity::Hint => Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb },    // Sky
    }
}

/// Pick the more important of two severities for ranges that overlap on the
/// same column — Error > Warning > Info > Hint.
fn merge_severity(prior: Option<Severity>, new: Severity) -> Severity {
    let rank = |s: Severity| match s {
        Severity::Error => 4,
        Severity::Warning => 3,
        Severity::Info => 2,
        Severity::Hint => 1,
    };
    match prior {
        Some(p) if rank(p) >= rank(new) => p,
        _ => new,
    }
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

/// Status-line tag for a file. Falls through to (lang_icon, lang_name) by
/// default, but overrides the label by extension where needed — .cshtml and
/// .razor both highlight as HTML internally but should announce as "razor".
fn lang_label(path: Option<&std::path::Path>, lang: Lang) -> (char, &'static str) {
    if let Some(ext) = path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        match ext {
            "cshtml" | "razor" => return ('\u{f81a}', "razor"),
            _ => {}
        }
    }
    (lang_icon(lang), lang_name(lang))
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
    let path = app.buffer.path.as_deref();
    let lang = path.and_then(Lang::detect);
    let right_text = match lang {
        Some(l) => {
            let (icon, name) = lang_label(path, l);
            format!(" {} {} ", icon, name)
        }
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
    // On the start page, no buffer cursor — only the cmdline/picker overlays
    // ever steal focus from the logo.
    if app.show_start_page
        && !matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Picker)
    {
        queue!(out, Hide)?;
        return Ok(());
    }
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
    // Account for horizontal scroll. `adjust_viewport` keeps the cursor
    // within `[view_left, view_left + buffer_cols)`, so subtraction is safe.
    let on_screen = visual.saturating_sub(app.view_left);
    let col = (gutter + on_screen) as u16;
    queue!(out, MoveTo(col, row))?;
    Ok(())
}
