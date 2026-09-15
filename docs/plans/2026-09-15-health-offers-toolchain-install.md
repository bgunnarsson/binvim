---
title: :health names the active buffer's missing toolchain and installs it on one key
date: 2026-09-15
status: in-progress
---

## Context

ROADMAP.md Horizon 1 asks for `:health` to be "the thing every confused newcomer is pointed at",
and to fix rather than only diagnose: "csharp-ls not found → press `y` to install". It is the last
0.6 item that can be built in-repo.

The diagnosis half already exists. The ACTIVE BUFFER box marks an LSP `✗ … NOT INSTALLED`
(`src/render.rs:4347`) and the FORMATTER box marks a missing formatter the same way
(`src/render.rs:4531`). The fix half exists too: `open_installer_for_bundle`
(`src/app/installer.rs:365`) opens `:install` with a bundle checked and the cursor on it, and it is
what `<leader>i` and the first-run toolchain picker already call. Nothing joins them — the page
says a tool is missing, every key but the scroll/dismiss set is swallowed
(`src/app/input.rs:502`), and the newcomer has to already know `<leader>i` or `:install`.

Outcome: when the active buffer's language is missing its primary LSP or formatter, the top of
`:health` says which, and one key opens the installer on that language.

Decisions made while planning (by the planner, from the code and project rules):

- **"Missing" is `missing_core_tools`, unchanged.** The first-run picker already uses it
  (`maybe_prompt_toolchain`, `src/app/installer.rs:271`): the bundle's first LSP and first
  formatter, only when auto-installable and not on `$PATH`. The key installs exactly what the page
  offers, so Manual-only tools (OmniSharp, netcoredbg) and DAP adapters stay out — the page keeps
  reporting them as `NOT INSTALLED` as it does today, with no key promising a fix it can't make.
- **The key is `i`, not the roadmap's `y`.** `i` is what `<leader>i` already means, and on every
  other surface `y` is yank — or the installer's own "run the plan" confirm, which a user
  pressing `y` twice would reach without reviewing it. The roadmap's wording was illustrative.
- **Not gated by `[install] prompt_on_open` or the once-per-session set.** Those exist so the
  unrequested first-run picker doesn't nag; `:health` is opened on purpose and should say the
  same thing every time. No large-file gate either: installing a server is harmless even if a
  large buffer won't attach it.
- **The notice goes above PROCESS**, right under the banner, in its own red-bordered SETUP box,
  so it is the first thing on the page without scrolling. The footer also leads with the key
  while a fix is on offer, since the box scrolls away and the footer doesn't.
- **The key handler resolves the fix itself** (`health_setup()`), rather than reading the last
  painted snapshot: building a full `HealthSnapshot` shells out to `ps`, and the check is two
  `$PATH` probes. `open_installer` already clears `show_health_page`, so the handler needs no
  dismissal of its own.
- **The role label is shared.** `open_toolchain_picker` maps `Role` → `"LSP"` / `"formatter"`
  inline; with a second consumer it moves to one `role_label` fn in `installer.rs`.
- **No config state is involved**, so the config-reload rule in CLAUDE.md does not apply: the
  check reads the buffer path and `$PATH` fresh on every call.

## Relevant lore

None found. The one doc in `docs/solutions` (config reload missing state derived from
`App.config`) covers caches built from the config; this work builds none.

## Acceptance criteria

- With `rust-analyzer` absent from `$PATH`, opening a `.rs` file and running `:health` shows a
  SETUP box directly below the banner naming the Rust bundle, `rust-analyzer · LSP`, and
  `press i to install`; the footer begins `i install Rust toolchain`.
- Pressing `i` there closes the dashboard and opens `:install` with the Rust row checked and the
  cursor on it.
- With the language's core tools installed, a Manual-only bundle (a `.cshtml` file), a bundleless
  language (`.json`), or a `[No Name]` buffer, there is no SETUP box, the footer is unchanged, and
  `i` does nothing.
- `i` on `:messages`, the test-results overlay, and the list overlay does nothing.
- The first-run picker's rows read exactly as before.
- README's `:health` row mentions the key; CHANGELOG `[Unreleased]` has an `Added` entry.

## Tasks

- [x] **Resolve the setup offer.** In `src/app/installer.rs`, make `bundle_for_lang`
  `pub(super)` and extract `pub(super) fn role_label(Role) -> &'static str` from
  `open_toolchain_picker`, which then calls it. In `src/app/health.rs`, add
  `pub struct HealthSetup { pub bundle_idx: usize, pub bundle: &'static str, pub missing:
  Vec<(&'static str, &'static str)> }` (label, role) with `fn from_missing(bundle_idx,
  &[&'static Tool]) -> Option<Self>` (None when the slice is empty), a
  `pub(super) fn health_setup(&self) -> Option<HealthSetup>` (buffer path → `Lang::detect` →
  `bundle_for_lang` → `missing_core_tools` → `from_missing`), and `setup: Option<HealthSetup>` on
  `HealthSnapshot`, filled in `build_health_snapshot`; re-export `HealthSetup` from `src/app.rs`
  beside `HealthSnapshot`. Verify with a new `app::health::tests`: `from_missing` with an empty
  slice is `None`; with the Rust bundle's LSP tool it carries the bundle index, `"Rust"` and
  `("rust-analyzer", "LSP")`; `health_setup` on a `[No Name]` buffer and on a `.json` path is
  `None`. `cargo test app::health::tests app::installer::tests install::tests`.
  Deviation: `HealthSetup` is not re-exported from `src/app.rs` — `render.rs` only reads the
  snapshot's fields, so the re-export was an unused import.
- [x] **Paint it.** In `build_health_rows` (`src/render.rs:4166`), when `snap.setup` is `Some`,
  push a `SETUP` section box in `p.red` after the banner's blank row and before PROCESS: one line
  `<bundle> — N not installed` in `p.text`, one indented line per missing tool as
  `<label>  ·  <role>` in `p.subtext1`, and a line `press i to install` in `p.yellow`, followed by
  `DashRow::Blank`. In `draw_health_page`, prefix the footer with `i install <bundle> toolchain · `
  when `snap.setup` is `Some`. Verify by eye in tmux against the release build (Verification
  step 2); `cargo build --release`.
- [x] **Bind `i`.** In the overlay block of `src/app/input.rs` (`:437` match), add an arm
  `KeyCode::Char('i') if normal && no_ctrl && self.show_health_page && !messages && !test_results
  && !registers`: when `health_setup()` is `Some`, call `open_installer_for_bundle` with its
  index; either way `return Ok(())`. Update the block's leading comment, which lists what passes
  through. Verify with tests in `app::input::tests`, built on the file's existing `App::new(None)`
  helper: health page up on a `[No Name]` buffer, `i` leaves `show_health_page` true and
  `installer` `None`; `:messages` up, `i` leaves `installer` `None`. And in
  `app::installer::tests`: with `show_health_page` true, `open_installer_for_bundle(idx)` clears
  it, sets `Mode::Installer`, and checks row `idx + binvim_offset()`.
  `cargo test app::input::tests app::installer::tests`.
  Deviation: the overlay key block is inline in `handle_event`, which reads from crossterm, and
  the test helpers' `replay_key` enters the per-mode handlers below it — so no unit test can press
  `i` there. The arm calls a `health_install` method in `health.rs` instead, tested in
  `app::health::tests` (`[No Name]`: dashboard stays up, no installer); the installer test is as
  planned. The arm's scoping was checked by hand in tmux against the release build: `i` on
  `:health` with Rust missing opens `:install` on Rust, and on `:registers` (the list overlay)
  does nothing. `:messages` with nothing logged is a notification, not the overlay, so it was not
  the one used.
- [ ] **Docs.** README `:health` row (`README.md:446`): add that when the active buffer's language
  is missing its LSP or formatter, the dashboard names them at the top and `i` opens `:install` on
  that language. CHANGELOG `[Unreleased]` → `### Added` entry in the 0.6.3 style (bold one-line
  summary, short paragraph, noting Manual-only tools are still only reported). Verify by reading
  both; `cargo fmt --check`.

## Files

- `src/app/installer.rs`: `bundle_for_lang` visibility, new `role_label` (used by
  `open_toolchain_picker`); reuses `open_installer_for_bundle` unchanged.
- `src/install.rs`: unchanged; `missing_core_tools` and `bundle_index_by_name` are reused.
- `src/app/health.rs`: `HealthSetup`, `health_setup`, snapshot field, new `mod tests`.
- `src/render.rs`: SETUP box in `build_health_rows`, footer in `draw_health_page`; reuses
  `push_section_box` and `SectionLine`.
- `src/app/input.rs`: `i` arm in the overlay key block.
- `README.md`, `CHANGELOG.md`.

## Verification

1. `cargo fmt --check && cargo test -- --test-threads=1` (parallel runs flake on macOS).
2. `cargo build --release`, then in tmux, with `rust-analyzer` hidden from `$PATH`
   (`PATH=/usr/bin:/bin target/release/binvim src/main.rs`, after checking
   `PATH=/usr/bin:/bin command -v rust-analyzer` prints nothing): dismiss the first-run picker,
   `:health` — the SETUP box sits under the banner naming Rust and `rust-analyzer · LSP`, and the
   footer leads with `i install Rust toolchain`. Press `i` — `:install` opens with Rust checked
   and the cursor on it. `q` to leave without running anything.
3. Same session, open `Cargo.toml` then a `.json` file, `:health` on each — TOML shows a SETUP
   box only if `taplo` is missing; JSON shows none and `i` does nothing.
4. With the normal `$PATH`, `binvim src/main.rs`, `:health` — no SETUP box, footer unchanged.
