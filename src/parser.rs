use crate::mode::{Operator, VisualKind};
use crate::text_object::TextObjectVerb;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy)]
pub enum MotionVerb {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    FirstNonBlank,
    LastNonBlank,
    WordForward,
    WordBackward,
    BigWordForward,
    BigWordBackward,
    EndWord,
    BigEndWord,
    EndWordBackward,
    BigEndWordBackward,
    FirstLine,
    LastLine,
    GotoLine(usize),
    FindChar { ch: char, forward: bool, before: bool },
    RepeatFind { reverse: bool },
    SearchNext { reverse: bool },
    #[allow(dead_code)]
    ViewportTop,
    ViewportMiddle,
    #[allow(dead_code)]
    ViewportBottom,
    Mark { name: char, exact: bool },
}

#[derive(Debug, Clone, Copy)]
pub enum PageScrollKind {
    HalfDown,
    HalfUp,
    FullDown,
    FullUp,
    LineDown,
    LineUp,
}

#[derive(Debug, Clone, Copy)]
pub enum FoldOp {
    Toggle,
    Open,
    Close,
    OpenAll,
    CloseAll,
}

#[derive(Debug, Clone, Copy)]
pub enum ViewportAdjust {
    Center,
    Top,
    Bottom,
    Left,
    Right,
    HalfLeft,
    HalfRight,
}

#[derive(Debug, Clone, Copy)]
pub enum MarkAction {
    Set,
    JumpLine,
    JumpExact,
}

/// Debugger sub-menu actions reachable via `<leader>d{key}`. Each variant
/// maps 1:1 onto a `:dap*` ex-command — the leader keys are a convenience
/// layer on top of the same dispatch.
#[derive(Debug, Clone, Copy)]
pub enum DebugAction {
    /// `<leader>ds` — start a debug session.
    Start,
    /// `<leader>dq` — stop the active debug session.
    Stop,
    /// `<leader>db` — toggle a breakpoint at the cursor line.
    ToggleBreakpoint,
    /// `<leader>dB` — clear every breakpoint set in the active buffer.
    ClearBreakpointsInFile,
    /// `<leader>dc` — continue execution.
    Continue,
    /// `<leader>dn` — step over (DAP `next`).
    Next,
    /// `<leader>di` — step into.
    StepIn,
    /// `<leader>dO` — step out.
    StepOut,
    /// `<leader>dp` — toggle the bottom debug pane.
    PaneToggle,
    /// `<leader>df` — focus the bottom debug pane for tree navigation.
    FocusPane,
}

#[derive(Debug, Clone, Copy)]
pub enum PickerLeader {
    Files,
    Recents,
    #[allow(dead_code)]
    Buffers,
    Grep,
    DocumentSymbols,
    WorkspaceSymbols,
    CodeActions,
}

#[derive(Debug, Clone, Copy)]
pub struct FindSpec {
    pub forward: bool,
    pub before: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum InsertWhere {
    Cursor,
    AfterCursor,
    LineBelow,
    LineAbove,
    LineFirstNonBlank,
    LineEnd,
}

#[derive(Debug, Clone)]
pub enum Action {
    Move { motion: MotionVerb, count: usize },
    Operate { op: Operator, motion: MotionVerb, count: usize, register: Option<char> },
    OperateLine { op: Operator, count: usize, register: Option<char> },
    OperateTextObject { op: Operator, obj: TextObjectVerb, count: usize, register: Option<char> },
    EnterInsert(InsertWhere),
    DeleteCharForward { count: usize, register: Option<char> },
    ReplaceChar { ch: char, count: usize },
    JoinLines { count: usize },
    ToggleCase { count: usize },
    Undo,
    Redo,
    Put { before: bool, count: usize, register: Option<char> },
    EnterCommand,
    EnterSearch { backward: bool },
    EnterVisual(VisualKind),
    Repeat,
    PageScroll(PageScrollKind),
    AdjustViewport(ViewportAdjust),
    SetMark { name: char },
    SearchWord { backward: bool },
    JumpBack,
    JumpForward,
    /// `Ctrl-A` (delta = +1) / `Ctrl-X` (delta = -1) — adjust the next number
    /// at or after the cursor on the current line. Multiplied by `count`.
    AdjustNumber { delta: i64, count: usize },
    /// `Ctrl-J` (down = true) / `Ctrl-K` (down = false) — move the current
    /// line (Normal mode) or the selected line range (Visual mode) up or
    /// down by `count` positions. Cursor and visual anchor follow the
    /// block; the selection stays attached to the moving lines.
    MoveLine { down: bool, count: usize },
    OpenPicker { kind: PickerLeader },
    OpenYazi,
    LspGotoDefinition,
    LspFindReferences,
    LspRename,
    /// `<leader>f` — run the buffer's formatter and replace its contents
    /// with the result. Same code path as `:fmt` / `:format`.
    Format,
    /// `<leader>d…` — debugger sub-menu. The associated `DebugAction`
    /// picks the specific command; dispatch is in `app/dap_glue.rs`.
    Debug(DebugAction),
    /// `<leader>R` — literal-string replace-all of the word under the
    /// cursor in the current buffer. Opens a prompt; on Enter applies the
    /// replacement to every occurrence via the same machinery as `:%s`.
    ReplaceAllInBuffer,
    /// `Ctrl-N` in Visual-char mode — find the next occurrence of the
    /// current selection's text and add it as an additional selection.
    /// Subsequent `d`/`c`/`y` apply to every selection; `c` lands in
    /// Insert mode with mirrored cursors at each former selection start.
    AddNextOccurrenceSelection,
    /// `ds{char}` — strip the surrounding pair around the cursor.
    SurroundDelete { ch: char },
    /// `cs{old}{new}` — swap the surrounding pair from `old` to `new`.
    SurroundChange { from: char, to: char },
    /// Visual `S{char}` — wrap the visual selection in the pair for `char`.
    SurroundVisual { ch: char },
    Fold(FoldOp),
    LspHover,
    VisualOperate { op: Operator, register: Option<char> },
    /// Visual `p` / `P` — replace the selection with the register's contents.
    /// `before` is unused (both keys behave the same in visual mode) but kept
    /// for symmetry with `Action::Put`.
    VisualPut { register: Option<char> },
    VisualSelectTextObject { obj: TextObjectVerb },
    VisualSwap,
    VisualSwitch(VisualKind),
    StartMacro { name: char },
    ReplayMacro { name: char },
    BufferDelete { force: bool },
    BufferOnly,
    BufferNext,
    BufferPrev,
    /// `]q` — jump to the next entry in the quickfix list.
    QuickfixNext,
    /// `[q` — jump to the previous entry in the quickfix list.
    QuickfixPrev,
    /// `]h` — jump to the next git hunk in the active buffer.
    HunkNext,
    /// `[h` — jump to the previous git hunk in the active buffer.
    HunkPrev,
    /// `<leader>hp` — preview the hunk under the cursor in a hover popup.
    HunkPreview,
    /// `<leader>hs` — stage the hunk under the cursor (`git apply --cached`).
    HunkStage,
    /// `<leader>hu` — unstage the hunk under the cursor.
    HunkUnstage,
    /// `<leader>hr` — discard the working-tree change for the hunk under
    /// the cursor (reset to the staged version).
    HunkReset,
}

#[derive(Debug, Clone, Default)]
pub struct PendingCmd {
    pub count1: Option<usize>,
    pub operator: Option<Operator>,
    pub count2: Option<usize>,
    pub awaiting_g: bool,
    pub awaiting_z: bool,
    pub awaiting_leader: bool,
    /// `Some(true)` for inner (`i`), `Some(false)` for around (`a`).
    pub awaiting_textobj: Option<bool>,
    /// Set after f/F/t/T — next char is the literal find target.
    pub awaiting_find: Option<FindSpec>,
    /// Set after `r` — next char is the replacement.
    pub awaiting_replace: bool,
    pub awaiting_mark: Option<MarkAction>,
    /// Set after `"` — next char is the register name.
    pub awaiting_register: bool,
    /// Selected register, applied to the next register-using action.
    pub register: Option<char>,
    /// Set after `q` (start macro) — next char is the register name.
    pub awaiting_macro_record: bool,
    /// Set after `@` — next char is the macro register to replay.
    pub awaiting_macro_play: bool,
    /// Set after `<leader>b` — next char picks a buffer-related action.
    pub awaiting_buffer_leader: bool,
    /// Set after `<leader>d` — next char picks a debug-related action.
    pub awaiting_debug_leader: bool,
    /// `ds{char}` — next char names the surround pair to delete.
    pub awaiting_ds: bool,
    /// `cs{old}{new}` — first the old char, then the new char.
    pub awaiting_cs_old: bool,
    /// Captured `old` from `cs` while waiting for the replacement char.
    pub cs_old: Option<char>,
    /// Visual `S{char}` — next char names the surround pair to wrap with.
    pub awaiting_visual_surround: bool,
    /// Set after `]` in Normal mode — next char (e.g. `q`) selects a
    /// "jump forward" target. Today consumers are the quickfix list
    /// (`]q`) and git hunks (`]h`). Cancels on any unrecognised follow-up.
    pub awaiting_bracket_close: bool,
    /// Set after `[` — mirror of `awaiting_bracket_close` for backward
    /// navigation (`[q` → previous quickfix entry, `[h` → previous hunk).
    pub awaiting_bracket_open: bool,
    /// Set after `<leader>h` — next char picks a git-hunk action
    /// (`p` preview, `s` stage, `u` unstage, `r` reset).
    pub awaiting_hunk_leader: bool,
}

impl PendingCmd {
    fn reset(&mut self) {
        *self = PendingCmd::default();
    }

    fn take_register(&mut self) -> Option<char> {
        self.register.take()
    }

    fn total_count(&self) -> usize {
        self.count1.unwrap_or(1).saturating_mul(self.count2.unwrap_or(1))
    }

    fn slot_in_progress(&self) -> bool {
        if self.operator.is_some() {
            self.count2.is_some()
        } else {
            self.count1.is_some()
        }
    }

    fn push_digit(&mut self, d: usize) {
        let target = if self.operator.is_some() {
            &mut self.count2
        } else {
            &mut self.count1
        };
        let cur = target.unwrap_or(0);
        *target = Some(cur.saturating_mul(10).saturating_add(d));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseCtx {
    Normal,
    Visual,
}

pub enum ParseResult {
    Pending,
    Action(Action),
    Cancelled,
}

pub fn parse(state: &mut PendingCmd, key: KeyEvent, ctx: ParseCtx) -> ParseResult {
    if matches!(key.code, KeyCode::Esc) {
        state.reset();
        return ParseResult::Cancelled;
    }

    // Arrow keys + Home/End mirror the hjkl/0/$ motions in any non-Insert context.
    let arrow_motion = match key.code {
        KeyCode::Left => Some(MotionVerb::Left),
        KeyCode::Right => Some(MotionVerb::Right),
        KeyCode::Up => Some(MotionVerb::Up),
        KeyCode::Down => Some(MotionVerb::Down),
        KeyCode::Home => Some(MotionVerb::LineStart),
        KeyCode::End => Some(MotionVerb::LineEnd),
        _ => None,
    };
    if let Some(motion) = arrow_motion {
        let count = state.total_count();
        if let Some(op) = state.operator.take() {
            let register = state.take_register();
            state.reset();
            return ParseResult::Action(Action::Operate { op, motion, count, register });
        }
        state.reset();
        return ParseResult::Action(Action::Move { motion, count });
    }
    let page = match key.code {
        KeyCode::PageUp => Some(PageScrollKind::FullUp),
        KeyCode::PageDown => Some(PageScrollKind::FullDown),
        _ => None,
    };
    if let Some(p) = page {
        state.reset();
        return ParseResult::Action(Action::PageScroll(p));
    }

    let ch = match key.code {
        KeyCode::Char(c) => c,
        _ => return ParseResult::Pending,
    };

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match ch {
            'r' | 'R' => {
                state.reset();
                ParseResult::Action(Action::Redo)
            }
            'd' | 'D' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::HalfDown))
            }
            'u' | 'U' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::HalfUp))
            }
            'f' | 'F' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::FullDown))
            }
            'b' | 'B' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::FullUp))
            }
            'e' | 'E' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::LineDown))
            }
            'y' | 'Y' => {
                state.reset();
                ParseResult::Action(Action::PageScroll(PageScrollKind::LineUp))
            }
            'o' | 'O' => {
                state.reset();
                ParseResult::Action(Action::JumpBack)
            }
            'i' | 'I' => {
                state.reset();
                ParseResult::Action(Action::JumpForward)
            }
            'a' | 'A' => {
                let count = state.total_count();
                state.reset();
                ParseResult::Action(Action::AdjustNumber { delta: 1, count })
            }
            'x' | 'X' => {
                let count = state.total_count();
                state.reset();
                ParseResult::Action(Action::AdjustNumber { delta: -1, count })
            }
            'v' | 'V' => {
                state.reset();
                match ctx {
                    ParseCtx::Visual => {
                        ParseResult::Action(Action::VisualSwitch(VisualKind::Block))
                    }
                    ParseCtx::Normal => {
                        ParseResult::Action(Action::EnterVisual(VisualKind::Block))
                    }
                }
            }
            'n' | 'N' if matches!(ctx, ParseCtx::Visual) => {
                state.reset();
                ParseResult::Action(Action::AddNextOccurrenceSelection)
            }
            'j' | 'J' => {
                let count = state.total_count();
                state.reset();
                ParseResult::Action(Action::MoveLine { down: true, count })
            }
            'k' | 'K' => {
                let count = state.total_count();
                state.reset();
                ParseResult::Action(Action::MoveLine { down: false, count })
            }
            _ => ParseResult::Pending,
        };
    }

    // Resolve register selection — `"x` selects register x for the next op.
    if state.awaiting_register {
        state.awaiting_register = false;
        if !is_valid_register(ch) {
            state.reset();
            return ParseResult::Cancelled;
        }
        state.register = Some(ch);
        return ParseResult::Pending;
    }

    // Resolve macro-record register: after `q`, next char is the macro name.
    if state.awaiting_macro_record {
        state.awaiting_macro_record = false;
        if !ch.is_ascii_alphabetic() && !ch.is_ascii_digit() {
            state.reset();
            return ParseResult::Cancelled;
        }
        let name = ch;
        state.reset();
        return ParseResult::Action(Action::StartMacro { name });
    }

    // Resolve macro-play register: after `@`, next char is the macro to replay.
    if state.awaiting_macro_play {
        state.awaiting_macro_play = false;
        if !ch.is_ascii_alphabetic() && !ch.is_ascii_digit() && ch != '@' {
            state.reset();
            return ParseResult::Cancelled;
        }
        let name = ch;
        state.reset();
        return ParseResult::Action(Action::ReplayMacro { name });
    }

    // Resolve a pending `r` — the next key is the replacement.
    if state.awaiting_replace {
        state.awaiting_replace = false;
        let count = state.total_count();
        state.reset();
        return ParseResult::Action(Action::ReplaceChar { ch, count });
    }

    // Resolve pending mark register.
    if let Some(act) = state.awaiting_mark.take() {
        if !ch.is_ascii_alphabetic() {
            state.reset();
            return ParseResult::Cancelled;
        }
        state.reset();
        return ParseResult::Action(match act {
            MarkAction::Set => Action::SetMark { name: ch },
            MarkAction::JumpLine => Action::Move {
                motion: MotionVerb::Mark { name: ch, exact: false },
                count: 1,
            },
            MarkAction::JumpExact => Action::Move {
                motion: MotionVerb::Mark { name: ch, exact: true },
                count: 1,
            },
        });
    }

    // Resolve pending `]` / `[` — Vim's "next" / "prev" prefixes. Today
    // only `q` is wired (quickfix list); anything else cancels so future
    // additions (`]d` for diagnostics, etc.) don't accidentally fire.
    if state.awaiting_bracket_close {
        state.awaiting_bracket_close = false;
        state.reset();
        return match ch {
            'q' => ParseResult::Action(Action::QuickfixNext),
            'h' => ParseResult::Action(Action::HunkNext),
            _ => ParseResult::Cancelled,
        };
    }
    if state.awaiting_bracket_open {
        state.awaiting_bracket_open = false;
        state.reset();
        return match ch {
            'q' => ParseResult::Action(Action::QuickfixPrev),
            'h' => ParseResult::Action(Action::HunkPrev),
            _ => ParseResult::Cancelled,
        };
    }

    // Resolve pending `z` — viewport adjust.
    if state.awaiting_z {
        state.awaiting_z = false;
        let kind = match ch {
            'z' | '.' => Some(ViewportAdjust::Center),
            't' => Some(ViewportAdjust::Top),
            'b' | '-' => Some(ViewportAdjust::Bottom),
            'h' => Some(ViewportAdjust::Left),
            'l' => Some(ViewportAdjust::Right),
            'H' => Some(ViewportAdjust::HalfLeft),
            'L' => Some(ViewportAdjust::HalfRight),
            _ => None,
        };
        if let Some(k) = kind {
            state.reset();
            return ParseResult::Action(Action::AdjustViewport(k));
        }
        // Fold commands — z + a/o/c/M/R.
        let fold = match ch {
            'a' => Some(FoldOp::Toggle),
            'o' => Some(FoldOp::Open),
            'c' => Some(FoldOp::Close),
            'M' => Some(FoldOp::CloseAll),
            'R' => Some(FoldOp::OpenAll),
            _ => None,
        };
        state.reset();
        return match fold {
            Some(op) => ParseResult::Action(Action::Fold(op)),
            None => ParseResult::Cancelled,
        };
    }

    // Resolve a pending f/F/t/T — the next key is the literal target char.
    if let Some(spec) = state.awaiting_find.take() {
        let count = state.total_count();
        let motion = MotionVerb::FindChar { ch, forward: spec.forward, before: spec.before };
        if let Some(op) = state.operator.take() {
            let register = state.take_register();
            state.reset();
            return ParseResult::Action(Action::Operate { op, motion, count, register });
        }
        state.reset();
        return ParseResult::Action(Action::Move { motion, count });
    }

    // Resolve text-object: previous key was `i` or `a` in op-pending or visual.
    if let Some(inner) = state.awaiting_textobj.take() {
        let obj = match ch {
            'w' => Some(TextObjectVerb::Word { inner }),
            'W' => Some(TextObjectVerb::BigWord { inner }),
            '"' | '\'' | '`' => Some(TextObjectVerb::Quotes { ch, inner }),
            '(' | ')' | 'b' => Some(TextObjectVerb::Pair { open: '(', close: ')', inner }),
            '[' | ']' => Some(TextObjectVerb::Pair { open: '[', close: ']', inner }),
            '{' | '}' | 'B' => Some(TextObjectVerb::Pair { open: '{', close: '}', inner }),
            '<' | '>' => Some(TextObjectVerb::Pair { open: '<', close: '>', inner }),
            _ => None,
        };
        let count = state.total_count();
        let op = state.operator.take();
        let register = state.take_register();
        state.reset();
        return match (obj, op, ctx) {
            (Some(o), Some(op), _) => ParseResult::Action(Action::OperateTextObject { op, obj: o, count, register }),
            (Some(o), None, ParseCtx::Visual) => ParseResult::Action(Action::VisualSelectTextObject { obj: o }),
            _ => ParseResult::Cancelled,
        };
    }

    // Resolve leader key (space) → picker dispatch. Only meaningful in normal mode.
    if state.awaiting_leader {
        state.awaiting_leader = false;
        if ctx == ParseCtx::Normal {
            // `b` opens a buffer-prefix sub-menu — defer to the next key.
            if ch == 'b' {
                state.awaiting_buffer_leader = true;
                return ParseResult::Pending;
            }
            // `d` opens the debugger sub-menu (`<leader>db`, `<leader>dc`, …).
            if ch == 'd' {
                state.awaiting_debug_leader = true;
                return ParseResult::Pending;
            }
            // `h` opens the git-hunk sub-menu (`<leader>hp` preview,
            // `<leader>hs` stage, `<leader>hu` unstage, `<leader>hr` reset).
            if ch == 'h' {
                state.awaiting_hunk_leader = true;
                return ParseResult::Pending;
            }
            let action = match ch {
                ' ' => Some(Action::OpenPicker { kind: PickerLeader::Files }),
                '?' => Some(Action::OpenPicker { kind: PickerLeader::Recents }),
                'g' => Some(Action::OpenPicker { kind: PickerLeader::Grep }),
                'e' => Some(Action::OpenYazi),
                // Doc-symbol / workspace-symbol pickers moved under
                // `<leader>d` so the debug sub-menu collects every
                // "navigate around code while debugging" action in one
                // place; Code actions stays at top level since it's used
                // independently of any debug flow.
                'a' => Some(Action::OpenPicker { kind: PickerLeader::CodeActions }),
                'r' => Some(Action::LspRename),
                'R' => Some(Action::ReplaceAllInBuffer),
                'f' => Some(Action::Format),
                _ => None,
            };
            if let Some(a) = action {
                state.reset();
                return ParseResult::Action(a);
            }
        }
        state.reset();
        return ParseResult::Cancelled;
    }

    // Buffer-prefix dispatch (after `<leader>b`).
    if state.awaiting_buffer_leader {
        state.awaiting_buffer_leader = false;
        let action = match ch {
            'd' => Some(Action::BufferDelete { force: false }),
            'D' => Some(Action::BufferDelete { force: true }),
            'o' => Some(Action::BufferOnly),
            'n' => Some(Action::BufferNext),
            'p' => Some(Action::BufferPrev),
            _ => None,
        };
        state.reset();
        return match action {
            Some(a) => ParseResult::Action(a),
            None => ParseResult::Cancelled,
        };
    }

    // Debugger-prefix dispatch (after `<leader>d`). `o` and `S` host the
    // doc-symbol / workspace-symbol pickers — Step out moves to `O`
    // (capital sibling of step-over `n` / step-in `i`) and Stop session
    // moves to `q` (matches the `:q` mnemonic).
    // Git-hunk prefix dispatch (after `<leader>h`).
    if state.awaiting_hunk_leader {
        state.awaiting_hunk_leader = false;
        let action = match ch {
            'p' => Some(Action::HunkPreview),
            's' => Some(Action::HunkStage),
            'u' => Some(Action::HunkUnstage),
            'r' => Some(Action::HunkReset),
            _ => None,
        };
        state.reset();
        return match action {
            Some(a) => ParseResult::Action(a),
            None => ParseResult::Cancelled,
        };
    }

    if state.awaiting_debug_leader {
        state.awaiting_debug_leader = false;
        let action = match ch {
            's' => Some(Action::Debug(DebugAction::Start)),
            'q' => Some(Action::Debug(DebugAction::Stop)),
            'b' => Some(Action::Debug(DebugAction::ToggleBreakpoint)),
            'B' => Some(Action::Debug(DebugAction::ClearBreakpointsInFile)),
            'c' => Some(Action::Debug(DebugAction::Continue)),
            'n' => Some(Action::Debug(DebugAction::Next)),
            'i' => Some(Action::Debug(DebugAction::StepIn)),
            'O' => Some(Action::Debug(DebugAction::StepOut)),
            'p' => Some(Action::Debug(DebugAction::PaneToggle)),
            'f' => Some(Action::Debug(DebugAction::FocusPane)),
            'o' => Some(Action::OpenPicker { kind: PickerLeader::DocumentSymbols }),
            'S' => Some(Action::OpenPicker { kind: PickerLeader::WorkspaceSymbols }),
            _ => None,
        };
        state.reset();
        return match action {
            Some(a) => ParseResult::Action(a),
            None => ParseResult::Cancelled,
        };
    }

    if state.awaiting_g {
        state.awaiting_g = false;
        // gd / gD jump to definition via LSP — only meaningful in normal mode.
        if ch == 'd' && ctx == ParseCtx::Normal && state.operator.is_none() {
            state.reset();
            return ParseResult::Action(Action::LspGotoDefinition);
        }
        // gr — find references via LSP. Opens a picker.
        if ch == 'r' && ctx == ParseCtx::Normal && state.operator.is_none() {
            state.reset();
            return ParseResult::Action(Action::LspFindReferences);
        }
        // gt / gT — Vim convention for next / previous tab. Aliases for
        // H / L which we already bind to the same actions.
        if ch == 't' && ctx == ParseCtx::Normal && state.operator.is_none() {
            state.reset();
            return ParseResult::Action(Action::BufferNext);
        }
        if ch == 'T' && ctx == ParseCtx::Normal && state.operator.is_none() {
            state.reset();
            return ParseResult::Action(Action::BufferPrev);
        }
        let mv = match ch {
            'g' => Some(MotionVerb::FirstLine),
            'e' => Some(MotionVerb::EndWordBackward),
            'E' => Some(MotionVerb::BigEndWordBackward),
            '_' => Some(MotionVerb::LastNonBlank),
            _ => None,
        };
        if let Some(motion) = mv {
            let count = state.total_count();
            if let Some(op) = state.operator.take() {
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::Operate { op, motion, count, register });
            }
            state.reset();
            return ParseResult::Action(Action::Move { motion, count });
        }
        state.reset();
        return ParseResult::Cancelled;
    }

    // Digits → counts. `0` is a digit only if a count is already in progress.
    if ch.is_ascii_digit() && !(ch == '0' && !state.slot_in_progress()) {
        let d = ch.to_digit(10).unwrap() as usize;
        state.push_digit(d);
        return ParseResult::Pending;
    }

    // Visual-only mode-switch keys.
    if ctx == ParseCtx::Visual {
        match ch {
            'v' => {
                state.reset();
                return ParseResult::Action(Action::VisualSwitch(VisualKind::Char));
            }
            'V' => {
                state.reset();
                return ParseResult::Action(Action::VisualSwitch(VisualKind::Line));
            }
            'o' => {
                state.reset();
                return ParseResult::Action(Action::VisualSwap);
            }
            'd' | 'D' | 'x' => {
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Delete, register });
            }
            'y' => {
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Yank, register });
            }
            'c' | 'C' | 's' => {
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Change, register });
            }
            'p' | 'P' => {
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::VisualPut { register });
            }
            'S' => {
                state.awaiting_visual_surround = true;
                return ParseResult::Pending;
            }
            '>' => {
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Indent, register: None });
            }
            '<' => {
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Outdent, register: None });
            }
            'i' | 'a' => {
                state.awaiting_textobj = Some(ch == 'i');
                return ParseResult::Pending;
            }
            _ => {}
        }
    } else {
        // Normal-mode entry into Visual.
        match ch {
            'v' => {
                state.reset();
                return ParseResult::Action(Action::EnterVisual(VisualKind::Char));
            }
            'V' => {
                state.reset();
                return ParseResult::Action(Action::EnterVisual(VisualKind::Line));
            }
            _ => {}
        }
    }

    // Surround pivots: `ds`, `cs` — when an operator (Delete or Change) is
    // already pending and the user types `s`, redirect to the surround
    // state machine instead of cancelling. `ys` (yank surround) is not
    // wired in this version.
    if ctx == ParseCtx::Normal && ch == 's' {
        if matches!(state.operator, Some(Operator::Delete)) {
            state.operator = None;
            state.awaiting_ds = true;
            return ParseResult::Pending;
        }
        if matches!(state.operator, Some(Operator::Change)) {
            state.operator = None;
            state.awaiting_cs_old = true;
            return ParseResult::Pending;
        }
    }

    // Resolve `ds{char}` / `cs{old}{new}` — first arg is always the next
    // printable char.
    if state.awaiting_ds {
        state.awaiting_ds = false;
        let target = ch;
        state.reset();
        return ParseResult::Action(Action::SurroundDelete { ch: target });
    }
    if state.awaiting_cs_old {
        state.awaiting_cs_old = false;
        state.cs_old = Some(ch);
        return ParseResult::Pending;
    }
    if let Some(old) = state.cs_old.take() {
        let new = ch;
        state.reset();
        return ParseResult::Action(Action::SurroundChange { from: old, to: new });
    }
    if state.awaiting_visual_surround {
        state.awaiting_visual_surround = false;
        let target = ch;
        state.reset();
        return ParseResult::Action(Action::SurroundVisual { ch: target });
    }

    // Operators (only in normal mode — visual handles d/c/y/>/< above).
    if ctx == ParseCtx::Normal && matches!(ch, 'd' | 'c' | 'y' | '>' | '<') {
        let op = match ch {
            'd' => Operator::Delete,
            'c' => Operator::Change,
            'y' => Operator::Yank,
            '>' => Operator::Indent,
            '<' => Operator::Outdent,
            _ => unreachable!(),
        };
        if let Some(existing) = state.operator {
            if existing == op {
                let count = state.count1.unwrap_or(1);
                let register = state.take_register();
                state.reset();
                return ParseResult::Action(Action::OperateLine { op, count, register });
            }
            state.reset();
            return ParseResult::Cancelled;
        }
        state.operator = Some(op);
        return ParseResult::Pending;
    }

    // In op-pending mode, `i` or `a` are text-object prefixes (not insert commands).
    if ctx == ParseCtx::Normal && state.operator.is_some() && (ch == 'i' || ch == 'a') {
        state.awaiting_textobj = Some(ch == 'i');
        return ParseResult::Pending;
    }

    // Bracket prefixes — `]q` / `[q` step through the quickfix list. The
    // pending state only kicks in when no operator is in flight (`d]` is
    // still text-object territory; brackets there go through `awaiting_textobj`).
    if ctx == ParseCtx::Normal && state.operator.is_none() {
        if ch == ']' {
            state.awaiting_bracket_close = true;
            return ParseResult::Pending;
        }
        if ch == '[' {
            state.awaiting_bracket_open = true;
            return ParseResult::Pending;
        }
    }

    if ch == 'g' {
        state.awaiting_g = true;
        return ParseResult::Pending;
    }

    if ch == 'z' {
        state.awaiting_z = true;
        return ParseResult::Pending;
    }

    if ch == ' '
        && (ctx == ParseCtx::Normal || ctx == ParseCtx::Visual)
        && state.operator.is_none()
    {
        state.awaiting_leader = true;
        return ParseResult::Pending;
    }

    // Mark prefixes — all three live before motion dispatch since `'` and `` ` `` are otherwise inert.
    let mark_act = match ch {
        'm' if ctx == ParseCtx::Normal => Some(MarkAction::Set),
        '\'' => Some(MarkAction::JumpLine),
        '`' => Some(MarkAction::JumpExact),
        _ => None,
    };
    if let Some(act) = mark_act {
        state.awaiting_mark = Some(act);
        return ParseResult::Pending;
    }

    // f / F / t / T set awaiting_find — next key is the literal target.
    let find_spec = match ch {
        'f' => Some(FindSpec { forward: true, before: false }),
        'F' => Some(FindSpec { forward: false, before: false }),
        't' => Some(FindSpec { forward: true, before: true }),
        'T' => Some(FindSpec { forward: false, before: true }),
        _ => None,
    };
    if let Some(spec) = find_spec {
        state.awaiting_find = Some(spec);
        return ParseResult::Pending;
    }

    // / and ? enter search mode.
    if ch == '/' || ch == '?' {
        state.reset();
        return ParseResult::Action(Action::EnterSearch { backward: ch == '?' });
    }

    let motion = match ch {
        'h' => Some(MotionVerb::Left),
        'l' => Some(MotionVerb::Right),
        'k' => Some(MotionVerb::Up),
        'j' => Some(MotionVerb::Down),
        'w' => Some(MotionVerb::WordForward),
        'b' => Some(MotionVerb::WordBackward),
        'W' => Some(MotionVerb::BigWordForward),
        'B' => Some(MotionVerb::BigWordBackward),
        'e' => Some(MotionVerb::EndWord),
        'E' => Some(MotionVerb::BigEndWord),
        '0' => Some(MotionVerb::LineStart),
        '^' => Some(MotionVerb::FirstNonBlank),
        '$' => Some(MotionVerb::LineEnd),
        // H / L are bound to buffer cycling instead of the viewport
        // top/bottom motions — see the singleton-Action match below.
        'M' => Some(MotionVerb::ViewportMiddle),
        'G' => {
            let n = state.count1.or(state.count2);
            Some(match n {
                Some(n) => MotionVerb::GotoLine(n),
                None => MotionVerb::LastLine,
            })
        }
        ';' => Some(MotionVerb::RepeatFind { reverse: false }),
        ',' => Some(MotionVerb::RepeatFind { reverse: true }),
        'n' => Some(MotionVerb::SearchNext { reverse: false }),
        'N' => Some(MotionVerb::SearchNext { reverse: true }),
        _ => None,
    };
    if let Some(m) = motion {
        let count = match m {
            MotionVerb::GotoLine(_) => 1,
            _ => state.total_count(),
        };
        if let Some(op) = state.operator.take() {
            let register = state.take_register();
            state.reset();
            return ParseResult::Action(Action::Operate { op, motion: m, count, register });
        }
        state.reset();
        return ParseResult::Action(Action::Move { motion: m, count });
    }

    // One-shot non-motion commands (Normal mode only).
    if ctx == ParseCtx::Normal {
        let act = match ch {
            'i' => Some(Action::EnterInsert(InsertWhere::Cursor)),
            'a' => Some(Action::EnterInsert(InsertWhere::AfterCursor)),
            'o' => Some(Action::EnterInsert(InsertWhere::LineBelow)),
            'O' => Some(Action::EnterInsert(InsertWhere::LineAbove)),
            'I' => Some(Action::EnterInsert(InsertWhere::LineFirstNonBlank)),
            'A' => Some(Action::EnterInsert(InsertWhere::LineEnd)),
            'x' => Some(Action::DeleteCharForward { count: state.total_count(), register: state.register }),
            'u' => Some(Action::Undo),
            'U' => Some(Action::Redo),
            'p' => Some(Action::Put { before: false, count: state.total_count(), register: state.register }),
            'P' => Some(Action::Put { before: true, count: state.total_count(), register: state.register }),
            ':' => Some(Action::EnterCommand),
            '.' => Some(Action::Repeat),
            // Buffer cycling — replaces the H/L viewport motions.
            // `<leader>bn`/`<leader>bp` still work for the same effect.
            'H' => Some(Action::BufferPrev),
            'L' => Some(Action::BufferNext),
            'D' => Some(Action::Operate {
                op: Operator::Delete,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
                register: state.register,
            }),
            'C' => Some(Action::Operate {
                op: Operator::Change,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
                register: state.register,
            }),
            'Y' => Some(Action::Operate {
                op: Operator::Yank,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
                register: state.register,
            }),
            'S' => Some(Action::OperateLine {
                op: Operator::Change,
                count: state.total_count(),
                register: state.register,
            }),
            'J' => Some(Action::JoinLines { count: state.total_count() }),
            '~' => Some(Action::ToggleCase { count: state.total_count() }),
            '*' => Some(Action::SearchWord { backward: false }),
            '#' => Some(Action::SearchWord { backward: true }),
            'K' => Some(Action::LspHover),
            _ => None,
        };
        if let Some(a) = act {
            state.reset();
            return ParseResult::Action(a);
        }
        if ch == 'r' {
            state.awaiting_replace = true;
            return ParseResult::Pending;
        }
        if ch == '"' {
            state.awaiting_register = true;
            return ParseResult::Pending;
        }
        if ch == 'q' {
            state.awaiting_macro_record = true;
            return ParseResult::Pending;
        }
        if ch == '@' {
            state.awaiting_macro_play = true;
            return ParseResult::Pending;
        }
    }

    if state.operator.is_some() {
        state.reset();
        return ParseResult::Cancelled;
    }
    ParseResult::Pending
}

fn is_valid_register(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '"' | '_' | '+' | '*' | '0'..='9')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn yy_emits_operate_line_yank() {
        let mut state = PendingCmd::default();
        match parse(&mut state, key('y'), ParseCtx::Normal) {
            ParseResult::Pending => {}
            other => panic!("first y produced {:?}", std::mem::discriminant(&other)),
        }
        match parse(&mut state, key('y'), ParseCtx::Normal) {
            ParseResult::Action(Action::OperateLine { op: Operator::Yank, count, register }) => {
                assert_eq!(count, 1);
                assert_eq!(register, None);
            }
            _ => panic!("second y did not produce OperateLine{{Yank}}"),
        }
    }

    #[test]
    fn ctrl_a_emits_adjust_number_plus_one() {
        let mut state = PendingCmd::default();
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        match parse(&mut state, key, ParseCtx::Normal) {
            ParseResult::Action(Action::AdjustNumber { delta, count }) => {
                assert_eq!(delta, 1);
                assert_eq!(count, 1);
            }
            _ => panic!("Ctrl-A did not emit AdjustNumber"),
        }
    }

    #[test]
    fn ctrl_x_with_count() {
        let mut state = PendingCmd::default();
        // 5<Ctrl-X> — should produce delta=-1 count=5.
        let _ = parse(
            &mut state,
            KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
            ParseCtx::Normal,
        );
        match parse(
            &mut state,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ParseCtx::Normal,
        ) {
            ParseResult::Action(Action::AdjustNumber { delta, count }) => {
                assert_eq!(delta, -1);
                assert_eq!(count, 5);
            }
            _ => panic!("5 Ctrl-X did not emit AdjustNumber"),
        }
    }

    #[test]
    fn ctrl_j_emits_move_line_down() {
        let mut state = PendingCmd::default();
        let k = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        match parse(&mut state, k, ParseCtx::Normal) {
            ParseResult::Action(Action::MoveLine { down, count }) => {
                assert!(down);
                assert_eq!(count, 1);
            }
            _ => panic!("Ctrl-J did not emit MoveLine"),
        }
    }

    #[test]
    fn ctrl_k_with_count_emits_move_line_up_n() {
        let mut state = PendingCmd::default();
        let _ = parse(
            &mut state,
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
            ParseCtx::Normal,
        );
        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        match parse(&mut state, k, ParseCtx::Normal) {
            ParseResult::Action(Action::MoveLine { down, count }) => {
                assert!(!down);
                assert_eq!(count, 3);
            }
            _ => panic!("3 Ctrl-K did not emit MoveLine"),
        }
    }

    #[test]
    fn count_prefixed_yy() {
        let mut state = PendingCmd::default();
        for k in ['3', 'y', 'y'] {
            let r = parse(&mut state, key(k), ParseCtx::Normal);
            if k == 'y' && matches!(r, ParseResult::Action(_)) {
                if let ParseResult::Action(Action::OperateLine {
                    op: Operator::Yank,
                    count,
                    register: None,
                }) = r
                {
                    assert_eq!(count, 3);
                    return;
                }
                panic!("3yy did not yank 3 lines");
            }
        }
        panic!("3yy never produced an action");
    }
}
