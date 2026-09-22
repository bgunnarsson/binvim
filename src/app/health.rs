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
use binvim::install::{BUNDLES, Tool, missing_core_tools};

/// Everything `draw_health_page` needs to paint the dashboard. Built
/// fresh per frame so the user sees live CPU / RAM / LSP-pending
/// counts; cheap enough that the per-render cost is dominated by the
/// `ps` shell-out for resource stats.
pub struct HealthSnapshot {
    pub version: &'static str,
    /// Newer published release, when the startup check found one. `None` when
    /// we're current, the check is off, or it hasn't landed yet.
    pub update_available: Option<String>,
    pub pid: u32,
    pub cwd: String,
    pub config_path: String,
    pub config_loaded: bool,
    /// `config.errors` — the startup notice names only the first.
    pub config_errors: Vec<String>,
    pub keymaps: HealthKeymaps,
    pub cpu: Option<f64>,
    pub ram_pct: Option<f64>,
    pub ram_mb: Option<f64>,
    pub buffers: Vec<HealthBuffer>,
    pub lsps: Vec<LspHealth>,
    /// Servers whose process exited on its own, as `(key, exit code,
    /// gave up)` — shown so a crashed server doesn't just silently
    /// vanish from the running list. `gave up` means the key hit its
    /// consecutive-crash cap and won't respawn this session.
    pub lsp_crashed: Vec<(String, i32, bool)>,
    pub active_buffer: Option<HealthActiveBuffer>,
    /// What the active buffer's language is missing that `i` can install.
    /// `None` when nothing is, or when nothing missing is auto-installable.
    pub setup: Option<HealthSetup>,
    pub tailwind: Option<PathBuf>,
    pub git: Option<GitStatusSummary>,
    /// On-save formatter that would run for the active buffer. `None`
    /// when no buffer is open or the extension has no formatter.
    pub formatter: Option<crate::format::FormatterStatus>,
    /// Effective `.editorconfig` settings for the active buffer plus
    /// the source files that produced them.
    pub editorconfig: HealthEditorConfig,
    /// Tree-sitter wiring status for the active buffer.
    pub tree_sitter: HealthTreeSitter,
    /// Session-restore status + recent-files count.
    pub session: HealthSession,
    /// Terminal capability bits, sampled from env at render time.
    pub terminal: HealthTerminal,
}

/// `[keymaps]` at a glance. The skipped entries are the same lines the
/// startup notice names — only the first of them fits there, and the
/// notice times out.
pub struct HealthKeymaps {
    /// `(mode, mappings)` for every mode, in table order.
    pub counts: Vec<(&'static str, usize)>,
    pub skipped: Vec<String>,
}

/// The SETUP box: the active buffer's bundle and the core tools it lacks.
/// Built from `missing_core_tools`, so it only ever offers what the
/// installer can set up unattended — the same set the first-run picker nags
/// about.
pub struct HealthSetup {
    pub bundle_idx: usize,
    pub bundle: &'static str,
    /// `(label, role)` per missing tool.
    pub missing: Vec<(&'static str, &'static str)>,
}

impl HealthSetup {
    fn from_missing(bundle_idx: usize, missing: &[&'static Tool]) -> Option<Self> {
        if missing.is_empty() {
            return None;
        }
        Some(Self {
            bundle_idx,
            bundle: BUNDLES.get(bundle_idx)?.name,
            missing: missing
                .iter()
                .map(|t| (t.label, super::installer::role_label(t.role)))
                .collect(),
        })
    }
}

pub struct HealthEditorConfig {
    pub indent: String,
    pub tab_width: usize,
    pub trim_trailing: bool,
    pub final_newline: bool,
    pub sources: Vec<PathBuf>,
}

pub struct HealthTreeSitter {
    pub language: Option<String>,
    pub highlight_cache_ready: bool,
    pub cache_byte_count: usize,
}

pub struct HealthSession {
    pub restored: bool,
    pub session_path: Option<PathBuf>,
    pub session_file_exists: bool,
    pub recents_count: usize,
}

pub struct HealthTerminal {
    pub width: u16,
    pub height: u16,
    pub term: Option<String>,
    pub colorterm: Option<String>,
    pub truecolor: bool,
    pub program: Option<String>,
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
    /// Number of cached `textDocument/documentHighlight` ranges for
    /// this buffer. 0 means "no cache" (server hasn't replied yet, or
    /// the cursor isn't on a symbol the server recognises).
    pub doc_highlights: usize,
    /// Total decoded `textDocument/semanticTokens/full` tokens for
    /// this buffer. 0 means no cache (server didn't advertise the
    /// capability, hasn't replied yet, or the buffer is empty).
    pub semantic_tokens: usize,
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
        self.health_content_height.get().saturating_sub(viewport)
    }

    /// Move the dashboard viewport by `delta` rows, clamping to
    /// `[0, health_max_scroll()]`. Negative deltas scroll up.
    pub(super) fn health_scroll_by(&mut self, delta: isize) {
        let max = self.health_max_scroll();
        let cur = self.health_scroll as isize;
        let next = (cur + delta).max(0) as usize;
        self.health_scroll = next.min(max);
    }

    /// The install offer for the active buffer. Resolved fresh rather than
    /// read off the last painted snapshot so the `i` handler doesn't pay for
    /// a whole snapshot (and its `ps` shell-out) on a keypress.
    pub(super) fn health_setup(&self) -> Option<HealthSetup> {
        let lang = self
            .buffer
            .path
            .as_deref()
            .and_then(crate::lang::Lang::detect)?;
        let bundle_idx = super::installer::bundle_for_lang(lang)?;
        HealthSetup::from_missing(bundle_idx, &missing_core_tools(bundle_idx))
    }

    /// `i` on the dashboard. Nothing to install means nothing happens —
    /// the key is only advertised while the SETUP box is showing.
    /// `open_installer` takes the dashboard down itself.
    pub(super) fn health_install(&mut self) {
        if let Some(setup) = self.health_setup() {
            self.open_installer_for_bundle(setup.bundle_idx);
        }
    }

    /// Sample every piece of state the dashboard needs. Called from
    /// the renderer per frame while the health page is showing.
    pub fn build_health_snapshot(&self) -> HealthSnapshot {
        let pid = std::process::id();
        let (cpu, ram_pct, ram_mb) = read_process_stats(pid);
        let cwd_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cwd = cwd_path.display().to_string();
        let config_path = crate::paths::config_dir()
            .map(|d| d.join("config.toml").display().to_string())
            .unwrap_or_default();
        let config_loaded = !config_path.is_empty() && std::path::Path::new(&config_path).is_file();

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
                        p.strip_prefix(&cwd_path)
                            .ok()
                            .map(|p| p.display().to_string())
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
        let lsp_crashed = self
            .lsp
            .crashed
            .iter()
            .map(|(key, code)| (key.clone(), *code, self.lsp.gave_up_on(key)))
            .collect();

        let active_buffer = self.buffer.path.as_ref().map(|p| {
            let display_path = p
                .strip_prefix(&cwd_path)
                .ok()
                .map(|rel| rel.display().to_string())
                .unwrap_or_else(|| p.display().to_string());
            let language = crate::lang::Lang::detect(p).map(|l| format!("{l:?}").to_lowercase());
            let lines = self.buffer.line_count();
            let indent = indent_label(&self.editorconfig);
            let cursor_line = self.window.cursor.line + 1;
            let cursor_col = self.window.cursor.col + 1;
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

            let doc_highlights = self
                .document_highlights
                .get(p)
                .map(|c| c.ranges.len())
                .unwrap_or(0);
            let semantic_tokens = self
                .semantic_tokens
                .get(p)
                .map(|c| c.by_line.iter().map(|row| row.len()).sum::<usize>())
                .unwrap_or(0);

            HealthActiveBuffer {
                display_path,
                language,
                lines,
                indent,
                cursor_line,
                cursor_col,
                statuses,
                diagnostics,
                doc_highlights,
                semantic_tokens,
            }
        });

        let tailwind = crate::lsp::find_tailwind_config(&cwd_path);

        let git = crate::git::status_summary(&cwd_path);

        let formatter = self
            .buffer
            .path
            .as_deref()
            .and_then(crate::format::primary_formatter_for_path);

        // Editorconfig source list — walks up from the active buffer
        // when there is one, otherwise from cwd so the user sees what
        // would apply to a fresh file in this directory.
        let ec_probe: PathBuf = self
            .buffer
            .path
            .clone()
            .unwrap_or_else(|| cwd_path.join("__binvim_probe__"));
        let ec_sources = crate::editorconfig::EditorConfig::sources(&ec_probe);
        let indent = indent_label(&self.editorconfig);
        let editorconfig = HealthEditorConfig {
            indent,
            tab_width: self.editorconfig.tab_width,
            trim_trailing: self.editorconfig.trim_trailing_whitespace,
            final_newline: self.editorconfig.insert_final_newline,
            sources: ec_sources,
        };

        let detected_lang = self
            .buffer
            .path
            .as_deref()
            .and_then(crate::lang::Lang::detect);
        let cache_matches_active = self
            .highlight_cache
            .as_ref()
            .map(|c| c.buffer_version == self.buffer.version)
            .unwrap_or(false);
        let cache_byte_count = self
            .highlight_cache
            .as_ref()
            .map(|c| c.byte_colors.len())
            .unwrap_or(0);
        let tree_sitter = HealthTreeSitter {
            language: detected_lang.map(|l| format!("{l:?}").to_lowercase()),
            highlight_cache_ready: cache_matches_active,
            cache_byte_count,
        };

        let session_path = crate::session::session_path(&cwd_path);
        let session_file_exists = session_path.as_ref().map(|p| p.is_file()).unwrap_or(false);
        let session = HealthSession {
            restored: self.session_restored,
            session_path,
            session_file_exists,
            recents_count: self.recents.len(),
        };

        let colorterm = std::env::var("COLORTERM").ok();
        let truecolor = colorterm
            .as_deref()
            .map(|s| matches!(s, "truecolor" | "24bit"))
            .unwrap_or(false);
        let terminal = HealthTerminal {
            width: self.width,
            height: self.height,
            term: std::env::var("TERM").ok(),
            colorterm,
            truecolor,
            program: std::env::var("TERM_PROGRAM").ok(),
        };

        let keymaps = HealthKeymaps {
            counts: self.config.keymaps.counts(),
            skipped: self.config.keymaps.errors.clone(),
        };

        HealthSnapshot {
            version: env!("CARGO_PKG_VERSION"),
            update_available: self.update.available.clone(),
            pid,
            cwd,
            config_path,
            config_loaded,
            config_errors: self.config.errors.clone(),
            keymaps,
            cpu,
            ram_pct,
            ram_mb,
            buffers,
            lsps,
            lsp_crashed,
            active_buffer,
            setup: self.health_setup(),
            tailwind,
            git,
            formatter,
            editorconfig,
            tree_sitter,
            session,
            terminal,
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

/// One spelling for the effective indent, shared by the ACTIVE BUFFER and
/// EDITORCONFIG panels — the two once phrased tabs differently and drifted.
fn indent_label(cfg: &crate::editorconfig::EditorConfig) -> String {
    match cfg.indent_style {
        crate::editorconfig::IndentStyle::Spaces => format!("spaces × {}", cfg.indent_size),
        crate::editorconfig::IndentStyle::Tabs => format!("tabs (width {})", cfg.tab_width),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use binvim::install::bundle_index_by_name;
    use std::path::PathBuf;

    #[test]
    fn indent_label_names_the_width_for_both_styles() {
        let mut cfg = crate::editorconfig::EditorConfig {
            indent_style: crate::editorconfig::IndentStyle::Spaces,
            indent_size: 2,
            ..Default::default()
        };
        assert_eq!(indent_label(&cfg), "spaces × 2");
        cfg.indent_style = crate::editorconfig::IndentStyle::Tabs;
        cfg.tab_width = 8;
        assert_eq!(indent_label(&cfg), "tabs (width 8)");
    }

    #[test]
    fn nothing_missing_offers_no_setup() {
        let rust = bundle_index_by_name("Rust").unwrap();
        assert!(HealthSetup::from_missing(rust, &[]).is_none());
    }

    #[test]
    fn a_missing_tool_names_its_bundle_and_role() {
        let rust = bundle_index_by_name("Rust").unwrap();
        let lsp = BUNDLES[rust]
            .tools
            .iter()
            .find(|t| t.bin == "rust-analyzer")
            .unwrap();
        let setup = HealthSetup::from_missing(rust, &[lsp]).unwrap();
        assert_eq!(setup.bundle_idx, rust);
        assert_eq!(setup.bundle, "Rust");
        assert_eq!(setup.missing, vec![("rust-analyzer", "LSP")]);
    }

    #[test]
    fn no_name_and_bundleless_buffers_offer_no_setup() {
        let mut app = crate::app::App::new(None).expect("App::new");
        assert!(app.health_setup().is_none());
        app.buffer.path = Some(PathBuf::from("/tmp/binvim-health-test.json"));
        assert!(app.health_setup().is_none());
    }

    #[test]
    fn install_key_with_nothing_missing_leaves_the_dashboard_up() {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.cmd_health();
        app.health_install();
        assert!(app.show_health_page);
        assert!(app.installer.is_none());
    }
}
