//! Per-mode key handlers, the mouse handler, and the `:`-command dispatch
//! that converts an `ExCommand` back into App mutations. Search input
//! lives in `search.rs` (it shares state with the search machinery), and
//! the LSP-related rename prompt lives in `lsp_glue.rs`.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::path::PathBuf;

use crate::command::{self, ExCommand, ExRange};
use crate::keymap::{KeymapMatch, MapMode};
use crate::mode::{Mode, VisualKind};
use crate::motion;
use crate::parser::{self, ParseCtx, ParseResult};

use super::pair::{
    detect_open_tag_to_close, is_close_char, is_html_like_buffer, open_pair_for, should_auto_pair,
};
use super::state::{self, LastEdit, WhichKeyState};

/// Characters that should re-fire `textDocument/completion` after being inserted.
/// Identifier chars catch the typing-a-name case; the symbol set covers the
/// trigger characters servers care about most: member access (`.`), Rust paths
/// and Tailwind variants (`:`), Razor/decorator anchors (`@`), JSX/HTML opens
/// (`<`), CSS property/utility separators (`-`), and Emmet abbreviation
/// anchors (`!` for the HTML5 boilerplate, `#` for id shorthand).
pub(super) fn is_completion_trigger(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '<' | '-' | '!' | '#')
}

/// Reverse of the renderer's display walk — given a visual column on
/// `line`, return the buffer char column that visual position sits in.
/// Tabs widen by `TAB_WIDTH`; inlay hints anchored at a buffer col take
/// their full label width *before* the char at that col, so clicking
/// inside a hint snaps to the buffer position immediately after it.
/// Click past end-of-line clamps to `line_len.saturating_sub(1)`
/// (matches Vim's "cursor sits on a char, not past it" Normal-mode rule).
fn visual_col_to_char_col(
    app: &super::App,
    line: usize,
    visual_col: usize,
    line_len: usize,
) -> usize {
    // Insert mode lets the cursor sit past the last char (col == line_len);
    // Normal / Visual clamp to `line_len - 1` so the cursor always sits on a
    // character. Same rule for clicks: an Insert-mode click past EOL parks
    // the cursor at end-of-line; a Normal-mode click past EOL snaps it to
    // the last char.
    let allow_past_eol = matches!(app.mode, crate::mode::Mode::Insert);
    // Markdown concealed mode warps source spans (`**` hidden, `- `
    // replaced by `•`, …), so the click → buffer-col mapping has to
    // walk the same per-line transforms the renderer used. Without
    // this, clicking on rendered "bold" text would land at whatever
    // raw-source position happened to share the visual column.
    if app.markdown_render_active() {
        if let Some(meta) = app.markdown_line_meta(line) {
            let chars: Vec<char> = app
                .buffer
                .rope
                .line(line)
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let col = crate::markdown_render::buffer_col_for_visual_col(
                &chars,
                meta,
                visual_col,
                crate::render::TAB_WIDTH,
            );
            let cap = if allow_past_eol {
                line_len
            } else {
                line_len.saturating_sub(1)
            };
            return col.min(cap);
        }
    }
    let hint_widths = crate::render::inlay_hint_widths_for_line(app, line);
    visual_col_to_char_col_with_hints(
        &app.buffer,
        line,
        visual_col,
        line_len,
        &hint_widths,
        allow_past_eol,
    )
}

/// Inner walk — extracted so tests can drive it without spinning up a
/// full `App`. `hint_widths[i]` is the total cell width of inlay hints
/// anchored at buffer col `i` on the line; empty slice = no hints.
/// `allow_past_eol` lifts the trailing clamp from `line_len - 1` (the
/// Normal-mode "cursor sits on a char" rule) to `line_len` (the
/// Insert-mode "cursor can sit past the last char" rule).
fn visual_col_to_char_col_with_hints(
    buffer: &crate::buffer::Buffer,
    line: usize,
    visual_col: usize,
    line_len: usize,
    hint_widths: &[usize],
    allow_past_eol: bool,
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
        let hw = hint_widths.get(chars).copied().unwrap_or(0);
        if hw > 0 {
            if visual + hw > visual_col {
                return chars;
            }
            visual += hw;
        }
        // Mirror of the renderer's advance: tabs expand to TAB_WIDTH cells, CJK /
        // wide glyphs are two cells. Matching the render walk is what makes a
        // click land on the character the user is actually pointing at.
        let w = crate::render::char_width(c, crate::render::TAB_WIDTH);
        if visual >= visual_col {
            break;
        }
        if visual + w > visual_col {
            return chars;
        }
        visual += w;
        chars += 1;
    }
    if allow_past_eol {
        chars.min(line_len)
    } else {
        chars.min(line_len - 1)
    }
}

/// Find the char column to delete back to for an Alt/Ctrl-Backspace or `Ctrl-W` on
/// `line` from cursor column `col`. Matches the macOS Option-Delete
/// convention: first eat any whitespace immediately before the cursor,
/// then eat one contiguous run of word chars (alphanumeric + `_`) or one
/// run of non-word, non-whitespace punctuation. Returns the column the
/// cursor should land on after the delete (`0` for "back to line start").
fn previous_word_boundary(buffer: &crate::buffer::Buffer, line: usize, col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    let slice = buffer.rope.line(line);
    let chars: Vec<char> = slice.chars().take_while(|c| *c != '\n').collect();
    let mut i = col.min(chars.len());
    // Step 1: skip trailing whitespace.
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    // Step 2: peel one homogeneous run — word chars OR punctuation.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let last_is_word = is_word_char(chars[i - 1]);
    while i > 0 {
        let c = chars[i - 1];
        if c.is_whitespace() {
            break;
        }
        if is_word_char(c) != last_is_word {
            break;
        }
        i -= 1;
    }
    i
}

/// Result of feeding one key to an Insert-mode `Ctrl-V` sequence.
struct LiteralStep {
    /// Char to insert now, if the sequence produced one.
    insert: Option<char>,
    /// State to keep waiting in; `None` ends the sequence.
    next: Option<super::state::LiteralPending>,
    /// The key ended a code without being part of it, so it still gets its
    /// normal Insert-mode handling — Vim's `Ctrl-V u41z` inserts `A`, then `z`.
    reprocess: bool,
}

/// Vim's `Ctrl-V` grammar: the next key literally, or `u` + up to 4 hex
/// digits, `U` + 8, `x` + 2, `o` + 3 octal, or up to 3 decimal digits.
fn literal_step(pending: &super::state::LiteralPending, key: KeyEvent) -> LiteralStep {
    use super::state::LiteralPending;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let done = |insert| LiteralStep {
        insert,
        next: None,
        reprocess: false,
    };
    let collect = |prefix, radix, max, digits| LiteralStep {
        insert: None,
        next: Some(LiteralPending::Code {
            prefix,
            radix,
            max,
            digits,
        }),
        reprocess: false,
    };
    match pending {
        LiteralPending::Key => match key.code {
            KeyCode::Tab => done(Some('\t')),
            KeyCode::Char(c) if !ctrl => match c {
                'u' => collect(c, 16, 4, String::new()),
                'U' => collect(c, 16, 8, String::new()),
                'x' | 'X' => collect(c, 16, 2, String::new()),
                'o' | 'O' => collect(c, 8, 3, String::new()),
                '0'..='9' => collect(c, 10, 3, c.to_string()),
                _ => done(Some(c)),
            },
            _ => done(None),
        },
        LiteralPending::Code {
            prefix,
            radix,
            max,
            digits,
        } => {
            let value = |d: &str| u32::from_str_radix(d, *radix).ok().and_then(char::from_u32);
            match key.code {
                KeyCode::Char(d) if !ctrl && d.is_digit(*radix) => {
                    let mut more = digits.clone();
                    more.push(d);
                    // Decimal codes stop at 255, as in Vim: `Ctrl-V 300` is
                    // code 30 followed by a typed `0`.
                    if *radix == 10 && matches!(more.parse::<u32>(), Ok(v) if v > 255) {
                        return LiteralStep {
                            insert: value(digits),
                            next: None,
                            reprocess: true,
                        };
                    }
                    if more.len() == *max {
                        return done(value(&more));
                    }
                    collect(*prefix, *radix, *max, more)
                }
                _ => {
                    let insert = if digits.is_empty() {
                        Some(*prefix)
                    } else {
                        value(digits)
                    };
                    LiteralStep {
                        insert,
                        next: None,
                        reprocess: true,
                    }
                }
            }
        }
    }
}

/// Should an Enter pressed at the cursor split a paired opener / closer
/// onto three lines with an indented middle row?
///
/// Triggered by `{|}` / `[|]` / `(|)`, and by `<tag>|</tag>` (cursor
/// between an open-tag close `>` and a `</…>` close tag). HTML / JSX /
/// TSX / Razor / Vue / Svelte / Astro / XML all benefit from this since
/// the auto-pair inserter already lands the cursor in that exact spot
/// when the user types `>` after a tag name. Anchored on the full `</`
/// (not bare `<`) so generic-type / comparison usages like `Foo<Bar>`
/// don't false-positive.
fn should_split_pair_on_enter(
    prev_non_ws: Option<char>,
    next_non_ws: Option<char>,
    next_next: Option<char>,
) -> bool {
    if matches!(
        (prev_non_ws, next_non_ws),
        (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
    ) {
        return true;
    }
    prev_non_ws == Some('>') && next_non_ws == Some('<') && next_next == Some('/')
}

/// Temporary paste diagnostics — appends one line to the file named in
/// `$BINVIM_PASTE_DEBUG` when that env var is set, otherwise a no-op
/// (the closure isn't even evaluated). Used to see whether a Cmd-V
/// arrives as `Event::Paste` or a keystroke flood, and whether the
/// embedded PTY had bracketed paste enabled at the time.
fn paste_dbg(msg: impl FnOnce() -> String) {
    let Ok(path) = std::env::var("BINVIM_PASTE_DEBUG") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", msg());
    }
}

impl super::App {
    pub(super) fn handle_event(&mut self) -> anyhow::Result<()> {
        let ev = crossterm::event::read()?;
        paste_dbg(|| match &ev {
            crossterm::event::Event::Paste(t) => format!("Paste({} chars): {:?}", t.len(), t),
            crossterm::event::Event::Key(k) => format!("Key {:?} mods={:?}", k.code, k.modifiers),
            other => format!("{other:?}"),
        });
        match ev {
            crossterm::event::Event::Key(k)
                if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                if !matches!(self.mode, Mode::Command) {
                    self.status_msg.clear();
                    self.status_msg_at = None;
                }
                // Hover popup intercepts scroll keys; everything else dismisses it.
                if self.hover.is_some() && self.try_scroll_hover(&k) {
                    return Ok(());
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
                let leader_pending = self.pending.any_leader_pending();
                // A mapped key is judged by what it expands to: `H = "^"`
                // stops `H` cycling buffers here, and a key mapped onto `:`
                // or the leader gets through the way those keys do. Keys
                // continuing a held `[keymaps]` sequence pass like a pending
                // leader chord's.
                let lead = self
                    .pending
                    .accepts_mapping()
                    .then(|| self.config.keymaps.exact(MapMode::Normal, &[k]))
                    .flatten()
                    .and_then(|rhs| rhs.first().copied())
                    .unwrap_or(k);
                if self.show_start_page
                    && matches!(self.mode, Mode::Normal)
                    && !leader_pending
                    && self.keymap_held.is_empty()
                    && !super::state::is_start_page_passthrough(&lead)
                {
                    // A restored session parks its tabs behind the start
                    // page (hydrate_from_session re-raises the logo after
                    // loading them). When tabs are waiting like that, any
                    // key dismisses the logo and drops the user onto the
                    // first tab. With no tabs (fresh launch, single empty
                    // buffer) the key is swallowed so the page stays put.
                    if self.show_tabs() {
                        self.show_start_page = false;
                        if let Some(idx) = self.first_real_tab() {
                            let _ = self.switch_to(idx);
                        }
                    }
                    return Ok(());
                }
                // While the health dashboard, messages overlay, or
                // test-results overlay is up, only Esc / `q` (in Normal
                // mode) / `:` to enter the cmdline pass through. `:q`
                // then dismisses via the ExCommand::Quit handler above.
                // Other keys are swallowed so the user can't
                // accidentally type into the underlying buffer. The
                // three overlays share the same scroll bindings — only
                // the dismiss flag differs.
                let overlay_active = self.show_health_page
                    || self.show_messages_page
                    || self.show_test_results_page
                    || self.show_list_page;
                if overlay_active {
                    let normal = matches!(self.mode, Mode::Normal);
                    let no_ctrl = !k.modifiers.contains(KeyModifiers::CONTROL);
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    let messages = self.show_messages_page;
                    let test_results = self.show_test_results_page;
                    let registers = self.show_list_page;
                    let scroll = |this: &mut Self, delta: isize| {
                        if test_results {
                            this.test_results_scroll_by(delta);
                        } else if messages {
                            this.messages_scroll_by(delta);
                        } else if registers {
                            this.list_scroll_by(delta);
                        } else {
                            this.health_scroll_by(delta);
                        }
                    };
                    let dismiss = |this: &mut Self| {
                        if test_results {
                            this.show_test_results_page = false;
                        } else if messages {
                            this.show_messages_page = false;
                        } else if registers {
                            this.show_list_page = false;
                        } else {
                            this.show_health_page = false;
                        }
                    };
                    match k.code {
                        KeyCode::Esc => {
                            dismiss(self);
                            return Ok(());
                        }
                        KeyCode::Char('q') if normal && no_ctrl => {
                            dismiss(self);
                            return Ok(());
                        }
                        // Scroll the overlay. j/k by one row, Ctrl-D/U
                        // by half a page, PgDn/PgUp by a full page, g/G
                        // to jump to top / bottom.
                        KeyCode::Char('j') | KeyCode::Down if normal && no_ctrl => {
                            scroll(self, 1);
                            return Ok(());
                        }
                        KeyCode::Char('k') | KeyCode::Up if normal && no_ctrl => {
                            scroll(self, -1);
                            return Ok(());
                        }
                        KeyCode::Char('d') if normal && ctrl => {
                            let step = (self.buffer_rows() / 2).max(1) as isize;
                            scroll(self, step);
                            return Ok(());
                        }
                        KeyCode::Char('u') if normal && ctrl => {
                            let step = (self.buffer_rows() / 2).max(1) as isize;
                            scroll(self, -step);
                            return Ok(());
                        }
                        KeyCode::Char('f') if normal && ctrl => {
                            let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                            scroll(self, step);
                            return Ok(());
                        }
                        KeyCode::Char('b') if normal && ctrl => {
                            let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                            scroll(self, -step);
                            return Ok(());
                        }
                        KeyCode::PageDown if normal => {
                            let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                            scroll(self, step);
                            return Ok(());
                        }
                        KeyCode::PageUp if normal => {
                            let step = self.buffer_rows().saturating_sub(1).max(1) as isize;
                            scroll(self, -step);
                            return Ok(());
                        }
                        KeyCode::Char('g') | KeyCode::Home if normal && no_ctrl => {
                            if test_results {
                                // Jump to the top — explicit "look at
                                // scrollback", so leave tail mode too.
                                self.test_results_at_tail = false;
                                self.test_results_scroll = 0;
                            } else if messages {
                                self.messages_scroll = 0;
                            } else if registers {
                                self.list_scroll = 0;
                            } else {
                                self.health_scroll = 0;
                            }
                            return Ok(());
                        }
                        KeyCode::Char('G') | KeyCode::End if normal => {
                            if test_results {
                                // Re-engage tail follow rather than
                                // freezing at the current bottom —
                                // matches `tail -f` semantics.
                                self.test_results_at_tail = true;
                            } else if messages {
                                self.messages_scroll = self.messages_max_scroll();
                            } else if registers {
                                self.list_scroll = self.list_max_scroll();
                            } else {
                                self.health_scroll = self.health_max_scroll();
                            }
                            return Ok(());
                        }
                        KeyCode::Char(':') if normal => {
                            // Fall through to the normal cmdline-entry path.
                        }
                        _ if matches!(self.mode, Mode::Command) => {
                            // Cmdline is in flight — let it handle keys (incl. `:q` Enter).
                        }
                        _ => return Ok(()),
                    }
                }
                let start = self.key_start();
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
                    Mode::Terminal => self.handle_terminal_key(k),
                    Mode::FileTree => self.handle_file_tree_key(k),
                    Mode::RenamePreview => self.handle_rename_preview_key(k),
                    Mode::Installer => self.handle_installer_key(k),
                }
                self.after_key(start);
            }
            crossterm::event::Event::Paste(text) => {
                self.handle_paste(text);
            }
            crossterm::event::Event::Mouse(me) => {
                self.handle_mouse_event(me);
            }
            crossterm::event::Event::Resize(w, h) => {
                self.width = w;
                self.height = h;
                // Propagate to every embedded PTY so each child
                // gets a SIGWINCH at the new size. Background tabs
                // need this too — when the user switches to a tab
                // that's been hidden behind another for a while,
                // its shell should already have the current
                // winsize so we don't see a reflow flash on switch.
                self.resize_all_terminals();
                self.resize_all_side_terminals();
            }
            _ => {}
        }
        Ok(())
    }

    /// What `after_key` needs to see what a key changed.
    pub(super) fn key_start(&self) -> state::KeyStart {
        let selection = match self.mode {
            Mode::Visual(_) => self.window.visual_anchor.map(|a| (a, self.window.cursor)),
            _ => None,
        };
        state::KeyStart {
            mode: self.mode,
            cursor: self.window.cursor,
            selection,
        }
    }

    /// Bookkeeping after every key, whichever mode handled it: closes the
    /// change `'[` / `']` follow unless Insert is still typing into it, sets
    /// `'<` / `'>` when a key leaves Visual and `^` when one leaves Insert, and
    /// brings a finished `Ctrl-O` command back to Insert.
    pub(super) fn after_key(&mut self, start: state::KeyStart) {
        if self.mode != Mode::Insert {
            self.buffer.close_change();
        }
        self.refresh_file_marks();
        let left_visual = start
            .selection
            .filter(|_| !matches!(self.mode, Mode::Visual(_)));
        if let (Some((anchor, cursor)), Mode::Visual(kind)) = (left_visual, start.mode) {
            self.remember_visual(kind, anchor, cursor);
        }
        if start.mode == Mode::Insert && self.mode != Mode::Insert {
            self.buffer
                .set_mark('^', start.cursor.line, start.cursor.col);
        }
        self.insert_oneshot_after(start.mode);
        // Replace mode is Insert with a session; leaving Insert ends it, unless
        // a `Ctrl-O` command is on its way back.
        if self.mode != Mode::Insert && self.insert_oneshot.is_none() {
            self.replace_session = None;
        }
    }

    /// After a key routed from any mode but Insert: once an Insert `Ctrl-O`
    /// command has finished, go back to Insert. A command still in flight —
    /// an operator waiting for its motion, a `:` line, a Visual selection —
    /// keeps it waiting; one that enters Insert itself or opens anything else
    /// ends the one-shot there. `.` replays whole commands, so it's skipped.
    pub(super) fn insert_oneshot_after(&mut self, was: Mode) {
        if was == Mode::Insert || self.replaying {
            return;
        }
        let Some(shot) = self.insert_oneshot else {
            return;
        };
        match self.mode {
            Mode::Normal if self.pending.is_clean() && self.keymap_held.is_empty() => {}
            Mode::Normal | Mode::Command | Mode::Search { .. } | Mode::Visual(_) => return,
            _ => {
                self.insert_oneshot = None;
                return;
            }
        }
        self.insert_oneshot = None;
        let cursor = &mut self.window.cursor;
        let line_len = self.buffer.line_len(cursor.line);
        // Past the end again if the command left the cursor where `Ctrl-O`
        // stepped it back to, or went there with `$`.
        let on_last = line_len > 0 && cursor.col + 1 == line_len;
        let left_there = shot.stepped_back == Some((cursor.line, cursor.col));
        if on_last && (left_there || cursor.want_col == usize::MAX) {
            cursor.col = line_len;
        }
        self.mode = Mode::Insert;
        // Replace mode's session outlives the command, so `.` replays an `R`.
        let prelude = if self.replace_session.is_some() {
            parser::Action::EnterReplace { count: 1 }
        } else {
            parser::Action::EnterInsert(parser::InsertWhere::Cursor)
        };
        self.recording = Some(state::RecordingState {
            prelude,
            keys: Vec::new(),
            resumed: true,
        });
    }

    /// Closes the Insert session's recording into `last_edit` for `.` — but
    /// not one `Ctrl-O` resumed that got nothing typed, which would take `.`
    /// away from the command.
    fn end_insert_recording(&mut self) {
        if self.replaying {
            return;
        }
        let Some(rec) = self.recording.take() else {
            return;
        };
        if rec.resumed && rec.keys.is_empty() {
            return;
        }
        self.last_edit = Some(LastEdit::InsertSession {
            prelude: rec.prelude,
            keys: rec.keys,
        });
    }

    /// A host paste (Cmd-V / middle-click) arrived as one atomic blob
    /// because `EnableBracketedPaste` is active. Route it by mode rather
    /// than replaying it as keystrokes — that's the whole point: a
    /// multi-line paste stays one unit instead of typing out char by
    /// char (and, in the AI panes, firing one message per newline).
    pub(super) fn handle_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        match self.mode {
            // Terminal panes (`:claude` / `:codex` / `:opencode` and the
            // bottom `:terminal`) get the blob handed to the focused PTY.
            // `write_paste` re-wraps it in bracketed-paste markers when the
            // embedded program asked for them, so the tool sees one paste.
            Mode::Terminal => {
                let term = match self.terminal_focus {
                    crate::app::TerminalFocus::Side => self
                        .active_side_terminal()
                        .or_else(|| self.active_terminal()),
                    crate::app::TerminalFocus::Bottom => self.active_terminal(),
                };
                if let Some(t) = term {
                    paste_dbg(|| {
                        format!("forward to PTY, bracketed={}", t.bracketed_paste_enabled())
                    });
                    t.snap_view_to_live();
                    let _ = t.write_paste(&text);
                }
            }
            // Insert mode: splice the literal text in at the cursor as a
            // single undo step, bypassing the autopair / snippet dance so
            // braces and quotes in the pasted source aren't doubled.
            Mode::Insert => {
                self.copilot_invalidate_ghost();
                self.history.record(&self.buffer.rope, self.window.cursor);
                let idx = self
                    .buffer
                    .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                self.buffer.insert_at_idx(idx, &text);
                self.cursor_to_idx(idx + text.chars().count());
            }
            // Command / search / prompt lines are single-line — drop any
            // interior newlines so a pasted path or pattern doesn't smear
            // across the (nonexistent) next line or submit early.
            Mode::Command | Mode::Search { .. } | Mode::Prompt(_) => {
                for c in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.cmdline_insert_char_at_cursor(c);
                }
            }
            // Picker query is likewise single-line.
            Mode::Picker => {
                if let Some(picker) = self.picker.as_mut() {
                    for c in text.chars().filter(|c| *c != '\n' && *c != '\r') {
                        picker.input.push(c);
                    }
                    self.refilter_picker();
                }
            }
            // Normal / Visual / and the read-only overlays have no
            // meaningful "insert pasted text here" target — ignore rather
            // than guess. The user's own yank registers (`p`) cover
            // buffer pastes in Normal mode.
            _ => {}
        }
    }

    fn handle_mouse_event(&mut self, ev: MouseEvent) {
        // Don't process mouse events while an overlay is up — picker/cmdline/etc
        // expect keyboard interaction. Scroll wheel still works to dismiss them.
        let in_overlay = self.has_modal_overlay();
        let row = ev.row as usize;
        let col = ev.column as usize;
        let buffer_rows = self.buffer_rows();

        // Mouse events that land inside the terminal pane belong to
        // the embedded shell — either forwarded as escape sequences
        // (when the program has enabled DECSET mouse tracking) or
        // turned into a focus-the-terminal click. Return early so
        // the editor's mouse handling doesn't also fire.
        if self.terminal_pane_open && self.handle_terminal_mouse_event(&ev, row, col) {
            return;
        }
        // Side-pane (`:claude` etc.) mouse handling. Header row
        // clicks switch tabs via the hit-box table the renderer
        // populated; clicks anywhere else in the pane pull focus to
        // the side terminal AND (when the embedded program has DECSET
        // mouse tracking enabled) forward the event to the PTY as an
        // xterm mouse escape sequence so scroll / drag / click inside
        // claude / codex / opencode reach the tool. Without the
        // forwarding path scroll-wheel events fell into the swallow
        // arm and never reached the tool.
        if self.side_terminal_pane_open
            && self.side_pane_cols() > 0
            && col >= self.side_pane_left()
            && row >= self.buffer_top()
            && row < self.buffer_top() + self.buffer_rows()
        {
            // Header row = the first row of the pane. Same row the
            // tab strip sits on.
            let header_row = self.buffer_top();
            if row == header_row
                && matches!(
                    ev.kind,
                    MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
                )
            {
                let hits = self.side_terminal_tab_hitboxes.take();
                let mut clicked: Option<usize> = None;
                for (idx, x_start, x_end) in &hits {
                    if (col as u16) >= *x_start && (col as u16) < *x_end {
                        clicked = Some(*idx);
                        break;
                    }
                }
                self.side_terminal_tab_hitboxes.set(hits);
                if let Some(idx) = clicked {
                    if idx < self.side_terminals.len() && idx != self.active_side_terminal_idx {
                        self.active_side_terminal_idx = idx;
                        // Drag selection is scoped to a single tab —
                        // dropping it on tab switch keeps the highlight
                        // from leaking into a grid the user didn't drag
                        // across.
                        self.side_terminal_selection = None;
                    }
                }
                self.terminal_focus = crate::app::TerminalFocus::Side;
                self.mode = Mode::Terminal;
                return;
            }
            // Body: handle pane-scoped mouse-drag selection first
            // (binvim's own, not forwarded to the PTY) — the host
            // terminal's Shift+drag selects the whole window and has
            // no awareness of where the side pane ends, so the user
            // needs an inside-pane gesture to grab just the embedded
            // tool's output. Plain left-drag (no modifier) is the
            // selection gesture; it's intercepted before the PTY
            // forward arm below. Click (`Down` → `Up` at same pos)
            // still reaches the PTY so AI tools' clickable buttons
            // keep working. Coords below are 0-based grid-local for
            // selection storage; the xterm forward uses 1-based.
            let content_left = self.side_pane_content_left();
            let body_top = header_row + 1;
            if row >= body_top && col >= content_left {
                let active_idx = self.active_side_terminal_idx;
                let (grid_row, grid_col, grid_rows, grid_cols) = {
                    let mut grow = row - body_top;
                    let mut gcol = col - content_left;
                    let (gr, gc) = if let Some(term) = self.active_side_terminal() {
                        let inner = term.grid();
                        let g = &inner.handler.grid;
                        (g.rows, g.cols)
                    } else {
                        (0, 0)
                    };
                    if gr > 0 {
                        grow = grow.min(gr - 1);
                    }
                    if gc > 0 {
                        gcol = gcol.min(gc - 1);
                    }
                    (grow, gcol, gr, gc)
                };
                let _ = (grid_rows, grid_cols);
                match ev.kind {
                    // Mouse wheel for a program that does NOT track the
                    // mouse. opencode enables DECSET mouse tracking, so
                    // its wheel events fall through the guard to the
                    // xterm-forward arm below and it scrolls its own
                    // view. claude / codex don't — they render on the
                    // normal screen via in-place repaint, with their
                    // committed history scrolling into binvim's own
                    // grid scrollback. There's nothing downstream to
                    // forward to, so we page that scrollback ourselves
                    // — the same fallback the bottom `:terminal` pane
                    // already has, which the side pane was missing. A
                    // hypothetical alt-screen pager without tracking
                    // gets the wheel translated to arrow keys (xterm
                    // "alternate scroll") since alt-screen has no
                    // scrollback to page.
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        if self
                            .active_side_terminal()
                            .map(|t| !t.mouse_state().any)
                            .unwrap_or(false) =>
                    {
                        let up = matches!(ev.kind, MouseEventKind::ScrollUp);
                        if let Some(term) = self.active_side_terminal() {
                            if term.alt_screen_active() {
                                let seq: &[u8] = if up {
                                    b"\x1b[A\x1b[A\x1b[A"
                                } else {
                                    b"\x1b[B\x1b[B\x1b[B"
                                };
                                let _ = term.write_bytes(seq);
                            } else {
                                term.scroll_view_by(if up { 3 } else { -3 });
                            }
                        }
                        return;
                    }
                    MouseEventKind::Drag(MouseButton::Left)
                        if !ev.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        // Start a fresh selection on the first drag
                        // sample, or extend an in-flight one. After a
                        // double-click the drag grows word-by-word.
                        // Don't forward to the PTY — drag is "ours".
                        if let Some(origin) = self.side_click.word_drag {
                            let dword = self.side_word_at(grid_row, grid_col);
                            let (lo, hi) =
                                crate::app::word_drag_span(origin, grid_row, grid_col, dword);
                            self.side_terminal_selection = Some(crate::app::SideSelection {
                                tab_idx: active_idx,
                                anchor: lo,
                                head: hi,
                                dragging: true,
                            });
                        } else {
                            let sel = self.side_terminal_selection.take();
                            let new_sel = match sel {
                                Some(mut s) if s.tab_idx == active_idx => {
                                    s.head = (grid_row, grid_col);
                                    s.dragging = true;
                                    s
                                }
                                _ => crate::app::SideSelection {
                                    tab_idx: active_idx,
                                    anchor: (grid_row, grid_col),
                                    head: (grid_row, grid_col),
                                    dragging: true,
                                },
                            };
                            self.side_terminal_selection = Some(new_sel);
                        }
                        if !matches!(self.mode, Mode::Terminal) {
                            self.mode = Mode::Terminal;
                            self.terminal_focus = crate::app::TerminalFocus::Side;
                        }
                        return;
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        // If a drag selection just ended, copy the
                        // covered cells to the system clipboard and
                        // hold the highlight until the next click.
                        if let Some(mut s) = self.side_terminal_selection.take() {
                            if s.tab_idx == active_idx && s.dragging {
                                s.dragging = false;
                                let copied = if let Some(term) = self.active_side_terminal() {
                                    let inner = term.grid();
                                    // `visible_row`-based so a selection
                                    // made while scrolled back copies the
                                    // history the user actually sees.
                                    crate::app::extract_visible_selection_text(
                                        &inner.handler.grid,
                                        &s,
                                    )
                                } else {
                                    String::new()
                                };
                                self.side_terminal_selection = Some(s);
                                if !copied.is_empty() {
                                    super::registers::set_system_clipboard(
                                        &copied,
                                        self.config.clipboard.osc52,
                                    );
                                    let n = copied.chars().count();
                                    self.status_msg = format!("ai: copied {n} chars");
                                }
                                return;
                            }
                            // Wasn't our drag — put it back so the
                            // next render still sees it.
                            self.side_terminal_selection = Some(s);
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        // Double-click selects the word under the cursor
                        // (and copies it), arming word-granular drag —
                        // it's "ours", so it returns without forwarding.
                        // A single click drops any held-over selection
                        // and falls through to the PTY so the embedded
                        // tool's clickable buttons keep working.
                        let now = std::time::Instant::now();
                        let is_double = self
                            .side_click
                            .last
                            .filter(|(t, r, c)| {
                                now.duration_since(*t) <= crate::app::DOUBLE_CLICK_WINDOW
                                    && *r == grid_row
                                    && *c == grid_col
                            })
                            .is_some();
                        if is_double {
                            if let Some((s, e)) = self.side_word_at(grid_row, grid_col) {
                                let sel = crate::app::SideSelection {
                                    tab_idx: active_idx,
                                    anchor: (grid_row, s),
                                    head: (grid_row, e.saturating_sub(1).max(s)),
                                    dragging: false,
                                };
                                let copied = if let Some(term) = self.active_side_terminal() {
                                    let inner = term.grid();
                                    crate::app::extract_visible_selection_text(
                                        &inner.handler.grid,
                                        &sel,
                                    )
                                } else {
                                    String::new()
                                };
                                self.side_terminal_selection = Some(sel);
                                self.side_click.word_drag = Some((grid_row, s, e));
                                if !copied.is_empty() {
                                    super::registers::set_system_clipboard(
                                        &copied,
                                        self.config.clipboard.osc52,
                                    );
                                    let n = copied.chars().count();
                                    self.status_msg = format!("ai: copied {n} chars");
                                }
                            } else {
                                self.side_terminal_selection = None;
                                self.side_click.word_drag = None;
                            }
                            self.side_click.last = None;
                            self.terminal_focus = crate::app::TerminalFocus::Side;
                            self.mode = Mode::Terminal;
                            return;
                        }
                        self.side_terminal_selection = None;
                        self.side_click.word_drag = None;
                        self.side_click.last = Some((now, grid_row, grid_col));
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        self.side_terminal_selection = None;
                        self.side_click.word_drag = None;
                    }
                    _ => {}
                }
                let pane_row = grid_row + 1;
                let pane_col = grid_col + 1;
                if let Some(term) = self.active_side_terminal() {
                    let mouse = term.mouse_state();
                    if mouse.any {
                        if let Some(bytes) = super::terminal_glue::encode_mouse_event_for_pty(
                            &ev, pane_row, pane_col, mouse,
                        ) {
                            let _ = term.write_bytes(&bytes);
                        }
                        if matches!(ev.kind, MouseEventKind::Down(_))
                            && !matches!(self.mode, Mode::Terminal)
                        {
                            self.mode = Mode::Terminal;
                            self.terminal_focus = crate::app::TerminalFocus::Side;
                        }
                        return;
                    }
                }
            }
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                self.terminal_focus = crate::app::TerminalFocus::Side;
                self.mode = Mode::Terminal;
            }
            return;
        }
        // Same gating for the debug pane — clicks land focus on the
        // pane (Mode::DebugPane), clicks on the tab bar switch tabs,
        // scroll wheel pages the active tab's body.
        if self.debug_pane_open && self.handle_debug_pane_mouse_event(&ev, row, col) {
            return;
        }
        // File-tree sidebar — clicks land focus on the pane and pick
        // a row when they're inside the body. Everything inside the
        // pane is swallowed so editor windows behind don't also fire.
        if self.file_tree.is_some()
            && self.file_tree_cols() > 0
            && col < self.file_tree_cols()
            && row >= self.buffer_top()
            && row < self.buffer_top() + self.buffer_rows()
        {
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                self.mode = Mode::FileTree;
                // Pane layout: chip row | spacer row | body…
                // Body therefore starts at `buffer_top + 2`.
                let body_start = self.buffer_top() + 2;
                let mut clicked_entry: Option<usize> = None;
                if row >= body_start {
                    let body_row = row - body_start;
                    let body_rows = self.buffer_rows().saturating_sub(2);
                    if let Some(state) = self.file_tree.as_mut() {
                        let scroll = if body_rows == 0 || state.entries.len() <= body_rows {
                            0
                        } else {
                            let half = body_rows / 2;
                            let max_scroll = state.entries.len().saturating_sub(body_rows);
                            state.cursor.saturating_sub(half).min(max_scroll)
                        };
                        let target = scroll + body_row;
                        if target < state.entries.len() {
                            state.cursor = target;
                            clicked_entry = Some(target);
                        }
                    }
                }
                // Double-click detection: a second click on the same
                // entry inside DOUBLE_CLICK_WINDOW opens the file (or
                // toggles the folder), matching the buffer-area click
                // convention.
                if let Some(idx) = clicked_entry {
                    let now = std::time::Instant::now();
                    let is_double = self
                        .last_tree_click
                        .filter(|(t, i)| {
                            now.duration_since(*t) <= crate::app::DOUBLE_CLICK_WINDOW && *i == idx
                        })
                        .is_some();
                    if is_double {
                        self.last_tree_click = None;
                        self.file_tree_activate_cursor();
                    } else {
                        self.last_tree_click = Some((now, idx));
                    }
                }
            }
            return;
        }
        // Click outside the terminal / debug panes while one of
        // those modes had focus — pull focus back into the editor
        // before the click is interpreted as a buffer-area mouse
        // event. Otherwise the click would fall through but the
        // mode would still be Terminal / DebugPane and the next
        // keystroke would route to the wrong place.
        if matches!(self.mode, Mode::Terminal | Mode::DebugPane | Mode::FileTree)
            && matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle | MouseButton::Right)
            )
        {
            self.mode = Mode::Normal;
        }

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
                } else if self.show_health_page {
                    self.health_scroll_by(-3);
                } else if self.show_install_page {
                    self.installer_scroll_by(-3);
                } else if self.show_messages_page {
                    self.messages_scroll_by(-3);
                } else if self.show_list_page {
                    self.list_scroll_by(-3);
                } else if self.show_test_results_page {
                    self.test_results_scroll_by(-3);
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
                } else if self.show_health_page {
                    self.health_scroll_by(3);
                } else if self.show_install_page {
                    self.installer_scroll_by(3);
                } else if self.show_messages_page {
                    self.messages_scroll_by(3);
                } else if self.show_list_page {
                    self.list_scroll_by(3);
                } else if self.show_test_results_page {
                    self.test_results_scroll_by(3);
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
                        let is_middle =
                            matches!(ev.kind, MouseEventKind::Down(MouseButton::Middle));
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
        // Translate the click into editor-pane-local coords. The pane's
        // left edge shifts right when the sidebar tree (or any
        // future left-anchored pane) is open, so a click at screen
        // col 30 inside an editor pane that starts at screen col 30
        // should read as pane col 0, not pane col 30. Without this
        // adjustment the gutter check passes spuriously and
        // `visual_col` ends up `tree_width` cells too far to the
        // right — clicking the first char lands the cursor at EOL.
        let pane_left = self.active_pane_rect().x as usize;
        if col < pane_left {
            return; // click landed in a left-side pane (tree), not the editor
        }
        let pane_col = col - pane_left;
        let gutter = self.gutter_width();
        if pane_col < gutter {
            return; // sign column / line numbers
        }
        // Walk forward from view_top, counting only visible (non-folded,
        // non-md-hidden) rows, until we've passed `buf_row` of them.
        // Without this the click would miscount when collapsed rows
        // sit between the viewport top and the click target. Lines
        // with code lenses contribute *two* rows (a phantom above
        // the buffer line); a click on the phantom routes through
        // `click_code_lens` instead of falling through to cursor
        // placement.
        let mut buf_line = self.window.view_top;
        let total = self.buffer.line_count();
        let mut visible_rows_seen = 0;
        let mut clicked_lens_line: Option<usize> = None;
        while buf_line < total {
            if self.line_is_folded(buf_line) || self.line_is_md_hidden(buf_line) {
                buf_line += 1;
                continue;
            }
            if self.line_has_code_lens(buf_line) {
                if visible_rows_seen == buf_row {
                    clicked_lens_line = Some(buf_line);
                    break;
                }
                visible_rows_seen += 1;
            }
            if visible_rows_seen == buf_row {
                break;
            }
            visible_rows_seen += 1;
            buf_line += 1;
        }
        if let Some(line) = clicked_lens_line {
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.click_code_lens_row(line, pane_col, gutter);
            }
            return;
        }
        if buf_line >= total {
            return;
        }
        let line_len = self.buffer.line_len(buf_line);
        // Translate the click's visual column (chars *as displayed*) to a
        // buffer char column. Tabs render at `TAB_WIDTH` cols but are still
        // a single buffer char, so a naive `raw_col` calculation lands the
        // cursor several chars past tab-indented text. We replay the same
        // width rule the renderer uses (tab = TAB_WIDTH, everything else = its
        // terminal display width) walking the line until we've consumed
        // `visual_col` cells.
        let visual_col = pane_col.saturating_sub(gutter) + self.window.view_left;
        let buf_col = visual_col_to_char_col(self, buf_line, visual_col, line_len);

        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Ctrl-click in Normal mode adds a secondary cursor at
                // the click position. Doesn't move the primary cursor —
                // that would defeat the purpose. The cursors persist
                // through the next `i`/`a` into Insert mode, where typing
                // and Backspace mirror at every position.
                // In any other mode the modifier falls through to the
                // normal click handler.
                if matches!(self.mode, Mode::Normal) && ev.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let line_start = self.buffer.line_start_idx(buf_line);
                    let pos = line_start + buf_col;
                    let primary = self
                        .buffer
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col);
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
                // Any fresh Down resets word-drag tracking; only a
                // double-click re-arms it below.
                self.word_drag_origin = None;
                // A click is an explicit "put my cursor here" — drop any
                // lens-phantom hop the user had parked from the keyboard.
                self.phantom_lens_idx = None;
                self.window.cursor.line = buf_line;
                self.window.cursor.col = buf_col;
                self.window.cursor.want_col = buf_col;
                if is_double {
                    // Expand to the inner word under the cursor and enter
                    // Visual-char mode with that span selected.
                    self.apply_visual_select_textobj(crate::text_object::TextObjectVerb::Word {
                        inner: true,
                    });
                    if let Some(anchor) = self.window.visual_anchor {
                        self.mode = Mode::Visual(VisualKind::Char);
                        // Remember the (start, end-exclusive) char range
                        // of the word so a subsequent drag can extend
                        // selection word-by-word.
                        let start = self.buffer.pos_to_char(anchor.line, anchor.col);
                        let end = self
                            .buffer
                            .pos_to_char(self.window.cursor.line, self.window.cursor.col)
                            + 1;
                        self.word_drag_origin = Some((start, end));
                    }
                    // Clear so a third click within the window doesn't
                    // re-trigger.
                    self.last_click = None;
                } else {
                    self.last_click = Some((now, buf_line, buf_col));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((origin_start, origin_end)) = self.word_drag_origin {
                    self.word_drag_extend(buf_line, buf_col, origin_start, origin_end);
                } else {
                    if !matches!(self.mode, Mode::Visual(_)) {
                        let anchor = self.window.cursor;
                        self.mode = Mode::Visual(VisualKind::Char);
                        self.window.visual_anchor = Some(anchor);
                    }
                    self.phantom_lens_idx = None;
                    self.window.cursor.line = buf_line;
                    self.window.cursor.col = buf_col;
                    self.window.cursor.want_col = buf_col;
                }
            }
            _ => {}
        }
    }

    /// Word-aware drag after a double-click. Anchors at the side of the
    /// origin word opposite the drag direction; the cursor snaps to the
    /// word boundary at the drag position. Dragging through whitespace
    /// leaves the selection at the previous word boundary so the visible
    /// span only jumps when a new word is actually entered.
    fn word_drag_extend(
        &mut self,
        buf_line: usize,
        buf_col: usize,
        origin_start: usize,
        origin_end: usize,
    ) {
        let line_start = self.buffer.line_start_idx(buf_line);
        let line_len = self.buffer.line_len(buf_line);
        let drag_pos = line_start + buf_col;

        // Only resolve a word range when the drag is on a non-whitespace
        // char — whitespace runs aren't worth selecting on their own and
        // would make the selection lurch through gaps.
        let drag_word: Option<(usize, usize)> = if buf_col < line_len {
            let c = self.buffer.rope.char(drag_pos);
            if !c.is_whitespace() {
                let cur = crate::cursor::Cursor {
                    line: buf_line,
                    col: buf_col,
                    want_col: buf_col,
                };
                crate::text_object::compute(
                    &self.buffer,
                    cur,
                    crate::text_object::TextObjectVerb::Word { inner: true },
                )
                .map(|r| (r.start, r.end))
            } else {
                None
            }
        } else {
            None
        };

        if !matches!(self.mode, Mode::Visual(_)) {
            self.mode = Mode::Visual(VisualKind::Char);
        }

        if drag_pos < origin_start {
            // Backward drag — anchor pinned to the end of the origin
            // word, cursor jumps to the start of the word at the drag.
            let sel_start = drag_word.map(|w| w.0).unwrap_or(origin_start);
            self.cursor_to_idx(origin_end.saturating_sub(1).max(origin_start));
            let anchor = self.window.cursor;
            self.cursor_to_idx(sel_start);
            self.window.visual_anchor = Some(anchor);
        } else if drag_pos >= origin_end {
            // Forward drag — anchor pinned to the start of the origin
            // word, cursor jumps to the last char of the word at the drag.
            let sel_end = drag_word.map(|w| w.1).unwrap_or(origin_end);
            self.cursor_to_idx(origin_start);
            let anchor = self.window.cursor;
            let cursor_idx = sel_end.saturating_sub(1).max(origin_start);
            self.cursor_to_idx(cursor_idx);
            self.window.visual_anchor = Some(anchor);
        } else {
            // Still inside the origin word — restore the origin selection.
            self.cursor_to_idx(origin_start);
            let anchor = self.window.cursor;
            self.cursor_to_idx(origin_end.saturating_sub(1).max(origin_start));
            self.window.visual_anchor = Some(anchor);
        }
    }

    /// Runs `key` through the `[keymaps]` matcher. `true` when the matcher
    /// took it — held toward a longer mapping, or expanded — and the caller
    /// must not parse it.
    pub(super) fn keymap_take(&mut self, key: KeyEvent, mode: MapMode) -> bool {
        // Insert and the command line have no parser state — any key may
        // start a mapping there.
        let may_start = mode.parse_ctx().is_none() || self.pending.accepts_mapping();
        if self.expanding_keymap || (self.keymap_held.is_empty() && !may_start) {
            return false;
        }
        self.keymap_held.push(key);
        match self.config.keymaps.lookup(mode, &self.keymap_held) {
            KeymapMatch::Pending => {
                let probe = self.held_as_pending(mode);
                // Only keys that already mean something alone need the
                // clock — a mapping of their own, or a finished command. An
                // unfinished prefix (`g`, `<leader>`) waits like it would
                // unmapped, so a pause to read the which-key popup doesn't
                // lose the mapping.
                let times_out = probe.is_none()
                    || (1..=self.keymap_held.len()).any(|n| {
                        self.config
                            .keymaps
                            .exact(mode, &self.keymap_held[..n])
                            .is_some()
                    });
                self.keymap_held_at = times_out.then(std::time::Instant::now);
                if probe.is_some_and(|p| p.any_leader_pending()) {
                    self.leader_pressed_at
                        .get_or_insert_with(std::time::Instant::now);
                }
            }
            KeymapMatch::Full(rhs) => {
                let rhs = rhs.to_vec();
                self.keymap_held.clear();
                self.keymap_held_at = None;
                self.leader_pressed_at = None;
                self.expand_keymap(&rhs);
            }
            // The common case: an unmapped key with nothing held.
            KeymapMatch::None if self.keymap_held.len() == 1 => {
                self.keymap_held.clear();
                return false;
            }
            KeymapMatch::None => self.keymap_flush(mode),
        }
        true
    }

    /// Resolves held keys whose longer mapping can no longer complete — the
    /// next key ruled it out, or the wait timed out. The longest held prefix
    /// that is a mapping of its own runs, as in Vim; with none, the first key
    /// goes through as typed. The keys after it are fed again, so they can
    /// start a mapping of their own.
    pub(super) fn keymap_flush(&mut self, mode: MapMode) {
        let held = std::mem::take(&mut self.keymap_held);
        self.keymap_held_at = None;
        let mapped = (1..=held.len()).rev().find_map(|n| {
            self.config
                .keymaps
                .exact(mode, &held[..n])
                .map(|rhs| (n, rhs.to_vec()))
        });
        let used = match mapped {
            Some((n, rhs)) => {
                self.expand_keymap(&rhs);
                n
            }
            None => {
                self.feed_unmapped(&held[..1]);
                1
            }
        };
        for &k in &held[used..] {
            if !self.replay_key(k) {
                break;
            }
        }
    }

    /// Resolves a held sequence whose wait has run out. `true` when it did,
    /// so the caller repaints.
    pub(super) fn keymap_flush_if_due(&mut self, now: std::time::Instant) -> bool {
        let Some(at) = self.keymap_held_at else {
            return false;
        };
        if now < at + self.config.keymaps.timeout {
            return false;
        }
        self.keymap_flush(self.keymap_mode());
        true
    }

    /// The `[keymaps]` table that applies in the current mode.
    pub(super) fn keymap_mode(&self) -> MapMode {
        match self.mode {
            Mode::Visual(_) => MapMode::Visual,
            Mode::Insert => MapMode::Insert,
            Mode::Command | Mode::Search { .. } => MapMode::Command,
            _ => MapMode::Normal,
        }
    }

    /// The parser state the held keys would leave if they went through
    /// unmapped — `None` when one of them would finish or cancel a command,
    /// and always in Insert and on the command line, where every key is text
    /// that means itself.
    /// It works on a copy, so nothing is dispatched: the timeout rule and
    /// the which-key popup judge held keys by it without committing to them.
    fn held_as_pending(&self, mode: MapMode) -> Option<parser::PendingCmd> {
        let ctx = mode.parse_ctx()?;
        let mut probe = self.pending.clone();
        self.keymap_held
            .iter()
            .all(|&k| matches!(parser::parse(&mut probe, k, ctx), ParseResult::Pending))
            .then_some(probe)
    }

    /// The which-key popup for the leader chord in flight, with the user's
    /// `[keymaps]` entries under it layered over the built-in rows. Keys a
    /// multi-key mapping is holding count as typed, so `<space>` held for
    /// `<leader>x` still opens the Leader popup.
    pub(super) fn whichkey_popup(&self) -> Option<WhichKeyState> {
        let ctx = if matches!(self.mode, Mode::Visual(_)) {
            ParseCtx::Visual
        } else {
            ParseCtx::Normal
        };
        let pending = self
            .held_as_pending(ctx.into())
            .unwrap_or_else(|| self.pending.clone());
        let (title, mut entries) = if pending.awaiting_leader {
            ("Leader", state::leader_entries())
        } else if pending.awaiting_buffer_leader {
            ("Buffer", state::buffer_prefix_entries())
        } else if pending.awaiting_debug_leader {
            ("Debug", state::debug_prefix_entries())
        } else if pending.awaiting_hunk_leader {
            ("Hunk", state::hunk_prefix_entries())
        } else if pending.awaiting_git_leader {
            ("Git", state::git_prefix_entries())
        } else if pending.awaiting_task_leader {
            ("Task", state::task_prefix_entries())
        } else if pending.awaiting_terminal_leader {
            ("Terminal", state::terminal_prefix_entries())
        } else if pending.awaiting_test_leader {
            ("Test", state::test_prefix_entries())
        } else if pending.awaiting_ai_leader {
            ("AI", state::ai_prefix_entries())
        } else if pending.awaiting_package_leader {
            ("Package", state::package_prefix_entries())
        } else if pending.awaiting_android_leader {
            ("Android", state::android_prefix_entries())
        } else {
            return None;
        };
        if let Some(chord) = pending.leader_chord() {
            let chord: Vec<KeyEvent> = chord
                .chars()
                .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .collect();
            self.config
                .keymaps
                .merge_whichkey(ctx.into(), &chord, &mut entries);
        }
        Some(WhichKeyState {
            title: title.into(),
            entries,
        })
    }

    fn expand_keymap(&mut self, rhs: &[KeyEvent]) {
        let skip = self.pending.absorb_mapping_count(rhs);
        self.feed_unmapped(&rhs[skip..]);
    }

    /// Feeds keys to the current mode with mappings off — Vim's `noremap`.
    fn feed_unmapped(&mut self, keys: &[KeyEvent]) {
        self.expanding_keymap = true;
        for &k in keys {
            if !self.replay_key(k) {
                break;
            }
        }
        self.expanding_keymap = false;
    }

    pub(super) fn handle_keyboard(&mut self, key: KeyEvent, ctx: ParseCtx) {
        // `:s///c` takes every key while it asks, before a mapping could.
        if self.sub_confirm.is_some() {
            self.sub_confirm_key(key);
            return;
        }
        if self.keymap_take(key, ctx.into()) {
            return;
        }
        // Bare ENTER on a lens-bearing line invokes the lens, same as
        // `<leader>l` and the mouse click. Only fires when the parser
        // has no partial state (no pending operator / count / prefix)
        // so `d<CR>` and similar future motions stay future-compatible.
        if matches!(ctx, parser::ParseCtx::Normal)
            && matches!(key.code, KeyCode::Enter)
            && key.modifiers.is_empty()
            && self.pending.is_clean()
            && self.line_has_code_lens(self.window.cursor.line)
        {
            self.apply_action(parser::Action::LspExecuteCodeLens);
            return;
        }
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
        let prefix_active = self.pending.any_leader_pending();
        if prefix_active {
            if self.leader_pressed_at.is_none() {
                self.leader_pressed_at = Some(std::time::Instant::now());
            }
        } else {
            self.leader_pressed_at = None;
        }
    }

    // The arrow-key arms keep their bound checks as in-body `if`s so all
    // four (Left/Right/Up/Down) read the same shape — Right/Down can't
    // collapse to a guard (they compute a length first), so collapsing
    // only Left/Up would split the block stylistically.
    #[allow(clippy::collapsible_match)]
    pub(super) fn handle_insert_key(&mut self, key: KeyEvent) {
        let is_esc = matches!(key.code, KeyCode::Esc);
        // Every Insert-mode keystroke resets the Copilot idle timer.
        // The actual `inlineCompletion` request fires from
        // `copilot_maybe_request_inline` once ~250ms of typing-idle
        // has passed — this just records "the user just typed."
        self.last_keystroke_at = std::time::Instant::now();
        // Any key other than `<Tab>` invalidates an active ghost.
        // Tab gets its own branch below where it either consumes or
        // falls through; we don't drop the ghost preemptively because
        // `copilot_accept_ghost` already does so on consume.
        if !matches!(key.code, KeyCode::Tab) {
            self.copilot_invalidate_ghost();
        }
        // Completion popup intercepts a small set of keys; everything else dismisses it.
        if self.completion.is_some() {
            let captured = self.handle_insert_key_with_completion(key);
            if captured {
                return;
            }
            // Fall through with completion now closed.
        }
        // Below the popup, so a mapping never takes a key the popup uses,
        // and never `<Tab>` from a Copilot ghost it would accept — every
        // other key has already dropped the ghost above.
        let ghost_tab = matches!(key.code, KeyCode::Tab) && self.copilot_ghost.is_some();
        // The register name after `Ctrl-R`, and every key of a `Ctrl-V`
        // sequence, is a literal — like `fH`'s target, never a mapping's key.
        let literal_next = self.insert_register_pending || self.insert_literal_pending.is_some();
        if !ghost_tab && !literal_next && self.keymap_take(key, MapMode::Insert) {
            return;
        }
        // Esc stays out of the recording because it ends the session — except
        // straight after `Ctrl-V`, which consumes it, so `.` needs it back.
        let esc_consumed = matches!(
            self.insert_literal_pending,
            Some(super::state::LiteralPending::Key)
        );
        if !self.replaying && (!is_esc || esc_consumed) {
            if let Some(rec) = self.recording.as_mut() {
                rec.keys.push(key);
            }
        }
        if self.insert_register_pending {
            self.insert_register_pending = false;
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.insert_register_at_cursor(c);
                }
                // Cancelled. Drop the recorded `Ctrl-R`, and the cancelling key
                // unless it was Esc (never recorded), so `.` doesn't replay a
                // `Ctrl-R` that swallows whatever was typed after it.
                _ if !self.replaying => {
                    if let Some(rec) = self.recording.as_mut() {
                        let n = if is_esc { 1 } else { 2 };
                        rec.keys.truncate(rec.keys.len().saturating_sub(n));
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(pending) = self.insert_literal_pending.take() {
            let step = literal_step(&pending, key);
            self.insert_literal_pending = step.next;
            if let Some(c) = step.insert {
                self.insert_literal_char(c);
            }
            if !step.reprocess {
                return;
            }
        }
        if self.replace_session.is_some() && self.replace_mode_key(key) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                // Vim convention: if the cursor's line is all whitespace
                // on Esc-out-of-Insert, strip it. This is almost always
                // auto-indent that the user landed on but never put real
                // content on — leaving the whitespace behind clutters the
                // file and trips trailing-whitespace formatters on save.
                let line_idx = self.window.cursor.line;
                let line_len = self.buffer.line_len(line_idx);
                let all_ws = line_len > 0
                    && (0..line_len).all(|c| {
                        matches!(self.buffer.char_at(line_idx, c), Some(' ') | Some('\t'))
                    });
                if all_ws {
                    let line_start = self.buffer.line_start_idx(line_idx);
                    self.buffer.delete_range(line_start, line_start + line_len);
                    self.window.cursor.col = 0;
                    self.window.cursor.want_col = 0;
                } else if self.window.cursor.col > 0 {
                    self.window.cursor.col -= 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                }
                self.mode = Mode::Normal;
                self.signature_help = None;
                // Collapse multi-cursor on the same Esc that exits Insert.
                self.additional_cursors.clear();
                // A snippet session is Insert-mode-only — Esc ends it.
                self.snippet_session = None;
                self.end_insert_recording();
            }
            // One Normal-mode command, then back to Insert. The session so far
            // ends here as Esc would end it, minus the step back and the
            // whitespace-line strip.
            KeyCode::Char('o' | 'O') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Recorded above with the typed text, but it isn't text.
                if !self.replaying {
                    if let Some(rec) = self.recording.as_mut() {
                        rec.keys.pop();
                    }
                }
                self.end_insert_recording();
                let line = self.window.cursor.line;
                let col = self.window.cursor.col;
                let stepped_back = if col > 0 && col == self.buffer.line_len(line) {
                    Some((line, col - 1))
                } else {
                    None
                };
                if let Some((_, c)) = stepped_back {
                    self.window.cursor.col = c;
                    self.window.cursor.want_col = c;
                }
                self.mode = Mode::Normal;
                self.signature_help = None;
                self.additional_cursors.clear();
                self.snippet_session = None;
                self.insert_oneshot = Some(state::InsertOneshot { stepped_back });
            }
            KeyCode::Char(c)
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'n' || c == 'p') =>
            {
                self.lsp_request_completion(None);
            }
            KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_register_pending = true;
            }
            KeyCode::Char('v' | 'V') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_literal_pending = Some(super::state::LiteralPending::Key);
            }
            // Copy the char at the cursor's column from the line below (`Ctrl-E`)
            // or above (`Ctrl-Y`). Multi-cursor leaves them alone — each cursor
            // would need its own neighbour.
            KeyCode::Char(c @ ('e' | 'E' | 'y' | 'Y'))
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.additional_cursors.is_empty() =>
            {
                let line = self.window.cursor.line;
                let col = self.window.cursor.col;
                let from = if c.eq_ignore_ascii_case(&'e') {
                    Some(line + 1).filter(|l| *l < self.buffer.line_count())
                } else {
                    line.checked_sub(1)
                };
                let copied = from
                    .filter(|l| col < self.buffer.line_len(*l))
                    .and_then(|l| self.buffer.char_at(l, col));
                if let Some(ch) = copied {
                    self.insert_literal_char(ch);
                }
            }
            // Multi-cursor leaves these alone: shifting one line would leave
            // the other cursors' char indices pointing at the wrong text.
            KeyCode::Char('t' | 'T')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.additional_cursors.is_empty() =>
            {
                // Not `indent_lines`: it skips blank lines, and a fresh `o`
                // line is where Ctrl-T gets used most.
                let unit = self.editorconfig.indent_string();
                let line_start = self.buffer.line_start_idx(self.window.cursor.line);
                self.buffer.insert_at_idx(line_start, &unit);
                self.window.cursor.col += unit.chars().count();
                self.window.cursor.want_col = self.window.cursor.col;
            }
            KeyCode::Char('d' | 'D')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.additional_cursors.is_empty() =>
            {
                let line = self.window.cursor.line;
                let col = self.window.cursor.col;
                let before = self.buffer.line_len(line);
                self.outdent_lines(line, line);
                let col = (col + self.buffer.line_len(line)).saturating_sub(before);
                self.window.cursor.col = col;
                self.window.cursor.want_col = col;
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
                    && self
                        .buffer
                        .char_at(self.window.cursor.line, self.window.cursor.col)
                        == Some(c)
                {
                    // If the cursor sits on the same closing char the user is typing,
                    // step past it instead of inserting a duplicate. Lets `}`/`)`/`"`
                    // skip over an auto-inserted closer.
                    self.window.cursor.col += 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                } else if let Some(close) = open_pair_for(c) {
                    if should_auto_pair(
                        c,
                        &self.buffer,
                        self.window.cursor.line,
                        self.window.cursor.col,
                    ) {
                        self.buffer
                            .insert_char(self.window.cursor.line, self.window.cursor.col, c);
                        self.buffer.insert_char(
                            self.window.cursor.line,
                            self.window.cursor.col + 1,
                            close,
                        );
                        self.window.cursor.col += 1;
                        self.window.cursor.want_col = self.window.cursor.col;
                    } else {
                        self.buffer
                            .insert_char(self.window.cursor.line, self.window.cursor.col, c);
                        self.window.cursor.col += 1;
                        self.window.cursor.want_col = self.window.cursor.col;
                    }
                } else {
                    self.buffer
                        .insert_char(self.window.cursor.line, self.window.cursor.col, c);
                    self.window.cursor.col += 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                }
                // Tag auto-completion: typing `>` at the end of an opening
                // HTML tag inserts the matching closer after the cursor so
                // `<div>` becomes `<div>|</div>`. Triggered after the `>`
                // has been written and the cursor advanced past it.
                if c == '>' && is_html_like_buffer(&self.buffer) {
                    if let Some(tag) = detect_open_tag_to_close(
                        &self.buffer,
                        self.window.cursor.line,
                        self.window.cursor.col,
                    ) {
                        let closer = format!("</{tag}>");
                        self.buffer.insert_str(
                            self.window.cursor.line,
                            self.window.cursor.col,
                            &closer,
                        );
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
                    // `.`, `:`, `@`, `<` are commonly declared as the
                    // server's `triggerCharacters` (member-access, Rust
                    // paths, Razor / decorator anchors, JSX/HTML opens) —
                    // sending them as triggerCharacter unlocks the
                    // member-access flavour of completion. `!` and `#`
                    // (Emmet abbreviation anchors) aren't typically in any
                    // server's declared trigger list, so emmet-ls would
                    // ignore them as "irrelevant" triggers and return
                    // nothing. Fall back to Invoked (`triggerKind=1`) for
                    // those so the server treats it as a manual request
                    // and returns its abbreviation matches.
                    let trigger = if matches!(c, '.' | ':' | '@' | '<') {
                        Some(c)
                    } else {
                        None
                    };
                    self.lsp_request_completion(trigger);
                }
            }
            KeyCode::Enter => self.handle_insert_newline(),
            // Plain `w` / `u` never reach this arm: `Char(c)` above takes
            // every char typed without Ctrl.
            KeyCode::Backspace | KeyCode::Char('w' | 'W' | 'u' | 'U') => {
                let popup_was_open = self.completion.is_some();
                // macOS-convention modifier shortcuts:
                //   Alt / Option + Backspace → delete previous word
                //   Cmd / Super  + Backspace → delete to start of line
                //   Ctrl + Backspace         → also delete previous word
                //                              (terminal / Linux alias)
                //   Ctrl-W                   → Vim's word-delete, same rules
                //   Ctrl-U                   → Vim's line-delete: to the
                //                              indent, then to column 0
                // Multi-cursor + a modifier falls back to plain mirror
                // for v1; per-cursor word/line semantics would need more
                // careful indexing and isn't urgent.
                let mods = key.modifiers;
                let ctrl_u = matches!(key.code, KeyCode::Char('u' | 'U'));
                let word_back = !ctrl_u
                    && (mods.contains(KeyModifiers::ALT) || mods.contains(KeyModifiers::CONTROL));
                let line_back = ctrl_u
                    || mods.contains(KeyModifiers::SUPER)
                    || mods.contains(KeyModifiers::META);
                if !self.additional_cursors.is_empty() {
                    self.mirror_backspace();
                } else if line_back && self.window.cursor.col > 0 {
                    let indent = motion::first_non_blank(&self.buffer, self.window.cursor)
                        .target
                        .col;
                    let to_col = if ctrl_u && self.window.cursor.col > indent {
                        indent
                    } else {
                        0
                    };
                    let line_start = self.buffer.line_start_idx(self.window.cursor.line);
                    let cursor_idx = self
                        .buffer
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                    self.buffer.delete_range(line_start + to_col, cursor_idx);
                    self.window.cursor.col = to_col;
                    self.window.cursor.want_col = to_col;
                } else if word_back && self.window.cursor.col > 0 {
                    let new_col = previous_word_boundary(
                        &self.buffer,
                        self.window.cursor.line,
                        self.window.cursor.col,
                    );
                    let line_start = self.buffer.line_start_idx(self.window.cursor.line);
                    let cursor_idx = line_start + self.window.cursor.col;
                    let to_idx = line_start + new_col;
                    self.buffer.delete_range(to_idx, cursor_idx);
                    self.window.cursor.col = new_col;
                    self.window.cursor.want_col = new_col;
                } else if self.window.cursor.col > 0 {
                    // If the cursor sits between an auto-inserted pair like {|},
                    // wipe out both characters in one stroke.
                    let prev = self
                        .buffer
                        .char_at(self.window.cursor.line, self.window.cursor.col - 1);
                    let next = self
                        .buffer
                        .char_at(self.window.cursor.line, self.window.cursor.col);
                    if let (Some(p), Some(n)) = (prev, next) {
                        if open_pair_for(p) == Some(n) {
                            let idx = self
                                .buffer
                                .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                            self.buffer.delete_range(idx - 1, idx + 1);
                            self.window.cursor.col -= 1;
                            self.window.cursor.want_col = self.window.cursor.col;
                            return;
                        }
                    }
                    let idx = self
                        .buffer
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                    self.buffer.delete_range(idx - 1, idx);
                    self.window.cursor.col -= 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                } else if self.window.cursor.line > 0 {
                    let prev = self.window.cursor.line - 1;
                    let prev_len = self.buffer.line_len(prev);
                    let idx = self.buffer.pos_to_char(prev, prev_len);
                    self.buffer.delete_range(idx, idx + 1);
                    self.window.cursor.line = prev;
                    self.window.cursor.col = prev_len;
                    self.window.cursor.want_col = prev_len;
                }
                if popup_was_open && !self.replaying {
                    self.lsp_request_completion(None);
                }
            }
            KeyCode::Tab => {
                // Copilot ghost takes priority over both snippet-stop
                // and literal-indent: if there's a live suggestion at
                // the cursor, consume it and skip the rest.
                if self.copilot_accept_ghost() {
                    return;
                }
                // Snippet session takes priority: Tab cycles to the next
                // stop. After advancing past the final stop we clear the
                // session and fall through; clearing only — no indent is
                // inserted on the cycle-completion Tab (the user almost
                // certainly meant "exit snippet", not "indent").
                if self.advance_snippet_session() {
                    return;
                }
                let s = self.editorconfig.indent_string();
                let inserted = s.chars().count();
                self.buffer
                    .insert_str(self.window.cursor.line, self.window.cursor.col, &s);
                self.window.cursor.col += inserted;
                self.window.cursor.want_col = self.window.cursor.col;
            }
            KeyCode::Left => {
                if self.window.cursor.col > 0 {
                    self.window.cursor.col -= 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                }
            }
            KeyCode::Right => {
                let len = self.buffer.line_len(self.window.cursor.line);
                if self.window.cursor.col < len {
                    self.window.cursor.col += 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                }
            }
            KeyCode::Up => {
                if self.window.cursor.line > 0 {
                    self.window.cursor.line -= 1;
                    let len = self.buffer.line_len(self.window.cursor.line);
                    self.window.cursor.col = self.window.cursor.want_col.min(len);
                }
            }
            KeyCode::Down => {
                let last = self.buffer.line_count().saturating_sub(1);
                if self.window.cursor.line < last {
                    self.window.cursor.line += 1;
                    let len = self.buffer.line_len(self.window.cursor.line);
                    self.window.cursor.col = self.window.cursor.want_col.min(len);
                }
            }
            KeyCode::Home => {
                self.window.cursor.col = 0;
                self.window.cursor.want_col = 0;
            }
            KeyCode::End => {
                let len = self.buffer.line_len(self.window.cursor.line);
                self.window.cursor.col = len;
                self.window.cursor.want_col = len;
            }
            _ => {}
        }
    }

    /// Replace mode's own keys. A typed char overwrites, `Backspace` takes it
    /// back, `Enter` breaks the line without overwriting anything, and `Esc`
    /// types the text again for a count before Insert's `Esc` runs. Any other
    /// key goes to Insert as usual — it may move the cursor or change the
    /// text, so `Backspace` stops putting chars back from there.
    fn replace_mode_key(&mut self, key: KeyEvent) -> bool {
        let modified = key.modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META,
        );
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.replace_mode_char(c);
                true
            }
            KeyCode::Backspace if !modified => {
                self.replace_mode_backspace();
                true
            }
            KeyCode::Enter => {
                let at = self
                    .buffer
                    .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                let before = self.buffer.total_chars();
                self.handle_insert_newline();
                let len = self.buffer.total_chars() - before;
                if let Some(session) = self.replace_session.as_mut() {
                    session.undo.push(state::ReplaceUndo::Added { at, len });
                    session.typed.push('\n');
                }
                true
            }
            KeyCode::Esc => {
                self.replace_mode_finish();
                false
            }
            _ => {
                if let Some(session) = self.replace_session.as_mut() {
                    session.undo.clear();
                    session.typed.clear();
                }
                false
            }
        }
    }

    /// Puts a `Ctrl-V` char in as it is: no auto-pair, closer-skip, tag close
    /// or completion trigger — which is the point of typing it literally.
    fn insert_literal_char(&mut self, c: char) {
        // The buffer renderer draws every char but tab, space and NBSP as-is,
        // so a control char would reach the terminal as a raw byte and
        // scramble the frame. Tab it already draws as spaces.
        if c.is_control() && c != '\t' {
            self.status_msg = format!(
                "Ctrl-V: U+{:04X} is a control character, which binvim can't display",
                c as u32
            );
            return;
        }
        if !self.additional_cursors.is_empty() {
            self.mirror_insert_char(c);
            return;
        }
        self.buffer
            .insert_char(self.window.cursor.line, self.window.cursor.col, c);
        self.window.cursor.col += 1;
        self.window.cursor.want_col = self.window.cursor.col;
    }

    /// Smart Enter — copies the current line's leading whitespace onto the
    /// new line, adds one indent unit when the previous non-whitespace char
    /// is an opener (`{`/`[`/`(`/`:`/`=>`/`->`), and splits paired
    /// openers/closers (`{|}`) onto three lines so the cursor lands on a
    /// double-indented middle row ready for the body.
    fn handle_insert_newline(&mut self) {
        // Multi-cursor: every additional cursor inserts a literal `\n`
        // alongside the primary's smart-indent newline. Smart indent at
        // the secondaries is non-trivial — neighbouring context can
        // disagree across positions — so for v1 we just keep them in
        // sync with a bare line break. Caller can take it from there.
        if !self.additional_cursors.is_empty() {
            self.mirror_insert_char('\n');
            return;
        }
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
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
        let before: String = chars[..col.min(chars.len())].iter().collect();
        // What's the first non-whitespace char at/after the cursor?
        let next_non_ws = chars.get(col).copied();
        let opener_after = super::edit::opens_block(&before);
        let split_pair =
            should_split_pair_on_enter(prev_non_ws, next_non_ws, chars.get(col + 1).copied());

        if split_pair {
            // `{|}` → three lines, cursor double-indented in the middle.
            let body_indent = format!("{lead}{unit}");
            let payload = format!("\n{body_indent}\n{lead}");
            self.buffer.insert_str(line, col, &payload);
            self.window.cursor.line = line + 1;
            self.window.cursor.col = body_indent.chars().count();
            self.window.cursor.want_col = self.window.cursor.col;
            return;
        }

        let next_indent = if opener_after {
            format!("{lead}{unit}")
        } else {
            lead
        };
        let payload = format!("\n{next_indent}");
        self.buffer.insert_str(line, col, &payload);
        self.window.cursor.line = line + 1;
        self.window.cursor.col = next_indent.chars().count();
        self.window.cursor.want_col = self.window.cursor.col;
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
            KeyCode::Enter => {
                // Enter accepts the LSP completion popup item. Tab is
                // reserved for the Copilot ghost (see below) — when
                // both surfaces are live, Tab → Copilot, Enter → LSP.
                self.completion_accept();
                true
            }
            KeyCode::Tab => {
                // Prefer the Copilot ghost over the LSP popup when
                // both are visible — the popup auto-closes on the
                // ghost insert so it doesn't keep fighting for the
                // text that just landed. If no ghost is live, fall
                // through to the popup-accept so users without
                // Copilot don't lose their existing Tab-accept flow.
                if self.copilot_accept_ghost() {
                    self.completion = None;
                    return true;
                }
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
                if !key.modifiers.contains(KeyModifiers::CONTROL) && is_completion_trigger(c) =>
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
        use super::cmdline_history::HistoryKind;
        if self.keymap_take(key, MapMode::Command) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.cmdline.clear();
                self.cmdline_cursor = 0;
                self.cmdline_completion_reset();
                self.history_reset();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                self.cmdline_cursor = 0;
                self.cmdline_completion_reset();
                self.history_record(HistoryKind::Command, &line);
                self.history_reset();
                self.mode = Mode::Normal;
                self.exec_command(&line);
            }
            KeyCode::Backspace => {
                self.cmdline_completion_reset();
                if self.cmdline.is_empty() {
                    self.history_reset();
                    self.mode = Mode::Normal;
                } else {
                    self.cmdline_backspace_at_cursor();
                }
            }
            KeyCode::Delete => {
                self.cmdline_completion_reset();
                self.cmdline_delete_forward_at_cursor();
            }
            KeyCode::Left => self.cmdline_cursor_move_back(),
            KeyCode::Right => self.cmdline_cursor_move_forward(),
            KeyCode::Home => self.cmdline_cursor = 0,
            KeyCode::End => self.cmdline_cursor = self.cmdline.len(),
            KeyCode::Tab => self.cmdline_tab(false),
            KeyCode::BackTab => self.cmdline_tab(true),
            KeyCode::Up => {
                self.cmdline_completion_reset();
                self.history_walk_back(HistoryKind::Command);
            }
            KeyCode::Down => {
                self.cmdline_completion_reset();
                self.history_walk_forward(HistoryKind::Command);
            }
            KeyCode::Char(c) => {
                self.cmdline_completion_reset();
                self.cmdline_insert_char_at_cursor(c);
            }
            _ => {}
        }
    }

    /// Drop every terminal before quitting so background processes (`pnpm
    /// dev`, `cargo watch`, an SSH session, …) get SIGHUP when their master
    /// PTY fd is released, rather than orphaning until we exit. Side-pane AI
    /// sessions get the same treatment.
    fn quit_now(&mut self) {
        self.terminals.clear();
        self.terminal_pane_open = false;
        self.side_terminals.clear();
        self.side_terminal_pane_open = false;
        self.should_quit = true;
    }

    /// Quit, unless a buffer other than the active one has unsaved changes:
    /// they'd be lost without the user ever seeing them go. Vim's E162.
    fn quit_unless_background_dirty(&mut self) {
        match self.dirty_background_buffer() {
            Some(name) => {
                self.status_msg = format!("E162: No write since last change for buffer \"{name}\"");
            }
            None => self.quit_now(),
        }
    }

    pub(super) fn exec_command(&mut self, line: &str) {
        let cmd = match command::parse(line) {
            ExCommand::Ranged { spec, rest } => match self.resolve_spec(&spec) {
                Ok((l1, l2)) => {
                    command::parse_after_range(ExRange::Lines(l1 + 1, l2 + 1), &rest, line)
                }
                Err(e) => {
                    self.status_msg = e;
                    return;
                }
            },
            cmd => cmd,
        };
        match cmd {
            ExCommand::Write => match self.save_active() {
                Ok(format_note) => {
                    // Show the basename only — full paths blow up the
                    // notification box for deep working trees. Disambiguating
                    // between two `index.ts` files isn't this message's job;
                    // the tab bar already carries that signal.
                    let name = self
                        .buffer
                        .path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "[No Name]".into());
                    let lines = self.buffer.line_count();
                    self.status_msg = match format_note {
                        Some(note) => format!("\"{name}\" {lines}L written ({note})"),
                        None => format!("\"{name}\" {lines}L written"),
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
                if self.show_health_page {
                    self.show_health_page = false;
                } else if self.show_messages_page {
                    self.show_messages_page = false;
                } else if self.show_list_page {
                    self.show_list_page = false;
                } else if self.show_test_results_page {
                    self.show_test_results_page = false;
                } else if self.buffer.dirty {
                    self.status_msg = "E37: No write since last change (use :q!)".into();
                } else {
                    self.quit_unless_background_dirty();
                }
            }
            ExCommand::QuitAll => {
                if self.buffer.dirty {
                    self.status_msg = "E37: No write since last change (use :qa!)".into();
                } else {
                    self.quit_unless_background_dirty();
                }
            }
            ExCommand::QuitForce | ExCommand::QuitAllForce => self.quit_now(),
            ExCommand::WriteQuit => match self.save_active() {
                Ok(_) => self.quit_unless_background_dirty(),
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::WriteQuitIfModified => {
                let saved = if self.buffer.dirty {
                    self.save_active().map(|_| ())
                } else {
                    Ok(())
                };
                match saved {
                    Ok(()) => self.quit_unless_background_dirty(),
                    Err(e) => self.status_msg = format!("error: {e}"),
                }
            }
            ExCommand::WriteAll => match self.save_all() {
                Ok(0) => self.status_msg = "No buffers were modified".into(),
                Ok(n) => {
                    self.status_msg = format!("{n} buffer{} written", if n == 1 { "" } else { "s" })
                }
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::WriteQuitAll => match self.save_all() {
                Ok(_) => self.quit_now(),
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::Revert => {
                self.status_msg = match self.force_reload_from_disk() {
                    Some(name) => format!("\"{name}\" reloaded"),
                    None if self.buffer.path.is_none() => "E32: No file name".into(),
                    None => "error: couldn't read the file back from disk".into(),
                };
            }
            ExCommand::Edit(p) if p.is_empty() => {
                self.status_msg = "E32: No file name".into();
            }
            ExCommand::Edit(p) => {
                let opened = if p == "#" {
                    self.switch_alternate(None)
                } else {
                    self.open_buffer(PathBuf::from(p))
                };
                if let Err(e) = opened {
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
            ExCommand::Substitute {
                range,
                pattern,
                replacement,
                flags,
            } => self.exec_substitute(range, &pattern, &replacement, flags),
            ExCommand::RepeatSubstitute {
                range,
                last_search,
                keep_flags,
                flags,
            } => self.repeat_substitute(range, last_search, keep_flags, flags),
            ExCommand::ProjectSubstitute {
                pattern,
                replacement,
                flags,
            } => {
                self.project_substitute(&pattern, &replacement, flags);
            }
            ExCommand::DeleteRange {
                range,
                register,
                count,
            } => {
                let (l1, l2) = self.counted_range(range, count);
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.delete_lines(l1, l2, register);
            }
            ExCommand::Filter { range, cmd } => {
                let (l1, l2) = self.resolve_range(range, true);
                self.filter_lines(l1, l2, &cmd);
            }
            ExCommand::YankRange {
                range,
                register,
                count,
            } => {
                let (l1, l2) = self.counted_range(range, count);
                self.yank_lines(l1, l2, register);
            }
            ExCommand::Normal { range, keys, remap } => {
                let lines = match range {
                    ExRange::Implicit => None,
                    _ => Some(self.counted_range(range, None)),
                };
                self.exec_normal(lines, &keys, remap);
            }
            ExCommand::MoveLines { range, to, copy } => {
                let (l1, l2) = self.counted_range(range, None);
                match self
                    .line_target(&to)
                    .and_then(|at| self.transfer_lines(l1, l2, at, copy))
                {
                    Ok(last) => self.cursor_to_first_non_blank(last),
                    Err(e) => self.status_msg = e,
                }
            }
            ExCommand::JoinRange {
                range,
                count,
                spaces,
            } => {
                let (l1, l2) = self.counted_range(range, count);
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.window.cursor.line = l1;
                self.join_lines((l2 - l1).max(1), spaces);
            }
            ExCommand::ShiftRange {
                range,
                count,
                right,
                times,
            } => {
                let (l1, l2) = self.counted_range(range, count);
                let op = if right {
                    crate::mode::Operator::Indent
                } else {
                    crate::mode::Operator::Outdent
                };
                self.history.record(&self.buffer.rope, self.window.cursor);
                for _ in 0..times {
                    self.shift_lines(op, l1, l2);
                }
                self.cursor_to_first_non_blank(l2);
            }
            ExCommand::PutLines {
                range,
                register,
                above,
            } => {
                let (_, line) = self.counted_range(range, None);
                // `:0put` goes above the first line.
                let above = above || matches!(range, ExRange::Single(0));
                self.put_lines(line, register, above);
            }
            ExCommand::AlignLines {
                range,
                align,
                width,
            } => {
                let (l1, l2) = self.counted_range(range, None);
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.align_lines(l1, l2, align, width);
                self.clamp_cursor_normal();
            }
            ExCommand::Retab {
                range,
                bang,
                tabstop,
            } => {
                // `:retab` takes the whole file unless given a range.
                let (l1, l2) = self.resolve_range(range, false);
                let l2 = l2.min(self.last_text_line());
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.retab_lines(l1, l2, bang, tabstop);
                self.clamp_cursor_normal();
            }
            ExCommand::NoHighlight => {
                self.search_hl_off = true;
            }
            ExCommand::Format => self.format_active(),
            ExCommand::Health => self.cmd_health(),
            ExCommand::Messages => self.cmd_messages(),
            ExCommand::Registers => self.cmd_registers(),
            ExCommand::Changes => self.cmd_changes(),
            ExCommand::Marks => self.cmd_marks(),
            ExCommand::Jumps => self.cmd_jumps(),
            ExCommand::CodeLensStatus => self.cmd_code_lens_status(),
            ExCommand::Workspaces => self.cmd_workspaces(),
            ExCommand::Terminal(cmd) => self.cmd_open_terminal(cmd),
            ExCommand::Lazygit => self.cmd_lazygit(),
            ExCommand::Install => self.cmd_install(),
            ExCommand::Update => self.cmd_update(),
            ExCommand::TaskPicker => self.cmd_task_picker(),
            ExCommand::TaskLast => self.cmd_task_last(),
            // Ex commands open the tool without the path handoff — the
            // uppercase leader bindings (`<leader>jC` / `jX` / `jO`) are
            // the explicit "with file context" path.
            ExCommand::AiTool(tool) => self.open_side_terminal(tool.label(), tool.command(), false),
            ExCommand::Debug(sub) => self.dispatch_debug(sub),
            ExCommand::DebugWatch(sub) => self.dispatch_debug_watch(sub),
            ExCommand::DebugWatchesShow => self.dispatch_debug_watches_show(),
            ExCommand::GitBlame => self.toggle_blame(),
            ExCommand::Copilot(sub) => {
                use crate::command::CopilotSubCmd;
                match sub {
                    CopilotSubCmd::Status => self.copilot_show_status(),
                    CopilotSubCmd::SignIn => self.copilot_signin(),
                    CopilotSubCmd::SignOut => self.copilot_signout(),
                    CopilotSubCmd::Reload => self.copilot_reload(),
                }
            }
            ExCommand::Quickfix(sub) => {
                use crate::command::QuickfixSubCmd;
                match sub {
                    QuickfixSubCmd::Next => self.qf_next(),
                    QuickfixSubCmd::Prev => self.qf_prev(),
                    QuickfixSubCmd::First => self.qf_first(),
                    QuickfixSubCmd::Last => self.qf_last(),
                    QuickfixSubCmd::List => self.qf_list(),
                    QuickfixSubCmd::Diagnostics => self.qf_load_from_diagnostics(),
                    QuickfixSubCmd::Close => self.qf_close(),
                }
            }
            ExCommand::SpellToggle => self.cmd_spell_toggle(),
            ExCommand::DebugTestNearest => self.cmd_debug_test_nearest(),
            ExCommand::Test(sub) => {
                use crate::command::TestSubCmd;
                match sub {
                    TestSubCmd::Picker => self.cmd_test_picker(),
                    TestSubCmd::Nearest => self.cmd_test_nearest(),
                    TestSubCmd::File => self.cmd_test_file(),
                    TestSubCmd::Last => self.cmd_test_last(),
                    TestSubCmd::Cancel => self.cmd_test_cancel(),
                    TestSubCmd::Results => self.cmd_test_results(),
                }
            }
            ExCommand::Goto(n) => {
                let m = motion::goto_line(&self.buffer, n);
                self.window.cursor = m.target;
            }
            ExCommand::Unknown(s) => {
                self.status_msg = format!("E492: Not an editor command: {s}");
            }
            ExCommand::Invalid(e) => self.status_msg = e,
            // Resolved into plain line numbers above.
            ExCommand::Ranged { .. } => {}
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
                self.cmdline_cursor = 0;
                match kind {
                    crate::mode::PromptKind::Rename => {
                        self.finish_rename(input);
                        self.mode = Mode::Normal;
                        self.rename_anchor = None;
                    }
                    crate::mode::PromptKind::ReplaceAll => {
                        self.finish_replace_all(input);
                        self.mode = Mode::Normal;
                        self.rename_anchor = None;
                    }
                    crate::mode::PromptKind::FileTreeCreate => {
                        // Finisher owns mode transition — it returns
                        // to `FileTree` so the pane stays the focus.
                        self.finish_file_tree_create(input);
                    }
                    crate::mode::PromptKind::FileTreeRename => {
                        self.finish_file_tree_rename(input);
                    }
                    crate::mode::PromptKind::AndroidAvdName => {
                        self.mode = Mode::Normal;
                        self.android_avd_name_entered(input);
                    }
                    crate::mode::PromptKind::DebugConsoleSearch => {
                        self.mode = Mode::DebugPane;
                        self.commit_dap_console_search(input);
                    }
                }
            }
            KeyCode::Backspace => {
                self.cmdline_backspace_at_cursor();
            }
            KeyCode::Delete => {
                self.cmdline_delete_forward_at_cursor();
            }
            KeyCode::Left => {
                self.cmdline_cursor_move_back();
            }
            KeyCode::Right => {
                self.cmdline_cursor_move_forward();
            }
            KeyCode::Home => {
                self.cmdline_cursor = 0;
            }
            KeyCode::End => {
                self.cmdline_cursor = self.cmdline.len();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cmdline_insert_char_at_cursor(c);
            }
            _ => {}
        }
    }

    /// Insert `c` at `cmdline_cursor` and advance past it.
    pub(super) fn cmdline_insert_char_at_cursor(&mut self, c: char) {
        let at = self.cmdline_cursor.min(self.cmdline.len());
        self.cmdline.insert(at, c);
        self.cmdline_cursor = at + c.len_utf8();
    }

    /// Delete the char immediately before `cmdline_cursor`. No-op
    /// when cursor is at start.
    pub(super) fn cmdline_backspace_at_cursor(&mut self) {
        if self.cmdline_cursor == 0 {
            return;
        }
        let mut prev = self.cmdline_cursor;
        // Walk back across the previous char's UTF-8 bytes.
        prev -= 1;
        while prev > 0 && !self.cmdline.is_char_boundary(prev) {
            prev -= 1;
        }
        self.cmdline.drain(prev..self.cmdline_cursor);
        self.cmdline_cursor = prev;
    }

    /// Delete the char at `cmdline_cursor` (forward delete). No-op
    /// when cursor is at end.
    pub(super) fn cmdline_delete_forward_at_cursor(&mut self) {
        if self.cmdline_cursor >= self.cmdline.len() {
            return;
        }
        let mut next = self.cmdline_cursor + 1;
        while next < self.cmdline.len() && !self.cmdline.is_char_boundary(next) {
            next += 1;
        }
        self.cmdline.drain(self.cmdline_cursor..next);
    }

    /// Move `cmdline_cursor` left by one char.
    pub(super) fn cmdline_cursor_move_back(&mut self) {
        if self.cmdline_cursor == 0 {
            return;
        }
        self.cmdline_cursor -= 1;
        while self.cmdline_cursor > 0 && !self.cmdline.is_char_boundary(self.cmdline_cursor) {
            self.cmdline_cursor -= 1;
        }
    }

    /// Move `cmdline_cursor` right by one char.
    pub(super) fn cmdline_cursor_move_forward(&mut self) {
        if self.cmdline_cursor >= self.cmdline.len() {
            return;
        }
        self.cmdline_cursor += 1;
        while self.cmdline_cursor < self.cmdline.len()
            && !self.cmdline.is_char_boundary(self.cmdline_cursor)
        {
            self.cmdline_cursor += 1;
        }
    }

    pub(super) fn cancel_prompt(&mut self) {
        let kind = match self.mode {
            Mode::Prompt(k) => Some(k),
            _ => None,
        };
        self.cmdline.clear();
        self.cmdline_cursor = 0;
        match kind {
            Some(
                crate::mode::PromptKind::FileTreeCreate | crate::mode::PromptKind::FileTreeRename,
            ) => {
                if let Some(state) = self.file_tree.as_mut() {
                    state.pending_op = None;
                }
                self.mode = Mode::FileTree;
            }
            Some(crate::mode::PromptKind::DebugConsoleSearch) => {
                // Esc on the search prompt returns to the pane
                // without overwriting any previously-committed query,
                // so the user can dismiss the box and keep their
                // existing highlights intact.
                self.mode = Mode::DebugPane;
            }
            _ => {
                self.mode = Mode::Normal;
                self.rename_anchor = None;
            }
        }
    }

    /// Resolve an `ExRange` to a 0-based inclusive `(start_line, end_line)` pair,
    /// clamped to the current buffer's bounds.
    fn resolve_range(&self, range: ExRange, default_current: bool) -> (usize, usize) {
        let last = self.buffer.line_count().saturating_sub(1);
        match range {
            ExRange::Implicit => {
                if default_current {
                    (self.window.cursor.line, self.window.cursor.line)
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

    /// A typed range's lines, 0-based: marks, searches and offsets resolved
    /// against the buffer, and a backwards range turned round.
    fn resolve_spec(&self, spec: &command::RangeSpec) -> Result<(usize, usize), String> {
        let cursor = self.window.cursor.line;
        let (first, second) = match spec {
            command::RangeSpec::Whole => return Ok((0, self.last_text_line())),
            command::RangeSpec::Addresses { first, second } => (first, second),
        };
        let a = self.resolve_line_spec(first, cursor)?;
        let Some((second, semicolon)) = second else {
            return Ok((a, a));
        };
        // After `;` the second address counts from the first, not the cursor.
        let from = if *semicolon { a } else { cursor };
        let b = self.resolve_line_spec(second, from)?;
        Ok((a.min(b), a.max(b)))
    }

    /// One address's line, 0-based: `from` is what `.` means and where a
    /// search starts from.
    fn resolve_line_spec(&self, spec: &command::LineSpec, from: usize) -> Result<usize, String> {
        let last = self.last_text_line();
        let base = match &spec.base {
            command::Address::Line(n) => n.saturating_sub(1),
            command::Address::Current => from,
            command::Address::Last => last,
            command::Address::Mark(name) => match self.buffer.mark(*name) {
                Some((line, _)) => line,
                None => return Err("E20: Mark not set".into()),
            },
            command::Address::Search { pattern, backward } => {
                self.search_line(pattern, *backward, from)?
            }
        };
        Ok(base.saturating_add_signed(spec.offset).min(last))
    }

    /// `range`'s lines, 0-based and within the text — or, with a count after
    /// the command, that many lines from the range's last, as Vim reads
    /// `:d 3`.
    fn counted_range(&self, range: ExRange, count: Option<usize>) -> (usize, usize) {
        let last = self.last_text_line();
        let (l1, l2) = self.resolve_range(range, true);
        let (l1, l2) = (l1.min(last), l2.min(last));
        match count {
            Some(n) => (l2, l2.saturating_add(n.saturating_sub(1)).min(last)),
            None => (l1, l2),
        }
    }

    /// Where `:m` / `:t` put lines: the line they'll start at — the one
    /// after the address's, or the top for `0`.
    fn line_target(&self, to: &command::LineSpec) -> Result<usize, String> {
        if to.base == command::Address::Line(0) && to.offset == 0 {
            return Ok(0);
        }
        Ok(self.resolve_line_spec(to, self.window.cursor.line)? + 1)
    }

    /// `:pu[t]` — a register's text on lines of its own below `line`, or
    /// above it, however it was yanked. The cursor ends on its last line.
    fn put_lines(&mut self, line: usize, register: Option<char>, above: bool) {
        let Some(reg) = self
            .read_register(register)
            .filter(|reg| !reg.text.is_empty())
        else {
            let name = register.unwrap_or('"');
            self.status_msg = format!("E353: Nothing in register {name}");
            return;
        };
        let text = reg.text.strip_suffix('\n').unwrap_or(&reg.text);
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let at = if above { line } else { line + 1 };
        self.history.record(&self.buffer.rope, self.window.cursor);
        self.replace_lines(at, at, &lines);
        self.cursor_to_first_non_blank(at + lines.len() - 1);
    }

    fn cursor_to_first_non_blank(&mut self, line: usize) {
        let from = crate::cursor::Cursor {
            line,
            col: 0,
            want_col: 0,
        };
        self.window.cursor = motion::first_non_blank(&self.buffer, from).target;
    }

    /// Runs a `:s` — asking about each match with `c` — and says how it went.
    fn exec_substitute(
        &mut self,
        range: ExRange,
        pattern: &str,
        replacement: &str,
        flags: command::SubFlags,
    ) {
        if flags.confirm && !flags.count_only {
            if let Err(e) = self.substitute_confirm(range, pattern, replacement, flags) {
                self.status_msg = format!("s: {e}");
            }
            return;
        }
        if !flags.count_only {
            self.history.record(&self.buffer.rope, self.window.cursor);
        }
        match self.substitute(range, pattern, replacement, flags) {
            Ok((0, _)) => self.status_msg = format!("Pattern not found: {pattern}"),
            Ok((n, lines)) if flags.count_only => {
                let es = if n == 1 { "" } else { "es" };
                let s = if lines == 1 { "" } else { "s" };
                self.status_msg = format!("{n} match{es} on {lines} line{s}");
            }
            Ok((n, _)) => {
                let s = if n == 1 { "" } else { "s" };
                self.status_msg = format!("{n} substitution{s}");
            }
            Err(e) => self.status_msg = format!("s: {e}"),
        }
    }

    /// `:&` / `:&&` / `:~`, and `&` / `g&`: the last `:s` again on `range` —
    /// with the last search's pattern when `last_search`, its own flags only
    /// when `keep_flags`, and `extra` on top.
    pub(super) fn repeat_substitute(
        &mut self,
        range: ExRange,
        last_search: bool,
        keep_flags: bool,
        extra: command::SubFlags,
    ) {
        let Some(last) = self.last_substitute.clone() else {
            self.status_msg = "E35: No previous regular expression".into();
            return;
        };
        // An empty pattern is the last search, as in `:s//`.
        let pattern = if last_search {
            String::new()
        } else {
            last.pattern
        };
        let base = if keep_flags {
            last.flags
        } else {
            command::SubFlags::default()
        };
        let flags = command::SubFlags {
            global: base.global || extra.global,
            ignore_case: extra.ignore_case.or(base.ignore_case),
            count_only: base.count_only || extra.count_only,
            confirm: base.confirm || extra.confirm,
        };
        self.exec_substitute(range, &pattern, &last.replacement, flags);
    }

    /// `:s` — Vim's pattern and replacement syntax, `flags` applied. How many
    /// matches there were, and on how many lines.
    pub(super) fn substitute(
        &mut self,
        range: ExRange,
        pat: &str,
        repl: &str,
        flags: command::SubFlags,
    ) -> Result<(usize, usize), String> {
        let re = self.substitute_regex(pat, repl, flags)?;
        let groups = super::search::vim_groups(&re);
        Ok(self.replace_matches(
            range,
            &re,
            flags.global,
            flags.count_only,
            &|caps, matched| super::search::expand_replacement(repl, caps, &groups, matched),
        ))
    }

    /// The regex `:s` runs: its pattern, or the last search when that's
    /// empty, with the case flags. As in Vim, it becomes the pattern `n`
    /// repeats, and the whole substitute is what `&` repeats.
    fn substitute_regex(
        &mut self,
        pat: &str,
        repl: &str,
        flags: command::SubFlags,
    ) -> Result<regex::Regex, String> {
        let pattern = if pat.is_empty() {
            match self.last_search.as_ref() {
                Some((last, _)) => last.clone(),
                None => return Err("E35: No previous regular expression".into()),
            }
        } else {
            pat.to_string()
        };
        let cased = super::search::with_case(&pattern, flags.ignore_case);
        let re = super::search::compile_search(&cased)?;
        let backward = self.last_search.as_ref().is_some_and(|(_, back)| *back);
        self.set_search(&pattern, backward);
        self.last_substitute = Some(super::state::LastSubstitute {
            pattern,
            replacement: repl.to_string(),
            flags,
        });
        Ok(re)
    }

    /// Every match of `re` in `range` — the first on each line, or all of
    /// them when `global` — replaced by what `with` makes of it, or only
    /// counted when `count_only`. How many matches, and on how many lines.
    pub(super) fn replace_matches(
        &mut self,
        range: ExRange,
        re: &regex::Regex,
        global: bool,
        count_only: bool,
        with: &dyn Fn(&regex::Captures, &str) -> String,
    ) -> (usize, usize) {
        let (l1, l2) = self.resolve_range(range, true);
        let l2 = l2.min(self.last_text_line());
        let mut total = 0usize;
        let mut lines = 0usize;
        // Iterate bottom-up so edits to lower lines don't shift higher line indices.
        for line in (l1..=l2).rev() {
            let line_start = self.buffer.line_start_idx(line);
            let line_len = self.buffer.line_len(line);
            let line_text: String = self
                .buffer
                .rope
                .slice(line_start..(line_start + line_len))
                .to_string();
            let (new_text, n) = super::search::substitute_line(re, &line_text, global, with);
            if n == 0 {
                continue;
            }
            total += n;
            lines += 1;
            if !count_only {
                self.buffer.delete_range(line_start, line_start + line_len);
                self.buffer.insert_at_idx(line_start, &new_text);
            }
        }
        if total > 0 && !count_only {
            self.window.cursor.line = l1;
            self.window.cursor.col = 0;
            self.window.cursor.want_col = 0;
            self.clamp_cursor_normal();
        }
        (total, lines)
    }

    /// `:s///c` — sets up the walk and asks about the first match. Each
    /// answer comes through `sub_confirm_key`.
    fn substitute_confirm(
        &mut self,
        range: ExRange,
        pat: &str,
        repl: &str,
        flags: command::SubFlags,
    ) -> Result<(), String> {
        let re = self.substitute_regex(pat, repl, flags)?;
        let (l1, l2) = self.resolve_range(range, true);
        self.sub_confirm = Some(super::state::SubConfirm {
            groups: super::search::vim_groups(&re),
            re,
            replacement: repl.to_string(),
            global: flags.global,
            line: l1,
            last_line: l2.min(self.last_text_line()),
            from: 0,
            last_end: None,
            current: None,
            made: 0,
            recorded: false,
        });
        self.sub_confirm_seek();
        if self.sub_confirm.is_none() {
            self.status_msg = format!("Pattern not found: {pat}");
        }
        self.sub_confirm_ask();
        Ok(())
    }

    /// A key while `:s///c` asks about a match: `y` replaces it, `n` passes
    /// it over, `a` replaces it and every one after, `l` replaces it and
    /// stops, `q` / `Esc` stop, and `Ctrl-E` / `Ctrl-Y` scroll.
    fn sub_confirm_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('e'), true) => self.page_scroll(parser::PageScrollKind::LineDown),
            (KeyCode::Char('y'), true) => self.page_scroll(parser::PageScrollKind::LineUp),
            (KeyCode::Char('y'), false) => {
                self.sub_confirm_accept();
                self.sub_confirm_seek();
            }
            (KeyCode::Char('n'), false) => {
                self.sub_confirm_pass();
                self.sub_confirm_seek();
            }
            (KeyCode::Char('a'), false) => {
                while self
                    .sub_confirm
                    .as_ref()
                    .is_some_and(|c| c.current.is_some())
                {
                    self.sub_confirm_accept();
                    self.sub_confirm_seek();
                }
            }
            (KeyCode::Char('l'), false) => {
                self.sub_confirm_accept();
                self.sub_confirm_finish();
            }
            (KeyCode::Char('q'), false) | (KeyCode::Esc, _) => self.sub_confirm_finish(),
            _ => {}
        }
        // Every keypress clears the status line, and the question has to stay.
        self.sub_confirm_ask();
    }

    /// Puts the `:s///c` question on the status line.
    fn sub_confirm_ask(&mut self) {
        if let Some(c) = self.sub_confirm.as_ref() {
            self.status_msg = format!("replace with {} (y/n/a/q/l/^E/^Y)?", c.replacement);
        }
    }

    /// Moves the `:s///c` walk on to the next match and puts the cursor on
    /// it, or ends the walk when there's none left.
    fn sub_confirm_seek(&mut self) {
        while let Some(c) = self.sub_confirm.as_mut() {
            if c.line > c.last_line {
                break;
            }
            let line_start = self.buffer.line_start_idx(c.line);
            let line_len = self.buffer.line_len(c.line);
            let text = self
                .buffer
                .rope
                .slice(line_start..line_start + line_len)
                .to_string();
            let (replacement, groups) = (&c.replacement, &c.groups);
            let hit =
                super::search::next_hit(&c.re, &text, c.from, c.last_end, &|caps, matched| {
                    super::search::expand_replacement(replacement, caps, groups, matched)
                });
            let Some(hit) = hit else {
                c.line += 1;
                c.from = 0;
                c.last_end = None;
                continue;
            };
            let start = line_start + text[..hit.part.0].chars().count();
            let end = line_start + text[..hit.part.1].chars().count();
            c.current = Some(super::state::ConfirmMatch {
                part: hit.part,
                end: hit.whole.1,
                resume: hit.resume(&text),
                chars: (start, end),
                with: hit.with,
            });
            self.cursor_to_idx(start);
            return;
        }
        self.sub_confirm_finish();
    }

    /// Replaces the match `:s///c` is asking about, and moves the walk on to
    /// just past the new text.
    fn sub_confirm_accept(&mut self) {
        let Some(c) = self.sub_confirm.as_mut() else {
            return;
        };
        let Some(m) = c.current.take() else {
            return;
        };
        let first = !c.recorded;
        c.recorded = true;
        c.made += 1;
        let breaks = m.with.matches('\n').count();
        c.last_line += breaks;
        c.line += breaks;
        c.from = match m.with.rfind('\n') {
            Some(at) => m.with.len() - at - 1,
            None => m.part.0 + m.with.len(),
        };
        c.last_end = Some(c.from);
        if !c.global {
            c.line += 1;
            c.from = 0;
            c.last_end = None;
        }
        if first {
            self.history.record(&self.buffer.rope, self.window.cursor);
        }
        self.buffer.delete_range(m.chars.0, m.chars.1);
        self.buffer.insert_at_idx(m.chars.0, &m.with);
    }

    /// Passes over the match `:s///c` is asking about.
    fn sub_confirm_pass(&mut self) {
        let Some(c) = self.sub_confirm.as_mut() else {
            return;
        };
        let Some(m) = c.current.take() else {
            return;
        };
        if c.global {
            c.from = m.resume;
            c.last_end = Some(m.end);
        } else {
            c.line += 1;
            c.from = 0;
            c.last_end = None;
        }
    }

    /// Ends `:s///c`, saying how many replacements it made.
    fn sub_confirm_finish(&mut self) {
        let Some(c) = self.sub_confirm.take() else {
            return;
        };
        let s = if c.made == 1 { "" } else { "s" };
        self.status_msg = format!("{} substitution{s}", c.made);
        self.clamp_cursor_normal();
    }

    fn delete_lines(&mut self, l1: usize, l2: usize, register: Option<char>) {
        let last_line = self.buffer.line_count().saturating_sub(1);
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2 + 1);
        let total = self.buffer.total_chars();
        let extend_back = end == total && l1 > 0;
        let effective_start = if extend_back { start - 1 } else { start };
        let raw = self.buffer.rope.slice(effective_start..end).to_string();
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
        self.write_register(register, reg_text, true);
        self.buffer.delete_range(effective_start, end);
        let new_last = self.buffer.line_count().saturating_sub(1);
        self.window.cursor.line = l1.min(new_last);
        self.window.cursor.col = 0;
        self.window.cursor.want_col = 0;
        self.status_msg = format!("{} lines deleted", l2 - l1 + 1);
        let _ = last_line;
    }

    /// Project-wide substitute. ripgrep enumerates the files that contain
    /// `pattern`, then we walk each, open it into a buffer, apply the
    /// substitution across every line, and save. The originally-active
    /// buffer is restored at the end so the user lands back where they
    /// were. No confirmation prompt — the user has git for safety.
    fn project_substitute(&mut self, pattern: &str, replacement: &str, flags: command::SubFlags) {
        if pattern.is_empty() {
            self.status_msg = "S: empty pattern".into();
            return;
        }
        if flags.confirm {
            self.status_msg = "S: c isn't supported across files".into();
            return;
        }
        // ripgrep runs the same regex engine, so the translated pattern finds
        // exactly the files `:s` would change.
        let cased = super::search::with_case(pattern, flags.ignore_case);
        let source = match super::search::search_source(&cased) {
            Ok(source) => source,
            Err(e) => {
                self.status_msg = format!("S: {e}");
                return;
            }
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // Use ripgrep's --files-with-matches to get the candidate file list.
        let mut rg = std::process::Command::new("rg");
        rg.arg("--files-with-matches").arg("--color=never");
        let files_output = rg
            .arg("--")
            .arg(&source)
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
            if !flags.count_only {
                self.history.record(&self.buffer.rope, self.window.cursor);
            }
            match self.substitute(crate::command::ExRange::Whole, pattern, replacement, flags) {
                Ok((n, _)) if n > 0 => {
                    total_subs += n;
                    files_changed += 1;
                    if !flags.count_only && self.save_active().is_err() {
                        errors += 1;
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    errors += 1;
                }
            }
        }
        if original_active < self.buffers.len() && self.active != original_active {
            let _ = self.switch_to(original_active);
        }
        if flags.count_only && total_subs > 0 {
            let es = if total_subs == 1 { "" } else { "es" };
            let s = if files_changed == 1 { "" } else { "s" };
            self.status_msg = format!("{total_subs} match{es} in {files_changed} file{s}");
            return;
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

    fn yank_lines(&mut self, l1: usize, l2: usize, register: Option<char>) {
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
        self.write_yank_register(register, reg_text, true);
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
            ..Buffer::default()
        }
    }

    fn app_with_keymaps(text: &str, keymaps: &str) -> crate::app::App {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.buffer = buf(text);
        app.mode = Mode::Normal;
        app.window.cursor.line = 0;
        app.window.cursor.col = 0;
        app.config.keymaps = toml::from_str(keymaps).expect("keymaps parse");
        app
    }

    /// Types `keys` into whichever mode the app is in, so `ijk` enters
    /// Insert with `i` and then reaches the Insert handler.
    fn press(app: &mut crate::app::App, keys: &str) {
        for c in keys.chars() {
            let k = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            app.replay_key(k);
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn insert_at(text: &str, line: usize, col: usize) -> crate::app::App {
        let mut app = app_with_keymaps(text, "");
        app.mode = Mode::Insert;
        app.window.cursor.line = line;
        app.window.cursor.col = col;
        app
    }

    fn tap(app: &mut crate::app::App, code: KeyCode) {
        app.replay_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn r_upper_types_over_the_text_and_on_past_the_line_end() {
        let mut app = app_with_keymaps("abc\n", "");
        app.window.cursor.col = 1;
        press(&mut app, "Rxyz");
        assert_eq!(app.buffer.rope.to_string(), "axyz\n");
        tap(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.window.cursor.col, 3);
    }

    #[test]
    fn backspace_in_replace_mode_puts_back_what_was_typed_over() {
        let mut app = app_with_keymaps("abc\n", "");
        app.window.cursor.col = 1;
        press(&mut app, "Rxyz");
        for _ in 0..3 {
            tap(&mut app, KeyCode::Backspace);
        }
        assert_eq!(app.buffer.rope.to_string(), "abc\n");
        assert_eq!(app.window.cursor.col, 1);
        tap(&mut app, KeyCode::Backspace);
        assert_eq!(app.buffer.rope.to_string(), "abc\n");
        assert_eq!(app.window.cursor.col, 0);
    }

    #[test]
    fn a_count_on_r_upper_types_the_text_that_many_times() {
        let mut app = app_with_keymaps("abcdefgh\n", "");
        press(&mut app, "3Rxy");
        tap(&mut app, KeyCode::Esc);
        assert_eq!(app.buffer.rope.to_string(), "xyxyxygh\n");
        assert_eq!(app.window.cursor.col, 5);
    }

    #[test]
    fn dot_repeats_a_replace() {
        let mut app = app_with_keymaps("abcdef\n", "");
        press(&mut app, "Rxy");
        tap(&mut app, KeyCode::Esc);
        press(&mut app, "l.");
        assert_eq!(app.buffer.rope.to_string(), "xyxyef\n");
    }

    #[test]
    fn enter_in_replace_mode_breaks_the_line_and_backspace_joins_it_again() {
        let mut app = app_with_keymaps("abcd\n", "");
        press(&mut app, "Rx");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "y");
        assert_eq!(app.buffer.rope.to_string(), "x\nycd\n");
        tap(&mut app, KeyCode::Backspace);
        tap(&mut app, KeyCode::Backspace);
        assert_eq!(app.buffer.rope.to_string(), "xbcd\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 1));
    }

    #[test]
    fn insert_after_replace_mode_inserts_again() {
        let mut app = app_with_keymaps("ab\n", "");
        press(&mut app, "Rx");
        tap(&mut app, KeyCode::Esc);
        press(&mut app, "iy");
        assert_eq!(app.buffer.rope.to_string(), "yxb\n");
    }

    #[test]
    fn visual_r_replaces_every_selected_char_but_the_line_breaks() {
        let mut app = app_with_keymaps("abc\ndef\n", "");
        app.window.cursor.col = 1;
        app.window.cursor.want_col = 1;
        press(&mut app, "vjrx");
        assert_eq!(app.buffer.rope.to_string(), "axx\nxxf\n");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 1));

        let mut app = app_with_keymaps("abcd\nefgh\n", "");
        app.window.cursor.col = 1;
        app.window.cursor.want_col = 1;
        app.replay_key(ctrl('v'));
        press(&mut app, "jlrx");
        assert_eq!(app.buffer.rope.to_string(), "axxd\nexxh\n");
    }

    #[test]
    fn gp_leaves_the_cursor_just_after_the_put_text() {
        let mut app = app_with_keymaps("ab cd\n", "");
        press(&mut app, "yiwgp");
        assert_eq!(app.buffer.rope.to_string(), "aabb cd\n");
        assert_eq!(app.window.cursor.col, 3);

        let mut app = app_with_keymaps("one\ntwo\n", "");
        press(&mut app, "yygp");
        assert_eq!(app.buffer.rope.to_string(), "one\none\ntwo\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (2, 0));

        let mut app = app_with_keymaps("one\ntwo\n", "");
        press(&mut app, "yygP");
        assert_eq!(app.buffer.rope.to_string(), "one\none\ntwo\n");
        assert_eq!(app.window.cursor.line, 1);
    }

    #[test]
    fn gp_on_the_last_line_stays_on_the_last_line() {
        let mut app = app_with_keymaps("one\ntwo\n", "");
        app.window.cursor.line = 1;
        press(&mut app, "yygp");
        assert_eq!(app.buffer.rope.to_string(), "one\ntwo\ntwo\n");
        assert_eq!(app.window.cursor.line, 2);
    }

    #[test]
    fn bracket_p_puts_lines_at_the_cursor_line_indent() {
        let mut app = app_with_keymaps("x {\n    b\n}\nc\n  d\n", "");
        app.window.cursor.line = 3;
        press(&mut app, "yj");
        app.window.cursor.line = 1;
        press(&mut app, "]p");
        assert_eq!(
            app.buffer.rope.to_string(),
            "x {\n    b\n    c\n      d\n}\nc\n  d\n"
        );
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (2, 4));

        let mut app = app_with_keymaps("x {\n    b\n}\nc\n  d\n", "");
        app.window.cursor.line = 3;
        press(&mut app, "yj");
        app.window.cursor.line = 1;
        press(&mut app, "[p");
        assert_eq!(
            app.buffer.rope.to_string(),
            "x {\n    c\n      d\n    b\n}\nc\n  d\n"
        );
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 4));
    }

    #[test]
    fn visual_ctrl_a_adds_to_the_first_number_on_each_line() {
        let mut app = app_with_keymaps("x 1 5\ny 2\nz 3\n", "");
        press(&mut app, "Vj");
        app.replay_key(ctrl('a'));
        assert_eq!(app.buffer.rope.to_string(), "x 2 5\ny 3\nz 3\n");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 0));
    }

    #[test]
    fn g_ctrl_a_counts_up_line_by_line() {
        let mut app = app_with_keymaps("0\n0\n0\n0\n", "");
        press(&mut app, "Vjjjg");
        app.replay_key(ctrl('a'));
        assert_eq!(app.buffer.rope.to_string(), "1\n2\n3\n4\n");

        let mut app = app_with_keymaps("0\n0\n0\n", "");
        press(&mut app, "Vjj2g");
        app.replay_key(ctrl('a'));
        assert_eq!(app.buffer.rope.to_string(), "2\n4\n6\n");
    }

    #[test]
    fn visual_ctrl_x_only_sees_the_selected_columns() {
        let mut app = app_with_keymaps("1 1\n1 1\n", "");
        app.window.cursor.col = 2;
        app.window.cursor.want_col = 2;
        app.replay_key(ctrl('v'));
        press(&mut app, "j");
        app.replay_key(ctrl('x'));
        assert_eq!(app.buffer.rope.to_string(), "1 0\n1 0\n");

        let mut app = app_with_keymaps("5 7\n", "");
        app.window.cursor.col = 2;
        app.window.cursor.want_col = 2;
        press(&mut app, "v");
        app.replay_key(ctrl('a'));
        assert_eq!(app.buffer.rope.to_string(), "5 8\n");
    }

    #[test]
    fn double_equals_and_equals_ip_reindent_by_the_block_structure() {
        let mut app = app_with_keymaps("fn a() {\nx\n}\n", "");
        let u = app.editorconfig.indent_string();
        app.window.cursor.line = 1;
        press(&mut app, "==");
        assert_eq!(
            app.buffer.rope.to_string(),
            format!("fn a() {{\n{u}x\n}}\n")
        );

        let mut app = app_with_keymaps("if a {\nb\nif c {\nd\n      }\n}\n", "");
        press(&mut app, "=ip");
        assert_eq!(
            app.buffer.rope.to_string(),
            format!("if a {{\n{u}b\n{u}if c {{\n{u}{u}d\n{u}}}\n}}\n")
        );
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 0));
    }

    #[test]
    fn equals_percent_and_visual_equals_reindent() {
        let mut app = app_with_keymaps("x {\n      y\n  }\n", "");
        let u = app.editorconfig.indent_string();
        app.window.cursor.col = 2;
        app.window.cursor.want_col = 2;
        press(&mut app, "=%");
        assert_eq!(app.buffer.rope.to_string(), format!("x {{\n{u}y\n}}\n"));

        let mut app = app_with_keymaps("a {\nb\n", "");
        press(&mut app, "Vj=");
        assert_eq!(app.buffer.rope.to_string(), format!("a {{\n{u}b\n"));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn dot_repeats_a_reindent() {
        let mut app = app_with_keymaps("a {\nb\nc\n}\n", "");
        let u = app.editorconfig.indent_string();
        app.window.cursor.line = 1;
        press(&mut app, "==j.");
        assert_eq!(
            app.buffer.rope.to_string(),
            format!("a {{\n{u}b\n{u}c\n}}\n")
        );
    }

    #[test]
    fn reindent_lines_up_block_comments_and_empties_blank_lines() {
        let mut app = app_with_keymaps("/*\n* a\n*/\n  \nx\n", "");
        press(&mut app, "Vjjjj=");
        assert_eq!(app.buffer.rope.to_string(), "/*\n * a\n */\n\nx\n");
    }

    #[test]
    fn gqq_and_gqip_fill_to_the_text_width() {
        let mut app = app_with_keymaps("aa bb cc dd\n", "");
        app.editorconfig.max_line_length = Some(8);
        press(&mut app, "gqq");
        assert_eq!(app.buffer.rope.to_string(), "aa bb cc\ndd\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 0));

        let mut app = app_with_keymaps("a\nb\nc\n\nd\n", "");
        press(&mut app, "gqip");
        assert_eq!(app.buffer.rope.to_string(), "a b c\n\nd\n");
    }

    #[test]
    fn gw_re_flows_and_leaves_the_cursor_where_it_was() {
        let mut app = app_with_keymaps("aa bb cc dd ee\n", "");
        app.editorconfig.max_line_length = Some(8);
        app.window.cursor.col = 3;
        app.window.cursor.want_col = 3;
        press(&mut app, "gww");
        assert_eq!(app.buffer.rope.to_string(), "aa bb cc\ndd ee\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 3));
    }

    #[test]
    fn visual_gq_and_dot_repeat_re_flow() {
        let mut app = app_with_keymaps("a\nb\n", "");
        press(&mut app, "Vjgq");
        assert_eq!(app.buffer.rope.to_string(), "a b\n");
        assert_eq!(app.mode, Mode::Normal);

        let mut app = app_with_keymaps("a\nb\n\nc\nd\n", "");
        press(&mut app, "gqjjj.");
        assert_eq!(app.buffer.rope.to_string(), "a b\n\nc d\n");
    }

    #[test]
    fn gq_keeps_a_comment_marker_on_every_line() {
        let mut app = app_with_keymaps("// aa bb\n// cc\n", "");
        app.buffer.path = Some(std::path::PathBuf::from("x.rs"));
        press(&mut app, "gqj");
        assert_eq!(app.buffer.rope.to_string(), "// aa bb cc\n");
    }

    #[test]
    fn bang_opens_the_command_line_with_the_range() {
        let mut app = app_with_keymaps("c\nb\na\n\nz\n", "");
        press(&mut app, "!ip");
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.cmdline, "1,3!");

        let mut app = app_with_keymaps("a\nb\n", "");
        app.window.cursor.line = 1;
        press(&mut app, "!!");
        assert_eq!(app.cmdline, "2!");

        let mut app = app_with_keymaps("a\nb\nc\n", "");
        press(&mut app, "Vj!");
        assert_eq!(app.cmdline, "1,2!");
        assert_eq!(app.mode, Mode::Command);
    }

    #[test]
    fn an_empty_filter_command_leaves_the_lines_alone() {
        let mut app = app_with_keymaps("keep\n", "");
        press(&mut app, "!!");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.buffer.rope.to_string(), "keep\n");
        assert!(app.status_msg.contains("E34"), "{}", app.status_msg);
    }

    #[cfg(unix)]
    #[test]
    fn a_filter_replaces_the_lines_with_what_the_command_prints() {
        let mut app = app_with_keymaps("c\nb\na\n\nz\n", "");
        press(&mut app, "!ipsort");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.buffer.rope.to_string(), "a\nb\nc\n\nz\n");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_filter_leaves_the_lines_alone_and_says_why() {
        let mut app = app_with_keymaps("keep\n", "");
        press(&mut app, "!!echo bad >&2; exit 3");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.buffer.rope.to_string(), "keep\n");
        assert!(app.status_msg.contains("exit 3: bad"), "{}", app.status_msg);
    }

    #[test]
    fn d_close_paren_deletes_to_the_next_sentence() {
        let mut app = app_with_keymaps("One. Two.\n", "");
        press(&mut app, "d)");
        assert_eq!(app.buffer.rope.to_string(), "Two.\n");

        // From a line's start to a sentence at a later line's start, `d)`
        // takes whole lines, as Vim's exclusive-linewise rule has it.
        let mut app = app_with_keymaps("a b\n\nc\n", "");
        press(&mut app, "d)");
        assert_eq!(app.buffer.rope.to_string(), "\nc\n");
    }

    #[test]
    fn das_and_cis_work_on_sentences() {
        let mut app = app_with_keymaps("One. Two. Three.\n", "");
        app.window.cursor.col = 5;
        app.window.cursor.want_col = 5;
        press(&mut app, "das");
        assert_eq!(app.buffer.rope.to_string(), "One. Three.\n");

        let mut app = app_with_keymaps("One. Two. Three.\n", "");
        app.window.cursor.col = 5;
        app.window.cursor.want_col = 5;
        press(&mut app, "cisX.");
        tap(&mut app, KeyCode::Esc);
        assert_eq!(app.buffer.rope.to_string(), "One. X. Three.\n");
    }

    #[test]
    fn dit_empties_an_element_and_again_does_nothing() {
        let mut app = app_with_keymaps("<p>hello</p>\n", "");
        app.window.cursor.col = 4;
        app.window.cursor.want_col = 4;
        press(&mut app, "dit");
        assert_eq!(app.buffer.rope.to_string(), "<p></p>\n");
        press(&mut app, "dit");
        assert_eq!(app.buffer.rope.to_string(), "<p></p>\n");
        let unnamed = app.read_register(None).map(|r| r.text);
        assert_eq!(unnamed.as_deref(), Some("hello"));
    }

    #[test]
    fn daa_and_cia_work_on_arguments() {
        let mut app = app_with_keymaps("call(one, two)\n", "");
        app.window.cursor.col = 10;
        app.window.cursor.want_col = 10;
        press(&mut app, "daa");
        assert_eq!(app.buffer.rope.to_string(), "call(one)\n");

        let mut app = app_with_keymaps("call(one, two)\n", "");
        app.window.cursor.col = 5;
        app.window.cursor.want_col = 5;
        press(&mut app, "cia1");
        tap(&mut app, KeyCode::Esc);
        assert_eq!(app.buffer.rope.to_string(), "call(1, two)\n");
    }

    fn with_diagnostics(text: &str, spots: &[(usize, usize, &str)]) -> crate::app::App {
        let mut app = app_with_keymaps(text, "");
        let path = std::path::PathBuf::from("diag.rs");
        app.buffer.path = Some(path.clone());
        let diags = spots
            .iter()
            .map(|&(line, col, message)| crate::lsp::Diagnostic {
                line,
                col,
                end_line: line,
                end_col: col + 1,
                severity: crate::lsp::Severity::Error,
                message: message.to_string(),
            })
            .collect();
        app.lsp.diagnostics.insert(path, diags);
        app
    }

    #[test]
    fn bracket_d_jumps_between_diagnostics_and_shows_them() {
        let mut app = with_diagnostics("a\nb\nc\nd\n", &[(2, 0, "second\nmore"), (1, 0, "first")]);
        press(&mut app, "]d");
        assert_eq!(app.window.cursor.line, 1);
        assert_eq!(app.status_msg, "error: first");
        press(&mut app, "]d");
        assert_eq!(app.window.cursor.line, 2);
        assert_eq!(app.status_msg, "error: second");
        press(&mut app, "]d");
        assert_eq!(app.window.cursor.line, 1);
        press(&mut app, "[d");
        assert_eq!(app.window.cursor.line, 2);
    }

    #[test]
    fn unmatched_bracket_motions_move_and_take_operators() {
        // f0 (1 a2 ,3 _4 (5 b6 )7 ,8 _9 c10 )11
        let mut app = app_with_keymaps("f(a, (b), c)\n", "");
        app.window.cursor.col = 10;
        app.window.cursor.want_col = 10;
        press(&mut app, "[(");
        assert_eq!(app.window.cursor.col, 1);

        let mut app = app_with_keymaps("f(a, (b), c)\n", "");
        app.window.cursor.col = 2;
        app.window.cursor.want_col = 2;
        press(&mut app, "d])");
        assert_eq!(app.buffer.rope.to_string(), "f()\n");
    }

    #[test]
    fn daf_takes_the_function_and_says_when_it_cannot() {
        let mut app = app_with_keymaps("fn a() {\n    1\n}\n\nfn b() {}\n", "");
        app.buffer.path = Some(std::path::PathBuf::from("x.rs"));
        app.window.cursor.line = 1;
        press(&mut app, "daf");
        assert_eq!(app.buffer.rope.to_string(), "\nfn b() {}\n");

        let mut app = app_with_keymaps("def f\nend\n", "");
        app.buffer.path = Some(std::path::PathBuf::from("x.rb"));
        press(&mut app, "daf");
        assert_eq!(app.buffer.rope.to_string(), "def f\nend\n");
        assert!(
            app.status_msg.contains("no function objects"),
            "{}",
            app.status_msg
        );
    }

    #[test]
    fn slash_searches_by_regex_and_n_takes_a_count() {
        // x0 _1 a2 1 _4 b5 _6 a7 2 2 _10 c11 _12 a13 3 3 3
        let mut app = app_with_keymaps("x a1 b a22 c a333\n", "");
        press(&mut app, "/a\\d\\+");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.col, 2);
        press(&mut app, "2n");
        assert_eq!(app.window.cursor.col, 13);
        let highlighted = app.line_search_matches_in(&app.buffer, 0);
        assert_eq!(highlighted, vec![(2, 4), (7, 10), (13, 17)]);
    }

    #[test]
    fn slash_moves_past_a_match_the_cursor_is_on() {
        let mut app = app_with_keymaps("ab ab\n", "");
        press(&mut app, "/ab");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.col, 3);
    }

    #[test]
    fn star_takes_whole_words_and_g_star_does_not() {
        let mut app = app_with_keymaps("foo food foo\n", "");
        press(&mut app, "*");
        assert_eq!(app.window.cursor.col, 9);

        let mut app = app_with_keymaps("foo food foo\n", "");
        press(&mut app, "g*");
        assert_eq!(app.window.cursor.col, 4);

        let mut app = app_with_keymaps("foo x foo\n", "");
        app.window.cursor.col = 7;
        app.window.cursor.want_col = 7;
        press(&mut app, "#");
        assert_eq!(app.window.cursor.col, 0);
    }

    #[test]
    fn a_search_offset_moves_off_the_match_and_n_keeps_it() {
        // f0 o1 o2 _3 b4 a5 r6 _7 f8 o9 o10
        let mut app = app_with_keymaps("foo bar foo\n", "");
        press(&mut app, "/foo/e");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.col, 2);
        press(&mut app, "n");
        assert_eq!(app.window.cursor.col, 10);
        press(&mut app, "n");
        assert_eq!(app.window.cursor.col, 2);
        // `*` starts afresh, with no offset.
        press(&mut app, "*");
        assert_eq!(app.window.cursor.col, 8);
        press(&mut app, "n");
        assert_eq!(app.window.cursor.col, 0);

        for (query, col) in [("/bar/e+1", 7), ("/bar/s-1", 3), ("/bar/b+1", 5)] {
            let mut app = app_with_keymaps("foo bar foo\n", "");
            press(&mut app, query);
            tap(&mut app, KeyCode::Enter);
            assert_eq!(app.window.cursor.col, col, "{query}");
        }

        // A step off a line's end lands on the next line.
        let mut app = app_with_keymaps("ab\ncd\n", "");
        press(&mut app, "/b/e+1");
        tap(&mut app, KeyCode::Enter);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 0));

        let mut app = app_with_keymaps("foo bar foo\n", "");
        app.window.cursor.col = 10;
        app.window.cursor.want_col = 10;
        press(&mut app, "?foo?e");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.col, 2);
    }

    #[test]
    fn a_line_offset_lands_on_the_first_non_blank_lines_away() {
        let mut app = app_with_keymaps("x\n  one\nx\n  two\n", "");
        press(&mut app, "/x/+1");
        tap(&mut app, KeyCode::Enter);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 2));
        press(&mut app, "n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (3, 2));
        press(&mut app, "N");
        assert_eq!(app.window.cursor.line, 1);

        let mut app = app_with_keymaps("x\nlast\n", "");
        press(&mut app, "/x/+5");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.line, 1);
    }

    #[test]
    fn double_slash_drops_the_offset_and_a_bare_slash_keeps_it() {
        // f0 o1 o2 _3 f4 o5 o6 _7 f8 o9 o10
        let mut app = app_with_keymaps("foo foo foo\n", "");
        let search = |app: &mut crate::app::App, query: &str| {
            press(app, query);
            tap(app, KeyCode::Enter);
            app.window.cursor.col
        };
        assert_eq!(search(&mut app, "/foo/e"), 2);
        assert_eq!(search(&mut app, "//"), 4);
        assert_eq!(search(&mut app, "/"), 8);
        assert_eq!(search(&mut app, "//e"), 10);
        assert_eq!(search(&mut app, "/"), 2);
    }

    #[test]
    fn an_operator_takes_the_search_offset_into_account() {
        let mut app = app_with_keymaps("foo bar foo\n", "");
        press(&mut app, "/bar/e");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "0dn");
        assert_eq!(app.buffer.rope.to_string(), " foo\n");

        let mut app = app_with_keymaps("x\none\nx\ntwo\n", "");
        press(&mut app, "/x/+1");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "ggdn");
        assert_eq!(app.buffer.rope.to_string(), "x\ntwo\n");
    }

    #[test]
    fn an_offset_vim_would_not_read_is_an_error() {
        let mut app = app_with_keymaps("foo bar\n", "");
        press(&mut app, "/bar/x");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.window.cursor.col, 0);
        assert!(app.status_msg.contains("E488"), "{}", app.status_msg);
    }

    #[test]
    fn cgn_then_dot_changes_one_match_after_another() {
        let mut app = app_with_keymaps("foo bar foo baz foo\n", "");
        press(&mut app, "/foo");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "0cgnX");
        tap(&mut app, KeyCode::Esc);
        assert_eq!(app.buffer.rope.to_string(), "X bar foo baz foo\n");
        press(&mut app, ".");
        assert_eq!(app.buffer.rope.to_string(), "X bar X baz foo\n");
        press(&mut app, ".");
        assert_eq!(app.buffer.rope.to_string(), "X bar X baz X\n");

        let mut app = app_with_keymaps("a1 b a2 a3\n", "");
        press(&mut app, "/a\\d");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "dgn.");
        assert_eq!(app.buffer.rope.to_string(), "a1 b  \n");
    }

    #[test]
    fn gn_selects_the_match_and_stretches_a_selection_to_the_next() {
        // f0 o1 o2 _3 b4 a5 r6 _7 f8 o9 o10
        let mut app = app_with_keymaps("foo bar foo\n", "");
        press(&mut app, "/foo");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "0gn");
        assert_eq!(app.mode, Mode::Visual(crate::mode::VisualKind::Char));
        assert_eq!(app.window.visual_anchor.map(|a| a.col), Some(0));
        assert_eq!(app.window.cursor.col, 2);
        press(&mut app, "gn");
        assert_eq!(app.window.visual_anchor.map(|a| a.col), Some(0));
        assert_eq!(app.window.cursor.col, 10);
        tap(&mut app, KeyCode::Esc);

        app.window.cursor.col = 5;
        app.window.cursor.want_col = 5;
        press(&mut app, "gN");
        assert_eq!(app.window.visual_anchor.map(|a| a.col), Some(2));
        assert_eq!(app.window.cursor.col, 0);
    }

    #[test]
    fn gn_without_a_search_says_so() {
        let mut app = app_with_keymaps("foo\n", "");
        press(&mut app, "dgn");
        assert_eq!(app.buffer.rope.to_string(), "foo\n");
        assert!(app.status_msg.contains("E35"), "{}", app.status_msg);
    }

    #[test]
    fn substitute_takes_vim_patterns_and_replacements() {
        let mut app = app_with_keymaps("one two\nthree four\n", "");
        app.exec_command("%s/\\(\\w\\+\\) \\(\\w\\+\\)/\\2 \\1/");
        assert_eq!(app.buffer.rope.to_string(), "two one\nfour three\n");
        assert_eq!(app.status_msg, "2 substitutions");

        let mut app = app_with_keymaps("Foo foo\n", "");
        app.exec_command("s/foo/[&]/g");
        assert_eq!(app.buffer.rope.to_string(), "[Foo] [foo]\n");
        let mut app = app_with_keymaps("Foo foo\n", "");
        app.exec_command("s/foo/[&]/gI");
        assert_eq!(app.buffer.rope.to_string(), "Foo [foo]\n");

        let mut app = app_with_keymaps("a\n\nb\n", "");
        app.exec_command("%s/^/# /");
        assert_eq!(app.buffer.rope.to_string(), "# a\n# \n# b\n");
    }

    #[test]
    fn substitute_n_counts_and_changes_nothing() {
        let mut app = app_with_keymaps("a a\nb\na\n", "");
        app.exec_command("%s/a/x/gn");
        assert_eq!(app.buffer.rope.to_string(), "a a\nb\na\n");
        assert_eq!(app.status_msg, "3 matches on 2 lines");
    }

    #[test]
    fn substitute_shares_its_pattern_with_search() {
        let mut app = app_with_keymaps("cat dog cat\n", "");
        press(&mut app, "/dog");
        tap(&mut app, KeyCode::Enter);
        app.exec_command("s//bird/");
        assert_eq!(app.buffer.rope.to_string(), "cat bird cat\n");
        app.exec_command("s/cat/cow/");
        assert_eq!(app.buffer.rope.to_string(), "cow bird cat\n");
        // c0 o1 w2 _3 b4 i5 r6 d7 _8 c9
        press(&mut app, "n");
        assert_eq!(app.window.cursor.col, 9);
    }

    #[test]
    fn substitute_flags_it_does_not_know_are_an_error() {
        let mut app = app_with_keymaps("a\n", "");
        app.exec_command("s/a/b/q");
        assert_eq!(app.buffer.rope.to_string(), "a\n");
        assert!(app.status_msg.contains("E488"), "{}", app.status_msg);
    }

    #[test]
    fn substitute_c_asks_about_each_match_and_undoes_as_one() {
        let mut app = app_with_keymaps("a a\na\n", "");
        app.exec_command("%s/a/b/gc");
        assert!(
            app.status_msg.starts_with("replace with b"),
            "{}",
            app.status_msg
        );
        assert_eq!(app.line_current_match(0), Some((0, 1)));
        press(&mut app, "yn");
        assert_eq!(app.line_current_match(1), Some((0, 1)));
        press(&mut app, "y");
        assert_eq!(app.buffer.rope.to_string(), "b a\nb\n");
        assert_eq!(app.status_msg, "2 substitutions");
        press(&mut app, "u");
        assert_eq!(app.buffer.rope.to_string(), "a a\na\n");
    }

    #[test]
    fn substitute_c_takes_all_last_and_quit() {
        let run = |keys: &str| {
            let mut app = app_with_keymaps("x x x\n", "");
            app.exec_command("s/x/y/gc");
            press(&mut app, keys);
            (app.buffer.rope.to_string(), app.line_current_match(0))
        };
        assert_eq!(run("a"), ("y y y\n".to_string(), None));
        assert_eq!(run("nl"), ("x y x\n".to_string(), None));
        assert_eq!(run("yq"), ("y x x\n".to_string(), None));

        // Without `g`, one match a line.
        let mut app = app_with_keymaps("a a\na\n", "");
        app.exec_command("%s/a/b/c");
        press(&mut app, "yy");
        assert_eq!(app.buffer.rope.to_string(), "b a\nb\n");

        // A line break in the replacement moves the rest of the walk down.
        let mut app = app_with_keymaps("a,b,c\n", "");
        app.exec_command("s/,/\\r/gc");
        press(&mut app, "yy");
        assert_eq!(app.buffer.rope.to_string(), "a\nb\nc\n");
    }

    #[test]
    fn ampersand_runs_the_last_substitute_again() {
        let mut app = app_with_keymaps("a a\na a\na a\n", "");
        app.exec_command("s/a/b/g");
        assert_eq!(app.buffer.rope.to_string(), "b b\na a\na a\n");
        // `:&&` keeps the flags...
        app.exec_command("2&&");
        assert_eq!(app.buffer.rope.to_string(), "b b\nb b\na a\n");
        // ...and `&` drops them, so only the first `a` on the line changes.
        press(&mut app, "j&");
        assert_eq!(app.buffer.rope.to_string(), "b b\nb b\nb a\n");

        // `g&` is `:%s//~/&`: the last search, the last replacement and flags.
        let mut app = app_with_keymaps("x y\ny x\n", "");
        app.exec_command("s/x/z/g");
        press(&mut app, "/y");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "g&");
        assert_eq!(app.buffer.rope.to_string(), "z z\nz x\n");

        // `:~` takes the last search's pattern with the last replacement.
        let mut app = app_with_keymaps("ab ab\n", "");
        app.exec_command("s/a/1/");
        press(&mut app, "/b");
        tap(&mut app, KeyCode::Enter);
        press(&mut app, "0");
        app.exec_command("~");
        assert_eq!(app.buffer.rope.to_string(), "11 ab\n");

        let mut app = app_with_keymaps("a\n", "");
        press(&mut app, "&");
        assert!(app.status_msg.contains("E35"), "{}", app.status_msg);
    }

    #[test]
    fn a_search_being_typed_moves_to_its_first_match_and_esc_goes_back() {
        let mut app = app_with_keymaps("one\ntwo\nthree two\n", "");
        press(&mut app, "/tw");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 0));
        assert_eq!(app.line_current_match(1), Some((0, 2)));
        assert_eq!(app.line_search_matches_in(&app.buffer, 2), vec![(6, 8)]);
        // Nothing matches `twx`, so the cursor goes back to where it began.
        press(&mut app, "x");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 0));
        tap(&mut app, KeyCode::Backspace);
        assert_eq!(app.window.cursor.line, 1);
        tap(&mut app, KeyCode::Esc);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 0));
        assert!(app.last_search.is_none());
        assert_eq!(app.line_current_match(1), None);

        // Half a pattern previews nothing and reports nothing.
        press(&mut app, "/\\(");
        assert_eq!(app.window.cursor.line, 0);
        assert!(!app.status_msg.contains("Invalid"), "{}", app.status_msg);
        tap(&mut app, KeyCode::Esc);

        // Enter searches from where the typing began.
        press(&mut app, "/two");
        tap(&mut app, KeyCode::Enter);
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 0));
        press(&mut app, "n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (2, 6));
    }

    #[test]
    fn ranges_resolve_marks_searches_and_offsets() {
        let text = "a\nb\nc\nd\ne\nf\n";
        let run = |cmd: &str| {
            let mut app = app_with_keymaps(text, "");
            app.buffer.set_mark('x', 4, 0);
            app.exec_command(cmd);
            let after = app.buffer.rope.to_string();
            (after, app.window.cursor.line, app.status_msg.clone())
        };
        assert_eq!(run(".,+1d").0, "c\nd\ne\nf\n");
        assert_eq!(run("/c/,$-1d").0, "a\nb\nf\n");
        // `;` counts the second address from the first: `'x` is `e`, one up is `d`.
        assert_eq!(run("'x;-1d").0, "a\nb\nc\nf\n");
        // A range alone goes to its line; `?e?` wraps round from the top.
        assert_eq!(run("?e?").1, 4);
        assert_eq!(run("$").1, 5);
        // Plain number ranges are as they were.
        assert_eq!(run("2,3d").0, "a\nd\ne\nf\n");
        let (after, _, status) = run("'q,.d");
        assert_eq!(after, text);
        assert!(status.contains("E20"), "{status}");
        let (after, _, status) = run("/zz/d");
        assert_eq!(after, text);
        assert!(status.contains("E486"), "{status}");
    }

    #[test]
    fn colon_on_a_selection_fills_in_its_lines() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        press(&mut app, "jVj:");
        assert_eq!(app.cmdline, "'<,'>");
        press(&mut app, "d");
        tap(&mut app, KeyCode::Enter);
        assert_eq!(app.buffer.rope.to_string(), "a\nd\n");
    }

    #[test]
    fn line_commands_move_copy_join_and_put() {
        let run = |text: &str, cmds: &[&str]| {
            let mut app = app_with_keymaps(text, "");
            for cmd in cmds {
                app.exec_command(cmd);
            }
            (app.buffer.rope.to_string(), app.status_msg.clone())
        };
        let text = "a\nb\nc\nd\n";
        assert_eq!(run(text, &["1m$"]).0, "b\nc\nd\na\n");
        assert_eq!(run(text, &["3,4m0"]).0, "c\nd\na\nb\n");
        assert_eq!(run(text, &["2t."]).0, "a\nb\nb\nc\nd\n");
        assert_eq!(run(text, &["1,2co$"]).0, "a\nb\nc\nd\na\nb\n");
        // A last line with no line break still has none after the move.
        assert_eq!(run("a\nb\nc", &["1m$"]).0, "b\nc\na");
        let (after, status) = run(text, &["2,3m2"]);
        assert_eq!(after, text);
        assert!(status.contains("E134"), "{status}");
        assert_eq!(run(text, &["1,3j"]).0, "a b c\nd\n");
        assert_eq!(run(text, &["j!"]).0, "ab\nc\nd\n");
        assert_eq!(
            run(text, &["2y a", "$pu a", "1pu! a"]).0,
            "b\na\nb\nc\nd\nb\n"
        );
        assert_eq!(run(text, &["2d b 2", "1pu b"]).0, "a\nb\nc\nd\n");
        assert!(run(text, &["pu q"]).1.contains("E353"));
    }

    #[test]
    fn line_commands_shift_align_and_retab() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        let unit = app.editorconfig.indent_string();
        app.exec_command("2,3>>");
        let want = format!("a\n{unit}{unit}b\n{unit}{unit}c\nd\n");
        assert_eq!(app.buffer.rope.to_string(), want);

        let mut app = app_with_keymaps("  ab\nabcd\n\n", "");
        app.editorconfig.indent_style = crate::editorconfig::IndentStyle::Spaces;
        app.editorconfig.tab_width = 4;
        app.exec_command("%ri 6");
        assert_eq!(app.buffer.rope.to_string(), "    ab\n  abcd\n\n");
        app.exec_command("%ce 8");
        assert_eq!(app.buffer.rope.to_string(), "   ab\n  abcd\n\n");
        app.exec_command("%le 1");
        assert_eq!(app.buffer.rope.to_string(), " ab\n abcd\n\n");

        let mut app = app_with_keymaps("\tx\n  \ty\na  b\n", "");
        app.editorconfig.indent_style = crate::editorconfig::IndentStyle::Spaces;
        app.editorconfig.tab_width = 4;
        app.exec_command("retab");
        assert_eq!(app.buffer.rope.to_string(), "    x\n    y\na  b\n");
        app.editorconfig.indent_style = crate::editorconfig::IndentStyle::Tabs;
        app.exec_command("retab! 2");
        assert_eq!(app.buffer.rope.to_string(), "\t\tx\n\t\ty\na\t b\n");
        assert_eq!(app.editorconfig.tab_width, 2);
    }

    #[test]
    fn normal_types_its_keys_on_every_line() {
        let mut app = app_with_keymaps("a\nb\nc\n", "");
        app.exec_command("%norm Ax");
        assert_eq!(app.buffer.rope.to_string(), "ax\nbx\ncx\n");
        assert_eq!(app.mode, Mode::Normal);
        // Each line undoes on its own.
        press(&mut app, "u");
        assert_eq!(app.buffer.rope.to_string(), "ax\nbx\nc\n");

        // Without a range, once where the cursor is.
        let mut app = app_with_keymaps("abc\n", "");
        app.window.cursor.col = 1;
        app.window.cursor.want_col = 1;
        app.exec_command("norm x");
        assert_eq!(app.buffer.rope.to_string(), "ac\n");

        // An operator left waiting is dropped, not left pending.
        let mut app = app_with_keymaps("abc\n", "");
        app.exec_command("norm d");
        press(&mut app, "l");
        assert_eq!(app.buffer.rope.to_string(), "abc\n");

        // Blanks at the end of the keys are typed too.
        let mut app = app_with_keymaps("a\n", "");
        app.exec_command("norm A  ");
        assert_eq!(app.buffer.rope.to_string(), "a  \n");
    }

    #[test]
    fn normal_bang_leaves_the_keymaps_out() {
        let keymaps = "[normal]\nx = \"dd\"";
        let mut app = app_with_keymaps("ab\ncd\n", keymaps);
        app.exec_command("norm x");
        assert_eq!(app.buffer.rope.to_string(), "cd\n");
        let mut app = app_with_keymaps("ab\ncd\n", keymaps);
        app.exec_command("norm! x");
        assert_eq!(app.buffer.rope.to_string(), "b\ncd\n");
    }

    #[test]
    fn ctrl_w_deletes_the_previous_word() {
        let mut app = insert_at("foo bar\n", 0, 7);
        app.replay_key(ctrl('w'));
        assert_eq!(app.buffer.rope.to_string(), "foo \n");
        assert_eq!(app.window.cursor.col, 4);
    }

    #[test]
    fn ctrl_w_takes_trailing_whitespace_with_the_word() {
        let mut app = insert_at("foo bar  \n", 0, 9);
        app.replay_key(ctrl('w'));
        assert_eq!(app.buffer.rope.to_string(), "foo \n");
    }

    #[test]
    fn ctrl_w_at_column_zero_joins_the_previous_line() {
        let mut app = insert_at("foo\nbar\n", 1, 0);
        app.replay_key(ctrl('w'));
        assert_eq!(app.buffer.rope.to_string(), "foobar\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 3));
    }

    #[test]
    fn ctrl_u_keeps_the_indent_on_the_first_press() {
        let mut app = insert_at("    foo bar\n", 0, 11);
        app.replay_key(ctrl('u'));
        assert_eq!(app.buffer.rope.to_string(), "    \n");
        assert_eq!(app.window.cursor.col, 4);
    }

    #[test]
    fn ctrl_u_from_the_indent_takes_the_indent() {
        let mut app = insert_at("    foo bar\n", 0, 11);
        app.replay_key(ctrl('u'));
        app.replay_key(ctrl('u'));
        assert_eq!(app.buffer.rope.to_string(), "\n");
        assert_eq!(app.window.cursor.col, 0);
    }

    #[test]
    fn ctrl_u_at_column_zero_joins_the_previous_line() {
        let mut app = insert_at("foo\nbar\n", 1, 0);
        app.replay_key(ctrl('u'));
        assert_eq!(app.buffer.rope.to_string(), "foobar\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 3));
    }

    #[test]
    fn ctrl_o_runs_one_normal_command_then_returns_to_insert() {
        let mut app = insert_at("one\ntwo\nthree\n", 1, 1);
        app.replay_key(ctrl('o'));
        assert_eq!(app.mode, Mode::Normal);
        press(&mut app, "d");
        assert_eq!(app.mode, Mode::Normal, "d still waits for its motion");
        press(&mut app, "d");
        assert_eq!(app.buffer.rope.to_string(), "one\nthree\n");
        assert_eq!(app.mode, Mode::Insert);
        assert!(app.insert_oneshot.is_none());
    }

    #[test]
    fn ctrl_o_at_end_of_line_resumes_past_the_last_char() {
        let mut app = insert_at("foo\n", 0, 3);
        app.replay_key(ctrl('o'));
        assert_eq!(app.window.cursor.col, 2);
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!((app.mode, app.window.cursor.col), (Mode::Insert, 3));
        press(&mut app, "x");
        assert_eq!(app.buffer.rope.to_string(), "foox\n");
    }

    #[test]
    fn ctrl_o_keeps_the_column_a_motion_leaves() {
        let mut app = insert_at("foo bar\n", 0, 7);
        app.replay_key(ctrl('o'));
        press(&mut app, "0");
        assert_eq!((app.mode, app.window.cursor.col), (Mode::Insert, 0));
    }

    #[test]
    fn ctrl_o_dollar_resumes_past_the_end() {
        let mut app = insert_at("foo bar\n", 0, 0);
        app.replay_key(ctrl('o'));
        press(&mut app, "$");
        assert_eq!((app.mode, app.window.cursor.col), (Mode::Insert, 7));
    }

    #[test]
    fn ctrl_o_runs_an_ex_command() {
        let mut app = insert_at("a\nb\nc\n", 0, 0);
        app.replay_key(ctrl('o'));
        press(&mut app, ":3");
        assert_eq!(app.mode, Mode::Command);
        app.replay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!((app.mode, app.window.cursor.line), (Mode::Insert, 2));
    }

    #[test]
    fn ctrl_o_command_that_enters_insert_ends_the_oneshot() {
        let mut app = insert_at("a\n", 0, 1);
        app.replay_key(ctrl('o'));
        press(&mut app, "o");
        assert_eq!((app.mode, app.window.cursor.line), (Mode::Insert, 1));
        assert!(app.insert_oneshot.is_none());
    }

    #[test]
    fn text_typed_after_ctrl_o_is_its_own_insert_for_dot() {
        let mut app = app_with_keymaps("xy\n", "");
        press(&mut app, "iab");
        app.replay_key(ctrl('o'));
        press(&mut app, "lc");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.buffer.rope.to_string(), "abxcy\n");
        press(&mut app, ".");
        assert_eq!(app.buffer.rope.to_string(), "abxccy\n");
    }

    #[test]
    fn empty_insert_after_ctrl_o_leaves_dot_on_the_command() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        press(&mut app, "i");
        app.replay_key(ctrl('o'));
        press(&mut app, "dd");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, ".");
        assert_eq!(app.buffer.rope.to_string(), "c\nd\n");
    }

    #[test]
    fn marks_follow_the_text_they_were_set_on() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        press(&mut app, "jjma");
        press(&mut app, "ggdd");
        press(&mut app, "'a");
        assert_eq!(app.window.cursor.line, 1);
    }

    #[test]
    fn quote_quote_goes_back_to_before_the_latest_jump() {
        let mut app = app_with_keymaps("a\nb\nc\nd", "");
        press(&mut app, "jG");
        assert_eq!(app.window.cursor.line, 3);
        press(&mut app, "''");
        assert_eq!(app.window.cursor.line, 1);
        press(&mut app, "``");
        assert_eq!(app.window.cursor.line, 3);
    }

    #[test]
    fn dot_mark_is_the_last_change() {
        let mut app = app_with_keymaps("one\ntwo\nthree\n", "");
        press(&mut app, "jlx");
        press(&mut app, "gg`.");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 1));
    }

    #[test]
    fn caret_mark_is_where_insert_stopped() {
        let mut app = app_with_keymaps("foo bar\n", "");
        press(&mut app, "ixy");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "$`^");
        assert_eq!(app.window.cursor.col, 2);
    }

    #[test]
    fn bracket_marks_span_the_last_yank() {
        let mut app = app_with_keymaps("one\ntwo\nthree\n", "");
        press(&mut app, "jyj");
        press(&mut app, "gg`[");
        assert_eq!(app.window.cursor.line, 1);
        press(&mut app, "`]");
        assert_eq!(app.window.cursor.line, 2);
    }

    #[test]
    fn bracket_marks_span_a_whole_insert() {
        let mut app = app_with_keymaps("ab\n", "");
        press(&mut app, "lifoo");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "0`]");
        assert_eq!(app.window.cursor.col, 3);
        press(&mut app, "`[");
        assert_eq!(app.window.cursor.col, 1);
    }

    #[test]
    fn angle_marks_are_the_last_visual_selection() {
        let mut app = app_with_keymaps("one\ntwo\nthree\n", "");
        press(&mut app, "lvjl");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "gg`>");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 2));
        press(&mut app, "`<");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (0, 1));
    }

    fn visual_ends(app: &crate::app::App) -> Option<((usize, usize), (usize, usize))> {
        let anchor = app.window.visual_anchor?;
        let cursor = app.window.cursor;
        Some(((anchor.line, anchor.col), (cursor.line, cursor.col)))
    }

    #[test]
    fn gv_reselects_the_last_visual_area() {
        let mut app = app_with_keymaps("one\ntwo\nthree\n", "");
        press(&mut app, "lVj");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "gggv");
        assert_eq!(app.mode, Mode::Visual(VisualKind::Line));
        assert_eq!(visual_ends(&app), Some(((0, 1), (1, 1))));
    }

    #[test]
    fn gv_puts_the_cursor_back_on_its_end() {
        let mut app = app_with_keymaps("one two three\n", "");
        press(&mut app, "wvb");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "$gv");
        assert_eq!(visual_ends(&app), Some(((0, 4), (0, 0))));
    }

    #[test]
    fn gv_in_visual_swaps_with_the_previous_selection() {
        let mut app = app_with_keymaps("one two three\n", "");
        press(&mut app, "vl");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "wvl");
        press(&mut app, "gv");
        assert_eq!(app.mode, Mode::Visual(VisualKind::Char));
        assert_eq!(visual_ends(&app), Some(((0, 0), (0, 1))));
        press(&mut app, "gv");
        assert_eq!(visual_ends(&app), Some(((0, 4), (0, 5))));
    }

    #[test]
    fn gv_follows_edits_above_the_selection() {
        let mut app = app_with_keymaps("a\nb\nc\n", "");
        press(&mut app, "jVj");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "ggyyPgv");
        assert_eq!(visual_ends(&app), Some(((2, 0), (3, 0))));
    }

    #[test]
    fn gi_inserts_where_insert_was_last_left() {
        let mut app = app_with_keymaps("one\ntwo\n", "");
        press(&mut app, "Ax");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "jgiy");
        assert_eq!(app.mode, Mode::Insert);
        assert_eq!(app.buffer.rope.to_string(), "onexy\ntwo\n");
    }

    #[test]
    fn gi_follows_edits_above_where_insert_was_left() {
        let mut app = app_with_keymaps("a\nb\n", "");
        press(&mut app, "jAx");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, "ggyyPgiy");
        assert_eq!(app.buffer.rope.to_string(), "a\na\nbxy\n");
    }

    #[test]
    fn gi_before_any_insert_starts_at_the_cursor() {
        let mut app = app_with_keymaps("abc\n", "");
        press(&mut app, "lgix");
        assert_eq!(app.buffer.rope.to_string(), "axbc\n");
    }

    #[test]
    fn g_semicolon_and_comma_walk_the_change_list() {
        let mut app = app_with_keymaps("one\ntwo\nthree\nfour\n", "");
        press(&mut app, "xjjxjx");
        press(&mut app, "ggg;");
        assert_eq!(app.window.cursor.line, 3);
        press(&mut app, "2g;");
        assert_eq!(app.window.cursor.line, 0);
        press(&mut app, "g;");
        assert_eq!(app.status_msg, "E662: At start of changelist");
        press(&mut app, "g,");
        assert_eq!(app.window.cursor.line, 2);
    }

    #[test]
    fn changes_on_one_line_are_one_change_list_entry() {
        let mut app = app_with_keymaps("abcdef\nxyz\n", "");
        press(&mut app, "xxxjx");
        assert_eq!(app.buffer.changes.len(), 2);
    }

    #[test]
    fn changes_command_lists_the_change_list() {
        let mut app = app_with_keymaps("one\ntwo\n", "");
        press(&mut app, "xjx");
        app.exec_command("changes");
        assert!(app.show_list_page);
        let listing = app.listing.as_ref().expect("listing");
        assert_eq!(listing.rows.len(), 2);
        assert_eq!(listing.rows[1].1, "wo");
        app.exec_command("registers");
        assert!(
            app.listing.is_none(),
            ":registers shows the registers again"
        );
    }

    #[test]
    fn marks_command_lists_marks_in_vim_order() {
        let mut app = app_with_keymaps("one\ntwo\nthree\n", "");
        press(&mut app, "jmbggma");
        app.exec_command("marks");
        let listing = app.listing.as_ref().expect("listing");
        let names: String = listing
            .rows
            .iter()
            .filter_map(|(label, _)| label.chars().next())
            .collect();
        assert_eq!(names, "'ab", "`gg` was a jump, so `'` is set too");
        assert_eq!(listing.rows[1].1, "one");
    }

    #[test]
    fn jumps_command_lists_the_jump_list() {
        let mut app = app_with_keymaps("a\nb\nc\nd", "");
        press(&mut app, "Ggg");
        app.exec_command("jumps");
        let listing = app.listing.as_ref().expect("listing");
        let texts: Vec<&str> = listing.rows.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(texts, ["a", "d"]);
    }

    #[test]
    fn case_operators_work_with_motions_and_text_objects() {
        let mut app = app_with_keymaps("hello world\n", "");
        press(&mut app, "gUiw");
        assert_eq!(app.buffer.rope.to_string(), "HELLO world\n");
        press(&mut app, "wg~$");
        assert_eq!(app.buffer.rope.to_string(), "HELLO WORLD\n");
        press(&mut app, "0g?e");
        assert_eq!(app.buffer.rope.to_string(), "URYYB WORLD\n");
    }

    #[test]
    fn case_operators_take_the_line_doubled() {
        let mut app = app_with_keymaps("Abc\nDef\n", "");
        press(&mut app, "guu");
        assert_eq!(app.buffer.rope.to_string(), "abc\nDef\n");
        press(&mut app, "jgUgU");
        assert_eq!(app.buffer.rope.to_string(), "abc\nDEF\n");
        press(&mut app, "g~~");
        assert_eq!(app.buffer.rope.to_string(), "abc\ndef\n");
        press(&mut app, "kg??");
        assert_eq!(app.buffer.rope.to_string(), "nop\ndef\n");
    }

    #[test]
    fn case_operators_leave_the_registers_alone() {
        let mut app = app_with_keymaps("abc\n", "");
        with_register(&mut app, '"', "kept");
        press(&mut app, "gUiw");
        assert_eq!(app.buffer.rope.to_string(), "ABC\n");
        let unnamed = app.registers.get(&'"').map(|r| r.text.as_str());
        assert_eq!(unnamed, Some("kept"));
    }

    #[test]
    fn visual_case_keys_change_the_selection() {
        let mut app = app_with_keymaps("abc def\nghi jkl\n", "");
        press(&mut app, "vlU");
        assert_eq!(app.buffer.rope.to_string(), "ABc def\nghi jkl\n");
        assert_eq!(app.mode, Mode::Normal);
        app.replay_key(ctrl('v'));
        press(&mut app, "jlg?");
        assert_eq!(app.buffer.rope.to_string(), "NOc def\ntui jkl\n");
    }

    #[test]
    fn dot_repeats_a_case_operator() {
        let mut app = app_with_keymaps("one two\n", "");
        press(&mut app, "gUiww.");
        assert_eq!(app.buffer.rope.to_string(), "ONE TWO\n");
    }

    #[test]
    fn ys_wraps_a_text_object_or_a_counted_motion() {
        let mut app = app_with_keymaps("foo bar baz\n", "");
        press(&mut app, "ysiw)");
        assert_eq!(app.buffer.rope.to_string(), "(foo) bar baz\n");
        press(&mut app, "fbys2w]");
        assert_eq!(app.buffer.rope.to_string(), "(foo) [bar baz]\n");
    }

    #[test]
    fn yss_wraps_the_line_from_its_first_non_blank() {
        let mut app = app_with_keymaps("  foo bar\n", "");
        press(&mut app, "yss\"");
        assert_eq!(app.buffer.rope.to_string(), "  \"foo bar\"\n");
    }

    #[test]
    fn ys_upper_s_puts_the_pair_on_lines_of_its_own() {
        let mut app = app_with_keymaps("foo\n", "");
        press(&mut app, "ySS}");
        let unit = app.editorconfig.indent_string();
        assert_eq!(app.buffer.rope.to_string(), format!("{{\n{unit}foo\n}}\n"));
    }

    #[test]
    fn dot_repeats_ys() {
        let mut app = app_with_keymaps("foo bar\n", "");
        press(&mut app, "ysiw)W.");
        assert_eq!(app.buffer.rope.to_string(), "(foo) (bar)\n");
    }

    #[test]
    fn g_upper_j_joins_without_touching_whitespace() {
        let mut app = app_with_keymaps("foo\n    bar\n", "");
        press(&mut app, "gJ");
        assert_eq!(app.buffer.rope.to_string(), "foo    bar\n");
    }

    #[test]
    fn a_count_on_j_joins_that_many_lines() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        press(&mut app, "3J");
        assert_eq!(app.buffer.rope.to_string(), "a b c\nd\n");
    }

    #[test]
    fn visual_j_and_g_upper_j_join_the_selected_lines() {
        let mut app = app_with_keymaps("a\n  b\n  c\nd\n", "");
        press(&mut app, "VjjJ");
        assert_eq!(app.buffer.rope.to_string(), "a b c\nd\n");
        assert_eq!(app.mode, Mode::Normal);
        let mut app = app_with_keymaps("a\n  b\nc\n", "");
        press(&mut app, "vjgJ");
        assert_eq!(app.buffer.rope.to_string(), "a  b\nc\n");
    }

    fn with_register(app: &mut crate::app::App, name: char, text: &str) {
        let reg = crate::app::state::Register {
            text: text.into(),
            linewise: false,
        };
        app.registers.insert(name, reg);
    }

    #[test]
    fn ctrl_r_inserts_a_register_at_the_cursor() {
        let mut app = insert_at("ab\n", 0, 1);
        with_register(&mut app, 'a', "XY");
        app.replay_key(ctrl('r'));
        press(&mut app, "a");
        assert_eq!(app.buffer.rope.to_string(), "aXYb\n");
        assert_eq!(app.window.cursor.col, 3);
        assert!(matches!(app.mode, Mode::Insert));
    }

    #[test]
    fn ctrl_r_register_with_newlines_makes_real_lines() {
        let mut app = insert_at("ab\n", 0, 1);
        with_register(&mut app, 'a', "one\ntwo");
        app.replay_key(ctrl('r'));
        press(&mut app, "a");
        assert_eq!(app.buffer.rope.to_string(), "aone\ntwob\n");
        assert_eq!((app.window.cursor.line, app.window.cursor.col), (1, 3));
    }

    #[test]
    fn esc_after_ctrl_r_cancels_and_stays_in_insert() {
        let mut app = insert_at("ab\n", 0, 1);
        app.replay_key(ctrl('r'));
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Insert));
        press(&mut app, "a");
        assert_eq!(app.buffer.rope.to_string(), "aab\n");
    }

    #[test]
    fn ctrl_t_indents_and_keeps_the_cursor_on_its_text() {
        let mut app = insert_at("foo\n", 0, 2);
        let unit = app.editorconfig.indent_string();
        app.replay_key(ctrl('t'));
        assert_eq!(app.buffer.rope.to_string(), format!("{unit}foo\n"));
        assert_eq!(app.window.cursor.col, unit.chars().count() + 2);
    }

    #[test]
    fn ctrl_t_indents_a_blank_line() {
        let mut app = insert_at("\n", 0, 0);
        let unit = app.editorconfig.indent_string();
        app.replay_key(ctrl('t'));
        assert_eq!(app.buffer.rope.to_string(), format!("{unit}\n"));
        assert_eq!(app.window.cursor.col, unit.chars().count());
    }

    #[test]
    fn ctrl_d_outdents_and_keeps_the_cursor_on_its_text() {
        let mut app = insert_at("foo\n", 0, 0);
        let unit = app.editorconfig.indent_string();
        app.buffer = buf(&format!("{unit}foo\n"));
        app.window.cursor.col = unit.chars().count() + 2;
        app.replay_key(ctrl('d'));
        assert_eq!(app.buffer.rope.to_string(), "foo\n");
        assert_eq!(app.window.cursor.col, 2);
    }

    /// Feeds `keys` after a `Ctrl-V` and returns what got inserted, plus
    /// whether the key that ended the sequence still needs normal handling.
    fn after_ctrl_v(keys: &str) -> (Vec<char>, bool) {
        let mut pending = Some(crate::app::state::LiteralPending::Key);
        let mut inserted = Vec::new();
        let mut reprocess = false;
        for c in keys.chars() {
            let Some(p) = pending.take() else { break };
            let step = literal_step(&p, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            inserted.extend(step.insert);
            pending = step.next;
            reprocess = step.reprocess;
        }
        (inserted, reprocess)
    }

    #[test]
    fn ctrl_v_takes_a_plain_key_literally() {
        assert_eq!(after_ctrl_v("("), (vec!['('], false));
    }

    #[test]
    fn ctrl_v_character_codes() {
        assert_eq!(after_ctrl_v("u00e9"), (vec!['é'], false));
        assert_eq!(after_ctrl_v("x41"), (vec!['A'], false));
        assert_eq!(after_ctrl_v("o101"), (vec!['A'], false));
        assert_eq!(after_ctrl_v("065"), (vec!['A'], false));
    }

    #[test]
    fn ctrl_v_code_ends_early_on_a_non_digit() {
        assert_eq!(after_ctrl_v("u41z"), (vec!['A'], true));
    }

    #[test]
    fn ctrl_v_u_with_no_digits_inserts_the_u() {
        assert_eq!(after_ctrl_v("uz"), (vec!['u'], true));
    }

    #[test]
    fn ctrl_v_decimal_code_stops_at_255() {
        assert_eq!(after_ctrl_v("300"), (vec!['\u{1e}'], true));
    }

    #[test]
    fn ctrl_v_tab_inserts_a_real_tab() {
        let mut app = insert_at("ab\n", 0, 1);
        app.replay_key(ctrl('v'));
        app.replay_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.buffer.rope.to_string(), "a\tb\n");
        assert_eq!(app.window.cursor.col, 2);
    }

    #[test]
    fn ctrl_v_bracket_is_not_auto_paired() {
        let mut app = insert_at("a\n", 0, 1);
        app.replay_key(ctrl('v'));
        press(&mut app, "(");
        assert_eq!(app.buffer.rope.to_string(), "a(\n");
    }

    #[test]
    fn typing_carries_on_after_a_ctrl_v_code() {
        let mut app = insert_at("a\n", 0, 1);
        app.replay_key(ctrl('v'));
        press(&mut app, "u41z");
        assert_eq!(app.buffer.rope.to_string(), "aAz\n");
    }

    #[test]
    fn ctrl_v_refuses_a_control_character() {
        let mut app = insert_at("a\n", 0, 1);
        app.replay_key(ctrl('v'));
        press(&mut app, "u001b");
        assert_eq!(app.buffer.rope.to_string(), "a\n");
        assert!(app.status_msg.contains("U+001B"));
    }

    #[test]
    fn ctrl_y_copies_the_char_above() {
        let mut app = insert_at("abc\nx\n", 1, 1);
        app.replay_key(ctrl('y'));
        app.replay_key(ctrl('y'));
        assert_eq!(app.buffer.rope.to_string(), "abc\nxbc\n");
        assert_eq!(app.window.cursor.col, 3);
    }

    #[test]
    fn ctrl_e_copies_the_char_below() {
        let mut app = insert_at("x\nabc\n", 0, 1);
        app.replay_key(ctrl('e'));
        assert_eq!(app.buffer.rope.to_string(), "xb\nabc\n");
    }

    #[test]
    fn ctrl_y_past_the_end_of_the_line_above_inserts_nothing() {
        let mut app = insert_at("a\nxyz\n", 1, 2);
        app.replay_key(ctrl('y'));
        assert_eq!(app.buffer.rope.to_string(), "a\nxyz\n");
        assert_eq!(app.window.cursor.col, 2);
    }

    #[test]
    fn d_brace_from_the_start_of_a_line_takes_whole_lines() {
        let mut app = app_with_keymaps("a\nb\n\nc\n", "");
        press(&mut app, "\"_d}");
        assert_eq!(app.buffer.rope.to_string(), "\nc\n");
    }

    #[test]
    fn d_open_brace_takes_the_lines_above_but_not_the_cursors() {
        let mut app = app_with_keymaps("a\n\nb\nc\n", "");
        app.window.cursor.line = 3;
        press(&mut app, "\"_d{");
        assert_eq!(app.buffer.rope.to_string(), "a\nc\n");
    }

    #[test]
    fn dap_deletes_the_paragraph_and_the_blank_line_after_it() {
        let mut app = app_with_keymaps("a\nb\n\nc\n", "");
        press(&mut app, "\"_dap");
        assert_eq!(app.buffer.rope.to_string(), "c\n");
    }

    #[test]
    fn d_underscore_and_d_plus_delete_whole_lines() {
        let mut app = app_with_keymaps("a\nb\nc\nd\n", "");
        press(&mut app, "\"_d_");
        assert_eq!(app.buffer.rope.to_string(), "b\nc\nd\n");
        press(&mut app, "\"_d+");
        assert_eq!(app.buffer.rope.to_string(), "d\n");
    }

    #[test]
    fn g0_goes_to_the_first_char_on_screen_when_scrolled() {
        let mut app = app_with_keymaps("0123456789abcdef\n", "");
        app.window.view_left = 5;
        app.window.cursor.col = 10;
        press(&mut app, "g0");
        assert_eq!(app.window.cursor.col, 5);
    }

    #[test]
    fn ctrl_caret_and_e_hash_toggle_between_two_files() {
        let dir = std::env::temp_dir().join(format!("binvim-alt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "a\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        let mut app = app_with_keymaps("", "");
        app.open_buffer(a.clone()).unwrap();
        app.open_buffer(b.clone()).unwrap();
        app.replay_key(ctrl('^'));
        assert_eq!(app.buffer.path.as_deref(), Some(a.as_path()));
        app.exec_command("e#");
        assert_eq!(app.buffer.path.as_deref(), Some(b.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two small files in a fresh temp dir; returns (dir, a, b).
    fn two_files(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("binvim-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "a\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        (dir, a, b)
    }

    #[test]
    fn uppercase_mark_goes_back_to_its_file() {
        let (dir, a, b) = two_files("filemark");
        std::fs::write(&a, "one\ntwo\nthree\n").unwrap();
        let mut app = app_with_keymaps("", "");
        app.open_buffer(a.clone()).unwrap();
        press(&mut app, "jjmA");
        app.open_buffer(b.clone()).unwrap();
        press(&mut app, "'A");
        assert_eq!(app.buffer.path.as_deref(), Some(a.as_path()));
        assert_eq!(app.window.cursor.line, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn uppercase_mark_reopens_its_closed_file() {
        let (dir, a, b) = two_files("filemark-closed");
        std::fs::write(&a, "one\ntwo\nthree\n").unwrap();
        let mut app = app_with_keymaps("", "");
        app.open_buffer(b.clone()).unwrap();
        app.open_buffer(a.clone()).unwrap();
        press(&mut app, "jmA");
        app.exec_command("bd");
        assert_ne!(app.buffer.path.as_deref(), Some(a.as_path()));
        press(&mut app, "'A");
        assert_eq!(app.buffer.path.as_deref(), Some(a.as_path()));
        assert_eq!(app.window.cursor.line, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn q_refuses_while_another_buffer_has_unsaved_changes() {
        let (dir, a, b) = two_files("q162");
        let mut app = app_with_keymaps("", "");
        app.open_buffer(a).unwrap();
        app.buffer.dirty = true;
        app.open_buffer(b).unwrap();
        app.exec_command("q");
        assert!(app.status_msg.contains("E162"), "{}", app.status_msg);
        assert!(!app.should_quit);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wa_writes_every_modified_buffer_and_stays_put() {
        let (dir, a, b) = two_files("wa");
        let mut app = app_with_keymaps("", "");
        app.open_buffer(a.clone()).unwrap();
        app.buffer.insert_str(0, 0, "x");
        app.buffer.dirty = true;
        app.open_buffer(b.clone()).unwrap();
        app.exec_command("wa");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "xa\n");
        assert_eq!(app.buffer.path.as_deref(), Some(b.as_path()));
        assert_eq!(app.alternate_path.as_deref(), Some(a.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zz_quits_a_clean_buffer() {
        let mut app = app_with_keymaps("x\n", "");
        press(&mut app, "ZZ");
        assert!(app.should_quit);
    }

    #[test]
    fn e_bang_reverts_to_the_file_on_disk() {
        let (dir, a, _) = two_files("revert");
        let mut app = app_with_keymaps("", "");
        app.open_buffer(a).unwrap();
        app.buffer.insert_str(0, 0, "junk");
        app.buffer.dirty = true;
        app.exec_command("e!");
        assert_eq!(app.buffer.rope.to_string(), "a\n");
        assert!(!app.buffer.dirty);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ctrl_caret_without_an_alternate_says_so() {
        let mut app = app_with_keymaps("x\n", "");
        app.replay_key(ctrl('^'));
        assert!(app.status_msg.contains("E23"));
    }

    #[test]
    fn cip_leaves_an_empty_line_to_type_on() {
        let mut app = app_with_keymaps("a\nb\n\nc\n", "");
        press(&mut app, "\"_cip");
        assert_eq!(app.buffer.rope.to_string(), "\n\nc\n");
        assert!(matches!(app.mode, Mode::Insert));
    }

    #[test]
    fn vip_selects_whole_lines() {
        let mut app = app_with_keymaps("a\nb\n\nc\n", "");
        press(&mut app, "vip");
        assert!(matches!(
            app.mode,
            Mode::Visual(crate::mode::VisualKind::Line)
        ));
        assert_eq!(app.window.cursor.line, 1);
    }

    #[test]
    fn mapped_key_runs_its_expansion() {
        let mut app = app_with_keymaps("    foo\n", "[normal]\nH = \"^\"");
        app.window.cursor.col = 6;
        press(&mut app, "H");
        assert_eq!(app.window.cursor.col, 4);
    }

    #[test]
    fn find_target_is_never_remapped() {
        let mut app = app_with_keymaps("  aHb\n", "[normal]\nH = \"^\"");
        press(&mut app, "fH");
        assert_eq!(app.window.cursor.col, 3, "fH must find a literal H");
    }

    #[test]
    fn mapping_supplies_an_operators_motion() {
        let mut app = app_with_keymaps("    foo bar\n", "[normal]\nH = \"^\"");
        app.window.cursor.col = 8;
        // Black-hole register: a plain `d` mirrors to the OS clipboard,
        // which every test in the process shares.
        press(&mut app, "\"_dH");
        assert_eq!(app.buffer.rope.to_string(), "    bar\n");
    }

    #[test]
    fn typed_count_multiplies_the_mappings_count() {
        let text = "x\n".repeat(40);
        let mut app = app_with_keymaps(&text, "[normal]\nJ = \"10j\"");
        press(&mut app, "3J");
        assert_eq!(app.window.cursor.line, 30);
    }

    #[test]
    fn expansions_are_not_remapped() {
        let mut app = app_with_keymaps("a\nb\nc\n", "[normal]\nj = \"k\"\nk = \"j\"");
        app.window.cursor.line = 1;
        press(&mut app, "j");
        assert_eq!(app.window.cursor.line, 0);
    }

    #[test]
    fn macro_run_from_a_mapping_still_sees_mappings() {
        let mut app = app_with_keymaps("    foo\n", "[normal]\nQ = \"@q\"\nH = \"^\"");
        app.window.cursor.col = 6;
        let h = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::NONE);
        app.macros.insert('q', vec![h]);
        press(&mut app, "Q");
        assert_eq!(app.window.cursor.col, 4);
    }

    fn time_out(app: &mut crate::app::App) {
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert!(app.keymap_flush_if_due(later), "nothing was held");
    }

    #[test]
    fn multi_key_mapping_expands() {
        let mut app = app_with_keymaps("    foo\n", "[normal]\ngh = \"^\"");
        app.window.cursor.col = 6;
        press(&mut app, "gh");
        assert_eq!(app.window.cursor.col, 4);
    }

    #[test]
    fn leader_mapping_expands() {
        let mut app = app_with_keymaps("    foo\n", "[normal]\n\"<leader>x\" = \"$\"");
        press(&mut app, " x");
        assert_eq!(app.window.cursor.col, 6);
    }

    #[test]
    fn unfinished_prefix_waits_without_a_timeout() {
        let mut app = app_with_keymaps("a\nb\nc\n", "[normal]\ngh = \"^\"");
        app.window.cursor.line = 2;
        press(&mut app, "g");
        assert_eq!(app.window.cursor.line, 2, "g must wait for its next key");
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert!(
            !app.keymap_flush_if_due(later),
            "g waits for its next key, as it does unmapped"
        );
        press(&mut app, "g");
        assert_eq!(app.window.cursor.line, 0, "the held g should start gg");
    }

    #[test]
    fn held_complete_command_runs_on_timeout() {
        let mut app = app_with_keymaps("abc\n", "[normal]\nxx = \"dd\"");
        // Black-hole register, for the same clipboard reason as `"_dH`.
        press(&mut app, "\"_x");
        assert_eq!(app.buffer.rope.to_string(), "abc\n", "xx might follow");
        time_out(&mut app);
        assert_eq!(app.buffer.rope.to_string(), "bc\n");
    }

    #[test]
    fn leader_mapping_survives_a_pause_on_the_popup() {
        let mut app = app_with_keymaps("    foo\n", "[normal]\n\"<leader>x\" = \"$\"");
        press(&mut app, " ");
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert!(!app.keymap_flush_if_due(later));
        press(&mut app, "x");
        assert_eq!(app.window.cursor.col, 6);
    }

    #[test]
    fn whichkey_lists_leader_mappings_over_the_built_in_rows() {
        let keymaps = r#"
            [normal]
            "<leader>x" = { keys = "$", desc = "End of line" }
            "<leader>e" = "^"
            "<leader>bz" = "gg"
        "#;
        let mut app = app_with_keymaps("foo\n", keymaps);
        press(&mut app, " ");
        assert!(
            app.leader_pressed_at.is_some(),
            "a held leader must start the which-key timer"
        );
        let popup = app.whichkey_popup().expect("Leader popup");
        assert_eq!(popup.title, "Leader");
        let row = |k: &str| {
            popup
                .entries
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, d)| d.clone())
        };
        assert_eq!(row("x").as_deref(), Some("End of line"));
        assert_eq!(row("e").as_deref(), Some("^"), "replaces File explorer");
        assert_eq!(row("b").as_deref(), Some("+Buffer"));

        press(&mut app, "b");
        let popup = app.whichkey_popup().expect("Buffer popup");
        assert_eq!(popup.title, "Buffer");
        assert!(popup.entries.iter().any(|(k, d)| k == "z" && d == "gg"));
    }

    #[test]
    fn insert_mapping_leaves_insert_mode() {
        let mut app = app_with_keymaps("ab\n", "[insert]\njk = \"<Esc>\"");
        press(&mut app, "ijk");
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.buffer.rope.to_string(), "ab\n");
    }

    #[test]
    fn held_insert_key_is_typed_on_timeout() {
        let mut app = app_with_keymaps("ab\n", "[insert]\njk = \"<Esc>\"");
        press(&mut app, "ij");
        assert_eq!(app.buffer.rope.to_string(), "ab\n", "jk might follow");
        time_out(&mut app);
        assert_eq!(app.buffer.rope.to_string(), "jab\n");
        assert!(matches!(app.mode, Mode::Insert));
    }

    #[test]
    fn held_insert_key_is_typed_when_the_next_key_rules_the_mapping_out() {
        let mut app = app_with_keymaps("ab\n", "[insert]\njk = \"<Esc>\"");
        press(&mut app, "ijx");
        assert_eq!(app.buffer.rope.to_string(), "jxab\n");
    }

    #[test]
    fn mappings_stay_in_their_own_mode() {
        let keymaps = "[normal]\nH = \"^\"\n[insert]\njk = \"<Esc>\"";
        let mut app = app_with_keymaps("a\nb\n", keymaps);
        press(&mut app, "j");
        assert_eq!(app.window.cursor.line, 1, "Insert's jk must not hold a j");
        press(&mut app, "iH");
        assert_eq!(
            app.buffer.rope.to_string(),
            "a\nHb\n",
            "Normal's H must not fire in Insert"
        );
    }

    #[test]
    fn command_line_mapping_expands_in_the_colon_prompt() {
        let mut app = app_with_keymaps("a\n", "[command]\nqq = \"wq\"");
        press(&mut app, ":qq");
        assert!(matches!(app.mode, Mode::Command));
        assert_eq!(app.cmdline, "wq");
    }

    #[test]
    fn command_line_mapping_expands_in_the_search_prompt() {
        let mut app = app_with_keymaps("a\n", "[command]\njj = \"<Esc>\"");
        press(&mut app, "/jj");
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.cmdline.is_empty());
    }

    #[test]
    fn held_command_line_key_is_typed_on_timeout() {
        let mut app = app_with_keymaps("a\n", "[command]\nqq = \"wq\"");
        press(&mut app, ":q");
        assert_eq!(app.cmdline, "", "qq might follow");
        time_out(&mut app);
        assert_eq!(app.cmdline, "q");
    }

    #[test]
    fn held_prefix_runs_as_typed_when_the_next_key_rules_it_out() {
        let mut app = app_with_keymaps("a\nb\nc\n", "[normal]\ngh = \"^\"");
        app.window.cursor.line = 2;
        press(&mut app, "gg");
        assert_eq!(app.window.cursor.line, 0);
    }

    #[test]
    fn shorter_mapping_runs_once_the_longer_one_is_ruled_out() {
        let text = "a\nb\nc\nd\ne\n";
        let keymaps = "[normal]\nJ = \"j\"\nJk = \"gg\"";
        let mut app = app_with_keymaps(text, keymaps);
        press(&mut app, "J");
        assert_eq!(app.window.cursor.line, 0, "J waits: Jk might follow");
        time_out(&mut app);
        assert_eq!(app.window.cursor.line, 1);
        press(&mut app, "Jj");
        assert_eq!(app.window.cursor.line, 3, "J runs as j, then j is typed");
        press(&mut app, "Jk");
        assert_eq!(app.window.cursor.line, 0);
    }

    #[test]
    fn visual_col_to_char_col_no_tabs() {
        // `hello world` — visual col == char col on plain ASCII.
        let b = buf("hello world\n");
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 0, 11, &[], false),
            0
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 6, 11, &[], false),
            6
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 10, 11, &[], false),
            10
        );
        // Click past EOL in Normal mode clamps to the last char.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 30, 11, &[], false),
            10
        );
    }

    #[test]
    fn visual_col_to_char_col_insert_past_eol() {
        // Same line, Insert-mode behaviour: a click past the last char
        // parks the cursor at `line_len` so the user can append.
        let b = buf("hello world\n");
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 11, 11, &[], true),
            11
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 30, 11, &[], true),
            11
        );
        // Inside the line, both modes agree.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 6, 11, &[], true),
            6
        );
    }

    #[test]
    fn visual_col_to_char_col_wide_chars() {
        // Regression: clicking into CJK text must land on the character
        // the cursor cell points at, matching the 2-cell terminal width.
        // "你好世界" — each char is 2 cells wide.
        let b = buf("\u{4F60}\u{597D}\u{4E16}\u{754C}\n");
        // First cell of each wide char snaps to that char.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 0, 4, &[], false),
            0
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 2, 4, &[], false),
            1
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 4, 4, &[], false),
            2
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 6, 4, &[], false),
            3
        );
        // Second cell of a wide char stays on the same char.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 1, 4, &[], false),
            0
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 7, 4, &[], false),
            3
        );
        // Past EOL in Normal mode clamps to the last char (col 3).
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 40, 4, &[], false),
            3
        );
    }

    #[test]
    fn visual_col_to_char_col_with_tabs() {
        // "\t\tx" — two tabs (4 visual cols each) then `x`. Char positions:
        //   0 = first tab, 1 = second tab, 2 = 'x'. Visual positions:
        //   0..4 = first tab, 4..8 = second tab, 8 = 'x'.
        let b = buf("\t\tx\n");
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 0, 3, &[], false),
            0
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 2, 3, &[], false),
            0
        ); // mid first tab
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 4, 3, &[], false),
            1
        ); // start of second tab
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 6, 3, &[], false),
            1
        ); // mid second tab
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 8, 3, &[], false),
            2
        ); // on `x`
        // The original bug: clicking at visual col 8 used to land at char 8
        // (past EOL). Now it clamps to the last char (`x`).
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 30, 3, &[], false),
            2
        );
    }

    #[test]
    fn visual_col_to_char_col_mixed_tabs_then_text() {
        // "\t\t<partial …" — clicking on `<` after two tabs should yield
        // char col 2 (the `<`), not 8 (which would be deep inside the word).
        let line = "\t\t<partial";
        let b = buf(&format!("{}\n", line));
        let line_len = line.chars().count();
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 8, line_len, &[], false),
            2
        );
        // Click 3 cells in (between the two tabs visually) clamps to char 0.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 3, line_len, &[], false),
            0
        );
    }

    #[test]
    fn visual_col_to_char_col_empty_line() {
        let b = buf("\n");
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 0, 0, &[], false),
            0
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 99, 0, &[], false),
            0
        );
    }

    #[test]
    fn visual_col_to_char_col_skips_inlay_hints() {
        // `foo bar` with a 10-cell inlay hint anchored at col 4 (right
        // before `b`). Visual layout:
        //   cols 0..3    → "foo"
        //   col   3      → space
        //   cols 4..14   → hint (10 cells)
        //   cols 14..17  → "bar"
        let b = buf("foo bar\n");
        let mut hints = vec![0usize; 8]; // line_len + 1 = 7 + 1
        hints[4] = 10;
        // Click on `f` (visual col 0) → buffer col 0.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 0, 7, &hints, false),
            0
        );
        // Click on the space (visual col 3) → buffer col 3.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 3, 7, &hints, false),
            3
        );
        // Click anywhere inside the hint (visual cols 4..13) → snap to
        // buffer col 4 (the `b` immediately after the hint), NOT col 5+
        // which would land inside `bar`.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 4, 7, &hints, false),
            4
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 8, 7, &hints, false),
            4
        );
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 13, 7, &hints, false),
            4
        );
        // Click on `b` (visual col 14) → buffer col 4.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 14, 7, &hints, false),
            4
        );
        // Click on `r` (visual col 16) → buffer col 6.
        assert_eq!(
            visual_col_to_char_col_with_hints(&b, 0, 16, 7, &hints, false),
            6
        );
    }

    #[test]
    fn previous_word_eats_word_run() {
        let b = buf("hello world\n");
        // From end of "world" (col 11) → after the trailing word run is
        // gone, the cursor lands at col 6 (just before "world").
        assert_eq!(previous_word_boundary(&b, 0, 11), 6);
    }

    #[test]
    fn previous_word_skips_trailing_whitespace_then_eats_word() {
        let b = buf("hello    \n");
        // From col 9 (after trailing spaces) → eat the spaces, then the
        // word "hello", landing at col 0.
        assert_eq!(previous_word_boundary(&b, 0, 9), 0);
    }

    #[test]
    fn previous_word_peels_punctuation_separately() {
        let b = buf("foo->bar\n");
        // From end (col 8) → cursor is on a word char, eats "bar".
        assert_eq!(previous_word_boundary(&b, 0, 8), 5);
        // From col 5 (just after `->`) → cursor is on punctuation, eats
        // the `->` run.
        assert_eq!(previous_word_boundary(&b, 0, 5), 3);
    }

    #[test]
    fn previous_word_at_line_start_is_a_noop() {
        let b = buf("hello\n");
        assert_eq!(previous_word_boundary(&b, 0, 0), 0);
    }

    #[test]
    fn previous_word_with_only_whitespace_lands_at_zero() {
        let b = buf("    \n");
        // From col 4 (after all the spaces) → eat them, land at 0.
        assert_eq!(previous_word_boundary(&b, 0, 4), 0);
    }

    // --- smart-Enter split-pair predicate ----------------------------

    #[test]
    fn enter_splits_curly_brace_pair() {
        assert!(should_split_pair_on_enter(Some('{'), Some('}'), None));
    }

    #[test]
    fn enter_splits_square_bracket_pair() {
        assert!(should_split_pair_on_enter(Some('['), Some(']'), None));
    }

    #[test]
    fn enter_splits_parenthesis_pair() {
        assert!(should_split_pair_on_enter(Some('('), Some(')'), None));
    }

    #[test]
    fn enter_splits_html_tag_pair() {
        // `<div>|</div>` — prev_non_ws='>', next_non_ws='<', next_next='/'.
        assert!(should_split_pair_on_enter(Some('>'), Some('<'), Some('/')));
    }

    #[test]
    fn enter_does_not_split_generic_type_followed_by_less_than() {
        // `Foo<Bar>|<other` — prev='>', next='<', next_next is a name
        // char rather than `/`. Should NOT split (it's just two
        // generic-type / comparison sites next to each other).
        assert!(!should_split_pair_on_enter(Some('>'), Some('<'), Some('o')));
    }

    #[test]
    fn enter_does_not_split_unmatched_pairs() {
        // No closer to the right → no split.
        assert!(!should_split_pair_on_enter(Some('{'), Some(')'), None));
        assert!(!should_split_pair_on_enter(Some('('), Some(']'), None));
        // No opener to the left → no split.
        assert!(!should_split_pair_on_enter(None, Some('}'), None));
        assert!(!should_split_pair_on_enter(Some('a'), Some(')'), None));
    }
}
