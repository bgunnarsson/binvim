use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // DocumentSymbols / WorkspaceSymbols / CodeActions are wired in upcoming commits
pub enum PickerKind {
    Files,
    Recents,
    Buffers,
    Grep,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    CodeActions,
    /// Pick which `.csproj` (or `.fsproj` / `.vbproj`) the DAP session
    /// should launch when the workspace has more than one.
    DebugProject,
    /// Pick which `launchSettings.json` profile to use when the chosen
    /// project has more than one `commandName: "Project"` profile.
    DebugProfile,
    /// Pick a launch target for a non-.NET adapter — Go main package,
    /// Python entry script, or Rust `[[bin]]`. Distinguished from
    /// `DebugProject` only so the picker title + accept hint can be
    /// adapter-flavoured.
    DebugTarget,
}

pub struct PickerState {
    #[allow(dead_code)]
    pub kind: PickerKind,
    pub title: String,
    /// All candidate items in display form (e.g. relative path, buffer name).
    pub items: Vec<String>,
    /// Original payload — for Files this is the absolute path; for Buffers the buffer index.
    pub payloads: Vec<PickerPayload>,
    pub input: String,
    /// Indices into `items`, sorted by descending score.
    pub filtered: Vec<usize>,
    /// Per-`filtered` row, the *char* indices in `items[filtered[i]]` that
    /// matched the query. Used to bold-highlight matched chars in the
    /// picker UI. Empty when `input` is empty.
    pub match_positions: Vec<Vec<usize>>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // CodeActionIdx is wired up in commit 7 (code actions)
pub enum PickerPayload {
    Path(PathBuf),
    BufferIdx(usize),
    Location { path: PathBuf, line: usize, col: usize },
    /// Index into a separately-stored vector of pending code actions on the
    /// app — the actual `WorkspaceEdit` is too heavy to carry around.
    CodeActionIdx(usize),
    /// Absolute path to a `.csproj` chosen from the DebugProject picker.
    /// Routed straight into `dap_start_session_with_project`.
    DebugProject(PathBuf),
    /// Index into `App.pending_debug_profiles`. The project context
    /// (path + profile list) was stashed when the picker opened, so the
    /// payload only needs to identify which profile in that list.
    DebugProfile(usize),
    /// Adapter-agnostic launch target — used by the Go / Python / Rust
    /// flows where there's no .NET-style two-stage project→profile
    /// picker. `path` is the package directory (Go), entry script
    /// (Python), or manifest path (Rust); `name` carries the `[[bin]]`
    /// target for Rust workspaces with multiple binaries.
    DebugTarget {
        adapter_key: String,
        path: PathBuf,
        name: Option<String>,
    },
}

impl PickerState {
    pub fn new(kind: PickerKind, title: String, items: Vec<(String, PickerPayload)>) -> Self {
        let (display, payloads): (Vec<_>, Vec<_>) = items.into_iter().unzip();
        let filtered: Vec<usize> = (0..display.len()).collect();
        Self {
            kind,
            title,
            items: display,
            payloads,
            input: String::new(),
            filtered,
            match_positions: Vec::new(),
            selected: 0,
        }
    }

    pub fn refilter(&mut self) {
        if self.input.is_empty() {
            self.filtered = (0..self.items.len()).collect();
            self.match_positions.clear();
        } else {
            let mut scored: Vec<(usize, i64, Vec<usize>)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    fuzzy_match(&self.input, item).map(|(s, p)| (i, s, p))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            self.match_positions = scored.iter().map(|(_, _, p)| p.clone()).collect();
            self.filtered = scored.into_iter().map(|(i, _, _)| i).collect();
        }
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    /// Move the selection by `delta` rows, clamping at both ends. Used for
    /// PageUp/PageDown, Ctrl-U/D, and mouse-wheel scrolling.
    pub fn move_by(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let max = (self.filtered.len() - 1) as i64;
        let new = (self.selected as i64 + delta).clamp(0, max);
        self.selected = new as usize;
    }

    pub fn current(&self) -> Option<&PickerPayload> {
        let item_idx = *self.filtered.get(self.selected)?;
        self.payloads.get(item_idx)
    }

}

/// Subsequence fuzzy match. Bonuses for consecutive runs and word-boundary hits.
/// Returns `None` if not all query chars appear in order. Returns
/// `Some((score, positions))` where `positions` is the char indices in
/// `item` where query chars matched — the renderer bolds those.
fn fuzzy_match(query: &str, item: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let item_lower = item.to_lowercase();
    let i_chars: Vec<char> = item_lower.chars().collect();
    let mut qi = 0;
    let mut score: i64 = 0;
    let mut last_idx: i64 = -2;
    let mut positions = Vec::with_capacity(q.len());
    for (idx, c) in i_chars.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            // Bonuses
            if last_idx + 1 == idx as i64 {
                score += 6; // consecutive
            }
            if idx == 0 {
                score += 4; // start of string
            } else {
                let prev = i_chars[idx - 1];
                if prev == '/' || prev == '\\' || prev == '_' || prev == '-' || prev == '.' {
                    score += 5; // path separator / word boundary
                }
            }
            score += 1; // base hit
            last_idx = idx as i64;
            positions.push(idx);
            qi += 1;
        }
    }
    if qi == q.len() {
        // Length penalty so shorter matches rank higher.
        Some((score - (i_chars.len() as i64 / 8), positions))
    } else {
        None
    }
}

/// Replace a picker's items with fresh results — used for Grep, where the candidate
/// set comes from outside (a ripgrep child process) rather than client-side filtering.
pub fn replace_items(picker: &mut PickerState, items: Vec<(String, PickerPayload)>) {
    let (display, payloads): (Vec<_>, Vec<_>) = items.into_iter().unzip();
    picker.items = display;
    picker.payloads = payloads;
    picker.filtered = (0..picker.items.len()).collect();
    picker.match_positions.clear();
    picker.selected = 0;
}

/// Run ripgrep with the given query in `cwd`. Empty query returns no results so the
/// picker shows nothing until the user has typed something to search for.
pub fn run_ripgrep(query: &str, cwd: &Path, max: usize) -> Vec<(String, PickerPayload)> {
    if query.is_empty() {
        return Vec::new();
    }
    let output = Command::new("rg")
        .arg("--vimgrep")
        .arg("--no-heading")
        .arg("--color=never")
        .arg("--smart-case")
        .arg(format!("--max-count={}", 200))
        .arg("--")
        .arg(query)
        .arg(".")
        .current_dir(cwd)
        .output();
    let Ok(out) = output else { return Vec::new(); };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut results = Vec::new();
    for line in stdout.lines() {
        if results.len() >= max {
            break;
        }
        // Format: path:line:col:text
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() != 4 {
            continue;
        }
        let rel = parts[0];
        let line_no: usize = match parts[1].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let col_no: usize = match parts[2].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let text = parts[3].trim_start();
        let display = format!("{}:{}: {}", rel, line_no, text);
        let path = cwd.join(rel);
        results.push((
            display,
            PickerPayload::Location { path, line: line_no, col: col_no },
        ));
    }
    results
}

pub fn enumerate_files(root: &std::path::Path, max: usize) -> Vec<(String, PickerPayload)> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        // Show dotfiles (.env.example, .github/, .gitignore) but never
        // descend into .git/ or node_modules/ — both flood the picker
        // (refs/pack objects, transitive deps) regardless of whether
        // the surrounding repo has them gitignored.
        .filter_entry(|e| {
            let name = e.file_name();
            name != ".git" && name != "node_modules"
        })
        .build()
        .flatten()
    {
        if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.into_path();
        let display = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.display().to_string());
        out.push((display, PickerPayload::Path(path)));
        if out.len() >= max {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
