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
    ViewportTop,
    ViewportMiddle,
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

#[derive(Debug, Clone, Copy)]
pub enum PickerLeader {
    Files,
    Buffers,
    Grep,
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
    OpenPicker { kind: PickerLeader },
    OpenYazi,
    LspGotoDefinition,
    LspHover,
    VisualOperate { op: Operator, register: Option<char> },
    VisualSelectTextObject { obj: TextObjectVerb },
    VisualSwap,
    VisualSwitch(VisualKind),
    StartMacro { name: char },
    ReplayMacro { name: char },
    BufferDelete { force: bool },
    BufferOnly,
    BufferNext,
    BufferPrev,
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
        state.reset();
        return match kind {
            Some(k) => ParseResult::Action(Action::AdjustViewport(k)),
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
            let action = match ch {
                ' ' => Some(Action::OpenPicker { kind: PickerLeader::Files }),
                'g' => Some(Action::OpenPicker { kind: PickerLeader::Grep }),
                'e' => Some(Action::OpenYazi),
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
            ' ' | 'b' => Some(Action::OpenPicker { kind: PickerLeader::Buffers }),
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

    if state.awaiting_g {
        state.awaiting_g = false;
        // gd / gD jump to definition via LSP — only meaningful in normal mode.
        if ch == 'd' && ctx == ParseCtx::Normal && state.operator.is_none() {
            state.reset();
            return ParseResult::Action(Action::LspGotoDefinition);
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

    if ch == 'g' {
        state.awaiting_g = true;
        return ParseResult::Pending;
    }

    if ch == 'z' {
        state.awaiting_z = true;
        return ParseResult::Pending;
    }

    if ch == ' ' && ctx == ParseCtx::Normal && state.operator.is_none() {
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
        'H' => Some(MotionVerb::ViewportTop),
        'M' => Some(MotionVerb::ViewportMiddle),
        'L' => Some(MotionVerb::ViewportBottom),
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
