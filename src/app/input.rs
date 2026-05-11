//! Per-mode key handlers, the mouse handler, and the `:`-command dispatch
//! that converts an `ExCommand` back into App mutations. Search input
//! lives in `search.rs` (it shares state with the search machinery), and
//! the LSP-related rename prompt lives in `lsp_glue.rs`.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::path::PathBuf;

use crate::command::{self, ExCommand, ExRange};
use crate::mode::{Mode, VisualKind};
use crate::motion;
use crate::parser::{self, ParseCtx, ParseResult};

use super::pair::{detect_open_tag_to_close, is_close_char, is_html_like_buffer, open_pair_for, should_auto_pair};
use super::state::LastEdit;

/// Characters that should re-fire `textDocument/completion` after being inserted.
/// Identifier chars catch the typing-a-name case; the symbol set covers the
/// trigger characters servers care about most: member access (`.`), Rust paths
/// and Tailwind variants (`:`), Razor/decorator anchors (`@`), JSX/HTML opens
/// (`<`), and CSS property/utility separators (`-`).
pub(super) fn is_completion_trigger(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '<' | '-')
}

/// Reverse of the renderer's `display_w` walk — given a visual column on
/// `line`, return the buffer char column that visual position sits in. A
/// click past end-of-line clamps to `line_len.saturating_sub(1)` (matches
/// Vim's "cursor sits on a char, not past it" rule in Normal mode).
fn visual_col_to_char_col(
    buffer: &crate::buffer::Buffer,
    line: usize,
    visual_col: usize,
    line_len: usize,
) -> usize {
    if line_len == 0 {
        return 0;
    }
    let slice = buffer.rope.line(line);
    let mut visual = 0usize;
    let mut chars = 0usize;
    for c in slice.chars() {
        if c == '\n' || c == '\r' {
            break;
        }
        let w = if c == '\t' { crate::render::TAB_WIDTH } else { 1 };
        // Clicked exactly on this char's first cell — return its index.
        if visual >= visual_col {
            break;
        }
        // Clicked into the *middle* of a multi-cell char (a tab). Snap to
        // the char itself rather than overshooting into the next one.
        if visual + w > visual_col {
            return chars;
        }
        visual += w;
        chars += 1;
    }
    chars.min(line_len - 1)
}

impl super::App {
    pub(super) fn handle_event(&mut self) -> anyhow::Result<()> {
        match crossterm::event::read()? {
            crossterm::event::Event::Key(k)
                if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                if !matches!(self.mode, Mode::Command) {
                    self.status_msg.clear();
                }
                // Hover popup intercepts scroll keys; everything else dismisses it.
                if self.hover.is_some() {
                    if self.try_scroll_hover(&k) {
                        return Ok(());
                    }
                }
                self.hover = None;
                self.whichkey = None;
                // IDE-parity debug function keys — work regardless of mode
                // so F10 / F11 / F5 behave the way the user's muscle
                // memory expects coming from Visual Studio / Rider.
                if self.try_handle_debug_function_key(&k) {
                    return Ok(());
                }
                // Macro recording: stop on `q` in normal, otherwise capture every key.
                if !self.replaying_macro && self.recording_macro.is_some() {
                    let stop = matches!(self.mode, Mode::Normal)
                        && matches!(k.code, KeyCode::Char('q'))
                        && !k.modifiers.contains(KeyModifiers::CONTROL);
                    if stop {
                        let name = self.recording_macro.take().unwrap();
                        let keys = std::mem::take(&mut self.macro_buffer);
                        self.status_msg = format!("recorded @{} ({} keys)", name, keys.len());
                        self.macros.insert(name, keys);
                        return Ok(());
                    }
                    self.macro_buffer.push(k);
                }
                // While the start page is visible the buffer is read-only —
                // only the cmdline (`:e`, `:q`) and the leader pickers can
                // navigate away from it. A pending leader chord (e.g. the
                // `e` after `<space>`) is also allowed so multi-key shortcuts
                // resolve normally.
                let leader_pending = self.pending.awaiting_leader
                    || self.pending.awaiting_buffer_leader
                    || self.pending.awaiting_debug_leader;
                if self.show_start_page
                    && matches!(self.mode, Mode::Normal)
                    && !leader_pending
                    && !super::state::is_start_page_passthrough(&k)
                {
                    return Ok(());
                }
                match self.mode {
                    Mode::Normal => self.handle_keyboard(k, ParseCtx::Normal),
                    Mode::Insert => self.handle_insert_key(k),
                    Mode::Command => self.handle_command_key(k),
                    Mode::Visual(_) => self.handle_keyboard(k, ParseCtx::Visual),
                    Mode::Search { .. } => self.handle_search_key(k),
                    Mode::Picker => self.handle_picker_key(k),
                    Mode::Prompt(_) => self.handle_prompt_key(k),
                    Mode::DebugPane => {
                        self.handle_debug_pane_key(k);
                    }
                }
            }
            crossterm::event::Event::Mouse(me) => {
                self.handle_mouse_event(me);
            }
            crossterm::event::Event::Resize(w, h) => {
                self.width = w;
                self.height = h;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, ev: MouseEvent) {
        // Don't process mouse events while an overlay is up — picker/cmdline/etc
        // expect keyboard interaction. Scroll wheel still works to dismiss them.
        let in_overlay = self.has_modal_overlay();
        let row = ev.row as usize;
        let col = ev.column as usize;
        let buffer_rows = self.buffer_rows();

        // Left-click on the top-right notification → copy its content to the
        // system clipboard and the unnamed register. Lets the user grab paths
        // and other reported strings without dropping into selection mode.
        if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
            && self.click_inside_notification(row, col)
        {
            let text = self.status_msg.clone();
            if !text.is_empty() {
                let mut copied_clipboard = false;
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if cb.set_text(text.clone()).is_ok() {
                        copied_clipboard = true;
                    }
                }
                self.write_register(None, text, false);
                self.status_msg = if copied_clipboard {
                    "Copied notification to clipboard".into()
                } else {
                    "Copied notification to register \"".into()
                };
            }
            return;
        }

        match ev.kind {
            MouseEventKind::ScrollUp => {
                self.hover = None;
                self.whichkey = None;
                if matches!(self.mode, Mode::Picker) {
                    if let Some(p) = self.picker.as_mut() {
                        p.move_by(-3);
                    }
                } else {
                    self.scroll_view(-3);
                }
                return;
            }
            MouseEventKind::ScrollDown => {
                self.hover = None;
                self.whichkey = None;
                if matches!(self.mode, Mode::Picker) {
                    if let Some(p) = self.picker.as_mut() {
                        p.move_by(3);
                    }
                } else {
                    self.scroll_view(3);
                }
                return;
            }
            MouseEventKind::ScrollLeft => {
                self.hover = None;
                self.whichkey = None;
                self.scroll_horizontal(-3);
                return;
            }
            MouseEventKind::ScrollRight => {
                self.hover = None;
                self.whichkey = None;
                self.scroll_horizontal(3);
                return;
            }
            _ => {}
        }

        if in_overlay {
            return;
        }
        // Tab-bar click: only on the top row when tabs are showing.
        // Left-click on a tab's close glyph deletes the buffer; click
        // anywhere else inside the tab switches to it. Middle-click
        // anywhere on a tab also deletes it (subject to the same dirty
        // guard) — faster than aiming for the `×`. Clicking the `‹` /
        // `›` overflow chevrons walks the active buffer one step in
        // that direction, which is what shifts the visible slice.
        let buffer_top = self.buffer_top();
        if buffer_top > 0 && row == 0 {
            let total_w = self.width as usize;
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                let slots = crate::render::tab_layout(self);
                let scrolled_left = slots.first().map(|s| s.idx > 0).unwrap_or(false);
                let truncated_right = slots
                    .last()
                    .map(|s| s.idx + 1 < self.buffers.len())
                    .unwrap_or(false);
                // Chevron clicks — only on Left, only when the indicator
                // is actually painted at that column. Middle on a chevron
                // falls through to no-op.
                if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
                    if scrolled_left && col == 0 {
                        let first_visible = slots.first().map(|s| s.idx).unwrap_or(0);
                        let _ = self.switch_to(first_visible.saturating_sub(1));
                        return;
                    }
                    if truncated_right && col == total_w.saturating_sub(1) {
                        let last_visible = slots
                            .last()
                            .map(|s| s.idx)
                            .unwrap_or(self.buffers.len() - 1);
                        let next = (last_visible + 1).min(self.buffers.len() - 1);
                        let _ = self.switch_to(next);
                        return;
                    }
                }
                for slot in &slots {
                    if col >= slot.start_col && col < slot.end_col {
                        self.show_start_page = false;
                        let is_middle = matches!(
                            ev.kind,
                            MouseEventKind::Down(MouseButton::Middle)
                        );
                        let on_close = slot.close_col == Some(col);
                        if is_middle || on_close {
                            // Match :bd behaviour: refuse to drop a
                            // dirty buffer. The user can :bd! force or
                            // save first.
                            if slot.idx != self.active {
                                let prev_active = self.active;
                                if self.switch_to(slot.idx).is_ok() {
                                    if let Err(e) = self.delete_buffer(false) {
                                        self.status_msg = format!("error: {e}");
                                        // delete_buffer left us on the
                                        // buffer it couldn't drop —
                                        // hop back to where the user was.
                                        if prev_active < self.buffers.len() {
                                            let _ = self.switch_to(prev_active);
                                        }
                                    }
                                }
                            } else if let Err(e) = self.delete_buffer(false) {
                                self.status_msg = format!("error: {e}");
                            }
                            return;
                        }
                        if slot.idx != self.active {
                            let _ = self.switch_to(slot.idx);
                        }
                        return;
                    }
                }
            }
            return;
        }
        if row < buffer_top {
            return;
        }
        let buf_row = row - buffer_top;
        if buf_row >= buffer_rows {
            return; // status line / off-buffer area
        }
        let gutter = self.gutter_width();
        if col < gutter {
            return; // sign column / line numbers
        }
        let buf_line = buf_row + self.view_top;
        if buf_line >= self.buffer.line_count() {
            return;
        }
        let line_len = self.buffer.line_len(buf_line);
        // Translate the click's visual column (chars *as displayed*) to a
        // buffer char column. Tabs render at `TAB_WIDTH` cols but are still
        // a single buffer char, so a naive `raw_col` calculation lands the
        // cursor several chars past tab-indented text. We replay the same
        // width rule the renderer uses (tab = TAB_WIDTH, everything else
        // = 1) walking the line until we've consumed `visual_col` cells.
        let visual_col = col.saturating_sub(gutter) + self.view_left;
        let buf_col = visual_col_to_char_col(&self.buffer, buf_line, visual_col, line_len);

        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Ctrl-click in Normal mode adds a secondary cursor at
                // the click position. Doesn't move the primary cursor —
                // that would defeat the purpose. The cursors persist
                // through the next `i`/`a` into Insert mode, where typing
                // and Backspace mirror at every position.
                // In any other mode the modifier falls through to the
                // normal click handler.
                if matches!(self.mode, Mode::Normal)
                    && ev.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let line_start = self.buffer.line_start_idx(buf_line);
                    let pos = line_start + buf_col;
                    let primary = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                    if pos != primary && !self.additional_cursors.contains(&pos) {
                        self.additional_cursors.push(pos);
                        self.additional_cursors.sort();
                    }
                    return;
                }
                let now = std::time::Instant::now();
                let is_double = self
                    .last_click
                    .filter(|(t, l, c)| {
                        now.duration_since(*t) <= crate::app::DOUBLE_CLICK_WINDOW
                            && *l == buf_line
                            && *c == buf_col
                    })
                    .is_some();
                if matches!(self.mode, Mode::Visual(_)) {
                    self.exit_visual();
                }
                // A plain click (non-Ctrl) outside multi-cursor scope
                // collapses any active additional cursors.
                if !self.additional_cursors.is_empty() {
                    self.additional_cursors.clear();
                }
                self.cursor.line = buf_line;
                self.cursor.col = buf_col;
                self.cursor.want_col = buf_col;
                if is_double {
                    // Expand to the inner word under the cursor and enter
                    // Visual-char mode with that span selected.
                    self.apply_visual_select_textobj(
                        crate::text_object::TextObjectVerb::Word { inner: true },
                    );
                    if self.visual_anchor.is_some() {
                        self.mode = Mode::Visual(VisualKind::Char);
                    }
                    // Clear so a third click within the window doesn't
                    // re-trigger.
                    self.last_click = None;
                } else {
                    self.last_click = Some((now, buf_line, buf_col));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !matches!(self.mode, Mode::Visual(_)) {
                    let anchor = self.cursor;
                    self.mode = Mode::Visual(VisualKind::Char);
                    self.visual_anchor = Some(anchor);
                }
                self.cursor.line = buf_line;
                self.cursor.col = buf_col;
                self.cursor.want_col = buf_col;
            }
            _ => {}
        }
    }

    pub(super) fn handle_keyboard(&mut self, key: KeyEvent, ctx: ParseCtx) {
        match parser::parse(&mut self.pending, key, ctx) {
            ParseResult::Pending => {}
            ParseResult::Cancelled => {
                if matches!(self.mode, Mode::Visual(_)) {
                    self.exit_visual();
                }
            }
            ParseResult::Action(a) => self.apply_action(a),
        }
        // Track any prefix that's awaiting its next key — drives the which-key timer.
        let prefix_active = self.pending.awaiting_leader
            || self.pending.awaiting_buffer_leader
            || self.pending.awaiting_debug_leader;
        if prefix_active {
            if self.leader_pressed_at.is_none() {
                self.leader_pressed_at = Some(std::time::Instant::now());
            }
        } else {
            self.leader_pressed_at = None;
        }
    }

    pub(super) fn handle_insert_key(&mut self, key: KeyEvent) {
        let is_esc = matches!(key.code, KeyCode::Esc);
        // Completion popup intercepts a small set of keys; everything else dismisses it.
        if self.completion.is_some() {
            let captured = self.handle_insert_key_with_completion(key);
            if captured {
                return;
            }
            // Fall through with completion now closed.
        }
        if !self.replaying && !is_esc {
            if let Some(rec) = self.recording.as_mut() {
                rec.keys.push(key);
            }
        }
        match key.code {
            KeyCode::Esc => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                    self.cursor.want_col = self.cursor.col;
                }
                self.mode = Mode::Normal;
                self.signature_help = None;
                // Collapse multi-cursor on the same Esc that exits Insert.
                self.additional_cursors.clear();
                if !self.replaying {
                    if let Some(rec) = self.recording.take() {
                        self.last_edit = Some(LastEdit::InsertSession {
                            prelude: rec.prelude,
                            keys: rec.keys,
                        });
                    }
                }
            }
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'n' || c == 'p') =>
            {
                self.lsp_request_completion(None);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Multi-cursor: skip the autopair / closer-skip dance and
                // just mirror the keystroke at every position. Autopair
                // across N positions is non-trivial (would need to mirror
                // the closer too and keep cursors balanced) and the
                // user is in mass-edit mode anyway.
                if !self.additional_cursors.is_empty() {
                    self.mirror_insert_char(c);
                } else if is_close_char(c)
                    && self.buffer.char_at(self.cursor.line, self.cursor.col) == Some(c)
                {
                    // If the cursor sits on the same closing char the user is typing,
                    // step past it instead of inserting a duplicate. Lets `}`/`)`/`"`
                    // skip over an auto-inserted closer.
                    self.cursor.col += 1;
                    self.cursor.want_col = self.cursor.col;
                } else if let Some(close) = open_pair_for(c) {
                    if should_auto_pair(c, &self.buffer, self.cursor.line, self.cursor.col) {
                        self.buffer.insert_char(self.cursor.line, self.cursor.col, c);
                        self.buffer.insert_char(self.cursor.line, self.cursor.col + 1, close);
                        self.cursor.col += 1;
                        self.cursor.want_col = self.cursor.col;
                    } else {
                        self.buffer.insert_char(self.cursor.line, self.cursor.col, c);
                        self.cursor.col += 1;
                        self.cursor.want_col = self.cursor.col;
                    }
                } else {
                    self.buffer.insert_char(self.cursor.line, self.cursor.col, c);
                    self.cursor.col += 1;
                    self.cursor.want_col = self.cursor.col;
                }
                // Tag auto-completion: typing `>` at the end of an opening
                // HTML tag inserts the matching closer after the cursor so
                // `<div>` becomes `<div>|</div>`. Triggered after the `>`
                // has been written and the cursor advanced past it.
                if c == '>' && is_html_like_buffer(&self.buffer) {
                    if let Some(tag) = detect_open_tag_to_close(
                        &self.buffer,
                        self.cursor.line,
                        self.cursor.col,
                    ) {
                        let closer = format!("</{tag}>");
                        self.buffer
                            .insert_str(self.cursor.line, self.cursor.col, &closer);
                    }
                }
                // Signature help: opening `(` starts the popup, `,` advances
                // the active parameter. Closers dismiss it. Skipped during
                // macro replay so playback doesn't spam LSP requests.
                if !self.replaying {
                    match c {
                        '(' | ',' => self.lsp_request_signature_help(),
                        ')' | '}' | ']' => self.signature_help = None,
                        _ => {}
                    }
                }
                // Auto-trigger completion on identifier and member-access chars.
                // Skipped during macro replay so playback doesn't spam LSP requests.
                if !self.replaying && is_completion_trigger(c) {
                    // Punctuation triggers (`.`, `:`, etc.) get sent to the
                    // server as triggerCharacter so it returns member-access
                    // completions; identifier chars are an Invoked refresh.
                    let trigger = if matches!(c, '.' | ':' | '@' | '<') {
                        Some(c)
                    } else {
                        None
                    };
                    self.lsp_request_completion(trigger);
                }
            }
            KeyCode::Enter => self.handle_insert_newline(),
            KeyCode::Backspace => {
                let popup_was_open = self.completion.is_some();
                if !self.additional_cursors.is_empty() {
                    self.mirror_backspace();
                } else if self.cursor.col > 0 {
                    // If the cursor sits between an auto-inserted pair like {|},
                    // wipe out both characters in one stroke.
                    let prev = self.buffer.char_at(self.cursor.line, self.cursor.col - 1);
                    let next = self.buffer.char_at(self.cursor.line, self.cursor.col);
                    if let (Some(p), Some(n)) = (prev, next) {
                        if open_pair_for(p) == Some(n) {
                            let idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                            self.buffer.delete_range(idx - 1, idx + 1);
                            self.cursor.col -= 1;
                            self.cursor.want_col = self.cursor.col;
                            return;
                        }
                    }
                    let idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                    self.buffer.delete_range(idx - 1, idx);
                    self.cursor.col -= 1;
                    self.cursor.want_col = self.cursor.col;
                } else if self.cursor.line > 0 {
                    let prev = self.cursor.line - 1;
                    let prev_len = self.buffer.line_len(prev);
                    let idx = self.buffer.pos_to_char(prev, prev_len);
                    self.buffer.delete_range(idx, idx + 1);
                    self.cursor.line = prev;
                    self.cursor.col = prev_len;
                    self.cursor.want_col = prev_len;
                }
                if popup_was_open && !self.replaying {
                    self.lsp_request_completion(None);
                }
            }
            KeyCode::Tab => {
                let s = self.editorconfig.indent_string();
                let inserted = s.chars().count();
                self.buffer.insert_str(self.cursor.line, self.cursor.col, &s);
                self.cursor.col += inserted;
                self.cursor.want_col = self.cursor.col;
            }
            KeyCode::Left => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                    self.cursor.want_col = self.cursor.col;
                }
            }
            KeyCode::Right => {
                let len = self.buffer.line_len(self.cursor.line);
                if self.cursor.col < len {
                    self.cursor.col += 1;
                    self.cursor.want_col = self.cursor.col;
                }
            }
            KeyCode::Up => {
                if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    let len = self.buffer.line_len(self.cursor.line);
                    self.cursor.col = self.cursor.want_col.min(len);
                }
            }
            KeyCode::Down => {
                let last = self.buffer.line_count().saturating_sub(1);
                if self.cursor.line < last {
                    self.cursor.line += 1;
                    let len = self.buffer.line_len(self.cursor.line);
                    self.cursor.col = self.cursor.want_col.min(len);
                }
            }
            KeyCode::Home => {
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            KeyCode::End => {
                let len = self.buffer.line_len(self.cursor.line);
                self.cursor.col = len;
                self.cursor.want_col = len;
            }
            _ => {}
        }
    }

    /// Smart Enter — copies the current line's leading whitespace onto the
    /// new line, adds one indent unit when the previous non-whitespace char
    /// is an opener (`{`/`[`/`(`/`:`/`=>`/`->`), and splits paired
    /// openers/closers (`{|}`) onto three lines so the cursor lands on a
    /// double-indented middle row ready for the body.
    fn handle_insert_newline(&mut self) {
        let line = self.cursor.line;
        let col = self.cursor.col;
        let line_len = self.buffer.line_len(line);
        let line_start = self.buffer.line_start_idx(line);
        let line_text: String = self
            .buffer
            .rope
            .slice(line_start..line_start + line_len)
            .to_string();
        let chars: Vec<char> = line_text.chars().collect();

        let lead: String = chars
            .iter()
            .take_while(|c| matches!(**c, ' ' | '\t'))
            .copied()
            .collect();
        let unit = self.editorconfig.indent_string();

        // What's the last non-whitespace char before the cursor on this line?
        let prev_non_ws = chars[..col.min(chars.len())]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace())
            .copied();
        let prev_two: String = chars[..col.min(chars.len())]
            .iter()
            .rev()
            .take_while(|c| !c.is_whitespace())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        // What's the first non-whitespace char at/after the cursor?
        let next_non_ws = chars.get(col).copied();
        let opener_after = matches!(
            prev_non_ws,
            Some('{') | Some('[') | Some('(') | Some(':')
        ) || prev_two.ends_with("=>")
            || prev_two.ends_with("->");
        let split_pair = matches!(
            (prev_non_ws, next_non_ws),
            (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
        );

        if split_pair {
            // `{|}` → three lines, cursor double-indented in the middle.
            let body_indent = format!("{lead}{unit}");
            let payload = format!("\n{body_indent}\n{lead}");
            self.buffer.insert_str(line, col, &payload);
            self.cursor.line = line + 1;
            self.cursor.col = body_indent.chars().count();
            self.cursor.want_col = self.cursor.col;
            return;
        }

        let next_indent = if opener_after {
            format!("{lead}{unit}")
        } else {
            lead
        };
        let payload = format!("\n{next_indent}");
        self.buffer.insert_str(line, col, &payload);
        self.cursor.line = line + 1;
        self.cursor.col = next_indent.chars().count();
        self.cursor.want_col = self.cursor.col;
    }

    fn handle_insert_key_with_completion(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.completion = None;
                true
            }
            KeyCode::Up => {
                self.completion_cycle(-1);
                true
            }
            KeyCode::Down => {
                self.completion_cycle(1);
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.completion_accept();
                true
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'n' | 'N' => {
                    self.completion_cycle(1);
                    true
                }
                'p' | 'P' => {
                    self.completion_cycle(-1);
                    true
                }
                _ => {
                    self.completion = None;
                    false
                }
            },
            // Typing an identifier/trigger char: keep popup open; the main handler
            // inserts the char and the auto-trigger refreshes the completion list.
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && is_completion_trigger(c) =>
            {
                false
            }
            // Backspace inside the popup: refresh, don't dismiss.
            KeyCode::Backspace => false,
            _ => {
                self.completion = None;
                false
            }
        }
    }

    pub(super) fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cmdline.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                self.mode = Mode::Normal;
                self.exec_command(&line);
            }
            KeyCode::Backspace => {
                if self.cmdline.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.cmdline.pop();
                }
            }
            KeyCode::Char(c) => {
                self.cmdline.push(c);
            }
            _ => {}
        }
    }

    fn exec_command(&mut self, line: &str) {
        match command::parse(line) {
            ExCommand::Write => match self.save_active() {
                Ok(format_note) => {
                    let path = self
                        .buffer
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "[No Name]".into());
                    let lines = self.buffer.line_count();
                    self.status_msg = match format_note {
                        Some(note) => format!("\"{path}\" {lines}L written ({note})"),
                        None => format!("\"{path}\" {lines}L written"),
                    };
                }
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::WriteAs(p) => {
                self.buffer.path = Some(PathBuf::from(p));
                self.refresh_editorconfig();
                if let Err(e) = self.save_active() {
                    self.status_msg = format!("error: {e}");
                }
            }
            ExCommand::Quit => {
                if self.buffer.dirty {
                    self.status_msg = "E37: No write since last change (use :q!)".into();
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::QuitForce => self.should_quit = true,
            ExCommand::WriteQuit => match self.save_active() {
                Ok(_) => self.should_quit = true,
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::Edit(p) => {
                if p.is_empty() {
                    self.status_msg = "E32: No file name".into();
                } else if let Err(e) = self.open_buffer(PathBuf::from(p)) {
                    self.status_msg = format!("error: {e}");
                }
            }
            ExCommand::BufferNext => self.cycle_buffer(1),
            ExCommand::BufferPrev => self.cycle_buffer(-1),
            ExCommand::BufferDelete { force } => {
                if let Err(e) = self.delete_buffer(force) {
                    self.status_msg = format!("error: {e}");
                }
            }
            ExCommand::BufferList => {
                self.status_msg = self.list_buffers();
            }
            ExCommand::BufferSwitch(spec) => {
                if let Err(e) = self.switch_buffer_by_spec(&spec) {
                    self.status_msg = format!("error: {e}");
                }
            }
            ExCommand::Substitute { range, pattern, replacement, global } => {
                self.history.record(&self.buffer.rope, self.cursor);
                let n = self.substitute(range, &pattern, &replacement, global);
                self.status_msg = if n == 0 {
                    format!("Pattern not found: {pattern}")
                } else {
                    format!("{n} substitution{}", if n == 1 { "" } else { "s" })
                };
            }
            ExCommand::ProjectSubstitute { pattern, replacement, global } => {
                self.project_substitute(&pattern, &replacement, global);
            }
            ExCommand::DeleteRange { range } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.delete_lines(range);
            }
            ExCommand::YankRange { range } => {
                self.yank_lines(range);
            }
            ExCommand::NoHighlight => {
                self.search_hl_off = true;
            }
            ExCommand::Format => self.format_active(),
            ExCommand::Health => self.cmd_health(),
            ExCommand::Debug(sub) => self.dispatch_debug(sub),
            ExCommand::Goto(n) => {
                let m = motion::goto_line(&self.buffer, n);
                self.cursor = m.target;
            }
            ExCommand::Unknown(s) => {
                self.status_msg = format!("E492: Not an editor command: {s}");
            }
        }
    }

    pub(super) fn handle_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cancel_prompt();
            }
            KeyCode::Enter => {
                let kind = match self.mode {
                    Mode::Prompt(k) => k,
                    _ => return,
                };
                let input = std::mem::take(&mut self.cmdline);
                match kind {
                    crate::mode::PromptKind::Rename => self.finish_rename(input),
                    crate::mode::PromptKind::ReplaceAll => self.finish_replace_all(input),
                }
                self.mode = Mode::Normal;
                self.rename_anchor = None;
            }
            KeyCode::Backspace => {
                self.cmdline.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cmdline.push(c);
            }
            _ => {}
        }
    }

    pub(super) fn cancel_prompt(&mut self) {
        self.cmdline.clear();
        self.mode = Mode::Normal;
        self.rename_anchor = None;
    }

    /// Resolve an `ExRange` to a 0-based inclusive `(start_line, end_line)` pair,
    /// clamped to the current buffer's bounds.
    fn resolve_range(&self, range: ExRange, default_current: bool) -> (usize, usize) {
        let last = self.buffer.line_count().saturating_sub(1);
        match range {
            ExRange::Implicit => {
                if default_current {
                    (self.cursor.line, self.cursor.line)
                } else {
                    (0, last)
                }
            }
            ExRange::Whole => (0, last),
            ExRange::Single(n) => {
                let line = n.saturating_sub(1).min(last);
                (line, line)
            }
            ExRange::Lines(a, b) => {
                let a = a.saturating_sub(1).min(last);
                let b = b.saturating_sub(1).min(last);
                if a <= b { (a, b) } else { (b, a) }
            }
        }
    }

    pub(super) fn substitute(&mut self, range: ExRange, pat: &str, repl: &str, global: bool) -> usize {
        if pat.is_empty() {
            return 0;
        }
        let (l1, l2) = self.resolve_range(range, true);
        let mut total = 0usize;
        // Iterate bottom-up so edits to lower lines don't shift higher line indices.
        for line in (l1..=l2).rev() {
            let line_len = self.buffer.line_len(line);
            if line_len == 0 {
                continue;
            }
            let line_start = self.buffer.line_start_idx(line);
            let line_text: String = self
                .buffer
                .rope
                .slice(line_start..(line_start + line_len))
                .to_string();
            let (new_text, n) = if global {
                let count = line_text.matches(pat).count();
                (line_text.replace(pat, repl), count)
            } else if line_text.contains(pat) {
                (line_text.replacen(pat, repl, 1), 1)
            } else {
                (line_text.clone(), 0)
            };
            if n > 0 {
                self.buffer.delete_range(line_start, line_start + line_len);
                self.buffer.insert_at_idx(line_start, &new_text);
                total += n;
            }
        }
        if total > 0 {
            self.cursor.line = l1;
            self.cursor.col = 0;
            self.cursor.want_col = 0;
            self.clamp_cursor_normal();
        }
        total
    }

    fn delete_lines(&mut self, range: ExRange) {
        let (l1, l2) = self.resolve_range(range, true);
        let last_line = self.buffer.line_count().saturating_sub(1);
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2 + 1);
        let total = self.buffer.total_chars();
        let extend_back = end == total && l1 > 0;
        let effective_start = if extend_back { start - 1 } else { start };
        let raw = self
            .buffer
            .rope
            .slice(effective_start..end)
            .to_string();
        let reg_text = if extend_back {
            let mut s = raw[1..].to_string();
            if !s.ends_with('\n') {
                s.push('\n');
            }
            s
        } else if !raw.ends_with('\n') {
            let mut s = raw.clone();
            s.push('\n');
            s
        } else {
            raw
        };
        self.write_register(None, reg_text, true);
        self.buffer.delete_range(effective_start, end);
        let new_last = self.buffer.line_count().saturating_sub(1);
        self.cursor.line = l1.min(new_last);
        self.cursor.col = 0;
        self.cursor.want_col = 0;
        self.status_msg = format!("{} lines deleted", l2 - l1 + 1);
        let _ = last_line;
    }

    /// Project-wide substitute. ripgrep enumerates the files that contain
    /// `pattern`, then we walk each, open it into a buffer, apply the
    /// substitution across every line, and save. The originally-active
    /// buffer is restored at the end so the user lands back where they
    /// were. No confirmation prompt — the user has git for safety.
    fn project_substitute(&mut self, pattern: &str, replacement: &str, global: bool) {
        if pattern.is_empty() {
            self.status_msg = "S: empty pattern".into();
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // Use ripgrep's --files-with-matches to get the candidate file list.
        let files_output = std::process::Command::new("rg")
            .arg("--files-with-matches")
            .arg("--color=never")
            .arg("--fixed-strings")
            .arg("--")
            .arg(pattern)
            .arg(".")
            .current_dir(&cwd)
            .output();
        let Ok(out) = files_output else {
            self.status_msg = "S: ripgrep not on PATH".into();
            return;
        };
        if !out.status.success() && out.stdout.is_empty() {
            self.status_msg = format!("S: pattern not found: {pattern}");
            return;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let files: Vec<PathBuf> = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| cwd.join(l))
            .collect();
        if files.is_empty() {
            self.status_msg = format!("S: pattern not found: {pattern}");
            return;
        }
        let original_active = self.active;
        let mut total_subs = 0usize;
        let mut files_changed = 0usize;
        let mut errors = 0usize;
        for path in files {
            if self.open_buffer(path.clone()).is_err() {
                errors += 1;
                continue;
            }
            self.history.record(&self.buffer.rope, self.cursor);
            let n = self.substitute(crate::command::ExRange::Whole, pattern, replacement, global);
            if n > 0 {
                total_subs += n;
                files_changed += 1;
                if self.save_active().is_err() {
                    errors += 1;
                }
            }
        }
        if original_active < self.buffers.len() && self.active != original_active {
            let _ = self.switch_to(original_active);
        }
        self.status_msg = if total_subs == 0 {
            format!("S: pattern not found: {pattern}")
        } else {
            format!(
                "{total_subs} substitution{} across {files_changed} file{}{}",
                if total_subs == 1 { "" } else { "s" },
                if files_changed == 1 { "" } else { "s" },
                if errors > 0 {
                    format!(" ({errors} error{})", if errors == 1 { "" } else { "s" })
                } else {
                    String::new()
                },
            )
        };
    }

    fn yank_lines(&mut self, range: ExRange) {
        let (l1, l2) = self.resolve_range(range, true);
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2 + 1);
        let raw = self.buffer.rope.slice(start..end).to_string();
        let reg_text = if !raw.ends_with('\n') {
            let mut s = raw.clone();
            s.push('\n');
            s
        } else {
            raw
        };
        self.write_yank_register(None, reg_text, true);
        self.flash_yank(start, end);
        self.status_msg = format!("{} lines yanked", l2 - l1 + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use ropey::Rope;

    fn buf(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            path: None,
            dirty: false,
            version: 0,
            disk_mtime: None,
            display_name: None,
        }
    }

    #[test]
    fn visual_col_to_char_col_no_tabs() {
        // `hello world` — visual col == char col on plain ASCII.
        let b = buf("hello world\n");
        assert_eq!(visual_col_to_char_col(&b, 0, 0, 11), 0);
        assert_eq!(visual_col_to_char_col(&b, 0, 6, 11), 6);
        assert_eq!(visual_col_to_char_col(&b, 0, 10, 11), 10);
        // Click past EOL clamps to the last char.
        assert_eq!(visual_col_to_char_col(&b, 0, 30, 11), 10);
    }

    #[test]
    fn visual_col_to_char_col_with_tabs() {
        // "\t\tx" — two tabs (4 visual cols each) then `x`. Char positions:
        //   0 = first tab, 1 = second tab, 2 = 'x'. Visual positions:
        //   0..4 = first tab, 4..8 = second tab, 8 = 'x'.
        let b = buf("\t\tx\n");
        assert_eq!(visual_col_to_char_col(&b, 0, 0, 3), 0);
        assert_eq!(visual_col_to_char_col(&b, 0, 2, 3), 0); // mid first tab
        assert_eq!(visual_col_to_char_col(&b, 0, 4, 3), 1); // start of second tab
        assert_eq!(visual_col_to_char_col(&b, 0, 6, 3), 1); // mid second tab
        assert_eq!(visual_col_to_char_col(&b, 0, 8, 3), 2); // on `x`
        // The original bug: clicking at visual col 8 used to land at char 8
        // (past EOL). Now it clamps to the last char (`x`).
        assert_eq!(visual_col_to_char_col(&b, 0, 30, 3), 2);
    }

    #[test]
    fn visual_col_to_char_col_mixed_tabs_then_text() {
        // "\t\t<partial …" — clicking on `<` after two tabs should yield
        // char col 2 (the `<`), not 8 (which would be deep inside the word).
        let line = "\t\t<partial";
        let b = buf(&format!("{}\n", line));
        let line_len = line.chars().count();
        assert_eq!(visual_col_to_char_col(&b, 0, 8, line_len), 2);
        // Click 3 cells in (between the two tabs visually) clamps to char 0.
        assert_eq!(visual_col_to_char_col(&b, 0, 3, line_len), 0);
    }

    #[test]
    fn visual_col_to_char_col_empty_line() {
        let b = buf("\n");
        assert_eq!(visual_col_to_char_col(&b, 0, 0, 0), 0);
        assert_eq!(visual_col_to_char_col(&b, 0, 99, 0), 0);
    }
}
