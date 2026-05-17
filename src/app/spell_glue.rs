//! Spell-check orchestration sitting on `App`. The pure check / suggestion
//! engine lives in `crate::spell`; this file owns the per-buffer enable
//! flag, the version-keyed misspelling cache, the `]s` / `[s` navigation,
//! and the `z=` suggestion picker.
//!
//! Activation is per-buffer (keyed by path) so prose buffers can have it
//! on while source-code buffers stay off — there's no global toggle. The
//! cache is rebuilt lazily in the render loop the first time `]s` /
//! `[s` / a status read needs it; recomputation happens when
//! `buffer.version` advances past the cached value.

use crate::picker::{PickerKind, PickerPayload, PickerState};
use crate::spell::{check_buffer, is_known, suggestions, SpellRange};

impl super::App {
    /// `:spell` — flip the enable flag for the active buffer.
    pub(super) fn cmd_spell_toggle(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "spell: no path on this buffer".into();
            return;
        };
        if self.spell_enabled.remove(&path) {
            self.spell_cache.remove(&path);
            self.status_msg = "spell: off".into();
        } else {
            if crate::spell::wordlist().is_none() {
                self.status_msg =
                    "spell: no wordlist (install /usr/share/dict/words or write \
                     ~/.local/share/binvim/words)"
                        .into();
                return;
            }
            self.spell_enabled.insert(path.clone());
            self.refresh_spell_cache();
            let count = self
                .spell_cache
                .get(&path)
                .map(|(_, r)| r.len())
                .unwrap_or(0);
            self.status_msg = format!("spell: on ({count} flagged)");
        }
    }

    /// Rebuild the active buffer's misspelling cache when the cached
    /// version is stale (or absent). No-op when spell isn't enabled
    /// for the buffer or it has no path.
    pub(super) fn refresh_spell_cache(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return };
        if !self.spell_enabled.contains(&path) {
            return;
        }
        let v = self.buffer.version;
        if let Some((cached_v, _)) = self.spell_cache.get(&path) {
            if *cached_v == v {
                return;
            }
        }
        let ranges = check_buffer(&self.buffer);
        self.spell_cache.insert(path, (v, ranges));
    }

    /// `]s` — jump to the next misspelling. Wraps from end of buffer
    /// back to the top. No-op when spell is off or the cache is empty.
    pub(super) fn cmd_spell_next(&mut self) {
        self.spell_jump(true);
    }

    /// `[s` — jump to the previous misspelling. Wraps from start of
    /// buffer to the bottom.
    pub(super) fn cmd_spell_prev(&mut self) {
        self.spell_jump(false);
    }

    fn spell_jump(&mut self, forward: bool) {
        let Some(path) = self.buffer.path.clone() else { return };
        if !self.spell_enabled.contains(&path) {
            self.status_msg = "spell: not enabled (`:spell` to toggle)".into();
            return;
        }
        self.refresh_spell_cache();
        let ranges = match self.spell_cache.get(&path) {
            Some((_, r)) if !r.is_empty() => r.clone(),
            _ => {
                self.status_msg = "spell: clean".into();
                return;
            }
        };
        let cur_line = self.window.cursor.line;
        let cur_col = self.window.cursor.col;
        let target = if forward {
            // First range strictly after the cursor; else first range
            // in the buffer (wrap).
            ranges
                .iter()
                .find(|r| {
                    r.line > cur_line || (r.line == cur_line && r.col > cur_col)
                })
                .cloned()
                .or_else(|| ranges.first().cloned())
        } else {
            // Last range strictly before the cursor; else last range
            // in the buffer (wrap).
            ranges
                .iter()
                .rev()
                .find(|r| {
                    r.line < cur_line || (r.line == cur_line && r.col < cur_col)
                })
                .cloned()
                .or_else(|| ranges.last().cloned())
        };
        let Some(r) = target else { return };
        self.window.cursor.line = r.line;
        self.window.cursor.col = r.col;
    }

    /// `z=` — open a picker of single-edit suggestions for the word
    /// under the cursor. Accepting one substitutes the word in-place.
    pub(super) fn cmd_spell_suggest(&mut self) {
        let Some(word) = self.word_under_cursor() else {
            self.status_msg = "spell: no word at cursor".into();
            return;
        };
        if is_known(&word) {
            self.status_msg = format!("spell: `{word}` is known");
            return;
        }
        let candidates = suggestions(&word, 12);
        if candidates.is_empty() {
            self.status_msg = format!("spell: no suggestions for `{word}`");
            return;
        }
        let items: Vec<(String, PickerPayload)> = candidates
            .into_iter()
            .map(|s| {
                (
                    s.clone(),
                    PickerPayload::SpellSuggestion {
                        word: word.clone(),
                        replacement: s,
                    },
                )
            })
            .collect();
        self.picker = Some(PickerState::new(
            PickerKind::SpellSuggestions,
            format!("spell: suggestions for `{word}`"),
            items,
        ));
        self.mode = crate::mode::Mode::Picker;
    }

    /// Accept a chosen suggestion — replaces the word at the cursor
    /// with `replacement`. Called from the picker dispatch when the
    /// user hits Enter on a `SpellSuggestion` row.
    pub(super) fn apply_spell_suggestion(&mut self, word: &str, replacement: &str) {
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        let line_text = self.buffer.rope.line(line).to_string();
        // Find the bounds of the word containing the cursor column.
        let chars: Vec<char> = line_text.chars().collect();
        let mut start = col.min(chars.len());
        while start > 0
            && chars[start - 1]
                .is_alphabetic()
        {
            start -= 1;
        }
        let mut end = start;
        while end < chars.len() && chars[end].is_alphabetic() {
            end += 1;
        }
        let captured: String = chars[start..end].iter().collect();
        if captured.to_ascii_lowercase() != word.to_ascii_lowercase() {
            self.status_msg =
                "spell: word at cursor changed; re-run `z=`".into();
            return;
        }
        let line_start = self.buffer.rope.line_to_char(line);
        let from = line_start + start;
        let to = line_start + end;
        let removed = self.buffer.delete_range(from, to);
        // Preserve the original capitalisation pattern when possible:
        // if the user wrote `Helo` and accepts `hello`, replace with
        // `Hello`. Only handles the common "first letter capital"
        // case; all-caps falls through as-is.
        let mut final_repl = replacement.to_string();
        if captured
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            if let Some(first) = final_repl.chars().next() {
                let mut up = first.to_uppercase().to_string();
                up.push_str(&final_repl[first.len_utf8()..]);
                final_repl = up;
            }
        }
        self.buffer.insert_at_idx(from, &final_repl);
        // Cursor stays at original position (which is now within or
        // at the start of the replacement). Bump the cache so the
        // next render recomputes ranges against the new text.
        let _ = removed;
        self.refresh_spell_cache();
    }

    /// Number of misspellings currently flagged on the active buffer.
    /// Returns 0 when spell is off or the cache is empty. Used by the
    /// status-line painter.
    #[allow(dead_code)]
    pub(super) fn spell_misspelling_count(&self) -> usize {
        let Some(path) = self.buffer.path.as_ref() else { return 0 };
        if !self.spell_enabled.contains(path) {
            return 0;
        }
        self.spell_cache.get(path).map(|(_, r)| r.len()).unwrap_or(0)
    }

    /// Misspelling ranges for the active buffer, if spell is on and
    /// the cache is fresh. Used by the renderer to paint undercurls.
    #[allow(dead_code)]
    pub(super) fn spell_ranges_for_render(&self) -> Option<&[SpellRange]> {
        let path = self.buffer.path.as_ref()?;
        if !self.spell_enabled.contains(path) {
            return None;
        }
        let (v, ranges) = self.spell_cache.get(path)?;
        if *v != self.buffer.version {
            return None;
        }
        Some(ranges.as_slice())
    }
}
