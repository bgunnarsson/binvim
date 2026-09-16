//! ctags-format `tags` files: parsing a line, finding the file, and turning a
//! tag's address into a line of a buffer. binvim reads these files and never
//! writes one — the user runs ctags.

use std::path::PathBuf;

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
}
