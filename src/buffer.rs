use anyhow::{Context, Result};
use ropey::Rope;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

impl Buffer {
    pub fn empty() -> Self {
        Self { rope: Rope::new(), path: None, dirty: false }
    }

    pub fn from_path(path: PathBuf) -> Result<Self> {
        if path.exists() {
            let file = File::open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            let rope = Rope::from_reader(BufReader::new(file))
                .with_context(|| format!("reading {}", path.display()))?;
            Ok(Self { rope, path: Some(path), dirty: false })
        } else {
            Ok(Self { rope: Rope::new(), path: Some(path), dirty: false })
        }
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self.path.as_ref().context("no file path set (use :w {filename})")?;
        let file = File::create(path)
            .with_context(|| format!("creating {}", path.display()))?;
        self.rope
            .write_to(BufWriter::new(file))
            .with_context(|| format!("writing {}", path.display()))?;
        self.dirty = false;
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    /// Char count of `line`, excluding the trailing newline.
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.line_count() {
            return 0;
        }
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len - 1
        } else {
            len
        }
    }

    /// Convert (line, col) to absolute char index. Clamps both.
    pub fn pos_to_char(&self, line: usize, col: usize) -> usize {
        let last = self.line_count().saturating_sub(1);
        let line = line.min(last);
        let line_start = self.rope.line_to_char(line);
        line_start + col.min(self.line_len(line))
    }

    pub fn char_at(&self, line: usize, col: usize) -> Option<char> {
        if col >= self.line_len(line) {
            return None;
        }
        let idx = self.pos_to_char(line, col);
        self.rope.get_char(idx)
    }

    /// Char index of the start of `line` (0..=line_count).
    pub fn line_start_idx(&self, line: usize) -> usize {
        let last = self.line_count();
        let line = line.min(last);
        if line == last {
            return self.rope.len_chars();
        }
        self.rope.line_to_char(line)
    }

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) {
        let idx = self.pos_to_char(line, col);
        let mut buf = [0u8; 4];
        self.rope.insert(idx, ch.encode_utf8(&mut buf));
        self.dirty = true;
    }

    pub fn insert_str(&mut self, line: usize, col: usize, s: &str) {
        let idx = self.pos_to_char(line, col);
        self.rope.insert(idx, s);
        self.dirty = true;
    }

    /// Insert text at an absolute char index.
    pub fn insert_at_idx(&mut self, idx: usize, s: &str) {
        self.rope.insert(idx, s);
        if !s.is_empty() {
            self.dirty = true;
        }
    }

    /// Delete chars in [start, end) and return the removed text.
    pub fn delete_range(&mut self, start: usize, end: usize) -> String {
        if end <= start {
            return String::new();
        }
        let removed = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        if !removed.is_empty() {
            self.dirty = true;
        }
        removed
    }

    pub fn total_chars(&self) -> usize {
        self.rope.len_chars()
    }
}
