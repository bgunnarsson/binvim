---
title: How can the notebook page draw image outputs through the kitty and iTerm2 protocols, and fall back to text?
kind: research
mode: afk
status: open
blocked_by: []
claimed_by:
---

## Question

Notebooks phase 3 draws images and plots. Find how a crossterm-drawn TUI places, scrolls, clips and clears images with the kitty graphics protocol (placements, unicode placeholders) and iTerm2 inline images; how to detect support (and Sixel, if worth it); how tmux passthrough affects each; and what the text fallback should be. Name the binvim code each part would touch.

## Context

- `docs/roadmap.md` Notebooks section.
- `src/notebook.rs` `output_images` and the `[image/png]` row, `src/notebook_page.rs` `layout`, `src/render.rs` `draw_notebook_page`.
- `docs/terminals.md` for which terminals binvim checks against.
- Protocol specs on the web (kitty graphics protocol, iTerm2 inline images); cite links.

## Answer

