//! Action dispatch — `apply_action` and the operator/motion/text-object
//! glue. Every keybinding ultimately resolves to an `Action`, which lands
//! here and fans out to the relevant primitive on `App`.

use crate::cursor::Cursor;
use crate::mode::{Mode, Operator};
use crate::motion::{self, MotionKind, MotionResult};
use crate::parser::{Action, MotionVerb};
use crate::text_object::{self, TextObjectVerb, TextRange};

use super::state::{FindRecord, is_jump_motion};

impl super::App {
    pub(super) fn apply_action(&mut self, action: Action) {
        self.maybe_record_edit(&action);
        // Self-heal: if the lens vanished between the motion that set
        // the index and now (server response cleared the cache), drop
        // the index so the next vertical motion behaves as if the
        // cursor was already on the content row.
        if self.phantom_lens_idx.is_some() && !self.line_has_code_lens(self.window.cursor.line) {
            self.phantom_lens_idx = None;
        }
        // Phantom is preserved only across single-step h/j/k/l (and arrow
        // equivalents). Everything else ungrounds the visual cursor back
        // to its content row.
        let preserve_phantom = matches!(
            action,
            Action::Move {
                motion: MotionVerb::Up | MotionVerb::Down | MotionVerb::Left | MotionVerb::Right,
                count: 1,
            }
        );
        if !preserve_phantom {
            self.phantom_lens_idx = None;
        }
        match action {
            Action::Move { motion, count } => {
                if count == 1 {
                    // `k` from content with a lens → hop up to segment 0.
                    if matches!(motion, MotionVerb::Up)
                        && self.phantom_lens_idx.is_none()
                        && self.line_has_code_lens(self.window.cursor.line)
                    {
                        self.phantom_lens_idx = Some(0);
                        return;
                    }
                    // While on the phantom, h/l walk between segments;
                    // j drops back to content; k walks up off the
                    // phantom to the line above.
                    if let Some(idx) = self.phantom_lens_idx {
                        match motion {
                            MotionVerb::Right => {
                                let total = self.lens_count_on_line(self.window.cursor.line);
                                if idx + 1 < total {
                                    self.phantom_lens_idx = Some(idx + 1);
                                }
                                return;
                            }
                            MotionVerb::Left => {
                                if idx > 0 {
                                    self.phantom_lens_idx = Some(idx - 1);
                                }
                                return;
                            }
                            MotionVerb::Down => {
                                self.phantom_lens_idx = None;
                                return;
                            }
                            MotionVerb::Up => {
                                self.phantom_lens_idx = None;
                                // Fall through and let `k` walk up by
                                // one line normally.
                            }
                            _ => {}
                        }
                    }
                }
                self.phantom_lens_idx = None;
                // `'A` from another file goes to that file first.
                if let MotionVerb::Mark { name, .. } = motion {
                    self.enter_file_mark(name);
                }
                // Target first: `''` reads the `'` mark that `push_jump` moves.
                let m = self.run_motion(motion, count);
                if is_jump_motion(motion) {
                    self.push_jump();
                }
                self.window.cursor = m.target;
                self.clamp_cursor_normal();
            }
            Action::Operate {
                op,
                motion,
                count,
                register,
            } => {
                self.record_before_op(op);
                if !self.try_multi_op_motion(op, motion, count, register) {
                    let m = self.run_motion(motion, count);
                    let m = self.paragraph_linewise(motion, m);
                    self.apply_op_with_motion(op, m, register);
                }
            }
            Action::OperateLine {
                op,
                count,
                register,
            } => {
                self.record_before_op(op);
                if !self.try_multi_op_linewise(op, count, register) {
                    self.apply_op_linewise(op, count, register);
                }
            }
            Action::OperateTextObject {
                op,
                obj,
                count,
                register,
            } => {
                self.record_before_op(op);
                if !self.try_multi_op_textobj(op, obj, register) {
                    self.apply_text_object(op, obj, count, register);
                }
            }
            Action::EnterInsert(w) => self.enter_insert(w),
            Action::EnterReplace { count } => self.enter_replace(count),
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
            Action::VisualReplace { ch } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.visual_replace(ch);
            }
            Action::JoinLines { count, spaces } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                // `NJ` joins N lines — N - 1 joins, and never fewer than one.
                self.join_lines(count.saturating_sub(1).max(1), spaces);
            }
            Action::VisualJoin { spaces } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.visual_join(spaces);
            }
            Action::AdjustNumber { delta, count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.adjust_number(delta, count);
            }
            Action::VisualAdjustNumber {
                delta,
                count,
                progressive,
            } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.visual_adjust_number(delta, count, progressive);
            }
            Action::MoveLine { down, count } => self.move_lines(down, count),
            Action::ToggleCase { count } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.toggle_case(count);
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Put {
                before,
                count,
                register,
                style,
            } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.put(before, count, register, style);
            }
            Action::EnterCommand => {
                self.cmdline.clear();
                // From Visual, `:` works on the selection's lines, which
                // `after_key` marks `'<` / `'>` as Visual ends.
                if matches!(self.mode, Mode::Visual(_)) {
                    self.exit_visual();
                    self.cmdline.push_str("'<,'>");
                }
                self.cmdline_cursor = self.cmdline.len();
                self.history_reset();
                self.mode = Mode::Command;
            }
            Action::EnterSearch { backward } => {
                self.cmdline.clear();
                self.history_reset();
                self.incsearch = Some(crate::app::state::IncSearch {
                    origin: self.window.cursor,
                    view_top: self.window.view_top,
                    view_left: self.window.view_left,
                    pattern: None,
                    current: None,
                });
                self.mode = Mode::Search { backward };
            }
            Action::Repeat => self.repeat_last_edit(),
            Action::PageScroll(kind) => self.page_scroll(kind),
            Action::AdjustViewport(kind) => self.adjust_viewport_to(kind),
            Action::SetMark { name } => {
                let Cursor { line, col, .. } = self.window.cursor;
                self.buffer.set_mark(name, line, col);
                if name.is_ascii_uppercase() {
                    self.set_file_mark(name, line, col);
                }
            }
            Action::SearchWord {
                backward,
                whole_word,
            } => self.search_word_under_cursor(backward, whole_word),
            Action::StartMacro { name } => self.start_macro_recording(name),
            Action::ReplayMacro { name, count } => self.replay_macro(name, count),
            Action::ExpressionPrompt => self.open_expression_prompt(false),
            Action::HistoryWindow { search, backward } => {
                self.open_history_window(search, backward)
            }
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
            Action::DiagnosticJump { forward, count } => self.goto_diagnostic(forward, count),
            Action::QuickfixNext => self.qf_next(),
            Action::QuickfixPrev => self.qf_prev(),
            Action::HunkNext => self.hunk_jump(true),
            Action::HunkPrev => self.hunk_jump(false),
            Action::SpellNext => self.cmd_spell_next(),
            Action::SpellPrev => self.cmd_spell_prev(),
            Action::SpellSuggest => self.cmd_spell_suggest(),
            Action::HunkPreview => self.hunk_preview(),
            Action::HunkStage => self.hunk_stage(),
            Action::HunkUnstage => self.hunk_unstage(),
            Action::HunkReset => self.hunk_reset(),
            Action::TerminalOpen => self.cmd_open_terminal(None),
            Action::TerminalClose => {
                if !self.terminals.is_empty() {
                    self.close_terminal();
                } else {
                    self.status_msg = "terminal: no pane to close".into();
                }
            }
            Action::TerminalFocus => {
                if !self.terminals.is_empty() {
                    self.terminal_pane_open = true;
                    self.mode = crate::mode::Mode::Terminal;
                    self.terminal_focus = crate::app::TerminalFocus::Bottom;
                    self.adjust_viewport();
                } else {
                    self.status_msg = "terminal: no pane (open with `<leader>tt`)".into();
                }
            }
            Action::TerminalToggle => self.toggle_terminal_pane(),
            Action::AiClaude => {
                use crate::command::AiTool;
                let t = AiTool::Claude;
                self.open_side_terminal(t.label(), t.command(), false);
            }
            Action::AiClaudeHandoff => {
                use crate::command::AiTool;
                let t = AiTool::Claude;
                self.open_side_terminal(t.label(), t.command(), true);
            }
            Action::AiCodex => {
                use crate::command::AiTool;
                let t = AiTool::Codex;
                self.open_side_terminal(t.label(), t.command(), false);
            }
            Action::AiCodexHandoff => {
                use crate::command::AiTool;
                let t = AiTool::Codex;
                self.open_side_terminal(t.label(), t.command(), true);
            }
            Action::AiOpencode => {
                use crate::command::AiTool;
                let t = AiTool::Opencode;
                self.open_side_terminal(t.label(), t.command(), false);
            }
            Action::AiOpencodeHandoff => {
                use crate::command::AiTool;
                let t = AiTool::Opencode;
                self.open_side_terminal(t.label(), t.command(), true);
            }
            Action::AiOpenClaw => {
                use crate::command::AiTool;
                let t = AiTool::OpenClaw;
                self.open_side_terminal(t.label(), t.command(), false);
            }
            Action::AiOpenClawHandoff => {
                use crate::command::AiTool;
                let t = AiTool::OpenClaw;
                self.open_side_terminal(t.label(), t.command(), true);
            }
            Action::AiHermes => {
                use crate::command::AiTool;
                let t = AiTool::Hermes;
                self.open_side_terminal(t.label(), t.command(), false);
            }
            Action::AiHermesHandoff => {
                use crate::command::AiTool;
                let t = AiTool::Hermes;
                self.open_side_terminal(t.label(), t.command(), true);
            }
            Action::AiClose => {
                if self.side_terminals.is_empty() {
                    self.status_msg = "ai: no side pane to close".into();
                } else {
                    self.close_side_terminal();
                }
            }
            Action::AiFocus => self.focus_side_terminal(),
            Action::AiToggle => self.toggle_side_terminal_pane(),
            Action::TestPicker => self.cmd_test_picker(),
            Action::TestNearest => self.cmd_test_nearest(),
            Action::TestFile => self.cmd_test_file(),
            Action::TestLast => self.cmd_test_last(),
            Action::TestCancel => self.cmd_test_cancel(),
            Action::TestResults => self.cmd_test_results(),
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
            Action::WindowResize { axis, delta } => {
                let area = self.editor_rect();
                self.layout.resize(self.active_window, axis, delta, area);
            }
            Action::WindowPromoteToTab => self.window_promote_to_tab(),
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
            Action::OpenPicker { kind } => self.open_picker(kind),
            Action::OpenFileExplorer => {
                if self.config.file_explorer.yazi {
                    self.open_yazi();
                } else {
                    self.toggle_file_tree();
                }
            }
            Action::LspGotoDefinition => self.lsp_request_goto(),
            Action::LspFindReferences => self.lsp_request_references(),
            Action::LspRename => self.start_rename_prompt(),
            Action::LspExecuteCodeLens => self.execute_code_lens_under_cursor(),
            Action::ReplaceAllInBuffer => self.start_replace_all_prompt(),
            Action::Format => self.format_active(),
            Action::ToggleComment => self.toggle_comment_range(),
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
            Action::SurroundAdd {
                target,
                ch,
                own_lines,
            } => {
                if let Some((start, end)) = self.surround_target_range(target) {
                    self.history.record(&self.buffer.rope, self.window.cursor);
                    self.surround_wrap(start, end, ch, own_lines);
                }
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
            Action::ReselectVisual => self.reselect_visual(),
            Action::ChangeListJump { older, count } => self.goto_change(older, count),
            Action::RepeatSubstitute { whole, count } => {
                let line = self.window.cursor.line + 1;
                let range = if whole {
                    crate::command::ExRange::Whole
                } else {
                    crate::command::ExRange::Lines(line, line.saturating_add(count.max(1) - 1))
                };
                self.repeat_substitute(range, whole, whole, crate::command::SubFlags::default());
            }
            Action::VisualOperate { op, register } => {
                self.record_before_op(op);
                self.apply_visual_operate(op, register);
            }
            Action::VisualPut { register } => {
                self.history.record(&self.buffer.rope, self.window.cursor);
                self.apply_visual_put(register);
            }
            Action::VisualSelectTextObject { obj } => {
                self.apply_visual_select_textobj(obj);
            }
            Action::WriteQuitIfModified => self.exec_command("x"),
            Action::QuitDiscard => self.exec_command("q!"),
            Action::AlternateBuffer { count } => {
                if let Err(e) = self.switch_alternate(count) {
                    self.status_msg = format!("error: {e}");
                }
            }
            Action::VisualSwap => {
                if let Some(anchor) = self.window.visual_anchor {
                    self.window.visual_anchor = Some(self.window.cursor);
                    self.window.cursor = anchor;
                }
            }
            Action::Lazygit => self.cmd_lazygit(),
            Action::TaskPicker => self.cmd_task_picker(),
            Action::TaskLast => self.cmd_task_last(),
            Action::PackageInstall => {
                self.package_begin(crate::app::state::PackageFlowKind::Install)
            }
            Action::PackageSearch => self.package_begin(crate::app::state::PackageFlowKind::Search),
            Action::InstallToolchain => self.install_toolchain_for_current(),
            Action::AndroidLaunchAvd => self.android_list_avds(),
            Action::AndroidCreateAvd => self.android_create_avd(),
            Action::AndroidDevices => self.android_list_devices(),
            Action::AndroidDebug => self.android_debug_session(),
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
        count: usize,
        target: Option<char>,
    ) {
        let Some(range) = self.text_object_range(obj, count) else {
            if let Some(hint) = text_object::syntax_object_hint(&self.buffer, obj) {
                self.status_msg = hint;
            } else if matches!(obj, TextObjectVerb::SearchMatch { .. }) {
                self.status_msg = self.search_missing();
            }
            return;
        };
        self.apply_op_to_range(op, range, target);
    }

    /// A text object's range. `gn` / `gN` go by the search, which the
    /// `text_object` module has no access to.
    fn text_object_range(
        &self,
        obj: TextObjectVerb,
        count: usize,
    ) -> Option<text_object::TextRange> {
        if let TextObjectVerb::SearchMatch { forward } = obj {
            let at = self
                .buffer
                .pos_to_char(self.window.cursor.line, self.window.cursor.col);
            let (start, end) = self.search_match_at(at, forward)?;
            return Some(text_object::TextRange {
                start,
                end,
                linewise: false,
            });
        }
        text_object::compute_counted(&self.buffer, self.window.cursor, obj, count)
    }

    /// The `[start, end)` a `ys` target covers. `yss` starts at the first
    /// non-blank, and trailing whitespace is left out so the pair closes
    /// against the text, as vim-surround does.
    fn surround_target_range(
        &mut self,
        target: crate::parser::SurroundTarget,
    ) -> Option<(usize, usize)> {
        use crate::parser::SurroundTarget;
        let (start, mut end) = match target {
            SurroundTarget::Motion { motion, count } => {
                let m = self.run_motion(motion, count);
                let m = self.paragraph_linewise(motion, m);
                self.range_from_motion(m)
            }
            SurroundTarget::TextObject { obj, count } => {
                let r = self.text_object_range(obj, count)?;
                (r.start, r.end)
            }
            SurroundTarget::Lines { count } => {
                let first = self.window.cursor.line;
                let last = (first + count.saturating_sub(1))
                    .min(self.buffer.line_count().saturating_sub(1));
                let line_start = Cursor {
                    line: first,
                    col: 0,
                    want_col: 0,
                };
                let from = motion::first_non_blank(&self.buffer, line_start).target;
                let start = self.buffer.pos_to_char(from.line, from.col);
                (
                    start,
                    self.buffer.line_start_idx(last) + self.buffer.line_len(last),
                )
            }
        };
        while end > start && self.buffer.rope.char(end - 1).is_whitespace() {
            end -= 1;
        }
        (end > start).then_some((start, end))
    }

    fn apply_op_to_range(&mut self, op: Operator, range: TextRange, target: Option<char>) {
        if range.end <= range.start {
            return;
        }
        if let Operator::Case(how) = op {
            self.recase_range(range.start, range.end, how);
            self.cursor_to_idx(range.start);
            self.clamp_cursor_normal();
            return;
        }
        // Indent / outdent on a text-object range: derive line span and shift them.
        if matches!(
            op,
            Operator::Indent
                | Operator::Outdent
                | Operator::Reindent
                | Operator::Format { .. }
                | Operator::Filter
        ) {
            let l1 = self.buffer.rope.char_to_line(range.start);
            let l2_idx = range.end.saturating_sub(1);
            let l2 = self
                .buffer
                .rope
                .char_to_line(l2_idx.min(self.buffer.total_chars()));
            self.shift_lines(op, l1, l2);
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
                // Changing whole lines leaves one empty line to type on, as
                // `cc` does; deleting the last newline too would pull the
                // next line up under the cursor.
                let keeps_newline = range.linewise
                    && range.end > range.start
                    && self.buffer.rope.char(range.end - 1) == '\n';
                let end = if keeps_newline {
                    range.end - 1
                } else {
                    range.end
                };
                self.buffer.delete_range(range.start, end);
                self.cursor_to_idx(range.start);
                self.mode = Mode::Insert;
            }
            Operator::Indent
            | Operator::Outdent
            | Operator::Reindent
            | Operator::Format { .. }
            | Operator::Filter
            | Operator::Case(_) => {
                unreachable!()
            }
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
            MotionVerb::WordForward => {
                motion::word_forward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::WordBackward => {
                motion::word_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::BigWordForward => {
                motion::big_word_forward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::BigWordBackward => {
                motion::big_word_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::EndWord => motion::end_word(&self.buffer, self.window.cursor, count),
            MotionVerb::BigEndWord => motion::big_end_word(&self.buffer, self.window.cursor, count),
            MotionVerb::EndWordBackward => {
                motion::end_word_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::BigEndWordBackward => {
                motion::big_end_word_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::FirstLine => motion::first_line(&self.buffer, self.window.cursor),
            MotionVerb::LastLine => motion::last_line(&self.buffer, self.window.cursor),
            MotionVerb::GotoLine(n) => motion::goto_line(&self.buffer, n),
            MotionVerb::PercentLine(n) => motion::percent_line(&self.buffer, n),
            MotionVerb::ParagraphForward => {
                motion::paragraph_forward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::ParagraphBackward => {
                motion::paragraph_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::SentenceForward => {
                motion::sentence_forward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::SentenceBackward => {
                motion::sentence_backward(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::UnmatchedBracket {
                open,
                close,
                forward,
            } => motion::unmatched_bracket(
                &self.buffer,
                self.window.cursor,
                open,
                close,
                forward,
                count,
            ),
            MotionVerb::NextLineStart => {
                motion::next_line_start(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::PrevLineStart => {
                motion::prev_line_start(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::LineStartDown => {
                motion::line_start_down(&self.buffer, self.window.cursor, count)
            }
            MotionVerb::ToColumn => motion::to_column(&self.buffer, self.window.cursor, count),
            MotionVerb::ScreenLineStart => motion::screen_col(
                &self.buffer,
                self.window.cursor,
                self.window.view_left,
                MotionKind::CharExclusive,
            ),
            MotionVerb::ScreenFirstNonBlank => motion::screen_first_non_blank(
                &self.buffer,
                self.window.cursor,
                self.window.view_left,
            ),
            MotionVerb::ScreenMiddle => {
                let middle = self.window.view_left + self.text_area_cols() / 2;
                motion::screen_col(
                    &self.buffer,
                    self.window.cursor,
                    middle,
                    MotionKind::CharExclusive,
                )
            }
            MotionVerb::ScreenLineEnd => {
                let right = self.window.view_left + self.text_area_cols().saturating_sub(1);
                motion::screen_col(
                    &self.buffer,
                    self.window.cursor,
                    right,
                    MotionKind::CharInclusive,
                )
            }
            MotionVerb::LineMiddle => motion::line_middle(&self.buffer, self.window.cursor),
            MotionVerb::MatchPair => {
                super::pair::match_pair_motion(&self.buffer, self.window.cursor).unwrap_or(
                    MotionResult {
                        target: self.window.cursor,
                        kind: MotionKind::CharExclusive,
                    },
                )
            }
            MotionVerb::FirstNonBlank => motion::first_non_blank(&self.buffer, self.window.cursor),
            MotionVerb::LastNonBlank => motion::last_non_blank(&self.buffer, self.window.cursor),
            MotionVerb::ViewportTop => self.viewport_motion(0),
            MotionVerb::ViewportMiddle => self.viewport_motion(self.buffer_rows() / 2),
            MotionVerb::ViewportBottom => {
                self.viewport_motion(self.buffer_rows().saturating_sub(1))
            }
            MotionVerb::Mark { name, exact } => self.mark_motion(name, exact),
            MotionVerb::FindChar {
                ch,
                forward,
                before,
            } => {
                self.last_find = Some(FindRecord {
                    ch,
                    forward,
                    before,
                });
                motion::find_char(&self.buffer, self.window.cursor, ch, forward, before, count)
                    .unwrap_or(MotionResult {
                        target: self.window.cursor,
                        kind: MotionKind::CharExclusive,
                    })
            }
            MotionVerb::RepeatFind { reverse } => match self.last_find {
                Some(rec) => {
                    let forward = if reverse { !rec.forward } else { rec.forward };
                    motion::find_char(
                        &self.buffer,
                        self.window.cursor,
                        rec.ch,
                        forward,
                        rec.before,
                        count,
                    )
                    .unwrap_or(MotionResult {
                        target: self.window.cursor,
                        kind: MotionKind::CharExclusive,
                    })
                }
                None => MotionResult {
                    target: self.window.cursor,
                    kind: MotionKind::CharExclusive,
                },
            },
            MotionVerb::SearchNext { reverse } => self.run_search_next(reverse, count),
        }
    }

    fn viewport_motion(&self, offset: usize) -> MotionResult {
        let line = (self.window.view_top + offset).min(self.buffer.line_count().saturating_sub(1));
        let r = motion::first_non_blank(
            &self.buffer,
            Cursor {
                line,
                col: 0,
                want_col: 0,
            },
        );
        // Treat as linewise so operators like dH delete whole lines.
        MotionResult {
            target: r.target,
            kind: MotionKind::Linewise,
        }
    }

    fn mark_motion(&self, name: char, exact: bool) -> MotionResult {
        let Some((mline, mcol)) = self.buffer.mark(name) else {
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
                target: Cursor {
                    line,
                    col,
                    want_col: col,
                },
                kind: MotionKind::CharExclusive,
            }
        } else {
            // ' jumps to first non-blank, linewise.
            let r = motion::first_non_blank(
                &self.buffer,
                Cursor {
                    line,
                    col: 0,
                    want_col: 0,
                },
            );
            MotionResult {
                target: r.target,
                kind: MotionKind::Linewise,
            }
        }
    }

    /// Vim's exclusive-linewise rule (`:h exclusive-linewise`): an exclusive
    /// motion that ends in column 0 of a later line, from a start at or
    /// before its line's first non-blank, covers whole lines — so `d}` from
    /// the top of a paragraph takes its lines and leaves the blank one. Only
    /// the paragraph motions opt in: `dw` leans on the plain
    /// exclusive-to-inclusive rule in `range_from_motion`. A backward motion
    /// moves the cursor to its first line, since the linewise span runs from
    /// the cursor.
    fn paragraph_linewise(&mut self, motion: MotionVerb, m: MotionResult) -> MotionResult {
        if !matches!(
            motion,
            MotionVerb::ParagraphForward
                | MotionVerb::ParagraphBackward
                | MotionVerb::SentenceForward
                | MotionVerb::SentenceBackward
        ) || !matches!(m.kind, MotionKind::CharExclusive)
        {
            return m;
        }
        let cur = self.window.cursor;
        let (start, end) = if (m.target.line, m.target.col) < (cur.line, cur.col) {
            (m.target, cur)
        } else {
            (cur, m.target)
        };
        let first_non_blank = motion::first_non_blank(&self.buffer, start).target.col;
        if end.col != 0 || end.line <= start.line || start.col > first_non_blank {
            return m;
        }
        self.window.cursor = start;
        MotionResult {
            target: Cursor {
                line: end.line - 1,
                col: 0,
                want_col: 0,
            },
            kind: MotionKind::Linewise,
        }
    }

    /// The undo step an operator takes before it runs — but not `!`, which
    /// only opens the `:` line. The filter takes its own step when it runs.
    fn record_before_op(&mut self, op: Operator) {
        if op != Operator::Filter {
            self.history.record(&self.buffer.rope, self.window.cursor);
        }
    }

    fn apply_op_with_motion(&mut self, op: Operator, m: MotionResult, target: Option<char>) {
        // Indent/outdent operate on whole lines from cursor to motion target,
        // regardless of motion kind. Bypass the byte-range path used by d/c/y.
        if matches!(
            op,
            Operator::Indent
                | Operator::Outdent
                | Operator::Reindent
                | Operator::Format { .. }
                | Operator::Filter
        ) {
            let l1 = self.window.cursor.line.min(m.target.line);
            let l2 = self.window.cursor.line.max(m.target.line);
            self.shift_lines(op, l1, l2);
            return;
        }
        let (start, end) = self.range_from_motion(m);
        if end <= start {
            return;
        }
        if let Operator::Case(how) = op {
            self.recase_range(start, end, how);
            self.cursor_to_idx(start);
            self.clamp_cursor_normal();
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
            Operator::Indent
            | Operator::Outdent
            | Operator::Reindent
            | Operator::Format { .. }
            | Operator::Filter
            | Operator::Case(_) => {
                unreachable!()
            }
        }
    }

    fn apply_op_linewise(&mut self, op: Operator, count: usize, target: Option<char>) {
        let last_line = self.buffer.line_count().saturating_sub(1);
        let l1 = self.window.cursor.line;
        let l2 = (l1 + count - 1).min(last_line);
        // Indent / outdent (>>, <<, count-prefixed) operate purely on line content.
        if matches!(
            op,
            Operator::Indent
                | Operator::Outdent
                | Operator::Reindent
                | Operator::Format { .. }
                | Operator::Filter
        ) {
            self.shift_lines(op, l1, l2);
            return;
        }
        if let Operator::Case(how) = op {
            let start = self.buffer.line_start_idx(l1);
            let end = self.buffer.line_start_idx(l2 + 1);
            self.recase_range(start, end, how);
            self.clamp_cursor_normal();
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
            Operator::Indent
            | Operator::Outdent
            | Operator::Reindent
            | Operator::Format { .. }
            | Operator::Filter
            | Operator::Case(_) => {
                unreachable!()
            }
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
            to = Cursor {
                line: prev,
                col,
                want_col: col,
            };
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
