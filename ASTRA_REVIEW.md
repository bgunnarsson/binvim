# Repository Review

Date: 2026-10-03

Found 10 actionable issues in the reviewed paths, including crashes and unintended file changes.

## High severity

1. **Pasting Unicode clipboard text can crash.** [src/app/registers.rs:177](src/app/registers.rs#L177) slices off the final byte before checking for a newline. External clipboard text ending in `é` or an emoji triggers an invalid UTF-8 boundary panic. Use `strip_suffix('\n')`.

2. **Multi-cursor Backspace can crash.** [src/app/edit.rs:110](src/app/edit.rs#L110) looks up original cursor positions after shortening the buffer. With `abc\nxyz`, block-select both lines, press `$A`, then Backspace: position 7 is queried against a five-character buffer. Classify cursors before deletion and remap all positions afterward.

3. **LSP edits use the wrong character offsets.** [src/app/lsp_glue.rs:1803](src/app/lsp_glue.rs#L1803) treats protocol columns as Unicode scalar indexes, despite advertising UTF-8 and UTF-16 positions. Renames after non-ASCII text can replace the wrong characters and save them. Track the negotiated encoding and convert positions both ways.

4. **File-tree rename silently overwrites existing files.** [src/app/file_tree.rs:572](src/app/file_tree.rs#L572) calls `std::fs::rename` without checking the destination. On Unix, renaming `a.txt` to existing `b.txt` destroys `b.txt`. Reject existing destinations or require explicit replacement confirmation.

5. **Visual-line paste corrupts the final line boundary.** [src/app/visual.rs:321](src/app/visual.rs#L321) includes the preceding newline in a final-line selection. With `a\nb\n`, `gg"ayyjV"ap` produces `aa\n` instead of `a\na\n`. Keep the shared selection range at the line start; apply deletion-specific adjustments separately.

6. **Staging a hunk changes its trailing whitespace.** [src/git.rs:326](src/git.rs#L326) calls `trim_end()` on the entire diff body. A final added line containing `new   ` becomes `new` in the staged version. Preserve diff-body whitespace.

7. **Shell filters can deadlock on large input.** [src/format.rs:90](src/format.rs#L90) writes all input before reading stdout/stderr. A streaming filter such as `:%!cat` can fill its output pipe while the editor is still writing, blocking both processes. Write stdin concurrently with output collection, as `run_shell` already does.

## Medium severity

8. **Horizontal splits scroll against the wrong height.** [src/app/view.rs:243](src/app/view.rs#L243) uses the full editor height instead of the active pane's height. Moving downward after `:split` can leave the cursor's line invisible and position the terminal cursor outside the pane. Use the active pane's dimensions.

9. **Workspace-edit save failures are reported as success.** [src/app/lsp_glue.rs:1818](src/app/lsp_glue.rs#L1818) discards save errors and returns the full applied count. Permission or disk-space failures therefore produce misleading success responses. Propagate errors and restore the original active buffer on failure.

10. **Test results can disappear when the runner exits.** [src/test/manager.rs:137](src/test/manager.rs#L137) drops the session when the child exits and its channel is temporarily empty. Reader threads may still be parsing final failures and summaries. Wait for reader completion/channel disconnection before dropping the receiver.

## Summary

| Severity | Count |
|---|---:|
| Critical | 0 |
| High | 7 |
| Medium | 3 |
| Low | 0 |
| Total | 10 |

Verdict: **WARNING**.

Validation: 1,192 tests passed initially; the sole failure passed when rerun outside the sandbox, which had blocked `ps`. Formatting checks passed. Findings above are based on code-path inspection, not added regression tests.

Scope: enumerated 169 first-party source/configuration files and reviewed across editing, integrations, UI/runtime, and persistence/build tooling. This was broad but not exhaustive; substantial portions of the larger modules remain unreviewed. No repository files changed during the review.

## Resolution

All ten findings were confirmed and fixed, each with a regression test except #3's manager wiring:

| # | Commit |
|---|---|
| 1 | 2412b84 |
| 2 | 9565202 |
| 3 | ce054dd |
| 4 | f22ac86 |
| 5 | bf7ca88 — normal-mode `dd` / `cc` on a last line ending in `\n` had the same bug |
| 6 | 4ab0c16 — CRLF hunks also lost their `\r` |
| 7 | 42f55c0 |
| 8 | ffe5fa0 |
| 9 | 7d5d383 |
| 10 | f3f8b4b |
