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
}

#[derive(Debug, Clone, Copy)]
pub enum MarkAction {
    Set,
    JumpLine,
    JumpExact,
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
}

#[derive(Debug, Clone)]
pub enum Action {
    Move { motion: MotionVerb, count: usize },
    Operate { op: Operator, motion: MotionVerb, count: usize },
    OperateLine { op: Operator, count: usize },
    OperateTextObject { op: Operator, obj: TextObjectVerb, count: usize },
    EnterInsert(InsertWhere),
    DeleteCharForward { count: usize },
    ReplaceChar { ch: char, count: usize },
    JoinLines { count: usize },
    ToggleCase { count: usize },
    Undo,
    Redo,
    Put { before: bool, count: usize },
    EnterCommand,
    EnterSearch { backward: bool },
    EnterVisual(VisualKind),
    Repeat,
    PageScroll(PageScrollKind),
    AdjustViewport(ViewportAdjust),
    SetMark { name: char },
    SearchWord { backward: bool },
    VisualOperate { op: Operator },
    VisualSelectTextObject { obj: TextObjectVerb },
    VisualSwap,
    VisualSwitch(VisualKind),
}

#[derive(Debug, Clone, Default)]
pub struct PendingCmd {
    pub count1: Option<usize>,
    pub operator: Option<Operator>,
    pub count2: Option<usize>,
    pub awaiting_g: bool,
    pub awaiting_z: bool,
    /// `Some(true)` for inner (`i`), `Some(false)` for around (`a`).
    pub awaiting_textobj: Option<bool>,
    /// Set after f/F/t/T — next char is the literal find target.
    pub awaiting_find: Option<FindSpec>,
    /// Set after `r` — next char is the replacement.
    pub awaiting_replace: bool,
    pub awaiting_mark: Option<MarkAction>,
}

impl PendingCmd {
    fn reset(&mut self) {
        *self = PendingCmd::default();
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
            _ => ParseResult::Pending,
        };
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
            state.reset();
            return ParseResult::Action(Action::Operate { op, motion, count });
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
        state.reset();
        return match (obj, op, ctx) {
            (Some(o), Some(op), _) => ParseResult::Action(Action::OperateTextObject { op, obj: o, count }),
            (Some(o), None, ParseCtx::Visual) => ParseResult::Action(Action::VisualSelectTextObject { obj: o }),
            _ => ParseResult::Cancelled,
        };
    }

    if state.awaiting_g {
        state.awaiting_g = false;
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
                state.reset();
                return ParseResult::Action(Action::Operate { op, motion, count });
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
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Delete });
            }
            'y' => {
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Yank });
            }
            'c' | 'C' | 's' => {
                state.reset();
                return ParseResult::Action(Action::VisualOperate { op: Operator::Change });
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

    // Operators (only in normal mode — visual handles d/c/y above).
    if ctx == ParseCtx::Normal && matches!(ch, 'd' | 'c' | 'y') {
        let op = match ch {
            'd' => Operator::Delete,
            'c' => Operator::Change,
            'y' => Operator::Yank,
            _ => unreachable!(),
        };
        if let Some(existing) = state.operator {
            if existing == op {
                let count = state.count1.unwrap_or(1);
                state.reset();
                return ParseResult::Action(Action::OperateLine { op, count });
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
            state.reset();
            return ParseResult::Action(Action::Operate { op, motion: m, count });
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
            'x' => Some(Action::DeleteCharForward { count: state.total_count() }),
            'u' => Some(Action::Undo),
            'p' => Some(Action::Put { before: false, count: state.total_count() }),
            'P' => Some(Action::Put { before: true, count: state.total_count() }),
            ':' => Some(Action::EnterCommand),
            '.' => Some(Action::Repeat),
            'D' => Some(Action::Operate {
                op: Operator::Delete,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
            }),
            'C' => Some(Action::Operate {
                op: Operator::Change,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
            }),
            'Y' => Some(Action::Operate {
                op: Operator::Yank,
                motion: MotionVerb::LineEnd,
                count: state.total_count(),
            }),
            'S' => Some(Action::OperateLine {
                op: Operator::Change,
                count: state.total_count(),
            }),
            'J' => Some(Action::JoinLines { count: state.total_count() }),
            '~' => Some(Action::ToggleCase { count: state.total_count() }),
            '*' => Some(Action::SearchWord { backward: false }),
            '#' => Some(Action::SearchWord { backward: true }),
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
    }

    if state.operator.is_some() {
        state.reset();
        return ParseResult::Cancelled;
    }
    ParseResult::Pending
}
