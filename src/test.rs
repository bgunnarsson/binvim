//! Integrated test runner. Spawns an external runner (e.g. `cargo
//! test`), parses its output line-by-line in a reader thread, and
//! routes structured `TestEvent`s into the editor's main loop the
//! same way the LSP / DAP layers do. Adapter-agnostic — the registry
//! in `specs.rs` is the only place that knows about specific tools.
//!
//! Sub-module map:
//! - [`types`]: wire-side data types — `TestEvent`, `TestStatus`,
//!   `TestSummary`, etc.
//! - [`specs`]: adapter registry + workspace-root discovery
//! - [`cargo`]: `cargo test` adapter — list / run / parse
//! - [`manager`]: `TestManager` owns the active run + drain loop

pub mod cargo;
pub mod dotnet;
pub mod gotest;
pub mod manager;
pub mod pytest;
pub mod specs;
pub mod types;
pub mod vitest;

pub use manager::TestManager;
pub use specs::{TestAdapterSpec, adapter_for_workspace};
pub use types::{OutputStream, TestEvent, TestOutputRow, TestRunRequest, TestStatus, TestSummary};
// Re-exports kept for crate-internal consumers of the test module — referenced
// from `app/test_glue.rs` and `render.rs` via fully-qualified paths.
#[allow(unused_imports)]
pub use manager::TestSession;
#[allow(unused_imports)]
pub use types::{ResolvedCommand, TestFailure, TestLocation};
