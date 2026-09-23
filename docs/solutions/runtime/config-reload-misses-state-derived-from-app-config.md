---
title: Replacing App.config leaves behind every cache and override built from the old one
date: 2026-09-14
category: runtime
module: src/app/config_glue.rs
tags: [config, reload, cache, lsp, set]
symptoms:
  - "'config reloaded' shows, but a changed [colors] keyword colour doesn't appear until the buffer is edited"
  - "semantic-token colours stay painted after [lsp] semantic_tokens = false and a reload"
  - "a reload turns relative line numbers back on after :set norelativenumber"
root_cause: "App.config is mostly read at use, but some state bakes it in — resolved colours in caches, toggles checked only before a request, :set values written into the struct — and a wholesale swap updates none of it"
related:
  - docs/solutions/runtime/a-buffers-disk-fields-are-set-in-three-places-and-the-reload-is-outside-buffer-rs.md
  - docs/solutions/conventions/an-audit-guarantee-is-read-from-the-code-not-from-the-lore-that-describes-it.md
---

## Problem

`:config reload` and `:w` on `config.toml` apply the file by replacing `App.config` with a freshly
parsed struct (`apply_config_text`, `src/app/config_glue.rs`). The plan assumed that was enough,
because most code reads `self.config` when it needs a value. Three settings still didn't apply,
or applied wrongly, until something unrelated happened.

## What didn't work

- **Trusting a research summary about the highlight cache.** The plan said "the highlight cache
  stores capture names, not colours", taken from a repo-research summary. It was wrong:
  `HighlightCache.byte_colors` is `Vec<Option<Color>>`, built by
  `lang::compute_highlights(lang, &buffer, &self.config)`. The error was caught while reading
  `src/lang.rs` during the work, before the first reload implementation shipped.
- **Swapping the struct and clearing only the highlight caches.** Review then found two more
  leftovers. Cached semantic tokens were still painted after `semantic_tokens = false`, because
  the flag was only read in `lsp_request_semantic_tokens_if_due`. A `:set norelativenumber` was
  reset, because `set_flag` wrote straight into `self.config.line_numbers.relative`.

## Root cause

Code that depends on `App.config` holds it in one of four ways, and only the first survives a
swap:

1. **Read at use:** `render.rs` calls `color_for_capture` per frame, and code lens checks
   `config.lsp.code_lens` wherever it's used. A swap applies these immediately.
2. **Resolved into a cache:** `HighlightCache.byte_colors`, one per buffer. For stashed buffers,
   nothing rebuilds it until the buffer is focused.
3. **Checked only before a request:** `semantic_tokens` and `document_highlights` were painted
   whenever the buffer version matched. The `[lsp]` flag gated only new requests, so an existing
   cache, or a response already in flight, kept painting.
4. **Written into the config by session code:** `:set list` / `:set relativenumber`. Also
   `lsp.copilot_enabled`, which is mirrored once at startup.

Points 2 and 4 are covered by tests (`a_reload_recolours_the_active_buffer`,
`set_options_outlast_a_reload`), and point 1 was checked in the release build. That stale semantic
tokens stay on screen was established from the render path (`render.rs`, the
`semantic_tokens.get(path)` block); it wasn't observed against a running rust-analyzer.

## Fix

- `e86f0f7`: `recolour_highlights` clears the active buffer's highlight cache and rebuilds every
  stashed one after a swap.
- `795c81d`:
  - The renderer checks `config.lsp.semantic_tokens` before painting cached tokens, and
    `line_document_highlights` checks `document_highlight`.
  - `:set list` / `:set relativenumber` record `SessionOptions.list` / `.relativenumber`, which
    `apply_config_text` lays back over the new config.
  - A change to `[copilot] enabled` is reported as needing a restart rather than applied.

## Prevention

- **A new cache built from the config.** A diff that passes `&self.config` (or any `config.*`
  value) into something whose result is stored on `App` or a `BufferStash` must also clear or
  rebuild that store in `config_glue::apply_config_text`. A new `compute_*(…, &self.config)`
  whose result is assigned to a field is the pattern to look for.
- **A new setting that gates a request.** A diff that adds an early return on a `config.*` flag
  in an `*_if_due` or request function must also check that flag where the cached result is
  painted or consumed. An `if !self.config.… { return; }` before a request, with no matching
  check in `render.rs` or the consumer, is a violation.
- **Session code writing into the config.** A diff where `:set`, a keybinding or other session
  code assigns to `self.config.*` must record the value in `SessionOptions` and reapply it in
  `apply_config_text`. Otherwise the next reload silently undoes it.
- **Plan claims about stored state.** A plan statement about what a cache or struct stores must
  name the field and its type as read in the source, not repeat a research agent's summary.
- Written into: CLAUDE.md
