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
/// Working-tree summary for the dashboard.
#[derive(Debug, Clone, Default)]
pub struct GitStatusSummary {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub modified: usize,
    pub untracked: usize,
}

/// Run `git status --porcelain=v1 -b` and pull out branch / upstream /
/// ahead-behind / modified / untracked counts. Returns `None` when the
/// directory isn't in a git repo or git isn't on `$PATH`.
///
/// Light-touch parser — `## main...origin/main [ahead 2, behind 1]`
/// on the first line, then one short-status line per affected file.
/// `?? ` prefix is untracked; anything else with a non-blank prefix
/// counts as modified for the dashboard's purpose.
pub fn status_summary(start: &Path) -> Option<GitStatusSummary> {
    let root = find_repo_root(start)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("status")
        .arg("--porcelain=v1")
        .arg("-b")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut out = GitStatusSummary::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // `<branch>` or `<branch>...<upstream> [ahead N, behind N]`.
            let (head, track) = match rest.find(' ') {
                Some(i) => (&rest[..i], Some(&rest[i + 1..])),
                None => (rest, None),
            };
            let (branch, upstream) = match head.find("...") {
                Some(i) => (&head[..i], Some(head[i + 3..].to_string())),
                None => (head, None),
            };
            out.branch = Some(branch.to_string());
            out.upstream = upstream;
            if let Some(t) = track {
                if let Some(idx) = t.find("ahead ") {
                    out.ahead = parse_count(&t[idx + 6..]);
                }
                if let Some(idx) = t.find("behind ") {
                    out.behind = parse_count(&t[idx + 7..]);
                }
            }
        } else if line.starts_with("?? ") {
            out.untracked += 1;
        } else if !line.is_empty() {
            out.modified += 1;
        }
    }
    Some(out)
}

fn parse_count(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

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
        .arg("--diff-algorithm=histogram")
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
        .arg("--diff-algorithm=histogram")
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
    cmd.arg("-C")
        .arg(&root)
        .arg("diff")
        .arg("--no-color")
        .arg("--unified=0")
        .arg("--diff-algorithm=histogram");
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

/// One line of blame metadata — what we render as inline virtual text.
/// `age` is a short relative date label (`"3d"`, `"2w"`, `"4mo"`).
#[derive(Debug, Clone)]
pub struct BlameLine {
    pub sha: String,
    pub author: String,
    pub age: String,
}

/// Run `git blame --porcelain -- <path>` and return one `BlameLine` per
/// 0-indexed line, or `None` if the file isn't tracked / git fails.
/// Porcelain output groups records by commit, so we keep a side table
/// keyed by SHA and emit a `BlameLine` for each line as it arrives.
pub fn blame(path: &Path) -> Option<Vec<BlameLine>> {
    let start = path.parent()?;
    let root = find_repo_root(start)?;
    let rel = path.strip_prefix(&root).ok()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("blame")
        .arg("--porcelain")
        .arg("--")
        .arg(rel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;

    // Commit-keyed metadata: author name + author-time epoch.
    let mut meta: std::collections::HashMap<String, (String, i64)> =
        std::collections::HashMap::new();
    let mut current_sha: Option<String> = None;
    let mut lines: Vec<BlameLine> = Vec::new();
    let mut iter = text.lines();
    while let Some(line) = iter.next() {
        // Header lines look like `<sha> <orig-line> <final-line>[ <count>]`.
        // Any other key/value lines come after — `author Foo Bar`,
        // `author-time 1700000000`, etc. The body line itself is
        // prefixed with a literal tab.
        if line.starts_with('\t') {
            // Body line — emit a record for the current sha.
            if let Some(sha) = &current_sha {
                let (author, t) = meta
                    .get(sha)
                    .cloned()
                    .unwrap_or_else(|| ("?".to_string(), now));
                lines.push(BlameLine {
                    sha: sha.chars().take(7).collect(),
                    author,
                    age: format_age((now - t).max(0)),
                });
            }
            continue;
        }
        // Header line — only when the first token is a 40-char hex SHA.
        let mut parts = line.splitn(2, ' ');
        if let (Some(first), Some(_rest)) = (parts.next(), parts.next()) {
            if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
                current_sha = Some(first.to_string());
                meta.entry(first.to_string())
                    .or_insert_with(|| ("?".to_string(), now));
                continue;
            }
        }
        // Key/value metadata for the most recent header.
        if let (Some(key), Some(val)) = (line.split_whitespace().next(), line.split_once(' ')) {
            let value = val.1;
            if let Some(sha) = &current_sha {
                let entry = meta
                    .entry(sha.clone())
                    .or_insert_with(|| ("?".to_string(), now));
                match key {
                    "author" => entry.0 = value.to_string(),
                    "author-time" => {
                        if let Ok(n) = value.parse::<i64>() {
                            entry.1 = n;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Some(lines)
}

/// Compact relative age — `45s`, `12m`, `3h`, `5d`, `2w`, `4mo`, `2y`.
/// Optimised for at-a-glance reading, not precision.
fn format_age(seconds: i64) -> String {
    let s = seconds.max(0) as u64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h");
    }
    let d = h / 24;
    if d < 7 {
        return format!("{d}d");
    }
    if d < 30 {
        return format!("{}w", d / 7);
    }
    if d < 365 {
        return format!("{}mo", d / 30);
    }
    format!("{}y", d / 365)
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
    fn format_age_buckets() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(30), "30s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3600), "1h");
        assert_eq!(format_age(86_400), "1d");
        assert_eq!(format_age(8 * 86_400), "1w");
        assert_eq!(format_age(40 * 86_400), "1mo");
        assert_eq!(format_age(400 * 86_400), "1y");
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
