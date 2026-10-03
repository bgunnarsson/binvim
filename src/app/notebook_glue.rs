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
