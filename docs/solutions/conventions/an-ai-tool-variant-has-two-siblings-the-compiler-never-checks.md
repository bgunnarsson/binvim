---
title: An AiTool variant has two siblings the compiler never checks, and both have already been missed once
date: 2026-09-25
category: conventions
module: src/command.rs, src/parser.rs, src/app/dispatch.rs, src/app/state.rs, src/app/cmdline_complete.rs
paths:
  - src/command.rs
  - src/parser.rs
  - src/app/dispatch.rs
  - src/app/state.rs
  - src/app/cmdline_complete.rs
tags: [ai-tool, side-terminal, which-key, cmdline-completion, exhaustive-match, registration-gap]
symptoms:
  - "grep -n \"openclaw\\|hermes\" src/app/cmdline_complete.rs returns nothing, months after both tools shipped"
  - ":openclaw<Tab> and :hermes<Tab> do not complete anything in the `:` command line"
  - "commit d9e7a96 exists solely to add openclaw/hermes's <leader>j which-key menu rows after cb99439 shipped without them"
root_cause: "AiTool has no single registration list. Three sites are Rust match arms the compiler checks exhaustively — command.rs's label()/command(), parser.rs's Action variants plus its <leader>j char-dispatch match, and app/dispatch.rs's handler arms — so a variant left out of any of them fails the build. Two more sites are plain &str/tuple literals with no such check: app/state.rs's ai_prefix_entries() which-key hints, and app/cmdline_complete.rs's COMMAND_NAMES completion list. A variant missing from either compiles clean and is only caught by using the feature (opening the which-key popup, or tab-completing the ex-command)."
related:
  - docs/solutions/runtime/a-buffers-disk-fields-are-set-in-three-places-and-the-reload-is-outside-buffer-rs.md
---

## Problem

Adding `binai` as a sixth AI-assistant tool (alongside claude/codex/opencode/openclaw/hermes)
meant finding every place `AiTool::Hermes` already appears, since there is no single enum-driven
registration point and no test exercises the feature end to end.

## What didn't work

- **Trusting the compiler to say when the job is done.** `AiTool::label()` and `AiTool::command()`
  in `src/command.rs`, the `Action` variants and `<leader>j` char match in `src/parser.rs`, and the
  handler arms in `src/app/dispatch.rs` are all exhaustive matches — leaving a variant out of any of
  them is a build error. That gave false confidence that a clean `cargo build` meant the tool was
  fully wired.
- **The historical record shows the same gap being hit for real.** `cb99439` ("add openclaw and
  hermes to the AI splits") touched only `command.rs`, `parser.rs` and `dispatch.rs` — the three
  compiler-enforced sites. `d9e7a96`, the very next commit, exists solely to add the `<leader>j`
  which-key menu rows (`src/app/state.rs::ai_prefix_entries`) that `cb99439` missed. Nothing forced
  that follow-up; it was caught by a human opening the which-key popup.
- **`src/app/cmdline_complete.rs`'s `COMMAND_NAMES` never got the same follow-up.** It still has no
  entry for `openclaw` or `hermes` as of this writing — `:openclaw<Tab>` completes nothing. This is
  a live, unfixed instance of the same gap, left in place because closing it was out of scope for
  the `binai` change and is called out here instead of folded in silently.

## Root cause

Verified by reading every match arm on `AiTool`/`Action::Ai*` (`grep -rn "AiTool::\|AiHermes\|AiOpenClaw"
src/`) and the two commits above. The five files that need an entry per tool split into two kinds:
match arms the compiler enforces, and literal lists it does not. Nothing marks the second kind as
part of "the AiTool feature" — they read like independent UI lists (a menu, a completion table) — so
a change that only chases compiler errors stops at the first kind.

## Fix

- `515fdc1`: adding `AiTool::Binai` covered all five sites — `command.rs` (enum + label/command +
  `:binai` parsing), `parser.rs` (`Action::AiBinai`/`AiBinaiHandoff` + `<leader>jb`/`jB` dispatch),
  `app/dispatch.rs` (handler arms), `app/state.rs` (`ai_prefix_entries` rows), and
  `app/cmdline_complete.rs` (`COMMAND_NAMES` entry) — plus `docs/keys.md`. Found by grepping for
  every existing reference to `AiTool::Hermes` / `"hermes"` across `src/` before writing anything,
  rather than editing only the files an IDE's "find usages" on the enum surfaces.
- `openclaw` and `hermes` remain missing from `COMMAND_NAMES` — not fixed here, since it's a
  pre-existing gap unrelated to `binai`'s own wiring.

## Prevention

- **A diff that adds an `AiTool` variant is checked against a plain-text grep for every existing
  variant's label, not just "where the enum's match arms are."** `grep -rn 'Hermes\|"hermes"' src/`
  (substituting the newest existing tool) lists all five sites; a new-variant diff touching fewer
  than all five — `command.rs`, `parser.rs`, `app/dispatch.rs`, `app/state.rs`,
  `app/cmdline_complete.rs` — is a violation unless the diff says which site is deliberately
  deferred and why.
- **`app/cmdline_complete.rs`'s `COMMAND_NAMES` and `app/state.rs`'s `ai_prefix_entries()` are held
  to the same completeness as the `AiTool` enum**, even though nothing enforces it mechanically. A
  reviewer checks these two files by eye on every `AiTool` diff, the way `lang.rs`'s two exhaustive
  `Lang` matches are checked for a new language.
