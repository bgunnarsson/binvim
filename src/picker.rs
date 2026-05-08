use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum PickerKind {
    Files,
    Buffers,
}

pub struct PickerState {
    #[allow(dead_code)]
    pub kind: PickerKind,
    pub title: String,
    /// All candidate items in display form (e.g. relative path, buffer name).
    pub items: Vec<String>,
    /// Original payload — for Files this is the absolute path; for Buffers the buffer index.
    pub payloads: Vec<PickerPayload>,
    pub input: String,
    /// Indices into `items`, sorted by descending score.
    pub filtered: Vec<usize>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub enum PickerPayload {
    Path(PathBuf),
    BufferIdx(usize),
}

impl PickerState {
    pub fn new(kind: PickerKind, title: String, items: Vec<(String, PickerPayload)>) -> Self {
        let (display, payloads): (Vec<_>, Vec<_>) = items.into_iter().unzip();
        let filtered: Vec<usize> = (0..display.len()).collect();
        Self {
            kind,
            title,
            items: display,
            payloads,
            input: String::new(),
            filtered,
            selected: 0,
        }
    }

    pub fn refilter(&mut self) {
        if self.input.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(usize, i64)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| fuzzy_match(&self.input, item).map(|s| (i, s)))
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn current(&self) -> Option<&PickerPayload> {
        let item_idx = *self.filtered.get(self.selected)?;
        self.payloads.get(item_idx)
    }

}

/// Subsequence fuzzy match. Bonuses for consecutive runs and word-boundary hits.
/// Returns `None` if not all query chars appear in order.
fn fuzzy_match(query: &str, item: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let item_lower = item.to_lowercase();
    let i_chars: Vec<char> = item_lower.chars().collect();
    let mut qi = 0;
    let mut score: i64 = 0;
    let mut last_idx: i64 = -2;
    for (idx, c) in i_chars.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            // Bonuses
            if last_idx + 1 == idx as i64 {
                score += 6; // consecutive
            }
            if idx == 0 {
                score += 4; // start of string
            } else {
                let prev = i_chars[idx - 1];
                if prev == '/' || prev == '\\' || prev == '_' || prev == '-' || prev == '.' {
                    score += 5; // path separator / word boundary
                }
            }
            score += 1; // base hit
            last_idx = idx as i64;
            qi += 1;
        }
    }
    if qi == q.len() {
        // Length penalty so shorter matches rank higher.
        Some(score - (i_chars.len() as i64 / 8))
    } else {
        None
    }
}

pub fn enumerate_files(root: &std::path::Path, max: usize) -> Vec<(String, PickerPayload)> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .build()
        .flatten()
    {
        if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.into_path();
        let display = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.display().to_string());
        out.push((display, PickerPayload::Path(path)));
        if out.len() >= max {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
