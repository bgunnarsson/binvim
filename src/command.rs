#[derive(Debug, Clone)]
pub enum ExCommand {
    Write,
    WriteAs(String),
    Quit,
    QuitForce,
    WriteQuit,
    Edit(String),
    Goto(usize),
    BufferNext,
    BufferPrev,
    BufferDelete { force: bool },
    BufferList,
    BufferSwitch(String),
    Substitute {
        range: ExRange,
        pattern: String,
        replacement: String,
        global: bool,
        /// `r` flag — pattern is a regex (otherwise plain literal text).
        /// Replacement honours `$1`/`$2`/… capture references when set.
        regex: bool,
    },
    /// `:S/pat/repl/[g]` — project-wide substitute. Scans the workspace
    /// with ripgrep, applies the substitution to every matching file,
    /// saves each one. The range prefix (if any) is ignored.
    ProjectSubstitute {
        pattern: String,
        replacement: String,
        global: bool,
        regex: bool,
    },
    DeleteRange { range: ExRange },
    YankRange { range: ExRange },
    NoHighlight,
    Format,
    Health,
    /// `:messages` — open the captured `window/showMessage` /
    /// `window/logMessage` log as a scrollable overlay.
    Messages,
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
    Unknown(String),
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
#[derive(Debug, Clone, Copy)]
pub enum DebugSubCmd {
    Start,
    Stop,
    Break,
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
        if let Some((pat, repl, global, regex)) = parse_substitute_args(args) {
            return ExCommand::Substitute {
                range,
                pattern: pat,
                replacement: repl,
                global,
                regex,
            };
        }
    }
    if let Some(args) = rest.strip_prefix('S') {
        if let Some((pat, repl, global, regex)) = parse_substitute_args(args) {
            return ExCommand::ProjectSubstitute {
                pattern: pat,
                replacement: repl,
                global,
                regex,
            };
        }
    }
    if rest == "d" || rest == "delete" {
        return ExCommand::DeleteRange { range };
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
        "wq" | "x" => ExCommand::WriteQuit,
        "e" | "edit" => ExCommand::Edit(rest.to_string()),
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
        "debug" | "dap" => ExCommand::Debug(DebugSubCmd::Start),
        "dapstop" => ExCommand::Debug(DebugSubCmd::Stop),
        "dapbreak" | "dapb" => ExCommand::Debug(DebugSubCmd::Break),
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
                    Ok(n) if n >= 1 => {
                        ExCommand::DebugWatch(DebugWatchCmd::Remove(Some(n)))
                    }
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
        _ => ExCommand::Unknown(line.to_string()),
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

/// Parse `:s/old/new/flags` style args. The first char after `s` is the delimiter.
fn parse_substitute_args(args: &str) -> Option<(String, String, bool, bool)> {
    let mut chars = args.chars();
    let delim = chars.next()?;
    if delim.is_alphanumeric() {
        return None;
    }
    let mut parts: Vec<String> = vec![String::new()];
    let mut escape = false;
    for c in chars {
        if escape {
            parts.last_mut().unwrap().push(c);
            escape = false;
        } else if c == '\\' {
            escape = true;
            parts.last_mut().unwrap().push(c);
        } else if c == delim {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    if parts.len() < 2 {
        return None;
    }
    let pat = parts.remove(0);
    let repl = parts.remove(0);
    let flags = parts.into_iter().next().unwrap_or_default();
    let global = flags.contains('g');
    let regex = flags.contains('r');
    Some((pat, repl, global, regex))
}
