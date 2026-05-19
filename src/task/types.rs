//! Wire-side types for the task runner. Adapter-agnostic — the discovery
//! modules in `task/*.rs` produce these structs; the orchestration layer
//! in `app/task_glue.rs` consumes them.

use std::path::PathBuf;

/// One discoverable task in the user's workspace. The runner spawns
/// `program` + `args` from `cwd`; `label` is what's shown in the picker
/// and on the tab strip after it spawns.
#[derive(Debug, Clone)]
pub struct Task {
    /// Short name (e.g. `"build"`, `"dev"`, `"lint"`). Doubles as the
    /// tab-strip label; the discovery layer is responsible for keeping
    /// these terse and free of source-prefix noise (use `source` for
    /// disambiguation in the picker, not the label).
    pub label: String,
    /// Where the task came from. Picker rows render this prefix so a
    /// user with both an npm `build` and a `just build` can tell them
    /// apart.
    pub source: TaskSource,
    /// Working directory the spawned command should run in.
    pub cwd: PathBuf,
    /// Program to invoke (`pnpm`, `just`, `cargo`, `make`, `dotnet`, …).
    pub program: String,
    /// Args to the program — typically just the task / recipe name.
    pub args: Vec<String>,
    /// Free-form one-liner shown alongside the label in the picker.
    /// For npm scripts this is the script body; for Just it's any
    /// `#`-comment preceding the recipe; for Makefile / cargo aliases
    /// it's the underlying command. `None` when nothing useful is
    /// available.
    pub description: Option<String>,
}

impl Task {
    /// Full command-line as a single shell-style string. Built once
    /// here so call sites that want to display or hand the command to
    /// a PTY don't have to re-stitch it.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }

    /// Heuristic: this task likely runs until the user kills it
    /// (`pnpm dev`, `cargo watch`, `make serve`, etc.) rather than
    /// exiting on its own. Drives the `[bg]` badge in the picker
    /// and the cautionary status-line hint on `:tasklast` so a
    /// stray re-run doesn't spawn a second dev server. Label-based
    /// because the discovery layer doesn't know the body
    /// (Justfile recipes are arbitrary shell; we'd be guessing).
    /// False positives stay annoying-but-harmless — the user can
    /// always close the extra tab.
    pub fn is_long_running(&self) -> bool {
        let label = self.label.to_ascii_lowercase();
        const HINTS: &[&str] = &["dev", "watch", "serve", "start", "preview"];
        HINTS.iter().any(|h| {
            label
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|tok| tok == *h)
        })
    }
}

/// Which adapter discovered a task. Drives the source prefix in the
/// picker (`npm`, `just`, `cargo`, `make`, `dotnet`) and gives consumers
/// a way to special-case behaviour later (e.g. quickfix-scrape compiler
/// output from `cargo build` but not from `just build`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSource {
    NpmScripts,
    Justfile,
    CargoAlias,
    Makefile,
    Dotnet,
}

impl TaskSource {
    /// Lowercase tag used as the picker row prefix.
    pub fn tag(self) -> &'static str {
        match self {
            TaskSource::NpmScripts => "npm",
            TaskSource::Justfile => "just",
            TaskSource::CargoAlias => "cargo",
            TaskSource::Makefile => "make",
            TaskSource::Dotnet => "dotnet",
        }
    }
}
