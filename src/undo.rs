use crate::cursor::Cursor;
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Snapshot {
    pub rope: Rope,
    pub cursor: Cursor,
}

/// One state in the undo tree. Its `snap` is filled in when the state is
/// left — the current state is the buffer itself — so every other node has
/// one.
#[derive(Clone)]
struct Node {
    snap: Option<Snapshot>,
    parent: Option<usize>,
    /// The child `redo` goes to: the one most recently made or left.
    redo_child: Option<usize>,
    /// When the state was made, for `:earlier` / `:later` by time.
    made: Instant,
    /// The file write that saved it, for `:earlier` / `:later` by writes.
    written: Option<usize>,
}

/// Undo history as a tree (D5): an edit after an undo starts a new branch and
/// keeps the old one. Nodes are numbered in the order their states were made,
/// which `g-` / `g+` walk. Only the current branch is persisted, in the linear
/// `past` / `future` form the files have always had.
#[derive(Default, Clone)]
pub struct History {
    nodes: Vec<Node>,
    cur: usize,
    /// File writes so far this session.
    writes: usize,
}

/// One `:undolist` row: the last state of a branch.
#[derive(Debug, Clone)]
pub struct Leaf {
    pub number: usize,
    /// How many changes from the oldest state.
    pub changes: usize,
    pub age: Duration,
    pub written: Option<usize>,
}

/// Cap so a long-running session doesn't OOM. 1000 is plenty — anything
/// older is academically interesting at best.
const MAX_STATES: usize = 1000;

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save the current state before a mutation.
    pub fn record(&mut self, rope: &Rope, cursor: Cursor) {
        if self.nodes.is_empty() {
            self.nodes.push(Node {
                snap: None,
                parent: None,
                redo_child: None,
                made: Instant::now(),
                written: None,
            });
            self.cur = 0;
        }
        let child = self.nodes.len();
        let node = &mut self.nodes[self.cur];
        node.snap = Some(Snapshot {
            rope: rope.clone(),
            cursor,
        });
        node.redo_child = Some(child);
        self.nodes.push(Node {
            snap: None,
            parent: Some(self.cur),
            redo_child: None,
            made: Instant::now(),
            written: None,
        });
        self.cur = child;
        if self.nodes.len() > MAX_STATES {
            self.drop_oldest();
        }
    }

    /// The oldest state gone and every number after it one lower; what hung
    /// off it becomes a root.
    fn drop_oldest(&mut self) {
        self.nodes.remove(0);
        let shift = |i: Option<usize>| i.and_then(|i| i.checked_sub(1));
        for node in &mut self.nodes {
            node.parent = shift(node.parent);
            node.redo_child = shift(node.redo_child);
        }
        self.cur = self.cur.saturating_sub(1);
    }

    /// The current state's ancestors, nearest first.
    fn ancestors(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = self.nodes.get(self.cur).and_then(|node| node.parent);
        while let Some(i) = at {
            out.push(i);
            at = self.nodes[i].parent;
        }
        out
    }

    /// How many steps `undo` can take back.
    pub fn depth(&self) -> usize {
        self.ancestors().len()
    }

    /// Everything recorded since the history was `depth` steps deep made one
    /// step: from the state before any of those changes straight to now. The
    /// states in between go too when they're the newest, so `g-` doesn't stop
    /// at them either.
    pub fn squash_since(&mut self, depth: usize) {
        let path = self.ancestors();
        let Some(split) = path.len().checked_sub(depth + 1) else {
            return;
        };
        let keep = path[split];
        let mut between: Vec<usize> = path[..split].to_vec();
        if between.is_empty() {
            return;
        }
        between.sort_unstable();
        let first = self.cur - between.len();
        let newest =
            self.cur + 1 == self.nodes.len() && between.iter().copied().eq(first..self.cur);
        if newest {
            let mut node = self.nodes.pop().expect("the current state is the last");
            self.nodes.truncate(first);
            node.parent = Some(keep);
            self.nodes.push(node);
            self.cur = first;
        } else {
            self.nodes[self.cur].parent = Some(keep);
        }
        self.nodes[keep].redo_child = Some(self.cur);
    }

    /// The current state, as it is now, kept on its node before moving off it.
    fn leave(&mut self, rope: &Rope, cursor: Cursor) {
        self.nodes[self.cur].snap = Some(Snapshot {
            rope: rope.clone(),
            cursor,
        });
    }

    /// Undo: back to the state this one was made from.
    pub fn undo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let parent = self.nodes.get(self.cur)?.parent?;
        let from = self.cur;
        self.leave(current_rope, current_cursor);
        self.nodes[parent].redo_child = Some(from);
        self.cur = parent;
        self.nodes[parent].snap.clone()
    }

    pub fn redo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let child = self.nodes.get(self.cur)?.redo_child?;
        self.leave(current_rope, current_cursor);
        self.cur = child;
        self.nodes[child].snap.clone()
    }

    /// `g-`: the state made just before this one, on whichever branch.
    pub fn earlier(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        if self.nodes.is_empty() || self.cur == 0 {
            return None;
        }
        self.leave(current_rope, current_cursor);
        self.cur -= 1;
        self.nodes[self.cur].snap.clone()
    }

    /// `g+`: the state made just after this one.
    pub fn later(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        if self.cur + 1 >= self.nodes.len() {
            return None;
        }
        self.leave(current_rope, current_cursor);
        self.cur += 1;
        self.nodes[self.cur].snap.clone()
    }

    /// The current state was just written to the file: it's that write's.
    pub fn mark_written(&mut self) {
        if self.nodes.is_empty() {
            self.nodes.push(Node {
                snap: None,
                parent: None,
                redo_child: None,
                made: Instant::now(),
                written: None,
            });
            self.cur = 0;
        }
        self.writes += 1;
        self.nodes[self.cur].written = Some(self.writes);
    }

    /// `:earlier` / `:later` by time: the newest state made at least `by`
    /// before this one — or, `later`, the oldest made at least `by` after it —
    /// measured from the current state's time, as Vim's are.
    pub fn target_by_time(&self, by: Duration, earlier: bool) -> Option<usize> {
        let now = self.nodes.get(self.cur)?.made;
        if earlier {
            let found = now
                .checked_sub(by)
                .and_then(|limit| (0..self.cur).rev().find(|&i| self.nodes[i].made <= limit));
            return Some(found.unwrap_or(0));
        }
        let limit = now.checked_add(by)?;
        let found = (self.cur + 1..self.nodes.len()).find(|&i| self.nodes[i].made >= limit);
        Some(found.unwrap_or(self.nodes.len() - 1))
    }

    /// `:earlier Nf` / `:later Nf`: the state `count` file writes back or on.
    /// From a state changed since the last write, the first one back is that
    /// write; past the first write is the oldest state, as in Vim.
    pub fn target_by_writes(&self, count: usize, earlier: bool) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let count = count.max(1);
        let written = (0..self.nodes.len()).filter(|&i| self.nodes[i].written.is_some());
        if earlier {
            let back: Vec<usize> = written.filter(|&i| i < self.cur).collect();
            return Some(back.len().checked_sub(count).map_or(0, |k| back[k]));
        }
        let on: Vec<usize> = written.filter(|&i| i > self.cur).collect();
        Some(on.get(count - 1).copied().unwrap_or(self.nodes.len() - 1))
    }

    /// Straight to state `target`, keeping the current one on its node.
    pub fn jump(
        &mut self,
        target: usize,
        current_rope: &Rope,
        current_cursor: Cursor,
    ) -> Option<Snapshot> {
        if target == self.cur || target >= self.nodes.len() {
            return None;
        }
        self.leave(current_rope, current_cursor);
        self.cur = target;
        self.nodes[target].snap.clone()
    }

    /// `:undolist`'s rows: the last state of every branch, oldest first.
    pub fn leaves(&self) -> Vec<Leaf> {
        let mut has_child = vec![false; self.nodes.len()];
        for node in &self.nodes {
            if let Some(parent) = node.parent {
                has_child[parent] = true;
            }
        }
        let now = Instant::now();
        (1..self.nodes.len())
            .filter(|&i| !has_child[i])
            .map(|i| {
                let mut changes = 0;
                let mut at = self.nodes[i].parent;
                while let Some(parent) = at {
                    changes += 1;
                    at = self.nodes[parent].parent;
                }
                Leaf {
                    number: i,
                    changes,
                    age: now.saturating_duration_since(self.nodes[i].made),
                    written: self.nodes[i].written,
                }
            })
            .collect()
    }

    /// The current branch as the linear stacks the files hold: the states
    /// before this one, oldest first, and the ones `redo` reaches, the next
    /// one last.
    fn branch(&self) -> (Vec<&Snapshot>, Vec<&Snapshot>) {
        let past = self
            .ancestors()
            .iter()
            .rev()
            .filter_map(|&i| self.nodes[i].snap.as_ref())
            .collect();
        let mut future = Vec::new();
        let mut at = self.nodes.get(self.cur).and_then(|node| node.redo_child);
        while let Some(i) = at {
            if let Some(snap) = self.nodes[i].snap.as_ref() {
                future.push(snap);
            }
            at = self.nodes[i].redo_child;
        }
        future.reverse();
        (past, future)
    }

    /// A tree holding one branch: `past` oldest first, then the live state,
    /// then `future` — a stack, its last the next redo.
    fn from_linear(past: Vec<Snapshot>, future: Vec<Snapshot>) -> Self {
        if past.is_empty() && future.is_empty() {
            return Self::default();
        }
        let cur = past.len();
        let snaps = past
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
            .chain(future.into_iter().rev().map(Some));
        let mut nodes: Vec<Node> = Vec::new();
        for (i, snap) in snaps.enumerate() {
            if let Some(prev) = i.checked_sub(1) {
                nodes[prev].redo_child = Some(i);
            }
            nodes.push(Node {
                snap,
                parent: i.checked_sub(1),
                redo_child: None,
                made: Instant::now(),
                written: None,
            });
        }
        Self {
            nodes,
            cur,
            writes: 0,
        }
    }

    /// Persist the current branch to `path` along with `file_hash`. We store
    /// the hash so a subsequent load can reject undo state that was recorded
    /// against a different version of the underlying file (someone edited it
    /// externally between sessions).
    pub fn save_to_path(&self, path: &Path, file_hash: u64) -> std::io::Result<()> {
        let (past, future) = self.branch();
        let stored = StoredHistory {
            file_hash,
            past: past.into_iter().map(StoredSnapshot::from).collect(),
            future: future.into_iter().map(StoredSnapshot::from).collect(),
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut tmp = path.to_path_buf();
        tmp.set_extension("tmp");
        let serialized = match serde_json::to_vec(&stored) {
            Ok(v) => v,
            Err(e) => return Err(std::io::Error::other(e)),
        };
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serialized)?;
        f.sync_all()?;
        std::fs::rename(tmp, path)
    }

    /// Inverse of `save_to_path`. Returns `None` if the file is missing,
    /// malformed, or stamped with a different `file_hash`.
    pub fn load_from_path(path: &Path, expected_hash: u64) -> Option<Self> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        let stored: StoredHistory = serde_json::from_slice(&buf).ok()?;
        if stored.file_hash != expected_hash {
            return None;
        }
        Some(Self::from_linear(
            stored.past.iter().map(Snapshot::from).collect(),
            stored.future.iter().map(Snapshot::from).collect(),
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct StoredHistory {
    file_hash: u64,
    past: Vec<StoredSnapshot>,
    future: Vec<StoredSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct StoredSnapshot {
    text: String,
    line: usize,
    col: usize,
    want_col: usize,
}

impl From<&Snapshot> for StoredSnapshot {
    fn from(s: &Snapshot) -> Self {
        Self {
            text: s.rope.to_string(),
            line: s.cursor.line,
            col: s.cursor.col,
            want_col: s.cursor.want_col,
        }
    }
}

impl From<&StoredSnapshot> for Snapshot {
    fn from(s: &StoredSnapshot) -> Self {
        Self {
            rope: Rope::from_str(&s.text),
            cursor: Cursor {
                line: s.line,
                col: s.col,
                want_col: s.want_col,
            },
        }
    }
}

/// Hash a file's contents. Used as the staleness key for persisted undo.
pub fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Resolve the on-disk persisted-undo file for `target` under
/// `<cache>/binvim/undo/`. Returns `None` if the cache dir can't be
/// resolved.
pub fn cache_path_for(target: &Path) -> Option<PathBuf> {
    let canon = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.to_string_lossy().hash(&mut h);
    let id = format!("{:016x}", h.finish());
    let mut p = crate::paths::cache_dir()?;
    p.push("undo");
    p.push(format!("{id}.json"));
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_and_later_by_writes_and_by_time() {
        let r = Rope::from_str;
        let mut history = History::new();
        // Writes at "ab" and "abc", then "abcd" unsaved — as the Vim check had it.
        history.record(&r("a"), at_start());
        history.mark_written();
        history.record(&r("ab"), at_start());
        history.mark_written();
        history.record(&r("abc"), at_start());
        assert_eq!(history.target_by_writes(1, true), Some(2));
        assert_eq!(history.target_by_writes(2, true), Some(1));
        assert_eq!(history.target_by_writes(3, true), Some(0));
        assert_eq!(history.target_by_writes(1, false), Some(3));
        assert_eq!(
            text(history.jump(1, &r("abcd"), at_start())).as_deref(),
            Some("ab")
        );
        assert_eq!(history.target_by_writes(1, false), Some(2));
        assert_eq!(history.target_by_writes(1, true), Some(0));
        // By time, from the current state's time.
        let now = Instant::now();
        for (i, secs) in [(0, 30), (1, 20), (2, 10), (3, 0)] {
            history.nodes[i].made = now.checked_sub(Duration::from_secs(secs)).expect("uptime");
        }
        history.cur = 3;
        assert_eq!(
            history.target_by_time(Duration::from_secs(15), true),
            Some(1)
        );
        history.cur = 0;
        assert_eq!(
            history.target_by_time(Duration::from_secs(15), false),
            Some(2)
        );
        assert_eq!(
            history.target_by_time(Duration::from_secs(900), false),
            Some(3)
        );
        // `:undolist` lists the ends of branches only.
        let leaves: Vec<usize> = history.leaves().iter().map(|leaf| leaf.number).collect();
        assert_eq!(leaves, vec![3]);
    }

    fn at_start() -> Cursor {
        Cursor {
            line: 0,
            col: 0,
            want_col: 0,
        }
    }

    fn text(snap: Option<Snapshot>) -> Option<String> {
        snap.map(|snap| snap.rope.to_string())
    }

    #[test]
    fn an_edit_after_undo_keeps_the_old_branch_for_g_minus_and_plus() {
        let r = Rope::from_str;
        let mut history = History::new();
        history.record(&r(""), at_start());
        history.record(&r("a"), at_start());
        assert_eq!(
            text(history.undo(&r("ab"), at_start())).as_deref(),
            Some("a")
        );
        history.record(&r("a"), at_start());
        // `u` from "ac" reaches "a", never "ab" — `g-` walks by time and does.
        assert_eq!(
            text(history.earlier(&r("ac"), at_start())).as_deref(),
            Some("ab")
        );
        assert_eq!(
            text(history.earlier(&r("ab"), at_start())).as_deref(),
            Some("a")
        );
        assert_eq!(
            text(history.earlier(&r("a"), at_start())).as_deref(),
            Some("")
        );
        assert!(history.earlier(&r(""), at_start()).is_none());
        assert_eq!(
            text(history.later(&r(""), at_start())).as_deref(),
            Some("a")
        );
        assert_eq!(
            text(history.later(&r("a"), at_start())).as_deref(),
            Some("ab")
        );
        assert_eq!(
            text(history.later(&r("ab"), at_start())).as_deref(),
            Some("ac")
        );
        assert!(history.later(&r("ac"), at_start()).is_none());
        assert_eq!(
            text(history.undo(&r("ac"), at_start())).as_deref(),
            Some("a")
        );
        assert_eq!(
            text(history.redo(&r("a"), at_start())).as_deref(),
            Some("ac")
        );
    }

    #[test]
    fn squashed_steps_leave_no_states_for_g_minus() {
        let mut history = History::new();
        for text in ["a", "b", "c"] {
            history.record(&Rope::from_str(text), at_start());
        }
        history.squash_since(0);
        assert_eq!(
            text(history.earlier(&Rope::from_str("d"), at_start())).as_deref(),
            Some("a")
        );
        assert!(history.earlier(&Rope::from_str("a"), at_start()).is_none());
    }

    #[test]
    fn files_in_the_linear_format_load_and_save_unchanged() {
        let snap = |text: &str| StoredSnapshot {
            text: text.into(),
            line: 0,
            col: 0,
            want_col: 0,
        };
        let stored = StoredHistory {
            file_hash: 7,
            past: vec![snap("a"), snap("ab")],
            future: vec![snap("abcd"), snap("abc")],
        };
        let bytes = serde_json::to_vec(&stored).expect("json");
        let dir = std::env::temp_dir().join(format!("binvim-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("history.json");
        std::fs::write(&path, &bytes).expect("write");
        let mut history = History::load_from_path(&path, 7).expect("loads");
        assert_eq!(history.depth(), 2);
        history.save_to_path(&path, 7).expect("saves");
        assert_eq!(std::fs::read(&path).expect("read"), bytes);
        let r = Rope::from_str;
        assert_eq!(
            text(history.undo(&r("abX"), at_start())).as_deref(),
            Some("ab")
        );
        assert_eq!(
            text(history.redo(&r("ab"), at_start())).as_deref(),
            Some("abX")
        );
        assert_eq!(
            text(history.redo(&r("abX"), at_start())).as_deref(),
            Some("abc")
        );
        assert_eq!(
            text(history.redo(&r("abc"), at_start())).as_deref(),
            Some("abcd")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn squashed_steps_undo_as_one() {
        let mut history = History::new();
        let cursor = Cursor {
            line: 0,
            col: 0,
            want_col: 0,
        };
        for text in ["a", "b", "c"] {
            history.record(&Rope::from_str(text), cursor);
        }
        history.squash_since(0);
        assert_eq!(history.depth(), 1);
        let snap = history
            .undo(&Rope::from_str("d"), cursor)
            .expect("one step");
        assert_eq!(snap.rope.to_string(), "a");
        assert!(history.undo(&Rope::from_str("a"), cursor).is_none());
    }
}
