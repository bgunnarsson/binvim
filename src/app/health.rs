//! `:health` dashboard — full-screen overlay showing version, process
//! resources, attached LSPs, Tailwind detection, and the buffer list.
//! Replaces what used to be a plain-text scratch buffer.
//!
//! Toggled by the `show_health_page` flag on `App`; dismissed by Esc /
//! `q` / `:q`. Rendering lives in `render::draw_health_page`; this file
//! owns the data structures and the snapshot builder.

use std::path::PathBuf;

use crate::git::GitStatusSummary;
use crate::lsp::{ActiveBufferLspStatus, LspHealth, Severity};

/// Everything `draw_health_page` needs to paint the dashboard. Built
/// fresh per frame so the user sees live CPU / RAM / LSP-pending
/// counts; cheap enough that the per-render cost is dominated by the
/// `ps` shell-out for resource stats.
pub struct HealthSnapshot {
    pub version: &'static str,
    pub pid: u32,
    pub cwd: String,
    pub config_path: String,
    pub config_loaded: bool,
    pub cpu: Option<f64>,
    pub ram_pct: Option<f64>,
    pub ram_mb: Option<f64>,
    pub buffers: Vec<HealthBuffer>,
    pub lsps: Vec<LspHealth>,
    pub active_buffer: Option<HealthActiveBuffer>,
    pub tailwind: Option<PathBuf>,
    pub git: Option<GitStatusSummary>,
}

pub struct HealthBuffer {
    pub label: String,
    pub active: bool,
    pub dirty: bool,
}

/// Per-buffer rollup for the ACTIVE BUFFER panel. Populated only when
/// the user has a real file open (no entry for `[No Name]`).
pub struct HealthActiveBuffer {
    pub display_path: String,
    pub language: Option<String>,
    pub lines: usize,
    pub indent: String,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub statuses: Vec<ActiveBufferLspStatus>,
    pub diagnostics: DiagnosticsCounts,
}

#[derive(Default, Clone, Copy)]
pub struct DiagnosticsCounts {
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
    pub hints: usize,
}

impl DiagnosticsCounts {
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.info + self.hints
    }
}

impl super::App {
    /// `:health` / `:checkhealth` — toggle the dashboard overlay on.
    /// Replaces the previous scratch-buffer implementation. Drawing
    /// happens in `render::draw_health_page`; this just flips the
    /// flag and the next frame paints it.
    pub(super) fn cmd_health(&mut self) {
        self.show_health_page = true;
        self.show_start_page = false;
        self.completion = None;
        self.hover = None;
        self.signature_help = None;
        self.whichkey = None;
        // Reset the refresh clock so the first auto-tick lines up one
        // full interval after the user opens the dashboard, not from
        // whenever the App was constructed.
        self.health_last_refresh = std::time::Instant::now();
        // Always open at the top, even if the user had scrolled the
        // previous session.
        self.health_scroll = 0;
    }

    /// Maximum value `health_scroll` may take given the most recently
    /// measured content height and the buffer-area viewport. Falls
    /// back to 0 before the first render measures the dashboard.
    pub(super) fn health_max_scroll(&self) -> usize {
        let rows = self.buffer_rows();
        // The footer row is reserved at the bottom; content scrolls
        // within everything above it.
        let viewport = rows.saturating_sub(1);
        self.health_content_height
            .get()
            .saturating_sub(viewport)
    }

    /// Move the dashboard viewport by `delta` rows, clamping to
    /// `[0, health_max_scroll()]`. Negative deltas scroll up.
    pub(super) fn health_scroll_by(&mut self, delta: isize) {
        let max = self.health_max_scroll();
        let cur = self.health_scroll as isize;
        let next = (cur + delta).max(0) as usize;
        self.health_scroll = next.min(max);
    }

    /// Sample every piece of state the dashboard needs. Called from
    /// the renderer per frame while the health page is showing.
    pub fn build_health_snapshot(&self) -> HealthSnapshot {
        let pid = std::process::id();
        let (cpu, ram_pct, ram_mb) = read_process_stats(pid);
        let cwd_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cwd = cwd_path.display().to_string();
        let config_path = std::env::var("HOME")
            .map(|h| format!("{h}/.config/binvim/config.toml"))
            .unwrap_or_default();
        let config_loaded = !config_path.is_empty()
            && std::path::Path::new(&config_path).is_file();

        let buffers: Vec<HealthBuffer> = (0..self.buffers.len())
            .map(|i| {
                let buf = if i == self.active {
                    &self.buffer
                } else {
                    &self.buffers[i].buffer
                };
                let label = buf
                    .path
                    .as_ref()
                    .and_then(|p| p.strip_prefix(&cwd_path).ok().map(|p| p.display().to_string()))
                    .or_else(|| buf.path.as_ref().map(|p| p.display().to_string()))
                    .or_else(|| buf.display_name.clone())
                    .unwrap_or_else(|| "[No Name]".into());
                HealthBuffer {
                    label,
                    active: i == self.active,
                    dirty: buf.dirty,
                }
            })
            .collect();

        let lsps = self.lsp.health_summary();

        let active_buffer = self.buffer.path.as_ref().map(|p| {
            let display_path = p
                .strip_prefix(&cwd_path)
                .ok()
                .map(|rel| rel.display().to_string())
                .unwrap_or_else(|| p.display().to_string());
            let language = crate::lang::Lang::detect(p).map(|l| format!("{l:?}").to_lowercase());
            let lines = self.buffer.line_count();
            let indent = match self.editorconfig.indent_style {
                crate::editorconfig::IndentStyle::Spaces => {
                    format!("spaces × {}", self.editorconfig.indent_size)
                }
                crate::editorconfig::IndentStyle::Tabs => {
                    format!("tabs (width {})", self.editorconfig.tab_width)
                }
            };
            let cursor_line = self.cursor.line + 1;
            let cursor_col = self.cursor.col + 1;
            let statuses = self.lsp.active_buffer_status(p);
            let diagnostics = self
                .lsp
                .diagnostics_for(p)
                .map(|diags| {
                    let mut c = DiagnosticsCounts::default();
                    for d in diags {
                        match d.severity {
                            Severity::Error => c.errors += 1,
                            Severity::Warning => c.warnings += 1,
                            Severity::Info => c.info += 1,
                            Severity::Hint => c.hints += 1,
                        }
                    }
                    c
                })
                .unwrap_or_default();

            HealthActiveBuffer {
                display_path,
                language,
                lines,
                indent,
                cursor_line,
                cursor_col,
                statuses,
                diagnostics,
            }
        });

        let tailwind = crate::lsp::find_tailwind_config(&cwd_path);

        let git = crate::git::status_summary(&cwd_path);

        HealthSnapshot {
            version: env!("CARGO_PKG_VERSION"),
            pid,
            cwd,
            config_path,
            config_loaded,
            cpu,
            ram_pct,
            ram_mb,
            buffers,
            lsps,
            active_buffer,
            tailwind,
            git,
        }
    }
}

/// Shell out to `ps` for a snapshot of the process's CPU% and memory share.
/// Best-effort — a failure surfaces as a `—` in the dashboard rather than
/// crashing the editor.
fn read_process_stats(pid: u32) -> (Option<f64>, Option<f64>, Option<f64>) {
    let out = std::process::Command::new("ps")
        .args(["-o", "%cpu=,%mem=,rss=", "-p", &pid.to_string()])
        .output();
    let Ok(out) = out else { return (None, None, None) };
    if !out.status.success() {
        return (None, None, None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.trim();
    let mut it = line.split_whitespace();
    let cpu = it.next().and_then(|s| s.parse::<f64>().ok());
    let mem = it.next().and_then(|s| s.parse::<f64>().ok());
    // `rss` is reported in KB on macOS/Linux; convert to MB for the dashboard.
    let rss_mb = it
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|kb| kb / 1024.0);
    (cpu, mem, rss_mb)
}
