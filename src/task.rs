//! Integrated task runner. Discovers project tasks from the conventions
//! a given workspace uses (`package.json` scripts, `justfile` recipes,
//! cargo aliases + builtin verbs, `Makefile` targets, dotnet verbs) and
//! lets the user pick one to run in a labelled bottom-terminal tab. The
//! task runner is intentionally *not* the test runner — that one parses
//! per-test events and renders a structured overlay; this one just
//! spawns a command and lets the terminal pane carry the output.
//!
//! Sub-module map:
//! - [`types`]: `Task`, `TaskSource`
//! - [`specs`]: workspace walk + per-source discovery dispatch
//! - [`npm_scripts`]: `package.json` scripts (npm / pnpm / yarn picker)
//! - [`justfile`]: Justfile recipe extraction
//! - [`cargo_aliases`]: `.cargo/config.toml` aliases + builtin verbs
//! - [`makefile`]: top-level Makefile target scrape
//! - [`dotnet`]: well-known dotnet verbs against `.sln` / `.csproj`

pub mod cargo_aliases;
pub mod dotnet;
pub mod justfile;
pub mod makefile;
pub mod npm_scripts;
pub mod specs;
pub mod types;

pub use specs::discover_all;
pub use types::{Task, TaskSource};

/// `:make`'s build among `tasks`: a task named `build`, from the nearest
/// workspace when several have one; failing that, plain `make` where a
/// Makefile was found, as Vim's `:make` runs it.
pub fn build_task(tasks: Vec<Task>) -> Option<Task> {
    let depth = |task: &Task| task.cwd.components().count();
    let mut best: Option<&Task> = None;
    for task in tasks.iter().filter(|task| task.label == "build") {
        if best.is_none_or(|best| depth(task) > depth(best)) {
            best = Some(task);
        }
    }
    if let Some(best) = best {
        return Some(best.clone());
    }
    let make = tasks
        .into_iter()
        .find(|task| task.source == TaskSource::Makefile)?;
    Some(Task {
        label: "make".into(),
        args: Vec::new(),
        description: None,
        ..make
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(label: &str, source: TaskSource, cwd: &str) -> Task {
        Task {
            label: label.into(),
            source,
            cwd: std::path::PathBuf::from(cwd),
            program: source.tag().into(),
            args: vec![label.into()],
            description: None,
        }
    }

    #[test]
    fn build_task_takes_the_nearest_build_then_plain_make() {
        let tasks = vec![
            task("build", TaskSource::CargoAlias, "/w"),
            task("build", TaskSource::NpmScripts, "/w/web"),
            task("test", TaskSource::Dotnet, "/w/web/api"),
        ];
        let picked = build_task(tasks).expect("a build");
        assert_eq!(picked.source, TaskSource::NpmScripts);
        let make = build_task(vec![task("lint", TaskSource::Makefile, "/w")]).expect("make");
        assert_eq!(make.command_line(), "make");
        assert!(build_task(vec![task("test", TaskSource::Dotnet, "/w")]).is_none());
    }
}
