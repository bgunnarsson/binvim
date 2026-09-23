---
title: A terminal compatibility matrix is tested against a written checklist and published
date: 2026-09-23
status: in-progress
---

## Context

0.7 ships when the "terminal matrix [is] documented" (`ROADMAP.md`, milestone table). Horizon 2
names Ghostty, Kitty, WezTerm, Alacritty, tmux, Windows Terminal and over-SSH, and says the
published matrix "doubles as a hardening checklist and marketing." Nothing of it exists yet. There
is no matrix file, and the only terminal-specific notes are OSC 52 (`docs/configuration.md`, which
already calls Terminal.app unsupported) and the `:health` rows for `TERM`, `COLORTERM` and
`TERM_PROGRAM` (`src/app/health.rs:389-400`).

binvim depends on these terminal features (from the source):

- Kitty keyboard disambiguation, `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`, pushed
  in `src/app.rs`, `picker_glue.rs`, `lazygit_glue.rs` and `installer.rs`. It's pushed
  unconditionally, with no `supports_keyboard_enhancement` check.
- Mouse capture.
- Bracketed paste.
- Synchronized output (`BeginSynchronizedUpdate`, `render.rs`).
- Undercurl with an underline colour, for diagnostics (`render.rs`).
- Italics.
- Cursor shape (`SetCursorStyle`).
- Truecolor.
- OSC 52 clipboard, raw or DCS-wrapped for tmux (`app/registers.rs`).
- Wide-char and emoji width, which the grapheme plan
  (`2026-09-23-the-cursor-moves-edits-and-measures-by-grapheme-cluster.md`) measures with
  `unicode-width`.

Decisions (the user chose the checklist route):

- **The matrix is `TERMINALS.md` at the repo root,** beside `WINDOWS.md`. It holds a checklist of
  numbered checks, each with the exact keys to press and what passing looks like, and a table of
  terminal × check with pass / fail / n/a and the terminal's version. `README.md` links it.
- **The checklist covers each feature above,** plus:
  - Esc response time
  - Alt and Ctrl-[ vs Esc
  - Shift / Ctrl arrows
  - resize
  - `:terminal`
  - the lazygit suspend round-trip
  - Nerd Font icons on the start page
- **I run what tmux can reach,** both tmux inside Ghostty and binvim over `ssh localhost` inside
  tmux if sshd is enabled. The user runs Ghostty (bare), Kitty, WezTerm, Alacritty, Windows
  Terminal and over-SSH, and reports each row.
- **Every failure becomes one of three things.** A fix in this plan, when it's binvim's (for
  example, pushing keyboard flags a terminal can't take). A `KNOWN_ISSUES.md` entry, when it's the
  terminal's. Or an n/a with the reason. The matrix never shows a bare "fail".
- **Terminal.app stays listed as unsupported for OSC 52,** as the README says, and is added as a
  row so the matrix states it.

## Relevant lore

- [tmux send-keys Escape then a key arrives as Alt](../solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md):
  tmux checks send `Escape` in its own `send-keys` call. The Esc-vs-Alt check (a checklist row)
  can't be judged through `send-keys` at all, so that row is the user's.
- [tmux new-session runs under the server's environment](../solutions/tooling/tmux-new-session-runs-under-the-servers-environment-not-the-scripts.md):
  environment for a check, such as `TERM` or `COLORTERM` overrides, goes on the pane's own command
  line.
- [A hung-up tty leaves crossterm's poll spinning](../solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md):
  checks end with `:q!`, not `tmux kill-session`, which leaves recovery and session files behind.
  The SSH row's "drop the connection" check is expected to leave them. It verifies they're there,
  then cleans them up.

## Acceptance criteria

- `TERMINALS.md` has the numbered checklist and a row for each of Ghostty, Kitty, WezTerm,
  Alacritty, tmux, Windows Terminal, over-SSH and Terminal.app. Every cell is pass, n/a with a
  reason, or a link to the `KNOWN_ISSUES.md` entry or commit that deals with it.
- The tmux and tmux-over-SSH rows were run by me, with each check's `capture-pane` evidence noted.
  The other rows are filled from the user's reports, with the terminal version each was run on.
- On a terminal without the Kitty keyboard protocol, binvim neither prints the push sequence as
  text nor mis-reads keys. Either the check passes, or the push is made conditional on
  `crossterm::terminal::supports_keyboard_enhancement()` in all four places.
- `README.md` links `TERMINALS.md`, and `ROADMAP.md`'s "terminal matrix documented" is marked
  done.

## Tasks

- [x] Write `TERMINALS.md`'s checklist: one numbered check per feature above, with exact keys and
  pass criteria, and an empty matrix. Include a small `docs/terminal-check.txt` fixture holding
  wide chars, emoji clusters, a long line and a diagnostic-bearing snippet to open during the
  checks. Verify by running the checklist once in tmux myself, to confirm every step is
  executable as written.
  Deviation: the fixture is `docs/terminal-check.md`, because Markdown's conceal is where binvim
  draws italic and bold in a buffer, plus `docs/terminal-check.rs` for undercurl, because only an
  LSP produces diagnostics and a `.txt` file has none. rust-analyzer reports its syntax error.
  Deviation: the start page has no Nerd Font icons (its logo is box-drawing characters). The
  glyph check looks at the status line and the file picker instead.
- [x] Run the tmux row and, if `ssh localhost` works, the tmux-over-SSH row, recording evidence.
  Fix any binvim-side failure as its own commit with a test where one can be written. Verify each
  fix with the check it failed, and record that in the matrix.
  Deviation: `ssh localhost` doesn't work here (nothing listens on port 22; Remote Login is off),
  so the tmux-over-SSH row wasn't run and the over-SSH column is left to the user's report.
  Three fixes, each with the check that found it: `Tab` as `Ctrl-i` (`2f81606`, check 2),
  `Ctrl-[` as Esc under the Kitty protocol (`c15d6b5`, check 4, found by sending that encoding
  through tmux by hand), and `Ctrl-C` at a suspended child no longer ending binvim (`a4ace97`,
  check 20). The last has no unit test: it is a signal disposition, checked in tmux.
  Check 19's `Ctrl-[` in `:terminal` can't pass without the protocol, so it's a `KNOWN_ISSUES.md`
  entry.
- [x] Check how crossterm 0.x (the version in `Cargo.lock`) handles
  `PushKeyboardEnhancementFlags` on a terminal that doesn't support it, by reading its source. If
  it can leak, gate the four pushes on `supports_keyboard_enhancement()`. Verify by reading the
  crossterm source path, and, if gated, with a tmux check on `TERM=xterm-256color` without the
  extended-keys option.
  Result: not gated. In crossterm 0.28.1 (`src/event.rs:493`) `PushKeyboardEnhancementFlags`
  writes `CSI > 1 u` unconditionally on Unix. That is a well-formed control sequence, which a
  terminal that doesn't know it consumes and drops, and without the protocol keys simply stay in
  the legacy encoding. In tmux on `TERM=xterm-256color` with `extended-keys off`, nothing was
  printed and keys read right (`TERMINALS.md`, tmux note 1). On Windows it is never written:
  `is_ansi_code_supported()` is `false` and `execute_winapi` returns `Unsupported`, which binvim
  drops, so Windows Terminal runs without the protocol. `supports_keyboard_enhancement()` would
  cost a query round trip with a two-second timeout at startup and after every suspend, for no
  failure seen. The `app.rs` comment now says what happens on Windows (`77dae2b`).
- [ ] Hand the checklist to the user for Ghostty, Kitty, WezTerm, Alacritty, Windows Terminal,
  over-SSH and Terminal.app. Fill the rows from their reports, fix binvim-side failures (each its
  own commit) and file terminal-side ones in `KNOWN_ISSUES.md`. Verify that every cell is
  resolved as the acceptance criteria require.
- [ ] Link `TERMINALS.md` from `README.md`, mark the matrix done in `ROADMAP.md`, and add a
  `CHANGELOG.md` entry for any fix. Verify by reading the diff.

## Files

- `TERMINALS.md` (new), `docs/terminal-check.txt` (new fixture), `README.md`, `ROADMAP.md`,
  `KNOWN_ISSUES.md`, `CHANGELOG.md`.
- Possible fixes: `src/app.rs`, `src/app/picker_glue.rs`, `src/app/lazygit_glue.rs`,
  `src/app/installer.rs` (the keyboard-flag pushes), and `src/render.rs` (undercurl, synchronized
  output, cursor style).
- The model to follow is `WINDOWS.md`'s on-device checklist.

## Verification

- Every row filled, with evidence or a report behind each cell. `cargo test -- --test-threads=1`
  and pinned clippy green for any fix.
- Run this plan after the grapheme plan, so the emoji-width check measures the grapheme-aware
  renderer. A terminal that draws a cluster differently from `unicode-width` is a terminal-side
  `KNOWN_ISSUES.md` entry, not a binvim bug.
