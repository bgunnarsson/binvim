//! Git working-tree diff against the index/HEAD — used to paint a per-line
//! "added / modified / deleted" stripe in the gutter and (later) to drive
//! hunk navigation, preview, and stage/unstage actions. Shells out to the
//! `git` binary rather than linking libgit2 to keep the dependency surface
//! flat. `unified=0` so we get one hunk per contiguous change with no
//! surrounding context muddying the line math.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHunkKind {
    Added,
    Modified,
    Deleted,
}

/// One contiguous run of changed (or deletion-marker) lines in the
/// working-tree diff. `start_line`/`end_line` are 0-indexed and refer to
/// the *new* file. For `Deleted` hunks the range is a single line — the
/// line at which the deletion sign should be painted (the line after
/// where the removed content used to be).
#[derive(Debug, Clone, Copy)]
pub struct GitHunk {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: GitHunkKind,
}

/// Walk up from `start` looking for a `.git` entry; return the repo root
/// (the directory that contains `.git`). Mirrors the search in
/// `save::detect_git_branch` but returns the directory rather than the
/// branch label, so hunk + branch detection can share the same traversal
/// in a future refactor.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        let parent = dir.parent()?.to_path_buf();
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// Run `git diff --no-color --unified=0 -- <path>` from the repo root and
/// parse its hunks. Returns `None` when `path` isn't inside a git repo,
/// or when the git binary fails (missing, non-zero, etc.) — both cases
/// are equivalent to "no signs to paint".
pub fn diff_against_worktree(path: &Path) -> Option<Vec<GitHunk>> {
    let start = path.parent()?;
    let root = find_repo_root(start)?;
    let rel = path.strip_prefix(&root).ok()?;

    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("diff")
        .arg("--no-color")
        .arg("--unified=0")
        .arg("--")
        .arg(rel)
        .output()
        .ok()?;
    // git-diff exits 0 even when there are changes; a non-zero exit
    // means a real error (binary file, untracked path, …) — treat as
    // "no signs to paint" rather than propagating.
    if !output.status.success() {
        return Some(Vec::new());
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(parse_unified_diff(&text))
}

/// Parse the headers of a `--unified=0` diff into per-line hunk markers.
/// Skips file-level metadata; only `@@ -A,B +C,D @@` headers contribute.
pub fn parse_unified_diff(diff: &str) -> Vec<GitHunk> {
    let mut out = Vec::new();
    for line in diff.lines() {
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let old_tok = parts.next().unwrap_or("");
        let new_tok = parts.next().unwrap_or("");
        let (_, old_count) = parse_range(old_tok.trim_start_matches('-'));
        let (new_start, new_count) = parse_range(new_tok.trim_start_matches('+'));

        match (old_count, new_count) {
            (0, n) if n > 0 => {
                // Pure addition — N new lines starting at new_start.
                let s = new_start.saturating_sub(1);
                out.push(GitHunk {
                    start_line: s,
                    end_line: s + n - 1,
                    kind: GitHunkKind::Added,
                });
            }
            (m, 0) if m > 0 => {
                // Pure deletion — paint the marker on the line that now
                // sits at the deletion point. `new_start` here is the
                // line *before* the missing block, so we attach to that
                // 0-indexed line (clamped to 0 if the deletion was at
                // the very top of the file).
                let line = new_start.saturating_sub(1).max(0);
                out.push(GitHunk {
                    start_line: line,
                    end_line: line,
                    kind: GitHunkKind::Deleted,
                });
            }
            (_, n) if n > 0 => {
                // Mixed change — N lines exist in the new file. Mark
                // every one as Modified regardless of whether the old
                // count was higher or lower.
                let s = new_start.saturating_sub(1);
                out.push(GitHunk {
                    start_line: s,
                    end_line: s + n - 1,
                    kind: GitHunkKind::Modified,
                });
            }
            _ => {}
        }
    }
    out
}

/// Return the unified-diff body (header + body lines, no file metadata)
/// of the hunk that covers `target_line` (1-indexed, new file). Runs
/// `git diff -U3` so the slice includes three context lines, which is
/// what most preview popups want. `None` when the path isn't tracked,
/// when there's no hunk at that line, or when git fails.
pub fn hunk_text_for_line(path: &Path, target_line: usize) -> Option<String> {
    let start = path.parent()?;
    let root = find_repo_root(start)?;
    let rel = path.strip_prefix(&root).ok()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("diff")
        .arg("--no-color")
        .arg("--unified=3")
        .arg("--")
        .arg(rel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    let mut header_indices: Vec<(usize, usize, usize)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let mut parts = rest.split_whitespace();
            let _old = parts.next();
            let new_tok = parts.next().unwrap_or("");
            let (new_start, new_count) = parse_range(new_tok.trim_start_matches('+'));
            header_indices.push((idx, new_start, new_count));
        }
    }
    for (i, &(idx, new_start, new_count)) in header_indices.iter().enumerate() {
        let end_inclusive = new_start + new_count.saturating_sub(1);
        if target_line >= new_start && target_line <= end_inclusive.max(new_start) {
            // Body ends at the next header, or at EOF.
            let next_idx = header_indices.get(i + 1).map(|&(j, _, _)| j);
            let body: Vec<&str> = text
                .lines()
                .skip(idx)
                .take(next_idx.map(|n| n - idx).unwrap_or(usize::MAX))
                .collect();
            return Some(body.join("\n"));
        }
    }
    None
}

/// Return the precise (unified=0) hunk body covering `target_line`, plus
/// the repo root and relative path. Used to build a synthetic patch for
/// `git apply` — we need the zero-context form so the apply succeeds
/// against the current working tree / index.
pub fn unidiff_zero_hunk_for_line(
    path: &Path,
    target_line: usize,
    cached: bool,
) -> Option<(PathBuf, PathBuf, String)> {
    let start = path.parent()?;
    let root = find_repo_root(start)?;
    let rel = path.strip_prefix(&root).ok()?.to_path_buf();
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&root).arg("diff").arg("--no-color").arg("--unified=0");
    if cached {
        cmd.arg("--cached");
    }
    cmd.arg("--").arg(&rel);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    // Find the hunk containing target_line in the new-side range.
    let mut headers: Vec<(usize, usize, usize)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let mut parts = rest.split_whitespace();
            let _old = parts.next();
            let new_tok = parts.next().unwrap_or("");
            let (new_start, new_count) = parse_range(new_tok.trim_start_matches('+'));
            headers.push((idx, new_start, new_count));
        }
    }
    for (i, &(idx, new_start, new_count)) in headers.iter().enumerate() {
        // For pure deletions new_count == 0 — treat target_line == new_start
        // as a hit so the user can reset a deletion they're standing on.
        let in_range = if new_count == 0 {
            target_line == new_start || target_line == new_start.saturating_sub(1).max(0)
        } else {
            target_line >= new_start && target_line < new_start + new_count
        };
        if in_range {
            let next_idx = headers.get(i + 1).map(|&(j, _, _)| j);
            let body: Vec<&str> = text
                .lines()
                .skip(idx)
                .take(next_idx.map(|n| n - idx).unwrap_or(usize::MAX))
                .collect();
            return Some((root, rel, body.join("\n")));
        }
    }
    None
}

/// Build a one-file unified diff suitable for `git apply --unidiff-zero`.
/// `rel` is the path relative to the repo root; `hunk` is the `@@ … @@`
/// header plus its body lines (no file metadata yet).
pub fn build_patch(rel: &Path, hunk: &str) -> String {
    let path_str = rel.display();
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{hunk}\n",
        path = path_str,
        hunk = hunk.trim_end()
    )
}

/// Pipe `patch` through `git -C <root> apply <args…>` on stdin. Returns
/// `Ok(())` on success, or `Err(stderr)` so the caller can surface a
/// useful status message.
pub fn apply_patch(root: &Path, patch: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).arg("apply");
    for a in args {
        cmd.arg(a);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| "git apply: stdin missing".to_string())?;
        stdin
            .write_all(patch.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if msg.is_empty() {
            "git apply: failed".into()
        } else {
            msg
        })
    }
}

/// `<start>[,<count>]` → `(start, count)`. Missing count defaults to 1,
/// matching the unified-diff convention.
fn parse_range(s: &str) -> (usize, usize) {
    let mut iter = s.split(',');
    let start = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let count = iter.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    (start, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pure_addition() {
        let h = parse_unified_diff("@@ -10,0 +11,3 @@\n");
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Added));
        assert_eq!(h[0].start_line, 10);
        assert_eq!(h[0].end_line, 12);
    }

    #[test]
    fn parses_pure_deletion() {
        let h = parse_unified_diff("@@ -5,2 +4,0 @@\n");
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Deleted));
        assert_eq!(h[0].start_line, 3);
        assert_eq!(h[0].end_line, 3);
    }

    #[test]
    fn parses_modification() {
        let h = parse_unified_diff("@@ -7,2 +7,2 @@\n");
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Modified));
        assert_eq!(h[0].start_line, 6);
        assert_eq!(h[0].end_line, 7);
    }

    #[test]
    fn parses_count_defaults_to_one() {
        let h = parse_unified_diff("@@ -5 +5 @@\n");
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Modified));
        assert_eq!(h[0].start_line, 4);
        assert_eq!(h[0].end_line, 4);
    }

    #[test]
    fn deletion_at_file_top_clamps_to_line_zero() {
        let h = parse_unified_diff("@@ -1,3 +0,0 @@\n");
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Deleted));
        assert_eq!(h[0].start_line, 0);
    }

    #[test]
    fn skips_non_hunk_lines() {
        let diff = "\
diff --git a/foo b/foo
index 1..2 100644
--- a/foo
+++ b/foo
@@ -1,1 +1,1 @@
-old
+new
";
        let h = parse_unified_diff(diff);
        assert_eq!(h.len(), 1);
        assert!(matches!(h[0].kind, GitHunkKind::Modified));
    }

    #[test]
    fn parses_multiple_hunks() {
        let diff = "\
@@ -2,0 +3,1 @@
+added
@@ -10,1 +12,0 @@
-removed
@@ -20,1 +21,1 @@
-old
+new
";
        let h = parse_unified_diff(diff);
        assert_eq!(h.len(), 3);
        assert!(matches!(h[0].kind, GitHunkKind::Added));
        assert!(matches!(h[1].kind, GitHunkKind::Deleted));
        assert!(matches!(h[2].kind, GitHunkKind::Modified));
    }
}
