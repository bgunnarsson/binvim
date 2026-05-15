//! Action dispatch — `apply_action` and the operator/motion/text-object
//! glue. Every keybinding ultimately resolves to an `Action`, which lands
//! here and fans out to the relevant primitive on `App`.

use crate::cursor::Cursor;
use crate::mode::{Mode, Operator};
use crate::motion::{self, MotionKind, MotionResult};
use crate::parser::{Action, MotionVerb};
use crate::text_object::{self, TextObjectVerb, TextRange};

use super::state::{is_jump_motion, FindRecord};

impl super::App {
    pub(super) fn apply_action(&mut self, action: Action) {
        self.maybe_record_edit(&action);
        match action {
            Action::Move { motion, count } => {
                if is_jump_motion(motion) {
                    self.push_jump();
                }
                let m = self.run_motion(motion, count);
                self.window.cursor = m.target;
                self.clamp_cursor_normal();
            }
            Action::Operate { op, motion, count, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                if !self.try_multi_op_motion(op, motion, count, register) {
                    let m = self.run_motion(motion, count);
                    self.apply_op_with_motion(op, m, register);
                }
            }
            Action::OperateLine { op, count, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                if !self.try_multi_op_linewise(op, count, register) {
                    self.apply_op_linewise(op, count, register);
                }
            }
            Action::OperateTextObject { op, obj, count, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                if !self.try_multi_op_textobj(op, obj, register) {
                    self.apply_text_object(op, obj, count, register);
                }
            }
            Action::EnterInsert(w) => self.enter_insert(w),
            Action::DeleteCharForward { count, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                if !self.try_multi_delete_char(count, register) {
                    self.delete_char_forward(count, register);
                }
            }
            Action::ReplaceChar { ch, count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.replace_char(ch, count);
            }
            Action::JoinLines { count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.join_lines(count);
            }
            Action::AdjustNumber { delta, count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.adjust_number(delta, count);
            }
            Action::MoveLine { down, count } => self.move_lines(down, count),
            Action::ToggleCase { count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.toggle_case(count);
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Put { before, count, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
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
                self.marks.insert(name, (self.window.cursor.line, self.window.cursor.col));
            }
            Action::SearchWord { backward } => self.search_word_under_cursor(backward),
            Action::StartMacro { name } => self.start_macro_recording(name),
            Action::ReplayMacro { name } => self.replay_macro(name),
            Action::BufferDelete { force } => {
                if let Err(e) = self.delete_buffer(force) {
                    self.status_msg = format!("error: {e}");
                }
            }
            Action::BufferDeleteAll { force } => {
                if let Err(e) = self.delete_all_buffers(force) {
                    self.status_msg = format!("error: {e}");
                }
            }
            Action::BufferOnly => {
                if let Err(e) = self.buffer_only() {
                    self.status_msg = format!("error: {e}");
                }
            }
            Action::BufferNext => self.cycle_buffer(1),
            Action::BufferPrev => self.cycle_buffer(-1),
            Action::QuickfixNext => self.qf_next(),
            Action::QuickfixPrev => self.qf_prev(),
            Action::HunkNext => self.hunk_jump(true),
            Action::HunkPrev => self.hunk_jump(false),
            Action::HunkPreview => self.hunk_preview(),
            Action::HunkStage => self.hunk_stage(),
            Action::HunkUnstage => self.hunk_unstage(),
            Action::HunkReset => self.hunk_reset(),
            Action::WindowSplitVertical => self.window_split(crate::layout::SplitDir::Vertical),
            Action::WindowSplitHorizontal => self.window_split(crate::layout::SplitDir::Horizontal),
            Action::WindowSplitVerticalPick => {
                self.window_split(crate::layout::SplitDir::Vertical);
                self.open_picker(crate::parser::PickerLeader::Files);
            }
            Action::WindowSplitHorizontalPick => {
                self.window_split(crate::layout::SplitDir::Horizontal);
                self.open_picker(crate::parser::PickerLeader::Files);
            }
            Action::WindowFocus { dir } => self.window_focus(dir),
            Action::WindowClose => self.window_close(),
            Action::WindowOnly => self.window_only(),
            Action::WindowEqualize => self.layout.equalize(),
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
            Action::OpenPicker { kind } => self.open_picker(kind),
            Action::OpenYazi => self.open_yazi(),
            Action::LspGotoDefinition => self.lsp_request_goto(),
            Action::LspFindReferences => self.lsp_request_references(),
            Action::LspRename => self.start_rename_prompt(),
            Action::ReplaceAllInBuffer => self.start_replace_all_prompt(),
            Action::Format => self.format_active(),
            Action::Debug(d) => {
                use crate::command::DebugSubCmd;
                use crate::parser::DebugAction;
                let sub = match d {
                    DebugAction::Start => DebugSubCmd::Start,
                    DebugAction::Stop => DebugSubCmd::Stop,
                    DebugAction::ToggleBreakpoint => DebugSubCmd::Break,
                    DebugAction::ClearBreakpointsInFile => DebugSubCmd::ClearBreakpointsInFile,
                    DebugAction::Continue => DebugSubCmd::Continue,
                    DebugAction::Next => DebugSubCmd::Next,
                    DebugAction::StepIn => DebugSubCmd::StepIn,
                    DebugAction::StepOut => DebugSubCmd::StepOut,
                    DebugAction::PaneToggle => DebugSubCmd::PaneToggle,
                    DebugAction::FocusPane => DebugSubCmd::FocusPane,
                };
                self.dispatch_debug(sub);
            }
            Action::AddNextOccurrenceSelection => self.add_next_occurrence_selection(),
            Action::SurroundDelete { ch } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.surround_delete(ch);
            }
            Action::SurroundChange { from, to } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.surround_change(from, to);
            }
            Action::SurroundVisual { ch } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.surround_visual(ch);
            }
            Action::Fold(op) => self.apply_fold_op(op),
            Action::LspHover => self.lsp_request_hover(),
            Action::EnterVisual(kind) => {
                self.mode = Mode::Visual(kind);
                self.window.visual_anchor = Some(self.window.cursor);
            }
            Action::VisualOperate { op, register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.apply_visual_operate(op, register);
            }
            Action::VisualPut { register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.apply_visual_put(register);
            }
            Action::VisualSelectTextObject { obj } => {
                self.apply_visual_select_textobj(obj);
            }
            Action::VisualSwap => {
                if let Some(anchor) = self.window.visual_anchor {
                    self.window.visual_anchor = Some(self.window.cursor);
                    self.window.cursor = anchor;
                }
            }
            Action::VisualSwitch(target) => match self.mode {
                Mode::Visual(cur) if cur == target => self.exit_visual(),
                _ => {
                    // Switching kinds invalidates multi-selection — the
                    // ranges were computed under the old kind's geometry.
                    self.additional_selections.clear();
                    self.mode = Mode::Visual(target);
                }
            },
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
        let range = match text_object::compute(&self.buffer, self.window.cursor, obj) {
            Some(r) => r,
            None => return,
        };
        self.apply_op_to_range(op, range, target);
    }

    fn apply_op_to_range(&mut self, op: Operator, range: TextRange, target: Option<char>) {
        if range.end <= range.start {
            return;
        }
        // Indent / outdent on a text-object range: derive line span and shift them.
        if matches!(op, Operator::Indent | Operator::Outdent) {
            let l1 = self.buffer.rope.char_to_line(range.start);
            let l2_idx = range.end.saturating_sub(1);
            let l2 = self.buffer.rope.char_to_line(l2_idx.min(self.buffer.total_chars()));
            if matches!(op, Operator::Indent) {
                self.indent_lines(l1, l2);
            } else {
                self.outdent_lines(l1, l2);
            }
            return;
        }
        let removed = self.buffer.rope.slice(range.start..range.end).to_string();
        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, range.linewise);
                self.flash_yank(range.start, range.end);
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
            Operator::Indent | Operator::Outdent => unreachable!(),
        }
    }

    fn run_motion(&mut self, m: MotionVerb, count: usize) -> MotionResult {
        match m {
            MotionVerb::Left => motion::left(&self.buffer, self.window.cursor, count),
            MotionVerb::Right => motion::right(&self.buffer, self.window.cursor, count),
            MotionVerb::Up => {
                let mut r = motion::up(&self.buffer, self.window.cursor, count);
                if self.markdown_render_active() {
                    r.target = self.adjust_target_past_md_hidden(r.target, -1);
                }
                r
            }
            MotionVerb::Down => {
                let mut r = motion::down(&self.buffer, self.window.cursor, count);
                if self.markdown_render_active() {
                    r.target = self.adjust_target_past_md_hidden(r.target, 1);
                }
                r
            }
            MotionVerb::LineStart => motion::line_start(&self.buffer, self.window.cursor),
            MotionVerb::LineEnd => motion::line_end(&self.buffer, self.window.cursor),
            MotionVerb::WordForward => motion::word_forward(&self.buffer, self.window.cursor, count),
            MotionVerb::WordBackward => motion::word_backward(&self.buffer, self.window.cursor, count),
            MotionVerb::BigWordForward => motion::big_word_forward(&self.buffer, self.window.cursor, count),
            MotionVerb::BigWordBackward => motion::big_word_backward(&self.buffer, self.window.cursor, count),
            MotionVerb::EndWord => motion::end_word(&self.buffer, self.window.cursor, count),
            MotionVerb::BigEndWord => motion::big_end_word(&self.buffer, self.window.cursor, count),
            MotionVerb::EndWordBackward => motion::end_word_backward(&self.buffer, self.window.cursor, count),
            MotionVerb::BigEndWordBackward => motion::big_end_word_backward(&self.buffer, self.window.cursor, count),
            MotionVerb::FirstLine => motion::first_line(&self.buffer, self.window.cursor),
            MotionVerb::LastLine => motion::last_line(&self.buffer, self.window.cursor),
            MotionVerb::GotoLine(n) => motion::goto_line(&self.buffer, n),
            MotionVerb::FirstNonBlank => motion::first_non_blank(&self.buffer, self.window.cursor),
            MotionVerb::LastNonBlank => motion::last_non_blank(&self.buffer, self.window.cursor),
            MotionVerb::ViewportTop => self.viewport_motion(0),
            MotionVerb::ViewportMiddle => self.viewport_motion(self.buffer_rows() / 2),
            MotionVerb::ViewportBottom => self.viewport_motion(self.buffer_rows().saturating_sub(1)),
            MotionVerb::Mark { name, exact } => self.mark_motion(name, exact),
            MotionVerb::FindChar { ch, forward, before } => {
                self.last_find = Some(FindRecord { ch, forward, before });
                motion::find_char(&self.buffer, self.window.cursor, ch, forward, before, count)
                    .unwrap_or(MotionResult { target: self.window.cursor, kind: MotionKind::CharExclusive })
            }
            MotionVerb::RepeatFind { reverse } => match self.last_find {
                Some(rec) => {
                    let forward = if reverse { !rec.forward } else { rec.forward };
                    motion::find_char(&self.buffer, self.window.cursor, rec.ch, forward, rec.before, count)
                        .unwrap_or(MotionResult { target: self.window.cursor, kind: MotionKind::CharExclusive })
                }
                None => MotionResult { target: self.window.cursor, kind: MotionKind::CharExclusive },
            },
            MotionVerb::SearchNext { reverse } => self.run_search_next(reverse, count),
        }
    }

    fn viewport_motion(&self, offset: usize) -> MotionResult {
        let line = (self.window.view_top + offset).min(self.buffer.line_count().saturating_sub(1));
        let r = motion::first_non_blank(&self.buffer, Cursor { line, col: 0, want_col: 0 });
        // Treat as linewise so operators like dH delete whole lines.
        MotionResult { target: r.target, kind: MotionKind::Linewise }
    }

    fn mark_motion(&self, name: char, exact: bool) -> MotionResult {
        let Some((mline, mcol)) = self.marks.get(&name).copied() else {
            return MotionResult {
                target: self.window.cursor,
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

    fn apply_op_with_motion(&mut self, op: Operator, m: MotionResult, target: Option<char>) {
        // Indent/outdent operate on whole lines from cursor to motion target,
        // regardless of motion kind. Bypass the byte-range path used by d/c/y.
        if matches!(op, Operator::Indent | Operator::Outdent) {
            let l1 = self.window.cursor.line.min(m.target.line);
            let l2 = self.window.cursor.line.max(m.target.line);
            if matches!(op, Operator::Indent) {
                self.indent_lines(l1, l2);
            } else {
                self.outdent_lines(l1, l2);
            }
            return;
        }
        let (start, end) = self.range_from_motion(m);
        if end <= start {
            return;
        }
        let removed = self.buffer.rope.slice(start..end).to_string();
        let linewise = matches!(m.kind, MotionKind::Linewise);

        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, linewise);
                self.flash_yank(start, end);
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
            Operator::Indent | Operator::Outdent => unreachable!(),
        }
    }

    fn apply_op_linewise(&mut self, op: Operator, count: usize, target: Option<char>) {
        let last_line = self.buffer.line_count().saturating_sub(1);
        let l1 = self.window.cursor.line;
        let l2 = (l1 + count - 1).min(last_line);
        // Indent / outdent (>>, <<, count-prefixed) operate purely on line content.
        if matches!(op, Operator::Indent) {
            self.indent_lines(l1, l2);
            return;
        }
        if matches!(op, Operator::Outdent) {
            self.outdent_lines(l1, l2);
            return;
        }
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
                let n = l2 - l1 + 1;
                self.write_yank_register(target, reg_text, true);
                self.flash_yank(start, end);
                self.status_msg = if n == 1 {
                    "1 line yanked".into()
                } else {
                    format!("{n} lines yanked")
                };
            }
            Operator::Delete => {
                self.write_register(target, reg_text, true);
                self.buffer.delete_range(effective_start, end);
                let new_last = self.buffer.line_count().saturating_sub(1);
                self.window.cursor.line = l1.min(new_last);
                self.window.cursor.col = 0;
                self.window.cursor.want_col = 0;
            }
            Operator::Change => {
                self.write_register(target, reg_text, true);
                self.buffer.delete_range(effective_start, end);
                self.buffer.insert_at_idx(effective_start, "\n");
                self.window.cursor.line = l1;
                self.window.cursor.col = 0;
                self.window.cursor.want_col = 0;
                self.mode = Mode::Insert;
            }
            Operator::Indent | Operator::Outdent => unreachable!(),
        }
    }

    fn range_from_motion(&self, m: MotionResult) -> (usize, usize) {
        let from = self.window.cursor;
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
}
