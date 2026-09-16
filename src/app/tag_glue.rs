//! Tag jumps over a ctags `tags` file — `Ctrl-]`, `Ctrl-T`, `g]` and the
//! `:tag` family. Useful where no language server gives `gd` a definition.

use crate::picker::{PickerKind, PickerPayload, PickerState};
use crate::tag::{self, Tag, TagAddress, TagIndex};

use super::state::{TagSelect, TagStackEntry};

/// A move within the top tag stack entry's matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TagMove {
    Next(usize),
    Prev(usize),
    First,
    Last,
}

impl super::App {
    /// Every tag named `name` in the nearest `tags` file above the buffer.
    fn tag_matches(&mut self, name: &str) -> Result<Vec<Tag>, String> {
        let start = match self.buffer.path.as_deref().and_then(|p| p.parent()) {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };
        let path = tag::find_tags_file(&start).ok_or("E433: No tags file")?;
        let index = TagIndex::load(self.tag_index.take(), &path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let matches = index.matches(name);
        self.tag_index = Some(index);
        if matches.is_empty() {
            return Err(format!("E426: Tag not found: {name}"));
        }
        Ok(matches)
    }

    /// `Ctrl-]` and `:tag {name}` — to the first match, as a new stack entry.
    pub(super) fn tag_jump(&mut self, name: Option<String>) {
        let Some(name) = name.or_else(|| self.word_under_cursor()) else {
            self.status_msg = "E349: No identifier under cursor".into();
            return;
        };
        let matches = match self.tag_matches(&name) {
            Ok(m) => m,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        self.tag_push_and_open(name, matches, 0);
    }

    /// Opens match `idx` and, only once that has worked, records where the
    /// jump was made from.
    fn tag_push_and_open(&mut self, name: String, matches: Vec<Tag>, idx: usize) {
        let Some(origin) = self.buffer.path.clone() else {
            self.status_msg = "E32: No file name".into();
            return;
        };
        let (line, col) = (self.window.cursor.line, self.window.cursor.col);
        if let Err(e) = self.tag_open(&matches[idx]) {
            self.status_msg = e;
            return;
        }
        if matches.len() > 1 {
            self.status_msg = format!("tag {} of {}", idx + 1, matches.len());
        }
        self.tagstack.push(TagStackEntry {
            tag: name,
            path: origin,
            line,
            col,
            matches,
            match_idx: idx,
        });
    }

    /// The buffer and cursor moved to `tag`, or nothing moved at all. The
    /// address is resolved before the buffer is switched, so a pattern that
    /// no longer matches leaves the view where it was.
    fn tag_open(&mut self, tag: &Tag) -> Result<(), String> {
        let line = self.tag_line(tag)?;
        self.push_jump();
        self.open_buffer(tag.path.clone())
            .map_err(|e| format!("error: {e}"))?;
        let col = match tag.address {
            TagAddress::Line(_) => self.first_non_blank_col(line),
            TagAddress::Pattern(_) => 0,
        };
        self.window.cursor.line = line;
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
        self.clamp_cursor_normal();
        Ok(())
    }

    /// The line `tag` names, read from the open buffer when there is one —
    /// its unsaved text is what the cursor will land in.
    fn tag_line(&self, tag: &Tag) -> Result<usize, String> {
        let open = std::iter::once(&self.buffer)
            .chain(
                self.buffers
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != self.active)
                    .map(|(_, s)| &s.buffer),
            )
            .find(|b| b.path.as_deref() == Some(tag.path.as_path()));
        let read;
        let buffer = match open {
            Some(b) => b,
            None if tag.path.is_file() => {
                read = crate::buffer::Buffer::from_path(tag.path.clone())
                    .map_err(|e| format!("error: {e}"))?;
                &read
            }
            None => {
                return Err(format!(
                    "E429: File \"{}\" does not exist",
                    tag.path.display()
                ));
            }
        };
        tag::resolve_address(&tag.address, buffer)
            .ok_or_else(|| "E434: Can't find tag pattern".into())
    }

    /// `Ctrl-T` / `:pop` — back to where the top entry's jump was made from.
    pub(super) fn tag_pop(&mut self) {
        let Some(entry) = self.tagstack.pop() else {
            self.status_msg = "E73: Tag stack empty".into();
            return;
        };
        if let Err(e) = self.open_buffer(entry.path.clone()) {
            self.status_msg = format!("error: {e}");
            self.tagstack.push(entry);
            return;
        }
        self.window.cursor.line = entry.line;
        self.window.cursor.col = entry.col;
        self.window.cursor.want_col = entry.col;
        self.clamp_cursor_normal();
    }

    /// `:tnext` / `:tprevious` / `:tfirst` / `:tlast`. A move past either
    /// end stops at it and reports the error, as Vim does. The stack's depth
    /// doesn't change, so one `Ctrl-T` still returns to the origin.
    pub(super) fn tag_goto_match(&mut self, how: TagMove) {
        let Some(entry) = self.tagstack.last() else {
            self.status_msg = "E73: Tag stack empty".into();
            return;
        };
        let (cur, last) = (entry.match_idx, entry.matches.len() - 1);
        let (idx, err) = match how {
            TagMove::Next(n) if cur + n > last => {
                (last, Some("E428: Cannot go beyond last matching tag"))
            }
            TagMove::Next(n) => (cur + n, None),
            TagMove::Prev(n) if n > cur => (0, Some("E425: Cannot go before first matching tag")),
            TagMove::Prev(n) => (cur - n, None),
            TagMove::First => (0, None),
            TagMove::Last => (last, None),
        };
        self.tag_goto_index(idx);
        if let Some(err) = err {
            self.status_msg = err.into();
        }
    }

    fn tag_goto_index(&mut self, idx: usize) {
        let Some(entry) = self.tagstack.last() else {
            return;
        };
        let tag = entry.matches[idx].clone();
        let total = entry.matches.len();
        if let Err(e) = self.tag_open(&tag) {
            self.status_msg = e;
            return;
        }
        if let Some(entry) = self.tagstack.last_mut() {
            entry.match_idx = idx;
        }
        self.status_msg = format!("tag {} of {total}", idx + 1);
    }

    /// `g]` and `:tselect [name]` — the matches in a picker, however many
    /// there are. With no name, the top entry's matches, starting at the one
    /// shown.
    pub(super) fn tag_select(&mut self, name: Option<String>) {
        let (select, selected) = match name {
            Some(name) => match self.tag_matches(&name) {
                Ok(matches) => (
                    TagSelect {
                        tag: name,
                        matches,
                        push: true,
                    },
                    0,
                ),
                Err(e) => {
                    self.status_msg = e;
                    return;
                }
            },
            None => match self.tagstack.last() {
                Some(entry) => (
                    TagSelect {
                        tag: entry.tag.clone(),
                        matches: entry.matches.clone(),
                        push: false,
                    },
                    entry.match_idx,
                ),
                None => {
                    self.status_msg = "E73: Tag stack empty".into();
                    return;
                }
            },
        };
        let cwd = std::env::current_dir().unwrap_or_default();
        let items = select
            .matches
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let path = t.path.strip_prefix(&cwd).unwrap_or(&t.path);
                let at = match &t.address {
                    TagAddress::Line(n) => n.to_string(),
                    TagAddress::Pattern(p) => p.clone(),
                };
                let kind = t.kind.as_deref().unwrap_or("");
                let row = format!("{}  {kind}  {}  {at}", t.name, path.display());
                (row, PickerPayload::TagMatch(i))
            })
            .collect();
        let mut picker = PickerState::new(PickerKind::Tags, format!("tags: {}", select.tag), items);
        picker.selected = selected;
        self.picker = Some(picker);
        self.tag_select = Some(select);
        self.mode = crate::mode::Mode::Picker;
    }

    /// A row accepted in the `g]` / `:tselect` picker.
    pub(super) fn tag_pick(&mut self, idx: usize) {
        let Some(select) = self.tag_select.take() else {
            return;
        };
        if idx >= select.matches.len() {
            return;
        }
        if select.push {
            self.tag_push_and_open(select.tag, select.matches, idx);
        } else {
            self.tag_goto_index(idx);
        }
    }

    /// `:tags` — the stack, `>` on the top entry.
    pub(super) fn cmd_tags(&mut self) {
        let top = self.tagstack.len().saturating_sub(1);
        let cwd = std::env::current_dir().unwrap_or_default();
        let rows = self
            .tagstack
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let here = if i == top { '>' } else { ' ' };
                let label = format!(
                    "{here}{:>2} {:>2} {:<20} {:>5}",
                    i + 1,
                    e.match_idx + 1,
                    e.tag,
                    e.line + 1
                );
                let path = e.path.strip_prefix(&cwd).unwrap_or(&e.path);
                (label, path.display().to_string())
            })
            .collect();
        self.show_listing(super::state::Listing {
            title: "  # TO tag                 FROM line  in file".into(),
            rows,
            empty: "(tag stack empty)".into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::TagMove;
    use crate::mode::Mode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::{Path, PathBuf};

    const TAGS: &str = "!_TAG_FILE_SORTED\t1\t/0=unsorted/\n\
        dup\tb.txt\t1\n\
        dup\tc.txt\t2\n\
        dup\td.txt\t/^dup three$/\n\
        gone\tmissing.txt\t1\n\
        stale\tb.txt\t/^no such line$/\n\
        target\tb.txt\t/^fn target() {$/;\"\tf\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("binvim-tags-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tags"), TAGS).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\ncall target\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\nfn target() {\ny\n").unwrap();
        std::fs::write(dir.join("c.txt"), "c\n  dup two\n").unwrap();
        std::fs::write(dir.join("d.txt"), "d\ndup three\n").unwrap();
        dir
    }

    /// An app on `a.txt` with the cursor on `target`.
    fn app_in(dir: &Path) -> crate::app::App {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.open_buffer(dir.join("a.txt")).unwrap();
        app.window.cursor.line = 2;
        app.window.cursor.col = 7;
        app
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn at(app: &crate::app::App) -> (String, usize, usize) {
        let name = app.buffer.path.as_deref().unwrap().file_name().unwrap();
        (
            name.to_string_lossy().into_owned(),
            app.window.cursor.line,
            app.window.cursor.col,
        )
    }

    fn spot(file: &str, line: usize, col: usize) -> (String, usize, usize) {
        (file.to_string(), line, col)
    }

    #[test]
    fn jump_and_pop_across_files_even_after_the_origin_is_closed() {
        let dir = scratch("jump");
        let mut app = app_in(&dir);
        app.replay_key(ctrl(']'));
        assert_eq!(at(&app), spot("b.txt", 1, 0));
        assert!(app.status_msg.is_empty(), "{}", app.status_msg);
        app.replay_key(ctrl('t'));
        assert_eq!(at(&app), spot("a.txt", 2, 7));

        app.replay_key(ctrl(']'));
        app.open_buffer(dir.join("a.txt")).unwrap();
        app.exec_command("bd");
        assert_eq!(at(&app).0, "b.txt");
        app.replay_key(ctrl('t'));
        assert_eq!(at(&app), spot("a.txt", 2, 7));
        assert!(app.tagstack.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failures_leave_the_cursor_and_the_stack_alone() {
        let dir = scratch("fail");
        let mut app = app_in(&dir);
        app.replay_key(ctrl('t'));
        assert_eq!(app.status_msg, "E73: Tag stack empty");
        for (name, err) in [
            ("gone", "E429"),
            ("stale", "E434"),
            ("nothing", "E426: Tag not found: nothing"),
        ] {
            app.tag_jump(Some(name.into()));
            assert!(
                app.status_msg.starts_with(err),
                "{name}: {}",
                app.status_msg
            );
            assert_eq!(at(&app), spot("a.txt", 2, 7), "{name}");
            assert!(app.tagstack.is_empty(), "{name}");
        }
        assert_eq!(app.buffers.len(), 1, "a failed jump opened a buffer");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_tags_file_is_reported() {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.buffer.path = Some(PathBuf::from("/binvim-no-such-dir/a.txt"));
        app.buffer.rope = ropey::Rope::from_str("word\n");
        app.replay_key(ctrl(']'));
        assert_eq!(app.status_msg, "E433: No tags file");
        assert!(app.tagstack.is_empty());
    }

    #[test]
    fn walking_the_matches_stops_at_either_end() {
        let dir = scratch("walk");
        let mut app = app_in(&dir);
        app.tag_jump(Some("dup".into()));
        assert_eq!(app.status_msg, "tag 1 of 3");
        assert_eq!(at(&app), spot("b.txt", 0, 0));
        app.exec_command("tnext");
        assert_eq!(app.status_msg, "tag 2 of 3");
        assert_eq!(at(&app), spot("c.txt", 1, 2));
        app.exec_command("tprevious");
        assert_eq!(at(&app), spot("b.txt", 0, 0));
        app.exec_command("tlast");
        assert_eq!(at(&app), spot("d.txt", 1, 0));
        app.exec_command("tnext");
        assert!(app.status_msg.starts_with("E428"), "{}", app.status_msg);
        assert_eq!(at(&app), spot("d.txt", 1, 0));
        app.tag_goto_match(TagMove::Prev(1));
        assert_eq!(at(&app), spot("c.txt", 1, 2));
        app.exec_command("tfirst");
        assert_eq!(at(&app), spot("b.txt", 0, 0));
        app.exec_command("tprevious");
        assert!(app.status_msg.starts_with("E425"), "{}", app.status_msg);
        assert_eq!(at(&app), spot("b.txt", 0, 0));

        for _ in 0..4 {
            app.exec_command("tnext");
        }
        assert_eq!(app.tagstack.len(), 1);
        app.replay_key(ctrl('t'));
        assert_eq!(at(&app), spot("a.txt", 2, 7));
        app.exec_command("tnext");
        assert_eq!(app.status_msg, "E73: Tag stack empty");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_lists_every_match_and_a_pick_becomes_current() {
        let dir = scratch("select");
        let mut app = app_in(&dir);
        app.replay_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.replay_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Picker));
        assert_eq!(app.picker.as_ref().unwrap().items.len(), 1);
        app.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.tagstack.is_empty());

        app.exec_command("tselect dup");
        assert_eq!(app.picker.as_ref().unwrap().items.len(), 3);
        app.replay_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.replay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(at(&app), spot("c.txt", 1, 2));
        assert_eq!(app.tagstack[0].match_idx, 1);
        app.exec_command("tnext");
        assert_eq!(at(&app), spot("d.txt", 1, 0));

        // With no name, the top entry's list, opened on its current match;
        // a pick moves within that entry rather than adding one.
        app.exec_command("tselect");
        assert_eq!(app.picker.as_ref().unwrap().selected, 2);
        app.replay_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.replay_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.replay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(at(&app), spot("b.txt", 0, 0));
        assert_eq!(app.tagstack.len(), 1);

        app.exec_command("tags");
        let listing = app.listing.as_ref().unwrap();
        assert_eq!(listing.rows.len(), 1);
        assert!(
            listing.rows[0].0.starts_with("> 1  1 dup"),
            "{:?}",
            listing.rows
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
