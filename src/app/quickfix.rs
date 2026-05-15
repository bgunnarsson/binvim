//! Quickfix-list flow — populate, navigate, jump.
//!
//! The list is loaded from one of three sources:
//!  - Grep pickers (the filtered ripgrep result set at accept time)
//!  - LSP references (the filtered reference set at accept time)
//!  - Diagnostics (`:cdiag` snapshots every open buffer's diagnostics)
//!
//! Navigation pushes the previous cursor to the jumplist so `<C-o>`
//! returns where the user was before they started stepping through.

use crate::app::state::{QuickfixEntry, QuickfixState};
use crate::picker::{PickerKind, PickerPayload, PickerState};

impl super::App {
    /// `:cdiag` — replace the qf list with every diagnostic from every
    /// LSP-attached buffer, sorted Error → Warning → Info → Hint, then
    /// by path / line for stable cursor order. Diagnostics returned by
    /// LSP are 0-indexed; the qf list speaks 1-indexed Vim coordinates,
    /// so we add 1 on conversion.
    pub(super) fn qf_load_from_diagnostics(&mut self) {
        use crate::lsp::Severity;
        let mut entries: Vec<(Severity, QuickfixEntry)> = Vec::new();
        for (path, diags) in self.lsp.diagnostics.iter() {
            for d in diags {
                entries.push((
                    d.severity,
                    QuickfixEntry {
                        path: path.clone(),
                        line: d.line + 1,
                        col: d.col + 1,
                        text: format!("{:?}: {}", d.severity, d.message),
                    },
                ));
            }
        }
        if entries.is_empty() {
            self.status_msg = "No diagnostics".into();
            self.quickfix = None;
            return;
        }
        entries.sort_by(|a, b| {
            severity_rank(a.0)
                .cmp(&severity_rank(b.0))
                .then_with(|| a.1.path.cmp(&b.1.path))
                .then_with(|| a.1.line.cmp(&b.1.line))
                .then_with(|| a.1.col.cmp(&b.1.col))
        });
        let entries: Vec<QuickfixEntry> = entries.into_iter().map(|(_, e)| e).collect();
        let n = entries.len();
        self.quickfix = Some(QuickfixState {
            entries,
            current: 0,
        });
        self.status_msg = format!("Quickfix: {n} diagnostic{}", if n == 1 { "" } else { "s" });
        self.qf_jump_current();
    }

    pub(super) fn qf_next(&mut self) {
        let Some(qf) = self.quickfix.as_mut() else {
            self.status_msg = "E42: No quickfix list".into();
            return;
        };
        if qf.current + 1 >= qf.entries.len() {
            self.status_msg = "E553: No more items".into();
            return;
        }
        qf.current += 1;
        self.qf_jump_current();
    }

    pub(super) fn qf_prev(&mut self) {
        let Some(qf) = self.quickfix.as_mut() else {
            self.status_msg = "E42: No quickfix list".into();
            return;
        };
        if qf.current == 0 {
            self.status_msg = "E553: No previous items".into();
            return;
        }
        qf.current -= 1;
        self.qf_jump_current();
    }

    pub(super) fn qf_first(&mut self) {
        let Some(qf) = self.quickfix.as_mut() else {
            self.status_msg = "E42: No quickfix list".into();
            return;
        };
        qf.current = 0;
        self.qf_jump_current();
    }

    pub(super) fn qf_last(&mut self) {
        let Some(qf) = self.quickfix.as_mut() else {
            self.status_msg = "E42: No quickfix list".into();
            return;
        };
        qf.current = qf.entries.len().saturating_sub(1);
        self.qf_jump_current();
    }

    pub(super) fn qf_list(&mut self) {
        let Some(qf) = self.quickfix.as_ref() else {
            self.status_msg = "E42: No quickfix list".into();
            return;
        };
        // Print the first ~12 entries to the status line for a quick
        // peek. Full listing would need a pane; the user already gets
        // the entries one-at-a-time via `]q` / `[q`.
        let total = qf.entries.len();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let preview = qf
            .entries
            .iter()
            .take(12)
            .enumerate()
            .map(|(i, e)| {
                let p = e
                    .path
                    .strip_prefix(&cwd)
                    .unwrap_or(&e.path)
                    .display()
                    .to_string();
                let marker = if i == qf.current { ">" } else { " " };
                let snippet: String = e.text.chars().take(40).collect();
                format!("{marker}{}: {}:{}:{} {}", i + 1, p, e.line, e.col, snippet)
            })
            .collect::<Vec<_>>()
            .join("  |  ");
        let more = if total > 12 {
            format!("  …(+{})", total - 12)
        } else {
            String::new()
        };
        self.status_msg = format!("[{total} qf] {preview}{more}");
    }

    pub(super) fn qf_close(&mut self) {
        if self.quickfix.is_some() {
            self.quickfix = None;
            self.status_msg = "Quickfix cleared".into();
        }
    }

    fn qf_jump_current(&mut self) {
        let Some(qf) = self.quickfix.as_ref() else { return; };
        let Some(entry) = qf.entries.get(qf.current).cloned() else { return; };
        let total = qf.entries.len();
        let pos = qf.current + 1;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let display_path = entry
            .path
            .strip_prefix(&cwd)
            .unwrap_or(&entry.path)
            .display()
            .to_string();
        self.push_jump();
        if let Err(e) = self.open_buffer(entry.path.clone()) {
            self.status_msg = format!("error: {e}");
            return;
        }
        // Coords coming from grep / references / diagnostics are 1-indexed.
        self.window.cursor.line = entry.line.saturating_sub(1);
        self.window.cursor.col = entry.col.saturating_sub(1);
        self.window.cursor.want_col = self.window.cursor.col;
        self.clamp_cursor_normal();
        self.status_msg = format!("({pos}/{total}) {display_path}:{}:{}", entry.line, entry.col);
    }
}

/// Lift Location entries out of a Grep / References picker. Other
/// picker kinds (Files, Buffers, …) return an empty list. Exposed at
/// `pub(super)` so `picker_glue` can snapshot the list at accept time
/// without going through a `&mut self` method (which would alias the
/// `self.picker` borrow held by the caller).
pub(super) fn entries_from_picker(picker: &PickerState) -> Vec<QuickfixEntry> {
    if !matches!(picker.kind, PickerKind::Grep | PickerKind::References) {
        return Vec::new();
    }
    picker
        .filtered
        .iter()
        .filter_map(|&i| {
            let label = picker.items.get(i)?;
            let payload = picker.payloads.get(i)?;
            if let PickerPayload::Location { path, line, col } = payload {
                Some(QuickfixEntry {
                    path: path.clone(),
                    line: *line,
                    col: *col,
                    text: label.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn severity_rank(s: crate::lsp::Severity) -> u8 {
    use crate::lsp::Severity;
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
        Severity::Hint => 3,
    }
}
