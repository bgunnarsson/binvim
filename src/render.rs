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
    if app.show_tabs() {
        draw_tab_bar(out, app)?;
    }
    draw_buffer(out, app)?;
    draw_debug_pane(out, app)?;
    draw_status_line(out, app)?;
    draw_notification(out, app)?;
    if matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Prompt(_)) {
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
    // Buffer-relative → screen y.
    let top_row = top_row + app.buffer_top();
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
    use crate::app::{HoverCodeBlock, HoverLine};

    let Some(hover) = app.hover.as_ref() else { return Ok(()); };
    if hover.lines.is_empty() {
        return Ok(());
    }

    let widest_actual = hover.widest_line().max(20);
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
    let top_row = top_row + app.buffer_top();
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
    let heading_fg = title_fg;

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

    // Pre-compute byte-colour maps for each code block, keyed by block_idx.
    // Cached per render call so multiple visible lines from one block share
    // a single tree-sitter pass.
    let mut block_colors: std::collections::HashMap<usize, Vec<Option<Color>>> =
        std::collections::HashMap::new();
    let needed_blocks: std::collections::HashSet<usize> = hover
        .lines
        .iter()
        .skip(hover.scroll)
        .take(visible)
        .filter_map(|l| match l {
            HoverLine::Code { block_idx, .. } => Some(*block_idx),
            _ => None,
        })
        .collect();
    for idx in needed_blocks {
        let HoverCodeBlock { lang, source } = &hover.code_blocks[idx];
        let colors = lang
            .and_then(|l| crate::lang::compute_byte_colors(l, source, &app.config))
            .unwrap_or_else(|| vec![None; source.len()]);
        block_colors.insert(idx, colors);
    }

    // Body — show `visible` lines starting at `scroll`.
    for i in 0..visible {
        let y = (top_row + 1 + i) as u16;
        let idx = hover.scroll + i;
        queue!(
            out,
            MoveTo(left_col as u16, y),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
            SetBackgroundColor(bg),
        )?;
        let written = match hover.lines.get(idx) {
            None | Some(HoverLine::Blank) => 0,
            Some(HoverLine::Prose(s)) => {
                let truncated: String = s.chars().take(content_w).collect();
                let n = truncated.chars().count();
                queue!(out, SetForegroundColor(text_fg), Print(&truncated))?;
                n
            }
            Some(HoverLine::Heading { text, .. }) => {
                let truncated: String = text.chars().take(content_w).collect();
                let n = truncated.chars().count();
                queue!(
                    out,
                    SetForegroundColor(heading_fg),
                    SetAttribute(Attribute::Bold),
                    Print(&truncated),
                    SetAttribute(Attribute::Reset),
                    SetBackgroundColor(bg),
                )?;
                n
            }
            Some(HoverLine::Rule) => {
                let line = "─".repeat(content_w);
                queue!(out, SetForegroundColor(border), Print(&line))?;
                content_w
            }
            Some(HoverLine::Code { block_idx, byte_offset, byte_len }) => {
                let block = &hover.code_blocks[*block_idx];
                let slice = &block.source[*byte_offset..*byte_offset + *byte_len];
                let colors = block_colors.get(block_idx);
                paint_code_line(out, slice, *byte_offset, colors, content_w, text_fg)?
            }
        };
        if written < content_w {
            queue!(out, Print(" ".repeat(content_w - written)))?;
        }
        queue!(out, SetForegroundColor(border), Print('│'))?;
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

/// Paint one code line into the popup body. `slice` is the original bytes
/// from the block's source; `byte_offset` is where `slice` starts inside
/// that source so we can index into the parallel `colors` map. Tabs expand
/// to TAB_WIDTH spaces in the same colour as the tab byte. Returns the
/// number of *display columns* written so the caller can pad.
fn paint_code_line(
    out: &mut impl Write,
    slice: &str,
    byte_offset: usize,
    colors: Option<&Vec<Option<Color>>>,
    max_w: usize,
    default_fg: Color,
) -> Result<usize> {
    let mut written = 0usize;
    let mut byte_pos = 0usize;
    for ch in slice.chars() {
        let len = ch.len_utf8();
        let abs = byte_offset + byte_pos;
        byte_pos += len;
        let fg = colors
            .and_then(|c| c.get(abs).copied().flatten())
            .unwrap_or(default_fg);
        if ch == '\t' {
            let cells = TAB_WIDTH;
            let avail = max_w.saturating_sub(written);
            let n = cells.min(avail);
            if n == 0 {
                return Ok(written);
            }
            queue!(out, SetForegroundColor(fg), Print(" ".repeat(n)))?;
            written += n;
        } else {
            if written >= max_w {
                return Ok(written);
            }
            queue!(out, SetForegroundColor(fg), Print(ch.to_string()))?;
            written += 1;
        }
    }
    Ok(written)
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
    if matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Prompt(_)) {
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
    // Sit immediately below the tab bar when it's visible so the
    // notification doesn't overlap any tab labels. `buffer_top()` is
    // 1 when tabs are showing, 0 otherwise — same offset the buffer
    // body uses.
    let top = app.buffer_top();

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
        Mode::Prompt(crate::mode::PromptKind::Rename) => ("Rename", ' '),
        Mode::Prompt(crate::mode::PromptKind::ReplaceAll) => ("Replace in buffer", ' '),
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

    // Row layout: ` chip  label                  detail `
    //              ^^^^   ^^^^^                  ^^^^^^
    //               4ch   left-aligned           right-aligned, dim
    // Fixed chrome: 1 (left pad) + 3 (chip) + 2 (gap) + 2 (gap) + 1 (right pad) = 9
    const CHIP_W: usize = 3;
    const CHROME: usize = 9; // pads + chip + gaps
    let max_label = c
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(8);
    let max_detail = c
        .items
        .iter()
        .filter_map(|i| i.detail.as_ref().map(|d| d.chars().count()))
        .max()
        .unwrap_or(0);
    // Cap label and detail so the popup doesn't blow past 80 chars.
    let label_w = max_label.min(40);
    let detail_w = max_detail.min(35);
    let mut popup_w = CHROME + label_w + detail_w;
    let max_popup_w = (app.width as usize).saturating_sub(4).min(80);
    if popup_w > max_popup_w {
        // Trim detail first; only trim the label if there's no detail.
        let over = popup_w - max_popup_w;
        let detail_trim = over.min(detail_w);
        let new_detail = detail_w - detail_trim;
        let label_trim = (over - detail_trim).min(label_w);
        let new_label = label_w - label_trim;
        popup_w = CHROME + new_label + new_detail;
    }
    let body_w = popup_w.saturating_sub(CHROME);
    // Re-derive label/detail widths from the final popup_w (we may have trimmed).
    let final_detail_w = body_w.saturating_sub(label_w).min(detail_w);
    let final_label_w = body_w.saturating_sub(final_detail_w);

    let start = if c.selected >= popup_h {
        c.selected + 1 - popup_h
    } else {
        0
    };

    let gutter = app.gutter_width();
    let cursor_row = app.cursor.line.saturating_sub(app.view_top);
    let cursor_col = gutter + app.cursor.col;
    let buffer_rows = app.buffer_rows();
    let mut top_row = cursor_row + 1;
    if top_row + popup_h > buffer_rows {
        top_row = cursor_row.saturating_sub(popup_h);
    }
    let top_row = top_row + app.buffer_top();
    let mut left_col = cursor_col;
    if left_col + popup_w > app.width as usize {
        left_col = (app.width as usize).saturating_sub(popup_w);
    }

    let bg_unsel = Color::Rgb { r: 0x31, g: 0x32, b: 0x44 }; // Surface0
    let bg_sel = Color::Rgb { r: 0x45, g: 0x47, b: 0x5a };   // Surface1
    let label_unsel = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let label_sel = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe };   // Lavender
    let detail_fg = Color::Rgb { r: 0x9a, g: 0xa0, b: 0xb0 };   // Overlay2

    for row in 0..popup_h {
        let pos = start + row;
        if pos >= c.items.len() {
            break;
        }
        let item = &c.items[pos];
        let selected = pos == c.selected;
        let y = (top_row + row) as u16;
        let row_bg = if selected { bg_sel } else { bg_unsel };
        let label_fg = if selected { label_sel } else { label_unsel };

        let (chip_text, chip_color) = completion_kind_chip(item.kind.as_deref());
        let chip_pad = CHIP_W.saturating_sub(chip_text.chars().count());

        let label: String = item
            .label
            .chars()
            .take(final_label_w)
            .collect();
        let label_pad = final_label_w.saturating_sub(label.chars().count());
        let detail_raw = item.detail.as_deref().unwrap_or("");
        let detail: String = if final_detail_w == 0 {
            String::new()
        } else {
            detail_raw.chars().take(final_detail_w).collect()
        };
        let detail_pad = final_detail_w.saturating_sub(detail.chars().count());

        queue!(
            out,
            MoveTo(left_col as u16, y),
            SetBackgroundColor(row_bg),
            Print(' '),
            SetForegroundColor(chip_color),
            Print(&*chip_text),
            Print(" ".repeat(chip_pad)),
            Print("  "),
            SetForegroundColor(label_fg),
            Print(&label),
            Print(" ".repeat(label_pad)),
            Print("  "),
            SetForegroundColor(detail_fg),
            // Right-align detail by padding before it.
            Print(" ".repeat(detail_pad)),
            Print(&detail),
            Print(' '),
            ResetColor,
        )?;
    }
    Ok(())
}

/// Pick a short kind chip + Catppuccin colour for an LSP completion item.
/// The chip text is always 3 chars (padded if shorter) so the body column
/// stays aligned across rows.
fn completion_kind_chip(kind: Option<&str>) -> (&'static str, Color) {
    let yellow = Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf };
    let blue = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa };
    let mauve = Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 };
    let teal = Color::Rgb { r: 0x94, g: 0xe2, b: 0xd5 };
    let peach = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 };
    let green = Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 };
    let sky = Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb };
    let subtext1 = Color::Rgb { r: 0xba, g: 0xc2, b: 0xde };
    match kind.unwrap_or("") {
        "function" | "method" => ("fn", blue),
        "constructor" => ("new", blue),
        "variable" => ("var", peach),
        "class" | "struct" => ("cls", yellow),
        "interface" => ("if", mauve),
        "field" | "property" => ("fld", teal),
        "module" => ("mod", sky),
        "snippet" => ("snp", green),
        "keyword" => ("kw", mauve),
        "enum" => ("enm", yellow),
        "enum-member" => ("em", yellow),
        "constant" => ("K", peach),
        "type-param" => ("T", yellow),
        "value" => ("val", subtext1),
        "folder" => ("/", subtext1),
        "file" => ("fi", subtext1),
        "color" => ("■", peach),
        "operator" => ("op", mauve),
        "event" => ("evt", peach),
        "unit" => ("u", subtext1),
        "reference" => ("ref", subtext1),
        _ => ("·", subtext1),
    }
}

/// Picker popup geometry. Layout inside the box, rows numbered relative
/// to the top border (row 0):
///   0           top border `╭─ Files ── 1/54 ─╮`
///   1           top padding (blank)
///   2           prompt row `│ › typed   …    │`
///   3           separator (blank)
///   4..N-3      list rows
///   N-3         bottom padding (blank)
///   N-2         footer hint `│ ↵ open  ^N/^P  esc │`
///   N-1         bottom border
struct PickerLayout {
    left: usize,
    top: usize,
    inner_w: usize,
    list_top: usize,
    list_h: usize,
    prompt_row: usize,
    footer_row: usize,
    bottom_row: usize,
}

/// Number of list rows the picker can display in its current geometry.
/// Driven from the same layout math the renderer uses, so PageUp/PageDown
/// in the picker handler match what's actually on screen.
pub(crate) fn picker_visible_rows(app: &App) -> usize {
    picker_layout(app).list_h
}

fn picker_layout(app: &App) -> PickerLayout {
    let total_w = app.width as usize;
    let total_h = app.height as usize;
    // Box dimensions — generous side margins so the popup floats clearly
    // above the dimmed buffer rather than touching the screen edges.
    let box_w = ((total_w * 4) / 5).clamp(50, 100).min(total_w.saturating_sub(4));
    // 7 rows of chrome: top border, top pad, prompt, separator, …, bottom
    // pad, footer, bottom border. Min 12 keeps at least 5 list rows visible.
    let box_h = ((total_h * 3) / 5)
        .clamp(12, 28)
        .min(total_h.saturating_sub(2));

    let inner_w = box_w.saturating_sub(2);
    let left = total_w.saturating_sub(box_w) / 2;
    // Bias slightly above centre so the popup doesn't visually fight the
    // status line.
    let bottom_chrome = 2;
    let top = (total_h.saturating_sub(bottom_chrome).saturating_sub(box_h) / 2).max(0);

    let prompt_row = top + 2;
    let footer_row = top + box_h - 2;
    let bottom_row = top + box_h - 1;
    let list_top = top + 4;
    let list_h = footer_row.saturating_sub(list_top + 1);

    PickerLayout {
        left,
        top,
        inner_w,
        list_top,
        list_h,
        prompt_row,
        footer_row,
        bottom_row,
    }
}

fn draw_picker(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(picker) = app.picker.as_ref() else { return Ok(()); };
    let layout = picker_layout(app);

    let bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let border = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }; // Surface2
    let title_fg = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let count_fg = Color::Rgb { r: 0xa6, g: 0xad, b: 0xc8 }; // Subtext0
    let prompt_fg = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }; // Peach
    let input_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let path_fg = Color::Rgb { r: 0x9a, g: 0xa0, b: 0xb0 }; // Overlay2
    let name_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let dim_fg = Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c }; // Overlay1
    let sel_bg = Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }; // Surface1
    let sel_accent = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let hint_fg = Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c }; // Overlay1
    let hint_key_fg = Color::Rgb { r: 0xa6, g: 0xad, b: 0xc8 }; // Subtext0

    // ── Top border with embedded title and counter ─────────────────────
    let title_seg = format!(" {} ", picker.title);
    let total = picker.filtered.len();
    let cur = if total == 0 { 0 } else { picker.selected + 1 };
    let count_seg = format!(" {}/{} ", cur, total);
    let title_w = title_seg.chars().count();
    let count_w = count_seg.chars().count();
    // Border layout: ╭─ {title} ─...─ {count} ─╮
    // Reserved for corners + flanking single dashes = 4 chars.
    let filler = layout.inner_w.saturating_sub(title_w + count_w + 2);
    queue!(
        out,
        MoveTo(layout.left as u16, layout.top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print('─'),
        SetForegroundColor(title_fg),
        SetAttribute(Attribute::Bold),
        Print(&title_seg),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(filler)),
        SetForegroundColor(count_fg),
        Print(&count_seg),
        SetForegroundColor(border),
        Print('─'),
        Print('╮'),
    )?;

    // ── Top padding row (blank inside borders) ─────────────────────────
    draw_padding_row(out, &layout, layout.top + 1, bg, border)?;

    // ── Prompt row: ` › <input>` ───────────────────────────────────────
    let input_chars: String = picker
        .input
        .chars()
        .take(layout.inner_w.saturating_sub(4))
        .collect();
    let input_w = input_chars.chars().count();
    let prompt_pad = layout.inner_w.saturating_sub(3 + input_w);
    queue!(
        out,
        MoveTo(layout.left as u16, layout.prompt_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        Print(' '),
        SetForegroundColor(prompt_fg),
        Print('›'),
        Print(' '),
        SetForegroundColor(input_fg),
        Print(&input_chars),
        Print(" ".repeat(prompt_pad)),
        SetForegroundColor(border),
        Print('│'),
    )?;

    // ── Separator below prompt ─────────────────────────────────────────
    draw_padding_row(out, &layout, layout.top + 3, bg, border)?;

    // ── List rows ──────────────────────────────────────────────────────
    let start = if picker.selected >= layout.list_h {
        picker.selected + 1 - layout.list_h
    } else {
        0
    };
    for row in 0..layout.list_h {
        let y = layout.list_top + row;
        let pos = start + row;
        let item_in_range = pos < picker.filtered.len();
        let selected = item_in_range && pos == picker.selected;
        let row_bg = if selected { sel_bg } else { bg };

        queue!(
            out,
            MoveTo(layout.left as u16, y as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
            SetBackgroundColor(row_bg),
        )?;

        // Selection accent bar (1 char) + 1 space gap before content.
        if selected {
            queue!(
                out,
                SetForegroundColor(sel_accent),
                SetAttribute(Attribute::Bold),
                Print('▌'),
            )?;
        } else {
            queue!(out, Print(' '))?;
        }
        queue!(out, Print(' '))?;

        // Body width = inner_w - 2 (one for accent, one for trailing pad).
        let body_w = layout.inner_w.saturating_sub(3);
        let mut written = 0usize;
        if item_in_range {
            let item_idx = picker.filtered[pos];
            let display = &picker.items[item_idx];
            // Path-based pickers get a file-type icon prefix; symbol /
            // code-action lists don't (the row isn't a file).
            let show_icon = matches!(
                picker.kind,
                crate::picker::PickerKind::Files
                    | crate::picker::PickerKind::Recents
                    | crate::picker::PickerKind::Buffers
                    | crate::picker::PickerKind::Grep
                    | crate::picker::PickerKind::References,
            );
            // Matched-char positions are stored per-filtered-row alongside
            // the indices into items — empty when the picker has no query.
            let positions = picker.match_positions.get(pos).map(|v| v.as_slice()).unwrap_or(&[]);
            written = paint_picker_row(
                out, display, body_w, selected, path_fg, name_fg, dim_fg, show_icon, positions,
            )?;
        }
        if written < body_w {
            queue!(out, Print(" ".repeat(body_w - written)))?;
        }

        if selected {
            queue!(out, SetAttribute(Attribute::Reset))?;
        }
        queue!(
            out,
            SetBackgroundColor(bg),
            Print(' '),
            SetForegroundColor(border),
            Print('│'),
        )?;
    }

    // ── Bottom padding ─────────────────────────────────────────────────
    draw_padding_row(out, &layout, layout.footer_row - 1, bg, border)?;

    // ── Footer hint row ────────────────────────────────────────────────
    // Render as ` ↵ open  ^N/^P navigate  esc cancel `, with the keymap
    // tokens dimmer than the surrounding labels so the eye picks the
    // shortcut first. Falls back to a shorter hint on narrow terminals.
    let full_hint: &[(&str, bool)] = &[
        (" ", false),
        ("↵", true),
        (" open  ", false),
        ("^J", true),
        ("/", false),
        ("^K", true),
        (" navigate  ", false),
        ("esc", true),
        (" cancel", false),
    ];
    let short_hint: &[(&str, bool)] = &[
        (" ", false),
        ("↵", true),
        (" open  ", false),
        ("^J", true),
        ("/", false),
        ("^K", true),
        ("  ", false),
        ("esc", true),
    ];
    let seg_width = |segs: &[(&str, bool)]| -> usize {
        segs.iter().map(|(s, _)| s.chars().count()).sum()
    };
    let hint_segments: &[(&str, bool)] =
        if seg_width(full_hint) <= layout.inner_w { full_hint }
        else if seg_width(short_hint) <= layout.inner_w { short_hint }
        else { &[] };
    let hint_w = seg_width(hint_segments);
    let footer_pad = layout.inner_w.saturating_sub(hint_w);
    queue!(
        out,
        MoveTo(layout.left as u16, layout.footer_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
    )?;
    for (seg, is_key) in hint_segments {
        let fg = if *is_key { hint_key_fg } else { hint_fg };
        queue!(out, SetForegroundColor(fg), Print(*seg))?;
    }
    queue!(
        out,
        Print(" ".repeat(footer_pad)),
        SetForegroundColor(border),
        Print('│'),
    )?;

    // ── Bottom border ──────────────────────────────────────────────────
    queue!(
        out,
        MoveTo(layout.left as u16, layout.bottom_row as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(layout.inner_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

fn draw_padding_row(
    out: &mut impl Write,
    layout: &PickerLayout,
    y: usize,
    bg: Color,
    border: Color,
) -> Result<()> {
    queue!(
        out,
        MoveTo(layout.left as u16, y as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        Print(" ".repeat(layout.inner_w)),
        Print('│'),
    )?;
    Ok(())
}

/// Paint one picker entry inside the body band. Splits at the last `/` so
/// the directory part renders dim and the basename pops bright. Returns
/// the number of chars written (so the caller can fill the trailing pad).
fn paint_picker_row(
    out: &mut impl Write,
    display: &str,
    max_w: usize,
    selected: bool,
    path_fg: Color,
    name_fg: Color,
    dim_fg: Color,
    show_icon: bool,
    matched: &[usize],
) -> Result<usize> {
    if max_w == 0 {
        return Ok(0);
    }
    // Grep / References rows look like `path/to/file:LN:COL:match-text` —
    // peel the trailing `:LN…` so it doesn't break path detection or
    // get mistaken for part of the filename.
    let (display_path, suffix) = split_grep_location(display);
    let dir_end = display_path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (dir_part, name_part) = display_path.split_at(dir_end);

    // Resolve a file-type icon from the basename — Nerd Font glyphs per
    // detected Lang, with a generic document fallback. Two columns wide
    // in the budget (icon + space).
    let icon: Option<char> = if show_icon {
        Some(icon_for_basename(name_part))
    } else {
        None
    };
    let icon_w = if icon.is_some() { 2 } else { 0 };

    let dir_chars: Vec<char> = dir_part.chars().collect();
    let name_chars: Vec<char> = name_part.chars().collect();
    let suffix_chars: Vec<char> = suffix.chars().collect();
    let body_w = max_w.saturating_sub(icon_w);

    // Truncate the basename first if it alone exceeds the budget — keeps
    // the directory visible at least partially. Otherwise truncate from
    // the dir's left so the basename is always intact. Suffix (grep
    // location) is preserved as-is — clipping it would lose the match
    // line/column that's the whole point of the row.
    let suffix_len = suffix_chars.len();
    let path_budget = body_w.saturating_sub(suffix_len);
    let total = dir_chars.len() + name_chars.len();
    let dir_skip;
    let name_skip;
    let (dir_slice, name_slice) = if total <= path_budget {
        dir_skip = 0;
        name_skip = 0;
        (dir_chars.as_slice(), name_chars.as_slice())
    } else if name_chars.len() >= path_budget {
        dir_skip = dir_chars.len();
        name_skip = name_chars.len() - path_budget;
        let n = &name_chars[name_skip..];
        (&[][..], n)
    } else {
        let drop = total - path_budget;
        dir_skip = drop;
        name_skip = 0;
        let d = &dir_chars[drop..];
        (d, name_chars.as_slice())
    };

    let _ = dim_fg;
    let dir_color = if selected { name_fg } else { path_fg };
    let name_color = name_fg;
    let highlight = Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }; // Yellow
    if let Some(ch) = icon {
        queue!(
            out,
            SetForegroundColor(if selected { name_fg } else { path_fg }),
            Print(ch),
            Print(' '),
        )?;
    }
    // `matched` indexes the FULL display string (no suffix split). The
    // dir part is [0..dir_chars.len()), the name part is
    // [dir_chars.len()..dir_chars.len()+name_chars.len()). After
    // truncation we apply the slice's own skip so the right chars get
    // highlighted.
    let dir_total_offset = 0usize;
    let name_total_offset = dir_chars.len();
    paint_chars(
        out,
        dir_slice,
        dir_color,
        highlight,
        matched,
        dir_total_offset + dir_skip,
    )?;
    paint_chars(
        out,
        name_slice,
        name_color,
        highlight,
        matched,
        name_total_offset + name_skip,
    )?;
    let mut written = icon_w + dir_slice.len() + name_slice.len();
    if !suffix.is_empty() && written < max_w {
        let room = max_w - written;
        let suffix_slice: &[char] = if suffix_chars.len() <= room {
            &suffix_chars
        } else {
            &suffix_chars[..room]
        };
        let suffix_offset = display_path.chars().count();
        paint_chars(out, suffix_slice, path_fg, highlight, matched, suffix_offset)?;
        written += suffix_slice.len();
    }
    Ok(written)
}

/// Paint `slice` one char at a time, switching to `highlight_color` for
/// chars whose absolute display-string index sits in `matched`. Reset
/// foreground per char to keep colour leaks contained.
fn paint_chars(
    out: &mut impl Write,
    slice: &[char],
    base_color: Color,
    highlight_color: Color,
    matched: &[usize],
    abs_start: usize,
) -> Result<()> {
    if matched.is_empty() {
        let s: String = slice.iter().collect();
        queue!(out, SetForegroundColor(base_color), Print(&s))?;
        return Ok(());
    }
    let mut prev_highlighted = false;
    queue!(out, SetForegroundColor(base_color))?;
    for (i, ch) in slice.iter().enumerate() {
        let highlighted = matched.binary_search(&(abs_start + i)).is_ok();
        if highlighted != prev_highlighted {
            queue!(
                out,
                SetForegroundColor(if highlighted { highlight_color } else { base_color }),
            )?;
            if highlighted {
                queue!(out, SetAttribute(Attribute::Bold))?;
            } else {
                queue!(out, SetAttribute(Attribute::NormalIntensity))?;
            }
            prev_highlighted = highlighted;
        }
        queue!(out, Print(ch))?;
    }
    if prev_highlighted {
        queue!(out, SetAttribute(Attribute::NormalIntensity))?;
    }
    Ok(())
}

/// Peel off a trailing `:LINE:COL:…` (grep) or `:LINE` (references)
/// suffix so the path-portion routes through the same dir/name split as
/// a bare filename. Returns `(path, suffix)` where suffix includes the
/// leading colon.
fn split_grep_location(display: &str) -> (&str, &str) {
    let bytes = display.as_bytes();
    // Find the first colon that's followed by a digit. Skip `C:` Windows-
    // style drive letters by requiring something before the colon.
    for (i, b) in bytes.iter().enumerate() {
        if *b == b':' && i > 0 {
            let next = bytes.get(i + 1).copied().unwrap_or(0);
            if next.is_ascii_digit() {
                return (&display[..i], &display[i..]);
            }
        }
    }
    (display, "")
}

/// File-type icon for a basename. Detects the language via the same
/// path-extension logic the buffer uses; falls back to a generic
/// document glyph when nothing matches.
fn icon_for_basename(basename: &str) -> char {
    let path = std::path::Path::new(basename);
    if let Some(lang) = Lang::detect(path) {
        return lang_icon(lang);
    }
    // Nerd Font generic file icon.
    '\u{f15b}'
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
    let top = app.buffer_top();
    let total_w = app.width as usize;
    // Blank every row so leftover content from a prior frame can't bleed
    // through. Don't touch the tab-bar row when it's painted above us.
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16), Clear(ClearType::CurrentLine))?;
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
            MoveTo(left as u16, (top + i + app.buffer_top()) as u16),
            SetForegroundColor(blue),
            Print(line),
            ResetColor,
        )?;
    }
    Ok(())
}

/// ANSI Shadow rendering of "binvim". 6 rows, 46 cols. Used as the
/// `:health` dashboard's banner header.
const HEALTH_BANNER: &[&str] = &[
    " ██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗",
    " ██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║",
    " ██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║",
    " ██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    " ██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    " ╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];

/// Full-screen `:health` dashboard. ASCII-art banner up top, then
/// densely-packed section headers (no boxes — boxes added too much
/// chrome). Esc / `q` / `:q` dismiss (handled in `app/input.rs`).
fn draw_health_page(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    if rows == 0 || total_w < 30 {
        return Ok(());
    }

    // Clear the buffer area first so leftover frame content can't bleed
    // through.
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16), Clear(ClearType::CurrentLine))?;
    }

    let snap = app.build_health_snapshot();
    let p = DashboardPalette::default();

    let left = 2usize;
    // Reserve the bottom row of the buffer area for the always-on
    // footer so the keybinding hint stays visible while scrolling.
    let viewport_rows = rows.saturating_sub(1);
    // Body width budget — leave a 2-col margin on the right too.
    let body_w = total_w.saturating_sub(left + 2).max(40);

    // Build the dashboard as a flat list of virtual rows first so we
    // can both measure its total height (for the input handler's
    // clamp) and paint just the slice the user has scrolled to.
    let banner_fits = total_w >= 50 && rows > 10;
    let mut rows_buf: Vec<DashRow> = Vec::new();
    build_health_rows(&mut rows_buf, &snap, &p, left, body_w, banner_fits);

    // Stash the total content height so input handlers can clamp the
    // scroll without re-running the snapshot.
    app.health_content_height.set(rows_buf.len());

    let scroll = app
        .health_scroll
        .min(rows_buf.len().saturating_sub(viewport_rows));

    for (i, row) in rows_buf
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport_rows)
    {
        let screen_y = (top + (i - scroll)) as u16;
        row.paint(out, screen_y, &p)?;
    }

    // --- Footer (anchored to bottom of buffer area) -------------------
    let has_more_below = scroll + viewport_rows < rows_buf.len();
    let has_more_above = scroll > 0;
    let footer = match (has_more_above, has_more_below) {
        (false, false) => "Esc · q · :q to dismiss",
        (false, true) => "Esc · q · :q to dismiss · ↓ j more below",
        (true, false) => "Esc · q · :q to dismiss · ↑ k more above",
        (true, true) => "Esc · q · :q to dismiss · ↑ k ↓ j to scroll",
    };
    queue!(
        out,
        MoveTo(left as u16, (top + rows - 1) as u16),
        SetForegroundColor(p.overlay0),
        Print(truncate(footer, total_w.saturating_sub(left))),
        ResetColor,
    )?;
    Ok(())
}

/// Layout the dashboard into a flat list of `DashRow`s. Splitting the
/// build out from the paint loop lets us measure the total height once
/// (for scroll clamping) and paint only the visible window.
fn build_health_rows(
    rows: &mut Vec<DashRow>,
    snap: &crate::app::HealthSnapshot,
    p: &DashboardPalette,
    left: usize,
    body_w: usize,
    banner_fits: bool,
) {
    // One blank row of breathing room above the banner.
    rows.push(DashRow::Blank);

    if banner_fits {
        for line in HEALTH_BANNER {
            rows.push(DashRow::Banner {
                x: left,
                text: (*line).to_string(),
                colour: p.mauve,
            });
        }
    }

    rows.push(DashRow::Blank);

    // --- PROCESS + RESOURCES (two columns) ----------------------------
    let cpu_str = snap
        .cpu
        .map(|v| format!("{v:.1} %"))
        .unwrap_or_else(|| "—".into());
    let ram_str = match (snap.ram_mb, snap.ram_pct) {
        (Some(mb), Some(pct)) => format!("{pct:.1} % · {mb:.0} MB"),
        (Some(mb), None) => format!("{mb:.0} MB"),
        (None, Some(pct)) => format!("{pct:.1} %"),
        (None, None) => "—".into(),
    };
    let process_lines = vec![
        SectionLine::Custom {
            parts: vec![
                ("version  ".into(), p.subtext1),
                (snap.version.to_string(), p.text),
            ],
        },
        SectionLine::Custom {
            parts: vec![
                ("pid      ".into(), p.subtext1),
                (snap.pid.to_string(), p.text),
            ],
        },
    ];
    let resource_lines = vec![
        SectionLine::Custom {
            parts: vec![("CPU      ".into(), p.subtext1), (cpu_str, p.text)],
        },
        SectionLine::Custom {
            parts: vec![("RAM      ".into(), p.subtext1), (ram_str, p.text)],
        },
    ];
    let gap = 2usize;
    let half = body_w.saturating_sub(gap) / 2;
    let left_w = half;
    let right_w = body_w.saturating_sub(gap + half);
    push_two_section_boxes(
        rows,
        (left, left_w, "PROCESS", p.mauve, &process_lines),
        (left + left_w + gap, right_w, "RESOURCES", p.blue, &resource_lines),
    );
    rows.push(DashRow::Blank);

    // --- ENVIRONMENT (cwd + config in one box) ------------------------
    let cwd_disp = home_relative_path(&snap.cwd);
    let cfg_path_disp = if snap.config_path.is_empty() {
        "—".to_string()
    } else {
        home_relative_path(&snap.config_path)
    };
    let (cfg_status_label, cfg_status_colour) = if snap.config_loaded {
        ("[loaded]", p.green)
    } else {
        ("[missing]", p.overlay1)
    };
    let env_lines = vec![
        SectionLine::Custom {
            parts: vec![("cwd     ".into(), p.subtext1), (cwd_disp, p.text)],
        },
        SectionLine::Custom {
            parts: vec![
                ("config  ".into(), p.subtext1),
                (cfg_path_disp, p.text),
                ("  ".into(), p.subtext1),
                (cfg_status_label.into(), cfg_status_colour),
            ],
        },
    ];
    push_section_box(rows, left, body_w, "ENVIRONMENT", p.peach, &env_lines);
    rows.push(DashRow::Blank);

    // --- ACTIVE BUFFER section ----------------------------------------
    let mut active_lines: Vec<SectionLine> = Vec::new();
    match &snap.active_buffer {
        Some(ab) => {
            active_lines.push(SectionLine::plain(&ab.display_path, p.lavender));
            let lang_part = ab.language.clone().unwrap_or_else(|| "plain".into());
            active_lines.push(SectionLine::plain(
                &format!(
                    "{} · {} lines · {} · cursor {}:{}",
                    lang_part, ab.lines, ab.indent, ab.cursor_line, ab.cursor_col,
                ),
                p.subtext1,
            ));
            let d = &ab.diagnostics;
            active_lines.push(if d.total() == 0 {
                SectionLine::plain("diagnostics: clean", p.overlay1)
            } else {
                diagnostics_chip_row(d, p)
            });
            if ab.statuses.is_empty() {
                active_lines.push(SectionLine::plain(
                    "(no LSP specs match this extension)",
                    p.overlay1,
                ));
            } else {
                for st in &ab.statuses {
                    let (glyph, colour) = match (st.running, st.resolved_binary.is_some()) {
                        (true, _) => ('✓', p.green),
                        (false, true) => ('!', p.yellow),
                        (false, false) => ('✗', p.red),
                    };
                    let suffix = match (st.running, st.resolved_binary.as_deref()) {
                        (true, _) => String::new(),
                        (false, Some(bin)) => {
                            format!("  installed but not running ({})", home_relative_path(bin))
                        }
                        (false, None) => "  NOT INSTALLED".into(),
                    };
                    active_lines.push(SectionLine::plain(
                        &format!("{} {:<18} {:<16}{}", glyph, st.key, st.language_id, suffix),
                        colour,
                    ));
                }
            }
        }
        None => {
            active_lines.push(SectionLine::plain(
                "[No Name] — save the buffer to attach an LSP",
                p.overlay1,
            ));
        }
    }
    push_section_box(rows, left, body_w, "ACTIVE BUFFER", p.teal, &active_lines);
    rows.push(DashRow::Blank);

    // --- LSP SERVERS section ------------------------------------------
    let mut lsp_lines: Vec<SectionLine> = Vec::new();
    if snap.lsps.is_empty() {
        lsp_lines.push(SectionLine::plain("(no servers running)", p.overlay1));
    } else {
        for h in &snap.lsps {
            let root = display_lsp_root(&h.root_uri, 40);
            let pending_colour = if h.pending_requests > 0 { p.peach } else { p.overlay1 };
            lsp_lines.push(SectionLine::Custom {
                parts: vec![
                    (format!("• {:<18} ", h.key), p.text),
                    (format!("{:<16} ", h.language_id), p.subtext1),
                    (format!("{:<40} ", root), p.overlay1),
                    (format!("{} pending", h.pending_requests), pending_colour),
                ],
            });
        }
    }
    let lsp_title = format!("LSP SERVERS ({} running)", snap.lsps.len());
    push_section_box(rows, left, body_w, &lsp_title, p.lavender, &lsp_lines);
    rows.push(DashRow::Blank);

    // --- GIT section ---------------------------------------------------
    let mut git_lines: Vec<SectionLine> = Vec::new();
    match &snap.git {
        Some(g) => {
            let branch = g.branch.clone().unwrap_or_else(|| "—".into());
            let mut bits: Vec<String> = vec![format!("branch {branch}")];
            if let Some(up) = &g.upstream {
                bits.push(format!("upstream {up}"));
            }
            if g.ahead > 0 {
                bits.push(format!("ahead {}", g.ahead));
            }
            if g.behind > 0 {
                bits.push(format!("behind {}", g.behind));
            }
            if g.modified > 0 {
                bits.push(format!("modified {}", g.modified));
            }
            if g.untracked > 0 {
                bits.push(format!("untracked {}", g.untracked));
            }
            if g.ahead == 0
                && g.behind == 0
                && g.modified == 0
                && g.untracked == 0
                && g.upstream.is_some()
            {
                bits.push("clean".into());
            }
            git_lines.push(SectionLine::plain(&bits.join(" · "), p.subtext1));
        }
        None => {
            git_lines.push(SectionLine::plain("(not a git repository)", p.overlay1));
        }
    }
    push_section_box(rows, left, body_w, "GIT", p.peach, &git_lines);
    rows.push(DashRow::Blank);

    // --- BUFFERS section ----------------------------------------------
    let mut buf_lines: Vec<SectionLine> = Vec::new();
    if snap.buffers.is_empty() {
        buf_lines.push(SectionLine::plain("(none)", p.overlay1));
    } else {
        for (i, b) in snap.buffers.iter().enumerate() {
            let mut parts: Vec<(String, Color)> =
                vec![(format!("{:>3}  ", i + 1), p.overlay0), (b.label.clone(), p.text)];
            if b.active {
                parts.push((" ".into(), p.text));
                parts.push(("[active]".into(), p.green));
            }
            if b.dirty {
                parts.push((" ".into(), p.text));
                parts.push(("[dirty]".into(), p.peach));
            }
            buf_lines.push(SectionLine::Custom { parts });
        }
    }
    let buf_title = format!("BUFFERS ({})", snap.buffers.len());
    push_section_box(rows, left, body_w, &buf_title, p.blue, &buf_lines);
    rows.push(DashRow::Blank);

    // --- TAILWIND section ---------------------------------------------
    let tw_lines: Vec<SectionLine> = match &snap.tailwind {
        Some(p_) => {
            let label = if p_.file_name().and_then(|s| s.to_str()) == Some("package.json") {
                "v4 — `tailwindcss` listed in package.json"
            } else {
                "v3 — tailwind.config.* present"
            };
            vec![
                SectionLine::plain(&home_relative_path(&p_.display().to_string()), p.text),
                SectionLine::plain(label, p.subtext1),
            ]
        }
        None => vec![
            SectionLine::plain(
                "(not detected — Tailwind LSP will not attach)",
                p.overlay1,
            ),
            SectionLine::plain(
                "add tailwind.config.* or list `tailwindcss` in package.json",
                p.overlay0,
            ),
        ],
    };
    push_section_box(rows, left, body_w, "TAILWIND", p.teal, &tw_lines);
    rows.push(DashRow::Blank);

    // --- FORMATTER section --------------------------------------------
    let fmt_lines: Vec<SectionLine> = match &snap.formatter {
        Some(f) => {
            let (glyph, glyph_colour) = match &f.binary {
                Some(_) => ('✓', p.green),
                None => ('✗', p.red),
            };
            let bin_part = match (&f.binary, f.via_node_modules) {
                (Some(b), true) => format!("{} (node_modules)", home_relative_path(&b.display().to_string())),
                (Some(b), false) => home_relative_path(&b.display().to_string()),
                (None, _) => "NOT INSTALLED".into(),
            };
            let mut row1 = vec![
                (format!("{glyph} "), glyph_colour),
                (format!("{:<24}", f.label), p.text),
                (bin_part, p.subtext1),
            ];
            if f.binary.is_none() {
                if let Some(fb) = &f.fallback_label {
                    row1.push((format!("  (also tried {fb})"), p.overlay1));
                }
            }
            vec![SectionLine::Custom { parts: row1 }]
        }
        None => match snap.active_buffer.as_ref() {
            Some(_) => vec![SectionLine::plain(
                "(no formatter configured for this extension)",
                p.overlay1,
            )],
            None => vec![SectionLine::plain(
                "[No Name] — open a file to see its formatter",
                p.overlay1,
            )],
        },
    };
    push_section_box(rows, left, body_w, "FORMATTER", p.mauve, &fmt_lines);
    rows.push(DashRow::Blank);

    // --- EDITORCONFIG section -----------------------------------------
    let ec = &snap.editorconfig;
    let mut ec_lines: Vec<SectionLine> = Vec::new();
    ec_lines.push(SectionLine::Custom {
        parts: vec![
            ("indent             ".into(), p.subtext1),
            (ec.indent.clone(), p.text),
            ("   tab width ".into(), p.subtext1),
            (ec.tab_width.to_string(), p.text),
        ],
    });
    ec_lines.push(SectionLine::Custom {
        parts: vec![
            ("trim trailing ws   ".into(), p.subtext1),
            (
                if ec.trim_trailing { "yes" } else { "no" }.into(),
                if ec.trim_trailing { p.green } else { p.overlay1 },
            ),
            ("   final newline ".into(), p.subtext1),
            (
                if ec.final_newline { "yes" } else { "no" }.into(),
                if ec.final_newline { p.green } else { p.overlay1 },
            ),
        ],
    });
    if ec.sources.is_empty() {
        ec_lines.push(SectionLine::plain(
            "sources            (defaults — no .editorconfig found)",
            p.overlay1,
        ));
    } else {
        for (i, src) in ec.sources.iter().enumerate() {
            let label = if i == 0 { "sources            " } else { "                   " };
            ec_lines.push(SectionLine::Custom {
                parts: vec![
                    (label.into(), p.subtext1),
                    (home_relative_path(&src.display().to_string()), p.text),
                ],
            });
        }
    }
    push_section_box(rows, left, body_w, "EDITORCONFIG", p.peach, &ec_lines);
    rows.push(DashRow::Blank);

    // --- TREE-SITTER section ------------------------------------------
    let ts = &snap.tree_sitter;
    let mut ts_lines: Vec<SectionLine> = Vec::new();
    match &ts.language {
        Some(name) => {
            ts_lines.push(SectionLine::Custom {
                parts: vec![
                    ("language           ".into(), p.subtext1),
                    (name.clone(), p.text),
                ],
            });
            let (glyph, colour, suffix) = if ts.highlight_cache_ready {
                (
                    '✓',
                    p.green,
                    format!("active · {} bytes coloured", ts.cache_byte_count),
                )
            } else if snap.active_buffer.is_some() {
                ('!', p.yellow, "no cache yet (paint pending)".into())
            } else {
                ('—', p.overlay1, "no buffer".into())
            };
            ts_lines.push(SectionLine::Custom {
                parts: vec![
                    (format!("{glyph} highlight cache   "), colour),
                    (suffix, p.text),
                ],
            });
        }
        None => match snap.active_buffer.as_ref() {
            Some(_) => ts_lines.push(SectionLine::plain(
                "(unknown extension — no tree-sitter parser configured)",
                p.overlay1,
            )),
            None => ts_lines.push(SectionLine::plain(
                "[No Name] — open a file to see its parser status",
                p.overlay1,
            )),
        },
    }
    push_section_box(rows, left, body_w, "TREE-SITTER", p.lavender, &ts_lines);
    rows.push(DashRow::Blank);

    // --- SESSION section ----------------------------------------------
    let s = &snap.session;
    let mut session_lines: Vec<SectionLine> = Vec::new();
    let (glyph, glyph_colour, label) = if s.restored {
        ('✓', p.green, "restored on launch")
    } else if s.session_file_exists {
        ('—', p.overlay1, "saved (will restore on next launch with no path arg)")
    } else {
        ('—', p.overlay1, "no session for this cwd yet")
    };
    session_lines.push(SectionLine::Custom {
        parts: vec![
            (format!("{glyph} "), glyph_colour),
            (format!("{:<24}", label), p.text),
        ],
    });
    if let Some(sp) = &s.session_path {
        session_lines.push(SectionLine::Custom {
            parts: vec![
                ("session file       ".into(), p.subtext1),
                (home_relative_path(&sp.display().to_string()), p.text),
            ],
        });
    }
    session_lines.push(SectionLine::Custom {
        parts: vec![
            ("recent files       ".into(), p.subtext1),
            (s.recents_count.to_string(), p.text),
        ],
    });
    push_section_box(rows, left, body_w, "SESSION", p.blue, &session_lines);
    rows.push(DashRow::Blank);

    // --- TERMINAL section ---------------------------------------------
    let t = &snap.terminal;
    let mut term_lines: Vec<SectionLine> = Vec::new();
    term_lines.push(SectionLine::Custom {
        parts: vec![
            ("size               ".into(), p.subtext1),
            (format!("{}×{}", t.width, t.height), p.text),
        ],
    });
    term_lines.push(SectionLine::Custom {
        parts: vec![
            ("$TERM              ".into(), p.subtext1),
            (t.term.clone().unwrap_or_else(|| "—".into()), p.text),
        ],
    });
    let truecolor_colour = if t.truecolor { p.green } else { p.overlay1 };
    let truecolor_label = if t.truecolor { "yes" } else { "no" };
    term_lines.push(SectionLine::Custom {
        parts: vec![
            ("truecolor          ".into(), p.subtext1),
            (truecolor_label.into(), truecolor_colour),
            ("   $COLORTERM ".into(), p.subtext1),
            (t.colorterm.clone().unwrap_or_else(|| "—".into()), p.text),
        ],
    });
    if let Some(prog) = &t.program {
        term_lines.push(SectionLine::Custom {
            parts: vec![
                ("$TERM_PROGRAM      ".into(), p.subtext1),
                (prog.clone(), p.text),
            ],
        });
    }
    push_section_box(rows, left, body_w, "TERMINAL", p.teal, &term_lines);
}

/// One row inside a dashboard section box.
enum SectionLine {
    /// Single coloured run. `truncate(text, inner_w - 2)` applied at draw time.
    Plain { text: String, colour: Color },
    /// Hand-laid coloured segments — used when a row needs more than
    /// one colour run (LSP rows with their pending counter, diagnostics
    /// chip strip, attached-LSP rows with status-coloured glyphs).
    Custom { parts: Vec<(String, Color)> },
}

impl SectionLine {
    fn plain(text: &str, colour: Color) -> SectionLine {
        SectionLine::Plain {
            text: text.to_string(),
            colour,
        }
    }
}

/// One virtual row of the health dashboard. Built up by
/// `build_health_rows` and painted by `draw_health_page` after
/// scrolling has been applied. Splitting build from paint lets us
/// measure the total height in a single pass without rendering
/// off-screen rows.
enum DashRow {
    /// Empty row used for vertical breathing space between sections.
    Blank,
    /// A line of ASCII-art banner text.
    Banner { x: usize, text: String, colour: Color },
    /// Top border of a single boxed section (with inline title).
    BoxTop {
        x: usize,
        width: usize,
        title: String,
        title_colour: Color,
    },
    /// Content row inside a boxed section.
    BoxContent {
        x: usize,
        width: usize,
        line: SectionLine,
    },
    /// Bottom border of a boxed section.
    BoxBottom { x: usize, width: usize },
    /// Top borders of two side-by-side boxes painted on the same row.
    BoxTopPair {
        a: (usize, usize, String, Color), // x, width, title, colour
        b: (usize, usize, String, Color),
    },
    /// Content rows of two side-by-side boxes painted on the same row.
    /// Each option is independent — an absent side leaves that column
    /// untouched (e.g. when one box has more content than the other).
    BoxContentPair {
        a: Option<(usize, usize, SectionLine)>,
        b: Option<(usize, usize, SectionLine)>,
    },
    /// Bottom borders of two side-by-side boxes painted on the same row.
    BoxBottomPair { a: (usize, usize), b: (usize, usize) },
}

impl DashRow {
    fn paint<W: Write>(&self, out: &mut W, y: u16, palette: &DashboardPalette) -> Result<()> {
        match self {
            DashRow::Blank => Ok(()),
            DashRow::Banner { x, text, colour } => {
                queue!(
                    out,
                    MoveTo(*x as u16, y),
                    SetForegroundColor(*colour),
                    Print(text),
                    ResetColor,
                )?;
                Ok(())
            }
            DashRow::BoxTop { x, width, title, title_colour } => {
                paint_box_top(out, *x, y, *width, title, *title_colour, palette)
            }
            DashRow::BoxContent { x, width, line } => {
                paint_box_content(out, *x, y, *width, line, palette)
            }
            DashRow::BoxBottom { x, width } => paint_box_bottom(out, *x, y, *width, palette),
            DashRow::BoxTopPair { a, b } => {
                paint_box_top(out, a.0, y, a.1, &a.2, a.3, palette)?;
                paint_box_top(out, b.0, y, b.1, &b.2, b.3, palette)?;
                Ok(())
            }
            DashRow::BoxContentPair { a, b } => {
                if let Some((x, w, line)) = a {
                    paint_box_content(out, *x, y, *w, line, palette)?;
                }
                if let Some((x, w, line)) = b {
                    paint_box_content(out, *x, y, *w, line, palette)?;
                }
                Ok(())
            }
            DashRow::BoxBottomPair { a, b } => {
                paint_box_bottom(out, a.0, y, a.1, palette)?;
                paint_box_bottom(out, b.0, y, b.1, palette)?;
                Ok(())
            }
        }
    }
}

fn paint_box_top<W: Write>(
    out: &mut W,
    x: usize,
    y: u16,
    width: usize,
    title: &str,
    title_colour: Color,
    palette: &DashboardPalette,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    let title_marked = format!(" {} ", title);
    let title_visible = title_marked.chars().count();
    let dashes = inner_w.saturating_sub(title_visible + 1);
    queue!(
        out,
        MoveTo(x as u16, y),
        SetForegroundColor(palette.border),
        Print("┌─"),
        SetForegroundColor(title_colour),
        SetAttribute(crossterm::style::Attribute::Bold),
        Print(&title_marked),
        SetAttribute(crossterm::style::Attribute::Reset),
        SetForegroundColor(palette.border),
        Print("─".repeat(dashes)),
        Print('┐'),
        ResetColor,
    )?;
    Ok(())
}

fn paint_box_content<W: Write>(
    out: &mut W,
    x: usize,
    y: u16,
    width: usize,
    line: &SectionLine,
    palette: &DashboardPalette,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    let body_w = inner_w.saturating_sub(2); // 1-col padding each side
    queue!(
        out,
        MoveTo(x as u16, y),
        SetForegroundColor(palette.border),
        Print('│'),
        SetForegroundColor(palette.text),
        Print(' '),
    )?;
    let painted = match line {
        SectionLine::Plain { text, colour } => {
            let trimmed = truncate(text, body_w);
            let w = trimmed.chars().count();
            queue!(out, SetForegroundColor(*colour), Print(trimmed))?;
            w
        }
        SectionLine::Custom { parts } => {
            let mut painted = 0usize;
            for (segment, colour) in parts {
                let avail = body_w.saturating_sub(painted);
                if avail == 0 {
                    break;
                }
                let trimmed = truncate(segment, avail);
                let w = trimmed.chars().count();
                queue!(out, SetForegroundColor(*colour), Print(trimmed))?;
                painted += w;
            }
            painted
        }
    };
    let pad = body_w.saturating_sub(painted);
    queue!(
        out,
        SetForegroundColor(palette.text),
        Print(" ".repeat(pad + 1)),
        SetForegroundColor(palette.border),
        Print('│'),
        ResetColor,
    )?;
    Ok(())
}

fn paint_box_bottom<W: Write>(
    out: &mut W,
    x: usize,
    y: u16,
    width: usize,
    palette: &DashboardPalette,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    queue!(
        out,
        MoveTo(x as u16, y),
        SetForegroundColor(palette.border),
        Print('└'),
        Print("─".repeat(inner_w)),
        Print('┘'),
        ResetColor,
    )?;
    Ok(())
}

/// Append one boxed section to the dashboard row list: top border with
/// inline title, one row per `SectionLine`, then a bottom border.
fn push_section_box(
    rows: &mut Vec<DashRow>,
    x: usize,
    width: usize,
    title: &str,
    title_colour: Color,
    lines: &[SectionLine],
) {
    rows.push(DashRow::BoxTop {
        x,
        width,
        title: title.to_string(),
        title_colour,
    });
    for line in lines {
        rows.push(DashRow::BoxContent {
            x,
            width,
            line: clone_section_line(line),
        });
    }
    rows.push(DashRow::BoxBottom { x, width });
}

/// Two side-by-side boxes on the same set of rows. When their content
/// lengths differ we pad the shorter side with empty content rows so
/// both bottom borders line up.
fn push_two_section_boxes(
    rows: &mut Vec<DashRow>,
    a: (usize, usize, &str, Color, &[SectionLine]),
    b: (usize, usize, &str, Color, &[SectionLine]),
) {
    let (ax, aw, at, ac, al) = a;
    let (bx, bw, bt, bc, bl) = b;
    rows.push(DashRow::BoxTopPair {
        a: (ax, aw, at.to_string(), ac),
        b: (bx, bw, bt.to_string(), bc),
    });
    let n = al.len().max(bl.len());
    for i in 0..n {
        let row_a = al.get(i).map(|l| (ax, aw, clone_section_line(l)));
        let row_b = bl.get(i).map(|l| (bx, bw, clone_section_line(l)));
        rows.push(DashRow::BoxContentPair { a: row_a, b: row_b });
    }
    rows.push(DashRow::BoxBottomPair { a: (ax, aw), b: (bx, bw) });
}

fn clone_section_line(line: &SectionLine) -> SectionLine {
    match line {
        SectionLine::Plain { text, colour } => SectionLine::Plain {
            text: text.clone(),
            colour: *colour,
        },
        SectionLine::Custom { parts } => SectionLine::Custom {
            parts: parts.clone(),
        },
    }
}

/// Pre-build the diagnostics chip row as a `SectionLine::Custom`.
fn diagnostics_chip_row(
    counts: &crate::app::DiagnosticsCounts,
    palette: &DashboardPalette,
) -> SectionLine {
    let mut parts: Vec<(String, Color)> = vec![("diagnostics  ".into(), palette.subtext1)];
    let chips: [(&str, usize, Color); 4] = [
        ("errors", counts.errors, palette.red),
        ("warnings", counts.warnings, palette.yellow),
        ("info", counts.info, palette.blue),
        ("hints", counts.hints, palette.teal),
    ];
    for (i, (label, n, colour)) in chips.iter().enumerate() {
        let chip_colour = if *n > 0 { *colour } else { palette.overlay1 };
        parts.push((format!("{label} {n}"), chip_colour));
        if i + 1 < chips.len() {
            parts.push((" · ".into(), palette.overlay0));
        }
    }
    SectionLine::Custom { parts }
}

/// Catppuccin Mocha palette grouped for dashboard reuse.
struct DashboardPalette {
    text: Color,
    subtext1: Color,
    overlay0: Color,
    overlay1: Color,
    border: Color,
    lavender: Color,
    mauve: Color,
    blue: Color,
    teal: Color,
    green: Color,
    yellow: Color,
    peach: Color,
    red: Color,
}

impl Default for DashboardPalette {
    fn default() -> Self {
        Self {
            text: Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 },
            subtext1: Color::Rgb { r: 0xba, g: 0xc2, b: 0xde },
            overlay0: Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 },
            overlay1: Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c },
            border: Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 }, // Surface2
            lavender: Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe },
            mauve: Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 },
            blue: Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa },
            teal: Color::Rgb { r: 0x94, g: 0xe2, b: 0xd5 },
            green: Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 },
            yellow: Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf },
            peach: Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 },
            red: Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 },
        }
    }
}

/// Cut a string down to `width` display chars, appending `…` when
/// truncation removes content.
fn truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Compact display for an LSP server's workspace root. Strips
/// `file://`, replaces `$HOME` with `~`, and if the result is still
/// wider than `width` keeps the trailing two path segments behind a
/// leading `…/`. Falls back to the basename when even two segments
/// don't fit.
fn display_lsp_root(uri: &str, width: usize) -> String {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    let home_relative = home_relative_path(stripped);
    if home_relative.chars().count() <= width {
        return home_relative;
    }
    // Take the trailing components, prepending `…/` to signal the trim.
    let segments: Vec<&str> = home_relative
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    for take in (1..=segments.len().min(2)).rev() {
        let tail: String = segments[segments.len() - take..].join("/");
        let candidate = if take < segments.len() {
            format!("…/{tail}")
        } else {
            tail
        };
        if candidate.chars().count() <= width {
            return candidate;
        }
    }
    // Last resort — straight truncation of the basename.
    let basename = segments.last().copied().unwrap_or(stripped);
    truncate(basename, width)
}

/// Replace a leading `$HOME` prefix with `~` so the dashboard's
/// long paths read cleanly. `$HOME` resolution best-effort — falls
/// back to the input unchanged when the env var is missing.
fn home_relative_path(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home_str = home.to_string_lossy();
    home_relative_with(path, &home_str)
}

/// Pure variant of `home_relative_path` — caller supplies the home
/// dir explicitly, so tests don't depend on the process environment.
fn home_relative_with(path: &str, home: &str) -> String {
    if home.is_empty() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(home) {
        if rest.is_empty() {
            return "~".to_string();
        }
        if let Some(rest) = rest.strip_prefix('/') {
            return format!("~/{rest}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::{display_lsp_root, home_relative_with, truncate};

    #[test]
    fn home_relative_strips_home_prefix() {
        assert_eq!(
            home_relative_with("/Users/bg/Dev/binvim", "/Users/bg"),
            "~/Dev/binvim"
        );
    }

    #[test]
    fn home_relative_handles_home_itself() {
        assert_eq!(home_relative_with("/Users/bg", "/Users/bg"), "~");
    }

    #[test]
    fn home_relative_passthrough_when_no_match() {
        assert_eq!(
            home_relative_with("/opt/cache", "/Users/bg"),
            "/opt/cache"
        );
    }

    #[test]
    fn display_lsp_root_uses_full_path_when_short() {
        // `file://` strip + tilde substitution would still keep us
        // within budget, so display_lsp_root keeps the whole thing.
        let out = display_lsp_root("file:///x/y", 40);
        assert_eq!(out, "/x/y");
    }

    #[test]
    fn display_lsp_root_trims_to_tail_segments_when_too_wide() {
        // 60-char input, 25-char budget — keeps the last two segments
        // behind a `…/` so the project context survives the trim.
        let long =
            "file:///Users/bg/Development/bgunnarsson/comp/packages/ui-apps/src";
        let out = display_lsp_root(long, 25);
        assert!(out.starts_with("…/"), "expected leading ellipsis, got {out:?}");
        assert!(out.ends_with("ui-apps/src"), "expected ui-apps/src tail, got {out:?}");
        assert!(out.chars().count() <= 25, "exceeded budget: {out:?}");
    }

    #[test]
    fn truncate_appends_ellipsis_when_over_width() {
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_passthrough_when_within_width() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hi", 5), "hi");
    }
}

/// One drawable tab on the bar. Tab body layout (left → right):
///
///   ` <label>[ +]  <×> `
///
/// `+` only appears for dirty buffers. `close_col` is the absolute
/// screen column of `×` so the mouse handler can hit-test it without
/// re-doing the width math.
pub(crate) struct TabSlot {
    pub idx: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub close_col: Option<usize>,
    pub label: String,
    pub dirty: bool,
    pub active: bool,
}

/// Per-tab chrome cost (excluding the filename itself):
///   1 leading + label + [ +] (2 if dirty) + 2 gap + 1 × + 1 trailing
fn tab_width(label_chars: usize, dirty: bool) -> usize {
    let chrome = 5;
    chrome + label_chars + if dirty { 2 } else { 0 }
}

/// Compute the tab layout for the current buffer set. The active tab is
/// guaranteed visible — the slice is anchored so it stays on screen,
/// scrolling left when needed. `scrolled_left()` / `truncated_right()`
/// on the result drive the chevron rendering.
pub(crate) fn tab_layout(app: &App) -> Vec<TabSlot> {
    let total_w = app.width as usize;
    let mut entries: Vec<(usize, String, bool)> = Vec::with_capacity(app.buffers.len());
    for (i, stash) in app.buffers.iter().enumerate() {
        let (path, dirty, display_name) = if i == app.active {
            (
                app.buffer.path.as_ref(),
                app.buffer.dirty,
                app.buffer.display_name.as_deref(),
            )
        } else {
            (
                stash.buffer.path.as_ref(),
                stash.buffer.dirty,
                stash.buffer.display_name.as_deref(),
            )
        };
        let label = path
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .or_else(|| display_name.map(|s| s.to_string()))
            .unwrap_or_else(|| "[No Name]".into());
        entries.push((i, label, dirty));
    }
    let widths: Vec<usize> = entries
        .iter()
        .map(|(_, label, dirty)| tab_width(label.chars().count(), *dirty))
        .collect();
    let total_widths: usize = widths.iter().sum();
    let mut start_idx = 0usize;
    if total_widths > total_w {
        let mut used = 0usize;
        for i in (0..=app.active).rev() {
            used += widths[i];
            if used > total_w / 2 {
                start_idx = (i + 1).min(app.active);
                break;
            }
            start_idx = i;
        }
    }
    let mut slots = Vec::new();
    let mut col = 0usize;
    for i in start_idx..entries.len() {
        let w = widths[i];
        if col + w > total_w {
            break;
        }
        let (idx, label, dirty) = &entries[i];
        // close_col = end_col - 2 (last interior column = `×`, then one
        // trailing space pad). Always set — slots are never narrower
        // than `tab_width` returns.
        let close_col = Some(col + w - 2);
        slots.push(TabSlot {
            idx: *idx,
            start_col: col,
            end_col: col + w,
            close_col,
            label: label.clone(),
            dirty: *dirty,
            // No tab is the "active" one while the start page is up —
            // we're not actually rendering any buffer. Highlighting one
            // would be misleading.
            active: *idx == app.active && !app.show_start_page,
        });
        col += w;
    }
    slots
}

fn draw_tab_bar(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine))?;

    let bar_bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 }; // Mantle
    let active_bg = Color::Rgb { r: 0x45, g: 0x47, b: 0x5a }; // Surface1
    let active_fg = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
    let inactive_fg = Color::Rgb { r: 0xa6, g: 0xad, b: 0xc8 }; // Subtext0
    let dirty_fg = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }; // Peach
    let close_fg = Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c }; // Overlay1
    let chevron_fg = Color::Rgb { r: 0xba, g: 0xc2, b: 0xde }; // Subtext1

    // Bar-wide bg fill so gaps between tabs render in the bar colour.
    queue!(
        out,
        SetBackgroundColor(bar_bg),
        SetForegroundColor(inactive_fg),
        Print(" ".repeat(total_w)),
    )?;

    let slots = tab_layout(app);
    let scrolled_left = slots.first().map(|s| s.idx > 0).unwrap_or(false);
    let truncated_right = slots
        .last()
        .map(|s| s.idx + 1 < app.buffers.len())
        .unwrap_or(false);

    for slot in &slots {
        let (bg, fg) = if slot.active {
            (active_bg, active_fg)
        } else {
            (bar_bg, inactive_fg)
        };
        queue!(
            out,
            MoveTo(slot.start_col as u16, 0),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
        )?;
        if slot.active {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        // ` label[ +]  × `
        queue!(out, Print(' '), Print(&slot.label))?;
        if slot.dirty {
            queue!(
                out,
                SetForegroundColor(dirty_fg),
                Print(" +"),
                SetForegroundColor(fg),
            )?;
        }
        queue!(
            out,
            Print("  "),
            SetForegroundColor(close_fg),
            Print('×'),
            SetForegroundColor(fg),
            Print(' '),
        )?;
        if slot.active {
            queue!(out, SetAttribute(Attribute::Reset))?;
        }
    }

    // Overflow chevrons on the content row.
    if scrolled_left {
        queue!(
            out,
            MoveTo(0, 0),
            SetBackgroundColor(bar_bg),
            SetForegroundColor(chevron_fg),
            Print('‹'),
        )?;
    }
    if truncated_right {
        queue!(
            out,
            MoveTo((total_w.saturating_sub(1)) as u16, 0),
            SetBackgroundColor(bar_bg),
            SetForegroundColor(chevron_fg),
            Print('›'),
        )?;
    }

    queue!(out, ResetColor)?;
    Ok(())
}

fn draw_buffer(out: &mut impl Write, app: &App) -> Result<()> {
    if app.show_health_page {
        return draw_health_page(out, app);
    }
    if app.show_start_page {
        return draw_start_page(out, app);
    }
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    let gutter = app.gutter_width();
    let avail = (app.width as usize).saturating_sub(gutter);
    let total_lines = app.buffer.line_count();
    let mut line_idx = app.view_top;
    // Skip any lines that are hidden by a closed fold from the start of
    // the viewport so the first visible row isn't mid-fold.
    while line_idx < total_lines && app.line_is_folded(line_idx) {
        line_idx += 1;
    }
    // Canonicalise the buffer's path once for the duration of this draw so
    // breakpoint + stopped-frame gutter lookups don't do one syscall per
    // visible row. Fall back to the raw path if canonicalisation fails
    // (e.g. unsaved or removed file).
    let canon_buf_path: Option<std::path::PathBuf> = app
        .buffer
        .path
        .as_ref()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
    // 1-based line number of the currently-stopped top frame, if the
    // session is paused inside this buffer.
    let pc_line: Option<usize> = match (&canon_buf_path, app.dap.session.as_ref()) {
        (Some(bp), Some(session)) if matches!(session.state, crate::dap::SessionState::Stopped { .. }) => {
            session.frames.first().and_then(|f| {
                let fs = f.source.as_ref()?;
                let fs_canon = fs.canonicalize().unwrap_or_else(|_| fs.clone());
                if &fs_canon == bp { Some(f.line) } else { None }
            })
        }
        _ => None,
    };
    for row in 0..rows {
        // Clear the row before drawing — guards against terminal-side wrap
        // from the previous row's render leaking onto this one.
        queue!(out, MoveTo(0, (row + top) as u16), Clear(ClearType::CurrentLine))?;
        if line_idx < total_lines {
            // Git stripe — leftmost gutter column. Mirrors gitsigns /
            // GitGutter conventions: a coloured vertical block for
            // added (Green) / modified (Yellow) / a horizontal block
            // for deleted (Red). Empty when the line is unchanged or
            // the buffer isn't tracked by git.
            let git_kind = app.git_hunk_kind_at(line_idx);
            if let Some(kind) = git_kind {
                let (glyph, color) = match kind {
                    crate::git::GitHunkKind::Added => (
                        '▎',
                        Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 }, // Green
                    ),
                    crate::git::GitHunkKind::Modified => (
                        '▎',
                        Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }, // Yellow
                    ),
                    crate::git::GitHunkKind::Deleted => (
                        '▁',
                        Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }, // Red
                    ),
                };
                queue!(
                    out,
                    SetForegroundColor(color),
                    Print(glyph.to_string()),
                    ResetColor
                )?;
            } else {
                queue!(out, Print(" "))?;
            }
            // Sign column priority: stopped-at marker > user breakpoint >
            // worst LSP diagnostic. The debug marks are user-actionable
            // ground truth and should win when they collide.
            let line_one_based = line_idx + 1;
            let pc_here = pc_line == Some(line_one_based);
            let bp_here = canon_buf_path
                .as_deref()
                .map(|p| app.dap.has_breakpoint(p, line_one_based))
                .unwrap_or(false);
            let sign = if pc_here {
                Some(('▶', Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 })) // Peach
            } else if bp_here {
                Some(('●', Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 })) // Red
            } else {
                app.worst_diagnostic(line_idx).map(|s| match s {
                    Severity::Error => ('!', Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }),
                    Severity::Warning => ('?', Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }),
                    Severity::Info => ('i', Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa }),
                    Severity::Hint => ('h', Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }),
                })
            };
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
            // Relative numbers (Vim convention): every row except the
            // cursor's shows its distance from the cursor; the cursor's
            // own row shows the absolute (1-indexed) line. Useful with
            // count-prefixed motions like `5j` / `12k` / `3dd`. The
            // cursor row gets a brighter Subtext1 tone so the eye can
            // anchor on it; other rows stay the muted Overlay0.
            let (label, label_color) = if app.config.line_numbers.relative
                && line_idx != app.cursor.line
            {
                let dist = if line_idx > app.cursor.line {
                    line_idx - app.cursor.line
                } else {
                    app.cursor.line - line_idx
                };
                (
                    format!("{:>width$} ", dist, width = gutter - 3),
                    Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }, // Overlay0
                )
            } else {
                // Cursor row in relative mode, or every row in absolute
                // mode → 1-indexed absolute line number.
                let bright = app.config.line_numbers.relative;
                (
                    format!("{:>width$} ", line_idx + 1, width = gutter - 3),
                    if bright {
                        Color::Rgb { r: 0xba, g: 0xc2, b: 0xde } // Subtext1
                    } else {
                        Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 } // Overlay0
                    },
                )
            };
            queue!(out, SetForegroundColor(label_color), Print(label), ResetColor)?;
            draw_line_with_selection(out, app, line_idx, avail)?;
            // Fold-start placeholder: append `… N lines` after the line's
            // own content so the user sees what's collapsed.
            if app.line_is_fold_start(line_idx) {
                let span = app.folded_line_span(line_idx);
                let folded = format!("  ⏷ {} lines", span);
                queue!(
                    out,
                    SetForegroundColor(Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }), // Overlay0
                    Print(folded),
                    ResetColor
                )?;
            }
            // Advance to the next visible line — past the fold's hidden
            // body if this row was a fold start, otherwise just by one.
            let span = app.folded_line_span(line_idx);
            line_idx += span.max(1);
            while line_idx < total_lines && app.line_is_folded(line_idx) {
                line_idx += 1;
            }
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
    // Belt-and-suspenders: even though file load normalises CRLF, a stray `\r`
    // could still arrive via paste or an LSP-applied edit. Printing it would
    // reset the terminal cursor to column 0 and clobber the inline diagnostic.
    if text.ends_with('\r') {
        text.pop();
    }
    let sel = app.line_selection(line_idx);
    let extra_sels = app.line_extra_selections(line_idx);
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
    let hint_fg = Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c }; // Overlay1
    // Multi-cursor positions on this line — the renderer paints a
    // Lavender block at each so the user can see where mirrored edits
    // will land.
    let multi_cursors: Vec<usize> = app.line_multi_cursor_cols(line_idx);

    // Pre-bin inlay hints by column so we can render them inline at the
    // start of each char iteration (and once more after the last char,
    // for hints anchored at end-of-line).
    let mut hint_at: Vec<Vec<&crate::lsp::InlayHint>> = vec![Vec::new(); chars.len() + 1];
    if !dim {
        if let Some(path) = app.buffer.path.as_ref() {
            if let Some(hints) = app.inlay_hints.get(path) {
                for h in hints {
                    if h.line == line_idx && h.col <= chars.len() {
                        hint_at[h.col].push(h);
                    }
                }
            }
        }
    }
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
    // Markdown "concealed render" mode — only active when the buffer
    // is markdown AND the editor is in Normal mode (Insert / Visual
    // flip back to raw markdown). When active, per-line transforms
    // hide / replace structural markers (`# `, `**`, `*`, `` ` ``,
    // `[…](…)`, `- `, `> `) and style ranges layer bold / italic /
    // underline / strikethrough / colour over the syntax-highlight
    // pass. Whole-line `kind` short-circuits the per-char loop for
    // horizontal rules and hidden rows (setext underlines, fence
    // closers).
    let md_meta: Option<&crate::markdown_render::MarkdownLineMeta> =
        if app.markdown_render_active() {
            app.markdown_line_meta(line_idx)
        } else {
            None
        };
    if let Some(meta) = md_meta {
        match meta.kind {
            crate::markdown_render::MarkdownLineKind::Hidden => {
                // Render an empty row — the line is part of a fence
                // boundary or setext underline that has been folded
                // into adjacent rendering.
                return Ok(());
            }
            crate::markdown_render::MarkdownLineKind::HorizontalRule => {
                // Paint `─` × `avail` in muted Overlay0 so the rule
                // visually separates sections without competing with
                // surrounding prose.
                let rule: String = "─".repeat(avail);
                queue!(
                    out,
                    SetForegroundColor(Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }),
                    Print(rule),
                    ResetColor
                )?;
                return Ok(());
            }
            crate::markdown_render::MarkdownLineKind::Default => {}
        }
    }
    let mut conceal_active: Option<&crate::markdown_render::MarkdownTransform> = None;
    for (col, c) in chars.iter().enumerate() {
        // Markdown conceal: exit a transform we just walked past.
        if let Some(t) = conceal_active {
            if col >= t.end {
                conceal_active = None;
            }
        }
        // Markdown conceal: enter a new transform that starts at this col.
        // For Replace transforms we paint the substitute glyph here so the
        // replacement lands at the source-marker's visual position; for
        // Hide transforms we just record state and let the inner cols be
        // skipped silently. Either way `line_visual_pos` advances by the
        // replacement's width so subsequent chars sit where the user
        // expects (right after the rendered glyph, not the source span).
        if conceal_active.is_none() {
            if let Some(meta) = md_meta {
                if let Some(t) = meta.transforms.iter().find(|t| t.start == col) {
                    let glyph_w = match &t.action {
                        crate::markdown_render::ConcealAction::Hide => 0,
                        crate::markdown_render::ConcealAction::Replace { glyph, color } => {
                            let w = glyph.chars().count();
                            // Paint only when at least one cell of the glyph
                            // would land inside the viewport. Fully off-left
                            // / off-right glyphs are silently skipped — the
                            // surrounding char loop already handles the
                            // surrounding bytes' state advancement.
                            if line_visual_pos + w > view_left {
                                let visible_left = line_visual_pos.max(view_left);
                                let visible_right =
                                    (line_visual_pos + w).min(view_left + avail);
                                if visible_right > visible_left {
                                    let visible = visible_right - visible_left;
                                    if visual_used + visible > avail {
                                        clipped_right = true;
                                        break;
                                    }
                                    let skip = visible_left - line_visual_pos;
                                    let printable: String =
                                        glyph.chars().skip(skip).take(visible).collect();
                                    queue!(
                                        out,
                                        SetForegroundColor(*color),
                                        Print(printable),
                                        ResetColor
                                    )?;
                                    visual_used += visible;
                                }
                            }
                            w
                        }
                    };
                    line_visual_pos += glyph_w;
                    conceal_active = Some(t);
                }
            }
        }
        if conceal_active.is_some() {
            byte_off += c.len_utf8();
            continue;
        }
        // Paint any inlay hints anchored at this column before the char.
        // Hints contribute to the on-screen width budget so the buffer
        // chars after them still wrap and clip correctly.
        if !hint_at[col].is_empty() && line_visual_pos >= view_left {
            for h in &hint_at[col] {
                let label_w = h.label.chars().count();
                let remaining = avail.saturating_sub(visual_used);
                if remaining == 0 {
                    break;
                }
                let printable: String = h.label.chars().take(remaining).collect();
                let written = printable.chars().count();
                // Parameter hints (kind == 2) read better in a slightly
                // warmer tone than type hints — they sit right before
                // the value the user just typed, so a touch of
                // differentiation makes them scannable as "this is the
                // parameter name" rather than blending into the type
                // hints elsewhere on the line. Type hints (kind == 1
                // or unknown) keep the muted Overlay1 tone.
                let fg = if h.kind == 2 {
                    Color::Rgb { r: 0x93, g: 0x99, b: 0xb2 } // Overlay2 — warmer than Overlay1
                } else {
                    hint_fg
                };
                queue!(
                    out,
                    SetForegroundColor(fg),
                    SetAttribute(Attribute::Italic),
                    Print(&printable),
                    SetAttribute(Attribute::Reset),
                    ResetColor,
                )?;
                visual_used += written;
                if written < label_w {
                    clipped_right = true;
                    break;
                }
            }
            if clipped_right {
                break;
            }
        }
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
        let in_sel = sel.map(|(s, e)| col >= s && col < e).unwrap_or(false)
            || extra_sels.iter().any(|(s, e)| col >= *s && col < *e);
        let in_search = !in_sel && search_matches.iter().any(|(s, e)| col >= *s && col < *e);
        let in_yank_flash = !in_sel
            && !in_search
            && yank_flash.map(|(s, e)| col >= s && col < e).unwrap_or(false);
        let in_match_pair = !in_sel
            && !in_search
            && !in_yank_flash
            && match_pair.iter().any(|(s, e)| col >= *s && col < *e);
        // Multi-cursor marker — paint the cell the cursor is sitting on
        // (i.e. the char to its right) in Lavender so the user can see
        // where their other cursors are.
        let is_multi_cursor = multi_cursors.contains(&col);
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
        } else if is_multi_cursor {
            // Lavender bg + Base fg — high contrast so the cursor pops,
            // matches the colour we'd use for the primary's cursor block.
            queue!(
                out,
                SetBackgroundColor(Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }), // Lavender
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
        // Markdown style overlay — bold / italic / underline + colour
        // override on top of whatever the syntax pass picked. Suppressed
        // when the cell is already speaking for itself (selection,
        // search, yank flash, multi-cursor, match-pair, whitespace
        // marker, modal dim) — those need to read as chrome, not
        // markdown styling.
        let md_style = md_meta.and_then(|m| crate::markdown_render::style_at(m, col));
        let md_attrs_set = if let Some(s) = md_style {
            let suppressed = in_sel
                || in_search
                || in_yank_flash
                || is_multi_cursor
                || in_match_pair
                || render_hidden
                || dim;
            if !suppressed {
                if let Some(c) = s.color {
                    queue!(out, SetForegroundColor(c))?;
                }
                if s.bold {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                }
                if s.italic {
                    queue!(out, SetAttribute(Attribute::Italic))?;
                }
                if s.underline {
                    queue!(out, SetAttribute(Attribute::Underlined))?;
                }
                if s.strikethrough {
                    queue!(out, SetAttribute(Attribute::CrossedOut))?;
                }
                s.bold || s.italic || s.underline || s.strikethrough
            } else {
                false
            }
        } else {
            false
        };
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
        } else if is_multi_cursor {
            queue!(out, ResetColor)?;
        } else if md_attrs_set {
            // Bold / italic / underline don't unset themselves on the
            // next char — clear all SGR so the styling stops at the
            // span boundary.
            queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        } else if in_search
            || in_yank_flash
            || syntax_color.is_some()
            || dim
            || render_hidden
            || md_style.and_then(|s| s.color).is_some()
        {
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

    // Multi-cursor anchored at end-of-line (col == chars.len()) — paint a
    // Lavender block so the user can see the cursor sitting past the
    // last char.
    if !clipped_right && multi_cursors.contains(&chars.len()) && visual_used + 1 <= avail {
        queue!(
            out,
            SetBackgroundColor(Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }), // Lavender
            SetForegroundColor(Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e }), // Base
            Print(' '),
            ResetColor,
        )?;
        visual_used += 1;
    }

    // Inlay hints anchored at end-of-line (col == chars.len()) — most
    // common case for type annotations on `let` bindings.
    if !clipped_right && !hint_at[chars.len()].is_empty() {
        for h in &hint_at[chars.len()] {
            let label_w = h.label.chars().count();
            let remaining = avail.saturating_sub(visual_used);
            if remaining == 0 {
                clipped_right = true;
                break;
            }
            let printable: String = h.label.chars().take(remaining).collect();
            let written = printable.chars().count();
            // Same kind-aware tone the inline pass uses — parameter
            // hints lean a shade warmer than type hints.
            let fg = if h.kind == 2 {
                Color::Rgb { r: 0x93, g: 0x99, b: 0xb2 }
            } else {
                hint_fg
            };
            queue!(
                out,
                SetForegroundColor(fg),
                SetAttribute(Attribute::Italic),
                Print(&printable),
                SetAttribute(Attribute::Reset),
                ResetColor,
            )?;
            visual_used += written;
            if written < label_w {
                clipped_right = true;
                break;
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
    let diags = app.line_diagnostics(line_idx);
    let has_diag = !diags.is_empty();
    if !dim {
        if let Some(diag) = diags.first() {
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

    // Inline git-blame virtual text — only when blame is toggled on, no
    // diagnostic is already painted on this row, and we have a blame
    // record for the line. Same width-safety math as the diagnostic
    // overlay; one column of slack so a miscount can't wrap.
    if !dim && app.blame_visible && !has_diag {
        if let Some(b) = app.blame.get(line_idx) {
            use unicode_width::UnicodeWidthChar;
            let label = format!("  {} • {} • {}", b.author, b.age, b.sha);
            let remaining = avail.saturating_sub(visual_used);
            let safety = 1usize;
            if remaining > safety {
                let budget = remaining - safety;
                let mut used = 0usize;
                let mut msg = String::new();
                for c in label.chars() {
                    let w = UnicodeWidthChar::width(c).unwrap_or(0);
                    if used + w > budget {
                        break;
                    }
                    msg.push(c);
                    used += w;
                }
                if !msg.is_empty() {
                    queue!(
                        out,
                        SetForegroundColor(Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 }), // Overlay0
                        SetAttribute(Attribute::Italic),
                        Print(&msg),
                        SetAttribute(Attribute::NoItalic),
                        ResetColor
                    )?;
                }
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
        Mode::Visual(VisualKind::Block) => Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 }, // Mauve
        Mode::Command => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        Mode::Search { .. } => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        Mode::Picker => Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }, // Sky
        Mode::Prompt(_) => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        Mode::DebugPane => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach — matches debug pane accent
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
        // Nerd Fonts v3 dropped `\u{f81a}` (the old "C#" glyph from v2),
        // so any user on a recent font (Hack Nerd Font, JetBrainsMono
        // Nerd Font 3.x, …) saw the missing-glyph tofu. The Seti
        // codepoint `\u{e648}` has been stable across v2 and v3 and
        // renders correctly on every current Nerd Font release.
        Lang::CSharp | Lang::Razor => '\u{e648}',
        Lang::Bash => '\u{f489}',
        Lang::Yaml => '\u{e6a8}',
        Lang::Xml => '\u{e619}',
        Lang::EditorConfig => '\u{e652}',
        Lang::GitIgnore => '\u{f1d3}',
        Lang::Python => '\u{e606}',
        Lang::C => '\u{e61e}',
        Lang::Cpp => '\u{e61d}',
        Lang::Lua => '\u{e620}',
        Lang::Java => '\u{e738}',
        Lang::Ruby => '\u{e21e}',
        Lang::Php => '\u{e608}',
        Lang::Toml => '\u{e6b2}',
        Lang::Svelte => '\u{e697}',
        Lang::Zig => '\u{e6a9}',
        Lang::Nix => '\u{f313}',
        Lang::Elixir => '\u{e62d}',
        Lang::Kotlin => '\u{e634}',
        Lang::Dockerfile => '\u{f308}',
        Lang::Sql => '\u{e7c4}',
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
        Lang::Razor => "razor",
        Lang::Bash => "bash",
        Lang::Yaml => "yaml",
        Lang::Xml => "xml",
        Lang::EditorConfig => "editorconfig",
        Lang::GitIgnore => "gitignore",
        Lang::Python => "python",
        Lang::C => "c",
        Lang::Cpp => "cpp",
        Lang::Lua => "lua",
        Lang::Java => "java",
        Lang::Ruby => "ruby",
        Lang::Php => "php",
        Lang::Toml => "toml",
        Lang::Svelte => "svelte",
        Lang::Zig => "zig",
        Lang::Nix => "nix",
        Lang::Elixir => "elixir",
        Lang::Kotlin => "kotlin",
        Lang::Dockerfile => "dockerfile",
        Lang::Sql => "sql",
    }
}

/// Status-line tag for a file. Falls through to (lang_icon, lang_name) by
/// default, but overrides the label by extension where needed — .cshtml and
/// .razor both highlight as HTML internally but should announce as "razor".
fn lang_label(path: Option<&std::path::Path>, lang: Lang) -> (char, &'static str) {
    if let Some(ext) = path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        match ext {
            "cshtml" | "razor" => return ('\u{e648}', "razor"),
            "zsh" => return (lang_icon(lang), "zsh"),
            "ksh" => return (lang_icon(lang), "ksh"),
            _ => {}
        }
    }
    if let Some(name) = path.and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
        match name {
            ".zshrc" | ".zprofile" | ".zshenv" | ".zlogin" | ".zlogout" => {
                return (lang_icon(lang), "zsh");
            }
            ".kshrc" => return (lang_icon(lang), "ksh"),
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

fn draw_debug_pane(out: &mut impl Write, app: &App) -> Result<()> {
    let rows = app.debug_pane_rows();
    if rows == 0 {
        return Ok(());
    }
    let top = app.debug_pane_top();
    let width = app.width as usize;

    // Mantle — same shade the tab bar uses, so the pane reads as
    // chrome rather than another buffer split.
    let pane_bg = Color::Rgb { r: 0x18, g: 0x18, b: 0x25 };
    let header_bg = pane_bg;
    let header_fg = Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 }; // Text
    let body_bg = pane_bg;
    let muted = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 };     // Overlay0
    let accent = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 };    // Peach — debug accent
    let base = Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e };

    // Header row: " DEBUG  <adapter> · <status> "
    let label = " DEBUG ";
    let hint = match app.dap.session.as_ref() {
        Some(s) => format!(" {} · {} ", s.adapter_key, s.status_line),
        None => " no session — :debug to start ".to_string(),
    };
    queue!(out, MoveTo(0, top as u16), Clear(ClearType::CurrentLine))?;
    queue!(
        out,
        SetBackgroundColor(accent),
        SetForegroundColor(base),
        SetAttribute(Attribute::Bold),
        Print(label),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(header_bg),
        SetForegroundColor(muted),
        Print(&hint),
    )?;
    let used = label.chars().count() + hint.chars().count();
    if width > used {
        queue!(out, SetBackgroundColor(header_bg), Print(" ".repeat(width - used)))?;
    }
    queue!(out, ResetColor)?;

    // Body — split horizontally when the terminal is wide enough. The
    // left column stacks call stack on top of locals (with a labelled
    // separator); the right column is the console-output tail.
    let body_rows = rows.saturating_sub(1);
    let split_at = if width >= 80 { width / 2 } else { width };
    let left_w = if split_at < width { split_at.saturating_sub(1) } else { width };

    enum LeftRow<'a> {
        Empty,
        Note(&'a str),
        Frame(String),
        Separator(&'a str),
        Local {
            depth: usize,
            marker: char,
            name: &'a str,
            value: &'a str,
            selected: bool,
        },
    }

    // Flat locals tree — computed once so the key handler and renderer
    // agree on row order (the renderer's selection highlight has to point
    // at the same row the cursor's index does).
    let flat = app
        .dap
        .session
        .as_ref()
        .map(crate::dap::flat_locals_view)
        .unwrap_or_default();
    let pane_focused = app.mode == Mode::DebugPane;
    let selected_local_idx = if pane_focused && !flat.is_empty() {
        Some(app.dap_pane_cursor.min(flat.len() - 1))
    } else {
        None
    };

    let mut left_rows: Vec<LeftRow> = Vec::new();
    if let Some(session) = app.dap.session.as_ref() {
        if session.frames.is_empty() {
            // Tag the empty-frames note with the actual state so a
            // stopped-but-no-frames situation reads as a problem to
            // diagnose instead of looking identical to "still running".
            let note = match session.state {
                crate::dap::SessionState::Stopped { .. } => "(stopped — waiting for stackTrace)",
                crate::dap::SessionState::Running => "(running — no frames)",
                crate::dap::SessionState::Initializing => "(initialising)",
                crate::dap::SessionState::Configuring => "(configuring)",
                crate::dap::SessionState::Terminated => "(terminated)",
            };
            left_rows.push(LeftRow::Note(note));
        }
        // Show every frame — overflow is handled by the left column's
        // scroll position (`dap_left_scroll`), driven by the key handler
        // and clamped below.
        for f in &session.frames {
            let loc = f
                .source
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| format!("{}:{}", n, f.line))
                .unwrap_or_else(|| format!("?:{}", f.line));
            left_rows.push(LeftRow::Frame(format!("{} — {}", loc, f.name)));
        }
        if !flat.is_empty() {
            left_rows.push(LeftRow::Separator(" Locals "));
            for (i, row) in flat.iter().enumerate() {
                let marker = if row.expandable {
                    if row.expanded { '▼' } else { '▶' }
                } else {
                    ' '
                };
                left_rows.push(LeftRow::Local {
                    depth: row.depth,
                    marker,
                    name: &row.var.name,
                    value: &row.var.value,
                    selected: selected_local_idx == Some(i),
                });
            }
        }
    }

    // Clamp the user-driven scroll positions to their valid range. The
    // key handler keeps them in range too, but resizing the terminal or
    // a fresh log line landing right before the draw can put them
    // slightly out of bounds — easier to clamp here than to chase every
    // upstream mutation.
    let left_scroll = {
        let max = left_rows.len().saturating_sub(body_rows);
        app.dap_left_scroll.min(max)
    };

    // Full flat-mapped output buffer — we need every line because the
    // user can scroll back through history with `K`. The buffer is
    // bounded by `OUTPUT_LOG_CAP` so this stays cheap.
    let output_all: Vec<&str> = app
        .dap
        .output_buffer
        .iter()
        .flat_map(|line| line.output.lines())
        .collect();
    let right_scroll = {
        let max = output_all.len().saturating_sub(body_rows);
        app.dap_right_scroll.min(max)
    };
    // Visible right-column window: last `body_rows` lines, then walked
    // back `right_scroll` lines into the past.
    let total_lines = output_all.len();
    let end = total_lines.saturating_sub(right_scroll);
    let start = end.saturating_sub(body_rows);
    let output_tail: &[&str] = &output_all[start..end];

    for r in 0..body_rows {
        let y = (top + 1 + r) as u16;
        queue!(out, MoveTo(0, y), Clear(ClearType::CurrentLine))?;
        queue!(out, SetBackgroundColor(body_bg))?;
        // Left column. Apply scroll offset so the user can pan through
        // a stack deeper than the visible viewport.
        let row = left_rows.get(r + left_scroll).unwrap_or(&LeftRow::Empty);
        let inner_w = left_w.saturating_sub(2);
        match row {
            LeftRow::Empty => {
                queue!(out, Print(" ".repeat(left_w)))?;
            }
            LeftRow::Note(s) => {
                queue!(
                    out,
                    SetForegroundColor(muted),
                    Print(format!(" {} ", truncate_left(s, inner_w))),
                )?;
                pad_right(out, 2 + s.chars().count(), left_w)?;
            }
            LeftRow::Frame(s) => {
                queue!(
                    out,
                    SetForegroundColor(header_fg),
                    Print(format!(" {} ", truncate_left(s, inner_w))),
                )?;
                pad_right(out, 2 + s.chars().count().min(inner_w), left_w)?;
            }
            LeftRow::Separator(label) => {
                let bar_room = inner_w.saturating_sub(label.chars().count());
                let left_bar = bar_room / 2;
                let right_bar = bar_room - left_bar;
                queue!(
                    out,
                    SetForegroundColor(muted),
                    Print(" "),
                    Print("─".repeat(left_bar)),
                    SetForegroundColor(header_fg),
                    Print(*label),
                    SetForegroundColor(muted),
                    Print("─".repeat(right_bar)),
                    Print(" "),
                )?;
                pad_right(out, 2 + bar_room + label.chars().count(), left_w)?;
            }
            LeftRow::Local {
                depth,
                marker,
                name,
                value,
                selected,
            } => {
                // Catppuccin Surface2 — visible against body_bg without
                // shouting.
                let selection_bg = Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 };
                let row_bg = if *selected { selection_bg } else { body_bg };
                let indent: String = "  ".repeat(*depth);
                let entry = format!("{}{} {} = {}", indent, marker, name, value);
                let max_inner = inner_w.saturating_sub(1);
                let visible = truncate_left(&entry, max_inner);
                queue!(
                    out,
                    SetBackgroundColor(row_bg),
                    SetForegroundColor(header_fg),
                    Print(format!(" {} ", visible)),
                )?;
                let used = 2 + visible.chars().count();
                if left_w > used {
                    queue!(
                        out,
                        SetBackgroundColor(row_bg),
                        Print(" ".repeat(left_w - used))
                    )?;
                }
                queue!(out, SetBackgroundColor(body_bg))?;
            }
        }
        // Right column (and column divider) — only when the pane is wide
        // enough to split.
        if split_at < width {
            queue!(
                out,
                SetForegroundColor(muted),
                Print("│"),
                SetForegroundColor(header_fg),
            )?;
            let right_w = width.saturating_sub(left_w + 1);
            let right_text = output_tail
                .get(r)
                .map(|s| format!(" {} ", s.trim_end()))
                .unwrap_or_default();
            let right_visible: String = right_text.chars().take(right_w).collect();
            queue!(out, Print(&right_visible))?;
            if right_visible.chars().count() < right_w {
                queue!(out, Print(" ".repeat(right_w - right_visible.chars().count())))?;
            }
        }
        queue!(out, ResetColor)?;
    }
    Ok(())
}

/// Pad the current row out to `target` columns by writing spaces. `written`
/// is how many character columns have already been printed for this row.
fn pad_right(out: &mut impl Write, written: usize, target: usize) -> Result<()> {
    if target > written {
        queue!(out, Print(" ".repeat(target - written)))?;
    }
    Ok(())
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
        .map(|b| {
            // Append `+A ~M -D` counts when there are working-tree changes
            // — A added hunks, M modified, D deleted. Skipped entirely
            // when the buffer is clean.
            let mut added = 0usize;
            let mut modified = 0usize;
            let mut deleted = 0usize;
            for h in &app.git_hunks {
                match h.kind {
                    crate::git::GitHunkKind::Added => added += 1,
                    crate::git::GitHunkKind::Modified => modified += 1,
                    crate::git::GitHunkKind::Deleted => deleted += 1,
                }
            }
            let stats = if added + modified + deleted > 0 {
                format!(" +{added} ~{modified} -{deleted}")
            } else {
                String::new()
            };
            format!(" {} {}{} ", NF_BRANCH, b, stats)
        })
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
    if (app.show_start_page || app.show_health_page)
        && !matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Picker)
    {
        queue!(out, Hide)?;
        return Ok(());
    }
    // In the debug pane focus mode the selection highlight in the pane
    // is the user's "cursor" — the editor's terminal cursor would just
    // distract.
    if app.mode == Mode::DebugPane {
        queue!(out, Hide)?;
        return Ok(());
    }
    let style = match app.mode {
        Mode::Insert => SetCursorStyle::SteadyBar,
        _ => SetCursorStyle::SteadyBlock,
    };
    queue!(out, style, Show)?;
    if matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Prompt(_)) {
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
            let layout = picker_layout(app);
            // Prompt row body: '│' ' ' '›' ' ' <input> — cursor sits 4 cols in
            // from the left border, plus the typed prefix.
            let input_max = layout.inner_w.saturating_sub(4);
            let visible_input = picker.input.chars().count().min(input_max);
            let col = (layout.left + 4 + visible_input) as u16;
            let row = layout.prompt_row as u16;
            queue!(out, SetCursorStyle::SteadyBar, MoveTo(col, row))?;
            return Ok(());
        }
    }
    let gutter = app.gutter_width();
    let row = (app.cursor.line.saturating_sub(app.view_top) + app.buffer_top()) as u16;
    let line = app.buffer.rope.line(app.cursor.line);
    // Per-buffer-col inlay-hint widths so the cursor's visual position
    // accounts for them. Without this, the cursor renders at the visual
    // column corresponding to its buffer-char count and visually lands
    // *inside* any hint(s) anchored at or before its col — making
    // Backspace / typing edit a buffer position that's "ahead" of where
    // the user thinks the cursor is.
    let hints_at: Vec<usize> = inlay_hint_widths_for_line(app, app.cursor.line);
    // Markdown concealed mode collapses / replaces source spans, so the
    // cursor's visual column needs to walk the same transforms the
    // renderer used. Without this, the cursor would land at the
    // buffer-char visual position — which is past the rendered
    // content for hidden ranges, putting the terminal cursor several
    // cells right of where the user sees their position.
    if app.markdown_render_active() {
        if let Some(meta) = app.markdown_line_meta(app.cursor.line) {
            let line_chars: Vec<char> = line
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let visual = crate::markdown_render::visual_col_for_buffer_col(
                &line_chars,
                meta,
                app.cursor.col,
                TAB_WIDTH,
            );
            let on_screen = visual.saturating_sub(app.view_left);
            let col = (gutter + on_screen) as u16;
            queue!(out, MoveTo(col, row))?;
            return Ok(());
        }
    }
    let mut visual = 0usize;
    for (i, c) in line.chars().enumerate() {
        // Stop *before* adding the hint at col i — the cursor sits at
        // buffer position N which means "between buffer chars N-1 and
        // N". A hint anchored at col N renders between those same two
        // chars, conceptually *after* the cursor (it annotates what's
        // behind the cursor, e.g. `var view│: string` where `│` is
        // the cursor and `: string` is a trailing type hint). Adding
        // the col-N hint here would push the cursor past it visually
        // and a backspace would feel like it ate from the next token.
        if i >= app.cursor.col {
            break;
        }
        if let Some(w) = hints_at.get(i) {
            visual += *w;
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

/// Per-buffer-col total inlay-hint label width on `line`. Index `i`
/// holds the total cell width of every hint anchored at buffer col `i`
/// (hints render *before* the char at that col, so the cursor's visual
/// position needs to skip over them). Length is `line_len + 1` so hints
/// anchored at end-of-line are included.
pub(crate) fn inlay_hint_widths_for_line(app: &App, line: usize) -> Vec<usize> {
    let line_len = app.buffer.line_len(line);
    let mut out = vec![0usize; line_len + 1];
    let Some(path) = app.buffer.path.as_ref() else { return out };
    let Some(hints) = app.inlay_hints.get(path) else { return out };
    for h in hints {
        if h.line == line && h.col <= line_len {
            out[h.col] = out[h.col].saturating_add(h.label.chars().count());
        }
    }
    out
}
