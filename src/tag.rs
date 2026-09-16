//! ctags-format `tags` files: parsing a line, finding the file, and turning a
//! tag's address into a line of a buffer. binvim reads these files and never
//! writes one — the user runs ctags.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::buffer::Buffer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub path: PathBuf,
    pub address: TagAddress,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAddress {
    /// 1-based, as the file writes it.
    Line(usize),
    /// The search pattern with its delimiters and escapes removed; `^` / `$`
    /// anchors are kept.
    Pattern(String),
}

/// One line of a `tags` file. `None` for a header (`!_TAG_…`) and for a line
/// too short to hold a name, a file and an address.
///
/// The address is read by hand rather than by splitting on tabs, because a
/// pattern can hold a literal tab.
pub fn parse_tag_line(line: &str) -> Option<Tag> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.starts_with("!_TAG_") {
        return None;
    }
    let mut parts = line.splitn(3, '\t');
    let name = parts.next().filter(|s| !s.is_empty())?;
    let file = parts.next().filter(|s| !s.is_empty())?;
    let rest = parts.next()?;
    let (address, fields) = parse_address(rest)?;
    let fields = fields.strip_prefix(";\"").unwrap_or(fields);
    let kind = fields
        .split('\t')
        .filter(|f| !f.is_empty())
        .find_map(|f| match f.split_once(':') {
            Some(("kind", k)) => Some(k.to_string()),
            Some(_) => None,
            // The old format writes the kind as a bare letter.
            None => Some(f.to_string()),
        });
    Some(Tag {
        name: name.to_string(),
        path: PathBuf::from(file),
        address,
        kind,
    })
}

/// The nearest `tags` file at or above `start`. Only `tags`: Vim's default
/// also reads `TAGS`, but that is Emacs's format.
pub fn find_tags_file(start: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(start).ok()?;
    start
        .ancestors()
        .map(|dir| dir.join("tags"))
        .find(|p| p.is_file() && !writable_by_others(p))
}

/// A `tags` file chooses what a jump opens, and opening a file starts its
/// language server, which may build the project. The search climbs out of
/// the project, so a `tags` file that any user can rewrite, or that sits in a
/// directory any user can write to (`/tmp`), could be another user's plant.
#[cfg(unix)]
fn writable_by_others(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.permissions().mode());
    let dir = path.parent().unwrap_or(Path::new("/"));
    (mode(path) | mode(dir)) & 0o002 != 0
}

#[cfg(not(unix))]
fn writable_by_others(_: &Path) -> bool {
    false
}

/// A parsed `tags` file, kept while the file's mtime and length stay put —
/// one for a large tree is megabytes, too much to read per jump.
#[derive(Debug)]
pub struct TagIndex {
    pub path: PathBuf,
    stamp: (SystemTime, u64),
    pub tags: Vec<Tag>,
    #[cfg(test)]
    loads: usize,
}

impl TagIndex {
    /// The index for `tags_path`, reusing `cached` when it was read from the
    /// same file and the file hasn't changed since.
    pub fn load(cached: Option<TagIndex>, tags_path: &Path) -> std::io::Result<TagIndex> {
        let meta = std::fs::metadata(tags_path)?;
        let stamp = (meta.modified()?, meta.len());
        if let Some(index) = cached.filter(|i| i.path == tags_path && i.stamp == stamp) {
            return Ok(index);
        }
        let bytes = std::fs::read(tags_path)?;
        let dir = tags_path.parent().unwrap_or(Path::new("/"));
        let tags = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(parse_tag_line)
            .map(|mut tag| {
                tag.path = resolve_path(dir, &tag.path);
                tag
            })
            .collect();
        Ok(TagIndex {
            path: tags_path.to_path_buf(),
            stamp,
            tags,
            #[cfg(test)]
            loads: 1,
        })
    }

    /// Every tag named `name`, in the file's order.
    pub fn matches(&self, name: &str) -> Vec<Tag> {
        self.tags
            .iter()
            .filter(|t| t.name == name)
            .cloned()
            .collect()
    }
}

/// The 0-based line `address` names in `buffer`. A line number past the end
/// is the last line; a pattern that matches nothing is `None`.
///
/// Patterns are literal apart from their `^` / `$` anchors, as ctags writes
/// them — read as a regex, the `.` and `*` most source lines hold would
/// misfire.
pub fn resolve_address(address: &TagAddress, buffer: &Buffer) -> Option<usize> {
    let pattern = match address {
        TagAddress::Line(n) => return Some(n.saturating_sub(1).min(buffer.line_count() - 1)),
        TagAddress::Pattern(p) => p.as_str(),
    };
    let (start, pattern) = match pattern.strip_prefix('^') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let (end, pattern) = match pattern.strip_suffix('$') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    (0..buffer.line_count()).find(|&i| {
        let line = buffer.rope.line(i).to_string();
        let line = line.trim_end_matches('\n');
        match (start, end) {
            (true, true) => line == pattern,
            (true, false) => line.starts_with(pattern),
            (false, true) => line.ends_with(pattern),
            (false, false) => line.contains(pattern),
        }
    })
}

/// A tag's file, which the tags file names relative to its own directory,
/// as an absolute path with `.` and `..` folded away — the form buffer paths
/// take, so opening it finds the buffer that's already open.
fn resolve_path(dir: &Path, file: &Path) -> PathBuf {
    let joined = std::path::absolute(dir.join(file)).unwrap_or_else(|_| dir.join(file));
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The address at the start of `s`, and what follows it.
fn parse_address(s: &str) -> Option<(TagAddress, &str)> {
    let delim = s.chars().next()?;
    if delim.is_ascii_digit() {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let n = s[..end].parse().ok()?;
        return Some((TagAddress::Line(n), &s[end..]));
    }
    if delim != '/' && delim != '?' {
        return None;
    }
    let mut pattern = String::new();
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if c == delim {
            return Some((TagAddress::Pattern(pattern), &s[i + 1..]));
        }
        if c == '\\' {
            match chars.next() {
                Some((_, e)) if e == delim || e == '\\' => pattern.push(e),
                Some((_, e)) => {
                    pattern.push('\\');
                    pattern.push(e);
                }
                None => return None,
            }
        } else {
            pattern.push(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("binvim-tag-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn line_number_address() {
        let tag = parse_tag_line("main\tsrc/main.c\t42").unwrap();
        assert_eq!(tag.name, "main");
        assert_eq!(tag.path, PathBuf::from("src/main.c"));
        assert_eq!(tag.address, TagAddress::Line(42));
        assert_eq!(tag.kind, None);
    }

    #[test]
    fn pattern_address() {
        let tag = parse_tag_line("main\tsrc/main.rs\t/^fn main() {$/;\"\tf").unwrap();
        assert_eq!(tag.address, TagAddress::Pattern("^fn main() {$".into()));
        assert_eq!(tag.kind.as_deref(), Some("f"));
    }

    #[test]
    fn pattern_with_escaped_slash_and_backslash() {
        let tag = parse_tag_line("div\tm.c\t/^int div(a) { return a \\/ 2; } \\\\$/").unwrap();
        assert_eq!(
            tag.address,
            TagAddress::Pattern("^int div(a) { return a / 2; } \\$".into())
        );
    }

    #[test]
    fn backward_pattern_and_a_literal_tab() {
        let tag = parse_tag_line("x\ta.c\t?^\tint x;$?").unwrap();
        assert_eq!(tag.address, TagAddress::Pattern("^\tint x;$".into()));
    }

    #[test]
    fn header_line_is_skipped() {
        assert_eq!(
            parse_tag_line("!_TAG_FILE_FORMAT\t2\t/extended format/"),
            None
        );
    }

    #[test]
    fn extended_format_fields() {
        let tag = parse_tag_line(
            "Buffer\tsrc/buffer.rs\t/^pub struct Buffer {$/;\"\tkind:struct\tline:12\tfile:",
        )
        .unwrap();
        assert_eq!(tag.kind.as_deref(), Some("struct"));
        assert_eq!(
            tag.address,
            TagAddress::Pattern("^pub struct Buffer {$".into())
        );
    }

    #[test]
    fn old_format_bare_kind_letter() {
        let tag = parse_tag_line("helper\tlib.sh\t/^helper() {$/;\"\tf\tline:3").unwrap();
        assert_eq!(tag.kind.as_deref(), Some("f"));
    }

    #[test]
    fn truncated_lines_are_rejected() {
        assert_eq!(parse_tag_line("name\tfile"), None);
        assert_eq!(parse_tag_line("name\tfile\t/unterminated"), None);
        assert_eq!(parse_tag_line("\tfile\t1"), None);
    }

    #[test]
    fn tags_file_is_found_above_the_start() {
        let root = scratch("find");
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_ne!(find_tags_file(&deep), Some(root.join("tags")));
        std::fs::write(root.join("tags"), "").unwrap();
        assert_eq!(find_tags_file(&deep), Some(root.join("tags")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn tags_files_others_can_write_are_passed_over() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("shared");
        let open = |p: &Path, mode| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let project = root.join("shared/project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(root.join("tags"), "").unwrap();
        std::fs::write(root.join("shared/tags"), "").unwrap();
        open(&root.join("shared"), 0o777);
        assert_eq!(find_tags_file(&project), Some(root.join("tags")));
        std::fs::write(project.join("tags"), "").unwrap();
        open(&project.join("tags"), 0o666);
        assert_eq!(find_tags_file(&project), Some(root.join("tags")));
        open(&project.join("tags"), 0o644);
        assert_eq!(find_tags_file(&project), Some(project.join("tags")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_tags_file_anywhere() {
        // `/` has no `tags` file on any machine this runs on; a scratch
        // directory can't be checked for "nowhere above" without it.
        assert_eq!(find_tags_file(Path::new("/")), None);
    }

    #[test]
    fn relative_paths_resolve_against_the_tags_file() {
        let root = scratch("resolve");
        let tags = root.join("sub/tags");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(&tags, "main\t../src/main.rs\t1\nabs\t/etc/hosts\t1\n").unwrap();
        let index = TagIndex::load(None, &tags).unwrap();
        assert_eq!(index.matches("main")[0].path, root.join("src/main.rs"));
        assert!(index.matches("main")[0].path.is_absolute());
        assert_eq!(index.matches("abs")[0].path, PathBuf::from("/etc/hosts"));
        assert!(index.matches("missing").is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn index_is_reread_only_when_the_file_changes() {
        let root = scratch("cache");
        let tags = root.join("tags");
        std::fs::write(&tags, "one\ta.c\t1\n").unwrap();
        let index = TagIndex::load(None, &tags).unwrap();
        assert_eq!(index.loads, 1);
        let index = TagIndex { loads: 7, ..index };
        let index = TagIndex::load(Some(index), &tags).unwrap();
        assert_eq!(index.loads, 7, "untouched file was read again");
        let later = SystemTime::now() + std::time::Duration::from_secs(10);
        std::fs::File::options()
            .write(true)
            .open(&tags)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let index = TagIndex::load(Some(index), &tags).unwrap();
        assert_eq!(index.loads, 1, "touched file was not read again");
        std::fs::write(&tags, "one\ta.c\t1\ntwo\tb.c\t2\n").unwrap();
        let index = TagIndex::load(Some(index), &tags).unwrap();
        assert_eq!(index.matches("two").len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    fn buffer(text: &str) -> Buffer {
        Buffer {
            rope: ropey::Rope::from_str(text),
            ..Buffer::default()
        }
    }

    fn pattern(p: &str) -> TagAddress {
        TagAddress::Pattern(p.into())
    }

    #[test]
    fn anchored_pattern_matches_the_whole_line() {
        let buf = buffer("fn main_loop() {}\n// fn main() {\nfn main() {\n}\n");
        assert_eq!(resolve_address(&pattern("^fn main() {$"), &buf), Some(2));
    }

    #[test]
    fn unanchored_and_half_anchored_patterns() {
        let buf = buffer("int a = 1;\nint *b.c = 2;\n");
        assert_eq!(resolve_address(&pattern("*b.c"), &buf), Some(1));
        assert_eq!(resolve_address(&pattern("^int *b"), &buf), Some(1));
        assert_eq!(resolve_address(&pattern("= 2;$"), &buf), Some(1));
    }

    #[test]
    fn pattern_matching_nothing_is_none() {
        let buf = buffer("fn other() {}\n");
        assert_eq!(resolve_address(&pattern("^fn main() {$"), &buf), None);
        // `.` is literal, not "any character".
        assert_eq!(resolve_address(&pattern("fn.other"), &buf), None);
    }

    #[test]
    fn line_number_is_clamped_to_the_buffer() {
        let buf = buffer("a\nb\nc");
        assert_eq!(resolve_address(&TagAddress::Line(2), &buf), Some(1));
        assert_eq!(resolve_address(&TagAddress::Line(99), &buf), Some(2));
        assert_eq!(resolve_address(&TagAddress::Line(0), &buf), Some(0));
    }
}
