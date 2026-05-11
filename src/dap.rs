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

pub use manager::{flat_locals_view, DapManager, StepKind};
#[allow(unused_imports)]
pub use specs::{adapter_for_workspace, DapAdapterSpec, PrelaunchCommand};
#[allow(unused_imports)]
pub use types::{
    Breakpoint, DapEvent, DapIncoming, OutputLine, Scope, SessionState, SourceBreakpoint,
    StackFrame, Variable,
};
