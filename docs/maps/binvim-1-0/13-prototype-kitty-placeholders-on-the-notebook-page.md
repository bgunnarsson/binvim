---
title: Does a PNG output drawn as kitty Unicode placeholders scroll and clip cleanly on the notebook page, in kitty, Ghostty and tmux?
kind: prototype
mode: hitl
status: open
blocked_by: [07]
claimed_by:
---

## Question

[07](07-drawing-images-on-the-notebook-page.md) recommends kitty Unicode placeholders as the first protocol. Build the smallest version on a scratch branch: upload one notebook's PNG output with `U=1`, reserve its rows in the page layout, paint the placeholder cells in `draw_notebook_page`. The user judges it in kitty, Ghostty and tmux (with `allow-passthrough on`): does it scroll, clip at the window edge and clear on close without leftovers, and does a redraw flicker?

## Context

- [07](07-drawing-images-on-the-notebook-page.md)'s answer.
- `src/notebook.rs:1191` `output_images`, `src/notebook_page.rs:80` `layout`, `src/render.rs:5683` `draw_notebook_page`.
- The kitty graphics protocol spec's Unicode placeholder section.

## Answer

