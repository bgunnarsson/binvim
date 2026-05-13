//! `:health` dashboard — full-screen overlay showing version, process
//! resources, attached LSPs, Tailwind detection, and the buffer list.
//! Replaces what used to be a plain-text scratch buffer.
//!
//! Toggled by the `show_health_page` flag on `App`; dismissed by Esc /
//! `q` / `:q`. Rendering lives in `render::draw_health_page`; this file
//! owns the data structures and the snapshot builder.

use std::path::PathBuf;

use crate::lsp::{ActiveBufferLspStatus, LspHealth};

/// Everything `draw_health_page` needs to paint the dashboard. Built
/// fresh per frame so the user sees live CPU / RAM / LSP-pending
/// counts; cheap enough that the per-render cost is dominated by the
/// `ps` shell-out for resource stats.
pub struct HealthSnapshot {
    pub version: &'static str,
    pub pid: u32,
    pub cwd: String,
    pub git_branch: String,
    pub config_path: String,
    pub config_loaded: bool,
    pub cpu: Option<f64>,
    pub ram_pct: Option<f64>,
    pub ram_mb: Option<f64>,
    pub buffers: Vec<HealthBuffer>,
    pub lsps: Vec<LspHealth>,
    pub active_buffer_path: Option<PathBuf>,
    pub active_buffer_status: Vec<ActiveBufferLspStatus>,
    pub tailwind: Option<PathBuf>,
}

pub struct HealthBuffer {
    pub label: String,
    pub active: bool,
    pub dirty: bool,
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
    }

    /// Sample every piece of state the dashboard needs. Called from
    /// the renderer per frame while the health page is showing.
    pub fn build_health_snapshot(&self) -> HealthSnapshot {
        let pid = std::process::id();
        let (cpu, ram_pct, ram_mb) = read_process_stats(pid);
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        let git_branch = self.git_branch.clone().unwrap_or_else(|| "—".into());
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
                    .and_then(|p| {
                        let cwd = std::env::current_dir().ok()?;
                        p.strip_prefix(&cwd).ok().map(|p| p.display().to_string())
                    })
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

        let (active_buffer_path, active_buffer_status) = match self.buffer.path.as_ref() {
            Some(p) => (Some(p.clone()), self.lsp.active_buffer_status(p)),
            None => (None, Vec::new()),
        };

        let tailwind = {
            let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            crate::lsp::find_tailwind_config(&start)
        };

        HealthSnapshot {
            version: env!("CARGO_PKG_VERSION"),
            pid,
            cwd,
            git_branch,
            config_path,
            config_loaded,
            cpu,
            ram_pct,
            ram_mb,
            buffers,
            lsps,
            active_buffer_path,
            active_buffer_status,
            tailwind,
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
