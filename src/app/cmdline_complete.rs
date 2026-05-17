//! Tab completion in `:` ex commands. Tab cycles candidates forward,
//! Shift-Tab cycles backward; any other key in the cmdline drops the
//! cached completion state so the next Tab recomputes against the
//! freshly-typed prefix.
//!
//! Three completion kinds, picked by the head of the cmdline:
//! - `:foo<Tab>` (no space yet) → command-name completion from a
//!   static list of every ex-command the parser recognises.
//! - `:e <path><Tab>` / `:w <path><Tab>` / `:edit` / `:write` →
//!   filesystem completion (directory listing of the path's parent,
//!   filtered by basename prefix; directories get a trailing `/`).
//! - `:b <name><Tab>` / `:buffer` → open-buffer completion against
//!   each buffer's basename.
//!
//! State (prefix, candidate list, current index) lives on
//! `App.cmdline_completion`; the cmdline string itself is the user-
//! visible source of truth, so re-typing after a Tab cleanly resets
//! the cycle and the next Tab re-derives candidates from the new
//! cmdline.

use std::path::PathBuf;

/// Every ex command name + alias the parser in `command.rs` accepts.
/// Sorted so command-name completion sorts deterministically across
/// platforms (the filesystem walks do their own sorting).
const COMMAND_NAMES: &[&str] = &[
    "Gblame",
    "b",
    "bd",
    "bd!",
    "bdelete",
    "bdelete!",
    "bn",
    "bnext",
    "bp",
    "bprev",
    "bprevious",
    "buffer",
    "buffers",
    "cclose",
    "cdiag",
    "cdiagnostics",
    "cfirst",
    "checkhealth",
    "claude",
    "clast",
    "cl",
    "clist",
    "cn",
    "cnext",
    "codelens",
    "codelenses",
    "codex",
    "copilot",
    "cp",
    "cprev",
    "cprevious",
    "cr",
    "crewind",
    "dap",
    "dapb",
    "dapbreak",
    "dapc",
    "dapclear",
    "dapcontinue",
    "dapi",
    "dapin",
    "dapn",
    "dapnext",
    "dapo",
    "dapout",
    "dappane",
    "dapstop",
    "dapunwatch",
    "dapuw",
    "dapw",
    "dapwatch",
    "dapwatches",
    "debug",
    "debugtest",
    "display",
    "dt",
    "e",
    "edit",
    "fmt",
    "format",
    "gblame",
    "health",
    "ls",
    "mes",
    "message",
    "messages",
    "noh",
    "nohlsearch",
    "opencode",
    "q",
    "q!",
    "quit",
    "quit!",
    "reg",
    "registers",
    "spell",
    "spelltoggle",
    "term",
    "terminal",
    "test",
    "testcancel",
    "testf",
    "testfile",
    "testl",
    "testlast",
    "testn",
    "testnearest",
    "testpick",
    "testq",
    "testr",
    "testresults",
    "tf",
    "tl",
    "tn",
    "w",
    "wq",
    "write",
    "x",
];

/// Active Tab-completion cycle. Kept on `App` between Tab presses so
/// repeated Tabs rotate through the same candidate list instead of
/// recomputing on every press. Cleared on any non-Tab key (typing,
/// Backspace, history walk, …) so the next Tab re-derives candidates
/// against the freshly-edited cmdline.
#[derive(Debug, Clone)]
pub struct CmdlineCompletion {
    /// Cmdline text up to (and including) the start of the token
    /// being completed. The cmdline post-Tab is always
    /// `prefix + matches[index]`; comparing against this on the next
    /// Tab tells us whether the user has typed in between (recompute)
    /// or just pressed Tab again (rotate).
    pub prefix: String,
    pub matches: Vec<String>,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionKind {
    Command,
    FilePath,
    Buffer,
}

/// Split `cmdline` into `(prefix, token, kind)`. Returns `None` when
/// the head doesn't have a completion handler — e.g. `:health <Tab>`
/// has nothing meaningful to offer after the command name.
fn split_for_completion(cmdline: &str) -> Option<(String, String, CompletionKind)> {
    let trimmed = cmdline.trim_start();
    let leading_ws_len = cmdline.len() - trimmed.len();
    if trimmed.is_empty() {
        return None;
    }
    let Some(space_idx) = trimmed.find(char::is_whitespace) else {
        // No space yet — the whole trimmed body is the command-name
        // token. Preserve any leading whitespace the user typed so the
        // cycle doesn't eat it.
        let prefix = cmdline[..leading_ws_len].to_string();
        let token = trimmed.to_string();
        return Some((prefix, token, CompletionKind::Command));
    };
    let head = &trimmed[..space_idx];
    let after_head_idx = leading_ws_len + space_idx;
    // Skip any extra whitespace between head and the token start so
    // `:e   foo` still anchors the token at `foo`.
    let rest = &cmdline[after_head_idx..];
    let token_offset = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
    let prefix = cmdline[..after_head_idx + token_offset].to_string();
    let token = cmdline[after_head_idx + token_offset..].to_string();
    let kind = match head {
        "e" | "edit" | "w" | "write" => CompletionKind::FilePath,
        "b" | "buffer" => CompletionKind::Buffer,
        _ => return None,
    };
    Some((prefix, token, kind))
}

fn command_candidates(token: &str) -> Vec<String> {
    COMMAND_NAMES
        .iter()
        .filter(|n| n.starts_with(token))
        .map(|n| (*n).to_string())
        .collect()
}

fn file_candidates(token: &str) -> Vec<String> {
    let (dir_part, base_prefix) = match token.rfind('/') {
        Some(i) => (&token[..=i], &token[i + 1..]),
        None => ("", token),
    };
    let read_dir = if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(dir_part)
    };
    let Ok(rd) = std::fs::read_dir(&read_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name_s) = name.to_str() else { continue };
        if !name_s.starts_with(base_prefix) {
            continue;
        }
        // Hidden files (and dirs) only surface when the user explicitly
        // started the basename with `.`; otherwise `:e <Tab>` listings
        // would be dominated by `.git/`, `.editorconfig`, etc.
        if name_s.starts_with('.') && !base_prefix.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut candidate = format!("{dir_part}{name_s}");
        if is_dir {
            candidate.push('/');
        }
        out.push(candidate);
    }
    // Directories first within the prefix match, then alphabetical
    // inside each group. Mirrors what most shell completions do.
    out.sort_by(|a, b| {
        let a_dir = a.ends_with('/');
        let b_dir = b.ends_with('/');
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        }
    });
    out
}

impl super::App {
    /// Drive Tab / Shift-Tab inside the cmdline. `backward = true`
    /// rotates the cycle in reverse; otherwise it rotates forward.
    /// Recomputes the candidate set whenever the cmdline no longer
    /// matches `prefix + matches[index]` (the user typed in between).
    pub(super) fn cmdline_tab(&mut self, backward: bool) {
        if let Some(state) = &self.cmdline_completion {
            if !state.matches.is_empty() {
                let applied = format!("{}{}", state.prefix, state.matches[state.index]);
                if applied == self.cmdline {
                    let len = state.matches.len();
                    let next = if backward {
                        (state.index + len - 1) % len
                    } else {
                        (state.index + 1) % len
                    };
                    let prefix = state.prefix.clone();
                    let matches = state.matches.clone();
                    let token = matches[next].clone();
                    self.cmdline = format!("{prefix}{token}");
                    self.cmdline_completion = Some(CmdlineCompletion {
                        prefix,
                        matches,
                        index: next,
                    });
                    return;
                }
            }
        }

        let Some((prefix, token, kind)) = split_for_completion(&self.cmdline) else {
            return;
        };
        let matches = match kind {
            CompletionKind::Command => command_candidates(&token),
            CompletionKind::FilePath => file_candidates(&token),
            CompletionKind::Buffer => self.buffer_candidates(&token),
        };
        if matches.is_empty() {
            return;
        }
        let first = matches[0].clone();
        self.cmdline = format!("{prefix}{first}");
        self.cmdline_completion = Some(CmdlineCompletion {
            prefix,
            matches,
            index: 0,
        });
    }

    /// Drop the cycle. Called on every non-Tab key inside the cmdline
    /// so the next Tab re-derives candidates against the latest text.
    pub(super) fn cmdline_completion_reset(&mut self) {
        self.cmdline_completion = None;
    }

    fn buffer_candidates(&self, token: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |path: Option<&std::path::Path>| {
            let Some(p) = path else { return };
            let basename = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if basename.is_empty() {
                return;
            }
            if !basename.starts_with(token) {
                return;
            }
            out.push(basename);
        };
        push(self.buffer.path.as_deref());
        for (i, stash) in self.buffers.iter().enumerate() {
            if i == self.active {
                continue;
            }
            push(stash.buffer.path.as_deref());
        }
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_only_no_space() {
        let (prefix, token, kind) = split_for_completion("t").unwrap();
        assert_eq!(prefix, "");
        assert_eq!(token, "t");
        assert_eq!(kind, CompletionKind::Command);
    }

    #[test]
    fn split_file_path_after_edit() {
        let (prefix, token, kind) = split_for_completion("e src/ap").unwrap();
        assert_eq!(prefix, "e ");
        assert_eq!(token, "src/ap");
        assert_eq!(kind, CompletionKind::FilePath);
    }

    #[test]
    fn split_buffer_after_b() {
        let (prefix, token, kind) = split_for_completion("b mai").unwrap();
        assert_eq!(prefix, "b ");
        assert_eq!(token, "mai");
        assert_eq!(kind, CompletionKind::Buffer);
    }

    #[test]
    fn split_returns_none_for_unknown_head() {
        assert!(split_for_completion("health foo").is_none());
    }

    #[test]
    fn split_collapses_extra_whitespace_into_prefix() {
        let (prefix, token, _) = split_for_completion("e   src").unwrap();
        assert_eq!(prefix, "e   ");
        assert_eq!(token, "src");
    }

    #[test]
    fn split_empty_token_after_head() {
        let (prefix, token, kind) = split_for_completion("e ").unwrap();
        assert_eq!(prefix, "e ");
        assert_eq!(token, "");
        assert_eq!(kind, CompletionKind::FilePath);
    }

    #[test]
    fn command_candidates_match_prefix() {
        let out = command_candidates("test");
        assert!(out.contains(&"test".to_string()));
        assert!(out.contains(&"testnearest".to_string()));
        assert!(!out.contains(&"write".to_string()));
    }

    #[test]
    fn command_candidates_empty_token_lists_everything() {
        let out = command_candidates("");
        assert!(out.len() >= COMMAND_NAMES.len());
        assert!(out.contains(&"w".to_string()));
        assert!(out.contains(&"quit".to_string()));
    }

    #[test]
    fn file_candidates_hide_dotfiles_unless_dot_typed() {
        // Run against the repo root — guaranteed to have `.git` and
        // `Cargo.toml`, so this isn't fragile against working dir.
        let visible = file_candidates("");
        assert!(visible.iter().any(|n| n == "Cargo.toml"));
        assert!(!visible.iter().any(|n| n.starts_with('.')));

        let hidden = file_candidates(".");
        assert!(hidden.iter().any(|n| n.starts_with('.')));
    }

    #[test]
    fn file_candidates_directory_marker() {
        let out = file_candidates("");
        assert!(out.iter().any(|n| n == "src/"));
    }

    #[test]
    fn file_candidates_respect_directory_prefix() {
        let out = file_candidates("src/");
        assert!(out.iter().any(|n| n == "src/app/" || n == "src/app.rs"));
        assert!(out.iter().all(|n| n.starts_with("src/")));
    }
}
