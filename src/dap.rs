//! Debug Adapter Protocol client. Spawns an external adapter (e.g.
//! `netcoredbg --interpreter=vscode`), drives the initialize/launch/
//! configurationDone handshake, and routes events into the editor's main
//! loop the same way the LSP layer does. Adapter-agnostic — the registry
//! in `specs.rs` is the only place that knows about specific debuggers.
//!
//! Sub-module map:
//! - [`types`]: wire-side data types — `DapIncoming`, `DapEvent`,
//!   breakpoint / frame / variable structs
//! - [`specs`]: adapter registry, workspace-root discovery, `$PATH` lookup
//! - [`manager`]: `DapManager` — owns the active session, the user's
//!   breakpoint table, and the receiver side of the reader-thread channel
//!
//! Phase 1 ships types + registry + an inert `DapManager`. Phase 2 adds
//! the `client` and `io` submodules that actually spawn the adapter and
//! drive the protocol.

mod client;
mod io;
mod manager;
mod specs;
mod types;

pub use manager::{flat_locals_view, DapManager, DapSession, StepKind};
#[allow(unused_imports)]
pub use specs::{
    adapter_for_workspace, find_dotnet_projects, find_dotnet_workspace_root,
    find_go_main_dirs, find_python_entry_scripts, find_rust_bin_targets, find_workspace_root,
    load_launch_profiles, DapAdapterSpec, LaunchContext, LaunchProfile, PrelaunchCommand,
    RustBinTarget,
};
#[allow(unused_imports)]
pub use types::{
    Breakpoint, DapEvent, DapIncoming, DapWatch, DapWatchResult, OutputLine, Scope, SessionState,
    SourceBreakpoint, StackFrame, Variable,
};
