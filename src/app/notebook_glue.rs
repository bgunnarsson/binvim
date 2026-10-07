//! Jupyter notebooks in the editor: the cell-level operations that sit on
//! top of `notebook.rs`'s projection. A notebook buffer is ordinary text, so
//! everything here is a text edit and undoes like one.

use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{PAGE_LAYOUTS, PageLayoutCache, PageLayoutKey};
use crate::kernel::{KernelCmd, RunScope};
use crate::mode::Mode;

impl super::App {
    /// `]c` / `[c`: to the header of the `count`th cell below / above the
    /// one the cursor is in, stopping at the first or last cell.
    pub(super) fn cell_jump(&mut self, forward: bool, count: usize) {
        if !self.buffer.is_notebook() {
            self.status_msg = "not a notebook".into();
            return;
        }
        let spans = crate::notebook::cell_spans(&self.buffer.rope);
        let Some(here) = crate::notebook::span_at(&spans, self.window.cursor.line) else {
            return;
        };
        // Inside a cell, `[c` first goes to that cell's own header, the way
        // `[[` goes to the start of the section it's in.
        let at_start = self.window.cursor.line == spans[here].start();
        let target = if forward {
            (here + count).min(spans.len() - 1)
        } else if at_start {
            here.saturating_sub(count)
        } else {
            here.saturating_sub(count - 1)
        };
        let line = spans[target].start();
        if line == self.window.cursor.line {
            self.status_msg = if forward {
                "no more cells below".into()
            } else {
                "no more cells above".into()
            };
            return;
        }
        self.push_jump();
        self.window.cursor.line = line;
        self.window.cursor.col = 0;
        self.window.cursor.want_col = 0;
        self.clamp_cursor_normal();
    }

    /// `:cell …` / `<leader>n…`: one whole-cell edit, recorded as one undo
    /// step. After an add or delete the cursor starts its new line at the
    /// first column; the other edits leave it on the text it was on.
    pub(super) fn cell_edit(&mut self, edit: crate::notebook::CellEdit) {
        use crate::notebook::CellEdit;
        if !self.buffer.is_notebook() {
            self.status_msg = "not a notebook".into();
            return;
        }
        let text = self.buffer.rope.to_string();
        let (edited, line) = match crate::notebook::edit_cells(&text, self.window.cursor.line, edit)
        {
            Ok(done) => done,
            Err(msg) => {
                self.status_msg = msg;
                return;
            }
        };
        self.apply_formatted(&edited);
        self.window.cursor.line = line;
        if matches!(edit, CellEdit::Add { .. } | CellEdit::Delete) {
            self.window.cursor.col = 0;
            self.window.cursor.want_col = 0;
        }
        self.clamp_cursor_normal();
    }

    /// Give every cell header its own id before a save, as one undo step.
    /// A pasted cell repeats its source's id and a typed `# %%` has none;
    /// rewriting them here, rather than only in the JSON, keeps the buffer
    /// naming the same cells the file now does.
    pub(super) fn notebook_fix_ids(&mut self) {
        let Some(doc) = self.buffer.notebook.as_ref() else { return };
        let Some(fixed) = crate::notebook::fix_ids(&self.buffer.rope.to_string(), doc.has_ids())
        else {
            return;
        };
        self.apply_formatted(&fixed);
    }

    /// Whether the active buffer is drawn as its page.
    pub(super) fn page_shown(&self) -> bool {
        self.buffer.page_shown()
    }

    pub(super) fn page_layout(&self) -> Rc<crate::notebook_page::PageLayout> {
        let width = self.active_pane_rect().w as usize;
        self.page_layout_for(&self.buffer, self.highlight_cache.as_ref(), width)
    }

    /// The page of `buffer` at `width`, laid out again only when something
    /// it's drawn from changed.
    pub(crate) fn page_layout_for(
        &self,
        buffer: &crate::buffer::Buffer,
        highlights: Option<&crate::lang::HighlightCache>,
        width: usize,
    ) -> Rc<crate::notebook_page::PageLayout> {
        let images = self.page_images();
        let key = PageLayoutKey {
            buffer: buffer.id,
            version: buffer.version,
            rev: buffer.notebook.as_ref().map(|nb| nb.rev()),
            width,
            highlights: highlights.map(|h| (h.lang, h.buffer_version)),
            images: images.is_some(),
        };
        let mut cache = self.page_layouts.borrow_mut();
        if let Some(i) = cache.iter().position(|c| c.key == key) {
            let images_current = images.is_none_or(|store| {
                let store = store.borrow();
                cache[i]
                    .layout
                    .rows
                    .iter()
                    .filter_map(|r| r.image)
                    .all(|im| store.is_current(im.id))
            });
            if images_current {
                let hit = cache.remove(i);
                let layout = hit.layout.clone();
                cache.push(hit);
                return layout;
            }
            cache.remove(i);
        }
        let colors = highlights.map(|c| c.byte_colors.as_slice());
        let layout = Rc::new(crate::notebook_page::layout(
            buffer,
            colors,
            width,
            &self.config,
            images,
        ));
        cache.retain(|c| c.key.buffer != key.buffer || c.key.width != key.width);
        if cache.len() >= PAGE_LAYOUTS {
            cache.remove(0);
        }
        cache.push(PageLayoutCache {
            key,
            layout: layout.clone(),
        });
        layout
    }

    /// The image store when images are drawn: the terminal can, and
    /// `[notebook] images` hasn't turned them off.
    pub(crate) fn page_images(&self) -> Option<&std::cell::RefCell<crate::graphics::ImageStore>> {
        (self.graphics && self.config.notebook.images).then_some(&self.images)
    }

    /// `:notebook [page|text]` / `<leader>v` / `<leader>nv`: show a notebook or a
    /// markdown file as its page or as its text; `None` flips between them.
    pub(super) fn notebook_view(&mut self, page: Option<bool>) {
        if !self.buffer.has_page() {
            self.status_msg = "not a notebook".into();
            return;
        }
        let page = page.unwrap_or(self.buffer.text_view);
        self.buffer.text_view = !page;
        if !page {
            return;
        }
        if matches!(self.mode, Mode::Visual(_)) {
            self.exit_visual();
        }
        self.page_pending_d = false;
        let layout = self.page_layout();
        if let Some(cell) = layout.cell_of_line(self.window.cursor.line) {
            self.window.page_top = layout.reveal(cell, self.window.page_top, self.pane_rows());
        }
    }

    /// The page's own keys, ahead of the parser. On a notebook every key the
    /// page doesn't pass on is swallowed: the text it would edit isn't on
    /// screen. A markdown page hands them to its text instead.
    pub(super) fn handle_page_key(&mut self, k: KeyEvent) -> bool {
        use crate::kernel::{KernelCmd, RunScope};
        use crate::notebook::{CellEdit, CellKind};
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if k.modifiers.contains(KeyModifiers::ALT) {
            return true;
        }
        let pending_d = std::mem::take(&mut self.page_pending_d);
        let layout = self.page_layout();
        let h = self.pane_rows().max(1);
        let here = layout.cell_of_line(self.window.cursor.line);
        let last = layout.spans.len().saturating_sub(1);
        let half = (h / 2).max(1) as isize;
        let full = h.saturating_sub(2).max(1) as isize;
        let edit = |e| Some(Err(e));
        let run = |c| Some(Ok(c));
        let act: Option<Result<KernelCmd, CellEdit>> = match k.code {
            // Undo and redo change the text, so a markdown page shows it.
            KeyCode::Char('u') if !ctrl && self.buffer.is_markdown() => {
                return self.markdown_page_key(k, &layout, here);
            }
            KeyCode::Char('r') if ctrl && self.buffer.is_markdown() => {
                return self.markdown_page_key(k, &layout, here);
            }
            KeyCode::Char(':' | ' ' | 'H' | 'L' | 'u' | 'Z') if !ctrl => return false,
            KeyCode::Char('r' | 'w' | 'o' | 'i' | 'c') if ctrl => return false,
            KeyCode::Esc if self.buffer.is_markdown() => {
                self.notebook_view(Some(false));
                return true;
            }
            KeyCode::Tab | KeyCode::Esc => return false,
            KeyCode::Enter => {
                self.notebook_view(Some(false));
                return true;
            }
            KeyCode::Char('j') | KeyCode::Down if !ctrl => {
                self.page_select(&layout, here.map_or(0, |c| (c + 1).min(last)));
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up if !ctrl => {
                self.page_select(&layout, here.map_or(0, |c| c.saturating_sub(1)));
                return true;
            }
            KeyCode::Char('g') | KeyCode::Home if !ctrl => {
                self.page_select(&layout, 0);
                return true;
            }
            KeyCode::Char('G') | KeyCode::End if !ctrl => {
                self.page_select(&layout, last);
                return true;
            }
            KeyCode::Char('e') if ctrl => return self.page_scroll_in(&layout, 1),
            KeyCode::Char('y') if ctrl => return self.page_scroll_in(&layout, -1),
            KeyCode::Char('d') if ctrl => return self.page_scroll_in(&layout, half),
            KeyCode::Char('u') if ctrl => return self.page_scroll_in(&layout, -half),
            KeyCode::Char('f') if ctrl => return self.page_scroll_in(&layout, full),
            KeyCode::Char('b') if ctrl => return self.page_scroll_in(&layout, -full),
            KeyCode::PageDown => return self.page_scroll_in(&layout, full),
            KeyCode::PageUp => return self.page_scroll_in(&layout, -full),
            _ if self.buffer.is_markdown() => return self.markdown_page_key(k, &layout, here),
            _ if ctrl => None,
            KeyCode::Char('r') => run(KernelCmd::Run(RunScope::Cell)),
            KeyCode::Char('n') => run(KernelCmd::Run(RunScope::Advance)),
            KeyCode::Char('R') => run(KernelCmd::Run(RunScope::All)),
            KeyCode::Char('i') => run(KernelCmd::Interrupt),
            KeyCode::Char('0') => run(KernelCmd::Restart),
            KeyCode::Char('c') => run(KernelCmd::Clear { all: false }),
            KeyCode::Char('C') => run(KernelCmd::Clear { all: true }),
            KeyCode::Char('o') => run(KernelCmd::Output),
            KeyCode::Char('a') => edit(CellEdit::Add {
                kind: CellKind::Code,
                above: false,
            }),
            KeyCode::Char('A') => edit(CellEdit::Add {
                kind: CellKind::Code,
                above: true,
            }),
            KeyCode::Char('m') => edit(CellEdit::Type(CellKind::Markdown)),
            KeyCode::Char('y') => edit(CellEdit::Type(CellKind::Code)),
            KeyCode::Char('J') => edit(CellEdit::Move { down: true }),
            KeyCode::Char('K') => edit(CellEdit::Move { down: false }),
            KeyCode::Char('d') if pending_d => edit(CellEdit::Delete),
            KeyCode::Char('d') => {
                self.page_pending_d = true;
                None
            }
            _ => None,
        };
        match act {
            Some(Ok(cmd)) => self.kernel_cmd(cmd),
            Some(Err(e)) => self.cell_edit(e),
            None => return true,
        }
        // An added or moved cell, or the next one a run advanced to, is
        // brought fully into view rather than left peeking at an edge.
        if self.page_shown() {
            let layout = self.page_layout();
            if let Some(cell) = layout.cell_of_line(self.window.cursor.line) {
                self.window.page_top = layout.reveal(cell, self.window.page_top, h);
            }
        }
        true
    }

    /// A markdown page is a preview with no commands of its own: search
    /// stays on it, and any other key shows the text and runs there, at the
    /// marked block, so editing never comes back to a page that swallows
    /// keys. Insert's keys start at the block's edge they name.
    fn markdown_page_key(
        &mut self,
        k: KeyEvent,
        layout: &crate::notebook_page::PageLayout,
        here: Option<usize>,
    ) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(k.code, KeyCode::Char('/' | '?' | 'n' | 'N')) && !ctrl {
            return false;
        }
        self.buffer.text_view = true;
        let KeyCode::Char(c @ ('i' | 'I' | 'a' | 'A' | 'o' | 'O')) = k.code else {
            return false;
        };
        if ctrl {
            return false;
        }
        if let Some(body) = here
            .and_then(|i| layout.spans.get(i))
            .map(|s| s.body.clone())
        {
            let first = matches!(c, 'i' | 'I' | 'O');
            let line = if first {
                body.start
            } else {
                body.end.saturating_sub(1).max(body.start)
            };
            self.window.cursor.line = line;
            // `a` appends after the last character, so it rests there.
            self.window.cursor.col = if first { 0 } else { self.buffer.line_len(line) };
            self.clamp_cursor_normal();
            self.window.cursor.want_col = self.window.cursor.col;
        }
        false
    }

    fn page_select(&mut self, layout: &crate::notebook_page::PageLayout, cell: usize) {
        let Some(line) = layout.cell_line(cell) else { return };
        self.window.cursor.line = line;
        self.window.cursor.col = 0;
        self.window.cursor.want_col = 0;
        self.clamp_cursor_normal();
        self.window.page_top = layout.reveal(cell, self.window.page_top, self.pane_rows());
    }

    fn page_scroll_in(&mut self, layout: &crate::notebook_page::PageLayout, delta: isize) -> bool {
        let h = self.pane_rows();
        let top = self
            .window
            .page_top
            .saturating_add_signed(delta)
            .min(layout.max_top(h));
        self.window.page_top = top;
        // A scroll that leaves the marked cell behind marks the first cell
        // still on screen, so the next frame doesn't scroll back to it.
        let here = layout.cell_of_line(self.window.cursor.line).unwrap_or(0);
        if let Some(cell) = layout.visible_cell(here, top, h, delta < 0) {
            if cell != here {
                if let Some(line) = layout.cell_line(cell) {
                    self.window.cursor.line = line;
                    self.window.cursor.col = 0;
                    self.window.cursor.want_col = 0;
                    self.clamp_cursor_normal();
                }
            }
        }
        true
    }

    /// The mouse wheel over the page.
    pub(super) fn notebook_page_scroll(&mut self, delta: isize) {
        let layout = self.page_layout();
        self.page_scroll_in(&layout, delta);
    }

    /// A click on the page at pane-local `row` / `col` opens the URL drawn
    /// there, or else marks the cell drawn there.
    pub(super) fn page_click(&mut self, row: usize, col: usize) {
        let layout = self.page_layout();
        let row = self.window.page_top + row;
        if let Some(page_row) = layout.rows.get(row) {
            // The label is a play button, a stop button while the cell runs.
            if let (Some((_, kind)), Some(cell)) = (&page_row.label, page_row.cell)
                && (1..layout.gutter.saturating_sub(1)).contains(&col)
            {
                let busy = *kind == crate::notebook_page::LabelKind::Busy;
                self.page_select(&layout, cell);
                self.kernel_cmd(if busy {
                    KernelCmd::Interrupt
                } else {
                    KernelCmd::Run(RunScope::Cell)
                });
                return;
            }
            let mut x = layout.gutter + page_row.indent;
            let link = page_row.segs.iter().find_map(|s| {
                let hit = (x..x + s.width).contains(&col);
                x += s.width;
                if hit { s.link.clone() } else { None }
            });
            if let Some(url) = link {
                self.open_url_in_browser(&url);
                return;
            }
        }
        if let Some(cell) = layout.cell_at_row(row) {
            self.page_select(&layout, cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut crate::app::App, keys: &str) {
        for c in keys.chars() {
            app.replay_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn open(name: &str) -> (std::path::PathBuf, crate::app::App) {
        let dir = crate::paths::test_scratch_dir("notebook", name);
        let path = dir.join("nb.ipynb");
        std::fs::write(
            &path,
            "{\"cells\": [{\"cell_type\": \"code\", \"execution_count\": null, \"id\": \"aa\", \"metadata\": {}, \"outputs\": [], \"source\": \"x = 1\"}], \"metadata\": {}, \"nbformat\": 4, \"nbformat_minor\": 5}\n",
        )
        .unwrap();
        let mut app = crate::app::App::new(Some(path.clone())).expect("App::new");
        // These tests drive the text; the page's tests switch it back.
        app.buffer.text_view = true;
        (dir, app)
    }

    fn key(app: &mut crate::app::App, code: KeyCode) {
        app.replay_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_page_moves_between_cells_and_enter_shows_the_text() {
        let (dir, mut app) = open("page");
        app.buffer.text_view = false;
        app.buffer
            .replace_all("# %% [markdown] id=a\n# Title\n# %% id=b\nx = 1\ny = 2\n# %% id=c\nz\n");
        app.window.cursor.line = 0;
        press(&mut app, "j");
        assert_eq!(app.window.cursor.line, 3, "to the second cell's code");
        press(&mut app, "jj");
        assert_eq!(app.window.cursor.line, 6, "and stops at the last");
        press(&mut app, "gk");
        assert_eq!(app.window.cursor.line, 1);
        // Keys that would edit text nobody can see do nothing.
        let before = app.buffer.rope.to_string();
        press(&mut app, "xp~");
        assert_eq!(app.buffer.rope.to_string(), before);
        key(&mut app, KeyCode::Enter);
        assert!(app.buffer.text_view);
        assert_eq!(
            app.window.cursor.line, 1,
            "the text opens where the page was"
        );
        press(&mut app, "jj");
        assert_eq!(app.window.cursor.line, 3, "j is a line in the text");
        // Esc out of Insert stays in the text; Esc again goes to the page.
        press(&mut app, "i");
        key(&mut app, KeyCode::Esc);
        assert!(app.buffer.text_view);
        key(&mut app, KeyCode::Esc);
        assert!(!app.buffer.text_view);
        app.apply_action(crate::parser::Action::NotebookView);
        assert!(app.buffer.text_view);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_markdown_page_hands_its_editing_keys_to_the_text() {
        let dir = crate::paths::test_scratch_dir("notebook", "markdown_page");
        let path = dir.join("notes.md");
        std::fs::write(&path, "# Title\n\nfirst para\nstill first\n\nlast\n").unwrap();
        let mut app = crate::app::App::new(Some(path)).expect("App::new");
        assert!(!app.buffer.page_shown(), "the text by default");
        press(&mut app, " v");
        assert!(app.buffer.page_shown(), "<leader>v shows the page");
        app.window.cursor.line = 0;
        press(&mut app, "j");
        assert_eq!(app.window.cursor.line, 2, "j marks the next block");
        press(&mut app, "A!");
        assert!(app.buffer.text_view, "Insert shows the text");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Esc);
        assert!(app.buffer.text_view, "Esc never goes back to the page");
        press(&mut app, "o");
        press(&mut app, "new");
        key(&mut app, KeyCode::Esc);
        assert_eq!(
            app.buffer.rope.to_string(),
            "# Title\n\nfirst para\nstill first!\nnew\n\nlast\n"
        );
        press(&mut app, " v");
        assert!(app.buffer.page_shown());
        press(&mut app, "ggI>");
        key(&mut app, KeyCode::Esc);
        assert!(app.buffer.rope.to_string().starts_with(">#"));
        // Any other key shows the text and runs there.
        press(&mut app, " v");
        press(&mut app, "Gdd");
        assert!(app.buffer.text_view);
        assert!(!app.buffer.rope.to_string().contains("last"));
        press(&mut app, " v");
        press(&mut app, "u");
        assert!(app.buffer.text_view, "undo shows what it undid");
        assert!(app.buffer.rope.to_string().contains("last"));
        press(&mut app, " v");
        key(&mut app, KeyCode::Esc);
        assert!(app.buffer.text_view, "Esc on the page leaves it");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_page_is_laid_out_again_only_when_its_buffer_changes() {
        let dir = crate::paths::test_scratch_dir("notebook", "page_cache");
        let path = dir.join("notes.md");
        std::fs::write(&path, "# Title\n\nbody\n").unwrap();
        let mut app = crate::app::App::new(Some(path)).expect("App::new");
        let first = app.page_layout();
        assert!(std::rc::Rc::ptr_eq(&first, &app.page_layout()));
        app.buffer.replace_all("# Title\n\nbody\n\nmore\n");
        let edited = app.page_layout();
        assert!(!std::rc::Rc::ptr_eq(&first, &edited));
        assert_eq!(edited.spans.len(), 3);
        // Another buffer at the same version is not the same page.
        let mut other = crate::buffer::Buffer::from_path(dir.join("notes.md")).unwrap();
        other.version = app.buffer.version;
        let width = app.active_pane_rect().w as usize;
        assert!(!std::rc::Rc::ptr_eq(
            &edited,
            &app.page_layout_for(&other, None, width)
        ));
        assert_eq!(app.page_layouts.borrow().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn page_edits_act_on_the_marked_cell_and_undo_as_text() {
        let (dir, mut app) = open("page_edit");
        app.buffer.text_view = false;
        app.buffer
            .replace_all("# %% id=a\nx = 1\n# %% id=b\ny = 2\n");
        app.window.cursor.line = 1;
        press(&mut app, "d");
        assert_eq!(
            app.buffer.rope.to_string().matches("# %%").count(),
            2,
            "one d waits"
        );
        press(&mut app, "d");
        assert_eq!(app.buffer.rope.to_string(), "# %% id=b\ny = 2\n");
        press(&mut app, "a");
        assert_eq!(app.buffer.rope.to_string().matches("# %%").count(), 2);
        assert!(
            app.window.cursor.line >= 2,
            "the mark moves to the added cell"
        );
        press(&mut app, "uu");
        assert_eq!(
            app.buffer.rope.to_string(),
            "# %% id=a\nx = 1\n# %% id=b\ny = 2\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_view_command_refuses_a_plain_buffer() {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.apply_action(crate::parser::Action::NotebookView);
        assert_eq!(app.status_msg, "not a notebook");
    }

    fn ids(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|l| crate::notebook::parse_header(l)?.id)
            .collect()
    }

    #[test]
    fn a_pasted_cell_is_given_its_own_id_on_save_and_undo_restores_it() {
        let (dir, mut app) = open("paste");
        press(&mut app, "2yyGp");
        let pasted = app.buffer.rope.to_string();
        assert_eq!(ids(&pasted), ["aa", "aa"]);
        app.save_active(false).unwrap();
        let text = app.buffer.rope.to_string();
        let buffer_ids = ids(&text);
        assert_eq!(buffer_ids[0], "aa");
        assert_ne!(buffer_ids[1], "aa");
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("nb.ipynb")).unwrap()).unwrap();
        let file_ids: Vec<&str> = json["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(file_ids, buffer_ids);
        press(&mut app, "u");
        assert_eq!(app.buffer.rope.to_string(), pasted);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bracket_c_walks_cell_headers() {
        let (dir, mut app) = open("jump");
        app.buffer
            .replace_all("# %% id=a\nx\n# %% id=b\ny\ny\n# %% id=c\nz\n");
        press(&mut app, "]c");
        assert_eq!(app.window.cursor.line, 2);
        press(&mut app, "j[c");
        assert_eq!(app.window.cursor.line, 2);
        press(&mut app, "[c");
        assert_eq!(app.window.cursor.line, 0);
        press(&mut app, "5]c");
        assert_eq!(app.window.cursor.line, 5);
        press(&mut app, "]c");
        assert_eq!(app.status_msg, "no more cells below");
        std::fs::remove_dir_all(&dir).ok();
    }

    const CELLS: &str = "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\nthere\n# %% id=c\ny\n";

    /// Run `keys` with the cursor on `line` of `CELLS`, check the text and
    /// cursor line it leaves, then that one `u` puts the text back.
    fn cell_op(name: &str, line: usize, keys: &str, want: &str, want_line: usize) {
        let (dir, mut app) = open(name);
        app.buffer.replace_all(CELLS);
        app.window.cursor.line = line;
        if let Some(cmd) = keys.strip_prefix(':') {
            app.exec_command(cmd);
        } else {
            press(&mut app, keys);
        }
        assert_eq!(app.buffer.rope.to_string(), want, "{keys}");
        assert_eq!(app.window.cursor.line, want_line, "{keys}");
        press(&mut app, "u");
        assert_eq!(app.buffer.rope.to_string(), CELLS, "{keys} then u");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cell_commands_edit_the_text_and_undo_in_one_step() {
        cell_op(
            "add",
            1,
            ":cell add markdown",
            "# %% id=a\nx = 1\n# %% [markdown]\n\n# %% [markdown] id=b\nhi\nthere\n# %% id=c\ny\n",
            3,
        );
        cell_op(
            "add-above",
            6,
            " nA",
            "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\nthere\n# %%\n\n# %% id=c\ny\n",
            6,
        );
        cell_op(
            "add-below",
            0,
            " na",
            "# %% id=a\nx = 1\n# %%\n\n# %% [markdown] id=b\nhi\nthere\n# %% id=c\ny\n",
            3,
        );
        cell_op("delete", 3, " nd", "# %% id=a\nx = 1\n# %% id=c\ny\n", 2);
        cell_op(
            "move",
            1,
            " nj",
            "# %% [markdown] id=b\nhi\nthere\n# %% id=a\nx = 1\n# %% id=c\ny\n",
            4,
        );
        cell_op(
            "move-up",
            6,
            ":cell move up",
            "# %% id=a\nx = 1\n# %% id=c\ny\n# %% [markdown] id=b\nhi\nthere\n",
            3,
        );
        cell_op(
            "to-markdown",
            1,
            " nm",
            &CELLS.replacen("# %% id=a", "# %% [markdown] id=a", 1),
            1,
        );
        cell_op(
            "to-code",
            3,
            " ny",
            &CELLS.replacen("# %% [markdown] id=b", "# %% id=b", 1),
            3,
        );
        cell_op(
            "split",
            4,
            " ns",
            "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\n# %% [markdown]\nthere\n# %% id=c\ny\n",
            5,
        );
        cell_op(
            "join",
            1,
            " nJ",
            "# %% id=a\nx = 1\nhi\nthere\n# %% id=c\ny\n",
            1,
        );
    }

    #[test]
    fn a_cell_edit_that_cannot_apply_says_why_and_changes_nothing() {
        let (dir, mut app) = open("refused");
        app.buffer.replace_all(CELLS);
        app.window.cursor.line = 6;
        press(&mut app, " nJ");
        assert_eq!(app.status_msg, "no cell below");
        assert_eq!(app.buffer.rope.to_string(), CELLS);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_added_markdown_cell_saves_with_an_id_and_no_outputs() {
        let (dir, mut app) = open("add-save");
        app.exec_command("cell add markdown");
        press(&mut app, "itext");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.save_active(false).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("nb.ipynb")).unwrap()).unwrap();
        let added = &json["cells"][1];
        assert_eq!(added["cell_type"], "markdown");
        assert_eq!(added["source"], serde_json::json!(["text"]));
        assert!(added["id"].as_str().is_some_and(|id| id != "aa"));
        assert!(added.get("outputs").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn substitute_leaves_cell_headers_alone() {
        let (dir, mut app) = open("subst");
        app.exec_command("%s/[xa]/q/g");
        assert_eq!(app.buffer.rope.to_string(), "# %% id=aa\nq = 1\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reloading_a_notebook_without_ids_drops_undo_that_would_misname_cells() {
        let dir = crate::paths::test_scratch_dir("notebook", "reload-ids");
        let path = dir.join("nb.ipynb");
        let nb = |cells: &[&str]| {
            let cells: Vec<String> = cells
                .iter()
                .map(|s| {
                    format!(
                        "{{\"cell_type\": \"markdown\", \"metadata\": {{}}, \"source\": \"{s}\"}}"
                    )
                })
                .collect();
            format!(
                "{{\"cells\": [{}], \"metadata\": {{}}, \"nbformat\": 4, \"nbformat_minor\": 4}}\n",
                cells.join(", ")
            )
        };
        std::fs::write(&path, nb(&["a", "b"])).unwrap();
        let mut app = crate::app::App::new(Some(path.clone())).expect("App::new");
        press(&mut app, "Gx");
        std::fs::write(&path, nb(&["b"])).unwrap();
        app.force_reload_from_disk().unwrap();
        let reloaded = app.buffer.rope.to_string();
        assert_eq!(reloaded, "# %% [markdown] id=~0\nb\n");
        press(&mut app, "u");
        assert_eq!(app.buffer.rope.to_string(), reloaded);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cell_commands_refuse_a_plain_buffer() {
        let dir = crate::paths::test_scratch_dir("notebook", "plain");
        let path = dir.join("x.py");
        std::fs::write(&path, "# %%\nx\n").unwrap();
        let mut app = crate::app::App::new(Some(path)).expect("App::new");
        press(&mut app, " nd");
        assert_eq!(app.status_msg, "not a notebook");
        assert_eq!(app.buffer.rope.to_string(), "# %%\nx\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_typed_header_is_given_an_id_on_save() {
        let (dir, mut app) = open("typed");
        press(&mut app, "Go# %%");
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.save_active(false).unwrap();
        let text = app.buffer.rope.to_string();
        assert_eq!(ids(&text).len(), 2, "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
