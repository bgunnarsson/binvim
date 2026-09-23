---
title: Overlay page flags stack, so a key handler must follow draw's order rather than assume one flag is set
date: 2026-09-15
category: ui
module: src/app/input.rs
tags: [overlay, health, messages, registers, test-results, keys, render]
symptoms:
  - "the :health SETUP box says 'press i to install' and the footer 'i install Rust toolchain', but i does nothing"
  - "a key an overlay advertises works when the overlay is opened from the buffer, and not when it is opened from another overlay's : prompt"
  - "q or Esc on :health does nothing the first time after :health was run from :messages or :registers"
root_cause: "show_health_page / show_messages_page / show_list_page / show_test_results_page are independent bools that most openers don't clear, and render::draw paints the first set one in a fixed order, while the overlay key block judged which page was up from the other flags being clear"
related: []
---

## Problem

`:health` gained a SETUP box and an `i` key that opens `:install` on the missing toolchain. The
arm in the shared overlay key block (`handle_event`, `src/app/input.rs`) was gated on
`!messages && !test_results && !registers`, meaning "only when the page up is health". Review
found that the dashboard can be on screen with one of those flags still set, showing the key
while the key is swallowed by the block's `_ => return Ok(())`.

## What didn't work

- **Gating by excluding the sibling flags.** It reads as "health is the page up", and it passes
  every test and the manual check that opens `:health` from a buffer. It fails when `:health` is
  run from another overlay's `:` prompt, which the block deliberately lets through.
- **Unit-testing the arm.** The overlay block is inline in `handle_event`, which reads its event
  from crossterm. The test helper `replay_key` (`src/app/registers.rs`) enters the per-mode
  handlers below the block, so no unit test presses a key there. The arm's behaviour went into
  `health_install` to be testable, but the scoping, which is where the bug was, could only be
  checked by hand in tmux. That check used `:registers` → `i` and `:health` → `i` separately,
  never one opened from the other.

## Root cause

Verified in the source. The overlay flags are not mutually exclusive:

- `cmd_health` (`src/app/health.rs`) sets `show_health_page` and clears the start page and popups,
  but not `show_messages_page`, `show_list_page` or `show_test_results_page`.
- `cmd_messages` (`src/app/lsp_glue.rs`), `cmd_registers` (`src/app/registers.rs`) and
  `show_listing` (`src/app/search.rs`) don't clear `show_health_page`.
- Only `open_installer` (`src/app/installer.rs`) and the test runner (`src/app/test_glue.rs`)
  clear sibling flags.

`render::draw` (`src/render.rs`) resolves the stack with an `if / else if` chain: install, health,
messages, list, test results, start page. What is on screen is the first set flag in that order.
`ExCommand::Quit` and the mouse-wheel handler in `input.rs` already test in that order. The
overlay block's `dismiss` / `scroll` closures and its `g` / `G` arms test in the reverse order
(test results, messages, list, then health).

Observed: after the fix, in tmux against the release build, `:registers` → `:health` → `i` opens
`:install` on Rust. Traced from the code but not run: the failure before the fix, and the same
mismatch in the existing `q` / `Esc` / scroll handling, where `q` on a health page painted over
`:messages` clears the hidden messages flag first, so the dashboard stays up.

## Fix

`c8c2944`: the `i` arm is gated on `self.show_health_page` itself, which is exactly when `draw`
paints the dashboard. The existing `dismiss` / `scroll` / `g` / `G` ordering predated this change
and was left as it was.

`c62a163` and `69530c0` closed the rest. `App::top_overlay()` (`src/app/state.rs`) returns the
page `draw` paints, and `draw`, the overlay key block, `:q` and the mouse wheel all branch on it;
the scroll / dismiss / top / bottom logic became `overlay_*` methods on `App`, which unit tests
reach. The mouse wheel had a mismatch of its own, checking health before install. Review then
split the type: `OverlayPage` is `Install` or `Pager(Pager)`, and the `overlay_*` methods take a
`Pager`, so the install page (its own mode, its own keys) can't be handed to them to do nothing.

## Prevention

- A handler that acts on the overlay page up asks `App::top_overlay()` — for health,
  `Some(OverlayPage::Pager(Pager::Health))`. Testing `show_*_page` flags for precedence in a
  handler, a guard written as `!messages && !test_results && !registers`, or any "the other flags
  are clear" form, is a violation.
- A new overlay page gets a variant placed in `top_overlay()` where `draw` paints it: under
  `Pager` if the shared overlay keys drive it, beside `Install` if it has a mode of its own. A new
  `show_*_page` flag that `top_overlay()` doesn't know is a violation, and so is an
  `OverlayPage::Install => {}` arm in a key helper.
- A manual check of an overlay key includes opening that overlay from another overlay's `:`
  prompt (`:registers`, then `:health`), because unit tests reach the `overlay_*` methods but not
  the key block that picks which page they get.
