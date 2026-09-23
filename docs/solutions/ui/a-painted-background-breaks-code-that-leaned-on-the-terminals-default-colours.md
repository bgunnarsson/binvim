---
title: Painting the buffer's background breaks render code that leaned on the terminal's default colours being a matching pair
date: 2026-09-23
category: ui
module: src/render.rs
paths:
  - src/render.rs
  - src/config.rs
tags: [render, colours, background, reverse-video, sgr, theme, catppuccin, performance]
symptoms:
  - "Visual selection is unreadable on a light terminal profile: dark text on a near-black block"
  - "selected cells capture as `\\e[48;2;30;30;46m\\e[7m` with no `38;2` foreground"
  - "a redraw of a prose or .txt file is dozens of times larger after the buffer background became painted by default"
  - "every uncaptured cell captures as `\\e[38;2;205;214;244mX\\e[0m\\e[48;2;30;30;46m`"
root_cause: "code written when the buffer used the terminal's own foreground and background (SGR 39 / 49) relied on the two matching; once binvim paints #1e1e2e by default, a cell that leaves the foreground at 39 pairs the terminal's text colour with binvim's background, and Reverse swaps them into an unreadable fill"
related:
  - docs/solutions/runtime/config-reload-misses-state-derived-from-app-config.md
---

## Problem

`Config::background_color()` began returning Catppuccin Base `#1e1e2e` when
`[colors] background` is unset (commit `e572ae2`), so every zero-config binvim
paints the buffer background. Plain text got `theme_fg()` through a fallback on
`syntax_color` in `draw_line_with_selection` (`9df97d8`). Review then found two
places in `src/render.rs` that had only worked because the terminal's default
colours were a pair.

## What didn't work

- **The fallback on `syntax_color` alone** (`.or(plain_fg)`). It coloured plain
  text, but the `in_sel` branch runs before the `syntax_color` arm and only sets
  `Attribute::Reverse`. A selected cell kept SGR 39, the terminal's foreground.
  Reverse turns that into the fill and `#1e1e2e` into the text. On a light
  profile (black text on white) that is `#1e1e2e` on black. Before the change the
  zero-config selection reversed the terminal's own pair, and was readable on any
  profile.
- **The same fallback made `syntax_color` always `Some`**, and the per-cell
  teardown fires on `syntax_color.is_some()`. Every plain character then carried
  a `38;2` set before it and `ResetColor` + `48;2` after it, about 40 bytes for a
  one-byte character, on a renderer that clears and repaints the whole frame each
  time.

## Root cause

Observed in `capture-pane -e` and `pipe-pane -O`. With `buf_bg` `None`, a cell
that sets no foreground, or that reverses whatever is in effect, gets a matching
foreground and background from the terminal. With `buf_bg` painted, the only
foreground that matches it is one binvim sets. Any path that leaves the
foreground at 39 now mixes two sources, and Reverse makes that visible at once.

The byte cost was a separate consequence of the same shift. The teardown was
written for coloured cells being the exception, and a default applied per cell
turned every cell into one.

## Fix

`2cdc5b9`, in `draw_line_with_selection`:

- A selected cell, and the empty-line selection block, set `plain_fg` before
  `Reverse`. The selection is `#1e1e2e` on `#cdd6f4`.
- `syntax_color` is only the capture or token colour again. A plain cell falls to
  its own arm, which sets `plain_fg` only when `plain_fg_on` says it isn't
  already in effect, and a plain cell has no teardown. Every teardown clears
  `plain_fg_on`.
- A doc-highlight cell with no capture now tears down its background.

A 200-line prose file redraws in 48 KB, against 41 KB with `background = "Reset"`
(both with whitespace marks off).

## Prevention

- **A cell that uses `Attribute::Reverse` on the buffer sets its foreground
  first when `buf_bg` is `Some`.** A new `SetAttribute(Attribute::Reverse)` in
  `render.rs` with no `SetForegroundColor` ahead of it in the same branch is a
  violation. The same goes for any cell that prints text and relies on the
  foreground being 39.
- **A colour applied to every cell by default doesn't go through the per-cell
  set-and-reset path.** It is set once per run and tracked (as `plain_fg_on` is).
  A fallback added to `syntax_color`, or a new `|| <always true>` in the teardown
  condition, is a violation. Check a render change's cost with `pipe-pane -O`
  bytes per frame against the `background = "Reset"` baseline.
- **A change to what `background_color()` returns for the unset case is checked
  in three configs:** unset, `background = "Reset"`, and a light theme
  (`catppuccin-latte`), each with a Visual selection in the capture.
