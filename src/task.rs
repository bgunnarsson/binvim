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
// TaskSource is re-exported for the test module; a plain `cargo clippy`
// (no --tests) sees no non-test consumer and flags it, hence the allow.
#[allow(unused_imports)]
pub use types::{Task, TaskSource};
