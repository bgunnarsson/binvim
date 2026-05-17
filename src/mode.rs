#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Visual(VisualKind),
    Search { backward: bool },
    Picker,
    /// Free-form prompt — used by LSP rename and any future single-string
    /// input flow. The associated kind tells the dispatcher what to do
    /// with the typed string on Enter.
    Prompt(PromptKind),
    /// Focus is in the bottom debug pane (frames + locals tree). `j`/`k`
    /// move the selection; `Enter` / `Tab` toggles a variable's expansion;
    /// `Esc` returns to Normal mode in the editor.
    DebugPane,
    /// Focus is on the `:terminal` overlay. Every keystroke is
    /// translated to bytes and forwarded to the PTY (so the embedded
    /// shell sees them, not the editor). `Esc` leaves Terminal mode
    /// for `TerminalNormal` — the grid stays painted but the editor
    /// regains its bindings.
    Terminal,
    /// Terminal overlay still showing, but the editor's bindings are
    /// active again. Used for "look at the output / scroll without
    /// sending input to the shell." `i` / `a` re-enters Terminal
    /// mode; `:q` (or `<C-w>q`) closes the terminal and drops the
    /// overlay.
    TerminalNormal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `<leader>r` — typed string is the new name for the symbol under
    /// the cursor at the time the prompt was opened.
    Rename,
    /// `Ctrl-D` — typed string replaces every occurrence of the word
    /// under the cursor in the current buffer. Literal-string match, not
    /// LSP-aware (use `<leader>r` for that).
    ReplaceAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
    Char,
    Line,
    /// Rectangular selection — `Ctrl-V`. Anchor and cursor define opposite
    /// corners of the rectangle; operators apply column-wise per line.
    Block,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Visual(VisualKind::Char) => "VISUAL",
            Mode::Visual(VisualKind::Line) => "V-LINE",
            Mode::Visual(VisualKind::Block) => "V-BLOCK",
            Mode::Search { .. } => "SEARCH",
            Mode::Picker => "PICK",
            Mode::Prompt(_) => "PROMPT",
            Mode::DebugPane => "DEBUG",
            Mode::Terminal => "TERMINAL",
            Mode::TerminalNormal => "T-NORMAL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}
