use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

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

    /// Whether this picker shows a fuzzy-search input line. Most do; the
    /// first-run toolchain picker is a fixed short list, so it renders without
    /// the `›` prompt and navigates with plain `j`/`k` instead of typing.
    pub fn searchable(&self) -> bool {
        !matches!(self.kind, PickerKind::InstallToolchain)
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

/// Per-file match cap handed to ripgrep. This does *not* bound the run — a
/// short query matches in nearly every file, so 200-per-file over a large tree
/// is still hundreds of megabytes. The `max` row cap below is what stops it.
const RG_MAX_COUNT: usize = 200;

/// Widest match line we ask ripgrep for. Bundled / minified files are single
/// multi-megabyte lines; without this one match can outweigh the entire rest
/// of the result set, and the picker can't render it anyway.
const RG_MAX_COLUMNS: usize = 200;

/// Run ripgrep for `query` in `cwd`, streaming its output and stopping the
/// moment `max` rows have been parsed. Empty query returns no results so the
/// picker shows nothing until the user has typed something to search for.
///
/// Reading incrementally is the whole point: `Command::output()` buffers the
/// child's entire stdout before returning, so a two-character query over a
/// large workspace pulls hundreds of megabytes into memory (and a lossy UTF-8
/// copy doubles it) just to keep the first `max` rows. Here the child is
/// killed as soon as the cap is hit, so cost is bounded by `max` rather than
/// by how much the query happens to match.
///
/// `child_slot` publishes the spawned child so another thread can cancel a
/// superseded query mid-scan; the lock is never held across a read. The
/// canceller only calls `kill` — reaping stays here, on the thread that owns
/// the child, so a cancel can never block on `wait`.
pub fn run_ripgrep(
    query: &str,
    cwd: &Path,
    max: usize,
    child_slot: &Mutex<Option<Child>>,
) -> Vec<(String, PickerPayload)> {
    if query.is_empty() || max == 0 {
        return Vec::new();
    }
    let spawned = Command::new("rg")
        .arg("--vimgrep")
        .arg("--no-heading")
        .arg("--color=never")
        .arg("--smart-case")
        .arg("--no-messages")
        .arg(format!("--max-count={RG_MAX_COUNT}"))
        .arg(format!("--max-columns={RG_MAX_COLUMNS}"))
        .arg("--")
        .arg(query)
        .arg(".")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return Vec::new();
    };
    // Take the pipe before publishing the child, so the reader owns stdout
    // outright and a concurrent canceller only ever touches `kill`.
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    };
    match child_slot.lock() {
        Ok(mut slot) => *slot = Some(child),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Vec::new();
        }
    }

    let mut results = Vec::new();
    for line in BufReader::new(stdout).lines() {
        // A cancel kills the child, which closes the pipe — the iterator then
        // ends (or errors) and we fall out of the loop here.
        let Ok(line) = line else { break };
        if let Some(item) = parse_vimgrep_line(&line, cwd) {
            results.push(item);
            if results.len() >= max {
                break;
            }
        }
    }
    // Kill unconditionally: on the `max` break ripgrep is still scanning, and
    // on natural EOF it's a no-op against an already-exited child. Either way
    // `wait` reaps it, so a burst of typing can't leave a trail of zombies.
    if let Ok(mut slot) = child_slot.lock()
        && let Some(mut child) = slot.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
    results
}

/// Parse one `--vimgrep` row (`path:line:col:text`) into a picker item.
/// Rows that don't split into four parts (ripgrep's own notices, stray output)
/// are skipped rather than surfaced.
fn parse_vimgrep_line(line: &str, cwd: &Path) -> Option<(String, PickerPayload)> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() != 4 {
        return None;
    }
    let rel = parts[0];
    let line_no: usize = parts[1].parse().ok()?;
    let col_no: usize = parts[2].parse().ok()?;
    let text = parts[3].trim_start();
    Some((
        format!("{rel}:{line_no}: {text}"),
        PickerPayload::Location {
            path: cwd.join(rel),
            line: line_no,
            col: col_no,
        },
    ))
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
    use super::{PickerPayload, fuzzy_match, parse_vimgrep_line, run_ripgrep};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[test]
    fn parses_vimgrep_row() {
        let (display, payload) =
            parse_vimgrep_line("src/app.rs:42:7:    let x = 1;", Path::new("/work"))
                .expect("well-formed row should parse");
        assert_eq!(display, "src/app.rs:42: let x = 1;");
        match payload {
            PickerPayload::Location { path, line, col } => {
                assert_eq!(path, PathBuf::from("/work/src/app.rs"));
                assert_eq!(line, 42);
                assert_eq!(col, 7);
            }
            _ => panic!("expected a Location payload"),
        }
    }

    #[test]
    fn keeps_colons_in_match_text() {
        // `splitn(4, ':')` matters here — a match containing colons (a Windows
        // path, a ternary, a CSS rule) must survive into the display text
        // rather than being cut at the fourth colon.
        let (display, _) = parse_vimgrep_line("a.css:1:1:color: red; width: 2px;", Path::new("/w"))
            .expect("row should parse");
        assert_eq!(display, "a.css:1: color: red; width: 2px;");
    }

    #[test]
    fn rejects_non_vimgrep_rows() {
        let cwd = Path::new("/w");
        // ripgrep notices and stray output have no line:col pair.
        assert!(parse_vimgrep_line("some plain text", cwd).is_none());
        assert!(parse_vimgrep_line("a.rs:notanumber:1:x", cwd).is_none());
        assert!(parse_vimgrep_line("a.rs:1:notanumber:x", cwd).is_none());
        assert!(parse_vimgrep_line("a.rs:1:2", cwd).is_none());
    }

    /// The freeze this cap exists to prevent: a query that matches nearly every
    /// line used to be read to completion (hundreds of MB) just to keep the
    /// first `max` rows. `run_ripgrep` must stop at `max` regardless of how
    /// much more ripgrep would have produced.
    #[test]
    fn stops_at_the_row_cap() {
        if std::process::Command::new("rg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return; // ripgrep is an optional install — nothing to assert.
        }
        let dir = std::env::temp_dir().join(format!("binvim-grep-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let body = "needle\n".repeat(5_000);
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}.txt")), &body).expect("fixture write");
        }

        let slot = Mutex::new(None);
        let items = run_ripgrep("needle", &dir, 10, &slot);
        assert_eq!(
            items.len(),
            10,
            "must stop at max, not read all 25k matches"
        );
        // The child is reaped by the same call that spawned it.
        assert!(slot.lock().unwrap().is_none(), "child should be reaped");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_query_never_spawns() {
        let slot = Mutex::new(None);
        assert!(run_ripgrep("", Path::new("."), 100, &slot).is_empty());
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn highlights_contiguous_trailing_run() {
        // Greedy matching would scatter "footer.cshtml" across the path; the DP
        // should bind it to the contiguous trailing "Footer.cshtml".
        let item = "App.Site/Views/Partials/Shared/Features/Footer/Footer.cshtml";
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
