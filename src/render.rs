use crate::app::App;
use crate::lang::Lang;
use crate::lsp::Severity;
use crate::mode::{Mode, VisualKind};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, MoveToColumn, SetCursorStyle, Show},
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
        SetUnderlineColor,
    },
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use std::io::Write;

pub const TAB_WIDTH: usize = 4;

/// Reset SGR state and immediately re-apply the optional theme background so
/// subsequent unstyled `Print` calls land on the theme bg instead of the
/// terminal's default. When `buf_bg` is `None` (no `background` set in
/// `[colors]`) this is a plain `ResetColor` and matches the pre-theme
/// behaviour exactly.
fn reset_to_buf_bg(out: &mut impl Write, buf_bg: Option<Color>) -> Result<()> {
    queue!(out, ResetColor)?;
    if let Some(c) = buf_bg {
        queue!(out, SetBackgroundColor(c))?;
    }
    Ok(())
}

fn apply_buf_bg(out: &mut impl Write, buf_bg: Option<Color>) -> Result<()> {
    if let Some(c) = buf_bg {
        queue!(out, SetBackgroundColor(c))?;
    }
    Ok(())
}

pub fn draw(out: &mut impl Write, app: &App) -> Result<()> {
    queue!(
        out,
        BeginSynchronizedUpdate,
        Hide,
        MoveTo(0, 0),
        Clear(ClearType::All)
    )?;
    if app.show_tabs() {
        draw_tab_bar(out, app)?;
    }
    // Start / health pages take over the full editor area — splits stay
    // dormant while they're up so the user isn't looking at a partitioned
    // "[No Name]" placeholder.
    if app.show_install_page {
        draw_install_page(out, app)?;
    } else if app.show_health_page {
        draw_health_page(out, app)?;
    } else if app.show_messages_page {
        draw_messages_page(out, app)?;
    } else if app.show_registers_page {
        draw_registers_page(out, app)?;
    } else if app.show_test_results_page {
        draw_test_results_page(out, app)?;
    } else if app.show_start_page {
        draw_start_page(out, app)?;
    } else {
        let editor_rect = app.editor_rect();
        let panes = app.layout.partition(editor_rect);
        for (id, rect) in &panes {
            let is_active = *id == app.active_window;
            let window = if is_active {
                &app.window
            } else {
                app.windows
                    .get(id)
                    .expect("layout window id not present in App.windows")
            };
            let bs = app.buffer_state(window.buffer_idx);
            draw_buffer(out, app, &bs, window, *rect, is_active)?;
        }
        draw_pane_dividers(out, app, editor_rect)?;
    }
    draw_file_tree_pane(out, app)?;
    draw_terminal_pane(out, app)?;
    draw_side_terminal_pane(out, app)?;
    draw_debug_pane(out, app)?;
    draw_status_line(out, app)?;
    draw_notification(out, app)?;
    if matches!(
        app.mode,
        Mode::Command | Mode::Search { .. } | Mode::Prompt(_)
    ) {
        draw_floating_cmdline(out, app)?;
    }
    // File-tree delete confirm — same popup chrome as the create /
    // rename prompts so the three ops feel uniform. Rendered in
    // FileTree mode (not Prompt mode) because the y/N input is
    // handled by `handle_file_tree_key`, not the prompt key path.
    if app.mode == Mode::FileTree && app.file_tree_pending_delete().is_some() {
        draw_file_tree_confirm(out, app)?;
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
    if app.pending_rename_preview.is_some() {
        draw_rename_preview(out, app)?;
    }
    place_cursor(out, app)?;
    queue!(out, EndSynchronizedUpdate)?;
    Ok(())
}

fn draw_whichkey(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(wk) = app.whichkey.as_ref() else {
        return Ok(());
    };
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
    let key_w = wk
        .entries
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(1);
    let label_w = wk
        .entries
        .iter()
        .map(|(_, l)| l.chars().count())
        .max()
        .unwrap_or(1);
    let entry_min = key_w + label_w + 4;
    let title_min = wk.title.chars().count() + 4; // some breathing space around the title
    let footer_min = " ESC close ".chars().count();
    let mut content_w = entry_min.max(title_min).max(footer_min);
    // Center inside the editor rect so the popup floats over the main
    // window, not over the AI side pane / file tree (which sit at fixed
    // columns and have their own focus / chrome).
    let rect = app.editor_rect();
    let area_w = rect.w as usize;
    let area_h = rect.h as usize;
    if content_w + 2 > area_w.saturating_sub(4) {
        content_w = area_w.saturating_sub(6);
    }
    let popup_w = content_w + 2;
    let popup_h = wk.entries.len() + 3; // top + N entries + footer + bottom

    let popup_h = popup_h.min(area_h.saturating_sub(2));
    let max_entries = popup_h.saturating_sub(3);

    let left = rect.x as usize + area_w.saturating_sub(popup_w) / 2;
    let top = rect.y as usize + area_h.saturating_sub(popup_h) / 2;

    let bg = app.config.chrome_bg();
    let border = app.config.theme_border();
    let title_fg = app.config.theme_emphasis();
    let key_fg = app
        .config
        .color_for_capture("keyword")
        .unwrap_or(Color::Rgb {
            r: 0xcb,
            g: 0xa6,
            b: 0xf7,
        });
    let label_fg = app.config.theme_fg();
    let arrow_fg = app.config.theme_dim();
    let hint_fg = app.config.theme_dim();

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

/// Modal LSP-rename preview overlay. Grouped by file (a non-selectable
/// header per file, then one checkbox row per edit underneath). The
/// user navigates between edit rows with `j` / `k`, toggles with
/// `<Space>`, and accepts (`<Enter>`) or cancels (`<Esc>`).
fn draw_rename_preview(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(preview) = app.pending_rename_preview.as_ref() else {
        return Ok(());
    };
    if preview.edits.is_empty() {
        return Ok(());
    }
    let total_w = app.width as usize;
    let total_h = app.height as usize;
    if total_w < 40 || total_h < 8 {
        return Ok(());
    }

    // Popup occupies ~85% of the host width, bounded to fit. Height
    // is whatever the body + chrome would naturally take, capped at
    // `total_h - 2`.
    let popup_w = (total_w * 85 / 100).clamp(50, total_w.saturating_sub(4));
    let content_w = popup_w.saturating_sub(2);
    if content_w < 30 {
        return Ok(());
    }

    // Build the display-row list: alternating file-headers + edit rows
    // in source order. The cursor index in state refers to edits; we
    // need to look up which display row the cursor is currently on.
    enum RowKind {
        Header(std::path::PathBuf, usize),
        Edit(usize),
    }
    let mut rows: Vec<RowKind> = Vec::new();
    let mut last: Option<&std::path::Path> = None;
    for (i, e) in preview.edits.iter().enumerate() {
        if last != Some(&e.edit.path) {
            // Count edits in this file for the header tally.
            let n = preview
                .edits
                .iter()
                .filter(|x| x.edit.path == e.edit.path)
                .count();
            rows.push(RowKind::Header(e.edit.path.clone(), n));
            last = Some(&e.edit.path);
        }
        rows.push(RowKind::Edit(i));
    }
    let cursor_row = rows
        .iter()
        .position(|r| matches!(r, RowKind::Edit(i) if *i == preview.cursor))
        .unwrap_or(0);

    // Body height = popup_h - top_border - bottom_border - footer (2 hint rows + a divider).
    let footer_h = 3usize; // divider + 2 hint lines
    let max_popup_h = total_h.saturating_sub(2);
    let want_h = rows.len() + 2 + footer_h + 1; // +1 spacer below header border
    let popup_h = want_h.min(max_popup_h).max(footer_h + 4);
    let body_h = popup_h.saturating_sub(footer_h + 2);
    if body_h == 0 {
        return Ok(());
    }
    // Keep cursor visible — adjust the scroll the renderer uses
    // locally; we don't write back into App state from a `&App`
    // render path. The handler's loose clamp keeps us in the right
    // ballpark; this just trims further to the live body height.
    let stash_scroll = preview.scroll.min(rows.len().saturating_sub(1));
    let scroll = if cursor_row < stash_scroll {
        cursor_row
    } else if cursor_row >= stash_scroll + body_h {
        cursor_row.saturating_sub(body_h.saturating_sub(1))
    } else {
        stash_scroll
    };

    let left = total_w.saturating_sub(popup_w) / 2;
    let top = total_h.saturating_sub(popup_h) / 2;
    let bg = app.config.chrome_bg();
    let border = app.config.theme_border();
    let title_fg = app.config.theme_emphasis();
    let label_fg = app.config.theme_fg();
    let dim_fg = app.config.theme_dim();
    let header_fg = app
        .config
        .color_for_capture("type")
        .unwrap_or_else(|| app.config.theme_emphasis());
    let accent = app
        .config
        .color_for_capture("keyword")
        .unwrap_or_else(|| app.config.theme_emphasis());
    let disabled_fg = app.config.theme_dim();
    let selected_bg = app
        .config
        .color_for_capture("surface")
        .unwrap_or_else(|| app.config.chrome_bg());

    // ── Top border with embedded title ──────────────────────────────────
    let title = format!(
        " {}  ({} edit{} · {} file{} · {} selected) ",
        preview.title_prefix(),
        preview.edits.len(),
        if preview.edits.len() == 1 { "" } else { "s" },
        preview.files_affected(),
        if preview.files_affected() == 1 {
            ""
        } else {
            "s"
        },
        preview.enabled_count(),
    );
    let title_chars: String = title.chars().take(content_w.saturating_sub(4)).collect();
    let title_w = title_chars.chars().count();
    let pre = content_w.saturating_sub(title_w) / 2;
    let post = content_w.saturating_sub(title_w + pre);
    queue!(
        out,
        MoveTo(left as u16, top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╭'),
        Print("─".repeat(pre)),
        SetForegroundColor(title_fg),
        SetAttribute(Attribute::Bold),
        Print(&title_chars),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(post)),
        Print('╮'),
    )?;

    // ── Body rows ───────────────────────────────────────────────────────
    let visible: Vec<&RowKind> = rows.iter().skip(scroll).take(body_h).collect();
    for (i, row) in visible.iter().enumerate() {
        let y = (top + 1 + i) as u16;
        queue!(
            out,
            MoveTo(left as u16, y),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
        )?;
        match row {
            RowKind::Header(path, n) => {
                // Path rendered relative to cwd when possible — easier
                // to scan than a full absolute path.
                let display_path = match std::env::current_dir() {
                    Ok(cwd) => path
                        .strip_prefix(&cwd)
                        .unwrap_or(path)
                        .display()
                        .to_string(),
                    Err(_) => path.display().to_string(),
                };
                let tail = format!(" [{}]", n);
                let tail_w = tail.chars().count();
                let head_max = content_w.saturating_sub(2 + tail_w);
                let display_path: String = display_path.chars().take(head_max).collect();
                let used = display_path.chars().count() + 1 + tail_w + 1;
                queue!(
                    out,
                    SetBackgroundColor(bg),
                    Print(' '),
                    SetForegroundColor(header_fg),
                    SetAttribute(Attribute::Bold),
                    Print(&display_path),
                    SetAttribute(Attribute::Reset),
                    SetBackgroundColor(bg),
                    SetForegroundColor(dim_fg),
                    Print(&tail),
                )?;
                let pad = content_w.saturating_sub(used);
                queue!(out, SetBackgroundColor(bg), Print(" ".repeat(pad)))?;
            }
            RowKind::Edit(idx) => {
                let e = &preview.edits[*idx];
                let is_cursor = *idx == preview.cursor;
                let row_bg = if is_cursor { selected_bg } else { bg };
                let cursor_glyph = if is_cursor { '>' } else { ' ' };
                let checkbox = if e.enabled { "[x]" } else { "[ ]" };
                let line_no = format!("{}:{}", e.edit.start_line + 1, e.edit.start_col + 1);
                // Render the after-line, trimmed for width and tab-
                // collapsed so a deeply-indented row doesn't push the
                // rename off the right edge.
                let preview_text = collapse_tabs(&e.after_text);
                // Layout: ` > [x]  ll:cc  preview…`
                // 1 cursor + 1 sp + 3 checkbox + 2 sp + ll:cc + 2 sp + preview
                let fixed = 1 + 1 + 3 + 2 + line_no.chars().count() + 2;
                let preview_max = content_w.saturating_sub(fixed + 1);
                let preview_trunc: String = preview_text.chars().take(preview_max).collect();
                let preview_w = preview_trunc.chars().count();
                let pad = content_w.saturating_sub(fixed + preview_w);
                let check_fg = if e.enabled { accent } else { disabled_fg };
                let text_fg = if e.enabled { label_fg } else { disabled_fg };
                queue!(
                    out,
                    SetBackgroundColor(row_bg),
                    SetForegroundColor(if is_cursor { accent } else { dim_fg }),
                    Print(cursor_glyph),
                    Print(' '),
                    SetForegroundColor(check_fg),
                    Print(checkbox),
                    SetForegroundColor(dim_fg),
                    Print("  "),
                    Print(&line_no),
                    SetForegroundColor(text_fg),
                    Print("  "),
                    Print(&preview_trunc),
                    Print(" ".repeat(pad)),
                )?;
            }
        }
        queue!(
            out,
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
        )?;
    }
    // Pad any unused body rows with empty borders so the popup stays
    // rectangular when the rename has fewer edits than `body_h`.
    if visible.len() < body_h {
        for i in visible.len()..body_h {
            let y = (top + 1 + i) as u16;
            queue!(
                out,
                MoveTo(left as u16, y),
                SetBackgroundColor(bg),
                SetForegroundColor(border),
                Print('│'),
                Print(" ".repeat(content_w)),
                Print('│'),
            )?;
        }
    }

    // ── Footer ──────────────────────────────────────────────────────────
    let footer_top = top + 1 + body_h;
    // Divider
    queue!(
        out,
        MoveTo(left as u16, footer_top as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        Print(" ".repeat(content_w)),
        Print('│'),
    )?;
    let hint1 = " j/k move · <Space> toggle · a all · n none ";
    let hint2 = format!(
        " <Enter> apply {} · <Esc> cancel · o open file at edit ",
        preview.enabled_count()
    );
    for (i, hint) in [hint1.to_string(), hint2].iter().enumerate() {
        let y = (footer_top + 1 + i) as u16;
        let hint_chars: String = hint.chars().take(content_w).collect();
        let pad = content_w.saturating_sub(hint_chars.chars().count());
        queue!(
            out,
            MoveTo(left as u16, y),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print('│'),
            SetForegroundColor(dim_fg),
            Print(&hint_chars),
            Print(" ".repeat(pad)),
            SetForegroundColor(border),
            Print('│'),
        )?;
    }

    // ── Bottom border ───────────────────────────────────────────────────
    queue!(
        out,
        MoveTo(left as u16, (top + popup_h - 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('╰'),
        Print("─".repeat(content_w)),
        Print('╯'),
        ResetColor,
    )?;
    Ok(())
}

/// Collapse runs of tabs in `s` into single spaces. Source lines often
/// lead with tab indentation; printing them verbatim in the overlay
/// stretches the row out by 8 cells per tab. A single space gives the
/// reader the shape of the line without burning the width budget.
fn collapse_tabs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_tab = false;
    for ch in s.chars() {
        if ch == '\t' {
            if !last_was_tab {
                out.push(' ');
            }
            last_was_tab = true;
        } else {
            out.push(ch);
            last_was_tab = false;
        }
    }
    out
}

fn draw_signature_popup(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(sig) = app.signature_help.as_ref() else {
        return Ok(());
    };
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
    let cursor_row = app.window.cursor.line.saturating_sub(app.window.view_top);
    // Prefer above the cursor — call sites have the popup above so it
    // doesn't cover the line you're typing into.
    let mut top_row = cursor_row.saturating_sub(popup_h);
    if cursor_row < popup_h {
        top_row = (cursor_row + 1).min(buffer_rows.saturating_sub(popup_h));
    }
    // Buffer-relative → screen y.
    let top_row = top_row + app.buffer_top();
    let gutter = app.gutter_width();
    let cursor_visual = app.cursor_visual_col().saturating_sub(app.window.view_left);
    let mut left_col = gutter + cursor_visual;
    if left_col + popup_w > total_w {
        left_col = total_w.saturating_sub(popup_w);
    }

    let bg = app.config.chrome_bg();
    let border = app.config.theme_border();
    let text_fg = app.config.theme_fg();
    let active_fg = app.config.theme_chip_fg();
    let active_bg = app.config.theme_warning();

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

    let Some(hover) = app.hover.as_ref() else {
        return Ok(());
    };
    if hover.lines.is_empty() {
        return Ok(());
    }

    let widest_actual = hover.widest_line().max(20);
    let content_w = widest_actual.min(hover.wrap_width).max(20);
    let popup_w = content_w + 2;

    // Height: cap at HOVER_MAX_HEIGHT, also cap at half the screen.
    let total_h = app.height as usize;
    let max_visible = crate::app::HOVER_MAX_HEIGHT.min(total_h.saturating_sub(4).max(4));
    let visible = hover.lines.len().min(max_visible);
    let popup_h = visible + 2;

    // Position: prefer below cursor; flip above if overflow.
    let buffer_rows = app.buffer_rows();
    let cursor_row = app.window.cursor.line.saturating_sub(app.window.view_top);
    let mut top_row = cursor_row + 1;
    if top_row + popup_h > buffer_rows {
        top_row = cursor_row.saturating_sub(popup_h);
    }
    let top_row = top_row + app.buffer_top();
    let gutter = app.gutter_width();
    let mut left_col = gutter + app.window.cursor.col;
    if left_col + popup_w > app.width as usize {
        left_col = (app.width as usize).saturating_sub(popup_w);
    }

    let bg = app.config.chrome_bg();
    let border = app.config.theme_border();
    let text_fg = app.config.theme_fg();
    let title_fg = app.config.theme_emphasis();
    let arrow_fg = app.config.theme_dim();
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
            Some(HoverLine::Code {
                block_idx,
                byte_offset,
                byte_len,
            }) => {
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

/// Paint a phantom row above a buffer line carrying its
/// `textDocument/codeLens` titles. Empty gutter (no line number, no
/// git stripe — phantom rows have no buffer position to anchor on),
/// then the lens titles joined by ` │ ` in the dim theme tone so the
/// row scans as commentary rather than code. Truncated to the line's
/// available width.
fn paint_code_lens_row(
    out: &mut impl Write,
    app: &App,
    line_idx: usize,
    gutter: usize,
    avail: usize,
    buf_bg: Option<Color>,
) -> Result<()> {
    let Some(path) = app.buffer.path.as_ref() else {
        return Ok(());
    };
    let Some(cache) = app.code_lens.get(path) else {
        return Ok(());
    };
    // Not gated on `cache.buffer_version` — `refresh_merged_code_lens`
    // keeps stale LSP entries in the merge across edits to stop the
    // viewport from reflowing on every keystroke. The titles match
    // the slightly-stale lens positions and snap back into place
    // once the LSP responds at the new version.
    // Pad the gutter so the lens text aligns with the buffer body
    // column, not the line-number column. Without this the title
    // would start at col 0 and overlap any future gutter glyph.
    queue!(out, Print(" ".repeat(gutter)))?;
    let mut parts: Vec<&str> = Vec::new();
    for lens in &cache.lenses {
        if lens.line != line_idx {
            continue;
        }
        if let Some(cmd) = &lens.command {
            if !cmd.title.is_empty() {
                parts.push(cmd.title.as_str());
            }
        }
    }
    if parts.is_empty() {
        return Ok(());
    }
    let text = parts.join(" │ ");
    let truncated: String = text.chars().take(avail).collect();
    queue!(
        out,
        SetForegroundColor(app.config.theme_dim()),
        Print(truncated),
    )?;
    reset_to_buf_bg(out, buf_bg)?;
    Ok(())
}

/// Classify a status message by content into a Catppuccin severity colour. We
/// avoid threading a level enum through every callsite by reading the prefix
/// patterns at render time.
fn notification_color(app: &App, msg: &str) -> Color {
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
        return app.config.notification_error();
    }
    // Success: file write, substitution count, range yank / delete summaries.
    if lower.contains("written")
        || lower.contains("substitution")
        || lower.ends_with(" yanked")
        || lower.ends_with(" deleted")
        || lower.starts_with("recorded ")
        || lower.starts_with("kept buffer")
    {
        return app.config.notification_success();
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
        return app.config.notification_warning();
    }
    app.config.notification_info()
}

/// Maximum content rows in a notification box before extra wrapped lines
/// get truncated with an ellipsis. Six is enough to read a typical path or
/// error message without letting a stack trace eat the whole screen.
const NOTIFICATION_MAX_ROWS: usize = 6;

fn draw_notification(out: &mut impl Write, app: &App) -> Result<()> {
    // Cmdline and search modes get the centred box; their floating widget covers any notification.
    if matches!(
        app.mode,
        Mode::Command | Mode::Search { .. } | Mode::Prompt(_)
    ) {
        return Ok(());
    }
    if app.status_msg.is_empty() {
        return Ok(());
    }
    let level = notification_color(app, &app.status_msg);

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
    let inner_w = wrapped.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 2; // padding inside borders
    let box_w = inner_w + 2;
    let left = total_w.saturating_sub(box_w + 1);
    // Sit immediately below the tab bar when it's visible so the
    // notification doesn't overlap any tab labels. `buffer_top()` is
    // 1 when tabs are showing, 0 otherwise — same offset the buffer
    // body uses.
    let top = app.buffer_top();

    let bg = app.config.chrome_bg();
    let text_fg = app.config.theme_fg();

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
        Mode::Prompt(crate::mode::PromptKind::FileTreeCreate) => ("New entry", ' '),
        Mode::Prompt(crate::mode::PromptKind::FileTreeRename) => ("Rename", ' '),
        Mode::Prompt(crate::mode::PromptKind::AndroidAvdName) => ("AVD name", ' '),
        _ => ("", ' '),
    }
}

fn draw_floating_cmdline(out: &mut impl Write, app: &App) -> Result<()> {
    let (left, top, box_w) = cmdline_box_layout(app);
    let (title, prompt) = cmdline_chrome(app.mode);
    let inner_w = box_w.saturating_sub(2);

    let border = app.config.theme_border();
    let bg = app.config.chrome_bg();
    let title_fg = app.config.theme_emphasis();
    let prompt_fg = app.config.theme_info();
    let text_fg = app.config.theme_fg();

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

    // Input row. The cursor is painted as a highlighted cell at
    // `cmdline_cursor` (a byte offset into `cmdline`), splitting the
    // input into before / under / after. The under-cursor char shows
    // in inverted colours so the cursor reads as a block regardless
    // of terminal cursor visibility — some terminals drop visibility
    // after a SetCursorStyle change inside a synchronized update,
    // and a system-cursor block against the popup bg is finicky.
    // Painting the cell explicitly sidesteps all of that.
    let cursor_byte = app.cmdline_cursor.min(app.cmdline.len());
    let (before, rest) = app.cmdline.split_at(cursor_byte);
    let (under, after) = match rest.chars().next() {
        Some(ch) => rest.split_at(ch.len_utf8()),
        None => ("", ""),
    };
    // Truncate from the END so the cursor stays visible when input
    // exceeds the popup width.
    let avail = inner_w.saturating_sub(4); // 1 border + 3 prompt segment
    let before_w = before.chars().count();
    let under_w = under.chars().count();
    let after_w = after.chars().count();
    let mut total = before_w + under_w + after_w;
    let mut after_trim = after.to_string();
    while total > avail && !after_trim.is_empty() {
        if let Some((idx, _)) = after_trim.char_indices().next_back() {
            after_trim.truncate(idx);
            total = total.saturating_sub(1);
        } else {
            break;
        }
    }
    let cursor_bg = app.config.theme_fg();
    let cursor_fg = app.config.chrome_bg();
    // Cursor cell always renders: under-char if present, else a space
    // (so end-of-input still has a visible cursor block).
    let cursor_glyph = if under.is_empty() {
        " ".to_string()
    } else {
        under.to_string()
    };
    let used = 3 + before_w + 1 + after_trim.chars().count(); // prompt + before + cursor + after
    let pad = inner_w.saturating_sub(used + 1).saturating_sub(0); // -1 for the right border
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
        Print(before),
        SetBackgroundColor(cursor_bg),
        SetForegroundColor(cursor_fg),
        Print(&cursor_glyph),
        SetBackgroundColor(bg),
        SetForegroundColor(text_fg),
        Print(&after_trim),
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

/// Confirm popup for `d` (delete) in the file-tree pane. Same chrome
/// as the floating cmdline (title in the top border, single-row body,
/// rounded corners) so the three file-tree ops — create / rename /
/// delete — look uniform. The y/N keystroke is intercepted in
/// `handle_file_tree_key`, not the prompt handler; this popup is
/// purely visual.
fn draw_file_tree_confirm(out: &mut impl Write, app: &App) -> Result<()> {
    let Some((name, is_dir)) = app.file_tree_pending_delete() else {
        return Ok(());
    };
    let (left, top, box_w) = cmdline_box_layout(app);
    let inner_w = box_w.saturating_sub(2);

    let border = app.config.theme_border();
    let bg = app.config.chrome_bg();
    let title_fg = app.config.theme_emphasis();
    let prompt_fg = app.config.theme_error();
    let text_fg = app.config.theme_fg();
    let dim_fg = app.config.theme_dim();

    // Top border with centred title.
    let title_text = " Delete ";
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
        Print(title_text),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("─".repeat(right_pad)),
        Print('╮'),
    )?;

    // Body row. Layout matches the cmdline: `│ ! <target>  <hint> │`
    // with the prompt glyph in the accent error colour so it reads
    // as "destructive". The hint is dim and right-aligned.
    let prompt_str = " ! ";
    let display_name = if is_dir { format!("{name}/") } else { name };
    let hint = "  y to delete · N / Esc to cancel";
    let prompt_w = prompt_str.chars().count();
    // Truncate the displayed name if the row is tight. Keep the hint
    // intact when there's room; drop it altogether on very narrow
    // popups so the name doesn't get squashed.
    let body_budget = inner_w.saturating_sub(prompt_w + 1);
    let hint_w = hint.chars().count();
    let (name_str, hint_str): (String, String) =
        if body_budget >= display_name.chars().count() + hint_w {
            (display_name.clone(), hint.to_string())
        } else if body_budget >= display_name.chars().count() {
            (display_name.clone(), String::new())
        } else {
            let trimmed: String = display_name.chars().take(body_budget).collect();
            (trimmed, String::new())
        };
    // Same explicit-cursor trick as the cmdline popup: paint a
    // highlighted cell as the "your y/N keystroke lands here"
    // indicator rather than relying on the terminal cursor.
    let cursor_w = 1usize;
    let used = prompt_w + name_str.chars().count() + hint_str.chars().count() + cursor_w;
    let pad = inner_w.saturating_sub(used + 1);
    let cursor_bg = app.config.theme_fg();
    let cursor_fg = app.config.chrome_bg();
    queue!(
        out,
        MoveTo(left as u16, (top + 1) as u16),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print('│'),
        SetForegroundColor(prompt_fg),
        SetAttribute(Attribute::Bold),
        Print(prompt_str),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(bg),
        SetForegroundColor(text_fg),
        Print(&name_str),
        SetForegroundColor(dim_fg),
        Print(&hint_str),
        SetBackgroundColor(cursor_bg),
        SetForegroundColor(cursor_fg),
        Print(' '),
        SetBackgroundColor(bg),
        SetForegroundColor(text_fg),
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
    let Some(c) = app.completion.as_ref() else {
        return Ok(());
    };
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
    let cursor_row = app.window.cursor.line.saturating_sub(app.window.view_top);
    let cursor_col = gutter + app.window.cursor.col;
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

    let bg_unsel = app.config.chrome_bg();
    let bg_sel = app.config.theme_surface();
    let label_unsel = app.config.theme_fg();
    let label_sel = app.config.theme_emphasis();
    let detail_fg = app.config.theme_dim();

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

        let (chip_text, chip_color) = completion_kind_chip(app, item.kind.as_deref());
        let chip_pad = CHIP_W.saturating_sub(chip_text.chars().count());

        let label: String = item.label.chars().take(final_label_w).collect();
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
fn completion_kind_chip(app: &App, kind: Option<&str>) -> (&'static str, Color) {
    let yellow = app.config.theme_warning();
    let blue = app.config.theme_info();
    let mauve = app
        .config
        .color_for_capture("keyword")
        .unwrap_or(Color::Rgb {
            r: 0xcb,
            g: 0xa6,
            b: 0xf7,
        });
    let teal = app
        .config
        .color_for_capture("character")
        .unwrap_or(Color::Rgb {
            r: 0x94,
            g: 0xe2,
            b: 0xd5,
        });
    let peach = app.config.theme_accent();
    let green = app.config.theme_accent_secondary();
    let sky = app.config.theme_hint();
    let subtext1 = app.config.theme_fg();
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
    // Float over the editor rect (the main-window region), not the
    // full terminal — keeps the popup centred when the AI side pane or
    // file-tree pane has narrowed the editor area.
    let rect = app.editor_rect();
    let area_w = rect.w as usize;
    let area_h = rect.h as usize;
    // Box dimensions — generous side margins so the popup floats clearly
    // above the dimmed buffer rather than touching the screen edges.
    let box_w = ((area_w * 4) / 5)
        .clamp(50, 100)
        .min(area_w.saturating_sub(4));
    // 7 rows of chrome: top border, top pad, prompt, separator, …, bottom
    // pad, footer, bottom border. Min 12 keeps at least 5 list rows visible.
    let box_h = ((area_h * 3) / 5)
        .clamp(12, 28)
        .min(area_h.saturating_sub(2));

    let inner_w = box_w.saturating_sub(2);
    let left = rect.x as usize + area_w.saturating_sub(box_w) / 2;
    // Bias slightly above centre so the popup doesn't visually fight the
    // status line.
    let bottom_chrome = 2;
    let top =
        rect.y as usize + (area_h.saturating_sub(bottom_chrome).saturating_sub(box_h) / 2).max(0);

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
    let Some(picker) = app.picker.as_ref() else {
        return Ok(());
    };
    let layout = picker_layout(app);

    let bg = app.config.chrome_bg();
    let border = app.config.theme_border();
    let title_fg = app.config.theme_emphasis();
    let count_fg = app.config.theme_dim();
    let prompt_fg = app.config.theme_accent();
    let input_fg = app.config.theme_fg();
    let path_fg = app.config.theme_dim();
    let name_fg = app.config.theme_fg();
    let dim_fg = app.config.theme_dim();
    let sel_bg = app.config.theme_surface();
    let sel_accent = app.config.theme_emphasis();
    let hint_fg = app.config.theme_dim();
    let hint_key_fg = app.config.theme_fg();

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
            // A `marked` row (the installed version in the version picker)
            // renders in the info colour so it stands out from its siblings;
            // selection styling still wins when the cursor is on it.
            let (path_fg, name_fg) = if picker.marked == Some(item_idx) && !selected {
                let c = app.config.theme_info();
                (c, c)
            } else {
                (path_fg, name_fg)
            };
            // Matched-char positions are stored per-filtered-row alongside
            // the indices into items — empty when the picker has no query.
            let positions = picker
                .match_positions
                .get(pos)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            written = paint_picker_row(
                out,
                display,
                body_w,
                selected,
                path_fg,
                name_fg,
                dim_fg,
                app.config.theme_warning(),
                show_icon,
                positions,
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
    let seg_width =
        |segs: &[(&str, bool)]| -> usize { segs.iter().map(|(s, _)| s.chars().count()).sum() };
    let hint_segments: &[(&str, bool)] = if seg_width(full_hint) <= layout.inner_w {
        full_hint
    } else if seg_width(short_hint) <= layout.inner_w {
        short_hint
    } else {
        &[]
    };
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
    highlight_fg: Color,
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
    let highlight = highlight_fg;
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
        paint_chars(
            out,
            suffix_slice,
            path_fg,
            highlight,
            matched,
            suffix_offset,
        )?;
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
                SetForegroundColor(if highlighted {
                    highlight_color
                } else {
                    base_color
                }),
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
    // Constrain to the editor rect so the AI side pane (`:claude` /
    // `:opencode` / `:codex`) and the file-tree pane keep their columns —
    // the side pane is painted after us, but centering the logo on the
    // full terminal width pushes it into (or under) those panes.
    let rect = app.editor_rect();
    let rows = rect.h as usize;
    let top = rect.y as usize;
    let left_off = rect.x as usize;
    let area_w = rect.w as usize;
    let page_bg = app.config.background_color();
    let blank: String = " ".repeat(area_w);
    for row in 0..rows {
        queue!(out, MoveTo(left_off as u16, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank))?;
        } else {
            // Print spaces rather than ClearType::CurrentLine so we don't
            // wipe the file-tree / side-pane columns to our left and right.
            queue!(out, Print(&blank))?;
        }
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
    if block_w == 0 || block_w > area_w {
        return Ok(());
    }
    let block_h = lines.len();
    if block_h > rows {
        return Ok(());
    }
    let logo_top = (rows.saturating_sub(block_h)) / 2;
    let blue = app.config.theme_info();
    for (i, line) in lines.iter().enumerate() {
        let line_w = line.chars().count();
        let left = left_off + (area_w.saturating_sub(line_w)) / 2;
        queue!(out, MoveTo(left as u16, (top + logo_top + i) as u16))?;
        apply_buf_bg(out, page_bg)?;
        queue!(out, SetForegroundColor(blue), Print(line))?;
        reset_to_buf_bg(out, page_bg)?;
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
    let page_bg = app.config.background_color();
    let blank: String = " ".repeat(total_w);
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank))?;
        } else {
            queue!(out, Clear(ClearType::CurrentLine))?;
        }
    }

    let snap = app.build_health_snapshot();
    let p = DashboardPalette::from_config(&app.config);

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

    for (i, row) in rows_buf.iter().enumerate().skip(scroll).take(viewport_rows) {
        let screen_y = (top + (i - scroll)) as u16;
        row.paint(out, screen_y, &p, page_bg)?;
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
    queue!(out, MoveTo(left as u16, (top + rows - 1) as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(footer, total_w.saturating_sub(left))),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

// ─── :install overlay ─────────────────────────────────────────────────────

const INSTALL_BANNER: &[&str] = &[
    "██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗",
    "██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║",
    "██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║",
    "██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    "██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    "╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];

fn draw_install_page(out: &mut impl Write, app: &App) -> Result<()> {
    use crate::app::installer::{
        InstallerKind, InstallerStage, bundle_picker_rows, node_picker_rows, plan_rows,
    };

    let Some(state) = app.installer.as_ref() else {
        return Ok(());
    };
    let total_w = app.width as usize;
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    if rows == 0 || total_w < 30 {
        return Ok(());
    }
    let page_bg = app.config.background_color();
    let blank: String = " ".repeat(total_w);
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank))?;
        } else {
            queue!(out, Clear(ClearType::CurrentLine))?;
        }
    }

    let p = DashboardPalette::from_config(&app.config);
    let left = 2usize;
    let body_w = total_w.saturating_sub(left + 2).max(40);

    // ── Banner ──
    let banner_fits = total_w >= 50 && rows > 12;
    let mut cursor_y = top;
    if banner_fits {
        for line in INSTALL_BANNER {
            queue!(out, MoveTo(left as u16, cursor_y as u16))?;
            apply_buf_bg(out, page_bg)?;
            queue!(
                out,
                SetForegroundColor(p.mauve),
                SetAttribute(Attribute::Bold),
                Print(line),
                SetAttribute(Attribute::Reset),
            )?;
            reset_to_buf_bg(out, page_bg)?;
            cursor_y += 1;
        }
        cursor_y += 1;
    }

    // ── Subtitle ──
    queue!(out, MoveTo(left as u16, cursor_y as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(&state.subtitle, body_w)),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    cursor_y += 2;

    // ── Help line ──
    let help = match state.stage {
        InstallerStage::Bundles | InstallerStage::NodeVersions => {
            "j/k move · Space toggle · a all · n none · Enter confirm · q quit"
        }
        InstallerStage::Plan => match state.kind {
            InstallerKind::Install => "y install · n back · q quit",
            InstallerKind::Update => "y update · n back · q quit",
        },
    };
    queue!(out, MoveTo(left as u16, cursor_y as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(help, body_w)),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    cursor_y += 2;

    // ── Body ──
    let body_top = cursor_y;
    let viewport = rows.saturating_sub(cursor_y - top + 1);
    match state.stage {
        InstallerStage::Bundles => {
            let rows_data = bundle_picker_rows(state);
            paint_checkbox_list(
                out,
                app,
                &rows_data,
                state.cursor,
                &state.checked,
                body_top,
                body_w,
                viewport,
                &p,
                page_bg,
            )?;
        }
        InstallerStage::NodeVersions => {
            let rows_data = node_picker_rows(state);
            paint_checkbox_list(
                out,
                app,
                &rows_data,
                state.cursor,
                &state.checked,
                body_top,
                body_w,
                viewport,
                &p,
                page_bg,
            )?;
        }
        InstallerStage::Plan => {
            let rows_data = plan_rows(state);
            paint_plan_rows(
                out, app, &rows_data, body_top, body_w, viewport, &p, page_bg,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_checkbox_list(
    out: &mut impl Write,
    _app: &App,
    rows: &[crate::app::installer::PickerRow],
    cursor: usize,
    checked: &[bool],
    top: usize,
    body_w: usize,
    viewport: usize,
    p: &DashboardPalette,
    page_bg: Option<Color>,
) -> Result<()> {
    use crate::app::installer::PickerSummary;
    if rows.is_empty() {
        return Ok(());
    }
    let name_w = rows
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(20);
    // Simple scroll — keep the cursor visible.
    let scroll = if cursor >= viewport {
        cursor + 1 - viewport
    } else {
        0
    };
    for (offset, (i, row)) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport)
        .enumerate()
    {
        let y = (top + offset) as u16;
        queue!(out, MoveTo(2, y))?;
        apply_buf_bg(out, page_bg)?;
        let active = i == cursor;
        if active {
            queue!(
                out,
                SetForegroundColor(p.peach),
                SetAttribute(Attribute::Bold),
                Print("▸ "),
            )?;
        } else {
            queue!(out, Print("  "))?;
        }
        let mark = if *checked.get(i).unwrap_or(&false) {
            "[x]"
        } else {
            "[ ]"
        };
        let mark_color = if *checked.get(i).unwrap_or(&false) {
            p.green
        } else {
            p.overlay0
        };
        queue!(out, SetForegroundColor(mark_color), Print(mark), Print(" "),)?;
        let name_color = if active { p.peach } else { p.text };
        queue!(out, SetForegroundColor(name_color))?;
        if active {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        let padded = format!("{:<width$}", row.name, width = name_w);
        queue!(out, Print(truncate(&padded, body_w / 3)))?;
        queue!(out, SetAttribute(Attribute::Reset))?;
        // Summary column. `Tools` paints each tool green when it's already on
        // PATH so the user can see what's installed at a glance; everything
        // else is dim.
        let summary_w = body_w.saturating_sub(name_w + 8);
        queue!(out, SetForegroundColor(p.overlay0), Print("  "))?;
        match &row.summary {
            PickerSummary::Plain(s) => {
                queue!(out, Print(truncate(s, summary_w)))?;
            }
            PickerSummary::Tools(tools) => {
                let mut used = 0usize;
                for (ti, t) in tools.iter().enumerate() {
                    if ti > 0 {
                        if used + 3 > summary_w {
                            break;
                        }
                        queue!(out, SetForegroundColor(p.overlay0), Print(" · "))?;
                        used += 3;
                    }
                    let remaining = summary_w.saturating_sub(used);
                    if remaining == 0 {
                        break;
                    }
                    let label = truncate(&t.label, remaining);
                    used += label.chars().count();
                    let color = if t.installed { p.green } else { p.overlay0 };
                    queue!(out, SetForegroundColor(color), Print(label))?;
                }
            }
        }
        reset_to_buf_bg(out, page_bg)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_plan_rows(
    out: &mut impl Write,
    _app: &App,
    rows: &[crate::app::installer::PlanRow],
    top: usize,
    body_w: usize,
    viewport: usize,
    p: &DashboardPalette,
    page_bg: Option<Color>,
) -> Result<()> {
    use crate::app::installer::PlanRowColor;
    if rows.is_empty() {
        return Ok(());
    }
    let mut y = top;
    for (idx, row) in rows.iter().enumerate() {
        if idx >= viewport {
            break;
        }
        queue!(out, MoveTo(2, y as u16))?;
        apply_buf_bg(out, page_bg)?;
        let glyph_color = match row.color {
            PlanRowColor::Green => p.green,
            PlanRowColor::Teal => p.teal,
            PlanRowColor::Yellow => p.yellow,
            PlanRowColor::Red => p.red,
            PlanRowColor::Subtle => p.overlay0,
        };
        queue!(
            out,
            SetForegroundColor(glyph_color),
            Print(row.glyph),
            SetForegroundColor(p.text),
            Print(&row.label),
            SetForegroundColor(p.overlay0),
            Print(format!("  [{}]  ", row.role)),
            Print(truncate(
                &row.detail,
                body_w.saturating_sub(row.label.chars().count() + 12)
            )),
        )?;
        reset_to_buf_bg(out, page_bg)?;
        y += 1;
        if !row.target.is_empty() && (y - top) < viewport {
            queue!(out, MoveTo(8, y as u16))?;
            apply_buf_bg(out, page_bg)?;
            queue!(
                out,
                SetForegroundColor(p.overlay0),
                Print(truncate(&row.target, body_w.saturating_sub(8))),
            )?;
            reset_to_buf_bg(out, page_bg)?;
            y += 1;
        }
    }
    Ok(())
}

/// `:messages` overlay — one row per captured `window/showMessage` /
/// `window/logMessage` notification, scrollable. Newest is at the top
/// so the most recent entries are visible on open. Severity colours
/// match diagnostics (red error / yellow warn / blue info / overlay
/// log) so the user scans them the same way as inline diagnostics.
/// `:terminal` bottom pane — paints the PTY grid in the rows
/// reserved by `terminal_pane_rows()`, leaving the buffer area
/// above and the status line / debug pane below. Cell-for-cell
/// paint with the SGR colour + attr fidelity the host terminal
/// supports. The grid's logical size is kept in sync with the
/// pane's row/col count by `cmd_open_terminal` / the resize
/// handler, so we never have to scale here. Active-cursor +
/// selection highlights overlay the cells.
fn draw_terminal_pane(out: &mut impl Write, app: &App) -> Result<()> {
    if !app.terminal_pane_open {
        return Ok(());
    }
    let pane_rows = app.terminal_pane_rows();
    if pane_rows == 0 {
        return Ok(());
    }
    let top = app.terminal_pane_top();
    let total_w = app.width as usize;
    if total_w == 0 {
        return Ok(());
    }

    // The first row of the pane is a header chip that doubles as a
    // visual separator from the buffer area above. Same model as
    // the debug pane's header. With one terminal the right side of
    // the header carries the hint line; with two or more it
    // carries a clickable tab strip (active tab = blue bg + white
    // text). The [TERMINAL] chip itself stays constant.
    let pane_bg = app.config.chrome_bg();
    let muted = app.config.theme_dim();
    let base = app.config.terminal_chip_fg();
    let accent_terminal = app.config.terminal_chip_bg();
    let active_tab_bg = app.config.terminal_active_tab_bg();
    let label = " TERMINAL ";
    let label_w = label.chars().count() as u16;
    let chip_bg = match app.mode {
        Mode::Terminal => accent_terminal,
        _ => muted,
    };
    queue!(out, MoveTo(0, top as u16), Clear(ClearType::CurrentLine))?;
    queue!(
        out,
        SetBackgroundColor(chip_bg),
        SetForegroundColor(base),
        SetAttribute(Attribute::Bold),
        Print(label),
        SetAttribute(Attribute::Reset),
    )?;

    let mut used = label_w as usize;
    let term_count = app.terminals.len();
    let mut hitboxes: Vec<(usize, u16, u16)> = Vec::new();
    if term_count > 1 {
        // Tab strip. Single-cell gap between tabs so the active
        // blue chip doesn't butt into the next label.
        queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
        used += 1;
        let mut tab_x: u16 = label_w + 1;
        for idx in 0..term_count {
            // Labelled tabs (e.g. "build" / "dev" set by the task
            // runner) show the name; un-labelled shells fall back to
            // the positional number so the strip stays predictable
            // for a freshly opened `:terminal`.
            let tab_label = match app.terminals.get(idx).and_then(|t| t.label()) {
                Some(name) => format!(" {} ", name),
                None => format!(" {} ", idx + 1),
            };
            let chip_chars = tab_label.chars().count() as u16;
            let is_active = idx == app.active_terminal_idx;
            let (bg, fg) = if is_active {
                (active_tab_bg, base)
            } else {
                (pane_bg, muted)
            };
            queue!(
                out,
                MoveTo(tab_x, top as u16),
                SetBackgroundColor(bg),
                SetForegroundColor(fg),
            )?;
            if is_active {
                queue!(out, SetAttribute(Attribute::Bold))?;
            } else {
                queue!(out, SetAttribute(Attribute::NormalIntensity))?;
            }
            queue!(out, Print(&tab_label))?;
            queue!(out, SetAttribute(Attribute::NormalIntensity))?;
            hitboxes.push((idx, tab_x, tab_x + chip_chars));
            tab_x += chip_chars + 1;
            // Trailing gap between tabs (painted with pane bg).
            queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
            used = tab_x as usize;
        }
    } else {
        // When the user has scrolled the pane into history, replace
        // the usual hint with a clear "scrolled back" marker so it's
        // obvious why typing won't reach the prompt. Shift+PageDown /
        // mouse-wheel down brings it live again.
        let scroll_back = app.active_terminal().map(|t| t.view_scroll()).unwrap_or(0);
        let hint = if scroll_back > 0 {
            format!(
                "  ↑ {} lines back · Shift+PageDown / scroll down to follow live",
                scroll_back
            )
        } else {
            match app.mode {
                Mode::Terminal => {
                    "  Esc leaves · Ctrl-[ sends Esc · drag selects · Shift+PageUp scrolls".into()
                }
                _ => "  <leader>tf focus · <leader>tq close".into(),
            }
        };
        queue!(
            out,
            SetBackgroundColor(pane_bg),
            SetForegroundColor(muted),
            Print(&hint),
        )?;
        used += hint.chars().count();
    }
    if total_w > used {
        queue!(
            out,
            SetBackgroundColor(pane_bg),
            Print(" ".repeat(total_w - used))
        )?;
    }
    queue!(out, ResetColor)?;
    app.terminal_tab_hitboxes.set(hitboxes);

    // Body = pane minus the header row. The PTY grid is sized to
    // these body rows by `cmd_open_terminal` + the resize handler,
    // so we paint cell-for-cell starting at `top + 1`.
    let body_top = top + 1;
    let body_rows = pane_rows.saturating_sub(1);
    let Some(term) = app.active_terminal() else {
        for r in 0..body_rows {
            queue!(
                out,
                MoveTo(0, (body_top + r) as u16),
                Clear(ClearType::CurrentLine)
            )?;
        }
        return Ok(());
    };
    let inner = term.grid();
    let grid = &inner.handler.grid;
    let grid_rows = grid.rows.min(body_rows);
    let grid_cols = grid.cols.min(total_w);

    // Mouse-drag selection overlay — flip `reverse` on each cell
    // inside the selection range so the user sees what they're
    // grabbing before release copies it to the clipboard. Gated on
    // `tab_idx` so switching tabs mid-drag doesn't leak the
    // highlight into the wrong tab's grid.
    let sel = app
        .terminal_selection
        .as_ref()
        .filter(|s| s.tab_idx == app.active_terminal_idx);
    for row in 0..body_rows {
        let screen_y = (body_top + row) as u16;
        queue!(out, MoveTo(0, screen_y), Clear(ClearType::CurrentLine))?;
        if row < grid_rows {
            // `visible_row` stitches scrollback to the live grid based
            // on the current `view_scroll`. When `view_scroll == 0`
            // the result is `grid.cells[row]` (live tail); when the
            // user has scrolled back into history it's the matching
            // scrollback row instead.
            let Some(line) = grid.visible_row(row) else {
                continue;
            };
            let line_cols = line.len().min(grid_cols);
            for col in 0..line_cols {
                let mut cell = line[col];
                // Wide-char continuation cells are tagged with `\0`
                // by the vte handler. The host terminal already
                // painted the right half of the wide glyph when we
                // printed the lead cell, so we must NOT emit
                // anything here — even a space would clobber it.
                if cell.ch == '\0' {
                    continue;
                }
                if let Some(s) = sel {
                    if s.contains(row, col) {
                        cell.reverse = !cell.reverse;
                    }
                }
                paint_terminal_cell(out, cell)?;
            }
        }
    }
    queue!(out, ResetColor)?;
    // Cursor positioning is intentionally deferred to `place_cursor`
    // — subsequent draw passes (debug pane / status line /
    // notification) MoveTo around, so any Show + MoveTo we emit
    // here gets stomped by the time the frame flushes.
    Ok(())
}

/// Translate one `crate::terminal::Cell` to crossterm style + glyph
/// and emit. Reverse attribute swaps fg/bg before applying.
///
/// Underline is intentionally *not* forwarded to the outer terminal.
/// Claude / Codex / opencode all use SGR 4 as the visual marker for
/// OSC 8 hyperlinks and wrap nearly every clickable chunk of text —
/// rendered into a side pane that doesn't carry the hyperlink
/// affordance, those underlines just stack into what reads as a row
/// of horizontal borders under every line. Stripping the attribute
/// here is lossy (we also lose underline as a styling signal in
/// `man` pages, prompts, etc.) but the trade-off favours the
/// pane-style UX. SGR 4 is still tracked in the cell so the call
/// can be flipped back on with one line if the policy changes.
fn paint_terminal_cell(out: &mut impl Write, cell: crate::terminal::Cell) -> std::io::Result<()> {
    let mut fg = cell.fg.unwrap_or(Color::Reset);
    let mut bg = cell.bg.unwrap_or(Color::Reset);
    if cell.reverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    queue!(
        out,
        SetForegroundColor(fg),
        SetBackgroundColor(bg),
        SetAttribute(Attribute::NoUnderline),
    )?;
    if cell.bold {
        queue!(out, SetAttribute(Attribute::Bold))?;
    } else {
        queue!(out, SetAttribute(Attribute::NormalIntensity))?;
    }
    if cell.italic {
        queue!(out, SetAttribute(Attribute::Italic))?;
    } else {
        queue!(out, SetAttribute(Attribute::NoItalic))?;
    }
    queue!(out, Print(cell.ch))?;
    Ok(())
}

/// Right-side terminal pane — dedicated to AI assistants (`:claude`,
/// `:codex`, `:opencode`). Sits flush against the right edge of the
/// editor band, spanning `buffer_top()..buffer_top()+buffer_rows()`
/// vertically. Header row at the top carries either the active tab's
/// label (single tab) or a clickable tab strip (2+ tabs). Cell-for-
/// cell paint with the bottom pane's SGR fidelity — see
/// `paint_terminal_cell` for the cell painter.
fn draw_file_tree_pane(out: &mut impl Write, app: &App) -> Result<()> {
    let Some(tree) = app.file_tree.as_ref() else {
        return Ok(());
    };
    let pane_cols = app.file_tree_cols();
    if pane_cols == 0 {
        return Ok(());
    }
    let pane_rows = app.buffer_rows();
    if pane_rows == 0 {
        return Ok(());
    }
    let top = app.buffer_top() as u16;
    let content_cols = pane_cols;

    let pane_bg = app.config.chrome_bg();
    let muted = app.config.theme_dim();
    let dir_fg = app.config.file_tree_folder();
    let file_fg = app.config.theme_fg();
    let accent = app.config.mode_picker();
    let title_fg = app.config.theme_emphasis();
    let focused = matches!(app.mode, Mode::FileTree);

    // Header row — title chip + truncated cwd. Highlights when
    // focused so the user can tell at a glance whether keystrokes
    // are going here or to a buffer pane.
    let title = " FILES ";
    let chip_bg = if focused { accent } else { muted };
    let chip = title.chars().take(content_cols).collect::<String>();
    let chip_w = chip.chars().count();
    queue!(
        out,
        MoveTo(0, top),
        SetBackgroundColor(chip_bg),
        SetForegroundColor(app.config.terminal_chip_fg()),
        SetAttribute(Attribute::Bold),
        Print(&chip),
        SetAttribute(Attribute::Reset),
    )?;
    // Pad the rest of the row with pane bg.
    if chip_w < content_cols {
        queue!(
            out,
            SetBackgroundColor(pane_bg),
            SetForegroundColor(title_fg),
            Print(" ".repeat(content_cols - chip_w)),
        )?;
    }

    // Spacer row between the title chip and the first entry — pure
    // breathing room so the FILES chip doesn't crowd the tree.
    let spacer_y = top + 1;
    queue!(
        out,
        MoveTo(0, spacer_y),
        SetBackgroundColor(pane_bg),
        Print(" ".repeat(content_cols)),
    )?;

    // Body — each visible row paints one tree entry (or blanks).
    // Scroll position is derived per-frame from the cursor: anchor
    // the cursor in the middle of the viewport (or as close as the
    // bounds permit). The state struct holds no scroll field —
    // renderer math is enough for the MVP, and recomputing every
    // frame matches whatever the cursor has done since last paint.
    let body_top = top + 2;
    let body_rows = pane_rows.saturating_sub(2);
    let scroll = if body_rows == 0 {
        0
    } else if tree.entries.len() <= body_rows {
        0
    } else {
        let half = body_rows / 2;
        let max_scroll = tree.entries.len().saturating_sub(body_rows);
        tree.cursor.saturating_sub(half).min(max_scroll)
    };

    // Canonicalise the focused buffer's path once so the per-row
    // "is this the open file" check is a straight comparison rather
    // than a syscall per entry. `canonicalize` resolves symlinks +
    // makes the path absolute; entries from `read_dir` are already
    // absolute relative to cwd, but a symlinked cwd would otherwise
    // produce a mismatched literal path.
    let active_canon = app
        .buffer
        .path
        .as_ref()
        .and_then(|p| std::fs::canonicalize(p).ok());

    for row in 0..body_rows {
        let entry_idx = scroll + row;
        let y = body_top + row as u16;
        queue!(out, MoveTo(0, y), SetBackgroundColor(pane_bg),)?;
        if entry_idx >= tree.entries.len() {
            queue!(out, Print(" ".repeat(content_cols)))?;
            continue;
        }
        let entry = &tree.entries[entry_idx];
        let is_cursor = entry_idx == tree.cursor;
        // Is this entry the file currently open in the focused editor
        // window? Canonicalise the entry too so a cwd reached through
        // a symlink still matches the buffer's canonical path.
        let is_active = !entry.is_dir
            && active_canon.is_some()
            && std::fs::canonicalize(&entry.path).ok() == active_canon;
        // Selected row sits just above the pane bg — `theme_surface` is
        // the "one layer above bg" helper (~12% shift toward white on
        // dark themes). Reads as a subtle highlight rather than the
        // saturated accent the picker uses.
        let row_bg = if is_cursor {
            app.config.theme_surface()
        } else {
            pane_bg
        };
        let name = entry
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        // Nerd Font glyphs: folder-closed / folder-open for dirs,
        // language-specific icon (via `icon_for_basename`) for files.
        let icon = if entry.is_dir {
            if tree.expanded.contains(&entry.path) {
                '\u{f07c}'
            } else {
                '\u{f07b}'
            }
        } else {
            icon_for_basename(name)
        };
        // Two cells of indent per depth level.
        let indent = "  ".repeat(entry.depth);
        let icon_fg = if entry.is_dir { dir_fg } else { file_fg };
        // Filename colour: the active open file shines in the accent
        // colour (and bold) so it stays visible even when the j/k
        // hover sits elsewhere. Non-active rows just use file_fg.
        let name_fg = if is_active { accent } else { file_fg };

        // Print indent + icon (icon keeps its file-type colour),
        // then a separator space, then the name (accent + bold when
        // active). Truncate at the pane edge.
        let prefix = format!(" {indent}{icon}  ");
        let prefix_chars: String = prefix.chars().take(content_cols).collect();
        let prefix_w = prefix_chars.chars().count();
        queue!(
            out,
            SetBackgroundColor(row_bg),
            SetForegroundColor(icon_fg),
            Print(&prefix_chars),
        )?;
        let name_budget = content_cols.saturating_sub(prefix_w);
        let name_chars: String = name.chars().take(name_budget).collect();
        let name_w = name_chars.chars().count();
        queue!(out, SetForegroundColor(name_fg))?;
        if is_active {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        queue!(out, Print(&name_chars))?;
        if is_active {
            queue!(out, SetAttribute(Attribute::NormalIntensity))?;
        }
        let written = prefix_w + name_w;
        if written < content_cols {
            queue!(
                out,
                SetBackgroundColor(row_bg),
                Print(" ".repeat(content_cols - written)),
            )?;
        }
    }
    queue!(out, ResetColor)?;
    Ok(())
}

fn draw_side_terminal_pane(out: &mut impl Write, app: &App) -> Result<()> {
    if !app.side_terminal_pane_open {
        return Ok(());
    }
    let pane_cols = app.side_pane_cols();
    if pane_cols == 0 {
        return Ok(());
    }
    let pane_rows = app.buffer_rows();
    if pane_rows == 0 {
        return Ok(());
    }
    let left = app.side_pane_left() as u16;
    let content_left = app.side_pane_content_left() as u16;
    let content_cols = app.side_pane_content_cols();
    let top = app.buffer_top() as u16;

    let pane_bg = app.config.chrome_bg();
    let muted = app.config.theme_dim();
    let base = app.config.terminal_chip_fg();
    let accent_terminal = app.config.terminal_chip_bg();
    let active_tab_bg = app.config.terminal_active_tab_bg();
    let border_fg = app.config.theme_border();

    let term_count = app.side_terminals.len();
    let active_label = app
        .side_terminals
        .get(app.active_side_terminal_idx)
        .map(|s| s.label.as_str())
        .unwrap_or("AI");
    let header_text = format!(" {} ", active_label.to_uppercase());
    let chip_bg = if matches!(app.mode, Mode::Terminal)
        && matches!(app.terminal_focus, crate::app::TerminalFocus::Side)
    {
        accent_terminal
    } else {
        muted
    };

    // Left border — a `│` glyph at column `left`, one per row of
    // pane height. Paint it first so subsequent header / body
    // writes layered on top can rely on the border already being
    // there for any cell they don't cover.
    for r in 0..pane_rows {
        queue!(
            out,
            MoveTo(left, top + r as u16),
            SetBackgroundColor(pane_bg),
            SetForegroundColor(border_fg),
            Print("│"),
        )?;
    }

    // Header row layout: tabs on the LEFT (starting at content_left,
    // one cell past the border), label chip RIGHT-aligned against
    // the pane's right edge, blank background in between. We draw
    // in three phases so the chip width is known before we pad.
    //
    //   [tab][tab]…<-- gap -->[CLAUDE]
    //
    // The chip doubles as a focus indicator — its background shifts
    // to the accent colour while the side pane is the active
    // `Mode::Terminal` target.
    let mut hitboxes: Vec<(usize, u16, u16)> = Vec::new();
    let chip_print = header_text.chars().take(content_cols).collect::<String>();
    let chip_w = chip_print.chars().count();
    // Budget the chip takes from the right edge; tabs get the rest.
    let tabs_budget = content_cols.saturating_sub(chip_w);

    // Phase 1: tabs from content_left. Each tab is ` N ` with a
    // single-cell gap; the leading gap balances the trailing one.
    queue!(out, MoveTo(content_left, top))?;
    let mut used: u16 = 0;
    if term_count > 1 {
        // Leading gap so the first tab doesn't butt into the border.
        if (used as usize) < tabs_budget {
            queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
            used += 1;
        }
        let mut tab_x = content_left + used;
        for idx in 0..term_count {
            let tab_label = format!(" {} ", idx + 1);
            let chip_chars = tab_label.chars().count() as u16;
            if (used + chip_chars) as usize > tabs_budget {
                break;
            }
            let is_active = idx == app.active_side_terminal_idx;
            let (bg, fg) = if is_active {
                (active_tab_bg, base)
            } else {
                (pane_bg, muted)
            };
            queue!(
                out,
                MoveTo(tab_x, top),
                SetBackgroundColor(bg),
                SetForegroundColor(fg),
            )?;
            if is_active {
                queue!(out, SetAttribute(Attribute::Bold))?;
            } else {
                queue!(out, SetAttribute(Attribute::NormalIntensity))?;
            }
            queue!(out, Print(&tab_label))?;
            queue!(out, SetAttribute(Attribute::NormalIntensity))?;
            hitboxes.push((idx, tab_x, tab_x + chip_chars));
            tab_x += chip_chars;
            used += chip_chars;
            if (used as usize) < tabs_budget {
                queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
                tab_x += 1;
                used += 1;
            }
        }
    }

    // Phase 2: pad the gap between tabs and the right-aligned chip.
    if (used as usize) < tabs_budget {
        let pad = tabs_budget - used as usize;
        queue!(out, SetBackgroundColor(pane_bg), Print(" ".repeat(pad)))?;
    }

    // Phase 3: right-aligned chip. MoveTo the exact column so we
    // don't accumulate width-arithmetic drift across the gap.
    let chip_left = content_left + (content_cols.saturating_sub(chip_w)) as u16;
    queue!(
        out,
        MoveTo(chip_left, top),
        SetBackgroundColor(chip_bg),
        SetForegroundColor(base),
        SetAttribute(Attribute::Bold),
        Print(&chip_print),
        SetAttribute(Attribute::Reset),
    )?;
    queue!(out, ResetColor)?;
    app.side_terminal_tab_hitboxes.set(hitboxes);

    // Body = pane minus the header row.
    let body_top = top + 1;
    let body_rows = pane_rows.saturating_sub(1);
    let Some(side) = app.side_terminals.get(app.active_side_terminal_idx) else {
        for r in 0..body_rows {
            queue!(
                out,
                MoveTo(content_left, body_top + r as u16),
                SetBackgroundColor(pane_bg),
                Print(" ".repeat(content_cols)),
            )?;
        }
        queue!(out, ResetColor)?;
        return Ok(());
    };
    // While the embedded tool is still settling, paint the binvim
    // loading splash instead of whatever transient frame the PTY
    // currently holds. claude / opencode / codex all paint a busy
    // splash + border decoration before their main UI lands, which
    // reads as broken when shown half-rendered.
    if crate::app::side_terminal_loading(side) {
        draw_side_loading_splash(
            out,
            app,
            content_left,
            body_top,
            content_cols,
            body_rows,
            &side.label,
        )?;
        return Ok(());
    }
    let term = &side.terminal;
    let inner = term.grid();
    let grid = &inner.handler.grid;
    let grid_rows = grid.rows.min(body_rows);
    let grid_cols = grid.cols.min(content_cols);
    // Mouse-drag selection — paint the covered cells with inverted
    // SGR so the user sees what they're grabbing before release
    // copies it to the clipboard. Gated on `tab_idx` so switching
    // tabs mid-drag doesn't leak the highlight into the wrong grid.
    let sel = app
        .side_terminal_selection
        .as_ref()
        .filter(|s| s.tab_idx == app.active_side_terminal_idx);
    for row in 0..body_rows {
        let screen_y = body_top + row as u16;
        queue!(out, MoveTo(content_left, screen_y))?;
        if row < grid_rows {
            let mut painted = 0usize;
            for col in 0..grid_cols {
                let mut cell = grid.cells[row][col];
                if cell.ch == '\0' {
                    continue;
                }
                if let Some(s) = sel {
                    if s.contains(row, col) {
                        cell.reverse = !cell.reverse;
                    }
                }
                paint_terminal_cell(out, cell)?;
                painted += 1;
            }
            // Backfill any uncovered columns so editor content from a
            // prior frame can't bleed through under a short PTY line.
            if painted < content_cols {
                queue!(
                    out,
                    SetBackgroundColor(pane_bg),
                    Print(" ".repeat(content_cols - painted)),
                )?;
            }
        } else {
            queue!(
                out,
                SetBackgroundColor(pane_bg),
                Print(" ".repeat(content_cols)),
            )?;
        }
    }
    queue!(out, ResetColor)?;
    Ok(())
}

/// Robot head shown over the side-pane loading splash. 3 rows ×
/// 9 cols — small enough that it always fits the side pane (min
/// ~28 cols), so no multi-variant fallback is needed. Reads as
/// "an assistant is booting up" without the editor identity claim
/// a "binvim" wordmark would carry in a surface that's about to
/// host Claude / Codex / opencode.
const SIDE_LOADING_LOGO: &[&str] = &["╔═══════╗", "║ ◉ ─ ◉ ║", "╚═══════╝"];

/// Braille spinner frames — 10 frames at ~80ms each rotates once
/// per second. Matches the conventional dotted-spinner pattern
/// every modern TUI loader uses (oh-my-zsh, npm, etc.).
const SIDE_LOADING_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Paint the binvim loading splash over the body of the right-side
/// terminal pane. Vertically centres a logo + spinner + caption;
/// the spinner frame is derived from the system clock so successive
/// renders show it advancing without per-frame state on the App.
fn draw_side_loading_splash(
    out: &mut impl Write,
    app: &App,
    left: u16,
    body_top: u16,
    pane_cols: usize,
    body_rows: usize,
    label: &str,
) -> Result<()> {
    let pane_bg = app.config.chrome_bg();
    let logo_fg = app.config.theme_info();
    let caption_fg = app.config.theme_fg();
    let muted_fg = app.config.theme_dim();
    // Blank the body first so leftover PTY content can't bleed
    // through under a logo line that doesn't reach the edge.
    let blank: String = " ".repeat(pane_cols);
    for r in 0..body_rows {
        queue!(
            out,
            MoveTo(left, body_top + r as u16),
            SetBackgroundColor(pane_bg),
            Print(&blank),
        )?;
    }
    // Show the head only when there's a 2-cell margin either side;
    // otherwise drop it and just show the spinner + caption.
    let logo: &[&str] = if pane_cols >= SIDE_LOADING_LOGO[0].chars().count() + 4 {
        SIDE_LOADING_LOGO
    } else {
        &[]
    };
    // Spinner frame indexed by wall-clock millis. 150ms per frame
    // (~6.7 fps) reads as a smooth rotation; 80ms looked like
    // blinking dots because the eye sees each braille glyph
    // discretely at high refresh rates.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let frame = SIDE_LOADING_SPINNER[(now / 150) as usize % SIDE_LOADING_SPINNER.len()];
    let caption = format!("{frame}  loading {label}…");
    let caption_w = caption.chars().count();
    let logo_h = logo.len();
    let caption_h = if caption_w + 2 <= pane_cols { 1 } else { 0 };
    let total_block_h = logo_h + if logo_h > 0 && caption_h > 0 { 1 } else { 0 } + caption_h;
    if total_block_h == 0 || total_block_h > body_rows {
        queue!(out, ResetColor)?;
        return Ok(());
    }
    let block_top = (body_rows.saturating_sub(total_block_h)) / 2;
    // Logo lines, centred horizontally.
    for (i, line) in logo.iter().enumerate() {
        let line_w = line.chars().count();
        if line_w > pane_cols {
            continue;
        }
        let inner_left = left + ((pane_cols - line_w) / 2) as u16;
        queue!(
            out,
            MoveTo(inner_left, body_top + (block_top + i) as u16),
            SetBackgroundColor(pane_bg),
            SetForegroundColor(logo_fg),
            SetAttribute(Attribute::Bold),
            Print(line),
            SetAttribute(Attribute::NormalIntensity),
        )?;
    }
    // Caption row, one blank line below the logo block.
    if caption_h > 0 {
        let caption_row = body_top + (block_top + logo_h + 1) as u16;
        let inner_left = left + ((pane_cols - caption_w) / 2) as u16;
        // Spinner segment in the accent colour, "loading …" in muted.
        queue!(
            out,
            MoveTo(inner_left, caption_row),
            SetBackgroundColor(pane_bg),
            SetForegroundColor(caption_fg),
            Print(frame),
            SetForegroundColor(muted_fg),
            Print(format!("  loading {label}…")),
        )?;
    }
    queue!(out, ResetColor)?;
    Ok(())
}

fn draw_messages_page(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    if rows == 0 || total_w < 30 {
        return Ok(());
    }
    let page_bg = app.config.background_color();
    let blank_row: String = " ".repeat(total_w);
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank_row))?;
        } else {
            queue!(out, Clear(ClearType::CurrentLine))?;
        }
    }
    let p = DashboardPalette::from_config(&app.config);
    let left = 2usize;
    let viewport_rows = rows.saturating_sub(1);
    let body_w = total_w.saturating_sub(left + 2).max(40);

    // Header row + one trailing blank.
    let mut lines: Vec<MessageRow> = Vec::new();
    lines.push(MessageRow::Header);
    lines.push(MessageRow::Blank);

    // Newest first — the user usually wants to read the latest server
    // crash, not a startup banner from ten minutes ago.
    for msg in app.lsp_messages.iter().rev() {
        let (sev_label, sev_colour) = match msg.severity {
            crate::lsp::MessageSeverity::Error => ("ERROR", p.red),
            crate::lsp::MessageSeverity::Warning => ("WARN ", p.yellow),
            crate::lsp::MessageSeverity::Info => ("INFO ", p.blue),
            crate::lsp::MessageSeverity::Log => ("LOG  ", p.overlay1),
        };
        let kind = if msg.is_show { "show" } else { "log " };
        let prefix = format!("[{sev_label}] {} ({kind}) ", msg.client_key);
        let prefix_w = prefix.chars().count();
        let body_max = body_w.saturating_sub(prefix_w).max(10);
        // Servers ship multi-line messages (Java stack traces especially);
        // each \n becomes its own wrapped continuation row, prefixed by
        // spaces so the message body lines up visually under the first.
        let mut first_line = true;
        let cont_indent = " ".repeat(prefix_w);
        for raw_line in msg.text.split('\n') {
            let trimmed = raw_line.trim_end_matches('\r');
            if trimmed.is_empty() && first_line {
                lines.push(MessageRow::Entry {
                    prefix: prefix.clone(),
                    prefix_colour: sev_colour,
                    body: String::new(),
                });
                first_line = false;
                continue;
            }
            for chunk in chunk_by_width(trimmed, body_max) {
                if first_line {
                    lines.push(MessageRow::Entry {
                        prefix: prefix.clone(),
                        prefix_colour: sev_colour,
                        body: chunk,
                    });
                    first_line = false;
                } else {
                    lines.push(MessageRow::Continuation {
                        indent: cont_indent.clone(),
                        body: chunk,
                    });
                }
            }
        }
        lines.push(MessageRow::Blank);
    }

    app.messages_content_height.set(lines.len());
    let scroll = app
        .messages_scroll
        .min(lines.len().saturating_sub(viewport_rows));

    for (i, row) in lines.iter().enumerate().skip(scroll).take(viewport_rows) {
        let screen_y = (top + (i - scroll)) as u16;
        match row {
            MessageRow::Header => {
                let title = format!(" {} captured server messages", app.lsp_messages.len());
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(p.lavender),
                    Print(truncate(&title, body_w)),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
            MessageRow::Blank => {}
            MessageRow::Entry {
                prefix,
                prefix_colour,
                body,
            } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(*prefix_colour),
                    Print(prefix),
                    SetForegroundColor(p.text),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(prefix.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
            MessageRow::Continuation { indent, body } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(p.overlay1),
                    Print(indent),
                    SetForegroundColor(p.subtext1),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(indent.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
        }
    }

    let has_more_below = scroll + viewport_rows < lines.len();
    let has_more_above = scroll > 0;
    let footer = match (has_more_above, has_more_below) {
        (false, false) => "Esc · q · :q to dismiss",
        (false, true) => "Esc · q · :q to dismiss · ↓ j more below",
        (true, false) => "Esc · q · :q to dismiss · ↑ k more above",
        (true, true) => "Esc · q · :q to dismiss · ↑ k ↓ j to scroll",
    };
    queue!(out, MoveTo(left as u16, (top + rows - 1) as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(footer, total_w.saturating_sub(left))),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

fn draw_registers_page(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    if rows == 0 || total_w < 30 {
        return Ok(());
    }
    let page_bg = app.config.background_color();
    let blank_row: String = " ".repeat(total_w);
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank_row))?;
        } else {
            queue!(out, Clear(ClearType::CurrentLine))?;
        }
    }
    let p = DashboardPalette::from_config(&app.config);
    let left = 2usize;
    let viewport_rows = rows.saturating_sub(1);
    let body_w = total_w.saturating_sub(left + 2).max(40);

    let mut lines: Vec<MessageRow> = Vec::new();

    // Yank registers — Vim's `:reg` order is `"`, `0`, `1`-`9`,
    // then named (`a`-`z`), then specials (`+`, `*`, `-`, `_`, `:`,
    // `.`, `/`, `=`, `#`). We surface whatever's actually populated.
    let mut yank_keys: Vec<char> = app.registers.keys().copied().collect();
    yank_keys.sort_by_key(|c| register_sort_key(*c));
    lines.push(MessageRow::Entry {
        prefix: format!(" Registers ({} populated)", yank_keys.len()),
        prefix_colour: p.lavender,
        body: String::new(),
    });
    lines.push(MessageRow::Blank);
    if yank_keys.is_empty() {
        lines.push(MessageRow::Continuation {
            indent: "  ".into(),
            body: "(no yank registers populated)".into(),
        });
    } else {
        for name in yank_keys {
            let r = app.registers.get(&name).unwrap();
            let preview = preview_register_text(&r.text, body_w.saturating_sub(8));
            let kind = if r.linewise { "L " } else { "  " };
            lines.push(MessageRow::Entry {
                prefix: format!("  \"{}  {}", name, kind),
                prefix_colour: p.blue,
                body: preview,
            });
        }
    }

    lines.push(MessageRow::Blank);

    // Macro registers — separate section so users can tell at a glance
    // that "g" holding 12 keys is a macro, not a yanked literal "g".
    let mut macro_keys: Vec<char> = app.macros.keys().copied().collect();
    macro_keys.sort();
    lines.push(MessageRow::Entry {
        prefix: format!(" Macros ({} recorded)", macro_keys.len()),
        prefix_colour: p.lavender,
        body: String::new(),
    });
    lines.push(MessageRow::Blank);
    if macro_keys.is_empty() {
        lines.push(MessageRow::Continuation {
            indent: "  ".into(),
            body: "(no macros — record with q<reg>…q)".into(),
        });
    } else {
        for name in macro_keys {
            let keys = app.macros.get(&name).unwrap();
            let preview = preview_macro_keys(keys, body_w.saturating_sub(10));
            lines.push(MessageRow::Entry {
                prefix: format!("  @{}  ({:>3}) ", name, keys.len()),
                prefix_colour: p.green,
                body: preview,
            });
        }
    }

    app.registers_content_height.set(lines.len());
    let scroll = app
        .registers_scroll
        .min(lines.len().saturating_sub(viewport_rows));

    for (i, row) in lines.iter().enumerate().skip(scroll).take(viewport_rows) {
        let screen_y = (top + (i - scroll)) as u16;
        match row {
            MessageRow::Header => {}
            MessageRow::Blank => {}
            MessageRow::Entry {
                prefix,
                prefix_colour,
                body,
            } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(*prefix_colour),
                    Print(prefix),
                    SetForegroundColor(p.text),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(prefix.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
            MessageRow::Continuation { indent, body } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(p.overlay1),
                    Print(indent),
                    SetForegroundColor(p.subtext1),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(indent.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
        }
    }

    let has_more_below = scroll + viewport_rows < lines.len();
    let has_more_above = scroll > 0;
    let footer = match (has_more_above, has_more_below) {
        (false, false) => "Esc · q · :q to dismiss",
        (false, true) => "Esc · q · :q to dismiss · ↓ j more below",
        (true, false) => "Esc · q · :q to dismiss · ↑ k more above",
        (true, true) => "Esc · q · :q to dismiss · ↑ k ↓ j to scroll",
    };
    queue!(out, MoveTo(left as u16, (top + rows - 1) as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(footer, total_w.saturating_sub(left))),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

/// Sort key matching Vim's `:reg` ordering — unnamed first, then yank
/// (`0`), then the numeric ring (`1`-`9`), named (`a`-`z`), then OS
/// clipboard mirrors and small specials.
fn register_sort_key(c: char) -> u32 {
    match c {
        '"' => 0,
        '0' => 1,
        '1'..='9' => 2 + (c as u32 - '1' as u32),
        'a'..='z' => 100 + (c as u32 - 'a' as u32),
        '+' => 200,
        '*' => 201,
        '-' => 202,
        _ => 300 + c as u32,
    }
}

/// Make a register's payload printable on a single line. Control chars
/// are shown as `^X`; newlines become a visible glyph; the string is
/// truncated to fit and tagged with a `…` when shortened.
fn preview_register_text(text: &str, max_chars: usize) -> String {
    let max = max_chars.max(8);
    let mut out = String::new();
    let mut count = 0usize;
    for c in text.chars() {
        let rendered: String = match c {
            '\n' => "↵".into(),
            '\t' => "→".into(),
            '\r' => "^M".into(),
            c if (c as u32) < 0x20 => format!("^{}", ((c as u8) + b'@') as char),
            c => c.to_string(),
        };
        let w = rendered.chars().count();
        if count + w > max {
            out.push('…');
            return out;
        }
        out.push_str(&rendered);
        count += w;
    }
    out
}

fn preview_macro_keys(keys: &[crossterm::event::KeyEvent], max_chars: usize) -> String {
    use crossterm::event::{KeyCode, KeyModifiers};
    let max = max_chars.max(8);
    let mut out = String::new();
    let mut count = 0usize;
    for k in keys {
        let rendered = match k.code {
            KeyCode::Char(c) => {
                let with_mods = format_key_with_mods(c, k.modifiers);
                with_mods
            }
            KeyCode::Enter => "<CR>".into(),
            KeyCode::Esc => "<Esc>".into(),
            KeyCode::Tab => "<Tab>".into(),
            KeyCode::BackTab => "<S-Tab>".into(),
            KeyCode::Backspace => "<BS>".into(),
            KeyCode::Delete => "<Del>".into(),
            KeyCode::Up => "<Up>".into(),
            KeyCode::Down => "<Down>".into(),
            KeyCode::Left => "<Left>".into(),
            KeyCode::Right => "<Right>".into(),
            KeyCode::Home => "<Home>".into(),
            KeyCode::End => "<End>".into(),
            KeyCode::PageUp => "<PgUp>".into(),
            KeyCode::PageDown => "<PgDn>".into(),
            KeyCode::Insert => "<Ins>".into(),
            KeyCode::F(n) => format!("<F{n}>"),
            _ => "?".into(),
        };
        let _ = KeyModifiers::NONE;
        let w = rendered.chars().count();
        if count + w > max {
            out.push('…');
            return out;
        }
        out.push_str(&rendered);
        count += w;
    }
    out
}

fn format_key_with_mods(c: char, mods: crossterm::event::KeyModifiers) -> String {
    use crossterm::event::KeyModifiers;
    if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) {
        let mut tag = String::new();
        tag.push('<');
        if mods.contains(KeyModifiers::CONTROL) {
            tag.push_str("C-");
        }
        if mods.contains(KeyModifiers::ALT) {
            tag.push_str("A-");
        }
        if mods.contains(KeyModifiers::SUPER) {
            tag.push_str("D-");
        }
        if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
            tag.push_str("S-");
        }
        tag.push(c);
        tag.push('>');
        tag
    } else {
        c.to_string()
    }
}

fn draw_test_results_page(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    let rows = app.buffer_rows();
    let top = app.buffer_top();
    if rows == 0 || total_w < 30 {
        return Ok(());
    }
    let page_bg = app.config.background_color();
    let blank_row: String = " ".repeat(total_w);
    for row in 0..rows {
        queue!(out, MoveTo(0, (row + top) as u16))?;
        if let Some(c) = page_bg {
            queue!(out, SetBackgroundColor(c), Print(&blank_row))?;
        } else {
            queue!(out, Clear(ClearType::CurrentLine))?;
        }
    }
    let p = DashboardPalette::from_config(&app.config);
    let left = 2usize;
    let viewport_rows = rows.saturating_sub(1);
    let body_w = total_w.saturating_sub(left + 2).max(40);

    let mut lines: Vec<MessageRow> = Vec::new();
    let header_text = if app.test.is_running() {
        " Test run (running…)".to_string()
    } else {
        let s = &app.test.summary;
        format!(
            " Test run — {} passed · {} failed · {} ignored",
            s.passed, s.failed, s.ignored
        )
    };
    lines.push(MessageRow::Entry {
        prefix: header_text,
        prefix_colour: p.lavender,
        body: String::new(),
    });
    lines.push(MessageRow::Blank);

    for row in &app.test.output_buffer {
        match row {
            crate::test::TestOutputRow::Header { command_line, .. } => {
                let prefix = "$ ".to_string();
                lines.push(MessageRow::Entry {
                    prefix,
                    prefix_colour: p.overlay1,
                    body: command_line.clone(),
                });
            }
            crate::test::TestOutputRow::Case {
                name,
                status,
                message,
            } => {
                let (label, colour) = match status {
                    crate::test::TestStatus::Passed => ("PASS ", p.green),
                    crate::test::TestStatus::Failed => ("FAIL ", p.red),
                    crate::test::TestStatus::Ignored => ("SKIP ", p.yellow),
                };
                let body = match message {
                    Some(m) if !m.is_empty() => format!("{name}  — {m}"),
                    _ => name.clone(),
                };
                let body_max = body_w.saturating_sub(label.chars().count()).max(10);
                let mut first = true;
                let cont_indent = " ".repeat(label.chars().count());
                for chunk in chunk_by_width(&body, body_max) {
                    if first {
                        lines.push(MessageRow::Entry {
                            prefix: label.to_string(),
                            prefix_colour: colour,
                            body: chunk,
                        });
                        first = false;
                    } else {
                        lines.push(MessageRow::Continuation {
                            indent: cont_indent.clone(),
                            body: chunk,
                        });
                    }
                }
            }
            crate::test::TestOutputRow::Output { stream, text } => {
                let prefix = match stream {
                    crate::test::OutputStream::Stdout => "  ",
                    crate::test::OutputStream::Stderr => "! ",
                };
                let prefix_colour = match stream {
                    crate::test::OutputStream::Stdout => p.overlay0,
                    crate::test::OutputStream::Stderr => p.red,
                };
                let body_max = body_w.saturating_sub(prefix.chars().count()).max(10);
                let mut first = true;
                let cont_indent = " ".repeat(prefix.chars().count());
                for chunk in chunk_by_width(text, body_max) {
                    if first {
                        lines.push(MessageRow::Entry {
                            prefix: prefix.to_string(),
                            prefix_colour,
                            body: chunk,
                        });
                        first = false;
                    } else {
                        lines.push(MessageRow::Continuation {
                            indent: cont_indent.clone(),
                            body: chunk,
                        });
                    }
                }
            }
            crate::test::TestOutputRow::Summary(s) => {
                lines.push(MessageRow::Blank);
                let body = format!(
                    "{} passed · {} failed · {} ignored{}",
                    s.passed,
                    s.failed,
                    s.ignored,
                    if s.filtered_out > 0 {
                        format!(" · {} filtered", s.filtered_out)
                    } else {
                        String::new()
                    },
                );
                let colour = if s.failed > 0 { p.red } else { p.green };
                lines.push(MessageRow::Entry {
                    prefix: " = ".to_string(),
                    prefix_colour: colour,
                    body,
                });
                lines.push(MessageRow::Blank);
            }
            crate::test::TestOutputRow::Aborted(msg) => {
                lines.push(MessageRow::Entry {
                    prefix: "ABORT ".to_string(),
                    prefix_colour: p.red,
                    body: msg.clone(),
                });
            }
        }
    }

    app.test_results_content_height.set(lines.len());
    // Tail-follow mode wins over `test_results_scroll` — pins the
    // viewport to the bottom every frame so streaming events stay
    // visible without the user having to press G between each tick.
    // Scrolling upward in `test_results_scroll_by` clears the flag.
    let max_scroll = lines.len().saturating_sub(viewport_rows);
    let scroll = if app.test_results_at_tail {
        max_scroll
    } else {
        app.test_results_scroll.min(max_scroll)
    };

    for (i, row) in lines.iter().enumerate().skip(scroll).take(viewport_rows) {
        let screen_y = (top + (i - scroll)) as u16;
        match row {
            MessageRow::Header => {}
            MessageRow::Blank => {}
            MessageRow::Entry {
                prefix,
                prefix_colour,
                body,
            } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(*prefix_colour),
                    Print(prefix),
                    SetForegroundColor(p.text),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(prefix.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
            MessageRow::Continuation { indent, body } => {
                queue!(out, MoveTo(left as u16, screen_y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(
                    out,
                    SetForegroundColor(p.overlay1),
                    Print(indent),
                    SetForegroundColor(p.subtext1),
                    Print(truncate(
                        body,
                        body_w.saturating_sub(indent.chars().count())
                    )),
                )?;
                reset_to_buf_bg(out, page_bg)?;
            }
        }
    }

    let has_more_below = scroll + viewport_rows < lines.len();
    let has_more_above = scroll > 0;
    let footer = match (has_more_above, has_more_below) {
        (false, false) => "Esc · q · :q to dismiss",
        (false, true) => "Esc · q · :q to dismiss · ↓ j more below",
        (true, false) => "Esc · q · :q to dismiss · ↑ k more above",
        (true, true) => "Esc · q · :q to dismiss · ↑ k ↓ j to scroll",
    };
    queue!(out, MoveTo(left as u16, (top + rows - 1) as u16))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(p.overlay0),
        Print(truncate(footer, total_w.saturating_sub(left))),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

enum MessageRow {
    Header,
    Blank,
    Entry {
        prefix: String,
        prefix_colour: Color,
        body: String,
    },
    Continuation {
        indent: String,
        body: String,
    },
}

/// Split `s` into chunks no wider than `width` display chars. Single
/// run of slicing — no word-wrap, just hard cuts. Empty input becomes
/// a single empty chunk so callers always emit at least one row.
fn chunk_by_width(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        current.push(ch);
        count += 1;
        if count >= width {
            out.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
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
        (
            left + left_w + gap,
            right_w,
            "RESOURCES",
            p.blue,
            &resource_lines,
        ),
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
            // LSP cache state — a 0 here means the server hasn't
            // replied yet (or doesn't speak that capability). Useful
            // for diagnosing "I see no highlights" / "tokens look
            // identical to tree-sitter": if doc-hi shows N > 0, the
            // server is responding and the renderer is the suspect;
            // if it's still 0 after several seconds of sitting on a
            // symbol, the server is the suspect.
            active_lines.push(SectionLine::plain(
                &format!(
                    "doc-hi: {} cached  ·  sem-tok: {} cached",
                    ab.doc_highlights, ab.semantic_tokens
                ),
                p.overlay1,
            ));
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
            let pending_colour = if h.pending_requests > 0 {
                p.peach
            } else {
                p.overlay1
            };
            // A server stuck in the init buffer is the "looks alive
            // but isn't" failure mode — surface it as a loud Red
            // marker on the main row so the user can't miss it.
            let status_chip = if !h.initialized {
                Some(("NOT INITIALIZED", p.red))
            } else {
                None
            };
            let mut parts = vec![
                (format!("• {:<18} ", h.key), p.text),
                (format!("{:<16} ", h.language_id), p.subtext1),
                (format!("{:<40} ", root), p.overlay1),
            ];
            if let Some((label, colour)) = status_chip {
                parts.push((format!("{label}  "), colour));
            }
            parts.push((format!("{} pending", h.pending_requests), pending_colour));
            lsp_lines.push(SectionLine::Custom { parts });
            // Indented follow-ups: stuck-init hint, then per-kind
            // pending breakdown. Either appears only when there's
            // something to say so the clean-state view doesn't grow.
            if !h.initialized && h.queued_init_frames > 0 {
                lsp_lines.push(SectionLine::Custom {
                    parts: vec![(
                        format!(
                            "    {} frames queued — server hasn't answered initialize. \
                             Likely a missing / wrapper binary; check `:messages` for stderr.",
                            h.queued_init_frames
                        ),
                        p.peach,
                    )],
                });
            }
            if !h.pending_breakdown.is_empty() {
                let detail = h
                    .pending_breakdown
                    .iter()
                    .map(|(kind, n)| format!("{n}× {kind}"))
                    .collect::<Vec<_>>()
                    .join("  ");
                lsp_lines.push(SectionLine::Custom {
                    parts: vec![(format!("    {detail}"), p.overlay1)],
                });
            }
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
            let mut parts: Vec<(String, Color)> = vec![
                (format!("{:>3}  ", i + 1), p.overlay0),
                (b.label.clone(), p.text),
            ];
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
            SectionLine::plain("(not detected — Tailwind LSP will not attach)", p.overlay1),
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
                (Some(b), true) => format!(
                    "{} (node_modules)",
                    home_relative_path(&b.display().to_string())
                ),
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
                if ec.trim_trailing {
                    p.green
                } else {
                    p.overlay1
                },
            ),
            ("   final newline ".into(), p.subtext1),
            (
                if ec.final_newline { "yes" } else { "no" }.into(),
                if ec.final_newline {
                    p.green
                } else {
                    p.overlay1
                },
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
            let label = if i == 0 {
                "sources            "
            } else {
                "                   "
            };
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
        (
            '—',
            p.overlay1,
            "saved (will restore on next launch with no path arg)",
        )
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
    Banner {
        x: usize,
        text: String,
        colour: Color,
    },
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
    BoxBottomPair {
        a: (usize, usize),
        b: (usize, usize),
    },
}

impl DashRow {
    fn paint<W: Write>(
        &self,
        out: &mut W,
        y: u16,
        palette: &DashboardPalette,
        page_bg: Option<Color>,
    ) -> Result<()> {
        match self {
            DashRow::Blank => Ok(()),
            DashRow::Banner { x, text, colour } => {
                queue!(out, MoveTo(*x as u16, y))?;
                apply_buf_bg(out, page_bg)?;
                queue!(out, SetForegroundColor(*colour), Print(text))?;
                reset_to_buf_bg(out, page_bg)?;
                Ok(())
            }
            DashRow::BoxTop {
                x,
                width,
                title,
                title_colour,
            } => paint_box_top(out, *x, y, *width, title, *title_colour, palette, page_bg),
            DashRow::BoxContent { x, width, line } => {
                paint_box_content(out, *x, y, *width, line, palette, page_bg)
            }
            DashRow::BoxBottom { x, width } => {
                paint_box_bottom(out, *x, y, *width, palette, page_bg)
            }
            DashRow::BoxTopPair { a, b } => {
                paint_box_top(out, a.0, y, a.1, &a.2, a.3, palette, page_bg)?;
                paint_box_top(out, b.0, y, b.1, &b.2, b.3, palette, page_bg)?;
                Ok(())
            }
            DashRow::BoxContentPair { a, b } => {
                if let Some((x, w, line)) = a {
                    paint_box_content(out, *x, y, *w, line, palette, page_bg)?;
                }
                if let Some((x, w, line)) = b {
                    paint_box_content(out, *x, y, *w, line, palette, page_bg)?;
                }
                Ok(())
            }
            DashRow::BoxBottomPair { a, b } => {
                paint_box_bottom(out, a.0, y, a.1, palette, page_bg)?;
                paint_box_bottom(out, b.0, y, b.1, palette, page_bg)?;
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
    page_bg: Option<Color>,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    let title_marked = format!(" {} ", title);
    let title_visible = title_marked.chars().count();
    let dashes = inner_w.saturating_sub(title_visible + 1);
    queue!(out, MoveTo(x as u16, y))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(palette.border),
        Print("┌─"),
        SetForegroundColor(title_colour),
        SetAttribute(crossterm::style::Attribute::Bold),
        Print(&title_marked),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(palette.border),
        Print("─".repeat(dashes)),
        Print('┐'),
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

fn paint_box_content<W: Write>(
    out: &mut W,
    x: usize,
    y: u16,
    width: usize,
    line: &SectionLine,
    palette: &DashboardPalette,
    page_bg: Option<Color>,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    let body_w = inner_w.saturating_sub(2); // 1-col padding each side
    queue!(out, MoveTo(x as u16, y))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
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
    )?;
    reset_to_buf_bg(out, page_bg)?;
    Ok(())
}

fn paint_box_bottom<W: Write>(
    out: &mut W,
    x: usize,
    y: u16,
    width: usize,
    palette: &DashboardPalette,
    page_bg: Option<Color>,
) -> Result<()> {
    let inner_w = width.saturating_sub(2);
    queue!(out, MoveTo(x as u16, y))?;
    apply_buf_bg(out, page_bg)?;
    queue!(
        out,
        SetForegroundColor(palette.border),
        Print('└'),
        Print("─".repeat(inner_w)),
        Print('┘'),
    )?;
    reset_to_buf_bg(out, page_bg)?;
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
    rows.push(DashRow::BoxBottomPair {
        a: (ax, aw),
        b: (bx, bw),
    });
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

impl DashboardPalette {
    fn from_config(config: &crate::config::Config) -> Self {
        let mauve = config.color_for_capture("keyword").unwrap_or(Color::Rgb {
            r: 0xcb,
            g: 0xa6,
            b: 0xf7,
        });
        let teal = config.color_for_capture("character").unwrap_or(Color::Rgb {
            r: 0x94,
            g: 0xe2,
            b: 0xd5,
        });
        Self {
            text: config.theme_fg(),
            subtext1: config.theme_fg(),
            overlay0: config.theme_dim(),
            overlay1: config.theme_dim(),
            border: config.theme_border(),
            lavender: config.theme_emphasis(),
            mauve,
            blue: config.theme_info(),
            teal,
            green: config.theme_accent_secondary(),
            yellow: config.theme_warning(),
            peach: config.theme_accent(),
            red: config.theme_error(),
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
    let segments: Vec<&str> = home_relative.split('/').filter(|s| !s.is_empty()).collect();
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

/// Replace a leading home-directory prefix with `~` so the dashboard's
/// long paths read cleanly. Resolution is best-effort — falls back to
/// the input unchanged when the home dir can't be resolved.
fn home_relative_path(path: &str) -> String {
    let Some(home) = crate::paths::home_dir() else {
        return path.to_string();
    };
    home_relative_with(path, &home.to_string_lossy())
}

/// Pure variant of `home_relative_path` — caller supplies the home
/// dir explicitly, so tests don't depend on the process environment.
/// Accepts both `/` and `\` separators so Windows paths display as
/// `~\foo\bar` while Unix paths stay `~/foo/bar`.
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
        if let Some(rest) = rest.strip_prefix('\\') {
            return format!("~\\{rest}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        DebugPalette, cursor_visual_col_walk, display_lsp_root, home_relative_with,
        tokenize_console_line, truncate,
    };

    #[test]
    fn cursor_visual_col_counts_tabs_as_tab_width() {
        // "\t\tfoo", cursor on the 'f' (col 2) → two tabs = 8 cells.
        let chars = "\t\tfoo".chars();
        assert_eq!(cursor_visual_col_walk(chars, 2, &[]), 8);
    }

    #[test]
    fn cursor_visual_col_adds_hints_before_cursor() {
        // "foo" with a 5-cell hint anchored at col 1 (after 'f'): cursor
        // at col 3 sits past the hint, so visual = 3 chars + 5 hint = 8.
        let hints = [0, 5, 0, 0];
        assert_eq!(cursor_visual_col_walk("foo".chars(), 3, &hints), 8);
    }

    #[test]
    fn cursor_visual_col_excludes_hint_at_cursor() {
        // A hint anchored exactly at the cursor col renders to the
        // cursor's right and must not shift it. "foo", cursor at col 1,
        // hint at col 1 → visual stays 1 (just the 'f').
        let hints = [0, 5, 0, 0];
        assert_eq!(cursor_visual_col_walk("foo".chars(), 1, &hints), 1);
    }

    #[test]
    fn cursor_visual_col_short_line_wide_hint_pushes_past_pane() {
        // Regression: a short buffer line ("x", 1 char) with a wide
        // leading hint must report a large visual col so the viewport
        // knows to scroll — otherwise the rendered cursor lands off-pane.
        let hints = [40, 0];
        assert_eq!(cursor_visual_col_walk("x".chars(), 1, &hints), 41);
    }

    #[test]
    fn tokenizer_splits_log_prefix_and_url() {
        let p = DebugPalette::default();
        let parts = tokenize_console_line(
            "[11:35:23 INF] Now listening on: http://localhost:15336",
            &p,
        );
        // Reconstructing the line from parts should round-trip
        // exactly (no chars dropped, no extras inserted).
        let joined: String = parts.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(
            joined,
            "[11:35:23 INF] Now listening on: http://localhost:15336"
        );
        // At least one part must carry the URL run as a single
        // chunk (so the colour applies to the whole URL).
        assert!(parts.iter().any(|q| q.text == "http://localhost:15336"));
        // And at least one part should be the INF level chunk.
        assert!(parts.iter().any(|q| q.text == "INF"));
    }

    #[test]
    fn tokenizer_recognises_pascal_case() {
        let p = DebugPalette::default();
        let parts = tokenize_console_line(
            "Starting a background hosted service for TempFileCleanupJob with delay",
            &p,
        );
        let joined: String = parts.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(
            joined,
            "Starting a background hosted service for TempFileCleanupJob with delay"
        );
        assert!(parts.iter().any(|q| q.text == "TempFileCleanupJob"));
    }

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
        assert_eq!(home_relative_with("/opt/cache", "/Users/bg"), "/opt/cache");
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
        let long = "file:///Users/bg/Development/bgunnarsson/comp/packages/ui-apps/src";
        let out = display_lsp_root(long, 25);
        assert!(
            out.starts_with("…/"),
            "expected leading ellipsis, got {out:?}"
        );
        assert!(
            out.ends_with("ui-apps/src"),
            "expected ui-apps/src tail, got {out:?}"
        );
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
        // Split-companion buffers (opened via <C-w>v + picker, never
        // visited as their own tab) stay out of the tabline — they're
        // already on screen in the split, and showing them as a tab
        // too would be redundant. `app.buffer_has_tab` codifies the
        // rule: active_tab OR has-a-stashed-layout.
        if !app.buffer_has_tab(i) {
            continue;
        }
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
    // Position of the active tab within the filtered `entries` list
    // (not the raw buffer index — those diverge once split-companion
    // buffers are filtered out).
    let active_entry_idx = entries
        .iter()
        .position(|(buf_idx, _, _)| *buf_idx == app.active_tab)
        .unwrap_or(0);
    let mut start_idx = 0usize;
    if total_widths > total_w {
        let mut used = 0usize;
        for i in (0..=active_entry_idx).rev() {
            used += widths[i];
            if used > total_w / 2 {
                start_idx = (i + 1).min(active_entry_idx);
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
            active: *idx == app.active_tab && !app.show_start_page,
        });
        col += w;
    }
    slots
}

fn draw_tab_bar(out: &mut impl Write, app: &App) -> Result<()> {
    let total_w = app.width as usize;
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine))?;

    let bar_bg = app.config.chrome_bg();
    let active_bg = app.config.tab_active_bg();
    let active_fg = app.config.tab_active_fg();
    let inactive_fg = app.config.tab_inactive_fg();
    let dirty_fg = app.config.tab_dirty();
    let close_fg = app.config.tab_close();
    let chevron_fg = app.config.theme_fg();

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

/// Paint the 1-cell gaps left between pane rects by `Layout::partition`.
/// Vertical gaps get a `│` glyph, horizontal gaps `─`; the colour is
/// Surface1 — same hue as the empty `~` placeholders, dim enough not to
/// fight with content but visible against the editor background.
fn draw_pane_dividers(
    out: &mut impl Write,
    app: &App,
    editor_rect: crate::layout::Rect,
) -> Result<()> {
    let panes = app.layout.partition(editor_rect);
    let surface1 = app.config.theme_surface();
    let buf_bg = app.config.background_color();
    queue!(out, SetForegroundColor(surface1))?;
    apply_buf_bg(out, buf_bg)?;
    // Vertical bars: any column directly between two horizontally
    // adjacent panes that share at least one row.
    for (i, (_, a)) in panes.iter().enumerate() {
        for (j, (_, b)) in panes.iter().enumerate() {
            if i == j {
                continue;
            }
            // `b` sits immediately to the right of `a` if the column
            // between them is unclaimed by any pane.
            if a.x + a.w + 1 == b.x {
                let lo = a.y.max(b.y);
                let hi = (a.y + a.h).min(b.y + b.h);
                if hi > lo {
                    let col = a.x + a.w;
                    for row in lo..hi {
                        queue!(out, MoveTo(col, row), Print("│"))?;
                    }
                }
            }
            // `b` sits immediately below `a`.
            if a.y + a.h + 1 == b.y {
                let lo = a.x.max(b.x);
                let hi = (a.x + a.w).min(b.x + b.w);
                if hi > lo {
                    let row = a.y + a.h;
                    queue!(out, MoveTo(lo, row))?;
                    for _ in lo..hi {
                        queue!(out, Print("─"))?;
                    }
                }
            }
        }
    }
    queue!(out, ResetColor)?;
    Ok(())
}

fn draw_buffer(
    out: &mut impl Write,
    app: &App,
    bs: &crate::app::state::BufferState<'_>,
    win: &crate::window::Window,
    rect: crate::layout::Rect,
    is_active: bool,
) -> Result<()> {
    let rows = rect.h as usize;
    let top = rect.y as usize;
    let left = rect.x as usize;
    let pane_w = rect.w as usize;
    let gutter = bs.gutter_width();
    let avail = pane_w.saturating_sub(gutter);
    let total_lines = bs.buffer.line_count();
    let mut line_idx = win.view_top;
    // Skip any lines that are hidden — by a closed fold or by the
    // markdown concealed-render pass (HTML chrome, setext
    // underlines) — from the start of the viewport so the first
    // visible row isn't on a row that has no visible body.
    while line_idx < total_lines && (bs.line_is_folded(line_idx) || bs.line_is_md_hidden(line_idx))
    {
        line_idx += 1;
    }
    // Canonicalise this pane's buffer path once for the duration of this
    // draw so breakpoint + stopped-frame gutter lookups don't do one
    // syscall per visible row. Fall back to the raw path if canonicalisation
    // fails (e.g. unsaved or removed file).
    let canon_buf_path: Option<std::path::PathBuf> = bs
        .buffer
        .path
        .as_ref()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
    // 1-based line number of the currently-stopped top frame, if the
    // session is paused inside this buffer.
    let pc_line: Option<usize> = match (&canon_buf_path, app.dap.session.as_ref()) {
        (Some(bp), Some(session))
            if matches!(session.state, crate::dap::SessionState::Stopped { .. }) =>
        {
            session.frames.first().and_then(|f| {
                let fs = f.source.as_ref()?;
                let fs_canon = fs.canonicalize().unwrap_or_else(|_| fs.clone());
                if &fs_canon == bp { Some(f.line) } else { None }
            })
        }
        _ => None,
    };
    // Pre-built blanker the same width as this pane — used in place of
    // `Clear(ClearType::CurrentLine)` so we don't nuke any pane sitting
    // to the left or right of us on the same terminal row.
    let pane_blank: String = " ".repeat(pane_w);
    let buf_bg = app.config.background_color();
    // A lens anchored to the topmost visible line carries a phantom
    // row *above* the line. We paint it first, on the very next loop
    // iteration the real line paints below.
    let mut pending_lens = line_idx < total_lines && bs.line_has_code_lens(line_idx);
    // Multi-line Copilot ghost. The first line of the visible tail is
    // painted inline at the cursor by draw_line_with_selection; lines
    // 2+ are queued here and emitted as phantom rows directly below
    // the anchor line — same row-injection trick code lens uses, so
    // the real buffer lines below get pushed down by the ghost height.
    // `ghost_overflow` is moved into `pending_ghost` once the anchor
    // line paints, then drained one row per loop iteration.
    let mut ghost_overflow: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let ghost_anchor_line: Option<usize> = if is_active {
        app.copilot_ghost.as_ref().and_then(|ghost| {
            // Same gate as the inline first-line render below.
            let on = ghost.col == app.window.cursor.col
                && Some(ghost.path.as_path()) == bs.buffer.path.as_deref()
                && matches!(app.mode, Mode::Insert);
            if !on {
                return None;
            }
            for tail_line in app.copilot_ghost_visible_tail(ghost).split('\n').skip(1) {
                ghost_overflow.push_back(tail_line.to_string());
            }
            Some(ghost.line)
        })
    } else {
        None
    };
    let mut pending_ghost: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for row in 0..rows {
        // Wipe this pane's row (leaves adjacent panes untouched), then
        // return the cursor to the pane's left edge so the per-line draw
        // below starts in the right column. The pane_blank fills the row
        // with the theme background (or terminal default if unset).
        queue!(out, MoveTo(left as u16, (row + top) as u16))?;
        apply_buf_bg(out, buf_bg)?;
        queue!(
            out,
            Print(&pane_blank),
            MoveTo(left as u16, (row + top) as u16),
        )?;
        // Ghost overflow rows sit immediately under the anchor line, so
        // they drain before the next real line's phantom lens row.
        if let Some(text) = pending_ghost.pop_front() {
            queue!(out, Print(" ".repeat(gutter)))?;
            let truncated: String = text.chars().take(avail).collect();
            if !truncated.is_empty() {
                queue!(
                    out,
                    SetForegroundColor(app.config.theme_dim()),
                    SetAttribute(Attribute::Italic),
                    Print(&truncated),
                    SetAttribute(Attribute::NoItalic),
                )?;
                reset_to_buf_bg(out, buf_bg)?;
            }
            continue;
        }
        if pending_lens && line_idx < total_lines {
            paint_code_lens_row(out, app, line_idx, gutter, avail, buf_bg)?;
            pending_lens = false;
            continue;
        }
        if line_idx < total_lines {
            // Captured before the fold-advance below mutates line_idx —
            // used to detect when we've just painted the ghost anchor.
            let drawn_line = line_idx;
            // Git stripe — leftmost gutter column. Mirrors gitsigns /
            // GitGutter conventions: a coloured vertical block for
            // added (Green) / modified (Yellow) / a horizontal block
            // for deleted (Red). Empty when the line is unchanged or
            // the buffer isn't tracked by git.
            let git_kind = bs.git_hunk_kind_at(line_idx);
            if let Some(kind) = git_kind {
                let (glyph, color) = match kind {
                    crate::git::GitHunkKind::Added => ('▎', app.config.git_added()),
                    crate::git::GitHunkKind::Modified => ('▎', app.config.git_modified()),
                    crate::git::GitHunkKind::Deleted => ('▁', app.config.git_deleted()),
                };
                queue!(out, SetForegroundColor(color), Print(glyph.to_string()))?;
                reset_to_buf_bg(out, buf_bg)?;
            } else {
                queue!(out, Print(" "))?;
            }
            // Sign column priority: stopped-at marker > user breakpoint >
            // worst LSP diagnostic. The debug marks are user-actionable
            // ground truth and should win when they collide.
            let line_one_based = line_idx + 1;
            let pc_here = pc_line == Some(line_one_based);
            // Look up the per-site breakpoint so a conditional /
            // hit-count one renders with a different glyph (`◆`) than
            // a plain pause (`●`). The shape is enough at glance —
            // the actual expression shows up in the breakpoints pane.
            let bp_here = canon_buf_path
                .as_deref()
                .and_then(|p| app.dap.breakpoint_at(p, line_one_based));
            let sign = if pc_here {
                Some(('▶', app.config.gutter_pc_marker()))
            } else if let Some(bp) = &bp_here {
                let glyph = if bp.is_conditional() { '◆' } else { '●' };
                Some((glyph, app.config.gutter_breakpoint()))
            } else if let Some(diag_path) = bs.buffer.path.as_deref() {
                app.worst_diagnostic_for(diag_path, line_idx)
                    .map(|s| match s {
                        Severity::Error => ('!', app.config.diagnostic_error()),
                        Severity::Warning => ('?', app.config.diagnostic_warning()),
                        Severity::Info => ('i', app.config.diagnostic_info()),
                        Severity::Hint => ('h', app.config.diagnostic_hint()),
                    })
            } else {
                None
            };
            if let Some((ch, color)) = sign {
                queue!(out, SetForegroundColor(color), Print(ch.to_string()))?;
                reset_to_buf_bg(out, buf_bg)?;
            } else {
                queue!(out, Print(" "))?;
            }
            // Relative numbers (Vim convention): every row except the
            // cursor's shows its distance from the cursor; the cursor's
            // own row shows the absolute (1-indexed) line. Useful with
            // count-prefixed motions like `5j` / `12k` / `3dd`. The
            // cursor row gets a brighter Subtext1 tone so the eye can
            // anchor on it; other rows stay the muted Overlay0.
            let (label, label_color) =
                if app.config.line_numbers.relative && line_idx != win.cursor.line {
                    let dist = if line_idx > win.cursor.line {
                        line_idx - win.cursor.line
                    } else {
                        win.cursor.line - line_idx
                    };
                    (
                        format!("{:>width$} ", dist, width = gutter - 3),
                        app.config.theme_dim(),
                    )
                } else {
                    // Cursor row in relative mode, or every row in absolute
                    // mode → 1-indexed absolute line number.
                    let bright = app.config.line_numbers.relative;
                    (
                        format!("{:>width$} ", line_idx + 1, width = gutter - 3),
                        if bright {
                            app.config.theme_fg()
                        } else {
                            app.config.theme_dim()
                        },
                    )
                };
            queue!(out, SetForegroundColor(label_color), Print(label))?;
            reset_to_buf_bg(out, buf_bg)?;
            draw_line_with_selection(out, app, bs, win, line_idx, avail, is_active, buf_bg)?;
            // Fold-start placeholder: append `… N lines` after the line's
            // own content so the user sees what's collapsed.
            if bs.line_is_fold_start(line_idx) {
                let span = bs.folded_line_span(line_idx);
                let folded = format!("  ⏷ {} lines", span);
                queue!(
                    out,
                    SetForegroundColor(app.config.theme_dim()),
                    Print(folded)
                )?;
                reset_to_buf_bg(out, buf_bg)?;
            }
            // Advance to the next visible line — past the fold's hidden
            // body if this row was a fold start, otherwise just by one.
            // Then keep skipping any consecutive folded / md-hidden
            // rows so the next iteration lands on a paintable row.
            let span = bs.folded_line_span(line_idx);
            line_idx += span.max(1);
            while line_idx < total_lines
                && (bs.line_is_folded(line_idx) || bs.line_is_md_hidden(line_idx))
            {
                line_idx += 1;
            }
            // Tee up a phantom lens row for the next iteration if the
            // line we just advanced to has any lenses anchored on it.
            pending_lens = line_idx < total_lines && bs.line_has_code_lens(line_idx);
            // If the line we just drew was the ghost's anchor, queue its
            // overflow lines so the next iterations paint them below.
            if Some(drawn_line) == ghost_anchor_line {
                pending_ghost = std::mem::take(&mut ghost_overflow);
            }
        } else {
            queue!(
                out,
                SetForegroundColor(app.config.theme_surface()),
                Print("~")
            )?;
            reset_to_buf_bg(out, buf_bg)?;
        }
    }
    Ok(())
}

fn draw_line_with_selection(
    out: &mut impl Write,
    app: &App,
    bs: &crate::app::state::BufferState<'_>,
    win: &crate::window::Window,
    line_idx: usize,
    avail: usize,
    is_active: bool,
    buf_bg: Option<Color>,
) -> Result<()> {
    let slice = bs.buffer.rope.line(line_idx);
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
    // Per-window UI affordances (selection, match-pair, multi-cursor,
    // yank flash) are tied to the active cursor — Vim shows them only
    // in the focused pane. Inactive panes still paint the buffer + git
    // stripe + diagnostics, just without the active-cursor decoration.
    let sel = if is_active {
        app.line_selection(line_idx)
    } else {
        None
    };
    let extra_sels = if is_active {
        app.line_extra_selections(line_idx)
    } else {
        Vec::new()
    };
    let search_matches = app.line_search_matches_in(bs.buffer, line_idx);
    let doc_highlights: Vec<(usize, usize)> = if let Some(path) = bs.buffer.path.as_deref() {
        app.line_document_highlights(path, line_idx)
    } else {
        Vec::new()
    };
    let yank_flash = if is_active {
        app.line_yank_highlight(line_idx)
    } else {
        None
    };
    let match_pair = if is_active {
        app.line_match_pair(line_idx)
    } else {
        Vec::new()
    };
    let line_byte_start = bs.buffer.rope.line_to_byte(line_idx);
    let chars: Vec<char> = text.chars().collect();
    let view_left = win.view_left;
    // Visual column from the start of the line — tracks where each char
    // would land if `view_left == 0`. Subtract `view_left` to get the
    // on-screen column.
    let mut line_visual_pos = 0usize;
    // Visual columns actually written to the terminal in this pass.
    let mut visual_used = 0usize;
    let mut byte_off = line_byte_start;
    let dim = app.has_modal_overlay();
    let hint_fg = app.config.theme_dim();
    // Multi-cursor positions on this line — the renderer paints a
    // Lavender block at each so the user can see where mirrored edits
    // will land.
    let multi_cursors: Vec<usize> = if is_active {
        app.line_multi_cursor_cols(line_idx)
    } else {
        Vec::new()
    };

    // Pre-bin inlay hints by column so we can render them inline at the
    // start of each char iteration (and once more after the last char,
    // for hints anchored at end-of-line). Hints are keyed on `App` by
    // path, so an inactive pane gets its own buffer's hints by looking
    // them up with `bs.buffer.path`.
    let mut hint_at: Vec<Vec<&crate::lsp::InlayHint>> = vec![Vec::new(); chars.len() + 1];
    if !dim {
        if let Some(path) = bs.buffer.path.as_ref() {
            if let Some(hints) = app.inlay_hints.get(path) {
                for h in hints {
                    if h.line == line_idx && h.col <= chars.len() {
                        hint_at[h.col].push(h);
                    }
                }
            }
        }
    }
    let dim_color = app.config.theme_dim();
    // `:set list` equivalent — render every space as `·`, every tab as
    // `→` + filler, every non-breaking space as `⎵`, and the end-of-line
    // as `¬`. All in the muted overlay colour. Configurable via
    // `[whitespace]` in config.toml; on by default.
    let show_hidden = app.config.whitespace.show;
    // Precompute per-column severity from the LSP's diagnostic ranges so we
    // can paint an undercurl directly under the offending tokens. Routed
    // by this pane's buffer path so inactive panes paint their own
    // diagnostics, not the active buffer's.
    let line_diags = if let Some(path) = bs.buffer.path.as_deref() {
        app.line_diagnostics_for(path, line_idx)
    } else {
        Vec::new()
    };
    // Pre-compute per-column semantic-token colours, if a cached LSP
    // response is available for this buffer. Tokens are anchored to a
    // specific buffer version; we drop the cache as stale when the
    // version no longer matches so a freshly-typed line doesn't get
    // mis-coloured against indices computed for an older revision.
    let mut sem_col_color: Vec<Option<Color>> = vec![None; chars.len()];
    if !dim {
        if let Some(path) = bs.buffer.path.as_ref() {
            if let Some(cache) = app.semantic_tokens.get(path) {
                if cache.buffer_version == bs.buffer.version {
                    if let Some(row_tokens) = cache.by_line.get(line_idx) {
                        for tok in row_tokens {
                            // Modifier list becomes dotted suffix so
                            // `function.async` and `variable.readonly`
                            // both flow through the same dotted-prefix
                            // resolver the tree-sitter pass uses.
                            let name = if tok.modifiers.is_empty() {
                                tok.token_type.clone()
                            } else {
                                let mut s = tok.token_type.clone();
                                for m in &tok.modifiers {
                                    s.push('.');
                                    s.push_str(m);
                                }
                                s
                            };
                            let Some(color) = app.config.color_for_capture(&name) else { continue };
                            let start = tok.start_col.min(chars.len());
                            let end = (tok.start_col + tok.length).min(chars.len());
                            for slot in &mut sem_col_color[start..end] {
                                *slot = Some(color);
                            }
                        }
                    }
                }
            }
        }
    }
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
        let span = if end > start {
            end
        } else {
            (start + 1).min(chars.len())
        };
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
    let md_meta: Option<&crate::markdown_render::MarkdownLineMeta> = if bs.markdown_render_active {
        bs.markdown_line_meta(line_idx)
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
                // Paint `─` × `avail` in muted dim so the rule
                // visually separates sections without competing with
                // surrounding prose.
                let rule: String = "─".repeat(avail);
                queue!(out, SetForegroundColor(app.config.theme_dim()), Print(rule))?;
                reset_to_buf_bg(out, buf_bg)?;
                return Ok(());
            }
            crate::markdown_render::MarkdownLineKind::Table(row_kind) => {
                // Tables paint a pre-rendered box-drawn replacement
                // string (column-padded, pipes/junctions) instead of
                // the source row. Style by row kind: header emphasis +
                // bold, separator dim, body normal text (so cell content
                // reads as foreground prose).
                if let Some(rendered) = meta.replacement.as_deref() {
                    let (color, bold) = match row_kind {
                        crate::markdown_render::TableRowKind::Header => {
                            (app.config.theme_emphasis(), true)
                        }
                        crate::markdown_render::TableRowKind::Separator => {
                            (app.config.theme_dim(), false)
                        }
                        crate::markdown_render::TableRowKind::Body => {
                            (app.config.theme_fg(), false)
                        }
                    };
                    let printable: String = rendered.chars().take(avail).collect();
                    queue!(out, SetForegroundColor(color))?;
                    if bold {
                        queue!(out, SetAttribute(Attribute::Bold))?;
                    }
                    queue!(out, Print(&printable))?;
                    queue!(out, SetAttribute(Attribute::Reset))?;
                    reset_to_buf_bg(out, buf_bg)?;
                }
                return Ok(());
            }
            crate::markdown_render::MarkdownLineKind::HtmlSummary => {
                // <summary>X</summary> — paint as a bold-Peach
                // disclosure title with `▼ ` prefix already baked
                // into `replacement`. Same return-after-paint shape
                // as the Table arm.
                if let Some(rendered) = meta.replacement.as_deref() {
                    let printable: String = rendered.chars().take(avail).collect();
                    queue!(
                        out,
                        SetForegroundColor(app.config.theme_accent()),
                        SetAttribute(Attribute::Bold),
                        Print(&printable),
                        SetAttribute(Attribute::Reset),
                    )?;
                    reset_to_buf_bg(out, buf_bg)?;
                }
                return Ok(());
            }
            crate::markdown_render::MarkdownLineKind::Default
            | crate::markdown_render::MarkdownLineKind::CodeBlock => {}
        }
    }
    // Code-fence rows (opener + body + closer) all share the Mantle
    // background so the block reads as a single dark slab. The bg is
    // applied per-char inside the loop and as a trailing fill after
    // the chars run out.
    let code_block_bg: Option<Color> = md_meta.and_then(|m| {
        if m.kind == crate::markdown_render::MarkdownLineKind::CodeBlock {
            Some(Color::Rgb {
                r: 0x18,
                g: 0x18,
                b: 0x25,
            }) // Mantle
        } else {
            None
        }
    });
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
                                let visible_right = (line_visual_pos + w).min(view_left + avail);
                                if visible_right > visible_left {
                                    let visible = visible_right - visible_left;
                                    if visual_used + visible > avail {
                                        clipped_right = true;
                                        break;
                                    }
                                    let skip = visible_left - line_visual_pos;
                                    let printable: String =
                                        glyph.chars().skip(skip).take(visible).collect();
                                    queue!(out, SetForegroundColor(*color), Print(printable),)?;
                                    reset_to_buf_bg(out, buf_bg)?;
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
                let fg = hint_fg;
                queue!(
                    out,
                    SetForegroundColor(fg),
                    SetAttribute(Attribute::Italic),
                    Print(&printable),
                    SetAttribute(Attribute::Reset),
                )?;
                reset_to_buf_bg(out, buf_bg)?;
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
            && yank_flash
                .map(|(s, e)| col >= s && col < e)
                .unwrap_or(false);
        let in_match_pair = !in_sel
            && !in_search
            && !in_yank_flash
            && match_pair.iter().any(|(s, e)| col >= *s && col < *e);
        let in_doc_highlight = !in_sel
            && !in_search
            && !in_yank_flash
            && !in_match_pair
            && doc_highlights.iter().any(|(s, e)| col >= *s && col < *e);
        // Multi-cursor marker — paint the cell the cursor is sitting on
        // (i.e. the char to its right) in Lavender so the user can see
        // where their other cursors are.
        let is_multi_cursor = multi_cursors.contains(&col);
        // LSP semantic tokens win over the tree-sitter highlight cache
        // when both have an opinion — the server's view of the symbol
        // (mutable / immutable / async / parameter / etc.) is strictly
        // richer than any static query. Falls back to tree-sitter when
        // the LSP didn't tag this column.
        let syntax_color = sem_col_color.get(col).copied().flatten().or_else(|| {
            bs.highlight_cache
                .and_then(|cache| cache.byte_colors.get(byte_off).copied())
                .flatten()
        });
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
                SetBackgroundColor(app.config.search_highlight_bg()),
                SetForegroundColor(app.config.theme_chip_fg())
            )?;
        } else if in_yank_flash {
            // Distinct flash — different from search so the two never
            // visually collide on shared text.
            queue!(
                out,
                SetBackgroundColor(app.config.yank_flash_bg()),
                SetForegroundColor(app.config.theme_chip_fg())
            )?;
        } else if is_multi_cursor {
            // High-contrast multi-cursor block — matches the primary
            // cursor's own colour for visual continuity across mirrored
            // edit sites.
            queue!(
                out,
                SetBackgroundColor(app.config.multi_cursor_bg()),
                SetForegroundColor(app.config.theme_chip_fg())
            )?;
        } else if in_match_pair {
            // Subtle match-pair background so the syntax-coloured fg
            // still shows through, plus Bold so the bracket/tag pops.
            queue!(
                out,
                SetBackgroundColor(app.config.match_pair_bg()),
                SetAttribute(Attribute::Bold)
            )?;
        } else if in_doc_highlight {
            // Subtle bg under the symbol-under-cursor occurrences.
            // Foreground stays on the syntax cache so the underlying
            // token colour still reads through.
            queue!(out, SetBackgroundColor(app.config.doc_highlight_bg()))?;
            if let Some(fg) = syntax_color {
                queue!(out, SetForegroundColor(fg))?;
            }
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
        // Code-fence rows want a Mantle background across the full
        // line width. Apply it whenever no other branch already set
        // a per-char bg (selection / search / yank / multi-cursor /
        // match-pair). The reset at end-of-cell would otherwise wipe
        // it; that's OK because we re-apply it per char.
        if let Some(bg) = code_block_bg {
            let bg_already_set = in_sel
                || in_search
                || in_yank_flash
                || is_multi_cursor
                || in_match_pair
                || in_doc_highlight;
            if !bg_already_set {
                queue!(out, SetBackgroundColor(bg))?;
            }
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
            let underline = severity_color(app, sev);
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
            apply_buf_bg(out, buf_bg)?;
        } else if in_match_pair {
            // Tear down the bold + bg in one shot.
            queue!(out, SetAttribute(Attribute::Reset))?;
            reset_to_buf_bg(out, buf_bg)?;
        } else if is_multi_cursor {
            reset_to_buf_bg(out, buf_bg)?;
        } else if md_attrs_set {
            // Bold / italic / underline don't unset themselves on the
            // next char — clear all SGR so the styling stops at the
            // span boundary.
            queue!(out, SetAttribute(Attribute::Reset))?;
            reset_to_buf_bg(out, buf_bg)?;
        } else if in_search
            || in_yank_flash
            || syntax_color.is_some()
            || dim
            || render_hidden
            || md_style.and_then(|s| s.color).is_some()
        {
            reset_to_buf_bg(out, buf_bg)?;
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
            SetBackgroundColor(app.config.multi_cursor_bg()),
            SetForegroundColor(app.config.theme_chip_fg()),
            Print(' '),
        )?;
        reset_to_buf_bg(out, buf_bg)?;
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
            let fg = hint_fg;
            queue!(
                out,
                SetForegroundColor(fg),
                SetAttribute(Attribute::Italic),
                Print(&printable),
                SetAttribute(Attribute::Reset),
            )?;
            reset_to_buf_bg(out, buf_bg)?;
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
    // Suppressed inside code-fence rows so the dark slab isn't broken by
    // a chrome glyph rendered with the terminal-default bg.
    if show_hidden && !clipped_right && visual_used + 1 <= avail && code_block_bg.is_none() {
        queue!(out, SetForegroundColor(dim_color), Print('¬'))?;
        reset_to_buf_bg(out, buf_bg)?;
    }

    // Copilot ghost suggestion — when the cursor is on this line in
    // Insert mode and we have a live ghost, render the first line of
    // its visible tail (the divergent portion after whatever the user
    // already typed) as muted Overlay0 italic after the line's real
    // content. Lines 2+ of a multi-line suggestion are painted by the
    // phantom-row loop in draw_buffer, directly below this line.
    // Skipped for inactive panes (the ghost belongs to the focused
    // cursor, not every viewport).
    if is_active {
        if let Some(ghost) = app.copilot_ghost.as_ref() {
            if ghost.line == line_idx
                && ghost.col == app.window.cursor.col
                && Some(ghost.path.as_path()) == bs.buffer.path.as_deref()
                && matches!(app.mode, Mode::Insert)
            {
                let tail = app.copilot_ghost_visible_tail(ghost);
                let first_line = tail.split('\n').next().unwrap_or("");
                // Truncate so the ghost can't wrap past the pane edge.
                let remaining = avail.saturating_sub(visual_used);
                if remaining > 0 && !first_line.is_empty() {
                    let truncated: String = first_line.chars().take(remaining).collect();
                    queue!(
                        out,
                        SetForegroundColor(app.config.theme_dim()),
                        SetAttribute(Attribute::Italic),
                        Print(&truncated),
                        SetAttribute(Attribute::NoItalic),
                    )?;
                    reset_to_buf_bg(out, buf_bg)?;
                }
            }
        }
    }

    // Error Lens-style inline diagnostic at the end of the line. We carefully
    // measure the prefix and message in display columns (not chars), and leave
    // one column of slack so any width-miscount can't push past the row edge
    // and force the terminal to wrap onto the next row — which would clobber
    // the next line's render with the diagnostic's tail.
    let diags = if let Some(path) = bs.buffer.path.as_deref() {
        app.line_diagnostics_for(path, line_idx)
    } else {
        Vec::new()
    };
    let has_diag = !diags.is_empty();
    if !dim {
        if let Some(diag) = diags.first() {
            use unicode_width::UnicodeWidthChar;
            let remaining = avail.saturating_sub(visual_used);
            let icon = match diag.severity {
                Severity::Warning => '▲',
                _ => '●',
            };
            let color = severity_color(app, diag.severity);
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
                )?;
                reset_to_buf_bg(out, buf_bg)?;
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
                        SetForegroundColor(app.config.theme_dim()),
                        SetAttribute(Attribute::Italic),
                        Print(&msg),
                        SetAttribute(Attribute::NoItalic),
                    )?;
                    reset_to_buf_bg(out, buf_bg)?;
                }
            }
        }
    }
    // Code-fence trailing fill — extend the Mantle background to the
    // right edge so the slab spans the full buffer width regardless
    // of how short the actual line content is. Done after EOL
    // decorations (¬ marker, diagnostics) so those still render.
    if let Some(bg) = code_block_bg {
        let trailing = avail.saturating_sub(visual_used);
        if trailing > 0 {
            queue!(out, SetBackgroundColor(bg), Print(" ".repeat(trailing)))?;
            reset_to_buf_bg(out, buf_bg)?;
        }
    }
    Ok(())
}

/// Catppuccin Mocha colour assignment per LSP severity. Used for both the
/// undercurl on the offending range and the inline Error Lens icon so the
/// two visuals match on the same line.
fn severity_color(app: &App, sev: Severity) -> Color {
    match sev {
        Severity::Error => app.config.diagnostic_error(),
        Severity::Warning => app.config.diagnostic_warning(),
        Severity::Info => app.config.diagnostic_info(),
        Severity::Hint => app.config.diagnostic_hint(),
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
fn mode_color(app: &App, mode: Mode) -> Color {
    match mode {
        Mode::Normal => app.config.mode_normal(),
        Mode::Insert => app.config.mode_insert(),
        Mode::Visual(_) => app.config.mode_visual(),
        Mode::Command => app.config.mode_command(),
        Mode::Search { .. } => app.config.mode_search(),
        Mode::Picker => app.config.mode_picker(),
        Mode::Prompt(_) => app.config.mode_prompt(),
        Mode::DebugPane => app.config.mode_debug(),
        Mode::Terminal => app.config.mode_terminal(),
        Mode::FileTree => app.config.mode_picker(),
        Mode::RenamePreview => app.config.mode_picker(),
        Mode::Installer => app.config.mode_picker(),
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
        Lang::Scss => '\u{e603}',
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
        Lang::Scss => "scss",
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

    // Single-row chrome: `[DEBUG | <adapter>] <tab> <tab> …`
    // sitting on the pane's Mantle background. No separate tab
    // strip. Per-stop status info ("breakpoint bound at line 13",
    // "stopped — breakpoint") has moved to the editor status line
    // so the header stays minimal.
    let pane_bg = app.config.chrome_bg();
    let header_fg = app.config.theme_fg();
    let body_bg = pane_bg;
    let muted = app.config.theme_dim();
    let active_bg = app.config.debug_active_tab_bg();
    let base = app.config.theme_chip_fg();

    // ---------------------------------------------------------------------
    // Header row. `[DEBUG | <adapter>]` chip on the left followed
    // by the tab labels. Active tab paints green bg + white text.
    // Inactive tabs are muted on the pane bg.
    // ---------------------------------------------------------------------
    let adapter_label = app
        .dap
        .session
        .as_ref()
        .map(|s| s.adapter_key.clone())
        // No session yet, but a prelaunch build is running — show its
        // adapter key so the chip reads `[DEBUG | dotnet]` instead of
        // `[DEBUG | idle]` while `dotnet build` streams into the pane.
        .or_else(|| app.dap.pending_build_adapter_key().map(|s| s.to_string()))
        .unwrap_or_else(|| "idle".into());
    let chip_text = format!(" DEBUG | {} ", adapter_label);
    let chip_w = chip_text.chars().count() as u16;
    // Chip styling stays as the original peach badge — clear "you
    // are in the debugger" identifier, distinct from the active
    // tab's green badge so the two never compete visually.
    let chip_bg = app.config.debug_chip_bg();
    queue!(out, MoveTo(0, top as u16))?;
    queue!(
        out,
        SetBackgroundColor(chip_bg),
        SetForegroundColor(base),
        SetAttribute(Attribute::Bold),
        Print(&chip_text),
        SetAttribute(Attribute::NormalIntensity),
    )?;
    let mut tab_x: u16 = chip_w + 1;
    let mut hitboxes: Vec<(crate::app::DapPaneTab, u16, u16)> = Vec::new();
    queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
    for tab in crate::app::DapPaneTab::all().iter() {
        let label = tab.label();
        let chip = format!(" {} ", label);
        let chip_chars = chip.chars().count() as u16;
        let is_active = *tab == app.dap_pane_tab;
        let (bg, fg) = if is_active {
            (active_bg, base)
        } else {
            (pane_bg, muted)
        };
        queue!(
            out,
            MoveTo(tab_x, top as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
        )?;
        if is_active {
            queue!(out, SetAttribute(Attribute::Bold))?;
        } else {
            queue!(out, SetAttribute(Attribute::NormalIntensity))?;
        }
        queue!(out, Print(&chip))?;
        queue!(out, SetAttribute(Attribute::NormalIntensity))?;
        hitboxes.push((*tab, tab_x, tab_x + chip_chars));
        // 1-cell gap between tabs so the active green chip doesn't
        // butt against the next label. Explicitly paint the gap with
        // the pane background — without this, buffer content drawn
        // earlier in the same frame (git stripe glyphs, line content)
        // leaks through the unpainted column.
        tab_x += chip_chars;
        queue!(out, SetBackgroundColor(pane_bg), Print(" "))?;
        tab_x += 1;
    }
    queue!(out, SetBackgroundColor(pane_bg))?;
    if (tab_x as usize) < width {
        queue!(out, Print(" ".repeat(width - tab_x as usize)))?;
    }
    queue!(out, ResetColor)?;
    app.dap_tab_hitboxes.set(hitboxes);

    // ---------------------------------------------------------------------
    // Body — one tab at a time. Body starts on the row directly
    // below the header (no tab strip anymore).
    // ---------------------------------------------------------------------
    let body_top = top + 1;
    let body_rows = rows.saturating_sub(1);
    let pane_focused = app.mode == Mode::DebugPane;

    let rows_buf: Vec<DapTabRow> = match app.dap_pane_tab {
        crate::app::DapPaneTab::Frames => build_frames_rows(app),
        crate::app::DapPaneTab::Locals => build_locals_rows(app, pane_focused),
        crate::app::DapPaneTab::Watches => build_watches_rows(app),
        crate::app::DapPaneTab::Breakpoints => build_breakpoints_rows(app),
        crate::app::DapPaneTab::Console => build_console_rows(app),
    };

    // Console glues the latest line to the bottom by default; every
    // other tab scrolls top-down. So Console's scroll offset = lines
    // hidden BELOW the bottom; others = lines hidden ABOVE the top.
    let total = rows_buf.len();
    let scroll = app.dap_tab_scroll(app.dap_pane_tab);
    let visible_start = if matches!(app.dap_pane_tab, crate::app::DapPaneTab::Console) {
        let end = total.saturating_sub(scroll);
        end.saturating_sub(body_rows)
    } else {
        scroll.min(total.saturating_sub(body_rows))
    };

    let h_skip = app.dap_tab_h_scroll(app.dap_pane_tab);
    for r in 0..body_rows {
        let screen_y = (body_top + r) as u16;
        // `Clear(CurrentLine)` resets the row to the terminal's
        // default background — which on a dark-theme host is
        // visibly lighter than Mantle, producing a flash when an
        // empty pane gradually fills with rows. Skip the Clear
        // and paint Mantle across the full width explicitly so
        // every pane row is the same shade regardless of whether
        // it carries content.
        queue!(out, MoveTo(0, screen_y), SetBackgroundColor(body_bg))?;
        let idx = visible_start + r;
        if let Some(row) = rows_buf.get(idx) {
            paint_dap_row(
                out,
                row,
                width,
                h_skip,
                body_bg,
                app.config.theme_border(),
                muted,
            )?;
        } else {
            queue!(out, Print(" ".repeat(width)))?;
        }
        queue!(out, ResetColor)?;
    }
    Ok(())
}

/// One row of debug-pane body content. `parts` is a list of
/// `(text, fg, bg, attr)` chunks emitted left-to-right. Truncated
/// against the pane width at paint time. Highlighting (selection,
/// breakpoint markers, syntax colour) lives in the part list rather
/// than the painter so the tab-specific builders own their look.
#[derive(Default)]
pub struct DapTabRow {
    pub parts: Vec<DapTabPart>,
    /// True if the row is the currently-selected one in the active
    /// tab (highlighted with Surface2 background).
    pub selected: bool,
    /// Optional char-column range `[start, end)` to highlight as a
    /// mouse-drag selection. Used by the Console tab. Painted in
    /// Surface2 on top of whatever the per-part colours were.
    pub selection_range: Option<(usize, usize)>,
}

pub struct DapTabPart {
    pub text: String,
    pub fg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
}

impl DapTabPart {
    pub fn plain(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bold: false,
            italic: false,
        }
    }
    pub fn bold(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bold: true,
            italic: false,
        }
    }
    pub fn italic(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bold: false,
            italic: true,
        }
    }
}

fn paint_dap_row(
    out: &mut impl Write,
    row: &DapTabRow,
    width: usize,
    h_skip: usize,
    pane_bg: Color,
    selection_bg: Color,
    muted: Color,
) -> Result<()> {
    let row_bg = if row.selected { selection_bg } else { pane_bg };
    let sel_bg = selection_bg;
    let sel_range = row.selection_range;
    if width == 0 {
        return Ok(());
    }

    let total_chars: usize = row.parts.iter().map(|p| p.text.chars().count()).sum();

    // Leading single cell. If we've scrolled right past col 0 we
    // hint at the hidden content with a muted ellipsis instead of
    // the usual blank pad — matches how editor `set listchars`
    // marks horizontal overflow.
    queue!(out, SetBackgroundColor(row_bg))?;
    if h_skip > 0 {
        queue!(
            out,
            SetForegroundColor(muted),
            SetAttribute(Attribute::NormalIntensity),
            SetAttribute(Attribute::NoItalic),
            Print("«"),
        )?;
    } else {
        queue!(out, Print(" "))?;
    }
    let mut used = 1usize;
    let mut logical_col = 0usize;

    // Per-cell paint with attr deduplication. Walking parts
    // directly would emit fewer SetBackgroundColor/SetForegroundColor
    // calls but the partial-skip / partial-take bookkeeping for an
    // h_skip that lands in the middle of a part — and the selection
    // range overlay on top of that — would be much messier. The
    // debug pane is rendered at most once per event; the extra
    // queue! calls don't matter here.
    let mut last_bg: Option<Color> = None;
    let mut last_fg: Option<Color> = None;
    let mut last_bold = false;
    let mut last_italic = false;

    'outer: for part in &row.parts {
        for ch in part.text.chars() {
            if logical_col < h_skip {
                logical_col += 1;
                continue;
            }
            if used >= width {
                break 'outer;
            }
            let in_sel = sel_range
                .map(|(s, e)| logical_col >= s && logical_col < e)
                .unwrap_or(false);
            let want_bg = if in_sel { sel_bg } else { row_bg };
            if last_bg != Some(want_bg) {
                queue!(out, SetBackgroundColor(want_bg))?;
                last_bg = Some(want_bg);
            }
            let want_fg = part.fg.unwrap_or(Color::Reset);
            if last_fg != Some(want_fg) {
                queue!(out, SetForegroundColor(want_fg))?;
                last_fg = Some(want_fg);
            }
            if part.bold != last_bold {
                if part.bold {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                } else {
                    queue!(out, SetAttribute(Attribute::NormalIntensity))?;
                }
                last_bold = part.bold;
            }
            if part.italic != last_italic {
                if part.italic {
                    queue!(out, SetAttribute(Attribute::Italic))?;
                } else {
                    queue!(out, SetAttribute(Attribute::NoItalic))?;
                }
                last_italic = part.italic;
            }
            queue!(out, Print(ch))?;
            used += 1;
            logical_col += 1;
        }
    }

    queue!(
        out,
        SetAttribute(Attribute::NormalIntensity),
        SetAttribute(Attribute::NoItalic),
    )?;

    if used < width {
        let pad = width - used;
        if let Some((sel_from, sel_to)) = sel_range {
            // Trailing pad in logical-col space. Selection ranges
            // that extend past the last content char paint over
            // pad cells too — gives "selected through EOL" feedback.
            let pad_start = logical_col;
            let pad_end = pad_start + pad;
            let sel_seg_start = pad_start.max(sel_from);
            let sel_seg_end = pad_end.min(sel_to);
            let mut emitted = 0usize;
            if sel_seg_start > pad_start {
                let n = sel_seg_start - pad_start;
                queue!(out, SetBackgroundColor(row_bg), Print(" ".repeat(n)))?;
                emitted += n;
            }
            if sel_seg_end > sel_seg_start {
                let n = sel_seg_end - sel_seg_start;
                queue!(out, SetBackgroundColor(sel_bg), Print(" ".repeat(n)))?;
                emitted += n;
            }
            if emitted < pad {
                queue!(
                    out,
                    SetBackgroundColor(row_bg),
                    Print(" ".repeat(pad - emitted))
                )?;
            }
        } else {
            queue!(out, SetBackgroundColor(row_bg), Print(" ".repeat(pad)))?;
        }
    }

    // Right-edge overflow marker. If the row still has content past
    // what we drew, overwrite the rightmost visible cell with `»`
    // so the user can see there is more to scroll to.
    if logical_col < total_chars && width >= 1 {
        queue!(
            out,
            MoveToColumn(width as u16 - 1),
            SetBackgroundColor(row_bg),
            SetForegroundColor(muted),
            Print("»"),
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Per-tab row builders. Each returns a Vec<DapTabRow> that the
// renderer then scroll-windows and paints.
// ---------------------------------------------------------------------

fn build_frames_rows(app: &App) -> Vec<DapTabRow> {
    let mut rows: Vec<DapTabRow> = Vec::new();
    let palette = DebugPalette::from_config(&app.config);
    let Some(session) = app.dap.session.as_ref() else {
        rows.push(note_row(no_session_note(), &palette));
        return rows;
    };
    if session.frames.is_empty() {
        rows.push(note_row(empty_frames_note(session), &palette));
        return rows;
    }
    let selected =
        if app.mode == Mode::DebugPane && app.dap_pane_tab == crate::app::DapPaneTab::Frames {
            Some(app.dap_pane_cursor.min(session.frames.len() - 1))
        } else {
            None
        };
    for (i, f) in session.frames.iter().enumerate() {
        let loc = f
            .source
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|n| format!("{}:{}", n, f.line))
            .unwrap_or_else(|| format!("?:{}", f.line));
        let parts = vec![
            DapTabPart::plain(format!("{:>3}  ", i), palette.muted),
            DapTabPart::plain(loc, palette.peach),
            DapTabPart::plain("  ", palette.muted),
            DapTabPart::plain(f.name.clone(), palette.blue),
        ];
        rows.push(DapTabRow {
            selection_range: None,
            parts,
            selected: selected == Some(i),
        });
    }
    rows
}

fn build_locals_rows(app: &App, pane_focused: bool) -> Vec<DapTabRow> {
    let mut rows: Vec<DapTabRow> = Vec::new();
    let palette = DebugPalette::from_config(&app.config);
    let Some(session) = app.dap.session.as_ref() else {
        rows.push(note_row(no_session_note(), &palette));
        return rows;
    };
    let flat = crate::dap::flat_locals_view(session);
    if flat.is_empty() {
        rows.push(note_row(
            "(no locals — frame may not have any in scope)",
            &palette,
        ));
        return rows;
    }
    let selected = if pane_focused && app.dap_pane_tab == crate::app::DapPaneTab::Locals {
        Some(app.dap_pane_cursor.min(flat.len() - 1))
    } else {
        None
    };
    for (i, row) in flat.iter().enumerate() {
        let marker = if row.expandable {
            if row.expanded { '▼' } else { '▶' }
        } else {
            ' '
        };
        let indent: String = "  ".repeat(row.depth);
        let mut parts = vec![
            DapTabPart::plain(indent, palette.muted),
            DapTabPart::plain(format!("{} ", marker), palette.muted),
            DapTabPart::plain(row.var.name.clone(), palette.lavender),
        ];
        if let Some(t) = &row.var.type_name {
            parts.push(DapTabPart::italic(format!(": {} ", t), palette.subtle));
        }
        parts.push(DapTabPart::plain("= ", palette.subtle));
        parts.push(value_part(&row.var.value, &palette));
        rows.push(DapTabRow {
            selection_range: None,
            parts,
            selected: selected == Some(i),
        });
    }
    rows
}

fn build_watches_rows(app: &App) -> Vec<DapTabRow> {
    let mut rows: Vec<DapTabRow> = Vec::new();
    let palette = DebugPalette::from_config(&app.config);
    if app.dap.watches.is_empty() {
        rows.push(note_row(
            "(no watches — add via `:dapwatch <expr>`)",
            &palette,
        ));
        return rows;
    }
    let selected =
        if app.mode == Mode::DebugPane && app.dap_pane_tab == crate::app::DapPaneTab::Watches {
            Some(app.dap_pane_cursor.min(app.dap.watches.len() - 1))
        } else {
            None
        };
    for (i, w) in app.dap.watches.iter().enumerate() {
        let mut parts = vec![
            DapTabPart::plain(format!("{:>3}  ", i + 1), palette.muted),
            DapTabPart::plain(w.expr.clone(), palette.lavender),
            DapTabPart::plain(" = ", palette.subtle),
        ];
        match &w.result {
            Some(r) if r.error => {
                parts.push(DapTabPart::plain(r.value.clone(), palette.red));
            }
            Some(r) => {
                parts.push(value_part(&r.value, &palette));
                if let Some(t) = &r.type_name {
                    parts.push(DapTabPart::italic(format!("  : {}", t), palette.subtle));
                }
            }
            None => {
                parts.push(DapTabPart::italic("…", palette.muted));
            }
        }
        rows.push(DapTabRow {
            selection_range: None,
            parts,
            selected: selected == Some(i),
        });
    }
    rows
}

fn build_breakpoints_rows(app: &App) -> Vec<DapTabRow> {
    let mut rows: Vec<DapTabRow> = Vec::new();
    let palette = DebugPalette::from_config(&app.config);
    if app.dap.breakpoints.is_empty() {
        rows.push(note_row(
            "(no breakpoints — F9 to toggle on the cursor's line)",
            &palette,
        ));
        return rows;
    }
    let mut entries: Vec<(String, usize, &Vec<crate::dap::SourceBreakpoint>)> = app
        .dap
        .breakpoints
        .iter()
        .map(|(p, b)| {
            let display = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>")
                .to_string();
            (display, b.len(), b)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let pane_focused_on_bp =
        app.mode == Mode::DebugPane && app.dap_pane_tab == crate::app::DapPaneTab::Breakpoints;
    let mut idx = 0usize;
    let total_bp: usize = entries.iter().map(|(_, n, _)| *n).sum();
    let selected = if pane_focused_on_bp && total_bp > 0 {
        Some(app.dap_pane_cursor.min(total_bp - 1))
    } else {
        None
    };
    for (display, _, bps) in &entries {
        for bp in *bps {
            // `◆` for conditional / hit-count, `●` for plain — same
            // glyph convention as the editor gutter so the user can
            // tell them apart at a glance in the pane.
            let glyph = if bp.is_conditional() {
                "◆  "
            } else {
                "●  "
            };
            let mut parts = vec![
                DapTabPart::plain(glyph, palette.red),
                DapTabPart::plain(display.clone(), palette.peach),
                DapTabPart::plain(":", palette.muted),
                DapTabPart::plain(format!("{}", bp.line), palette.yellow),
            ];
            // Surface the expression(s) inline so the user can see
            // *why* this is a `◆` without round-tripping through the
            // status line. Conditions and hit-counts can both be
            // present; print them as `if <expr>` / `hit <expr>` with
            // a separator.
            if let Some(cond) = &bp.condition {
                parts.push(DapTabPart::plain("  if ", palette.muted));
                parts.push(DapTabPart::plain(cond.clone(), palette.green));
            }
            if let Some(hit) = &bp.hit_condition {
                parts.push(DapTabPart::plain("  hit ", palette.muted));
                parts.push(DapTabPart::plain(hit.clone(), palette.green));
            }
            rows.push(DapTabRow {
                selection_range: None,
                parts,
                selected: selected == Some(idx),
            });
            idx += 1;
        }
    }
    rows
}

fn build_console_rows(app: &App) -> Vec<DapTabRow> {
    let palette = DebugPalette::from_config(&app.config);
    let mut rows: Vec<DapTabRow> = Vec::new();
    if app.dap.output_buffer.is_empty() {
        rows.push(note_row("(no console output yet)", &palette));
        return rows;
    }
    // Pre-compute the normalised selection range so we can drop
    // a per-row `selection_range` on each affected line. The
    // mouse handler stores anchor + head as
    // `(flat_line_idx, char_col)`, so we walk the same flat order
    // here to assign per-row ranges.
    let selection = app.dap_console_selection.map(|s| s.ordered());
    let mut flat_idx = 0usize;
    for line in app.dap.output_buffer.iter() {
        for one in line.output.lines() {
            let parts = match line.category.as_str() {
                "stderr" => vec![DapTabPart::plain(one.to_string(), palette.red)],
                "console" => vec![DapTabPart::plain(one.to_string(), palette.subtle)],
                _ => tokenize_console_line(one, &palette),
            };
            let line_len = one.chars().count();
            let selection_range = selection.and_then(|(start, end)| {
                if flat_idx < start.0 || flat_idx > end.0 {
                    return None;
                }
                let from = if flat_idx == start.0 { start.1 } else { 0 };
                let to = if flat_idx == end.0 { end.1 } else { line_len };
                if to > from { Some((from, to)) } else { None }
            });
            rows.push(DapTabRow {
                parts,
                selected: false,
                selection_range,
            });
            flat_idx += 1;
        }
    }
    rows
}

/// Split a single console line into syntax-coloured parts. Pattern
/// recognition is intentionally tiny — just enough to make the
/// dominant log shapes (`.NET ILogger` / `microsoft.extensions.logging`
/// / structured-logging frameworks generally) read clearly. Falls
/// through to plain text for anything that doesn't match.
fn tokenize_console_line(line: &str, p: &DebugPalette) -> Vec<DapTabPart> {
    let mut parts: Vec<DapTabPart> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    // Try to peel a leading `[HH:MM:SS LVL]` prefix. The `[…]` chunk
    // goes through with the timestamp dim and the level severity-
    // coloured; the rest of the line continues into the
    // pattern-matching tokeniser below.
    if let Some(prefix_end) = match_log_prefix(&chars) {
        // chars: `[HH:MM:SS LVL]` → split into `[`, `HH:MM:SS`, ` `, `LVL`, `]`.
        let raw: String = chars[..prefix_end].iter().collect();
        let inner: &str = raw.trim_matches(|c| c == '[' || c == ']');
        let (time_part, level_part) = match inner.rfind(' ') {
            Some(sp) => (inner[..sp].to_string(), inner[sp + 1..].to_string()),
            None => (inner.to_string(), String::new()),
        };
        parts.push(DapTabPart::plain("[".to_string(), p.muted));
        parts.push(DapTabPart::plain(time_part, p.subtle));
        if !level_part.is_empty() {
            parts.push(DapTabPart::plain(" ".to_string(), p.muted));
            parts.push(DapTabPart::bold(
                level_part.clone(),
                severity_colour(&level_part, p),
            ));
        }
        parts.push(DapTabPart::plain("] ".to_string(), p.muted));
        i = prefix_end;
        // Skip a single trailing space the bracket usually carries.
        if chars.get(i).copied() == Some(' ') {
            i += 1;
        }
    }
    // Tokenise the body: URLs, paths, numbers, identifiers, quoted
    // strings. Everything between matches is plain text.
    let mut plain_start = i;
    while i < chars.len() {
        let c = chars[i];
        // URL — `https://...` / `http://...`
        if (c == 'h' || c == 'H')
            && starts_with_ci(&chars, i, "http")
            && next_is_scheme_tail(&chars, i)
        {
            flush_plain(&mut parts, &chars, plain_start, i, p);
            let end = scan_url(&chars, i);
            let token: String = chars[i..end].iter().collect();
            parts.push(DapTabPart::plain(token, p.blue));
            i = end;
            plain_start = i;
            continue;
        }
        // Quoted string — `"..."` or `'...'`. Greedy to the next
        // matching quote on the same line (no embedded escape
        // handling — log lines rarely need it).
        if c == '"' || c == '\'' {
            flush_plain(&mut parts, &chars, plain_start, i, p);
            let end = scan_quoted(&chars, i, c);
            let token: String = chars[i..end].iter().collect();
            parts.push(DapTabPart::plain(token, p.green));
            i = end;
            plain_start = i;
            continue;
        }
        // Number — runs of digits + a few embedded chars (`:`/`.` for
        // timestamps + decimals). Anchored to a non-word boundary so
        // `Box64` doesn't get a colour split mid-word.
        if c.is_ascii_digit() && is_word_boundary(&chars, i.wrapping_sub(1)) {
            flush_plain(&mut parts, &chars, plain_start, i, p);
            let end = scan_number_run(&chars, i);
            let token: String = chars[i..end].iter().collect();
            parts.push(DapTabPart::plain(token, p.peach));
            i = end;
            plain_start = i;
            continue;
        }
        // PascalCase identifier — `TempFileCleanupJob`, `MyController`.
        // Anchored to a word-boundary start so embedded caps in
        // camelCase don't split.
        if c.is_ascii_uppercase() && is_word_boundary(&chars, i.wrapping_sub(1)) {
            let end = scan_identifier(&chars, i);
            if end - i >= 2 && pascal_has_lower(&chars[i..end]) {
                flush_plain(&mut parts, &chars, plain_start, i, p);
                let token: String = chars[i..end].iter().collect();
                parts.push(DapTabPart::plain(token, p.yellow));
                i = end;
                plain_start = i;
                continue;
            }
        }
        i += 1;
    }
    flush_plain(&mut parts, &chars, plain_start, chars.len(), p);
    if parts.is_empty() {
        parts.push(DapTabPart::plain(line.to_string(), p.text));
    }
    parts
}

fn match_log_prefix(chars: &[char]) -> Option<usize> {
    // `[…]` with non-empty contents, found within the first ~24 chars.
    if chars.first().copied() != Some('[') {
        return None;
    }
    let cap = chars.len().min(64);
    let mut i = 1;
    while i < cap {
        if chars[i] == ']' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn severity_colour(level: &str, p: &DebugPalette) -> Color {
    match level {
        "ERR" | "ERROR" | "FATAL" | "FTL" | "CRT" | "CRIT" | "CRITICAL" => p.red,
        "WRN" | "WARN" | "WARNING" => p.yellow,
        "INF" | "INFO" => p.blue,
        "DBG" | "DEBUG" => p.lavender,
        "TRC" | "TRACE" | "VRB" | "VERBOSE" => p.muted,
        _ => p.subtle,
    }
}

fn flush_plain(
    parts: &mut Vec<DapTabPart>,
    chars: &[char],
    start: usize,
    end: usize,
    p: &DebugPalette,
) {
    if end > start {
        let text: String = chars[start..end].iter().collect();
        parts.push(DapTabPart::plain(text, p.text));
    }
}

fn starts_with_ci(chars: &[char], i: usize, prefix: &str) -> bool {
    let pchars: Vec<char> = prefix.chars().collect();
    if i + pchars.len() > chars.len() {
        return false;
    }
    for (k, p) in pchars.iter().enumerate() {
        if chars[i + k].to_ascii_lowercase() != p.to_ascii_lowercase() {
            return false;
        }
    }
    true
}

fn next_is_scheme_tail(chars: &[char], i: usize) -> bool {
    // After `http` / `HTTP`, expect `s?://`.
    let mut j = i + 4;
    if chars.get(j).copied() == Some('s') || chars.get(j).copied() == Some('S') {
        j += 1;
    }
    chars.get(j).copied() == Some(':')
        && chars.get(j + 1).copied() == Some('/')
        && chars.get(j + 2).copied() == Some('/')
}

fn scan_url(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while j < chars.len() {
        let c = chars[j];
        if c.is_whitespace()
            || c == ')'
            || c == ']'
            || c == '"'
            || c == '\''
            || c == ','
            || c == ';'
        {
            break;
        }
        j += 1;
    }
    j
}

fn scan_quoted(chars: &[char], i: usize, q: char) -> usize {
    let mut j = i + 1;
    while j < chars.len() {
        if chars[j] == q {
            return j + 1;
        }
        j += 1;
    }
    chars.len()
}

fn scan_number_run(chars: &[char], i: usize) -> usize {
    let mut j = i;
    let mut saw_digit_after_punct = true;
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_digit() {
            saw_digit_after_punct = true;
            j += 1;
            continue;
        }
        if (c == ':' || c == '.' || c == '_') && saw_digit_after_punct {
            // Only extend through a separator if the next char is
            // another digit — otherwise stop here so `12.` at end
            // of a sentence doesn't swallow the period.
            if chars.get(j + 1).is_some_and(|n| n.is_ascii_digit()) {
                saw_digit_after_punct = false;
                j += 1;
                continue;
            }
        }
        break;
    }
    j
}

fn scan_identifier(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_alphanumeric() || c == '_' {
            j += 1;
        } else {
            break;
        }
    }
    j
}

fn pascal_has_lower(slice: &[char]) -> bool {
    // True for PascalCase / mixed identifiers ("FooBar", "MyClass")
    // but false for SCREAMING_SNAKE_CASE acronyms ("URLS", "JSON")
    // which shouldn't get type-name colouring.
    slice.iter().skip(1).any(|c| c.is_ascii_lowercase())
}

fn is_word_boundary(chars: &[char], i: usize) -> bool {
    if i >= chars.len() {
        return true;
    }
    let c = chars[i];
    !c.is_ascii_alphanumeric() && c != '_'
}

/// Pick a colour for a DAP variable value based on a cheap shape
/// heuristic — quotes / digits / true/false / null. Matches the
/// existing tree-sitter palette so the pane reads consistent with
/// source-code highlighting.
fn value_part(value: &str, p: &DebugPalette) -> DapTabPart {
    let trim = value.trim();
    if trim.starts_with('"') || trim.starts_with('\'') {
        return DapTabPart::plain(value.to_string(), p.green);
    }
    if matches!(trim, "true" | "false" | "True" | "False") {
        return DapTabPart::plain(value.to_string(), p.peach);
    }
    if matches!(
        trim,
        "null" | "Null" | "None" | "nil" | "undefined" | "(null)"
    ) {
        return DapTabPart::italic(value.to_string(), p.muted);
    }
    if trim.chars().all(|c| {
        c.is_ascii_digit() || c == '-' || c == '.' || c == 'x' || c == 'X' || c.is_ascii_hexdigit()
    }) && !trim.is_empty()
    {
        return DapTabPart::plain(value.to_string(), p.peach);
    }
    DapTabPart::plain(value.to_string(), p.text)
}

fn note_row(text: &str, p: &DebugPalette) -> DapTabRow {
    DapTabRow {
        selection_range: None,
        parts: vec![DapTabPart::italic(text.to_string(), p.muted)],
        selected: false,
    }
}

fn empty_frames_note(session: &crate::dap::DapSession) -> &'static str {
    match session.state {
        crate::dap::SessionState::Stopped { .. } => "(stopped — waiting for stackTrace)",
        crate::dap::SessionState::Running => "(running — no frames)",
        crate::dap::SessionState::Initializing => "(initialising)",
        crate::dap::SessionState::Configuring => "(configuring)",
        crate::dap::SessionState::Terminated => "(terminated)",
    }
}

fn no_session_note() -> &'static str {
    "(no debug session — :debug to start)"
}

/// Catppuccin-Mocha-derived colours used across the debug pane.
/// Mirrors the editor's syntax palette so variable names / values
/// / types read consistently between source and pane.
struct DebugPalette {
    base: Color,
    text: Color,
    /// Subtext0 — readable but distinctly less prominent than text.
    /// Used for type chips and `=` separators where "this is
    /// metadata, not the primary identifier" needs to read
    /// quietly without becoming invisible. Pure Overlay0 (0x6c…)
    /// turned out too dim — types like `Articles.PagedRequest`
    /// disappeared into the bg.
    subtle: Color,
    muted: Color,
    accent: Color,
    blue: Color,
    lavender: Color,
    green: Color,
    peach: Color,
    yellow: Color,
    red: Color,
}

impl DebugPalette {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            base: config.theme_chip_fg(),
            text: config.theme_fg(),
            subtle: config.theme_fg(),
            muted: config.theme_dim(),
            accent: config.theme_accent(),
            blue: config.theme_info(),
            lavender: config.theme_emphasis(),
            green: config.theme_accent_secondary(),
            peach: config.theme_accent(),
            yellow: config.theme_warning(),
            red: config.theme_error(),
        }
    }
}

impl Default for DebugPalette {
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
    }
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

    let mode_bg = mode_color(app, app.mode);
    let mode_fg = app.config.theme_chip_fg();
    let branch_bg = app.config.theme_surface();
    let branch_fg = app.config.theme_fg();
    let path_bg = app.config.chrome_bg();
    let path_fg = app.config.theme_dim();
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

    let path_used = mode_w + mode_arrow_w + branch_w + branch_arrow_w + right_arrow_w + right_w;
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
        + (if branch_text.is_empty() {
            0
        } else {
            branch_w + branch_arrow_w
        })
        + 2
        + path_str.chars().count()
        + dirty.chars().count();
    let fill = total.saturating_sub(drawn + right_arrow_w + right_w);
    queue!(out, SetBackgroundColor(path_bg), Print(" ".repeat(fill)),)?;

    // === Right segment (language) ===
    // PL_LEFT (`\u{e0b2}`) is a triangle that fills the *right* half
    // of the cell. To taper from the path segment (left, dark) into
    // the lang chip (right, lighter), the wedge cell needs the path
    // colour as its bg (covering the cell's left half) and the chip
    // colour as its fg (the triangle on the right half). Reversing
    // the pair paints the triangle dark and leaves a dark block
    // butted up against the chip.
    if !right_text.is_empty() {
        queue!(
            out,
            SetBackgroundColor(path_bg),
            SetForegroundColor(right_bg),
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
    if (app.show_start_page
        || app.show_health_page
        || app.show_messages_page
        || app.show_registers_page
        || app.show_test_results_page)
        && !matches!(app.mode, Mode::Command | Mode::Search { .. } | Mode::Picker)
    {
        queue!(out, Hide)?;
        return Ok(());
    }
    // Focus on the terminal pane (input mode) — show the system
    // cursor at the PTY grid position. This is the last cursor
    // move of the frame so the subsequent flush leaves it where
    // we want it, instead of wherever the last Print landed.
    if matches!(app.mode, Mode::Terminal) {
        // Route cursor placement to whichever pane is focused. The
        // bottom pane sits below the editor; the side pane sits flush
        // against the right edge of the editor band.
        match app.terminal_focus {
            crate::app::TerminalFocus::Side => {
                // Loading splash is up — hide the cursor entirely; it
                // would otherwise blink over the centred logo at
                // whatever stale position the PTY's first frame put
                // it. Once the splash drops, the real PTY cursor
                // resumes positioning.
                if let Some(side) = app.side_terminals.get(app.active_side_terminal_idx) {
                    if crate::app::side_terminal_loading(side) {
                        queue!(out, Hide)?;
                        return Ok(());
                    }
                }
                if let Some(t) = app.active_side_terminal() {
                    let (cur_row, cur_col) = t.cursor();
                    let body_top = app.buffer_top() + 1;
                    let screen_y = body_top + cur_row;
                    let screen_x = app.side_pane_content_left() + cur_col;
                    if (screen_y as u16) < app.height && (screen_x as u16) < app.width {
                        queue!(out, Show, MoveTo(screen_x as u16, screen_y as u16))?;
                        return Ok(());
                    }
                }
            }
            crate::app::TerminalFocus::Bottom => {
                if let Some(t) = app.active_terminal() {
                    let (cur_row, cur_col) = t.cursor();
                    // Body of the pane starts one row below `terminal_pane_top()`
                    // (the first pane row is the header chip).
                    let body_top = app.terminal_pane_top() + 1;
                    let screen_y = body_top + cur_row;
                    if (screen_y as u16) < app.height && (cur_col as u16) < app.width {
                        queue!(out, Show, MoveTo(cur_col as u16, screen_y as u16))?;
                        return Ok(());
                    }
                }
            }
        }
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
    // Same for the file-tree pane — the highlighted row IS the cursor;
    // a flashing block in the buffer area would just be noise. The
    // delete-confirm popup paints its own "your input lands here"
    // cursor as part of the popup body (see `draw_file_tree_confirm`),
    // so the terminal cursor stays hidden in either case.
    if app.mode == Mode::FileTree {
        queue!(out, Hide)?;
        return Ok(());
    }
    // The floating cmdline popup (Command / Search / Prompt modes)
    // also paints its own cursor as a highlighted cell — bulletproof
    // against terminals that drop the system cursor inside
    // synchronized updates. Keep the system cursor hidden so we don't
    // get a duplicate.
    if matches!(
        app.mode,
        Mode::Command | Mode::Search { .. } | Mode::Prompt(_)
    ) {
        queue!(out, Hide)?;
        return Ok(());
    }
    let style = match app.mode {
        Mode::Insert => SetCursorStyle::SteadyBar,
        _ => SetCursorStyle::SteadyBlock,
    };
    queue!(out, style, Show)?;
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
    // Cursor lives inside the active pane — translate the cursor's
    // logical row/col into terminal coords through that pane's rect,
    // not the full editor area.
    let pane = app.active_pane_rect();
    // Hidden rows (folded code blocks, markdown chrome like
    // `<details>`/`</details>`) collapse out of the visible render,
    // so the cursor's on-screen row needs to count *visible* rows
    // between view_top and the cursor's source line — not the raw
    // line-index delta.
    let content_row = app.visible_rows_between(app.window.view_top, app.window.cursor.line) as u16;
    // Phantom hop: park the visual cursor on the lens row above the
    // content line. `cursor.line` is unchanged so edits / ENTER target
    // the right line. Self-heals if the lens vanished between the
    // motion and this frame (server response cleared the cache).
    let phantom_idx = app
        .phantom_lens_idx
        .filter(|_| app.line_has_code_lens(app.window.cursor.line) && content_row > 0);
    let row = pane.y
        + if phantom_idx.is_some() {
            content_row - 1
        } else {
            content_row
        };
    if let Some(idx) = phantom_idx {
        // Phantom row paints `gutter` blanks, then lens titles joined by
        // " │ ". Replay the join widths so the visual cursor lands on the
        // first cell of the selected segment.
        let titles = app
            .buffer
            .path
            .as_ref()
            .map(|p| app.lens_commands_on_line(p, app.window.cursor.line))
            .unwrap_or_default();
        let mut text_col = 0usize;
        let separator_w = " │ ".chars().count();
        for (i, cmd) in titles.iter().enumerate() {
            if i == idx {
                break;
            }
            text_col += cmd.title.chars().count();
            if i + 1 < titles.len() {
                text_col += separator_w;
            }
        }
        let col = pane.x + (gutter + text_col) as u16;
        queue!(out, MoveTo(col, row))?;
        return Ok(());
    }
    let line = app.buffer.rope.line(app.window.cursor.line);
    // Per-buffer-col inlay-hint widths so the cursor's visual position
    // accounts for them. Without this, the cursor renders at the visual
    // column corresponding to its buffer-char count and visually lands
    // *inside* any hint(s) anchored at or before its col — making
    // Backspace / typing edit a buffer position that's "ahead" of where
    // the user thinks the cursor is.
    let hints_at: Vec<usize> = inlay_hint_widths_for_line(app, app.window.cursor.line);
    // Markdown concealed mode collapses / replaces source spans, so the
    // cursor's visual column needs to walk the same transforms the
    // renderer used. Without this, the cursor would land at the
    // buffer-char visual position — which is past the rendered
    // content for hidden ranges, putting the terminal cursor several
    // cells right of where the user sees their position.
    if app.markdown_render_active() {
        if let Some(meta) = app.markdown_line_meta(app.window.cursor.line) {
            let line_chars: Vec<char> = line.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            let visual = crate::markdown_render::visual_col_for_buffer_col(
                &line_chars,
                meta,
                app.window.cursor.col,
                TAB_WIDTH,
            );
            let on_screen = visual.saturating_sub(app.window.view_left);
            let col = pane.x + (gutter + on_screen) as u16;
            queue!(out, MoveTo(col, row))?;
            return Ok(());
        }
    }
    let visual = cursor_visual_col_walk(line.chars(), app.window.cursor.col, &hints_at);
    // Account for horizontal scroll. `adjust_viewport` keeps the cursor
    // within `[view_left, view_left + buffer_cols)`, so subtraction is safe.
    let on_screen = visual.saturating_sub(app.window.view_left);
    let col = pane.x + (gutter + on_screen) as u16;
    queue!(out, MoveTo(col, row))?;
    Ok(())
}

/// Visual column of `cursor_col` on a line: tabs count as `TAB_WIDTH`,
/// plus the label width of every inlay hint anchored *strictly before*
/// the cursor (`hint_widths[i]` = total hint width at char col `i`; a
/// short/empty slice means no hints). Hints anchored *at* `cursor_col`
/// are deliberately excluded — the cursor sits between buffer chars
/// `N-1` and `N`, and a hint at col `N` renders just to the cursor's
/// right (it annotates the char behind the cursor, e.g. `var view│: int`
/// where `│` is the cursor). Counting it would push the cursor past the
/// hint and make a backspace feel like it ate from the next token.
///
/// Shared by the cursor renderer (`place_cursor`) and the viewport
/// tracker (`App::cursor_visual_col`) so the two can't drift: when the
/// viewport's walk ignored hints, it kept thinking the cursor was near
/// the left edge and never scrolled, drawing the cursor off the pane on
/// hint-heavy lines (Razor / C# type hints) even when the buffer line
/// itself was short.
pub(crate) fn cursor_visual_col_walk(
    chars: impl Iterator<Item = char>,
    cursor_col: usize,
    hint_widths: &[usize],
) -> usize {
    let mut visual = 0usize;
    for (i, c) in chars.enumerate() {
        if i >= cursor_col {
            break;
        }
        if let Some(w) = hint_widths.get(i) {
            visual += *w;
        }
        visual += if c == '\t' { TAB_WIDTH } else { 1 };
    }
    visual
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
