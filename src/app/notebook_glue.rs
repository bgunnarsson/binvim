//! Jupyter notebooks in the editor: the cell-level operations that sit on
//! top of `notebook.rs`'s projection. A notebook buffer is ordinary text, so
//! everything here is a text edit and undoes like one.

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
        let app = crate::app::App::new(Some(path.clone())).expect("App::new");
        (dir, app)
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
