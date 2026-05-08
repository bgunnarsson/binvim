use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::buffer::Buffer;
use crate::command::{self, ExCommand, ExRange};
use crate::config::Config;
use crate::lang::{self, HighlightCache};
use crate::lsp::{CompletionItem, Diagnostic, LspEvent, LspManager, Severity};
use crate::picker::{self, PickerKind, PickerPayload, PickerState};
use crate::cursor::Cursor;
use crate::mode::{Mode, Operator, VisualKind};
use crate::motion::{self, MotionKind, MotionResult};
use crate::parser::{
    self, Action, InsertWhere, MotionVerb, PageScrollKind, ParseCtx, ParseResult, PendingCmd,
    PickerLeader, ViewportAdjust,
};
use crate::render;
use crate::text_object::{self, TextObjectVerb, TextRange};
use crate::undo::History;

#[derive(Debug, Clone)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FindRecord {
    pub ch: char,
    pub forward: bool,
    pub before: bool,
}

/// Per-buffer state. The active buffer's state lives directly on App fields;
/// inactive buffers are stored as stashes in `App.buffers`.
#[derive(Default, Clone)]
pub struct BufferStash {
    pub buffer: Buffer,
    pub cursor: Cursor,
    pub view_top: usize,
    pub history: History,
    pub visual_anchor: Option<Cursor>,
    pub marks: HashMap<char, (usize, usize)>,
    pub jumplist: Vec<(usize, usize)>,
    pub jump_idx: usize,
}

#[derive(Debug, Clone)]
pub enum LastEdit {
    Plain(Action),
    InsertSession {
        prelude: Action,
        keys: Vec<KeyEvent>,
    },
}

#[derive(Debug)]
struct RecordingState {
    prelude: Action,
    keys: Vec<KeyEvent>,
}

pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// Position where the existing word-prefix begins; replaced with the chosen item on accept.
    pub anchor_line: usize,
    pub anchor_col: usize,
}

pub struct App {
    pub buffer: Buffer,
    pub cursor: Cursor,
    pub mode: Mode,
    pub pending: PendingCmd,
    pub history: History,
    pub registers: HashMap<char, Register>,
    pub cmdline: String,
    pub status_msg: String,
    pub view_top: usize,
    pub width: u16,
    pub height: u16,
    pub should_quit: bool,
    pub visual_anchor: Option<Cursor>,
    pub last_find: Option<FindRecord>,
    /// `(query, backward)` — direction is the original search direction so `n`/`N` honour it.
    pub last_search: Option<(String, bool)>,
    /// True when `:noh` has temporarily silenced search highlight; auto-cleared on next search.
    pub search_hl_off: bool,
    pub last_edit: Option<LastEdit>,
    pub marks: HashMap<char, (usize, usize)>,
    pub jumplist: Vec<(usize, usize)>,
    pub jump_idx: usize,
    pub macros: HashMap<char, Vec<KeyEvent>>,
    pub recording_macro: Option<char>,
    pub macro_buffer: Vec<KeyEvent>,
    pub last_replayed_macro: Option<char>,
    /// All buffers; `buffers[active]` is a placeholder while its real state lives on App fields.
    pub buffers: Vec<BufferStash>,
    pub active: usize,
    pub highlight_cache: Option<HighlightCache>,
    pub picker: Option<PickerState>,
    pub config: Config,
    pub lsp: LspManager,
    /// Last buffer version we shipped to the LSP, keyed by path.
    pub last_sent_version: HashMap<PathBuf, u64>,
    pub completion: Option<CompletionState>,
    replaying_macro: bool,
    recording: Option<RecordingState>,
    replaying: bool,
}

impl App {
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        let buffer = match path {
            Some(p) => Buffer::from_path(p)?,
            None => Buffer::empty(),
        };
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        Ok(Self {
            buffer,
            cursor: Cursor::default(),
            mode: Mode::Normal,
            pending: PendingCmd::default(),
            history: History::new(),
            registers: HashMap::new(),
            cmdline: String::new(),
            status_msg: String::new(),
            view_top: 0,
            width: w,
            height: h,
            should_quit: false,
            visual_anchor: None,
            last_find: None,
            last_search: None,
            search_hl_off: false,
            last_edit: None,
            marks: HashMap::new(),
            jumplist: Vec::new(),
            jump_idx: 0,
            macros: HashMap::new(),
            recording_macro: None,
            macro_buffer: Vec::new(),
            last_replayed_macro: None,
            buffers: vec![BufferStash::default()],
            active: 0,
            highlight_cache: None,
            picker: None,
            config: Config::load(),
            lsp: LspManager::new(),
            last_sent_version: HashMap::new(),
            completion: None,
            replaying_macro: false,
            recording: None,
            replaying: false,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let _guard = TerminalGuard::enable()?;
        let mut stdout = io::stdout();
        // Bootstrap LSP for the file we were started with.
        self.lsp_attach_active();
        while !self.should_quit {
            self.adjust_viewport();
            self.ensure_highlights();
            let events = self.lsp.drain();
            self.handle_lsp_events(events);
            self.lsp_sync_active();
            render::draw(&mut stdout, self)?;
            stdout.flush()?;
            if crossterm::event::poll(Duration::from_millis(100))? {
                self.handle_event()?;
            }
        }
        Ok(())
    }

    fn handle_lsp_events(&mut self, events: Vec<LspEvent>) {
        for ev in events {
            match ev {
                LspEvent::GotoDef { path, line, col } => {
                    self.push_jump();
                    if let Err(e) = self.open_buffer(path) {
                        self.status_msg = format!("error: {e}");
                        continue;
                    }
                    self.cursor.line = line;
                    self.cursor.col = col;
                    self.cursor.want_col = col;
                    self.clamp_cursor_normal();
                }
                LspEvent::Hover { text } => {
                    // Status line is one row — show only the first non-empty line.
                    let summary = text
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .to_string();
                    self.status_msg = summary;
                }
                LspEvent::NotFound(kind) => {
                    self.status_msg = format!("LSP: no {kind} found");
                    if kind == "completions" {
                        self.completion = None;
                    }
                }
                LspEvent::Completion { items } => {
                    let (anchor_line, anchor_col) = self.word_prefix_start();
                    self.completion = Some(CompletionState {
                        items,
                        selected: 0,
                        anchor_line,
                        anchor_col,
                    });
                }
            }
        }
    }

    /// Walk back from the cursor through identifier-class chars to find where the
    /// in-progress word started — that's the chunk we'll replace on completion accept.
    fn word_prefix_start(&self) -> (usize, usize) {
        let line = self.cursor.line;
        let mut col = self.cursor.col;
        while col > 0 {
            let prev = self
                .buffer
                .char_at(line, col - 1)
                .unwrap_or(' ');
            if prev.is_alphanumeric() || prev == '_' {
                col -= 1;
            } else {
                break;
            }
        }
        (line, col)
    }

    fn lsp_request_completion(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_completion(&path, line, col) {
            // No LSP — silently ignore so editing isn't disrupted.
        }
    }

    fn completion_cycle(&mut self, delta: i64) {
        let Some(c) = self.completion.as_mut() else {
            return;
        };
        if c.items.is_empty() {
            return;
        }
        let n = c.items.len() as i64;
        c.selected = ((c.selected as i64 + delta).rem_euclid(n)) as usize;
    }

    fn completion_accept(&mut self) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(item) = c.items.get(c.selected).cloned() else {
            return;
        };
        // Replace [(anchor_line, anchor_col), cursor) with insert_text.
        if c.anchor_line == self.cursor.line {
            let start = self.buffer.pos_to_char(c.anchor_line, c.anchor_col);
            let end = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
            if end >= start {
                self.buffer.delete_range(start, end);
            }
            self.buffer.insert_at_idx(start, &item.insert_text);
            let new_idx = start + item.insert_text.chars().count();
            self.cursor_to_idx(new_idx);
        }
    }

    fn lsp_attach_active(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return; };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Some(client) = self.lsp.ensure_for_path(&path, &cwd) {
            let text = self.buffer.rope.to_string();
            let _ = client.did_open(&path, "rust", &text);
            self.last_sent_version
                .insert(path.clone(), self.buffer.version);
        }
    }

    fn lsp_sync_active(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return; };
        let last = self.last_sent_version.get(&path).copied().unwrap_or(u64::MAX);
        if last == self.buffer.version {
            return;
        }
        // Make sure a client exists for this path (handles late attach on first edit).
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if self.lsp.ensure_for_path(&path, &cwd).is_none() {
            return;
        }
        let text = self.buffer.rope.to_string();
        if last == u64::MAX {
            // First sync — emit didOpen.
            if let Some(client) = self.lsp.ensure_for_path(&path, &cwd) {
                let _ = client.did_open(&path, "rust", &text);
            }
        } else {
            self.lsp.did_change_all(&path, self.buffer.version, &text);
        }
        self.last_sent_version
            .insert(path, self.buffer.version);
    }

    fn lsp_request_goto(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_definition(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    fn lsp_request_hover(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_hover(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    pub fn line_diagnostics(&self, line: usize) -> Vec<&Diagnostic> {
        let Some(path) = self.buffer.path.as_ref() else { return Vec::new(); };
        let Some(diags) = self.lsp.diagnostics_for(path) else { return Vec::new(); };
        diags
            .iter()
            .filter(|d| d.line == line)
            .collect()
    }

    pub fn worst_diagnostic(&self, line: usize) -> Option<Severity> {
        let mut worst: Option<Severity> = None;
        for d in self.line_diagnostics(line) {
            worst = match (worst, d.severity) {
                (None, s) => Some(s),
                (Some(Severity::Error), _) => Some(Severity::Error),
                (_, Severity::Error) => Some(Severity::Error),
                (Some(Severity::Warning), _) => Some(Severity::Warning),
                (_, Severity::Warning) => Some(Severity::Warning),
                (Some(s), _) => Some(s),
            };
        }
        worst
    }

    fn handle_event(&mut self) -> Result<()> {
        match event::read()? {
            Event::Key(k)
                if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                if !matches!(self.mode, Mode::Command) {
                    self.status_msg.clear();
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
                match self.mode {
                    Mode::Normal => self.handle_keyboard(k, ParseCtx::Normal),
                    Mode::Insert => self.handle_insert_key(k),
                    Mode::Command => self.handle_command_key(k),
                    Mode::Visual(_) => self.handle_keyboard(k, ParseCtx::Visual),
                    Mode::Search { .. } => self.handle_search_key(k),
                    Mode::Picker => self.handle_picker_key(k),
                }
            }
            Event::Resize(w, h) => {
                self.width = w;
                self.height = h;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_keyboard(&mut self, key: KeyEvent, ctx: ParseCtx) {
        match parser::parse(&mut self.pending, key, ctx) {
            ParseResult::Pending => {}
            ParseResult::Cancelled => {
                if matches!(self.mode, Mode::Visual(_)) {
                    self.exit_visual();
                }
            }
            ParseResult::Action(a) => self.apply_action(a),
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) {
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
                self.lsp_request_completion();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.buffer.insert_char(self.cursor.line, self.cursor.col, c);
                self.cursor.col += 1;
                self.cursor.want_col = self.cursor.col;
            }
            KeyCode::Enter => {
                self.buffer
                    .insert_char(self.cursor.line, self.cursor.col, '\n');
                self.cursor.line += 1;
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            KeyCode::Backspace => {
                if self.cursor.col > 0 {
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
            }
            KeyCode::Tab => {
                self.buffer.insert_str(self.cursor.line, self.cursor.col, "  ");
                self.cursor.col += 2;
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

    /// Return `true` if the key was handled by the completion popup (cycle / accept / dismiss).
    /// Otherwise close the popup and let the normal insert handler process the key.
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
            _ => {
                self.completion = None;
                false
            }
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
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
            ExCommand::Write => match self.buffer.save() {
                Ok(()) => {
                    let path = self
                        .buffer
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "[No Name]".into());
                    self.status_msg =
                        format!("\"{}\" {}L written", path, self.buffer.line_count());
                }
                Err(e) => self.status_msg = format!("error: {e}"),
            },
            ExCommand::WriteAs(p) => {
                self.buffer.path = Some(PathBuf::from(p));
                if let Err(e) = self.buffer.save() {
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
            ExCommand::WriteQuit => match self.buffer.save() {
                Ok(()) => self.should_quit = true,
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
            ExCommand::Goto(n) => {
                let m = motion::goto_line(&self.buffer, n);
                self.cursor = m.target;
            }
            ExCommand::Unknown(s) => {
                self.status_msg = format!("E492: Not an editor command: {s}");
            }
        }
    }

    fn apply_action(&mut self, action: Action) {
        self.maybe_record_edit(&action);
        match action {
            Action::Move { motion, count } => {
                if is_jump_motion(motion) {
                    self.push_jump();
                }
                let m = self.run_motion(motion, count);
                self.cursor = m.target;
                self.clamp_cursor_normal();
            }
            Action::Operate { op, motion, count, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                let m = self.run_motion(motion, count);
                self.apply_op_with_motion(op, m, register);
            }
            Action::OperateLine { op, count, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.apply_op_linewise(op, count, register);
            }
            Action::OperateTextObject { op, obj, count, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.apply_text_object(op, obj, count, register);
            }
            Action::EnterInsert(w) => self.enter_insert(w),
            Action::DeleteCharForward { count, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.delete_char_forward(count, register);
            }
            Action::ReplaceChar { ch, count } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.replace_char(ch, count);
            }
            Action::JoinLines { count } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.join_lines(count);
            }
            Action::ToggleCase { count } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.toggle_case(count);
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Put { before, count, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.put(before, count, register);
            }
            Action::EnterCommand => {
                self.cmdline.clear();
                self.mode = Mode::Command;
            }
            Action::EnterSearch { backward } => {
                self.cmdline.clear();
                self.mode = Mode::Search { backward };
            }
            Action::Repeat => self.repeat_last_edit(),
            Action::PageScroll(kind) => self.page_scroll(kind),
            Action::AdjustViewport(kind) => self.adjust_viewport_to(kind),
            Action::SetMark { name } => {
                self.marks.insert(name, (self.cursor.line, self.cursor.col));
            }
            Action::SearchWord { backward } => self.search_word_under_cursor(backward),
            Action::StartMacro { name } => self.start_macro_recording(name),
            Action::ReplayMacro { name } => self.replay_macro(name),
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
            Action::OpenPicker { kind } => self.open_picker(kind),
            Action::OpenYazi => self.open_yazi(),
            Action::LspGotoDefinition => self.lsp_request_goto(),
            Action::LspHover => self.lsp_request_hover(),
            Action::EnterVisual(kind) => {
                self.mode = Mode::Visual(kind);
                self.visual_anchor = Some(self.cursor);
            }
            Action::VisualOperate { op, register } => {
                self.history.record(&self.buffer.rope, self.cursor);
                self.apply_visual_operate(op, register);
            }
            Action::VisualSelectTextObject { obj } => {
                self.apply_visual_select_textobj(obj);
            }
            Action::VisualSwap => {
                if let Some(anchor) = self.visual_anchor {
                    self.visual_anchor = Some(self.cursor);
                    self.cursor = anchor;
                }
            }
            Action::VisualSwitch(target) => match self.mode {
                Mode::Visual(cur) if cur == target => self.exit_visual(),
                _ => self.mode = Mode::Visual(target),
            },
        }
    }

    fn exit_visual(&mut self) {
        self.mode = Mode::Normal;
        self.visual_anchor = None;
    }

    fn write_register(&mut self, target: Option<char>, text: String, linewise: bool) {
        if matches!(target, Some('_')) {
            return;
        }
        let r = Register { text, linewise };
        self.registers.insert('"', r.clone());
        if let Some(name) = target {
            if name != '"' {
                self.registers.insert(name, r);
            }
        }
    }

    fn write_yank_register(&mut self, target: Option<char>, text: String, linewise: bool) {
        if matches!(target, Some('_')) {
            return;
        }
        let r = Register { text, linewise };
        self.registers.insert('"', r.clone());
        self.registers.insert('0', r.clone());
        if let Some(name) = target {
            if name != '"' && name != '0' {
                self.registers.insert(name, r);
            }
        }
    }

    fn read_register(&self, name: Option<char>) -> Option<Register> {
        let key = name.unwrap_or('"');
        if key == '_' {
            return None;
        }
        self.registers.get(&key).cloned()
    }

    fn start_macro_recording(&mut self, name: char) {
        if self.recording_macro.is_some() {
            return;
        }
        self.recording_macro = Some(name);
        self.macro_buffer.clear();
        self.status_msg = format!("recording @{}", name);
    }

    fn replay_macro(&mut self, name: char) {
        let target = if name == '@' {
            self.last_replayed_macro
        } else {
            Some(name)
        };
        let Some(name) = target else {
            self.status_msg = "No previous macro".into();
            return;
        };
        let Some(keys) = self.macros.get(&name).cloned() else {
            self.status_msg = format!("Empty register: {}", name);
            return;
        };
        self.last_replayed_macro = Some(name);
        self.replaying_macro = true;
        for k in keys {
            match self.mode {
                Mode::Normal => self.handle_keyboard(k, ParseCtx::Normal),
                Mode::Insert => self.handle_insert_key(k),
                Mode::Command => self.handle_command_key(k),
                Mode::Visual(_) => self.handle_keyboard(k, ParseCtx::Visual),
                Mode::Search { .. } => self.handle_search_key(k),
                Mode::Picker => self.handle_picker_key(k),
            }
        }
        self.replaying_macro = false;
    }

    /// Decide whether an about-to-fire action should set up a recording for `.` repeat.
    fn maybe_record_edit(&mut self, action: &Action) {
        if self.replaying {
            return;
        }
        // Actions that enter insert mode begin a recording session that ends on Esc.
        let enters_insert = matches!(
            action,
            Action::EnterInsert(_)
                | Action::Operate { op: Operator::Change, .. }
                | Action::OperateLine { op: Operator::Change, .. }
                | Action::OperateTextObject { op: Operator::Change, .. }
        );
        if enters_insert {
            self.recording = Some(RecordingState { prelude: action.clone(), keys: Vec::new() });
            return;
        }
        let plain_recordable = match action {
            Action::Operate { op, .. }
            | Action::OperateLine { op, .. }
            | Action::OperateTextObject { op, .. } => matches!(op, Operator::Delete),
            Action::DeleteCharForward { .. }
            | Action::Put { .. }
            | Action::ReplaceChar { .. }
            | Action::JoinLines { .. }
            | Action::ToggleCase { .. } => true,
            _ => false,
        };
        if plain_recordable {
            self.last_edit = Some(LastEdit::Plain(action.clone()));
        }
    }

    fn repeat_last_edit(&mut self) {
        let Some(last) = self.last_edit.clone() else {
            self.status_msg = "No previous edit to repeat".into();
            return;
        };
        self.replaying = true;
        match last {
            LastEdit::Plain(action) => self.apply_action(action),
            LastEdit::InsertSession { prelude, keys } => {
                self.apply_action(prelude);
                for k in keys {
                    self.handle_insert_key(k);
                }
                let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                self.handle_insert_key(esc);
            }
        }
        self.replaying = false;
    }

    fn visual_range_chars(&self, kind: VisualKind) -> (usize, usize, bool) {
        let anchor = self.visual_anchor.unwrap_or(self.cursor);
        match kind {
            VisualKind::Char => {
                let a = self.buffer.pos_to_char(anchor.line, anchor.col);
                let c = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
                let total = self.buffer.total_chars();
                (lo, (hi + 1).min(total), false)
            }
            VisualKind::Line => {
                let l1 = anchor.line.min(self.cursor.line);
                let l2 = anchor.line.max(self.cursor.line);
                let s = self.buffer.line_start_idx(l1);
                let e = self.buffer.line_start_idx(l2 + 1);
                let total = self.buffer.total_chars();
                let extend = e == total && l1 > 0;
                let s_eff = if extend { s - 1 } else { s };
                (s_eff, e, true)
            }
        }
    }

    fn apply_visual_operate(&mut self, op: Operator, target: Option<char>) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
        let (start, end, linewise) = self.visual_range_chars(kind);
        if end <= start {
            self.exit_visual();
            return;
        }
        let removed = self.buffer.rope.slice(start..end).to_string();
        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, linewise);
                self.cursor_to_idx(start);
                self.clamp_cursor_normal();
                self.exit_visual();
            }
            Operator::Delete => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                self.cursor_to_idx(start);
                self.clamp_cursor_normal();
                self.exit_visual();
            }
            Operator::Change => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                if linewise {
                    self.buffer.insert_at_idx(start, "\n");
                }
                self.cursor_to_idx(start);
                self.mode = Mode::Insert;
                self.visual_anchor = None;
            }
        }
    }

    fn apply_text_object(
        &mut self,
        op: Operator,
        obj: TextObjectVerb,
        _count: usize,
        target: Option<char>,
    ) {
        // TODO: count > 1 should expand the object (e.g. d2aw = delete 2 around-words).
        let range = match text_object::compute(&self.buffer, self.cursor, obj) {
            Some(r) => r,
            None => return,
        };
        self.apply_op_to_range(op, range, target);
    }

    fn apply_op_to_range(&mut self, op: Operator, range: TextRange, target: Option<char>) {
        if range.end <= range.start {
            return;
        }
        let removed = self.buffer.rope.slice(range.start..range.end).to_string();
        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, range.linewise);
            }
            Operator::Delete => {
                self.write_register(target, removed, range.linewise);
                self.buffer.delete_range(range.start, range.end);
                self.cursor_to_idx(range.start);
                self.clamp_cursor_normal();
            }
            Operator::Change => {
                self.write_register(target, removed, range.linewise);
                self.buffer.delete_range(range.start, range.end);
                self.cursor_to_idx(range.start);
                self.mode = Mode::Insert;
            }
        }
    }

    fn apply_visual_select_textobj(&mut self, obj: TextObjectVerb) {
        let range = match text_object::compute(&self.buffer, self.cursor, obj) {
            Some(r) => r,
            None => return,
        };
        // Anchor → start, cursor → end-1 (inclusive endpoint for visual).
        self.cursor_to_idx(range.start);
        let anchor = self.cursor;
        let end_idx = range.end.saturating_sub(1).max(range.start);
        self.cursor_to_idx(end_idx);
        self.visual_anchor = Some(anchor);
    }

    fn run_motion(&mut self, m: MotionVerb, count: usize) -> MotionResult {
        match m {
            MotionVerb::Left => motion::left(&self.buffer, self.cursor, count),
            MotionVerb::Right => motion::right(&self.buffer, self.cursor, count),
            MotionVerb::Up => motion::up(&self.buffer, self.cursor, count),
            MotionVerb::Down => motion::down(&self.buffer, self.cursor, count),
            MotionVerb::LineStart => motion::line_start(&self.buffer, self.cursor),
            MotionVerb::LineEnd => motion::line_end(&self.buffer, self.cursor),
            MotionVerb::WordForward => motion::word_forward(&self.buffer, self.cursor, count),
            MotionVerb::WordBackward => motion::word_backward(&self.buffer, self.cursor, count),
            MotionVerb::BigWordForward => motion::big_word_forward(&self.buffer, self.cursor, count),
            MotionVerb::BigWordBackward => motion::big_word_backward(&self.buffer, self.cursor, count),
            MotionVerb::EndWord => motion::end_word(&self.buffer, self.cursor, count),
            MotionVerb::BigEndWord => motion::big_end_word(&self.buffer, self.cursor, count),
            MotionVerb::EndWordBackward => motion::end_word_backward(&self.buffer, self.cursor, count),
            MotionVerb::BigEndWordBackward => motion::big_end_word_backward(&self.buffer, self.cursor, count),
            MotionVerb::FirstLine => motion::first_line(&self.buffer, self.cursor),
            MotionVerb::LastLine => motion::last_line(&self.buffer, self.cursor),
            MotionVerb::GotoLine(n) => motion::goto_line(&self.buffer, n),
            MotionVerb::FirstNonBlank => motion::first_non_blank(&self.buffer, self.cursor),
            MotionVerb::LastNonBlank => motion::last_non_blank(&self.buffer, self.cursor),
            MotionVerb::ViewportTop => self.viewport_motion(0),
            MotionVerb::ViewportMiddle => self.viewport_motion(self.buffer_rows() / 2),
            MotionVerb::ViewportBottom => self.viewport_motion(self.buffer_rows().saturating_sub(1)),
            MotionVerb::Mark { name, exact } => self.mark_motion(name, exact),
            MotionVerb::FindChar { ch, forward, before } => {
                self.last_find = Some(FindRecord { ch, forward, before });
                motion::find_char(&self.buffer, self.cursor, ch, forward, before, count)
                    .unwrap_or(MotionResult { target: self.cursor, kind: MotionKind::CharExclusive })
            }
            MotionVerb::RepeatFind { reverse } => match self.last_find {
                Some(rec) => {
                    let forward = if reverse { !rec.forward } else { rec.forward };
                    motion::find_char(&self.buffer, self.cursor, rec.ch, forward, rec.before, count)
                        .unwrap_or(MotionResult { target: self.cursor, kind: MotionKind::CharExclusive })
                }
                None => MotionResult { target: self.cursor, kind: MotionKind::CharExclusive },
            },
            MotionVerb::SearchNext { reverse } => self.run_search_next(reverse, count),
        }
    }

    fn viewport_motion(&self, offset: usize) -> MotionResult {
        let line = (self.view_top + offset).min(self.buffer.line_count().saturating_sub(1));
        let r = motion::first_non_blank(&self.buffer, Cursor { line, col: 0, want_col: 0 });
        // Treat as linewise so operators like dH delete whole lines.
        MotionResult { target: r.target, kind: MotionKind::Linewise }
    }

    fn mark_motion(&self, name: char, exact: bool) -> MotionResult {
        let Some((mline, mcol)) = self.marks.get(&name).copied() else {
            return MotionResult {
                target: self.cursor,
                kind: MotionKind::CharExclusive,
            };
        };
        let last = self.buffer.line_count().saturating_sub(1);
        let line = mline.min(last);
        if exact {
            let len = self.buffer.line_len(line);
            let col = if len == 0 { 0 } else { mcol.min(len - 1) };
            MotionResult {
                target: Cursor { line, col, want_col: col },
                kind: MotionKind::CharExclusive,
            }
        } else {
            // ' jumps to first non-blank, linewise.
            let r = motion::first_non_blank(&self.buffer, Cursor { line, col: 0, want_col: 0 });
            MotionResult { target: r.target, kind: MotionKind::Linewise }
        }
    }

    fn page_scroll(&mut self, kind: PageScrollKind) {
        let rows = self.buffer_rows();
        if rows == 0 {
            return;
        }
        let last = self.buffer.line_count().saturating_sub(1);
        match kind {
            PageScrollKind::HalfDown | PageScrollKind::HalfUp => {
                let amount = (rows / 2).max(1);
                let down = matches!(kind, PageScrollKind::HalfDown);
                self.shift_view_and_cursor(amount, down, last);
            }
            PageScrollKind::FullDown | PageScrollKind::FullUp => {
                let amount = rows.saturating_sub(2).max(1);
                let down = matches!(kind, PageScrollKind::FullDown);
                self.shift_view_and_cursor(amount, down, last);
            }
            PageScrollKind::LineDown => {
                self.view_top = (self.view_top + 1).min(last);
                if self.cursor.line < self.view_top {
                    self.cursor.line = self.view_top;
                }
                self.snap_cursor_col_to_want();
            }
            PageScrollKind::LineUp => {
                self.view_top = self.view_top.saturating_sub(1);
                if self.cursor.line > self.view_top + rows.saturating_sub(1) {
                    self.cursor.line = self.view_top + rows.saturating_sub(1);
                }
                self.snap_cursor_col_to_want();
            }
        }
    }

    fn shift_view_and_cursor(&mut self, amount: usize, down: bool, last: usize) {
        if down {
            self.view_top = (self.view_top + amount).min(last);
            self.cursor.line = (self.cursor.line + amount).min(last);
        } else {
            self.view_top = self.view_top.saturating_sub(amount);
            self.cursor.line = self.cursor.line.saturating_sub(amount);
        }
        self.snap_cursor_col_to_want();
    }

    fn snap_cursor_col_to_want(&mut self) {
        let len = self.buffer.line_len(self.cursor.line);
        let max = if len == 0 { 0 } else { len - 1 };
        self.cursor.col = self.cursor.want_col.min(max);
    }

    fn adjust_viewport_to(&mut self, kind: ViewportAdjust) {
        let rows = self.buffer_rows();
        if rows == 0 {
            return;
        }
        let cur = self.cursor.line;
        self.view_top = match kind {
            ViewportAdjust::Top => cur,
            ViewportAdjust::Center => cur.saturating_sub(rows / 2),
            ViewportAdjust::Bottom => cur.saturating_sub(rows.saturating_sub(1)),
        };
    }

    fn search_word_under_cursor(&mut self, backward: bool) {
        let Some(word) = self.word_under_cursor() else {
            self.status_msg = "No word under cursor".into();
            return;
        };
        self.last_search = Some((word.clone(), backward));
        self.search_hl_off = false;
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let total = self.buffer.total_chars();
        let from = if backward {
            cur_idx.saturating_sub(1)
        } else {
            (cur_idx + 1).min(total)
        };
        match self.search(&word, from, !backward, true) {
            Some(idx) => {
                self.push_jump();
                self.cursor_to_idx(idx);
                self.clamp_cursor_normal();
            }
            None => self.status_msg = format!("Pattern not found: {word}"),
        }
    }

    fn push_jump(&mut self) {
        let pos = (self.cursor.line, self.cursor.col);
        // If we've stepped back via Ctrl-O, drop the forward history before pushing.
        self.jumplist.truncate(self.jump_idx);
        // Avoid duplicate consecutive entries.
        if self.jumplist.last() != Some(&pos) {
            self.jumplist.push(pos);
        }
        self.jump_idx = self.jumplist.len();
    }

    fn jump_back(&mut self) {
        if self.jump_idx == 0 {
            self.status_msg = "Already at oldest jump".into();
            return;
        }
        // If we're at the head, save current position so Ctrl-I can return to it.
        if self.jump_idx == self.jumplist.len() {
            let pos = (self.cursor.line, self.cursor.col);
            if self.jumplist.last() != Some(&pos) {
                self.jumplist.push(pos);
            }
        }
        self.jump_idx -= 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.cursor.line = l;
        self.cursor.col = c;
        self.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    fn jump_forward(&mut self) {
        if self.jump_idx + 1 >= self.jumplist.len() {
            self.status_msg = "Already at newest jump".into();
            return;
        }
        self.jump_idx += 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.cursor.line = l;
        self.cursor.col = c;
        self.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    fn word_under_cursor(&self) -> Option<String> {
        let line_len = self.buffer.line_len(self.cursor.line);
        if line_len == 0 {
            return None;
        }
        let cls = |c: char| -> u8 {
            if c.is_whitespace() {
                0
            } else if c.is_alphanumeric() || c == '_' {
                1
            } else {
                2
            }
        };
        let here = self.buffer.char_at(self.cursor.line, self.cursor.col)?;
        let here_class = cls(here);
        if here_class == 0 {
            return None;
        }
        let mut start = self.cursor.col;
        while start > 0 {
            let c = self.buffer.char_at(self.cursor.line, start - 1)?;
            if cls(c) == here_class {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = self.cursor.col + 1;
        while end < line_len {
            let c = self.buffer.char_at(self.cursor.line, end)?;
            if cls(c) == here_class {
                end += 1;
            } else {
                break;
            }
        }
        let line_start = self.buffer.line_start_idx(self.cursor.line);
        Some(
            self.buffer
                .rope
                .slice((line_start + start)..(line_start + end))
                .to_string(),
        )
    }

    fn run_search_next(&self, reverse: bool, _count: usize) -> MotionResult {
        let Some((query, was_backward)) = self.last_search.clone() else {
            return MotionResult { target: self.cursor, kind: MotionKind::CharExclusive };
        };
        // n continues original direction; N reverses it.
        let forward = if reverse { was_backward } else { !was_backward };
        let total = self.buffer.total_chars();
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let from = if forward { (cur_idx + 1).min(total) } else { cur_idx.saturating_sub(1) };
        match self.search(&query, from, forward, true) {
            Some(idx) => {
                let line = self.buffer.rope.char_to_line(idx);
                let col = idx - self.buffer.rope.line_to_char(line);
                MotionResult {
                    target: Cursor { line, col, want_col: col },
                    kind: MotionKind::CharExclusive,
                }
            }
            None => MotionResult { target: self.cursor, kind: MotionKind::CharExclusive },
        }
    }

    fn search(&self, query: &str, from_char: usize, forward: bool, wrap: bool) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let rope = &self.buffer.rope;
        let text = rope.to_string();
        let total = rope.len_chars();
        let from_byte = rope.char_to_byte(from_char.min(total));
        if forward {
            if let Some(b) = text.get(from_byte..).and_then(|s| s.find(query)) {
                return Some(rope.byte_to_char(from_byte + b));
            }
            if wrap {
                if let Some(b) = text.get(..from_byte).and_then(|s| s.find(query)) {
                    return Some(rope.byte_to_char(b));
                }
            }
        } else {
            if let Some(b) = text.get(..from_byte).and_then(|s| s.rfind(query)) {
                return Some(rope.byte_to_char(b));
            }
            if wrap {
                if let Some(b) = text.get(from_byte..).and_then(|s| s.rfind(query)) {
                    return Some(rope.byte_to_char(from_byte + b));
                }
            }
        }
        None
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cmdline.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let query = std::mem::take(&mut self.cmdline);
                let backward = match self.mode {
                    Mode::Search { backward } => backward,
                    _ => return,
                };
                self.mode = Mode::Normal;
                self.execute_search(&query, backward);
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

    fn execute_search(&mut self, query: &str, backward: bool) {
        let q = if query.is_empty() {
            match self.last_search.as_ref() {
                Some((q, _)) => q.clone(),
                None => return,
            }
        } else {
            query.to_string()
        };
        self.last_search = Some((q.clone(), backward));
        self.search_hl_off = false;
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let forward = !backward;
        match self.search(&q, cur_idx, forward, true) {
            Some(idx) => {
                self.push_jump();
                self.cursor_to_idx(idx);
                self.clamp_cursor_normal();
            }
            None => {
                self.status_msg = format!("Pattern not found: {q}");
            }
        }
    }

    fn apply_op_with_motion(&mut self, op: Operator, m: MotionResult, target: Option<char>) {
        let (start, end) = self.range_from_motion(m);
        if end <= start {
            return;
        }
        let removed = self.buffer.rope.slice(start..end).to_string();
        let linewise = matches!(m.kind, MotionKind::Linewise);

        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, linewise);
            }
            Operator::Delete => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                self.cursor_to_idx(start);
                self.clamp_cursor_normal();
            }
            Operator::Change => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                self.cursor_to_idx(start);
                self.mode = Mode::Insert;
            }
        }
    }

    fn apply_op_linewise(&mut self, op: Operator, count: usize, target: Option<char>) {
        let last_line = self.buffer.line_count().saturating_sub(1);
        let l1 = self.cursor.line;
        let l2 = (l1 + count - 1).min(last_line);
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2 + 1);
        let total = self.buffer.total_chars();
        let extend_back = end == total && l1 > 0;
        let effective_start = if extend_back { start - 1 } else { start };

        // Build register text — always presented as linewise (ends with '\n').
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

        match op {
            Operator::Yank => {
                self.write_yank_register(target, reg_text, true);
            }
            Operator::Delete => {
                self.write_register(target, reg_text, true);
                self.buffer.delete_range(effective_start, end);
                let new_last = self.buffer.line_count().saturating_sub(1);
                self.cursor.line = l1.min(new_last);
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            Operator::Change => {
                self.write_register(target, reg_text, true);
                self.buffer.delete_range(effective_start, end);
                self.buffer.insert_at_idx(effective_start, "\n");
                self.cursor.line = l1;
                self.cursor.col = 0;
                self.cursor.want_col = 0;
                self.mode = Mode::Insert;
            }
        }
    }

    fn range_from_motion(&self, m: MotionResult) -> (usize, usize) {
        let from = self.cursor;
        let mut to = m.target;
        let mut kind = m.kind;
        // Vim "exclusive becomes inclusive" rule: if the motion is exclusive and lands on
        // column 0 of a later line, push target back to end of the previous line and treat
        // as inclusive. This is what makes `dw` feel right across line breaks.
        if matches!(kind, MotionKind::CharExclusive) && to.col == 0 && to.line > from.line {
            let prev = to.line - 1;
            let len = self.buffer.line_len(prev);
            let col = if len == 0 { 0 } else { len - 1 };
            to = Cursor { line: prev, col, want_col: col };
            kind = MotionKind::CharInclusive;
        }
        match kind {
            MotionKind::CharExclusive => {
                let f = self.buffer.pos_to_char(from.line, from.col);
                let t = self.buffer.pos_to_char(to.line, to.col);
                if f <= t { (f, t) } else { (t, f) }
            }
            MotionKind::CharInclusive => {
                let f = self.buffer.pos_to_char(from.line, from.col);
                let t = self.buffer.pos_to_char(to.line, to.col);
                if f <= t {
                    (f, (t + 1).min(self.buffer.total_chars()))
                } else {
                    (t, (f + 1).min(self.buffer.total_chars()))
                }
            }
            MotionKind::Linewise => {
                let l1 = from.line.min(to.line);
                let l2 = from.line.max(to.line);
                let start = self.buffer.line_start_idx(l1);
                let end = self.buffer.line_start_idx(l2 + 1);
                (start, end)
            }
        }
    }

    fn enter_insert(&mut self, w: InsertWhere) {
        self.history.record(&self.buffer.rope, self.cursor);
        match w {
            InsertWhere::Cursor => {}
            InsertWhere::AfterCursor => {
                let len = self.buffer.line_len(self.cursor.line);
                if self.cursor.col < len {
                    self.cursor.col += 1;
                    self.cursor.want_col = self.cursor.col;
                }
            }
            InsertWhere::LineBelow => {
                let len = self.buffer.line_len(self.cursor.line);
                let idx = self.buffer.pos_to_char(self.cursor.line, len);
                self.buffer.insert_at_idx(idx, "\n");
                self.cursor.line += 1;
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            InsertWhere::LineAbove => {
                let idx = self.buffer.line_start_idx(self.cursor.line);
                self.buffer.insert_at_idx(idx, "\n");
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
        }
        self.mode = Mode::Insert;
    }

    fn replace_char(&mut self, ch: char, count: usize) {
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        let start = self.buffer.pos_to_char(line, self.cursor.col);
        let max_end = self.buffer.pos_to_char(line, line_len);
        let end = (start + count.max(1)).min(max_end);
        let actual = end - start;
        if actual == 0 {
            return;
        }
        self.buffer.delete_range(start, end);
        let mut buf = String::new();
        for _ in 0..actual {
            buf.push(ch);
        }
        self.buffer.insert_at_idx(start, &buf);
        self.cursor.col = self.cursor.col + actual.saturating_sub(1);
        self.cursor.want_col = self.cursor.col;
        self.clamp_cursor_normal();
    }

    fn join_lines(&mut self, count: usize) {
        let times = count.max(1);
        for _ in 0..times {
            let cur_line = self.cursor.line;
            if cur_line + 1 >= self.buffer.line_count() {
                break;
            }
            let line_len = self.buffer.line_len(cur_line);
            let nl_idx = self.buffer.pos_to_char(cur_line, line_len);
            // Skip leading whitespace on the next line.
            let next_len = self.buffer.line_len(cur_line + 1);
            let mut skip = 0usize;
            while skip < next_len {
                match self.buffer.char_at(cur_line + 1, skip) {
                    Some(c) if c.is_whitespace() => skip += 1,
                    _ => break,
                }
            }
            self.buffer.delete_range(nl_idx, nl_idx + 1 + skip);
            // Insert a single space unless the cur line is empty or already ends in whitespace,
            // or the next line started with `)`.
            let cur_ends_ws = line_len > 0
                && self
                    .buffer
                    .char_at(cur_line, line_len - 1)
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false);
            let next_starts_close = self
                .buffer
                .char_at(cur_line, line_len)
                .map(|c| c == ')')
                .unwrap_or(false);
            let insert_space = line_len > 0 && !cur_ends_ws && !next_starts_close;
            if insert_space {
                self.buffer.insert_at_idx(nl_idx, " ");
            }
            self.cursor.col = line_len;
            self.cursor.want_col = self.cursor.col;
        }
        self.clamp_cursor_normal();
    }

    fn toggle_case(&mut self, count: usize) {
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        for _ in 0..count.max(1) {
            if self.cursor.col >= self.buffer.line_len(self.cursor.line) {
                break;
            }
            let c = match self.buffer.char_at(self.cursor.line, self.cursor.col) {
                Some(c) => c,
                None => break,
            };
            let new_c = if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                c
            };
            let idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
            self.buffer.delete_range(idx, idx + 1);
            self.buffer.insert_char(self.cursor.line, self.cursor.col, new_c);
            // Advance unless we're at end of line.
            let len_now = self.buffer.line_len(self.cursor.line);
            if self.cursor.col + 1 < len_now {
                self.cursor.col += 1;
            }
        }
        self.cursor.want_col = self.cursor.col;
        self.clamp_cursor_normal();
    }

    fn delete_char_forward(&mut self, count: usize, target: Option<char>) {
        let line_len = self.buffer.line_len(self.cursor.line);
        if line_len == 0 {
            return;
        }
        let start = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let max_end = self.buffer.pos_to_char(self.cursor.line, line_len);
        let end = (start + count).min(max_end);
        let removed = self.buffer.delete_range(start, end);
        if !removed.is_empty() {
            self.write_register(target, removed, false);
        }
        self.clamp_cursor_normal();
    }

    fn put(&mut self, before: bool, count: usize, target: Option<char>) {
        let Some(reg) = self.read_register(target) else {
            return;
        };
        if reg.text.is_empty() {
            return;
        }
        if reg.linewise {
            let target_line = if before {
                self.cursor.line
            } else {
                self.cursor.line + 1
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let total = self.buffer.total_chars();
            let idx = self.buffer.line_start_idx(target_line);
            // If pasting "below" past the end of a file with no trailing newline,
            // we need to lead with a newline rather than trailing one.
            let has_trailing_nl = total == 0
                || self
                    .buffer
                    .rope
                    .get_char(total - 1)
                    .map(|c| c == '\n')
                    .unwrap_or(false);
            if idx >= total && !has_trailing_nl {
                let to_insert = format!("\n{}", text.trim_end_matches('\n'));
                self.buffer.insert_at_idx(idx, &to_insert);
            } else {
                self.buffer.insert_at_idx(idx, &text);
            }
            self.cursor.line = target_line;
            self.cursor.col = 0;
            self.cursor.want_col = 0;
        } else {
            let target_idx = if before {
                self.buffer.pos_to_char(self.cursor.line, self.cursor.col)
            } else {
                let line_len = self.buffer.line_len(self.cursor.line);
                if line_len == 0 {
                    self.buffer.line_start_idx(self.cursor.line)
                } else {
                    self.buffer
                        .pos_to_char(self.cursor.line, self.cursor.col + 1)
                }
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            let inserted_chars = text.chars().count();
            self.buffer.insert_at_idx(target_idx, &text);
            if inserted_chars > 0 {
                let new_idx = target_idx + inserted_chars - 1;
                self.cursor_to_idx(new_idx);
            }
            self.clamp_cursor_normal();
        }
    }

    fn undo(&mut self) {
        if let Some(snap) = self.history.undo(&self.buffer.rope, self.cursor) {
            self.buffer.rope = snap.rope;
            self.cursor = snap.cursor;
            self.buffer.dirty = true;
            self.clamp_cursor_normal();
        } else {
            self.status_msg = "Already at oldest change".into();
        }
    }

    fn redo(&mut self) {
        if let Some(snap) = self.history.redo(&self.buffer.rope, self.cursor) {
            self.buffer.rope = snap.rope;
            self.cursor = snap.cursor;
            self.buffer.dirty = true;
            self.clamp_cursor_normal();
        } else {
            self.status_msg = "Already at newest change".into();
        }
    }

    fn cursor_to_idx(&mut self, idx: usize) {
        let total = self.buffer.total_chars();
        let idx = idx.min(total);
        let line = self.buffer.rope.char_to_line(idx);
        let line_start = self.buffer.rope.line_to_char(line);
        let col = idx - line_start;
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.want_col = col;
    }

    fn clamp_cursor_normal(&mut self) {
        let last = self.buffer.line_count().saturating_sub(1);
        if self.cursor.line > last {
            self.cursor.line = last;
        }
        let len = self.buffer.line_len(self.cursor.line);
        let max = if len == 0 { 0 } else { len - 1 };
        if self.cursor.col > max {
            self.cursor.col = max;
        }
    }

    fn adjust_viewport(&mut self) {
        let buffer_rows = self.buffer_rows();
        if buffer_rows == 0 {
            return;
        }
        let scrolloff = 3.min(buffer_rows / 2);
        let cur = self.cursor.line;
        if cur < self.view_top + scrolloff {
            self.view_top = cur.saturating_sub(scrolloff);
        }
        if cur >= self.view_top + buffer_rows.saturating_sub(scrolloff) {
            let want = cur + scrolloff + 1;
            self.view_top = want.saturating_sub(buffer_rows);
        }
    }

    pub fn buffer_rows(&self) -> usize {
        (self.height as usize).saturating_sub(2)
    }

    pub fn gutter_width(&self) -> usize {
        let n = self.buffer.line_count();
        let digits = format!("{n}").len();
        // 1 sign column + digits + 1 trailing space.
        digits + 2
    }

    /// Char-column ranges of search-highlight matches on `line`.
    pub fn line_search_matches(&self, line: usize) -> Vec<(usize, usize)> {
        if self.search_hl_off {
            return Vec::new();
        }
        let Some((q, _)) = &self.last_search else {
            return Vec::new();
        };
        if q.is_empty() {
            return Vec::new();
        }
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return Vec::new();
        }
        let line_start = self.buffer.line_start_idx(line);
        let text: String = self
            .buffer
            .rope
            .slice(line_start..(line_start + line_len))
            .to_string();
        let qlen = q.chars().count();
        let mut out = Vec::new();
        let mut byte = 0usize;
        while byte <= text.len() {
            let Some(rel) = text[byte..].find(q.as_str()) else {
                break;
            };
            let abs_byte = byte + rel;
            let char_start = text[..abs_byte].chars().count();
            out.push((char_start, char_start + qlen));
            byte = abs_byte + q.len().max(1);
        }
        out
    }

    /// For visual mode rendering: return the half-open `[start_col, end_col)` of selected
    /// chars on this line, or `None` if none. For V-line, returns full line range.
    pub fn line_selection(&self, line: usize) -> Option<(usize, usize)> {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return None,
        };
        let anchor = self.visual_anchor?;
        let cursor = self.cursor;
        let (lo, hi) = if (anchor.line, anchor.col) <= (cursor.line, cursor.col) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        if line < lo.line || line > hi.line {
            return None;
        }
        let line_len = self.buffer.line_len(line);
        match kind {
            VisualKind::Line => {
                let end = if line_len == 0 { 1 } else { line_len };
                Some((0, end))
            }
            VisualKind::Char => {
                let start_col = if line == lo.line { lo.col } else { 0 };
                let end_col = if line == hi.line {
                    (hi.col + 1).min(line_len.max(1))
                } else {
                    line_len.max(1)
                };
                Some((start_col, end_col))
            }
        }
    }
}

impl App {
    fn snapshot_active(&mut self) -> BufferStash {
        BufferStash {
            buffer: std::mem::take(&mut self.buffer),
            cursor: std::mem::take(&mut self.cursor),
            view_top: std::mem::take(&mut self.view_top),
            history: std::mem::take(&mut self.history),
            visual_anchor: self.visual_anchor.take(),
            marks: std::mem::take(&mut self.marks),
            jumplist: std::mem::take(&mut self.jumplist),
            jump_idx: std::mem::take(&mut self.jump_idx),
        }
    }

    fn load_stash(&mut self, stash: BufferStash) {
        self.buffer = stash.buffer;
        self.cursor = stash.cursor;
        self.view_top = stash.view_top;
        self.history = stash.history;
        self.visual_anchor = stash.visual_anchor;
        self.marks = stash.marks;
        self.jumplist = stash.jumplist;
        self.jump_idx = stash.jump_idx;
    }

    fn switch_to(&mut self, idx: usize) -> Result<()> {
        if idx >= self.buffers.len() {
            anyhow::bail!("invalid buffer index {idx}");
        }
        if idx == self.active {
            return Ok(());
        }
        let active = self.active;
        let snap = self.snapshot_active();
        self.buffers[active] = snap;
        let stash = std::mem::take(&mut self.buffers[idx]);
        self.load_stash(stash);
        self.active = idx;
        Ok(())
    }

    pub fn open_buffer(&mut self, path: PathBuf) -> Result<()> {
        // Switch to existing buffer if this path is already open.
        if self.buffer.path.as_deref() == Some(path.as_path()) {
            return Ok(());
        }
        for (i, stash) in self.buffers.iter().enumerate() {
            if i == self.active {
                continue;
            }
            if stash.buffer.path.as_deref() == Some(path.as_path()) {
                return self.switch_to(i);
            }
        }
        let buf = Buffer::from_path(path)?;
        let stash = BufferStash {
            buffer: buf,
            ..Default::default()
        };
        self.buffers.push(stash);
        let new_idx = self.buffers.len() - 1;
        self.switch_to(new_idx)?;
        self.lsp_attach_active();
        Ok(())
    }

    fn cycle_buffer(&mut self, step: i64) {
        if self.buffers.len() <= 1 {
            self.status_msg = "Only one buffer".into();
            return;
        }
        let n = self.buffers.len() as i64;
        let next = ((self.active as i64) + step).rem_euclid(n) as usize;
        if let Err(e) = self.switch_to(next) {
            self.status_msg = format!("error: {e}");
        }
    }

    fn switch_buffer_by_spec(&mut self, spec: &str) -> Result<()> {
        let spec = spec.trim();
        if spec.is_empty() {
            anyhow::bail!("E94: No matching buffer");
        }
        // Numeric: 1-based buffer number.
        if let Ok(n) = spec.parse::<usize>() {
            if n == 0 || n > self.buffers.len() {
                anyhow::bail!("E86: Buffer {n} does not exist");
            }
            return self.switch_to(n - 1);
        }
        // Substring match against buffer paths.
        let mut matches: Vec<usize> = Vec::new();
        for (i, stash) in self.buffers.iter().enumerate() {
            let path = if i == self.active {
                self.buffer.path.as_ref()
            } else {
                stash.buffer.path.as_ref()
            };
            if let Some(p) = path {
                if p.to_string_lossy().contains(spec) {
                    matches.push(i);
                }
            }
        }
        match matches.len() {
            0 => anyhow::bail!("E94: No matching buffer for '{spec}'"),
            1 => self.switch_to(matches[0]),
            _ => anyhow::bail!("E93: More than one match for '{spec}'"),
        }
    }

    fn delete_buffer(&mut self, force: bool) -> Result<()> {
        if !force && self.buffer.dirty {
            anyhow::bail!("E89: No write since last change (use :bd!)");
        }
        if self.buffers.len() == 1 {
            // Last buffer — replace with an empty one.
            self.buffer = Buffer::empty();
            self.cursor = Cursor::default();
            self.view_top = 0;
            self.history = History::default();
            self.visual_anchor = None;
            self.marks.clear();
            self.jumplist.clear();
            self.jump_idx = 0;
            self.buffers[0] = BufferStash::default();
            self.status_msg = "Buffer closed".into();
            return Ok(());
        }
        let prev = self.active;
        let next = if prev + 1 < self.buffers.len() { prev + 1 } else { prev - 1 };
        self.switch_to(next)?;
        // Now the slot at `prev` holds the snapshot we want to drop.
        self.buffers.remove(prev);
        if self.active > prev {
            self.active -= 1;
        }
        Ok(())
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

    fn substitute(&mut self, range: ExRange, pat: &str, repl: &str, global: bool) -> usize {
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
        self.status_msg = format!("{} lines yanked", l2 - l1 + 1);
    }

    fn open_yazi(&mut self) {
        use crossterm::{
            cursor::{Hide, Show},
            execute,
            terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
        };
        use std::process::Command;

        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let chooser = std::env::temp_dir()
            .join(format!("binvim-yazi-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&chooser);

        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();

        let status = Command::new("yazi")
            .arg("--chooser-file")
            .arg(&chooser)
            .arg(&start_dir)
            .status();

        let _ = enable_raw_mode();
        let _ = execute!(stdout, EnterAlternateScreen, Hide);

        match status {
            Err(_) => {
                self.status_msg = "yazi not on PATH".into();
            }
            Ok(_) => {
                if let Ok(text) = std::fs::read_to_string(&chooser) {
                    for line in text.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let path = PathBuf::from(trimmed);
                        if let Err(e) = self.open_buffer(path) {
                            self.status_msg = format!("error: {e}");
                        }
                        break;
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&chooser);
    }

    fn open_picker(&mut self, kind: PickerLeader) {
        let state = match kind {
            PickerLeader::Files => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let items = picker::enumerate_files(&cwd, 5000);
                if items.is_empty() {
                    self.status_msg = "No files found".into();
                    return;
                }
                PickerState::new(PickerKind::Files, "Files".into(), items)
            }
            PickerLeader::Grep => {
                PickerState::new(PickerKind::Grep, "Grep".into(), Vec::new())
            }
            PickerLeader::Buffers => {
                let mut items: Vec<(String, PickerPayload)> = Vec::new();
                for (i, stash) in self.buffers.iter().enumerate() {
                    let name = if i == self.active {
                        self.buffer
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "[No Name]".into())
                    } else {
                        stash
                            .buffer
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "[No Name]".into())
                    };
                    items.push((name, PickerPayload::BufferIdx(i)));
                }
                PickerState::new(PickerKind::Buffers, "Buffers".into(), items)
            }
        };
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.picker.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let payload = picker.current().cloned();
                self.picker = None;
                self.mode = Mode::Normal;
                if let Some(p) = payload {
                    match p {
                        PickerPayload::Path(path) => {
                            if let Err(e) = self.open_buffer(path) {
                                self.status_msg = format!("error: {e}");
                            }
                        }
                        PickerPayload::BufferIdx(idx) => {
                            if let Err(e) = self.switch_to(idx) {
                                self.status_msg = format!("error: {e}");
                            }
                        }
                        PickerPayload::Location { path, line, col } => {
                            if let Err(e) = self.open_buffer(path) {
                                self.status_msg = format!("error: {e}");
                            } else {
                                self.push_jump();
                                self.cursor.line = line.saturating_sub(1);
                                self.cursor.col = col.saturating_sub(1);
                                self.cursor.want_col = self.cursor.col;
                                self.clamp_cursor_normal();
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                picker.input.pop();
                self.refilter_picker();
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'n' | 'j' => picker.move_down(),
                'p' | 'k' => picker.move_up(),
                _ => {}
            },
            KeyCode::Char(c) => {
                picker.input.push(c);
                self.refilter_picker();
            }
            _ => {}
        }
    }

    fn refilter_picker(&mut self) {
        let Some(picker) = self.picker.as_mut() else { return; };
        match picker.kind {
            PickerKind::Files | PickerKind::Buffers => picker.refilter(),
            PickerKind::Grep => {
                if picker.input.len() < 2 {
                    picker::replace_items(picker, Vec::new());
                    return;
                }
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let query = picker.input.clone();
                let results = picker::run_ripgrep(&query, &cwd, 500);
                picker::replace_items(picker, results);
            }
        }
    }

    fn ensure_highlights(&mut self) {
        let lang = self
            .buffer
            .path
            .as_deref()
            .and_then(lang::Lang::detect);
        let need_refresh = match (&self.highlight_cache, lang) {
            (None, Some(_)) => true,
            (Some(c), Some(l)) => c.lang != l || c.buffer_version != self.buffer.version,
            (Some(_), None) => true,
            (None, None) => false,
        };
        if !need_refresh {
            return;
        }
        self.highlight_cache = match lang {
            Some(l) => lang::compute_highlights(l, &self.buffer, &self.config),
            None => None,
        };
    }

    fn list_buffers(&self) -> String {
        let mut out = String::new();
        for (i, stash) in self.buffers.iter().enumerate() {
            let (path, dirty) = if i == self.active {
                (
                    self.buffer.path.as_ref().map(|p| p.display().to_string()),
                    self.buffer.dirty,
                )
            } else {
                (
                    stash.buffer.path.as_ref().map(|p| p.display().to_string()),
                    stash.buffer.dirty,
                )
            };
            let name = path.unwrap_or_else(|| "[No Name]".into());
            let marker = if i == self.active { "%" } else { " " };
            let dirty_marker = if dirty { "+" } else { " " };
            if !out.is_empty() {
                out.push_str(" | ");
            }
            out.push_str(&format!("{} {}{} {}", i + 1, marker, dirty_marker, name));
        }
        if out.is_empty() {
            "[No buffers]".into()
        } else {
            out
        }
    }
}

fn is_jump_motion(m: MotionVerb) -> bool {
    matches!(
        m,
        MotionVerb::FirstLine
            | MotionVerb::LastLine
            | MotionVerb::GotoLine(_)
            | MotionVerb::Mark { .. }
            | MotionVerb::ViewportTop
            | MotionVerb::ViewportMiddle
            | MotionVerb::ViewportBottom
            | MotionVerb::SearchNext { .. }
    )
}

struct TerminalGuard;

impl TerminalGuard {
    fn enable() -> Result<Self> {
        use crossterm::{
            execute,
            terminal::{enable_raw_mode, EnterAlternateScreen},
        };
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        use crossterm::{
            cursor::{SetCursorStyle, Show},
            execute,
            terminal::{disable_raw_mode, LeaveAlternateScreen},
        };
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            SetCursorStyle::DefaultUserShape,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
