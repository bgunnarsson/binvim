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
    /// Pick a single test to run via the integrated test runner.
    /// Items are adapter-canonical test names (e.g. `motion::tests::foo`
    /// for the cargo adapter).
    TestTarget,
    /// Pick which code lens to invoke when more than one is anchored
    /// on the cursor's line (e.g. rust-analyzer's "Run" + "Debug" pair).
    CodeLens,
    /// `z=` suggestion picker — choose one of the up-to-12 single-edit
    /// neighbours of the misspelled word under the cursor.
    SpellSuggestions,
    /// Pick a discoverable workspace task (npm script, justfile recipe,
    /// cargo alias / builtin verb, Makefile target, dotnet verb). The
    /// selected task spawns in a labelled bottom-terminal tab.
    Task,
    /// `<leader>p` step 1 — pick which dependency manifest (`.csproj`) to
    /// operate on when the workspace has more than one.
    PackageManifest,
    /// `<leader>pi` step 2 — pick an already-installed package to change its
    /// version. Opens empty (`(loading…)`) while `dotnet list package` runs.
    PackageInstalled,
    /// `<leader>ps` step 2 — free-text registry search. Typing fires a
    /// debounced `dotnet package search`; the local fuzzy filter is disabled
    /// (the server does the matching).
    PackageSearch,
    /// `<leader>p` step 3 — pick a version to install. Installed version is
    /// `marked`; `Tab` toggles prereleases; the built-in fuzzy filter narrows.
    PackageVersion,
    /// `<leader>Al` — pick a defined AVD to launch.
    AndroidAvd,
    /// `<leader>Ac` step 1 — pick a system image for the new AVD. Installed
    /// images are `marked`. Opens empty (`(loading…)`) while `sdkmanager` runs.
    AndroidSystemImage,
    /// `<leader>Ad` — running devices / emulators (`adb devices`). Selection
    /// is informational today; in a debug flow it picks the attach target.
    AndroidDevice,
    /// First-run toolchain nudge — auto-opened when a buffer's language is
    /// missing its LSP / formatter. Rows list the missing tools; accepting any
    /// opens `:install` preselected to that language's bundle. A popup (rather
    /// than a status-line notification) so a competing notice — Copilot
    /// sign-in, an LSP message — can't paint over it.
    InstallToolchain,
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
    /// Optional item index (into `items`, not `filtered`) to render in an
    /// accent colour — used by the version picker to flag the installed
    /// version. `None` for every other picker.
    pub marked: Option<usize>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // CodeActionIdx is wired up in commit 7 (code actions)
pub enum PickerPayload {
    Path(PathBuf),
    BufferIdx(usize),
    Location {
        path: PathBuf,
        line: usize,
        col: usize,
    },
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
    /// One picked test — `name` is the adapter-canonical test name
    /// (passed verbatim as the run filter), `adapter_key` ties the
    /// pick back to a specific adapter so a workspace change between
    /// open and accept is detectable.
    TestTarget {
        adapter_key: String,
        name: String,
    },
    /// Index into a separately-stored vector of pending code-lens
    /// commands on the app. Same pattern as `CodeActionIdx`: the
    /// `LspCommand` is too clumsy to carry inside the payload (it
    /// contains a `serde_json::Value` arguments list), so we stash
    /// the actual list on App and route by index.
    CodeLensIdx(usize),
    /// One spell-correction suggestion accepted by the user. `word`
    /// is the misspelled token at the cursor (kept so we can verify
    /// the cursor still sits on the right word at accept time);
    /// `replacement` is the dictionary form to substitute in.
    SpellSuggestion {
        word: String,
        replacement: String,
    },
    /// Index into `App.pending_tasks` — the picked task. The full
    /// `Task` is too heavy to embed (path + arg list), so the picker
    /// payload is just a route key. Same pattern as `CodeActionIdx`.
    TaskIdx(usize),
    /// Absolute path to a `.csproj` chosen from the package-manifest picker.
    PackageManifest(PathBuf),
    /// An installed package chosen for a version change. `installed` is its
    /// currently-resolved version, carried through so the version picker can
    /// highlight + preselect it without a second lookup.
    PackageInstalled {
        id: String,
        installed: String,
    },
    /// A package id chosen from the registry-search results, to be added fresh.
    PackageSearchHit {
        id: String,
    },
    /// A version chosen from the version picker — installed into the manifest
    /// stashed on `App.package.flow`.
    PackageVersion {
        version: String,
    },
    /// An AVD name chosen to launch (`emulator -avd <name>`).
    AndroidAvd {
        name: String,
    },
    /// A system image package chosen for AVD creation.
    AndroidSystemImage {
        pkg: String,
    },
    /// A running device / emulator serial (`adb` `-s` target).
    AndroidDevice {
        serial: String,
    },
    /// A missing-toolchain row. `bundle_idx` is the `install::BUNDLES` index
    /// for the current buffer's language; every row in the picker carries the
    /// same index (accepting any one sets up the whole language), so the
    /// payload only needs to route to the installer.
    InstallToolchain {
        bundle_idx: usize,
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
            marked: None,
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
                .filter_map(|(i, item)| fuzzy_match(&self.input, item).map(|(s, p)| (i, s, p)))
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
///
/// Uses a Needleman-Wunsch-style DP rather than greedy left-to-right matching:
/// a greedy walk binds each query char to its *first* occurrence, which scatters
/// the highlight (matching `footer` across `Features/Footer` instead of the
/// contiguous trailing `Footer`). The DP maximises the bonus total, so the
/// best-scoring alignment — and the positions it highlights — favours
/// consecutive runs at word boundaries.
fn fuzzy_match(query: &str, item: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let item_lower = item.to_lowercase();
    let i_chars: Vec<char> = item_lower.chars().collect();
    let n = i_chars.len();
    let m = q.len();
    if m > n {
        return None;
    }

    // Bonus for matching a query char at item position `idx` (base hit + boundary).
    let pos_bonus = |idx: usize| -> i64 {
        let mut b = 1; // base hit
        if idx == 0 {
            b += 4; // start of string
        } else {
            let prev = i_chars[idx - 1];
            if prev == '/' || prev == '\\' || prev == '_' || prev == '-' || prev == '.' {
                b += 5; // path separator / word boundary
            }
        }
        b
    };

    const NEG: i64 = i64::MIN / 4;
    // `prev_row[j]` = best score for matching q[0..=i] with q[i] placed at item
    // position j (NEG = unreachable). `parent[i][j]` = the item position q[i-1]
    // was matched at on that best path, for backtracking.
    let mut prev_row = vec![NEG; n];
    let mut parent: Vec<Vec<usize>> = Vec::with_capacity(m);

    for i in 0..m {
        let mut cur = vec![NEG; n];
        let mut par = vec![usize::MAX; n];
        // Running max of prev_row[k] over k < j, plus its argmax.
        let mut best_prev = NEG;
        let mut best_prev_k = usize::MAX;
        for j in 0..n {
            if i_chars[j] == q[i] {
                if i == 0 {
                    cur[j] = pos_bonus(j);
                } else {
                    let mut score = NEG;
                    let mut from = usize::MAX;
                    if best_prev > NEG {
                        score = best_prev + pos_bonus(j);
                        from = best_prev_k;
                    }
                    // Consecutive bonus when q[i-1] sat immediately before j.
                    if j > 0 && prev_row[j - 1] > NEG {
                        let consec = prev_row[j - 1] + pos_bonus(j) + 6;
                        if consec > score {
                            score = consec;
                            from = j - 1;
                        }
                    }
                    if score > NEG {
                        cur[j] = score;
                        par[j] = from;
                    }
                }
            }
            // Fold prev_row[j] into the running best for the next column (k < j+1).
            if prev_row[j] > best_prev {
                best_prev = prev_row[j];
                best_prev_k = j;
            }
        }
        parent.push(par);
        prev_row = cur;
    }

    // Pick the best end position for the final query char, then backtrack.
    let mut best = NEG;
    let mut best_j = usize::MAX;
    for j in 0..n {
        if prev_row[j] > best {
            best = prev_row[j];
            best_j = j;
        }
    }
    if best_j == usize::MAX {
        return None;
    }
    let mut positions = vec![0usize; m];
    let mut j = best_j;
    for i in (0..m).rev() {
        positions[i] = j;
        j = parent[i][j];
    }
    // Length penalty so shorter matches rank higher.
    Some((best - (n as i64 / 8), positions))
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
    let Ok(out) = output else {
        return Vec::new();
    };
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
            PickerPayload::Location {
                path,
                line: line_no,
                col: col_no,
            },
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

#[cfg(test)]
mod tests {
    use super::fuzzy_match;

    #[test]
    fn highlights_contiguous_trailing_run() {
        // Greedy matching would scatter "footer.cshtml" across the path; the DP
        // should bind it to the contiguous trailing "Footer.cshtml".
        let item = "Vettvangur.Site/Views/Partials/KH/Features/Footer/Footer.cshtml";
        let (_, pos) = fuzzy_match("footer.cshtml", item).expect("should match");
        let matched: String = pos.iter().map(|&i| item.chars().nth(i).unwrap()).collect();
        assert_eq!(matched.to_lowercase(), "footer.cshtml");
        // The run must be contiguous (consecutive char indices).
        assert!(
            pos.windows(2).all(|w| w[1] == w[0] + 1),
            "positions not contiguous: {pos:?}"
        );
        // And it must be the *last* "Footer", i.e. starts after the final '/'.
        let last_slash = item.rfind('/').unwrap();
        assert!(pos[0] > last_slash);
    }

    #[test]
    fn no_match_when_chars_missing() {
        assert!(fuzzy_match("zzz", "footer.cshtml").is_none());
    }

    #[test]
    fn empty_query_matches_with_no_positions() {
        let (score, pos) = fuzzy_match("", "anything").unwrap();
        assert_eq!(score, 0);
        assert!(pos.is_empty());
    }

    #[test]
    fn prefers_word_boundary_over_earlier_occurrence() {
        // "foo" appears mid-word in "scaffolder" and at a boundary in "/foo".
        let item = "scaffolder/foo";
        let (_, pos) = fuzzy_match("foo", item).expect("should match");
        let matched: String = pos.iter().map(|&i| item.chars().nth(i).unwrap()).collect();
        assert_eq!(matched, "foo");
        assert_eq!(pos[0], item.find("/foo").unwrap() + 1);
    }
}
