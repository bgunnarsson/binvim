#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Visual(VisualKind),
    Search {
        backward: bool,
    },
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
    /// Focus is in the left-side file-tree pane. `j`/`k` move the
    /// cursor, `Enter` / `l` opens the file or expands the folder,
    /// `h` collapses, `q` / `Esc` closes the pane and returns to
    /// Normal in the editor.
    FileTree,
    /// Modal LSP-rename preview overlay. The server's `WorkspaceEdit`
    /// has been parsed but not applied yet — the user is picking which
    /// per-site edits to accept before binvim writes them to disk.
    /// `j`/`k` move the selection across edits, `<Space>` toggles the
    /// current edit, `a`/`n` flip every edit on/off, `o` opens the file
    /// at the selected edit (cancelling the preview), `<Enter>` applies
    /// only the enabled edits, `<Esc>` cancels the whole rename.
    RenamePreview,
    /// `:install` overlay — three-stage checkbox flow that mirrors the
    /// `binvim-install` CLI (bundles → optional Node versions → plan).
    /// `j/k` move, Space toggles, `a`/`n` all/none, Enter advances; on
    /// the plan stage `y` runs (with a lazygit-style suspend takeover
    /// so install output streams to the host terminal), `n` goes back,
    /// `q`/`Esc` cancels.
    Installer,
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
    /// `a` inside the file-tree pane — typed string is the new
    /// basename inside the cursor's parent dir (or the cursor dir
    /// itself, if the cursor is on a folder). Trailing `/` creates a
    /// directory; otherwise a regular file. Returns focus to the
    /// file-tree pane on Enter / Esc.
    FileTreeCreate,
    /// `r` inside the file-tree pane — typed string replaces the
    /// basename of the cursor entry. Pre-fills with the current
    /// basename. Returns focus to the file-tree pane on Enter / Esc.
    FileTreeRename,
    /// `<leader>Ac` — typed string names the new Android Virtual Device.
    /// The system image chosen in the preceding picker is stashed on
    /// `App.android.flow`; Enter kicks off create (installing the image
    /// first if it isn't downloaded yet).
    AndroidAvdName,
    /// `/` inside the DAP Console tab — typed string becomes a literal
    /// substring search across the visible (post-filter) console
    /// lines. Enter commits the query and jumps to the first match;
    /// `n`/`N` walks subsequent matches from the Console-mode key
    /// handler. Esc cancels without overwriting any prior query.
    DebugConsoleSearch,
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
            Mode::FileTree => "FILES",
            Mode::RenamePreview => "RENAME",
            Mode::Installer => "INSTALL",
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
    /// `gu` / `gU` / `g~` / `g?` — rewrite the range's case in place.
    Case(CaseOp),
}

/// What a case operator does to each character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseOp {
    Lower,
    Upper,
    Toggle,
    Rot13,
}

impl CaseOp {
    /// The case operator a key names after `g`: `u`, `U`, `~` or `?`.
    pub fn for_key(ch: char) -> Option<Self> {
        match ch {
            'u' => Some(CaseOp::Lower),
            'U' => Some(CaseOp::Upper),
            '~' => Some(CaseOp::Toggle),
            '?' => Some(CaseOp::Rot13),
            _ => None,
        }
    }

    pub fn apply(self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match self {
                CaseOp::Lower => out.extend(c.to_lowercase()),
                CaseOp::Upper => out.extend(c.to_uppercase()),
                CaseOp::Toggle if c.is_uppercase() => out.extend(c.to_lowercase()),
                CaseOp::Toggle => out.extend(c.to_uppercase()),
                CaseOp::Rot13 => out.push(rot13(c)),
            }
        }
        out
    }
}

fn rot13(c: char) -> char {
    let base = match c {
        'a'..='z' => b'a',
        'A'..='Z' => b'A',
        _ => return c,
    };
    ((c as u8 - base + 13) % 26 + base) as char
}
