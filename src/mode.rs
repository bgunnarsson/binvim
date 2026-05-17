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
    /// Focus is on the `:terminal` pane. Every keystroke (including
    /// `Esc`) is translated to bytes and forwarded to the PTY — the
    /// embedded shell behaves like a normal terminal, no Vim sub-
    /// mode layered on. `<C-w>` is the lone escape hatch: it pops
    /// the user back to `Normal` (and primes the window-leader
    /// parser so `<C-w>k` etc. continue to work). Selection /
    /// copy works through the host terminal app's native Shift+drag
    /// → Cmd-C path.
    Terminal,
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
