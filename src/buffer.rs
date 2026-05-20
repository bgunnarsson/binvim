use anyhow::{Context, Result};
use ropey::Rope;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::SystemTime;

/// Line-ending convention for a buffer. We always normalize to LF in
/// the rope so the rest of the editor (motion, render, LSP) only ever
/// sees `\n`; this enum records what the file *was* on disk so save
/// can emit the same. `.editorconfig`'s `end_of_line` overrides the
/// inferred-from-disk value when set.
///
/// Mixed-ending files collapse to `Lf` on load — saving then loses
/// the trailing CRs, which is fine: a Windows file with stray LFs is
/// already inconsistent, and rewriting it uniformly is the lesser
/// surprise. The detector counts CRLFs vs bare LFs and picks the
/// majority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n` — Unix, the rope's internal representation.
    Lf,
    /// `\r\n` — Windows; the load+save round-trip preserves these.
    Crlf,
}

impl LineEnding {
    /// Platform default for path-less buffers (`Buffer::empty`).
    /// `Crlf` on Windows, `Lf` everywhere else.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        }
    }
}

/// Walk `bytes` counting CRLF vs bare-LF occurrences and pick the
/// majority. Files with neither (no newlines at all) get the platform
/// default — same heuristic editors like VS Code use.
fn detect_line_ending(bytes: &[u8]) -> LineEnding {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    if crlf == 0 && lf == 0 {
        LineEnding::platform_default()
    } else if crlf > lf {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// Stream the rope to `w`, translating `\n` → `\r\n` when `ending`
/// is `Crlf`. `Lf` falls through to ropey's native `write_to`. The
/// `Crlf` branch chunks through the rope's underlying str slices so
/// throughput stays near memcpy speed.
fn write_rope_with_eol<W: Write>(rope: &Rope, ending: LineEnding, mut w: W) -> std::io::Result<()> {
    match ending {
        LineEnding::Lf => rope.write_to(&mut w).map_err(std::io::Error::other),
        LineEnding::Crlf => {
            for chunk in rope.chunks() {
                let bytes = chunk.as_bytes();
                let mut start = 0;
                for (i, b) in bytes.iter().enumerate() {
                    if *b == b'\n' {
                        if start < i {
                            w.write_all(&bytes[start..i])?;
                        }
                        w.write_all(b"\r\n")?;
                        start = i + 1;
                    }
                }
                if start < bytes.len() {
                    w.write_all(&bytes[start..])?;
                }
            }
            Ok(())
        }
    }
}

/// Bytes above which a buffer is considered "large" — the rope still
/// handles the volume fine, but tree-sitter highlight passes take
/// seconds (blocking the event loop) and most LSPs choke on initial
/// didOpen. Large-file mode disables both. 5MB picks up typical
/// generated bundles / minified JS / SQL dumps without false-tripping
/// on regular source files.
pub const LARGE_FILE_BYTES: usize = 5 * 1024 * 1024;
/// Line-count threshold for the same gate. Some files (machine-
/// generated JSON, log captures) sit under the byte cap but still
/// have enough lines to drag tree-sitter through tens of thousands of
/// captures per refresh.
pub const LARGE_FILE_LINES: usize = 50_000;

#[derive(Clone, Debug)]
pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// Bumped on every mutation; used to invalidate the syntax-highlight cache.
    pub version: u64,
    /// File mtime captured at the most-recent on-disk load or save. Drives
    /// the auto-reload watcher — if the file's current mtime is newer and
    /// the buffer isn't dirty, the watcher reloads from disk.
    pub disk_mtime: Option<SystemTime>,
    /// Synthetic label for path-less internal buffers (e.g. `[Health]`). Lets
    /// the buffer list show something meaningful instead of `[No Name]`.
    pub display_name: Option<String>,
    /// What the file was on disk (or platform default for path-less
    /// buffers). `save` emits the matching ending; `.editorconfig`'s
    /// `end_of_line` overrides this when set.
    pub line_ending: LineEnding,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::empty()
    }
}

impl Buffer {
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            dirty: false,
            version: 0,
            disk_mtime: None,
            display_name: None,
            line_ending: LineEnding::platform_default(),
        }
    }

    pub fn from_path(path: PathBuf) -> Result<Self> {
        if path.exists() {
            let mut file =
                File::open(&path).with_context(|| format!("opening {}", path.display()))?;
            let mtime = file.metadata().ok().and_then(|m| m.modified().ok());
            // Normalize CRLF → LF on load. ropey preserves bytes verbatim, so a
            // stray `\r` left in a line would reach the renderer and reset the
            // terminal cursor to column 0 — clobbering inline diagnostics and
            // any subsequent same-row content. The detected ending is stored
            // on the buffer so `save` can re-emit the same convention.
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .with_context(|| format!("reading {}", path.display()))?;
            let line_ending = detect_line_ending(&bytes);
            let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
            let rope = Rope::from_str(&text);
            Ok(Self {
                rope,
                path: Some(path),
                dirty: false,
                version: 0,
                disk_mtime: mtime,
                display_name: None,
                line_ending,
            })
        } else {
            Ok(Self {
                rope: Rope::new(),
                path: Some(path),
                dirty: false,
                version: 0,
                disk_mtime: None,
                display_name: None,
                line_ending: LineEnding::platform_default(),
            })
        }
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .context("no file path set (use :w {filename})")?;
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        write_rope_with_eol(&self.rope, self.line_ending, BufWriter::new(file))
            .with_context(|| format!("writing {}", path.display()))?;
        self.dirty = false;
        // Refresh mtime so the watcher doesn't immediately think the file
        // changed under us.
        if let Ok(meta) = std::fs::metadata(path) {
            self.disk_mtime = meta.modified().ok();
        }
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
        self.version = self.version.wrapping_add(1);
    }

    pub fn insert_str(&mut self, line: usize, col: usize, s: &str) {
        let idx = self.pos_to_char(line, col);
        self.rope.insert(idx, s);
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    /// Insert text at an absolute char index.
    pub fn insert_at_idx(&mut self, idx: usize, s: &str) {
        self.rope.insert(idx, s);
        if !s.is_empty() {
            self.dirty = true;
            self.version = self.version.wrapping_add(1);
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
            self.version = self.version.wrapping_add(1);
        }
        removed
    }

    pub fn total_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// True when the buffer trips the large-file threshold by either
    /// byte volume or line count. Callers (highlight cache, LSP
    /// attach, render hot paths) bail early when this returns true so
    /// the editor stays responsive on huge files.
    pub fn is_large(&self) -> bool {
        self.rope.len_bytes() > LARGE_FILE_BYTES || self.rope.len_lines() > LARGE_FILE_LINES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_with_text(s: &str) -> Buffer {
        let mut b = Buffer::empty();
        b.rope = ropey::Rope::from_str(s);
        b
    }

    #[test]
    fn small_buffer_is_not_large() {
        let b = buf_with_text("fn main() { println!(\"hello\"); }\n");
        assert!(!b.is_large());
    }

    #[test]
    fn buffer_over_byte_threshold_is_large() {
        let big = "x".repeat(LARGE_FILE_BYTES + 1);
        let b = buf_with_text(&big);
        assert!(b.is_large());
    }

    #[test]
    fn buffer_over_line_threshold_is_large() {
        let many = "a\n".repeat(LARGE_FILE_LINES + 1);
        let b = buf_with_text(&many);
        assert!(b.is_large());
    }

    #[test]
    fn buffer_just_under_thresholds_is_not_large() {
        // (LARGE_FILE_LINES - 1) lines of one char each ≈ 100KB, well
        // under the byte cap.
        let text = "a\n".repeat(LARGE_FILE_LINES - 1);
        let b = buf_with_text(&text);
        assert!(!b.is_large());
    }

    #[test]
    fn detect_lf_only_file() {
        assert_eq!(detect_line_ending(b"a\nb\nc\n"), LineEnding::Lf);
    }

    #[test]
    fn detect_crlf_only_file() {
        assert_eq!(detect_line_ending(b"a\r\nb\r\nc\r\n"), LineEnding::Crlf);
    }

    #[test]
    fn detect_mixed_picks_majority() {
        // 2 CRLF, 1 LF → Crlf.
        assert_eq!(detect_line_ending(b"a\r\nb\r\nc\n"), LineEnding::Crlf);
        // 1 CRLF, 2 LF → Lf.
        assert_eq!(detect_line_ending(b"a\r\nb\nc\n"), LineEnding::Lf);
    }

    #[test]
    fn detect_no_newlines_falls_back_to_platform_default() {
        assert_eq!(
            detect_line_ending(b"no newlines"),
            LineEnding::platform_default()
        );
    }

    #[test]
    fn lf_load_save_roundtrip() {
        let tmp = std::env::temp_dir().join("binvim_lf_roundtrip.txt");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, "hello\nworld\n").unwrap();
        let mut buf = Buffer::from_path(tmp.clone()).unwrap();
        assert_eq!(buf.line_ending, LineEnding::Lf);
        buf.save().unwrap();
        let out = std::fs::read(&tmp).unwrap();
        assert_eq!(out, b"hello\nworld\n");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn crlf_load_save_roundtrip() {
        let tmp = std::env::temp_dir().join("binvim_crlf_roundtrip.txt");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, "hello\r\nworld\r\n").unwrap();
        let mut buf = Buffer::from_path(tmp.clone()).unwrap();
        assert_eq!(buf.line_ending, LineEnding::Crlf);
        // The rope itself has been normalized to LF — verify.
        assert_eq!(buf.rope.to_string(), "hello\nworld\n");
        buf.save().unwrap();
        let out = std::fs::read(&tmp).unwrap();
        assert_eq!(out, b"hello\r\nworld\r\n");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn crlf_can_be_overridden_to_lf_on_save() {
        let tmp = std::env::temp_dir().join("binvim_crlf_to_lf.txt");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, "hello\r\nworld\r\n").unwrap();
        let mut buf = Buffer::from_path(tmp.clone()).unwrap();
        assert_eq!(buf.line_ending, LineEnding::Crlf);
        // Simulate `.editorconfig`'s `end_of_line = lf` override.
        buf.line_ending = LineEnding::Lf;
        buf.save().unwrap();
        let out = std::fs::read(&tmp).unwrap();
        assert_eq!(out, b"hello\nworld\n");
        let _ = std::fs::remove_file(&tmp);
    }
}
