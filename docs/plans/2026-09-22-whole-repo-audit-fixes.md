---
title: The whole-repo audit's findings are fixed — quoting, crash detection, quit teardown, large-file gates, and the drifted duplicates consolidated
date: 2026-09-22
status: in-progress
---

## Context

A whole-repo review (eight parallel reviewers plus a cross-cutting duplication scan over all 106
Rust files) found no criticals but 3 HIGH, 13 MEDIUM and 10 LOW issues. The user asked for all of
them fixed. The audit's overall verdict — the codebase is disciplined, duplication concentrates in
a few mechanical idioms — shapes the plan: consolidate only where copies have drifted or already
caused a bug, and keep every fix inside the existing module layout (no new `src/` directories;
helpers go into `paths.rs`, `format.rs`, `render.rs`, `package.rs`).

Decisions made here, from the code and the lore:

- **Task spawning keeps the shell.** `task_kickoff` needs `$SHELL -l -i -c` so rc-file PATH shims
  (nvm/asdf/direnv) load; the fix is quoting, not bypassing. `shell_quote` widens to `&str` and is
  applied to `task.program` and every element of `task.args` when building the `-c` string.
  `Task::command_line()` remains the unquoted display string for `status_msg` only.
- **LSP crash detection mirrors DAP.** `LspClient` gains a `try_exit_status()` shaped like
  `dap/client.rs:113`; `LspManager::drain` polls it (`iter` becomes `iter_mut`). A dead client's
  entry is removed so the next `ensure_for_path` respawns it, the four `*_in_flight` sets in
  `app/lsp_glue.rs` are cleared for its paths, the death is surfaced in the status line and
  `health_summary`, and `lsp_attach_active` re-runs for the active buffer so the common case
  recovers without a buffer switch.
- **Quit teardown runs in `App::run`'s exit block only** (`app.rs:1569`, beside
  `discard_all_recovery`): `test.cancel()` (exists, never called at quit),
  `dap.stop_session_blocking(short timeout)` when active, and a new best-effort
  `LspManager::shutdown_all` (send `shutdown`/`exit`, don't block long). The signal thread keeps
  its current behavior — per the hung-up-tty lore, extending teardown to signals would have to run
  on the signal thread itself, and that's out of scope here; the plan does not pretend signals get
  it.
- **The marker walk consolidates into `paths.rs`** beside `others_can_plant`, with two behaviors
  because the drift is intentional in one caller: `find_marker_root(start, markers) ->
  Option<PathBuf>` (task's semantics) and a fallback-to-start wrapper (lsp/dap/test semantics),
  both honoring the `*.ext` glob-marker convention and calling `others_can_plant` per candidate.
  `android::find_gradle_root` and `package::find_root_by_marker` stay separate — genuinely
  different semantics (git-root priority / nearest-fallback), as the audit and the prior plan both
  concluded.
- **The Windows URL-open fix changes the spawn, not the tokenizer.** `&` and `%` are legal and
  common in URLs, so `find_url_at` must not exclude them; instead `cmd /C start` is replaced with
  `rundll32 url.dll,FileProtocolHandler <url>`, which takes argv without shell reparsing.
- **adb identifiers are validated, not escaped**: `applicationId`/activity must match
  `[A-Za-z0-9._]+` before being interpolated into `adb shell` argument strings; anything else is
  rejected with a status message.
- **`clip_lines` lives in `package.rs`** beside `run_capture` — precedent: `update.rs` already
  reuses `package::http_get`. `format.rs` and `android.rs` call it; `android::run_capture`
  collapses onto `package::run_capture` only if the signatures align, otherwise it just shares the
  helper.
- **Popup borders get their own small helper**, not `paint_box_*`: the dashboard helpers are
  coupled to `DashboardPalette`, and threading that through seven popups is a bigger refactor than
  the duplication justifies. A `popup_box_top/bottom(out, x, y, w, title, fg, bg)` pair taking
  colors directly replaces the seven hand-rolled title-centering computations.
- **`apply_concrete_edits` gets its comment narrowed**, not staged atomicity — the recovery
  precondition is the only whole-batch check, and building rollback for save failures is a feature,
  not a fix.

## Relevant lore

- [An upward search trusts what other users own or can write](../solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md):
  the consolidation task must keep `others_can_plant` as the single check on every step down to the
  candidate, keep the fallback-to-start held to the same check, and verify by grepping
  `\.parent()\|ancestors()` that no walk is missed — the last pass missed three by listing from
  memory. Tests plant nested candidates under `0755` dirs beneath a `0777` ancestor.
- [Replacing a file by rename changes more than its contents](../solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md):
  formatter temp files and the yazi chooser use exclusive create (`create_temp_beside` /
  `create_exclusive`) under unpredictable names, never `File::create` + `set_permissions`; cache
  directories holding user text are narrowed to `0700` at startup, not only on write. No existing
  `write_atomic` guard may be removed.
- [A hung-up tty leaves crossterm's poll spinning](../solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md):
  quit teardown must not be gated behind the event loop noticing a flag; this plan adds it to the
  normal exit path only and leaves signal behavior unchanged. Manual tmux checks send Escape in its
  own `send-keys`, wait for the mode line, and finish with `:q!`.
- [Overlay flags stack and draw order decides what shows](../solutions/ui/overlay-flags-stack-and-draw-order-decides-what-shows.md):
  the render.rs page-chrome consolidation must not introduce any handler or helper that tests
  sibling `show_*_page` flags; everything keeps going through `App::top_overlay()`.
- [open_buffer runs for batch edits nobody sees](../solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md):
  the `apply_concrete_edits` comment fix must keep the `pending_recovery` precondition described
  accurately; nothing added to `open_buffer` in any task here may assume a user is watching.
- [Config reload misses state derived from App.config](../solutions/runtime/config-reload-misses-state-derived-from-app-config.md):
  the new gates (`ensure_folds`, `syntax_object`) key on `Buffer::is_large()`, not config, so no
  `apply_config_text` change is needed — verified against the field, not assumed. Any task that
  does end up caching config-derived state must add matching reload handling.
- [A recorded pid does not identify a process](../solutions/runtime/a-recorded-pid-does-not-identify-a-process.md):
  LSP/test/DAP teardown acts on owned `Child` handles, never on stored pids; no new pid-liveness
  checks are introduced.

## Acceptance criteria

- A `package.json` script whose name contains `$(date)` (and a justfile recipe named
  `` x`date` ``), run via `:task`, executes with the name passed literally — no substitution — and
  a task name with spaces runs at all. Covered by a unit test on the new quoting fn plus a tmux
  check.
- `kill -9` on a running language server: the status line reports the death within a tick, and
  hover/completion/inlay hints work again in the same session without relaunching binvim.
  `:health` never shows a dead server as running.
- `:q` while a DAP session is paused on a breakpoint leaves no adapter process behind; `:q` during
  a `:test` run leaves no test child behind (`ps` after quit in a tmux check).
- `grep -rn "fn has_any_marker\|fn dir_contains_extension" src` finds only `paths.rs`; the four
  specs call `paths::find_marker_root*`; every existing planted-marker test still passes and at
  least one nested-candidate test moves to `paths.rs`.
- The csharpier/ktfmt/php-cs-fixer temp file is created exclusively under an unpredictable name
  (unit test: pre-creating a file/symlink where the helper would write makes it pick another name,
  never write through).
- `~/.cache/binvim` and `sessions/` are `0700` after startup + save (asserted the way
  `undo.rs`'s tests assert it).
- Opening a file with >`is_large` thresholds does not run `compute_indent_folds` or allow
  `dif`/`daf` to parse the whole buffer (unit tests on the gates).
- The stderr-clip helper is the single implementation (`grep` for the old `take(4)` idiom finds
  only the helper); `run_gofmt` calls `run_stdin_pipe`; behavior-identical output asserted by the
  existing formatter/package/android tests.
- The four drifted pairs (health indent label, save format-apply, windows adopt-block, buffers
  phantom-seed) each go through one helper; the health label shows tab width in both panels, and
  the format-apply clamp covers `.col` as well as `.line`.
- `cargo test --locked --all-features -- --test-threads=1`, `cargo +1.98.0 clippy --locked
  --all-targets -- -D warnings`, and `cargo fmt --check` all pass; no commit trips the
  attribution hook.

## Tasks

- [x] **A1 — Quote task argv into the shell string.** Widen `shell_quote` to accept `&str`
  (keep the `&Path` call working), apply it to `task.program` and each of `task.args` in
  `task_kickoff`'s launcher. Unit tests in `app/task_glue.rs`'s `mod tests` for `$(…)`, backticks,
  spaces, quotes. Verify: new tests + tmux run of a crafted npm script name showing literal text.
- [x] **A2 — Detect LSP child death.**
  Deviation: no unit test — `LspClient` has no test harness (only parsers are
  unit-tested, same as the DAP side, whose `try_exit_status` is also untested);
  verified by build + the manual `kill -9` check in Verification. `try_exit_status` on `LspClient` (model:
  `dap/client.rs:113-121`; `_child` becomes mutably pollable), polled in `LspManager::drain`;
  on death remove the entry, emit an event the glue uses to clear the four `*_in_flight` sets,
  set the status line, record in `health_summary` (`LspHealth` gains the field), and re-attach the
  active buffer. Verify: unit test with a client whose child exits; manual `kill -9` check per
  acceptance criteria.
- [x] **A3 — Quit-time teardown.** In `App::run`'s exit block: `self.test.cancel()`,
  `self.dap.stop_session_blocking(..)` when active, new `LspManager::shutdown_all` (best-effort
  `shutdown` request + `exit` notification per client). Verify: tmux checks per acceptance
  criteria; no unit-testable surface beyond `shutdown_all` sending the two messages
  (assert on the wire via the existing client test harness if present, else the tmux check).
- [x] **A4 — Overlap-drop in `apply_multi_selection_operate`.** Extract
  `fn drop_overlapping_ranges(ranges: &mut Vec<(usize, usize)>)` used by all three sites
  (`visual.rs:461` gains it; `visual.rs:628` and `multi_cursor.rs:156` converge on it). Unit test
  overlapping-range input on the operate path.
- [x] **A5 — Formatter temp files through exclusive create, once.** Expose a `pub(crate)` wrapper
  over `create_temp_beside` in `paths.rs`; factor the csharpier/ktfmt/php-cs-fixer temp-file dance
  into one `format.rs` helper built on it. Unit test: pre-planted path forces a different name.
- [x] **A6 — Yazi chooser out of the shared temp dir.**
  Deviation: verified by build + code reading (the fn doesn't split cleanly
  for a unit test); the yazi open/pick flow is in the manual Verification pass. `open_yazi` writes its chooser file under
  `paths::cache_dir()` (0700-narrowed like `undo.rs`) with the existing unpredictable-name
  machinery. Verify: manual yazi open/pick still works; path asserted in a unit test if the fn
  splits cleanly, else by reading the code in review.
- [x] **A7 — Narrow cache permissions.** Narrow `~/.cache/binvim` at startup and `sessions/` on
  save to `0700`, following `undo.rs`'s save+startup pattern. Unit test mirrors `undo.rs`'s
  permission assertions.
- [x] **A8 — Windows URL open without `cmd /C`.** `open_url_in_browser` uses
  `rundll32 url.dll,FileProtocolHandler`; `find_url_at` unchanged. Verify: compile-only on this
  platform; note in hand-off that Windows is untested here.
- [x] **A9 — Validate Android identifiers.**
  Deviation: the activity name additionally allows `$` (inner-class spelling),
  neutralized by single-quoting the `-n` component for the device shell. Reject `applicationId`/activity not matching
  `[A-Za-z0-9._]+` before any `adb shell` interpolation, with a status message. Unit tests beside
  `parse_application_id`.
- [x] **B1 — Gate and flatten folds.** `ensure_folds` gates on `buffer.is_large()` like
  `ensure_highlights`; `compute_indent_folds` becomes a single stack-based pass. Unit tests: fold
  results unchanged on existing fixtures; a deep-indent fixture computes in one pass; large buffer
  skips.
- [x] **B2 — Gate `syntax_object`.** Early-return `None` on `is_large()` inside `syntax_object`
  (covers all three callers). Unit test with an over-threshold buffer.
- [x] **B3 — Cap `TestManager::drain`.** Count-bound per call like `lsp/manager.rs`'s
  `MAX_PER_CALL`, returning a "more pending" signal consistent with how the run loop re-polls.
  Unit test: a burst larger than the cap drains across calls without loss.
- [x] **B4 — Render nits.** Reuse the `line_diags` binding in `draw_line_with_selection` (both
  sites), drop the dead `dim_fg` parameter, remove the `.saturating_sub(0)` and fix its comment.
  Verify: `cargo test` + visual tmux smoke.
- [x] **C1 — `paths::find_marker_root`.**
  Deviation: `lsp::find_workspace_root` and `dap::find_workspace_root` stay as
  one-line delegating wrappers (their signatures are public API of the specs
  modules); the four walk bodies and helpers now exist only in `paths.rs`. Shared walk (+`has_any_marker`/`dir_contains_extension`)
  in `paths.rs` with Option and fallback variants; `lsp/dap/test/task` specs call it; their walk
  tests collapse into `paths.rs` (nested-candidate planting per the lore); the per-module callers
  keep only integration-shaped tests. Verify: the lore's grep accounting plus full test suite.
- [x] **C2 — One stderr clip.**
  Deviation: `android::run_capture` keeps its own signature (it takes a
  prebuilt `Command` and label — not a match for `package::run_capture`'s
  bin/args/cwd/env shape); only the clip is shared. `clip_lines` in `package.rs`; all eight sites call it (biome keeps
  its box-strip around it, take counts preserved per site); `run_gofmt` becomes a
  `run_stdin_pipe` call; `android::run_capture` merges with `package::run_capture` only if
  signatures align. Verify: existing formatter/package/android parser tests unchanged.
- [x] **C3 — Parser leader tail.** One `finish(state, action)` helper for the ten
  `awaiting_*_leader` blocks. Verify: existing parser tests.
- [x] **C4 — Shared page chrome.** Footer-hint match and the clear/scroll/paint boilerplate of the
  four scrollable pages factor into one painter; nothing tests sibling overlay flags
  (`top_overlay` untouched). Verify: tmux visual check of all four pages + `:registers`-then-
  `:health` stacking per the overlay lore.
- [x] **C5 — Popup border helper.**
  Deviation: the hover and picker top borders keep their own layout (they
  carry right-aligned segments — scroll label, match count); the helper took
  the seven bottoms and the five centred/plain tops. `popup_box_top/bottom` taking colors directly; the seven
  popups adopt it. Verify: tmux visual check of whichkey, hover, picker, rename preview.
- [x] **C6 — Drifted pairs.**
  Deviation: the audit's "missing `.col` clamp" claim was wrong —
  `clamp_cursor_normal` clamps both line and col, so the explicit line-clamp
  in both copies was redundant and the helper simply drops it. No unit test
  for `apply_formatted` (needs a formatter binary on PATH); covered by the
  existing save tests plus the indent-label test. `indent_label` (with width, both panels), `apply_formatted_replace`
  (clamping `.col` too), `adopt_window_buffer`, `strip_phantom_seed_if_unused` — verifying first
  that both copies of each really match the audit's description. Unit tests where the helpers are
  pure (indent label, clamp); existing window/buffer tests for the rest.
- [ ] **D1 — Small cleanups, batch 1.** `apply_history_snapshot` helper (edit.rs ×4);
  `Buffer` constructors via `..Self::empty()`; picker page-move helper; spell.rs dead closure
  removed; lsp/dap specs share one `#[cfg(test)]` scratch helper (home: `paths.rs`'s existing
  test-support). Verify: `cargo test`.
- [ ] **D2 — Small cleanups, batch 2.** Extract `handle_side_terminal_mouse_event` and
  `handle_file_tree_mouse_event` per the existing extraction pattern; narrow the
  `apply_concrete_edits` comment to the recovery-precondition guarantee; gate
  `resend_breakpoints_for` while DAP `state == Initializing`. Verify: `cargo test` + tmux mouse
  smoke on side terminal and file tree.

## Files

- `src/paths.rs`: `find_marker_root` variants + walk helpers beside `others_can_plant`;
  `pub(crate)` temp-file wrapper; startup cache narrowing; shared `#[cfg(test)]` scratch helper.
- `src/app/task_glue.rs` (`task_kickoff:165`, `shell_quote:432`), `src/task/types.rs`
  (`command_line:39` stays display-only).
- `src/lsp/client.rs`, `src/lsp/manager.rs` (`drain:412`, `health_summary:357`),
  `src/app/lsp_glue.rs` (four `*_in_flight` sets, `:1056-1366`; comment at `:1735`).
- `src/app.rs:1569` exit block; `src/dap/manager.rs` (`stop_session_blocking:546` reused);
  `src/test/manager.rs` (`cancel:164` reused; `drain:116` capped; model `lsp/manager.rs:412`
  `MAX_PER_CALL`).
- `src/app/visual.rs:461/628`, `src/app/multi_cursor.rs:156` (overlap helper).
- `src/app/view.rs:640/1127`, `src/text_object.rs:343`, `src/buffer.rs:476` (`is_large`).
- `src/format.rs` (temp-file helper; `run_gofmt:640` → `run_stdin_pipe:76`; clip call sites),
  `src/package.rs` (`clip_lines` beside `run_capture:525`), `src/android.rs`
  (`run_capture:151`, identifier validation near `:319/:389/:507`).
- `src/app/picker_glue.rs` (`open_yazi:394`, page helper `:273`), `src/session.rs` (`save:172`).
- `src/app/dap_glue.rs` (`open_url_in_browser:589`), `src/dap/manager.rs`
  (`resend_breakpoints_for:1380`).
- `src/render.rs` (`:6025/:6634` diags; footer sites `:2276/:3599/:3754/:4111`; popup fns;
  `paint_box_*:4879` untouched), `src/parser.rs:1616-1886`, `src/app/health.rs:274/:346`,
  `src/app/save.rs:28/:65`, `src/app/windows.rs:70/:110`, `src/app/buffers.rs:280/:842`,
  `src/app/edit.rs:1327-1384`, `src/app/input.rs:684-1381`, `src/spell.rs:223`.

## Verification

After all tasks: `cargo test --locked --all-features -- --test-threads=1`, `cargo +1.98.0 clippy
--locked --all-targets -- -D warnings`, `cargo fmt --check`, then `cargo build --release` (the
user's alias runs the release binary). Manual tmux pass (Escape alone in its own `send-keys`, wait
for the mode line, finish with `:q!`): the crafted-task-name check, the `kill -9` LSP recovery
check, `:q` with a paused DAP session then `ps` for the adapter, `:q` mid-`:test` then `ps` for
the runner, all four scrollable pages plus the `:registers`→`:health` stack, yazi open/pick, and
one save through csharpier or ktfmt if installed. Finally the lore's grep accounting:
`grep -rn "\.parent()\|ancestors()" src` with every upward walk accounted for.
