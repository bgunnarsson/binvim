//! `DapManager` owns at most one debug session plus the user's breakpoint
//! table. Phase 1: just the data shell + breakpoint mutators — no session
//! lifecycle yet. The Phase 2 work adds spawn / handshake / step / variable
//! fetch driven by an internal `DapClient` and reader-thread channel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{DapEvent, OutputLine, SessionState, SourceBreakpoint, StackFrame};

#[derive(Default)]
#[allow(dead_code)]
pub struct DapManager {
    /// Breakpoints the user has toggled in the editor, keyed by absolute
    /// path. Persisted across sessions in memory so re-launching reuses
    /// them. The map outlives any session.
    pub breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    /// Active session, if any. `None` between launches.
    pub session: Option<DapSession>,
    /// Backlog of debug-console output captured before the panel was opened
    /// or while no UI was rendering. Drained by the renderer.
    pub output_buffer: Vec<OutputLine>,
}

#[allow(dead_code)]
pub struct DapSession {
    /// Adapter spec key — `"dotnet"`, `"go"`, … — for status display.
    pub adapter_key: String,
    /// Workspace root the session was launched against.
    pub workspace_root: PathBuf,
    /// Current high-level state.
    pub state: SessionState,
    /// Call stack of the currently-stopped thread, if any.
    pub frames: Vec<StackFrame>,
    /// Stopped thread id, if a `stopped` event has been received.
    pub current_thread: Option<u64>,
}

impl DapManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when there is a session attempting to drive (or driving) a
    /// debuggee. Used by the renderer to decide whether to show the
    /// placeholder text or the real panes.
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| !matches!(s.state, SessionState::Terminated))
            .unwrap_or(false)
    }

    /// Toggle a line breakpoint. Returns the new state (`true` if the
    /// breakpoint now exists, `false` if it was removed).
    #[allow(dead_code)]
    pub fn toggle_breakpoint(&mut self, path: &Path, line: usize) -> bool {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        if let Some(idx) = entry.iter().position(|b| b.line == line) {
            entry.remove(idx);
            if entry.is_empty() {
                self.breakpoints.remove(path);
            }
            false
        } else {
            entry.push(SourceBreakpoint {
                line,
                condition: None,
            });
            true
        }
    }

    /// True when `path` has a user-set breakpoint at `line`. Renderer-side
    /// gutter marker uses this.
    #[allow(dead_code)]
    pub fn has_breakpoint(&self, path: &Path, line: usize) -> bool {
        self.breakpoints
            .get(path)
            .map(|v| v.iter().any(|b| b.line == line))
            .unwrap_or(false)
    }

    /// Phase-1 placeholder. The real implementation drains the reader-thread
    /// channel and translates raw `DapIncoming`s into `DapEvent`s.
    #[allow(dead_code)]
    pub fn drain(&mut self) -> Vec<DapEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_breakpoint_adds_and_removes() {
        let mut m = DapManager::new();
        let p = PathBuf::from("/tmp/x.cs");
        assert!(!m.has_breakpoint(&p, 10));
        assert!(m.toggle_breakpoint(&p, 10));
        assert!(m.has_breakpoint(&p, 10));
        assert!(!m.toggle_breakpoint(&p, 10));
        assert!(!m.has_breakpoint(&p, 10));
        // Map entry pruned when the last breakpoint is removed.
        assert!(m.breakpoints.is_empty());
    }

    #[test]
    fn breakpoint_table_is_per_path() {
        let mut m = DapManager::new();
        let a = PathBuf::from("/tmp/a.cs");
        let b = PathBuf::from("/tmp/b.cs");
        m.toggle_breakpoint(&a, 5);
        m.toggle_breakpoint(&b, 5);
        assert!(m.has_breakpoint(&a, 5));
        assert!(m.has_breakpoint(&b, 5));
        assert_eq!(m.breakpoints.len(), 2);
    }

    #[test]
    fn idle_manager_is_inactive_and_drains_empty() {
        let mut m = DapManager::new();
        assert!(!m.is_active());
        assert!(m.drain().is_empty());
    }
}
