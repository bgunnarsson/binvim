---
title: The buffer paints Catppuccin Mocha by default, and background = "Reset" hands it back to the terminal
date: 2026-09-23
status: done
---

## Context

Testing Kitty for the terminal matrix (`TERMINALS.md`), the user saw the default theme "only
partly applied". The buffer was black and plain identifiers were Kitty's white, while the chrome
and the syntax captures were Catppuccin. That is how binvim works today. With no
`[colors] background`, `Config::background_color()` (`src/config.rs:419`) returns `None`. Every
buffer paint path then leaves the terminal's background alone (`reset_to_buf_bg` /
`apply_buf_bg`, `src/render.rs:88-101`). Plain text, with no tree-sitter capture or semantic
token, gets no `SetForegroundColor` at all (`draw_line_with_selection`, `render.rs:~6305`), so it
shows the terminal's own foreground. The chrome derivations (`chrome_bg`, `theme_fg`,
`theme_dim`, `theme_surface`, `theme_border`, `config.rs:423-627`) fall back to hardcoded
Catppuccin constants when the key is unset, which is why the chrome looked right.

The user chose to paint Catppuccin by default: with the key unset, binvim looks the same in every
terminal, and a light terminal no longer puts Mocha's pastels on white. Wanting the terminal's own
background (transparency, a matching terminal theme) becomes an opt-in:
`background = "Reset"`. `parse_color` already accepts `"Reset"` (`config.rs:1089`).

Decisions:

- **`background_color()` returns `Some(#1e1e2e)` when the key is unset, and `None` when it is
  `"Reset"`.** Mapping `Reset` to `None` in this one function keeps every existing `None` path
  (start page, overlay pages, buffer rows) doing exactly what it does today for that case, with
  no caller changed.
- **The chrome derivations don't read `background_color()` any more.** They read the explicit
  key, treating unset and `"Reset"` alike, so both reach their existing hardcoded Catppuccin
  constants. If they read the new default, zero-config chrome would become
  `mix(#1e1e2e, black, 0.15)` and not Mantle `#181825`, and the chrome would shift for every
  default user. As a side effect, `background = "Reset"` now gives the old zero-config look
  exactly: buffer inherits, chrome opaque Catppuccin. Today an explicit `"Reset"` makes the
  chrome transparent too (`mix` passes a non-`Rgb` colour through, and `is_dark` calls it dark),
  and that stops.
- **Plain buffer text takes `theme_fg()` whenever the buffer background is painted,** and the
  terminal's foreground when it is `"Reset"`. `theme_fg()` already honours `[colors] foreground`
  and picks `#cdd6f4` or `#4c4f69` by the background's luminance. So a shipped light theme
  (`catppuccin-latte`), which sets `background` but today leaves plain text in the terminal's
  colour, gets its own text colour too. `foreground`'s docs change from "chrome text" to "chrome
  and plain buffer text".
- The hardcoded Mantle `code_block_bg` in `draw_line_with_selection` is out of scope.

## Relevant lore

- [Config reload misses state derived from App.config](../solutions/runtime/config-reload-misses-state-derived-from-app-config.md):
  anything derived from `App.config` must survive `:config reload` / `:w config.toml`. The new
  default text colour is read from `app.config` at paint time, not cached, so the reload needs no
  new handling. Task 4 checks that by switching to `background = "Reset"` and back in a running
  binvim.
- [tmux send-keys Escape then a key arrives as Alt](../solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md)
  and [tmux new-session runs under the server's environment](../solutions/tooling/tmux-new-session-runs-under-the-servers-environment-not-the-scripts.md):
  the tmux checks send `Escape` alone, set `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` on the pane's
  command line, and wait for the mode line before typing.

## Acceptance criteria

- With no `[colors]` in the config, `capture-pane -e` of a `.txt` file (no highlighting) shows
  every buffer cell on `48;2;30;30;46` and plain text in `38;2;205;214;244`, and the status line
  and tab bar carry the same colours as before this change.
- With `background = "Reset"`, the buffer rows carry no `48;2` background and plain text has no
  `38;2` foreground. The tab bar and status line match the zero-config case.
- With a light theme (`themes/catppuccin-latte/theme.toml` pasted into the config), plain text is
  `theme_fg()`'s light-theme colour (`#4c4f69`).
- `:config reload` after switching between unset and `"Reset"` repaints the buffer to match,
  without a restart.
- `default_config.toml`, `docs/configuration.md` and the `catppuccin-mocha` theme's comment say
  what an unset `background` and `"Reset"` do now, and `CHANGELOG.md` has a Changed entry that
  names the opt-out.

## Tasks

- [x] In `src/config.rs`, add a private `explicit_background()` returning the parsed key with
  `Color::Reset` filtered to `None`. Point the five chrome derivations (`chrome_bg`, `theme_fg`,
  `theme_dim`, `theme_surface`, `theme_border`) at it in place of `background_color()`. Then make
  `background_color()` return `Some(Catppuccin Base #1e1e2e)` when the key is absent and `None`
  when it is `"Reset"`. Unit tests in `config.rs`'s `mod tests`:
  - `background_color()` is `Some(#1e1e2e)` unset, `None` for `"Reset"`, and the hex for a hex.
  - `chrome_bg()` and `theme_fg()` are equal for unset and `"Reset"` (Mantle `#181825` and
    `#cdd6f4`).
  - The existing `the_default_config_file_holds_the_defaults` still passes.

  Verify with `cargo test --bin binvim config::tests`.
- [x] In `src/render.rs`'s `draw_line_with_selection`, fall back from `syntax_color` to
  `theme_fg()` when the buffer background is painted (`buf_bg.is_some()`), so every branch that
  reads `syntax_color` gets the default. Then check the other text the buffer body prints without
  a colour (markdown table / rule rows, code-lens and inlay rows, virtual text), and give it the
  same default where it prints buffer text rather than chrome. Verify in tmux on a `.txt` and a
  `.md` file (acceptance criteria 1–3), reading `capture-pane -e`.
  Deviation: nothing else needed it. The markdown table, `<summary>` and fold-placeholder rows
  already set their own colour (`theme_fg` / `theme_emphasis` / `theme_dim` / `theme_accent`).
- [x] Update `src/default_config.toml` (the `background` and `foreground` comments),
  `docs/configuration.md` (the example block's `background` line, the chrome-palette paragraph
  and `foreground`'s role), the `themes/catppuccin-mocha/theme.toml` comment, and `CHANGELOG.md`
  (`### Changed`, naming `background = "Reset"` as the way back to the terminal's background).
  Verify with `cargo test --bin binvim config::tests` (the default-config agreement tests) and by
  reading the diff.
- [x] Check reload and the full suite: in tmux, a running binvim on a `.txt` file, set
  `background = "Reset"` in its config with `:config`, `:w`, and confirm the buffer loses its
  `48;2` background; remove the line, `:w`, and confirm it comes back (acceptance criterion 4).
  Then run `cargo test -- --test-threads=1`, pinned clippy and `cargo fmt --check`.
  Deviation: the lines went in with `:$` and `:r <file>`, not a paste. A tmux `paste-buffer -p`
  put a CR between them (`[colors]\rbackground`), which the reload rejected as invalid TOML.
  That is a bracketed-paste bug of its own, added as a task to the terminal-matrix plan.

## Files

- `src/config.rs`: `background_color()`, the five derivations at `423-627`, `parse_color`
  (already takes `"Reset"`), tests at the bottom.
- `src/render.rs`: `draw_line_with_selection` (`syntax_color` at `~6305`); `reset_to_buf_bg` /
  `apply_buf_bg` (`88-101`) stay as they are.
- `src/default_config.toml`, `docs/configuration.md`, `themes/catppuccin-mocha/theme.toml`,
  `CHANGELOG.md`.

## Verification

- `cargo test -- --test-threads=1`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`,
  `cargo fmt --check`.
- A release build in tmux with a scratch `XDG_CONFIG_HOME`, reading `capture-pane -e` for the
  three configs in the acceptance criteria (unset, `"Reset"`, catppuccin-latte), and the reload
  round trip.
- The user reopens binvim in Kitty with no `background` set and sees the Catppuccin buffer. That
  closes the report that prompted this, and fills the Kitty column's check 7 note.
