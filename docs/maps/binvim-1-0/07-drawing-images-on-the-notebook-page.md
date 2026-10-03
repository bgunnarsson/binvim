---
title: How can the notebook page draw image outputs through the kitty and iTerm2 protocols, and fall back to text?
kind: research
mode: afk
status: resolved
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

The plan is a fallback chain, chosen once at startup:
1. The kitty graphics protocol with Unicode placeholders, for kitty, Ghostty and any terminal that answers the `a=q` query.
2. iTerm2 OSC 1337 inline images, for iTerm2, WezTerm and VS Code, drawn only when the image's rows are wholly on screen.
3. Half-block `▀` cells under truecolor.
4. The current `[image/png]` label.

Sixel is left out for now.

- **Placeholders fit how binvim draws.** `draw_notebook_page` queues crossterm commands straight to the terminal and keeps no previous-frame grid (`src/render.rs:5683`). A placeholder image is ordinary cells, U+10EEEE with the image id in the foreground colour and row/column diacritics. So it scrolls, clips and is overwritten with the page, and needs no delete-and-replace on each scroll. The PNG is uploaded once per output with `a=t,f=100,U=1,i=<id>` in 4096-byte base64 chunks, sent direct rather than as a file so it works over ssh, and freed with `a=d,d=I` when the output goes. Over tmux, only the upload needs passthrough (`allow-passthrough on`, DCS-wrapped, every ESC doubled). The cells pass through tmux like text, which is why yazi and ratatui-image use placeholders on kitty and Ghostty.
- **iTerm2 images have no ids and no delete.** An image paints at the cursor and stays until text covers it, so it can't be cropped. Draw it only when wholly visible and re-emit it after any scroll that moves it; otherwise show the label. Under tmux, use the multipart form (iTerm2 3.5+).
- **Detection.** Send the kitty query `a=q` followed by DA1: a graphics reply before the DA1 reply means support. `TERM_PROGRAM` picks iTerm.app, WezTerm or vscode. binvim queries nothing today: `TERM_PROGRAM`, `COLORTERM` and `TERM` are only read for `:health` (`src/app/health.rs:389-400`), and `TMUX` and SSH are detected in `src/app/registers.rs:792-820`.
- **Code each part touches:**
  - `src/notebook.rs`: `output_images` (line 1191) decodes the bytes but keeps no size, and the image row is made at line 1121. Rows need the image's cell size, so the PNG header's width and height are read.
  - `src/notebook_page.rs`: `layout` / `code_cell` (lines 80, 263-281) reserve rows for the image.
  - `src/render.rs`: `draw_notebook_page` paints the placeholder cells or emits the inline image.
  - Detection runs at startup, beside `TerminalGuard`, and gets a `:health` row.
- **Terminals.** kitty 0.28+ and Ghostty support placeholders. Sixel would only add foot, Windows Terminal 1.22+ and Konsole, and costs a quantising encoder. Windows Terminal and Alacritty get half-blocks.

Unverified: the exact kitty delete-key table, the precise form of the `a=q` probe, and Ghostty's "complete" kitty support (from a tweet). Sources: [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), [iTerm2 inline images](https://iterm2.com/documentation-images.html), [ratatui-image](https://github.com/benjajaja/ratatui-image), [yazi image preview](https://yazi-rs.github.io/docs/image-preview/), [tmux FAQ](https://github.com/tmux/tmux/wiki/FAQ), [arewesixelyet](https://www.arewesixelyet.com/).
