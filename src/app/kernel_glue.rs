//! Running notebook cells: one Jupyter kernel per notebook buffer, keyed by
//! its path, started on the first run. Outputs land in the buffer's
//! `NotebookDoc` overlay, not its text, so running a cell never moves the
//! text under the cursor or adds an undo step.

use std::path::{Path, PathBuf};

use crate::kernel::{Kernel, KernelCmd, KernelEvent, RunScope};

/// Events taken from one kernel per run-loop tick, so a cell printing in a
/// tight loop can't keep keys from being read.
const MAX_EVENTS_PER_TICK: usize = 500;

impl super::App {
    pub(super) fn kernel_cmd(&mut self, cmd: KernelCmd) {
        if !self.buffer.is_notebook() {
            self.status_msg = "not a notebook".into();
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "save the notebook first: its kernel starts in its directory".into();
            return;
        };
        match cmd {
            KernelCmd::Run(scope) => self.run_cells(&path, scope),
            KernelCmd::Start => {
                if self.kernels.contains_key(&path) {
                    self.status_msg = "the kernel is already running".into();
                } else {
                    self.start_kernel(&path);
                }
            }
            KernelCmd::Restart => {
                self.stop_kernel(&path);
                self.start_kernel(&path);
                self.status_msg = "restarting the kernel…".into();
            }
            KernelCmd::Interrupt => match self.kernels.get(&path) {
                Some(k) => {
                    k.interrupt();
                    self.status_msg = "interrupted".into();
                }
                None => self.status_msg = "no kernel running".into(),
            },
            KernelCmd::Stop => {
                if self.stop_kernel(&path) {
                    self.status_msg = "kernel stopped".into();
                } else {
                    self.status_msg = "no kernel running".into();
                }
            }
            KernelCmd::Clear { all } => self.clear_cell_outputs(all),
            KernelCmd::Output => self.show_cell_output(),
        }
    }

    fn start_kernel(&mut self, path: &Path) {
        let dir = path.parent().unwrap_or(Path::new("/"));
        self.kernels.insert(path.to_path_buf(), Kernel::start(dir));
        self.status_msg = "starting the kernel…".into();
    }

    /// Drop the kernel for `path`, which shuts it down, and stop counting its
    /// cells as running. False when there was none.
    fn stop_kernel(&mut self, path: &Path) -> bool {
        let had = self.kernels.remove(path).is_some();
        if let Some(doc) = self.notebook_buffer(path).and_then(|b| b.notebook.as_mut()) {
            doc.clear_busy();
        }
        had
    }

    fn run_cells(&mut self, path: &Path, scope: RunScope) {
        // A typed or pasted header has no id, or a copied one; outputs are
        // filed by id, so every cell needs its own before it runs.
        self.notebook_fix_ids();
        let text = self.buffer.rope.to_string();
        let spans = crate::notebook::cell_spans_text(&text);
        let Some(here) = crate::notebook::span_at(&spans, self.window.cursor.line) else {
            self.status_msg = "no cell here".into();
            return;
        };
        let range = match scope {
            RunScope::Cell | RunScope::Advance => here..here + 1,
            RunScope::All => 0..spans.len(),
            RunScope::Above => 0..here,
            RunScope::Below => here..spans.len(),
        };
        let cells: Vec<(String, String)> = spans[range]
            .iter()
            .filter(|s| s.kind == crate::notebook::CellKind::Code)
            .filter_map(|s| {
                let source = crate::notebook::span_source(&text, s);
                let id = s.id.clone()?;
                (!source.trim().is_empty()).then_some((id, source))
            })
            .collect();
        if cells.is_empty() && scope != RunScope::Advance {
            self.status_msg = "no code to run".into();
            return;
        }
        if !cells.is_empty() {
            if !self.kernels.contains_key(path) {
                self.start_kernel(path);
            }
            let kernel = self.kernels.get_mut(path).expect("started above");
            let doc = self
                .buffer
                .notebook
                .as_mut()
                .expect("checked by kernel_cmd");
            for (id, code) in &cells {
                doc.begin_run(id);
                kernel.execute(id, code);
            }
            self.buffer.dirty = true;
        }
        if scope == RunScope::Advance {
            match spans.get(here + 1) {
                Some(next) => {
                    self.window.cursor.line = if next.body.is_empty() {
                        next.start()
                    } else {
                        next.body.start
                    };
                    self.window.cursor.col = 0;
                    self.window.cursor.want_col = 0;
                    self.clamp_cursor_normal();
                }
                None => self.cell_edit(crate::notebook::CellEdit::Add {
                    kind: crate::notebook::CellKind::Code,
                    above: false,
                }),
            }
        }
    }

    /// The open buffer whose file is `path`, active or stashed.
    fn notebook_buffer(&mut self, path: &Path) -> Option<&mut crate::buffer::Buffer> {
        if self.buffer.path.as_deref() == Some(path) {
            return Some(&mut self.buffer);
        }
        let active = self.active;
        self.buffers
            .iter_mut()
            .enumerate()
            .find(|(i, s)| *i != active && s.buffer.path.as_deref() == Some(path))
            .map(|(_, s)| &mut s.buffer)
    }

    /// Apply what every kernel has sent since the last tick. True when
    /// anything came, so the frame is redrawn.
    pub(super) fn handle_kernel_events(&mut self) -> bool {
        let paths: Vec<PathBuf> = self.kernels.keys().cloned().collect();
        let mut any = false;
        for path in paths {
            for _ in 0..MAX_EVENTS_PER_TICK {
                let Some(ev) = self.kernels.get(&path).and_then(Kernel::try_recv) else {
                    break;
                };
                any = true;
                self.apply_kernel_event(&path, ev);
            }
        }
        any
    }

    fn apply_kernel_event(&mut self, path: &Path, ev: KernelEvent) {
        match ev {
            KernelEvent::Ready { python, version } => {
                if let Some(k) = self.kernels.get_mut(path) {
                    k.ready = true;
                }
                self.status_msg = format!("kernel ready: Python {version} ({})", python.display());
            }
            KernelEvent::Missing(env) => {
                self.stop_kernel(path);
                match env {
                    Some(py) => {
                        self.status_msg = format!(
                            "no ipykernel in {0} — install it there: {0} -m pip install ipykernel",
                            py.display()
                        );
                    }
                    None => {
                        self.status_msg =
                            "no Python with ipykernel found — :install can add it (Python)".into();
                        if let Some(idx) = binvim::install::bundle_index_by_name("Python") {
                            self.open_installer_for_bundle(idx);
                        }
                    }
                }
            }
            KernelEvent::Dead(msg) => {
                self.stop_kernel(path);
                self.status_msg = format!("kernel stopped: {msg}");
            }
            KernelEvent::Busy(_) => {}
            KernelEvent::Count(id, n) => self.with_doc(path, |doc| doc.set_count(&id, n)),
            KernelEvent::Output(id, out) => self.with_doc(path, |doc| doc.push_output(&id, out)),
            KernelEvent::Clear(id, wait) => {
                self.with_doc(path, |doc| doc.clear_outputs(&id, wait));
            }
            KernelEvent::Done(id) => {
                if let Some(k) = self.kernels.get_mut(path) {
                    k.outstanding = k.outstanding.saturating_sub(1);
                }
                if let Some(doc) = self.notebook_buffer(path).and_then(|b| b.notebook.as_mut()) {
                    doc.finish_run(&id);
                }
            }
        }
    }

    /// Change the run overlay of the notebook at `path`; the change is one
    /// the next save writes, so the buffer becomes dirty.
    fn with_doc(&mut self, path: &Path, f: impl FnOnce(&mut crate::notebook::NotebookDoc)) {
        let Some(buffer) = self.notebook_buffer(path) else { return };
        let Some(doc) = buffer.notebook.as_mut() else { return };
        f(doc);
        buffer.dirty = true;
    }

    /// True while a kernel is starting or has cells to finish, so the run
    /// loop wakes often enough to show output as it streams.
    pub(super) fn kernels_busy(&self) -> bool {
        self.kernels.values().any(|k| !k.ready || k.outstanding > 0)
    }

    fn current_cell_id(&self) -> Option<String> {
        let spans = crate::notebook::cell_spans(&self.buffer.rope);
        let here = crate::notebook::span_at(&spans, self.window.cursor.line)?;
        spans[here].id.clone()
    }

    fn clear_cell_outputs(&mut self, all: bool) {
        let ids: Vec<String> = if all {
            crate::notebook::cell_spans(&self.buffer.rope)
                .into_iter()
                .filter(|s| s.kind == crate::notebook::CellKind::Code)
                .filter_map(|s| s.id)
                .collect()
        } else {
            self.current_cell_id().into_iter().collect()
        };
        let Some(doc) = self.buffer.notebook.as_mut() else { return };
        let mut changed = false;
        for id in &ids {
            if !doc.outputs(id).is_empty() || doc.cell_info(id).is_some_and(|(c, _)| c.is_some()) {
                doc.clear_cell(id);
                changed = true;
            }
        }
        if changed {
            self.buffer.dirty = true;
        }
    }

    /// `:cell output`: the cell's whole output as a buffer, and each image
    /// in the system viewer — what the rows under a cell elide or can't
    /// draw.
    fn show_cell_output(&mut self) {
        let Some(id) = self.current_cell_id() else {
            self.status_msg = "no cell here".into();
            return;
        };
        let Some(doc) = self.buffer.notebook.as_ref() else { return };
        let outputs = doc.outputs(&id).to_vec();
        if outputs.is_empty() {
            self.status_msg = "this cell has no output".into();
            return;
        }
        let Some(dir) = crate::paths::cache_dir().map(|d| d.join("notebook-output")) else {
            self.status_msg = "no cache directory to write the output to".into();
            return;
        };
        if let Err(e) = crate::paths::create_private_dir(&dir) {
            self.status_msg = format!("{}: {e}", dir.display());
            return;
        }
        let key = self
            .buffer
            .path
            .as_deref()
            .map(crate::paths::path_key)
            .unwrap_or_default();
        let images = crate::notebook::output_images(&outputs);
        for (i, (ext, bytes)) in images.iter().enumerate() {
            let file = dir.join(format!("{key}-{id}-{i}.{ext}"));
            if std::fs::write(&file, bytes).is_ok() {
                self.open_url_in_browser(&file.to_string_lossy());
            }
        }
        let text = crate::notebook::output_text(&outputs);
        let only_images = text
            .lines()
            .all(|l| l.trim().is_empty() || l.starts_with("[image/"));
        if only_images && !images.is_empty() {
            return;
        }
        let file = dir.join(format!("{key}-{id}.txt"));
        if let Err(e) = std::fs::write(&file, text) {
            self.status_msg = format!("{}: {e}", file.display());
            return;
        }
        // A fresh copy each time: reopening an already-open buffer would show
        // the text it had when it was first opened.
        match self.open_buffer(file.clone()) {
            Ok(()) => {
                self.force_reload_from_disk();
            }
            Err(e) => self.status_msg = format!("{}: {e}", file.display()),
        }
    }
}
