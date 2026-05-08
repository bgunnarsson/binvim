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
    Substitute { range: ExRange, pattern: String, replacement: String, global: bool },
    DeleteRange { range: ExRange },
    YankRange { range: ExRange },
    NoHighlight,
    Format,
    Unknown(String),
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
        if let Some((pat, repl, global)) = parse_substitute_args(args) {
            return ExCommand::Substitute { range, pattern: pat, replacement: repl, global };
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
fn parse_substitute_args(args: &str) -> Option<(String, String, bool)> {
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
    Some((pat, repl, global))
}
