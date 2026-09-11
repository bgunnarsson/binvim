#[derive(Debug, Clone)]
pub enum ExCommand {
    Write,
    WriteAs(String),
    Quit,
    QuitForce,
    WriteQuit,
    /// `:x` / `ZZ` — write only if modified, then quit.
    WriteQuitIfModified,
    /// `:wa` — write every modified buffer.
    WriteAll,
    /// `:qa` — quit unless a buffer has unsaved changes.
    QuitAll,
    /// `:qa!` — quit, discarding every unsaved change.
    QuitAllForce,
    /// `:wqa` / `:xa` — write every modified buffer, then quit.
    WriteQuitAll,
    /// `:e!` — reload the file from disk, discarding changes.
    Revert,
    Edit(String),
    Goto(usize),
    BufferNext,
    BufferPrev,
    BufferDelete {
        force: bool,
    },
    BufferList,
    BufferSwitch(String),
    /// `:s/pat/repl/[flags]` — Vim's pattern and replacement syntax.
    Substitute {
        range: ExRange,
        pattern: String,
        replacement: String,
        flags: SubFlags,
    },
    /// `:S/pat/repl/[flags]` — project-wide substitute. Scans the workspace
    /// with ripgrep, applies the substitution to every matching file,
    /// saves each one. The range prefix (if any) is ignored.
    ProjectSubstitute {
        pattern: String,
        replacement: String,
        flags: SubFlags,
    },
    /// `:d [x] [count]` — the lines into register `x`.
    DeleteRange {
        range: ExRange,
        register: Option<char>,
        count: Option<usize>,
    },
    /// `:{range}!cmd` — the lines through a shell command, replaced by what
    /// it prints.
    Filter {
        range: ExRange,
        cmd: String,
    },
    /// `:y [x] [count]`.
    YankRange {
        range: ExRange,
        register: Option<char>,
        count: Option<usize>,
    },
    /// `:m` / `:t` / `:co` — the lines moved, or copied, to below the `to`
    /// address; `0` puts them at the top.
    MoveLines {
        range: ExRange,
        to: LineSpec,
        copy: bool,
    },
    /// `:j[!] [count]` — the lines joined as `J` does, or as `gJ` with `!`.
    JoinRange {
        range: ExRange,
        count: Option<usize>,
        spaces: bool,
    },
    /// `:>` / `:<` — the lines shifted `times` indents, one per `>` / `<`.
    ShiftRange {
        range: ExRange,
        count: Option<usize>,
        right: bool,
        times: usize,
    },
    /// `:pu[t][!] [x]` — a register's text on lines of its own below the
    /// line, or above it with `!`.
    PutLines {
        range: ExRange,
        register: Option<char>,
        above: bool,
    },
    /// `:le [indent]` / `:ri [width]` / `:ce [width]`.
    AlignLines {
        range: ExRange,
        align: Align,
        width: Option<usize>,
    },
    /// `:retab[!] [N]`.
    Retab {
        range: ExRange,
        bang: bool,
        tabstop: Option<usize>,
    },
    /// `:norm[al][!] {keys}` — the keys typed in Normal mode on each line;
    /// `remap: false` (`!`) leaves `[keymaps]` out.
    Normal {
        range: ExRange,
        keys: String,
        remap: bool,
    },
    /// `:g/pat/cmd` — `cmd` on every line of `range` that matches `pattern`,
    /// or, `invert`ed (`:g!` / `:v`), doesn't.
    Global {
        range: ExRange,
        pattern: String,
        invert: bool,
        cmd: String,
    },
    /// `:sor[t][!] [i n x u r] [/pat/]` — the lines sorted, the whole file
    /// without a range.
    Sort {
        range: ExRange,
        opts: SortOpts,
    },
    /// `:sp` / `:vs` / `:new` / `:vne[w]` — the window split, the new pane
    /// showing `file`, a fresh empty buffer (`empty`, `:new` without a file)
    /// or the same buffer.
    Split {
        vertical: bool,
        file: Option<String>,
        empty: bool,
    },
    /// `:gr[ep][!] args` — `rg --vimgrep args` through the shell, its matches
    /// the quickfix list, the first jumped to unless `!`.
    Grep {
        args: String,
        jump: bool,
    },
    /// `:vim[grep] /pat/[g][j] files` — a Vim pattern searched for over
    /// `files` into the quickfix list; `g` keeps every match on a line, `j`
    /// stays put.
    VimGrep {
        pattern: String,
        files: String,
        all: bool,
        jump: bool,
    },
    /// `:mak[e][!] [args]` — the workspace's build run in a task tab, its
    /// errors the quickfix list when it exits, the first jumped to unless `!`.
    Make {
        args: String,
        jump: bool,
    },
    /// `:bufdo` / `:windo` / `:cdo` / `:cfdo` `{cmd}` — `cmd` run in each
    /// buffer, window, quickfix entry or quickfix file, `range` picking them
    /// by number.
    Each {
        range: ExRange,
        over: EachOver,
        cmd: String,
    },
    /// `:on[ly]` — every window but this one closed.
    OnlyWindow,
    /// `:clo[se]` — this window closed.
    CloseWindow,
    /// `:!cmd` — `cmd` run by the shell, its output shown.
    Shell {
        cmd: String,
    },
    /// `:r [file]` — the file's lines below the line; the current file
    /// without one.
    ReadFile {
        range: ExRange,
        path: String,
    },
    /// `:r !cmd` — what `cmd` prints, below the line.
    ReadCommand {
        range: ExRange,
        cmd: String,
    },
    /// `:w !cmd` — the lines, the whole file without a range, on `cmd`'s
    /// stdin; its output shown and the file left as it is.
    WriteCommand {
        range: ExRange,
        cmd: String,
    },
    NoHighlight,
    /// `:se[t] [args]` (D4) — every option shown without any.
    Set(Vec<SetArg>),
    /// `:ea[rlier]` / `:lat[er]` `[N|Ns|Nm|Nh|Nd|Nf]`.
    UndoTime {
        earlier: bool,
        amount: UndoAmount,
    },
    /// `:undol[ist]` — the ends of the undo branches.
    UndoList,
    /// `:cd [dir]` / `:chd[ir]` — the working directory moved to `dir`, the
    /// home directory without one, or the previous one for `-`.
    ChangeDir(String),
    /// `:pw[d]` — the working directory shown.
    PrintDir,
    Format,
    Health,
    /// `:messages` — open the captured `window/showMessage` /
    /// `window/logMessage` log as a scrollable overlay.
    Messages,
    /// `:reg` / `:registers` — open a scrollable overlay listing every
    /// yank register and recorded macro register with a short preview.
    Registers,
    /// `:dig[raphs]` — the Insert `Ctrl-K` digraphs.
    Digraphs,
    /// `:changes` — the change list in the list overlay.
    Changes,
    /// `:marks` — the buffer's marks in the list overlay.
    Marks,
    /// `:jumps` — the jump list in the list overlay.
    Jumps,
    /// `:codelens` — dump the active buffer's code-lens cache to the
    /// status line. Diagnostic aid for when the lens row isn't
    /// showing up: surfaces whether lenses were received, what lines
    /// they're anchored on, and the resolved-command state.
    CodeLensStatus,
    /// `:workspaces` / `:ws` — dump every running LSP client + the
    /// list of workspace folders it currently has attached. Surfaces
    /// the multi-root state for monorepo / sibling-repo sessions; in
    /// a single-root session each client lists exactly one folder.
    Workspaces,
    /// `:terminal [cmd]` — open the embedded terminal overlay. With
    /// no argument, spawns `$SHELL` (fallback `/bin/sh`); with an
    /// argument, spawns that command line.
    Terminal(Option<String>),
    /// `:task` / `:tasks` — open a fuzzy picker of every discoverable
    /// task in the workspace (npm scripts, justfile recipes, cargo
    /// aliases + builtins, Makefile targets, dotnet verbs). Selecting
    /// a task spawns it in a fresh, labelled bottom-terminal tab.
    TaskPicker,
    /// `:tasklast` / `:trun` — re-run the most recent task. No-op (with
    /// a status hint) when no task has been run yet this session.
    TaskLast,
    /// `:lazygit` / `:lg` — suspend the editor, run `lazygit` as a
    /// foreground child with the host terminal handed off to it
    /// directly (yazi-style takeover, not a PTY-embedded pane), and
    /// on exit reclaim the terminal + refresh git gutter state for
    /// every open buffer so staged / committed / checked-out changes
    /// show up immediately.
    Lazygit,
    /// `:install` — open the in-editor toolchain installer overlay.
    /// Three-stage flow (bundles → optional Node.js versions → plan),
    /// then suspends the editor lazygit-style and runs the installs
    /// against the shared `binvim::install` catalog.
    Install,
    /// `:update` — same overlay as `:install`, but the plan only upgrades
    /// tools already on `$PATH` (to the catalog's pinned / newest versions);
    /// tools that aren't installed are left for `:install`.
    Update,
    /// `:claude` / `:codex` / `:opencode` — open (or focus) the
    /// right-side AI-assistant terminal pane and start the named
    /// tool inside a fresh shell tab. Re-running the same command
    /// focuses the existing tab rather than spawning a duplicate.
    /// The PTY inherits the editor's cwd, so the tool runs from the
    /// project root.
    AiTool(AiTool),
    Debug(DebugSubCmd),
    /// `:dapwatch <expr>` / `:dapunwatch <idx>` / `:dapunwatch all`.
    DebugWatch(DebugWatchCmd),
    /// `:dapwatches` — open the watch list overlay (for listing /
    /// inspecting more than fits in the pane). For v1 we just
    /// surface this as a status line dump.
    DebugWatchesShow,
    /// Quickfix-list sub-commands — `:cn`/`:cp`/`:clist`/`:cfirst`/
    /// `:clast`/`:cdiag`/`:cclose`. Dispatch lives in `app/input.rs`.
    Quickfix(QuickfixSubCmd),
    /// `:Gblame` — toggle inline git-blame virtual text for every line
    /// of the active buffer.
    GitBlame,
    /// `:copilot [signin|signout|reload|status]` — bare `:copilot`
    /// reports current sign-in state; subcommands drive the auth
    /// flow without restarting the editor.
    Copilot(CopilotSubCmd),
    /// `:test` (picker) / `:testnearest` / `:testfile` / `:testlast`
    /// / `:testcancel` / `:testresults`. Dispatched into
    /// `app/test_glue.rs`.
    Test(TestSubCmd),
    /// `:spell` — toggle spell-check on the active buffer. Dispatched
    /// into `app/spell_glue.rs::cmd_spell_toggle`.
    SpellToggle,
    /// `:debugtest` — find the test enclosing the cursor and run it
    /// under the debugger. Dispatched into
    /// `app/dap_glue.rs::cmd_debug_test_nearest`.
    DebugTestNearest,
    Unknown(String),
    /// A command that parsed but can't run as typed; the status line says
    /// why.
    Invalid(String),
    /// `:&` / `:&&` / `:~` — the last `:s` again on `range`: with its own
    /// pattern, or the last search's for `:~`; with its flags only after a
    /// second `&`; and `flags` on top.
    RepeatSubstitute {
        range: ExRange,
        last_search: bool,
        keep_flags: bool,
        flags: SubFlags,
    },
    /// A command whose range names lines only the buffer knows — marks,
    /// searches, `.`, `$`, offsets. The app resolves it, then parses the
    /// rest with `parse_after_range`.
    Ranged {
        spec: RangeSpec,
        rest: String,
    },
}

/// AI-assistant launcher tags — one per shell command we know how
/// to spawn in the right-side terminal pane. Each variant maps to a
/// stable label + command via `label()` / `command()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTool {
    Claude,
    Codex,
    Opencode,
    OpenClaw,
    Hermes,
}

impl AiTool {
    /// Tab label + dedup key. Re-running `:claude` while a side tab
    /// labelled "claude" already exists focuses that tab.
    pub fn label(self) -> &'static str {
        match self {
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
            AiTool::Opencode => "opencode",
            AiTool::OpenClaw => "openclaw",
            AiTool::Hermes => "hermes",
        }
    }

    /// Shell command written to the freshly-spawned PTY's stdin
    /// (followed by `\n`). Defaults to the tool's name — assumes the
    /// binary is on the user's `$PATH`.
    pub fn command(self) -> &'static str {
        match self {
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
            AiTool::Opencode => "opencode",
            AiTool::OpenClaw => "openclaw",
            AiTool::Hermes => "hermes",
        }
    }
}

/// Test-runner sub-commands. Grouped so the dispatch arm stays tight.
#[derive(Debug, Clone, Copy)]
pub enum TestSubCmd {
    /// Open a picker of discovered tests for the active workspace.
    Picker,
    /// Run the test enclosing the cursor.
    Nearest,
    /// Run every test in the active buffer's file.
    File,
    /// Re-run the most recent test invocation.
    Last,
    /// Kill the running adapter.
    Cancel,
    /// Toggle the streaming results overlay.
    Results,
}

/// Sub-commands under `:copilot`. Bare `:copilot` falls through to
/// `Status` so the user gets a quick "am I signed in?" answer.
#[derive(Debug, Clone, Copy)]
pub enum CopilotSubCmd {
    Status,
    SignIn,
    SignOut,
    /// `:copilot reload` — re-fire `checkStatus`. Used to pick up
    /// "I just finished signing in" without waiting for the 3s poll.
    Reload,
}

/// Quickfix sub-commands. Grouped so the dispatch arm stays tight.
#[derive(Debug, Clone, Copy)]
pub enum QuickfixSubCmd {
    Next,
    Prev,
    First,
    Last,
    List,
    /// Replace the qf list with diagnostics from every open buffer.
    Diagnostics,
    /// Clear the qf list.
    Close,
}

/// Debugger sub-commands accessible via `:debug`, `:dapstop`, `:dapbreak`,
/// `:dapc`, etc. Grouped into one variant so the dispatch in `input.rs`
/// has a single arm and the parser stays compact.
#[derive(Debug, Clone)]
pub enum DebugSubCmd {
    Start,
    Stop,
    /// Bare `:dapb` — toggle a plain breakpoint at the cursor line.
    Break,
    /// `:dapb if <expr>` — attach a `condition` to the cursor line's
    /// breakpoint (creates one if absent). `None` strips the condition
    /// while keeping the breakpoint intact (same as `:dapb if`).
    BreakCondition(Option<String>),
    /// `:dapb hit <expr>` — attach a `hitCondition` (DAP-style: bare
    /// integer for "pause after N hits", `>= 5` for comparators).
    /// `None` strips it.
    BreakHitCondition(Option<String>),
    /// `:dapb plain` — strip BOTH `condition` and `hitCondition` from
    /// the cursor line's breakpoint, keeping it as an unconditional
    /// pause. Status hint when there's no breakpoint to plain-ify.
    BreakPlain,
    /// Clear every breakpoint in the active buffer.
    ClearBreakpointsInFile,
    Continue,
    Next,
    StepIn,
    StepOut,
    PaneToggle,
    FocusPane,
}

/// Watch-expression sub-commands accessible via `:dapwatch <expr>`
/// and `:dapunwatch <index>` / `:dapunwatch all`.
#[derive(Debug, Clone)]
pub enum DebugWatchCmd {
    /// Add `expr` to the watch list. Re-evaluated on every stop.
    Add(String),
    /// Remove the watch at `index` (1-based, matches `:health`-style
    /// listing). `None` = clear all watches.
    Remove(Option<usize>),
}

#[derive(Debug, Clone, Copy)]
pub enum ExRange {
    /// No range given — most commands default to current line.
    Implicit,
    /// `%` — whole buffer.
    Whole,
    /// `N` — single line.
    Single(usize),
    /// `N,M` — line range.
    Lines(usize, usize),
}

impl ExRange {
    /// The 0-based first and last of `len` items that `:bufdo`, `:windo`,
    /// `:cdo` and `:cfdo` pick with this range — all of them without one.
    /// `None` when it picks none.
    pub fn pick(self, len: usize) -> Option<(usize, usize)> {
        let (a, b) = match self {
            ExRange::Implicit | ExRange::Whole => (1, len),
            ExRange::Single(n) => (n, n),
            ExRange::Lines(a, b) => (a.min(b), a.max(b)),
        };
        if a == 0 || a > len {
            return None;
        }
        Some((a - 1, b.min(len) - 1))
    }
}

/// What `:bufdo` / `:windo` / `:cdo` / `:cfdo` run their command over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EachOver {
    Buffers,
    Windows,
    /// Every quickfix entry.
    Entries,
    /// The first entry of each run of entries in one file — where `:cnfile`
    /// stops.
    Files,
}

/// `:set`'s options (D4): full name, short name, and whether it holds a
/// number rather than being on or off.
pub const SET_OPTIONS: &[(&str, &str, bool)] = &[
    ("ignorecase", "ic", false),
    ("smartcase", "scs", false),
    ("wrapscan", "ws", false),
    ("hlsearch", "hls", false),
    ("incsearch", "is", false),
    ("textwidth", "tw", true),
    ("relativenumber", "rnu", false),
    ("list", "list", false),
    ("expandtab", "et", false),
    ("shiftwidth", "sw", true),
    ("tabstop", "ts", true),
];

/// One `:set` argument, its option named in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetArg {
    /// `opt?`, or a number option named alone — its value shown.
    Show(&'static str),
    /// `opt` / `noopt`.
    Flag(&'static str, bool),
    /// `opt!` / `invopt`.
    Toggle(&'static str),
    /// `opt=n` / `opt:n`.
    Number(&'static str, usize),
}

/// How far `:earlier` / `:later` go: undo steps, a time, or file writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoAmount {
    Steps(usize),
    Time(std::time::Duration),
    Writes(usize),
}

/// `:set`'s arguments, space-separated; `all`, or nothing, for every option.
fn parse_set_args(args: &str) -> Result<Vec<SetArg>, String> {
    if args == "all" {
        return Ok(Vec::new());
    }
    args.split_whitespace().map(parse_set_arg).collect()
}

fn parse_set_arg(arg: &str) -> Result<SetArg, String> {
    let option = |name: &str| {
        SET_OPTIONS
            .iter()
            .find(|&&(full, short, _)| name == full || name == short)
            .map(|&(full, _, number)| (full, number))
    };
    let unknown = || format!("E518: Unknown option: {arg}");
    let invalid = || format!("E474: Invalid argument: {arg}");
    if let Some((name, value)) = arg.split_once(['=', ':']) {
        let (name, number) = option(name).ok_or_else(unknown)?;
        if !number {
            return Err(invalid());
        }
        let value = value
            .parse()
            .map_err(|_| format!("E521: Number required after =: {arg}"))?;
        return Ok(SetArg::Number(name, value));
    }
    if let Some(name) = arg.strip_suffix('?') {
        let (name, _) = option(name).ok_or_else(unknown)?;
        return Ok(SetArg::Show(name));
    }
    if let Some(name) = arg.strip_suffix('!') {
        return match option(name) {
            Some((name, false)) => Ok(SetArg::Toggle(name)),
            Some(_) => Err(invalid()),
            None => Err(unknown()),
        };
    }
    match option(arg) {
        Some((name, true)) => return Ok(SetArg::Show(name)),
        Some((name, false)) => return Ok(SetArg::Flag(name, true)),
        None => {}
    }
    if let Some(Some((name, false))) = arg.strip_prefix("no").map(option) {
        return Ok(SetArg::Flag(name, false));
    }
    if let Some(Some((name, false))) = arg.strip_prefix("inv").map(option) {
        return Ok(SetArg::Toggle(name));
    }
    Err(unknown())
}

/// `:sort`'s options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortOpts {
    /// `!` — the other way round.
    pub reverse: bool,
    /// `i`.
    pub ignore_case: bool,
    /// `n` — on the first decimal number in the line.
    pub numeric: bool,
    /// `x` — on the first hex number.
    pub hex: bool,
    /// `u` — only the first of lines that sort the same.
    pub unique: bool,
    /// `r` — on what `pattern` matches rather than what follows it.
    pub on_match: bool,
    /// `/pat/`; empty is the last search.
    pub pattern: Option<String>,
}

/// Where `:le` / `:ri` / `:ce` put lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// One line address, as typed: the app resolves it against the buffer,
/// since marks, searches and `.` need one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// `N`.
    Line(usize),
    /// `.`, and a bare `+N` / `-N`.
    Current,
    /// `$`.
    Last,
    /// `'x` — a mark, `'<` / `'>` included.
    Mark(char),
    /// `/pat/` or `?pat?`: the next line down, or up, with a match. An empty
    /// pattern — `//`, or `\/` / `\?` — is the last search.
    Search { pattern: String, backward: bool },
}

/// An address with the `+N` / `-N` offsets after it added up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpec {
    pub base: Address,
    pub offset: isize,
}

/// A range as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeSpec {
    /// `%`.
    Whole,
    /// One address, or two joined by `,` — or by `;`, which counts the
    /// second from the first rather than from the cursor.
    Addresses {
        first: LineSpec,
        second: Option<(LineSpec, bool)>,
    },
}

impl RangeSpec {
    /// The range as plain line numbers, when that's all it is — so `:5,7d`
    /// and `:%s` need nothing from the buffer.
    fn as_numbers(&self) -> Option<ExRange> {
        let number = |spec: &LineSpec| match spec.base {
            Address::Line(n) if spec.offset == 0 => Some(n),
            _ => None,
        };
        match self {
            RangeSpec::Whole => Some(ExRange::Whole),
            RangeSpec::Addresses {
                first,
                second: None,
            } => number(first).map(ExRange::Single),
            RangeSpec::Addresses {
                first,
                second: Some((second, _)),
            } => Some(ExRange::Lines(number(first)?, number(second)?)),
        }
    }
}

pub fn parse(line: &str) -> ExCommand {
    // Only the start: `:normal` types trailing blanks as keys.
    let line = line.trim_start();
    if line.trim_end().is_empty() {
        return ExCommand::Unknown(String::new());
    }
    // Bare line number: ":42" jumps to that line.
    if let Ok(n) = line.parse::<usize>() {
        return ExCommand::Goto(n);
    }

    let (spec, rest) = match split_range(line) {
        Ok(split) => split,
        Err(e) => return ExCommand::Invalid(e),
    };
    let range = match spec.map(|spec| spec.as_numbers().ok_or(spec)) {
        None => ExRange::Implicit,
        Some(Ok(range)) => range,
        Some(Err(spec)) => {
            return ExCommand::Ranged {
                spec,
                rest: rest.trim_start().to_string(),
            };
        }
    };
    parse_after_range(range, rest, line)
}

/// The command after its range, once the range is plain line numbers.
/// `line` is the whole command line, for the commands that take no range.
pub fn parse_after_range(range: ExRange, rest: &str, line: &str) -> ExCommand {
    if let Some(command) = parse_normal(range, rest) {
        return command;
    }
    if let Some(command) = parse_global(range, rest) {
        return command;
    }
    if let Some(command) = parse_each(range, rest) {
        return command;
    }
    // Range-only commands: shorthand for `:Nd`, `:%d`, etc.
    let rest = rest.trim();
    // `:&` / `:~`, and `:&&` / `:~&`, which keep the last `:s`'s flags.
    let repeat = match rest.chars().next() {
        Some('&') => Some(false),
        Some('~') => Some(true),
        _ => None,
    };
    if let Some(last_search) = repeat {
        let args = &rest[1..];
        let (keep_flags, flags) = match args.strip_prefix('&') {
            Some(flags) => (true, flags),
            None => (false, args),
        };
        return match parse_sub_flags(flags.trim()) {
            Ok(flags) => ExCommand::RepeatSubstitute {
                range,
                last_search,
                keep_flags,
                flags,
            },
            Err(e) => ExCommand::Invalid(e),
        };
    }
    if let Some(args) = rest.strip_prefix('s') {
        match parse_substitute_args(args) {
            Some(Ok((pattern, replacement, flags))) => {
                return ExCommand::Substitute {
                    range,
                    pattern,
                    replacement,
                    flags,
                };
            }
            Some(Err(e)) => return ExCommand::Invalid(e),
            None => {}
        }
    }
    if let Some(args) = rest.strip_prefix('S') {
        match parse_substitute_args(args) {
            Some(Ok((pattern, replacement, flags))) => {
                return ExCommand::ProjectSubstitute {
                    pattern,
                    replacement,
                    flags,
                };
            }
            Some(Err(e)) => return ExCommand::Invalid(e),
            None => {}
        }
    }
    // `:!cmd` runs a command; after a range it filters the lines instead.
    if let Some(cmd) = rest.strip_prefix('!') {
        let cmd = cmd.trim().to_string();
        if !matches!(range, ExRange::Implicit) {
            return ExCommand::Filter { range, cmd };
        }
        if cmd.is_empty() {
            return ExCommand::Invalid("E471: Argument required".into());
        }
        return ExCommand::Shell { cmd };
    }
    if let Some(command) = parse_line_command(range, rest) {
        return command;
    }

    // A range and nothing after it goes to its last line, as `:'a` does.
    if rest.is_empty() {
        match range {
            ExRange::Single(n) | ExRange::Lines(_, n) => return ExCommand::Goto(n),
            ExRange::Whole => return ExCommand::Goto(usize::MAX),
            ExRange::Implicit => {}
        }
    }
    // Anything left that opened with a range but didn't match → unknown.
    if !matches!(range, ExRange::Implicit) {
        return ExCommand::Unknown(line.to_string());
    }

    if let Some(command) = parse_quickfix_command(rest) {
        return command;
    }
    let (head, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    if let Some(command) = parse_window_command(head, rest) {
        return command;
    }
    if let Some(command) = parse_undo_command(head, rest) {
        return command;
    }
    match head {
        "w" | "write" => {
            if rest.is_empty() {
                ExCommand::Write
            } else {
                ExCommand::WriteAs(rest.to_string())
            }
        }
        "q" | "quit" => ExCommand::Quit,
        "q!" | "quit!" => ExCommand::QuitForce,
        "wq" => ExCommand::WriteQuit,
        "x" | "xit" | "exit" => ExCommand::WriteQuitIfModified,
        "wa" | "wall" => ExCommand::WriteAll,
        "qa" | "qall" | "quitall" => ExCommand::QuitAll,
        "qa!" | "qall!" | "quitall!" => ExCommand::QuitAllForce,
        "wqa" | "wqall" | "xa" | "xall" => ExCommand::WriteQuitAll,
        "e" | "edit" => ExCommand::Edit(rest.to_string()),
        "e!" | "edit!" if rest.is_empty() => ExCommand::Revert,
        "e!" | "edit!" => ExCommand::Edit(rest.to_string()),
        // `:e#` / `:b#` are usually typed without the space.
        "e#" | "edit#" => ExCommand::Edit("#".into()),
        "b#" | "buffer#" => ExCommand::BufferSwitch("#".into()),
        "bn" | "bnext" => ExCommand::BufferNext,
        "bp" | "bprev" | "bprevious" => ExCommand::BufferPrev,
        "bd" | "bdelete" => ExCommand::BufferDelete { force: false },
        "bd!" | "bdelete!" => ExCommand::BufferDelete { force: true },
        "ls" | "buffers" => ExCommand::BufferList,
        "b" | "buffer" => ExCommand::BufferSwitch(rest.to_string()),
        "noh" | "nohlsearch" => ExCommand::NoHighlight,
        "se" | "set" => match parse_set_args(rest) {
            Ok(args) => ExCommand::Set(args),
            Err(e) => ExCommand::Invalid(e),
        },
        "cd" | "chd" | "chdi" | "chdir" => ExCommand::ChangeDir(rest.to_string()),
        "pw" | "pwd" => ExCommand::PrintDir,
        "fmt" | "format" => ExCommand::Format,
        "health" | "checkhealth" => ExCommand::Health,
        "messages" | "message" | "mes" => ExCommand::Messages,
        "reg" | "registers" | "display" => ExCommand::Registers,
        "dig" | "digr" | "digra" | "digrap" | "digraph" | "digraphs" => ExCommand::Digraphs,
        "changes" => ExCommand::Changes,
        "marks" => ExCommand::Marks,
        "jumps" | "ju" => ExCommand::Jumps,
        "codelens" | "codelenses" => ExCommand::CodeLensStatus,
        "workspaces" | "ws" => ExCommand::Workspaces,
        "terminal" | "term" => {
            if rest.is_empty() {
                ExCommand::Terminal(None)
            } else {
                ExCommand::Terminal(Some(rest.to_string()))
            }
        }
        "task" | "tasks" => ExCommand::TaskPicker,
        "tasklast" | "trun" => ExCommand::TaskLast,
        "lazygit" | "lg" => ExCommand::Lazygit,
        "install" | "installer" => ExCommand::Install,
        "update" => ExCommand::Update,
        "claude" => ExCommand::AiTool(AiTool::Claude),
        "codex" => ExCommand::AiTool(AiTool::Codex),
        "opencode" => ExCommand::AiTool(AiTool::Opencode),
        "openclaw" => ExCommand::AiTool(AiTool::OpenClaw),
        "hermes" => ExCommand::AiTool(AiTool::Hermes),
        "debug" | "dap" => ExCommand::Debug(DebugSubCmd::Start),
        "dapstop" => ExCommand::Debug(DebugSubCmd::Stop),
        "dapbreak" | "dapb" => parse_dapbreak_args(rest),
        "dapclear" => ExCommand::Debug(DebugSubCmd::ClearBreakpointsInFile),
        "dapcontinue" | "dapc" => ExCommand::Debug(DebugSubCmd::Continue),
        "dapnext" | "dapn" => ExCommand::Debug(DebugSubCmd::Next),
        "dapin" | "dapi" => ExCommand::Debug(DebugSubCmd::StepIn),
        "dapout" | "dapo" => ExCommand::Debug(DebugSubCmd::StepOut),
        "dappane" => ExCommand::Debug(DebugSubCmd::PaneToggle),
        "dapwatch" | "dapw" => {
            if rest.is_empty() {
                ExCommand::Unknown("dapwatch needs an expression".into())
            } else {
                ExCommand::DebugWatch(DebugWatchCmd::Add(rest.to_string()))
            }
        }
        "dapunwatch" | "dapuw" => {
            if rest.is_empty() {
                ExCommand::Unknown("dapunwatch needs an index or 'all'".into())
            } else if rest == "all" || rest == "*" {
                ExCommand::DebugWatch(DebugWatchCmd::Remove(None))
            } else {
                match rest.parse::<usize>() {
                    Ok(n) if n >= 1 => ExCommand::DebugWatch(DebugWatchCmd::Remove(Some(n))),
                    _ => ExCommand::Unknown(format!(
                        "dapunwatch: expected positive integer or 'all', got `{rest}`"
                    )),
                }
            }
        }
        "dapwatches" => ExCommand::DebugWatchesShow,
        "cn" | "cnext" => ExCommand::Quickfix(QuickfixSubCmd::Next),
        "cp" | "cprev" | "cprevious" | "cN" => ExCommand::Quickfix(QuickfixSubCmd::Prev),
        "cfirst" | "cr" | "crewind" => ExCommand::Quickfix(QuickfixSubCmd::First),
        "clast" => ExCommand::Quickfix(QuickfixSubCmd::Last),
        "cl" | "clist" => ExCommand::Quickfix(QuickfixSubCmd::List),
        "cdiag" | "cdiagnostics" => ExCommand::Quickfix(QuickfixSubCmd::Diagnostics),
        "cclose" => ExCommand::Quickfix(QuickfixSubCmd::Close),
        "Gblame" | "gblame" => ExCommand::GitBlame,
        "copilot" => {
            let sub = match rest.trim() {
                "" | "status" => CopilotSubCmd::Status,
                "signin" | "login" => CopilotSubCmd::SignIn,
                "signout" | "logout" => CopilotSubCmd::SignOut,
                "reload" | "refresh" => CopilotSubCmd::Reload,
                _ => return ExCommand::Unknown(line.to_string()),
            };
            ExCommand::Copilot(sub)
        }
        "spell" | "spelltoggle" => ExCommand::SpellToggle,
        "debugtest" | "dt" | "dapdt" => ExCommand::DebugTestNearest,
        "test" | "testpick" => ExCommand::Test(TestSubCmd::Picker),
        "testnearest" | "testn" | "tn" => ExCommand::Test(TestSubCmd::Nearest),
        "testfile" | "testf" | "tf" => ExCommand::Test(TestSubCmd::File),
        "testlast" | "testl" | "tl" => ExCommand::Test(TestSubCmd::Last),
        "testcancel" | "testq" => ExCommand::Test(TestSubCmd::Cancel),
        "testresults" | "testr" => ExCommand::Test(TestSubCmd::Results),
        _ => ExCommand::Unknown(line.to_string()),
    }
}

/// Parse the optional argument tail on `:dapb`. Recognised forms:
/// - bare (no rest) → toggle a plain breakpoint
/// - `if <expr>` / `if` → set / clear the `condition`
/// - `hit <expr>` / `hit` → set / clear the `hitCondition`
/// - `plain` → strip both
///
/// Anything else lands in `ExCommand::Unknown` with a hint so the
/// user knows the expected shape.
fn parse_dapbreak_args(rest: &str) -> ExCommand {
    let rest = rest.trim();
    if rest.is_empty() {
        return ExCommand::Debug(DebugSubCmd::Break);
    }
    // Split on first whitespace; the head is the sub-verb, the tail
    // (if any) is the user expression and passes through verbatim.
    let (head, tail) = match rest.split_once(char::is_whitespace) {
        Some((h, t)) => (h, t.trim()),
        None => (rest, ""),
    };
    match head {
        "if" | "cond" | "condition" => {
            let expr = if tail.is_empty() {
                None
            } else {
                Some(tail.to_string())
            };
            ExCommand::Debug(DebugSubCmd::BreakCondition(expr))
        }
        "hit" | "hitcount" => {
            let expr = if tail.is_empty() {
                None
            } else {
                Some(tail.to_string())
            };
            ExCommand::Debug(DebugSubCmd::BreakHitCondition(expr))
        }
        "plain" | "clear" => ExCommand::Debug(DebugSubCmd::BreakPlain),
        _ => ExCommand::Unknown(format!(
            ":dapb expected `if <expr>` | `hit <expr>` | `plain`, got `{rest}`"
        )),
    }
}

/// Where `:cd arg` goes: the home directory without an argument, the
/// previous directory for `-`, `~` expanded, and anything else as typed —
/// relative to the working directory.
pub fn cd_target(
    arg: &str,
    previous: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    let home_or_fail = || {
        home.map(std::path::Path::to_path_buf)
            .ok_or_else(|| "E472: Command failed".to_string())
    };
    if arg.is_empty() || arg == "~" {
        return home_or_fail();
    }
    if arg == "-" {
        return previous
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| "E186: No previous directory".to_string());
    }
    if let Some(rest) = arg.strip_prefix("~/") {
        return home_or_fail().map(|home| home.join(rest));
    }
    Ok(std::path::PathBuf::from(arg))
}

/// `:ea[rlier]` / `:lat[er]` `[count][s|m|h|d|f]` and `:undol[ist]`.
fn parse_undo_command(head: &str, rest: &str) -> Option<ExCommand> {
    if abbreviates(head, "undolist", 5) {
        return Some(ExCommand::UndoList);
    }
    let earlier = if abbreviates(head, "earlier", 2) {
        true
    } else if abbreviates(head, "later", 3) {
        false
    } else {
        return None;
    };
    let command = match parse_undo_amount(rest) {
        Some(amount) => ExCommand::UndoTime { earlier, amount },
        None => ExCommand::Invalid(format!("E475: Invalid argument: {rest}")),
    };
    Some(command)
}

/// `:earlier`'s argument: a count, with `s` / `m` / `h` / `d` for a time or
/// `f` for file writes. No count, or none at all, is 1.
fn parse_undo_amount(text: &str) -> Option<UndoAmount> {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits);
    let n: u64 = if number.is_empty() {
        1
    } else {
        number.parse().ok()?
    };
    let time = |per: u64| {
        Some(UndoAmount::Time(std::time::Duration::from_secs(
            n.saturating_mul(per),
        )))
    };
    match unit {
        "" => Some(UndoAmount::Steps(n as usize)),
        "s" => time(1),
        "m" => time(60),
        "h" => time(3600),
        "d" => time(86400),
        "f" => Some(UndoAmount::Writes(n as usize)),
        _ => None,
    }
}

/// Whether `name` is `full` cut short, to no fewer than `min` letters, the
/// way Vim abbreviates command names.
fn abbreviates(name: &str, full: &str, min: usize) -> bool {
    name.len() >= min && full.starts_with(name)
}

/// `:sp` / `:vs` / `:new` / `:vne[w]` `[file]`, `:on[ly]` and `:clo[se]`.
fn parse_window_command(head: &str, rest: &str) -> Option<ExCommand> {
    let name = head.trim_end_matches('!');
    let file = (!rest.is_empty()).then(|| rest.to_string());
    let split = |vertical: bool, empty: bool| {
        Some(ExCommand::Split {
            vertical,
            file: file.clone(),
            empty,
        })
    };
    if abbreviates(name, "split", 2) {
        return split(false, false);
    }
    if abbreviates(name, "vsplit", 2) {
        return split(true, false);
    }
    if name == "new" {
        return split(false, true);
    }
    if abbreviates(name, "vnew", 3) {
        return split(true, true);
    }
    if abbreviates(name, "only", 2) && rest.is_empty() {
        return Some(ExCommand::OnlyWindow);
    }
    if abbreviates(name, "close", 3) && rest.is_empty() {
        return Some(ExCommand::CloseWindow);
    }
    None
}

/// `:gr[ep][!]`, `:vim[grep][!]` and `:mak[e][!]`, read from the whole line
/// so `:vimgrep/pat/ file` needs no space.
fn parse_quickfix_command(line: &str) -> Option<ExCommand> {
    let name_end = line
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(line.len());
    let (name, after) = line.split_at(name_end);
    let (bang, after) = match after.strip_prefix('!') {
        Some(after) => (true, after),
        None => (false, after),
    };
    let args = after.trim();
    if abbreviates(name, "grep", 2) {
        if args.is_empty() {
            return Some(ExCommand::Invalid("E471: Argument required".into()));
        }
        return Some(ExCommand::Grep {
            args: args.to_string(),
            jump: !bang,
        });
    }
    if abbreviates(name, "make", 3) {
        return Some(ExCommand::Make {
            args: args.to_string(),
            jump: !bang,
        });
    }
    if abbreviates(name, "vimgrep", 3) {
        return Some(parse_vimgrep_args(args));
    }
    None
}

/// `:vimgrep`'s `/pat/[g][j] files`, any non-identifier character standing
/// in for `/`, or a bare `pat files` — as Vim reads them.
fn parse_vimgrep_args(args: &str) -> ExCommand {
    let invalid = || ExCommand::Invalid("E683: File name missing or invalid pattern".into());
    let Some(delim) = args.chars().next() else {
        return invalid();
    };
    if delim.is_alphanumeric() || delim == '_' {
        let (pattern, files) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
        return ExCommand::VimGrep {
            pattern: pattern.to_string(),
            files: files.trim().to_string(),
            all: false,
            jump: true,
        };
    }
    let (pattern, Some(rest)) = split_pattern(&args[delim.len_utf8()..], delim) else {
        return invalid();
    };
    let flags_end = rest
        .find(|c: char| c != 'g' && c != 'j')
        .unwrap_or(rest.len());
    let (flags, files) = rest.split_at(flags_end);
    ExCommand::VimGrep {
        pattern,
        files: files.trim().to_string(),
        all: flags.contains('g'),
        jump: !flags.contains('j'),
    }
}

/// `:bufd[o]`, `:wind[o]`, `:cdo` and `:cfd[o]`, each with an optional `!`,
/// and the command after them kept as typed.
fn parse_each(range: ExRange, rest: &str) -> Option<ExCommand> {
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    let over = if abbreviates(name, "bufdo", 4) {
        EachOver::Buffers
    } else if abbreviates(name, "windo", 4) {
        EachOver::Windows
    } else if name == "cdo" {
        EachOver::Entries
    } else if abbreviates(name, "cfdo", 3) {
        EachOver::Files
    } else {
        return None;
    };
    let cmd = after.strip_prefix('!').unwrap_or(after).trim_start();
    if cmd.is_empty() {
        return Some(ExCommand::Invalid("E471: Argument required".into()));
    }
    Some(ExCommand::Each {
        range,
        over,
        cmd: cmd.to_string(),
    })
}

/// `:g/pat/cmd`, `:g!/pat/cmd` and `:v/pat/cmd`, with any delimiter `:s`
/// takes in place of `/`. The command after the pattern is kept as typed.
fn parse_global(range: ExRange, rest: &str) -> Option<ExCommand> {
    let rest = rest.trim_start();
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    let global = abbreviates(name, "global", 1);
    let vglobal = abbreviates(name, "vglobal", 1);
    if !global && !vglobal {
        return None;
    }
    let (bang, after) = match after.strip_prefix('!') {
        Some(after) => (true, after),
        None => (false, after),
    };
    let after = after.trim_start();
    let Some(delim) = after
        .chars()
        .next()
        .filter(|&c| !c.is_alphanumeric() && !matches!(c, '"' | '|' | '\\'))
    else {
        return Some(ExCommand::Invalid("E476: Invalid command".into()));
    };
    let (pattern, cmd) = split_pattern(&after[delim.len_utf8()..], delim);
    Some(ExCommand::Global {
        range,
        pattern,
        invert: vglobal || bang,
        cmd: cmd.unwrap_or("").to_string(),
    })
}

/// `:norm[al][!] {keys}` — the keys kept as typed, trailing blanks and all.
fn parse_normal(range: ExRange, rest: &str) -> Option<ExCommand> {
    let rest = rest.trim_start();
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    if !abbreviates(name, "normal", 4) {
        return None;
    }
    let (remap, keys) = match after.strip_prefix('!') {
        Some(keys) => (false, keys),
        None => (true, after),
    };
    // Vim skips the blanks between the name and the keys.
    let keys = keys.trim_start();
    if keys.is_empty() {
        return Some(ExCommand::Invalid("E471: Argument required".into()));
    }
    Some(ExCommand::Normal {
        range,
        keys: keys.to_string(),
        remap,
    })
}

/// The line commands — `:d` / `:y`, `:m`, `:t` / `:co`, `:j`, `:>` / `:<`,
/// `:pu`, `:le` / `:ri` / `:ce` and `:retab` — or `None` when `rest` is none
/// of them.
fn parse_line_command(range: ExRange, rest: &str) -> Option<ExCommand> {
    if let Some(dir @ ('>' | '<')) = rest.chars().next() {
        let times = rest.chars().take_while(|&c| c == dir).count();
        let command = match parse_count(&rest[times..]) {
            Ok(count) => ExCommand::ShiftRange {
                range,
                count,
                right: dir == '>',
                times,
            },
            Err(e) => ExCommand::Invalid(e),
        };
        return Some(command);
    }
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    let (bang, args) = match after.strip_prefix('!') {
        Some(args) => (true, args.trim()),
        None => (false, after.trim()),
    };
    let is = |full: &str, min: usize| abbreviates(name, full, min);
    let align = [
        ("left", Align::Left),
        ("right", Align::Right),
        ("center", Align::Center),
    ]
    .into_iter()
    .find(|&(full, _)| is(full, 2))
    .map(|(_, align)| align);
    let command = if is("delete", 1) && !bang {
        parse_register_count(args).map(|(register, count)| ExCommand::DeleteRange {
            range,
            register,
            count,
        })
    } else if is("yank", 1) && !bang {
        parse_register_count(args).map(|(register, count)| ExCommand::YankRange {
            range,
            register,
            count,
        })
    } else if (is("move", 1) || is("t", 1) || is("copy", 2)) && !bang {
        let copy = !is("move", 1);
        parse_address(args).map(|to| ExCommand::MoveLines { range, to, copy })
    } else if is("join", 1) {
        parse_count(args).map(|count| ExCommand::JoinRange {
            range,
            count,
            spaces: !bang,
        })
    } else if is("put", 2) {
        parse_register(args).map(|register| ExCommand::PutLines {
            range,
            register,
            above: bang,
        })
    } else if let Some(align) = align.filter(|_| !bang) {
        parse_number(args).map(|width| ExCommand::AlignLines {
            range,
            align,
            width,
        })
    } else if is("retab", 3) {
        parse_number(args).map(|tabstop| ExCommand::Retab {
            range,
            bang,
            tabstop,
        })
    } else if is("sort", 3) {
        parse_sort_args(args, bang).map(|opts| ExCommand::Sort { range, opts })
    } else if is("read", 1) {
        // `:r!cmd` reads a command too, with the `!` against the name.
        let command = match (bang, args.strip_prefix('!')) {
            (true, _) => ExCommand::ReadCommand {
                range,
                cmd: args.to_string(),
            },
            (false, Some(cmd)) => ExCommand::ReadCommand {
                range,
                cmd: cmd.trim().to_string(),
            },
            (false, None) => ExCommand::ReadFile {
                range,
                path: args.to_string(),
            },
        };
        Ok(command)
    } else if is("write", 1) && !bang && args.starts_with('!') {
        Ok(ExCommand::WriteCommand {
            range,
            cmd: args[1..].trim().to_string(),
        })
    } else {
        return None;
    };
    Some(command.unwrap_or_else(ExCommand::Invalid))
}

/// `:sort`'s flags and `/pat/`, in any order, with blanks between.
fn parse_sort_args(args: &str, reverse: bool) -> Result<SortOpts, String> {
    let mut opts = SortOpts {
        reverse,
        ..SortOpts::default()
    };
    let mut rest = args.trim_start();
    while let Some(c) = rest.chars().next() {
        match c {
            'i' => opts.ignore_case = true,
            'n' => opts.numeric = true,
            'x' => opts.hex = true,
            'u' => opts.unique = true,
            'r' => opts.on_match = true,
            _ if c.is_whitespace() => {}
            _ if !c.is_alphanumeric() && !matches!(c, '"' | '\\' | '|') => {
                let (pattern, after) = split_pattern(&rest[c.len_utf8()..], c);
                opts.pattern = Some(pattern);
                rest = after.unwrap_or("");
                continue;
            }
            _ => return Err(format!("E474: Invalid argument: {rest}")),
        }
        rest = &rest[c.len_utf8()..];
    }
    Ok(opts)
}

/// `[x] [count]` after `:d` / `:y`: a register name, then a count.
fn parse_register_count(args: &str) -> Result<(Option<char>, Option<usize>), String> {
    let (register, rest) = match args.chars().next() {
        Some(c) if !c.is_ascii_digit() => (Some(c), &args[c.len_utf8()..]),
        _ => (None, args),
    };
    Ok((register, parse_count(rest)?))
}

/// `[x]` after `:pu`: a register name and nothing else.
fn parse_register(args: &str) -> Result<Option<char>, String> {
    let mut chars = args.chars();
    match (chars.next(), chars.next()) {
        (None, _) => Ok(None),
        (Some(c), None) => Ok(Some(c)),
        _ => Err(format!("E488: Trailing characters: {args}")),
    }
}

/// The address after `:m` / `:t` / `:co`, and nothing else.
fn parse_address(args: &str) -> Result<LineSpec, String> {
    match parse_line_spec(args)? {
        (Some(spec), rest) if rest.trim().is_empty() => Ok(spec),
        (Some(_), rest) => Err(format!("E488: Trailing characters: {}", rest.trim())),
        (None, _) => Err("E14: Invalid address".into()),
    }
}

/// A number argument, or none.
fn parse_number(args: &str) -> Result<Option<usize>, String> {
    let args = args.trim();
    if args.is_empty() {
        return Ok(None);
    }
    args.parse()
        .map(Some)
        .map_err(|_| format!("E488: Trailing characters: {args}"))
}

/// A count after a command, which Vim wants to be at least one.
fn parse_count(args: &str) -> Result<Option<usize>, String> {
    match parse_number(args)? {
        Some(0) => Err("E939: Positive count required".into()),
        count => Ok(count),
    }
}

/// The range at the front of a command line, as typed, and the text after
/// it: `%`, or one or two addresses joined by `,` / `;`. A missing address
/// either side of the separator is `.`.
pub fn split_range(line: &str) -> Result<(Option<RangeSpec>, &str), String> {
    let s = line.trim_start();
    if let Some(rest) = s.strip_prefix('%') {
        return Ok((Some(RangeSpec::Whole), rest));
    }
    let (first, rest) = parse_line_spec(s)?;
    let Some(sep) = rest.chars().next().filter(|c| matches!(c, ',' | ';')) else {
        let spec = first.map(|first| RangeSpec::Addresses {
            first,
            second: None,
        });
        return Ok((spec, rest));
    };
    let (second, rest) = parse_line_spec(&rest[1..])?;
    let current = || LineSpec {
        base: Address::Current,
        offset: 0,
    };
    let spec = RangeSpec::Addresses {
        first: first.unwrap_or_else(current),
        second: Some((second.unwrap_or_else(current), sep == ';')),
    };
    Ok((Some(spec), rest))
}

/// One address and its offsets from the front of `s`, and the text after.
fn parse_line_spec(s: &str) -> Result<(Option<LineSpec>, &str), String> {
    let s = s.trim_start();
    let mut chars = s.chars();
    let (base, mut rest) = match chars.next() {
        Some('.') => (Some(Address::Current), &s[1..]),
        Some('$') => (Some(Address::Last), &s[1..]),
        Some('\'') => {
            let name = chars.next().ok_or("E20: Mark not set")?;
            (Some(Address::Mark(name)), &s[1 + name.len_utf8()..])
        }
        Some(delim @ ('/' | '?')) => {
            let (pattern, after) = split_pattern(&s[1..], delim);
            let search = Address::Search {
                pattern,
                backward: delim == '?',
            };
            (Some(search), after.unwrap_or(""))
        }
        Some('\\') => match chars.next() {
            Some(delim @ ('/' | '?')) => {
                let search = Address::Search {
                    pattern: String::new(),
                    backward: delim == '?',
                };
                (Some(search), &s[2..])
            }
            _ => (None, s),
        },
        Some(c) if c.is_ascii_digit() => {
            let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            let n = s[..end].parse().unwrap_or(usize::MAX);
            (Some(Address::Line(n)), &s[end..])
        }
        _ => (None, s),
    };
    let mut offset: Option<isize> = None;
    while let Some(sign) = rest.chars().next().filter(|c| matches!(c, '+' | '-')) {
        let digits = rest[1..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(rest.len(), |i| i + 1);
        // A bare `+` or `-` is one line.
        let n = if digits == 1 {
            1
        } else {
            rest[1..digits].parse().unwrap_or(isize::MAX)
        };
        let by = if sign == '+' { n } else { -n };
        offset = Some(offset.unwrap_or(0).saturating_add(by));
        rest = &rest[digits..];
    }
    let spec = match (base, offset) {
        (Some(base), offset) => Some(LineSpec {
            base,
            offset: offset.unwrap_or(0),
        }),
        (None, Some(offset)) => Some(LineSpec {
            base: Address::Current,
            offset,
        }),
        (None, None) => None,
    };
    Ok((spec, rest))
}

/// Text split at its first unescaped `delim` into the pattern before it and
/// what comes after, `None` when there's no closing delimiter — a `/pat/`
/// address, or a search's `/pat/e` offset. Between `?`s a `\?` is a plain
/// `?`, as in Vim.
pub fn split_pattern(text: &str, delim: char) -> (String, Option<&str>) {
    let mut pattern = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == delim {
            return (pattern, Some(&text[i + c.len_utf8()..]));
        }
        if c != '\\' {
            pattern.push(c);
            continue;
        }
        match chars.next() {
            Some((_, '?')) if delim == '?' => pattern.push('?'),
            Some((_, n)) => {
                pattern.push('\\');
                pattern.push(n);
            }
            None => pattern.push('\\'),
        }
    }
    (pattern, None)
}

/// The flags after `:s/pat/repl/`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubFlags {
    /// `g` — every match on a line, not only the first.
    pub global: bool,
    /// `i` / `I` — ignore case, or don't, whatever the pattern says.
    pub ignore_case: Option<bool>,
    /// `n` — count the matches and change nothing.
    pub count_only: bool,
    /// `c` — ask before each replacement.
    pub confirm: bool,
}

/// `:s` / `:S` arguments — `/pat/repl/flags`, with any delimiter Vim
/// allows in place of `/`. `None` when they aren't that shape at all, an
/// error when the flags are wrong. With no replacement the match is
/// deleted.
fn parse_substitute_args(args: &str) -> Option<Result<(String, String, SubFlags), String>> {
    let delim = args.chars().next()?;
    if delim.is_alphanumeric() || matches!(delim, ' ' | '\\' | '"' | '|') {
        return None;
    }
    let body = &args[delim.len_utf8()..];
    let mut fields = vec![String::new()];
    let mut flags = "";
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped || c != delim {
            escaped = !escaped && c == '\\';
            let last = fields.len() - 1;
            fields[last].push(c);
        } else if fields.len() == 2 {
            flags = &body[i + c.len_utf8()..];
            break;
        } else {
            fields.push(String::new());
        }
    }
    let mut fields = fields.into_iter();
    let pattern = fields.next().unwrap_or_default();
    let replacement = fields.next().unwrap_or_default();
    Some(parse_sub_flags(flags.trim()).map(|flags| (pattern, replacement, flags)))
}

fn parse_sub_flags(text: &str) -> Result<SubFlags, String> {
    let mut flags = SubFlags::default();
    for c in text.chars() {
        match c {
            'g' => flags.global = true,
            'i' => flags.ignore_case = Some(true),
            'I' => flags.ignore_case = Some(false),
            'n' => flags.count_only = true,
            'c' => flags.confirm = true,
            // A pattern that isn't found is reported, never an error, so
            // there's nothing for `e` to hold back — but `:bufdo %s/a/b/ge`
            // has to read.
            'e' => {}
            // Once the switch to a regex; every pattern is one now.
            'r' => {}
            _ => return Err(format!("E488: Trailing characters: {text}")),
        }
    }
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_splits_at_its_delimiter_and_reads_the_flags() {
        match parse("%s/a\\/b/c/gI") {
            ExCommand::Substitute {
                pattern,
                replacement,
                flags,
                ..
            } => {
                assert_eq!(pattern, "a\\/b");
                assert_eq!(replacement, "c");
                let want = SubFlags {
                    global: true,
                    ignore_case: Some(false),
                    count_only: false,
                    confirm: false,
                };
                assert_eq!(flags, want);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse("s#x#y#n"),
            ExCommand::Substitute {
                flags: SubFlags {
                    count_only: true,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            parse("s/x"),
            ExCommand::Substitute { replacement, .. } if replacement.is_empty()
        ));
        assert!(matches!(parse("s/x/y/r"), ExCommand::Substitute { .. }));
        assert!(matches!(
            parse("S/x/y/i"),
            ExCommand::ProjectSubstitute {
                flags: SubFlags {
                    ignore_case: Some(true),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(parse("s/x/y/q"), ExCommand::Invalid(e) if e.contains("E488")));
        assert!(!matches!(parse("set"), ExCommand::Substitute { .. }));
    }

    #[test]
    fn ampersand_and_tilde_repeat_the_last_substitute() {
        assert!(matches!(
            parse("&"),
            ExCommand::RepeatSubstitute {
                range: ExRange::Implicit,
                last_search: false,
                keep_flags: false,
                ..
            }
        ));
        assert!(matches!(
            parse("%&&"),
            ExCommand::RepeatSubstitute {
                range: ExRange::Whole,
                keep_flags: true,
                ..
            }
        ));
        assert!(matches!(
            parse("~g"),
            ExCommand::RepeatSubstitute {
                last_search: true,
                flags: SubFlags { global: true, .. },
                ..
            }
        ));
        assert!(matches!(parse("&x"), ExCommand::Invalid(_)));
    }

    #[test]
    fn a_pattern_splits_at_its_delimiter() {
        let own = |p: &str| p.to_string();
        assert_eq!(split_pattern("foo", '/'), (own("foo"), None));
        assert_eq!(split_pattern("foo/e", '/'), (own("foo"), Some("e")));
        assert_eq!(split_pattern("a\\/b/", '/'), (own("a\\/b"), Some("")));
        assert_eq!(split_pattern("/e", '/'), (own(""), Some("e")));
        assert_eq!(split_pattern("a\\?b?s", '?'), (own("a?b"), Some("s")));
        assert_eq!(split_pattern("a/b", '?'), (own("a/b"), None));
    }

    #[test]
    fn ranges_take_every_kind_of_address() {
        let spec = |line: &str| split_range(line).map(|(spec, _)| spec);
        let at = |base: Address, offset: isize| LineSpec { base, offset };
        let search = |pattern: &str, backward: bool| Address::Search {
            pattern: pattern.to_string(),
            backward,
        };
        let two = |first, second, semicolon| {
            Ok(Some(RangeSpec::Addresses {
                first,
                second: Some((second, semicolon)),
            }))
        };
        let one = |first| {
            Ok(Some(RangeSpec::Addresses {
                first,
                second: None,
            }))
        };
        assert_eq!(
            spec(".,$d"),
            two(at(Address::Current, 0), at(Address::Last, 0), false)
        );
        assert_eq!(
            spec("'a;+2y"),
            two(at(Address::Mark('a'), 0), at(Address::Current, 2), true)
        );
        assert_eq!(
            spec("/foo/+1,?bar?-d"),
            two(
                at(search("foo", false), 1),
                at(search("bar", true), -1),
                false
            )
        );
        assert_eq!(spec("\\?d"), one(at(search("", true), 0)));
        assert_eq!(
            spec("'<,'>s/a/b/"),
            two(at(Address::Mark('<'), 0), at(Address::Mark('>'), 0), false)
        );
        assert_eq!(
            spec(",5d"),
            two(at(Address::Current, 0), at(Address::Line(5), 0), false)
        );
        assert_eq!(spec("-3d"), one(at(Address::Current, -3)));
        assert_eq!(spec("$-1+3"), one(at(Address::Last, 2)));
        assert_eq!(spec("%d"), Ok(Some(RangeSpec::Whole)));
        assert_eq!(spec("d"), Ok(None));
        assert!(split_range("'").is_err());

        // Numbers alone still parse as plain ranges, so nothing changes there.
        assert!(matches!(
            parse("5,7d"),
            ExCommand::DeleteRange {
                range: ExRange::Lines(5, 7),
                ..
            }
        ));
        assert!(matches!(parse("'a,'bd"), ExCommand::Ranged { rest, .. } if rest == "d"));
        assert!(matches!(parse("7"), ExCommand::Goto(7)));
        assert!(matches!(parse("3,9"), ExCommand::Goto(9)));
    }

    #[test]
    fn line_commands_read_their_arguments() {
        assert!(matches!(
            parse("2,3d a 4"),
            ExCommand::DeleteRange {
                range: ExRange::Lines(2, 3),
                register: Some('a'),
                count: Some(4),
            }
        ));
        assert!(matches!(
            parse("y 3"),
            ExCommand::YankRange {
                register: None,
                count: Some(3),
                ..
            }
        ));
        assert!(matches!(
            parse("delete b"),
            ExCommand::DeleteRange {
                register: Some('b'),
                count: None,
                ..
            }
        ));
        assert!(matches!(
            parse("m0"),
            ExCommand::MoveLines {
                copy: false,
                to: LineSpec {
                    base: Address::Line(0),
                    offset: 0,
                },
                ..
            }
        ));
        assert!(matches!(
            parse("t."),
            ExCommand::MoveLines {
                copy: true,
                to: LineSpec {
                    base: Address::Current,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            parse("co $"),
            ExCommand::MoveLines {
                copy: true,
                to: LineSpec {
                    base: Address::Last,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            parse("j!"),
            ExCommand::JoinRange {
                spaces: false,
                count: None,
                ..
            }
        ));
        assert!(matches!(
            parse(">>> 2"),
            ExCommand::ShiftRange {
                right: true,
                times: 3,
                count: Some(2),
                ..
            }
        ));
        assert!(matches!(
            parse("<"),
            ExCommand::ShiftRange {
                right: false,
                times: 1,
                ..
            }
        ));
        assert!(matches!(
            parse("pu! a"),
            ExCommand::PutLines {
                above: true,
                register: Some('a'),
                ..
            }
        ));
        assert!(matches!(
            parse("ce 40"),
            ExCommand::AlignLines {
                align: Align::Center,
                width: Some(40),
                ..
            }
        ));
        assert!(matches!(
            parse("retab! 4"),
            ExCommand::Retab {
                bang: true,
                tabstop: Some(4),
                ..
            }
        ));
        assert!(matches!(parse("m"), ExCommand::Invalid(e) if e.contains("E14")));
        assert!(matches!(parse("d a b"), ExCommand::Invalid(_)));
        assert!(matches!(parse("d 0"), ExCommand::Invalid(e) if e.contains("E939")));
        // Longer commands that start with the same letters are still themselves.
        assert!(matches!(parse("marks"), ExCommand::Marks));
        assert!(matches!(parse("jumps"), ExCommand::Jumps));
        assert!(matches!(parse("copilot"), ExCommand::Copilot(_)));
    }

    #[test]
    fn normal_keeps_its_keys_as_typed() {
        assert!(matches!(
            parse("%norm Ax "),
            ExCommand::Normal { range: ExRange::Whole, keys, remap: true } if keys == "Ax "
        ));
        assert!(matches!(
            parse("normal! dd"),
            ExCommand::Normal { keys, remap: false, .. } if keys == "dd"
        ));
        assert!(matches!(
            parse("'a,'bnorm x "),
            ExCommand::Ranged { rest, .. } if rest == "norm x "
        ));
        assert!(matches!(parse("norm"), ExCommand::Invalid(e) if e.contains("E471")));
        assert!(matches!(parse("noh"), ExCommand::NoHighlight));
        assert!(matches!(parse("42 "), ExCommand::Goto(42)));
    }

    #[test]
    fn global_takes_a_pattern_and_a_command() {
        assert!(matches!(
            parse("g/x/d"),
            ExCommand::Global { range: ExRange::Implicit, pattern, invert: false, cmd }
                if pattern == "x" && cmd == "d"
        ));
        assert!(matches!(
            parse("g!#a\\#b#s/a/b/"),
            ExCommand::Global { pattern, invert: true, cmd, .. }
                if pattern == "a\\#b" && cmd == "s/a/b/"
        ));
        assert!(matches!(
            parse("1,5v/x/norm A "),
            ExCommand::Global { range: ExRange::Lines(1, 5), invert: true, cmd, .. }
                if cmd == "norm A "
        ));
        assert!(matches!(parse("global/x"), ExCommand::Global { cmd, .. } if cmd.is_empty()));
        assert!(matches!(parse("g"), ExCommand::Invalid(e) if e.contains("E476")));
        assert!(matches!(parse("gblame"), ExCommand::GitBlame));
    }

    #[test]
    fn sort_reads_its_flags_and_pattern() {
        let opts = |line: &str| match parse(line) {
            ExCommand::Sort { opts, .. } => opts,
            other => panic!("{line}: {other:?}"),
        };
        assert_eq!(opts("sort"), SortOpts::default());
        let want = SortOpts {
            reverse: true,
            ignore_case: true,
            unique: true,
            ..SortOpts::default()
        };
        assert_eq!(opts("sor! iu"), want);
        let want = SortOpts {
            pattern: Some("\\d\\+".into()),
            on_match: true,
            numeric: true,
            ..SortOpts::default()
        };
        assert_eq!(opts("sort /\\d\\+/ r n"), want);
        let want = SortOpts {
            hex: true,
            pattern: Some(String::new()),
            ..SortOpts::default()
        };
        assert_eq!(opts("sort x //"), want);
        assert!(matches!(parse("sort q"), ExCommand::Invalid(e) if e.contains("E474")));
        assert!(matches!(parse("so"), ExCommand::Unknown(_)));
    }

    #[test]
    fn shell_commands_parse() {
        assert!(matches!(parse("!ls -l"), ExCommand::Shell { cmd } if cmd == "ls -l"));
        assert!(matches!(
            parse("3!sort"),
            ExCommand::Filter { range: ExRange::Single(3), cmd } if cmd == "sort"
        ));
        assert!(matches!(parse("!"), ExCommand::Invalid(e) if e.contains("E471")));
        assert!(matches!(
            parse("r notes.txt"),
            ExCommand::ReadFile { range: ExRange::Implicit, path } if path == "notes.txt"
        ));
        assert!(matches!(parse("r"), ExCommand::ReadFile { path, .. } if path.is_empty()));
        assert!(matches!(
            parse("0r !date"),
            ExCommand::ReadCommand { range: ExRange::Single(0), cmd } if cmd == "date"
        ));
        assert!(matches!(parse("r!date"), ExCommand::ReadCommand { cmd, .. } if cmd == "date"));
        assert!(matches!(
            parse("w !wc -l"),
            ExCommand::WriteCommand { range: ExRange::Implicit, cmd } if cmd == "wc -l"
        ));
        assert!(matches!(
            parse("1,3w !cat"),
            ExCommand::WriteCommand {
                range: ExRange::Lines(1, 3),
                ..
            }
        ));
        // Plain writes, and commands sharing a first letter, are as they were.
        assert!(matches!(parse("w"), ExCommand::Write));
        assert!(matches!(parse("w out.txt"), ExCommand::WriteAs(p) if p == "out.txt"));
        assert!(matches!(parse("reg"), ExCommand::Registers));
    }

    #[test]
    fn window_commands_parse() {
        assert!(matches!(
            parse("sp"),
            ExCommand::Split {
                vertical: false,
                file: None,
                empty: false,
            }
        ));
        assert!(matches!(
            parse("vsplit a.txt"),
            ExCommand::Split { vertical: true, file: Some(f), .. } if f == "a.txt"
        ));
        assert!(matches!(
            parse("new"),
            ExCommand::Split {
                vertical: false,
                file: None,
                empty: true,
            }
        ));
        assert!(matches!(
            parse("vne b.rs"),
            ExCommand::Split { vertical: true, file: Some(f), empty: true } if f == "b.rs"
        ));
        assert!(matches!(parse("on"), ExCommand::OnlyWindow));
        assert!(matches!(parse("only!"), ExCommand::OnlyWindow));
        assert!(matches!(parse("clo"), ExCommand::CloseWindow));
        // Commands that share the first letters are still themselves.
        assert!(matches!(parse("spell"), ExCommand::SpellToggle));
        assert!(matches!(parse("cl"), ExCommand::Quickfix(_)));
        assert!(matches!(parse("copilot"), ExCommand::Copilot(_)));
    }

    #[test]
    fn digraphs_parse() {
        assert!(matches!(parse("dig"), ExCommand::Digraphs));
        assert!(matches!(parse("digraphs"), ExCommand::Digraphs));
    }

    #[test]
    fn earlier_later_and_undolist_parse() {
        use std::time::Duration;
        let amount = |line: &str| match parse(line) {
            ExCommand::UndoTime { amount, .. } => Some(amount),
            _ => None,
        };
        assert_eq!(amount("earlier"), Some(UndoAmount::Steps(1)));
        assert_eq!(amount("ea 3"), Some(UndoAmount::Steps(3)));
        assert_eq!(
            amount("earlier 10s"),
            Some(UndoAmount::Time(Duration::from_secs(10)))
        );
        assert_eq!(
            amount("lat 2m"),
            Some(UndoAmount::Time(Duration::from_secs(120)))
        );
        assert_eq!(
            amount("later 1h"),
            Some(UndoAmount::Time(Duration::from_secs(3600)))
        );
        assert_eq!(amount("earlier 2f"), Some(UndoAmount::Writes(2)));
        assert_eq!(amount("earlier f"), Some(UndoAmount::Writes(1)));
        assert!(matches!(
            parse("later"),
            ExCommand::UndoTime { earlier: false, .. }
        ));
        assert!(matches!(parse("earlier 3x"), ExCommand::Invalid(e) if e.contains("E475")));
        assert!(matches!(parse("undolist"), ExCommand::UndoList));
        assert!(matches!(parse("undol"), ExCommand::UndoList));
    }

    #[test]
    fn set_parses_every_form() {
        match parse("set ic noscs hls! invws tw=72 ts:4 sw? tw list") {
            ExCommand::Set(args) => assert_eq!(
                args,
                vec![
                    SetArg::Flag("ignorecase", true),
                    SetArg::Flag("smartcase", false),
                    SetArg::Toggle("hlsearch"),
                    SetArg::Toggle("wrapscan"),
                    SetArg::Number("textwidth", 72),
                    SetArg::Number("tabstop", 4),
                    SetArg::Show("shiftwidth"),
                    SetArg::Show("textwidth"),
                    SetArg::Flag("list", true),
                ]
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse("se"), ExCommand::Set(args) if args.is_empty()));
        assert!(matches!(parse("set all"), ExCommand::Set(args) if args.is_empty()));
        assert!(matches!(parse("set bogus"), ExCommand::Invalid(e) if e.contains("E518")));
        assert!(matches!(parse("set ic=1"), ExCommand::Invalid(e) if e.contains("E474")));
        assert!(matches!(parse("set tw!"), ExCommand::Invalid(e) if e.contains("E474")));
        assert!(matches!(parse("set tw=x"), ExCommand::Invalid(e) if e.contains("E521")));
    }

    #[test]
    fn grep_vimgrep_and_make_parse() {
        assert!(matches!(
            parse("grep -w foo src"),
            ExCommand::Grep { args, jump: true } if args == "-w foo src"
        ));
        assert!(matches!(
            parse("gr! foo"),
            ExCommand::Grep { jump: false, .. }
        ));
        assert!(matches!(parse("grep"), ExCommand::Invalid(e) if e.contains("E471")));
        assert!(matches!(
            parse("vimgrep /a b/gj *.rs %"),
            ExCommand::VimGrep { pattern, files, all: true, jump: false }
                if pattern == "a b" && files == "*.rs %"
        ));
        // `\#` stays escaped, as `:s#…#` keeps it; the search reads it as `#`.
        assert!(
            matches!(
                parse("vim#a\\#b# x.rs"),
                ExCommand::VimGrep { pattern, files, all: false, jump: true }
                    if pattern == "a\\#b" && files == "x.rs"
            ),
            "{:?}",
            parse("vim#a\\#b# x.rs")
        );
        // A bare pattern takes no flags, so `generated.rs` stays a file.
        assert!(matches!(
            parse("vimgrep foo generated.rs"),
            ExCommand::VimGrep { pattern, files, .. } if pattern == "foo" && files == "generated.rs"
        ));
        assert!(matches!(parse("vimgrep /a"), ExCommand::Invalid(e) if e.contains("E683")));
        assert!(matches!(
            parse("make --release"),
            ExCommand::Make { args, jump: true } if args == "--release"
        ));
        assert!(matches!(parse("mak!"), ExCommand::Make { jump: false, .. }));
    }

    #[test]
    fn each_commands_parse() {
        assert!(matches!(
            parse("bufdo %s/a/b/ge"),
            ExCommand::Each { range: ExRange::Implicit, over: EachOver::Buffers, cmd }
                if cmd == "%s/a/b/ge"
        ));
        assert!(matches!(
            parse("2,3windo normal x"),
            ExCommand::Each {
                range: ExRange::Lines(2, 3),
                over: EachOver::Windows,
                ..
            }
        ));
        assert!(matches!(
            parse("cdo! s/a/b/"),
            ExCommand::Each { over: EachOver::Entries, cmd, .. } if cmd == "s/a/b/"
        ));
        assert!(matches!(
            parse("cfd w"),
            ExCommand::Each {
                over: EachOver::Files,
                ..
            }
        ));
        assert!(matches!(parse("bufdo"), ExCommand::Invalid(e) if e.contains("E471")));
        // `:cd` and `:win…` commands stay themselves.
        assert!(matches!(parse("cd src"), ExCommand::ChangeDir(_)));
    }

    #[test]
    fn a_range_picks_items_by_number() {
        assert_eq!(ExRange::Implicit.pick(3), Some((0, 2)));
        assert_eq!(ExRange::Whole.pick(3), Some((0, 2)));
        assert_eq!(ExRange::Single(2).pick(3), Some((1, 1)));
        assert_eq!(ExRange::Lines(3, 2).pick(3), Some((1, 2)));
        assert_eq!(ExRange::Lines(2, 9).pick(3), Some((1, 2)));
        assert_eq!(ExRange::Single(4).pick(3), None);
        assert_eq!(ExRange::Single(0).pick(3), None);
        assert_eq!(ExRange::Implicit.pick(0), None);
    }

    #[test]
    fn cd_and_pwd_parse() {
        assert!(matches!(parse("cd src"), ExCommand::ChangeDir(d) if d == "src"));
        assert!(matches!(parse("chdir"), ExCommand::ChangeDir(d) if d.is_empty()));
        assert!(matches!(parse("pwd"), ExCommand::PrintDir));
        assert!(matches!(parse("pw"), ExCommand::PrintDir));
    }

    #[test]
    fn cd_target_goes_home_back_and_expands_the_tilde() {
        use std::path::{Path, PathBuf};
        let home = Some(Path::new("/home/u"));
        let previous = Some(Path::new("/work"));
        assert_eq!(cd_target("", None, home), Ok(PathBuf::from("/home/u")));
        assert_eq!(cd_target("~", None, home), Ok(PathBuf::from("/home/u")));
        assert_eq!(
            cd_target("~/src", None, home),
            Ok(PathBuf::from("/home/u/src"))
        );
        assert_eq!(cd_target("-", previous, home), Ok(PathBuf::from("/work")));
        assert_eq!(cd_target("src", previous, home), Ok(PathBuf::from("src")));
        assert!(cd_target("-", None, home).unwrap_err().contains("E186"));
        assert!(cd_target("", None, None).unwrap_err().contains("E472"));
    }

    #[test]
    fn alternate_file_commands_parse_with_or_without_a_space() {
        for line in ["e#", "e #", "edit#"] {
            assert!(
                matches!(parse(line), ExCommand::Edit(p) if p == "#"),
                "{line}"
            );
        }
        for line in ["b#", "b #", "buffer#"] {
            assert!(
                matches!(parse(line), ExCommand::BufferSwitch(p) if p == "#"),
                "{line}"
            );
        }
    }

    #[test]
    fn write_and_quit_family() {
        assert!(matches!(parse("x"), ExCommand::WriteQuitIfModified));
        assert!(matches!(parse("wq"), ExCommand::WriteQuit));
        assert!(matches!(parse("wa"), ExCommand::WriteAll));
        assert!(matches!(parse("qa"), ExCommand::QuitAll));
        assert!(matches!(parse("qa!"), ExCommand::QuitAllForce));
        assert!(matches!(parse("wqa"), ExCommand::WriteQuitAll));
        assert!(matches!(parse("xa"), ExCommand::WriteQuitAll));
        assert!(matches!(parse("e!"), ExCommand::Revert));
        assert!(matches!(parse("e! b.txt"), ExCommand::Edit(p) if p == "b.txt"));
    }

    #[test]
    fn a_range_and_a_bang_parse_as_a_filter() {
        assert!(matches!(
            parse("5,7!sort -r"),
            ExCommand::Filter { range: ExRange::Lines(5, 7), cmd } if cmd == "sort -r"
        ));
        assert!(matches!(
            parse("3!tr a b"),
            ExCommand::Filter { range: ExRange::Single(3), cmd } if cmd == "tr a b"
        ));
        assert!(!matches!(parse("!ls"), ExCommand::Filter { .. }));
    }
}
