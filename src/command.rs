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
    DeleteRange {
        range: ExRange,
    },
    /// `:{range}!cmd` — the lines through a shell command, replaced by what
    /// it prints.
    Filter {
        range: ExRange,
        cmd: String,
    },
    YankRange {
        range: ExRange,
    },
    NoHighlight,
    Format,
    Health,
    /// `:messages` — open the captured `window/showMessage` /
    /// `window/logMessage` log as a scrollable overlay.
    Messages,
    /// `:reg` / `:registers` — open a scrollable overlay listing every
    /// yank register and recorded macro register with a short preview.
    Registers,
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

pub fn parse(line: &str) -> ExCommand {
    let line = line.trim();
    if line.is_empty() {
        return ExCommand::Unknown(String::new());
    }
    // Bare line number: ":42" jumps to that line.
    if let Ok(n) = line.parse::<usize>() {
        return ExCommand::Goto(n);
    }

    // Try to peel a range prefix off the front (`%`, `N`, `N,M`).
    let (range, rest) = parse_range(line);

    // Range-only commands: shorthand for `:Nd`, `:%d`, etc.
    let rest = rest.trim();
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
    if rest == "d" || rest == "delete" {
        return ExCommand::DeleteRange { range };
    }
    // Only after a range: a bare `:!cmd` runs a command in Vim, which binvim
    // doesn't do.
    if let Some(cmd) = rest
        .strip_prefix('!')
        .filter(|_| !matches!(range, ExRange::Implicit))
    {
        return ExCommand::Filter {
            range,
            cmd: cmd.trim().to_string(),
        };
    }
    if rest == "y" || rest == "yank" {
        return ExCommand::YankRange { range };
    }

    // Anything left that opened with a range but didn't match → unknown.
    if !matches!(range, ExRange::Implicit) {
        return ExCommand::Unknown(line.to_string());
    }

    let (head, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
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
        "fmt" | "format" => ExCommand::Format,
        "health" | "checkhealth" => ExCommand::Health,
        "messages" | "message" | "mes" => ExCommand::Messages,
        "reg" | "registers" | "display" => ExCommand::Registers,
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

/// Peel an ex range prefix (`%`, `N`, `N,M`) off the front of `s`.
/// Returns the parsed range (or `Implicit`) and the remaining text.
fn parse_range(s: &str) -> (ExRange, &str) {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('%') {
        return (ExRange::Whole, rest);
    }
    let n_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if n_end == 0 {
        return (ExRange::Implicit, s);
    }
    let n: usize = s[..n_end].parse().unwrap_or(0);
    let after = &s[n_end..];
    if let Some(after_comma) = after.strip_prefix(',') {
        let m_end = after_comma
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_comma.len());
        if m_end > 0 {
            let m: usize = after_comma[..m_end].parse().unwrap_or(0);
            return (ExRange::Lines(n, m), &after_comma[m_end..]);
        }
    }
    (ExRange::Single(n), after)
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
