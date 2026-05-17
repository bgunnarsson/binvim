//! Spell check. Toggleable per-buffer via `:spell`; navigation with
//! `]s` / `[s`; suggestions with `z=`. The wordlist comes from
//! `/usr/share/dict/words` (shipped with macOS and most Linux
//! distributions); a user-local override at
//! `~/.local/share/binvim/words` is consulted first.
//!
//! No external library dependency. The check itself is a HashSet
//! membership test on a lowercased token; suggestions are computed
//! by enumerating every single-edit neighbour of the misspelled word
//! (insert / delete / substitute / transpose) and filtering against
//! the same set. ~235k entries fit comfortably in memory; the load
//! takes ~30ms on a warm-cache SSD.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::buffer::Buffer;

/// One misspelled span inside the buffer. `line` / `col` are 0-indexed
/// char positions matching the rest of the codebase; `len` is char
/// count (not bytes) so the caller can use it directly with
/// `rope.char_to_byte` etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellRange {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub word: String,
}

static WORDLIST: OnceLock<Option<HashSet<String>>> = OnceLock::new();

/// Load the wordlist on first use. Returns `None` when neither the
/// user-local override nor the system path exists — callers should
/// surface a status message so the user knows to install one.
pub fn wordlist() -> Option<&'static HashSet<String>> {
    WORDLIST
        .get_or_init(|| {
            let user_path = dirs_local_words();
            for candidate in [user_path, Some(PathBuf::from("/usr/share/dict/words"))]
                .into_iter()
                .flatten()
            {
                if let Ok(text) = std::fs::read_to_string(&candidate) {
                    let mut set: HashSet<String> = HashSet::with_capacity(250_000);
                    for line in text.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        set.insert(trimmed.to_ascii_lowercase());
                    }
                    if !set.is_empty() {
                        return Some(set);
                    }
                }
            }
            None
        })
        .as_ref()
}

fn dirs_local_words() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/binvim/words"))
}

/// Walk the buffer, extracting word-shaped tokens, and return the
/// list of ones the dictionary doesn't recognise. Words split on
/// non-letter / non-apostrophe characters; tokens that are purely
/// numeric or shorter than 3 chars are skipped (too many false
/// positives — `id`, `ok`, version numbers, etc.). camelCase /
/// snake_case identifiers are split into their constituent parts and
/// each part is checked separately, so a single `getPlayerName` only
/// trips on a constituent the dictionary doesn't know.
pub fn check_buffer(buffer: &Buffer) -> Vec<SpellRange> {
    let Some(words) = wordlist() else { return Vec::new() };
    let mut out = Vec::new();
    for (line_idx, line) in buffer.rope.lines().enumerate() {
        let line_s = line.to_string();
        for token in tokens(&line_s) {
            for part in split_identifier(&token.text) {
                if !is_check_candidate(&part) {
                    continue;
                }
                let lower = part.to_ascii_lowercase();
                if words.contains(&lower) {
                    continue;
                }
                // Re-derive the byte/char offset of `part` within the
                // raw line so navigation / render line up with what
                // the user sees.
                if let Some(part_col) = find_subtoken_col(&line_s, &token, &part) {
                    out.push(SpellRange {
                        line: line_idx,
                        col: part_col,
                        len: part.chars().count(),
                        word: part,
                    });
                }
            }
        }
    }
    out
}

/// True when `word` is in the dictionary (or trivially fine to skip).
/// Public so callers like `suggestions` can short-circuit on already-
/// valid input.
pub fn is_known(word: &str) -> bool {
    if !is_check_candidate(word) {
        return true;
    }
    let Some(words) = wordlist() else { return true };
    words.contains(&word.to_ascii_lowercase())
}

/// Up to `cap` single-edit suggestions for a misspelled word. The
/// candidate generator enumerates every insert / delete / substitute
/// / transpose at every position; each candidate that's in the
/// dictionary becomes a suggestion. Capped + deduplicated so the
/// picker stays snappy on long words.
pub fn suggestions(word: &str, cap: usize) -> Vec<String> {
    let Some(dict) = wordlist() else { return Vec::new() };
    let lower = word.to_ascii_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for cand in enumerate_edits(&lower) {
        if dict.contains(&cand) && seen.insert(cand.clone()) {
            out.push(cand);
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    char_offset: usize,
}

/// Split a line into word-shaped tokens. A word is a maximal run of
/// alphabetic chars (plus interior apostrophes for contractions like
/// `don't`). Anything else terminates a token. `char_offset` is the
/// 0-indexed char column of the token's first char within the line.
fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start: Option<usize> = None;
    for (i, c) in line.chars().enumerate() {
        if c.is_alphabetic() || (c == '\'' && !current.is_empty()) {
            if start.is_none() {
                start = Some(i);
            }
            current.push(c);
        } else if !current.is_empty() {
            // Trim a trailing apostrophe (`foo'` shouldn't include it).
            while current.ends_with('\'') {
                current.pop();
            }
            if !current.is_empty() {
                out.push(Token {
                    text: std::mem::take(&mut current),
                    char_offset: start.unwrap_or(i),
                });
            }
            current.clear();
            start = None;
        }
    }
    if !current.is_empty() {
        while current.ends_with('\'') {
            current.pop();
        }
        if !current.is_empty() {
            out.push(Token {
                text: current,
                char_offset: start.unwrap_or(0),
            });
        }
    }
    out
}

/// Split a token like `getPlayerName` or `player_name` into its
/// constituent words. Camel boundaries trigger on lowercase→uppercase
/// or on the first letter following a digit. Snake / kebab split on
/// `_` / `-`. Acronym runs (`URL` in `parseURLPath`) stay grouped.
fn split_identifier(token: &str) -> Vec<String> {
    if token.is_empty() {
        return Vec::new();
    }
    // First pass: split on `_` / `-` boundaries.
    let parts_initial: Vec<&str> = token.split(|c: char| c == '_' || c == '-').collect();
    let mut out: Vec<String> = Vec::new();
    for chunk in parts_initial {
        if chunk.is_empty() {
            continue;
        }
        // Second pass: camelCase / PascalCase / acronym splits.
        let chars: Vec<char> = chunk.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let prev = chars[i - 1];
            let cur = chars[i];
            let is_camel_boundary = prev.is_lowercase() && cur.is_uppercase();
            // `URLPath` → `URL` + `Path` — split right before the
            // last upper of a run when it's followed by a lower.
            let is_acronym_end = i + 1 < chars.len()
                && prev.is_uppercase()
                && cur.is_uppercase()
                && chars[i + 1].is_lowercase();
            let is_acronym_end_combined = prev.is_uppercase() && cur.is_uppercase() && {
                // Apply when the next char is lowercase — caught
                // above — OR when we're at the last position. The
                // above arm handles it.
                false
            };
            if is_camel_boundary || is_acronym_end || is_acronym_end_combined {
                out.push(chars[start..i].iter().collect());
                start = i;
            }
        }
        out.push(chars[start..].iter().collect());
    }
    out
}

fn is_check_candidate(word: &str) -> bool {
    if word.len() < 3 {
        return false;
    }
    // Skip pure-uppercase abbreviations (HTTP, JSON, etc.) — too
    // many false positives for source code.
    if word.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    // Skip if any char isn't alphabetic or apostrophe — defensive
    // against tokens that snuck through with stray bytes.
    if !word.chars().all(|c| c.is_alphabetic() || c == '\'') {
        return false;
    }
    true
}

/// Compute the 0-indexed char column of `part` inside `line` given
/// the parent token's position. Walks forward from the parent's
/// start to locate the first occurrence — works for camelCase splits
/// because each constituent appears exactly once in order.
fn find_subtoken_col(line: &str, parent: &Token, part: &str) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let part_chars: Vec<char> = part.chars().collect();
    let parent_end = parent.char_offset + parent.text.chars().count();
    let mut i = parent.char_offset;
    while i + part_chars.len() <= parent_end.min(chars.len()) {
        if chars[i..i + part_chars.len()] == part_chars[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Enumerate every single-edit neighbour of `s`. ASCII-letters only;
/// non-ASCII words get insert / delete / transpose but no
/// substitution alphabet (otherwise the candidate set explodes).
fn enumerate_edits(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let alphabet: Vec<char> = ('a'..='z').collect();
    let mut out: Vec<String> = Vec::with_capacity(64);

    // Deletions.
    for i in 0..chars.len() {
        let mut cand: String = chars[..i].iter().collect();
        cand.extend(chars[i + 1..].iter());
        out.push(cand);
    }
    // Transpositions (`teh` → `the`).
    for i in 0..chars.len().saturating_sub(1) {
        let mut cand: Vec<char> = chars.clone();
        cand.swap(i, i + 1);
        out.push(cand.iter().collect());
    }
    // Substitutions.
    for i in 0..chars.len() {
        if !chars[i].is_ascii_lowercase() {
            continue;
        }
        for &letter in &alphabet {
            if letter == chars[i] {
                continue;
            }
            let mut cand: Vec<char> = chars.clone();
            cand[i] = letter;
            out.push(cand.iter().collect());
        }
    }
    // Insertions.
    for i in 0..=chars.len() {
        for &letter in &alphabet {
            let mut cand: String = chars[..i].iter().collect();
            cand.push(letter);
            cand.extend(chars[i..].iter());
            out.push(cand);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn buf_with(text: &str) -> Buffer {
        let mut b = Buffer::empty();
        b.rope = Rope::from_str(text);
        b
    }

    #[test]
    fn tokenizer_splits_on_punctuation() {
        let t = tokens("hello, world! it's a 'test' — yep");
        let texts: Vec<&str> = t.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["hello", "world", "it's", "a", "test", "yep"]);
    }

    #[test]
    fn tokenizer_offsets_are_char_columns() {
        let t = tokens("  hello there");
        assert_eq!(t[0].text, "hello");
        assert_eq!(t[0].char_offset, 2);
        assert_eq!(t[1].text, "there");
        assert_eq!(t[1].char_offset, 8);
    }

    #[test]
    fn identifier_splits_camel_and_snake() {
        let parts = split_identifier("getPlayerName");
        assert_eq!(parts, vec!["get", "Player", "Name"]);

        let parts = split_identifier("player_name");
        assert_eq!(parts, vec!["player", "name"]);

        let parts = split_identifier("kebab-case-thing");
        assert_eq!(parts, vec!["kebab", "case", "thing"]);
    }

    #[test]
    fn identifier_keeps_acronym_runs() {
        let parts = split_identifier("parseURLPath");
        assert_eq!(parts, vec!["parse", "URL", "Path"]);
    }

    #[test]
    fn check_candidate_filters_short_and_all_caps() {
        assert!(!is_check_candidate("ok"));
        assert!(!is_check_candidate("HTTP"));
        assert!(is_check_candidate("hello"));
    }

    #[test]
    fn enumerate_edits_includes_obvious_neighbours() {
        let edits = enumerate_edits("teh");
        // Transposition produces "the".
        assert!(edits.iter().any(|s| s == "the"));
        // Single-letter deletion produces "eh", "th", "te".
        assert!(edits.iter().any(|s| s == "eh"));
        assert!(edits.iter().any(|s| s == "te"));
    }

    #[test]
    fn check_buffer_skips_when_no_wordlist() {
        // We don't know which environment the test runs in. If
        // /usr/share/dict/words doesn't exist, check_buffer returns
        // an empty list regardless of input — that's the documented
        // contract. We only assert that the call doesn't panic.
        let buf = buf_with("hello world tehe\nthis is some text\n");
        let _ = check_buffer(&buf);
    }
}
