//! `:health` command — opens a scratch buffer with a snapshot of editor
//! state (process resources, attached buffers, running LSPs, Tailwind
//! detection). Read-only summary; user dismisses with `:bd`.

use std::path::PathBuf;

use crate::buffer::Buffer;
use crate::cursor::Cursor;

use super::state::BufferStash;

impl super::App {
    /// Build a snapshot of editor state and open it as a scratch buffer. The
    /// buffer has no path so `:w` won't write it back to disk; the user can
    /// `:bd` to dismiss it.
    pub(super) fn cmd_health(&mut self) {
        // Push an empty buffer first, switch to it, then write the report into
        // the live `self.buffer`. Building the report afterwards means the
        // buffer list reflects the post-switch state — the new `[Health]`
        // buffer shows up as active rather than the previous buffer.
        let mut buf = Buffer::empty();
        buf.display_name = Some("[Health]".into());
        let stash = BufferStash {
            buffer: buf,
            ..Default::default()
        };
        self.buffers.push(stash);
        let new_idx = self.buffers.len() - 1;
        if let Err(e) = self.switch_to(new_idx) {
            self.status_msg = format!("error: {e}");
            return;
        }
        let report = self.build_health_report();
        self.buffer.insert_at_idx(0, &report);
        self.buffer.dirty = false;
        self.buffer.version = 0;
        self.show_start_page = false;
        self.cursor = Cursor::default();
        self.view_top = 0;
        self.view_left = 0;
    }

    fn build_health_report(&self) -> String {
        let mut out = String::new();
        out.push_str("binvim — health\n");
        out.push_str("================\n\n");

        let pid = std::process::id();
        let (cpu, mem, rss_mb) = read_process_stats(pid);
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        let branch = self.git_branch.as_deref().unwrap_or("—");
        let cfg_path = std::env::var("HOME")
            .map(|h| format!("{h}/.config/binvim/config.toml"))
            .unwrap_or_default();
        let cfg_loaded = std::path::Path::new(&cfg_path).is_file();

        out.push_str(&format!("  version            {}\n", env!("CARGO_PKG_VERSION")));
        out.push_str(&format!("  pid                {pid}\n"));
        out.push_str(&format!("  cwd                {cwd}\n"));
        out.push_str(&format!("  git branch         {branch}\n"));
        out.push_str(&format!(
            "  config             {} ({})\n",
            if cfg_path.is_empty() { "—".into() } else { cfg_path.clone() },
            if cfg_loaded { "loaded" } else { "missing" }
        ));
        out.push('\n');

        // Resources — share of host CPU/RAM as reported by `ps`. RAM is a
        // percentage of total physical memory; CPU is a recent average.
        out.push_str("Resources\n");
        out.push_str(&format!(
            "  CPU: {}\n",
            cpu.map(|v| format!("{v:.1}%")).unwrap_or_else(|| "—".into())
        ));
        out.push_str(&format!(
            "  RAM: {}\n",
            match (mem, rss_mb) {
                (Some(pct), Some(mb)) => format!("{pct:.1}% - {mb:.0}MB"),
                (Some(pct), None) => format!("{pct:.1}%"),
                (None, Some(mb)) => format!("{mb:.0}MB"),
                (None, None) => "—".into(),
            }
        ));
        out.push('\n');

        // Buffers. The active slot in `self.buffers` is taken (its real
        // contents live in `self.buffer`/`self.cursor`/etc.), so reading
        // `stash.buffer.path` for that index would always show `[No Name]`.
        // Pull the live buffer for the active slot instead.
        out.push_str(&format!("Buffers ({})\n", self.buffers.len()));
        for i in 0..self.buffers.len() {
            let buf = if i == self.active {
                &self.buffer
            } else {
                &self.buffers[i].buffer
            };
            let name = buf
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .or_else(|| buf.display_name.clone())
                .unwrap_or_else(|| "[No Name]".into());
            let mut tags = Vec::new();
            if i == self.active {
                tags.push("active");
            }
            if buf.dirty {
                tags.push("dirty");
            }
            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", tags.join("] ["))
            };
            out.push_str(&format!("  • {name}{tag_str}\n"));
        }
        out.push('\n');

        // LSP
        let lsps = self.lsp.health_summary();
        out.push_str(&format!("LSP servers ({} running)\n", lsps.len()));
        if lsps.is_empty() {
            out.push_str("  (none attached)\n");
        }
        for h in &lsps {
            out.push_str(&format!(
                "  • {:<14} language={:<18} root={}\n",
                h.key, h.language_id, h.root_uri
            ));
            out.push_str(&format!(
                "      pending requests: {}\n",
                h.pending_requests
            ));
        }
        out.push('\n');

        // Active buffer — which servers should attach, whether they did,
        // and whether their binaries even exist on PATH. This is the panel
        // that explains "why isn't completion firing here?" without making
        // the user grep their PATH.
        out.push_str("Active buffer\n");
        match self.buffer.path.as_ref() {
            Some(p) => {
                out.push_str(&format!("  path:    {}\n", p.display()));
                let statuses = self.lsp.active_buffer_status(p);
                if statuses.is_empty() {
                    out.push_str("  matched: (no server specs match this extension)\n");
                } else {
                    for s in statuses {
                        let bin_note = match &s.resolved_binary {
                            Some(path) => format!("binary={path}"),
                            None => "binary=NOT INSTALLED".into(),
                        };
                        let run = if s.running { "running" } else { "not running" };
                        out.push_str(&format!(
                            "  • {:<14} ({run}) lang={:<18} {bin_note}\n",
                            s.key, s.language_id
                        ));
                    }
                }
            }
            None => out.push_str("  path:    [No Name] — save the buffer to attach an LSP\n"),
        }
        out.push('\n');

        // Tailwind config
        out.push_str("Tailwind\n");
        let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match crate::lsp::find_tailwind_config(&start) {
            Some(p) => {
                let label = if p.file_name().and_then(|s| s.to_str()) == Some("package.json") {
                    "package.json (v4 — tailwindcss in dependencies)"
                } else {
                    "tailwind.config.*"
                };
                out.push_str(&format!("  config:  {} — {label}\n", p.display()));
            }
            None => out.push_str(
                "  config:  (not found — Tailwind LSP will not attach. Add tailwind.config.* or list `tailwindcss` in package.json.)\n",
            ),
        }
        out.push('\n');

        out.push_str("Press :bd to close this view.\n");
        out
    }
}

/// Shell out to `ps` for a snapshot of the process's CPU% and memory share.
/// Both fields are best-effort — a failure just shows up as `—` in the
/// `:health` report rather than crashing the editor.
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
    // `rss` is reported in KB on macOS/Linux; convert to MB for the report.
    let rss_mb = it
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|kb| kb / 1024.0);
    (cpu, mem, rss_mb)
}
