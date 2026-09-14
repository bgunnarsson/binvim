---
title: A bad config.toml is reported, not silently dropped, and :config opens, reloads and documents it
date: 2026-09-14
status: draft
---

## Context

`config.toml` is binvim's only extension point (ROADMAP.md, Horizon 1 "config ergonomics"), and
today it fails silently: `Config::load` (`src/config.rs:331`) runs `toml::from_str(..)
.unwrap_or_default()`, so one typo anywhere — `show = "yes"`, `[lsp] semantic_token = false`, a
stray `]` — throws away the user's colours and every other setting with no message. Unknown keys
are worse: serde drops them without even failing. Only `[keymaps]` is lenient
(`Keymaps::deserialize`, `src/keymap.rs:209`), and it is the pattern to generalise. Changing the
config also means restarting the editor.

Outcome: problems are named, the rest of the file still applies, and `:config` makes the file
reachable, reloadable and self-documenting from inside the editor.

Decisions made while planning (by the planner, from the code and project rules):

- **Leniency is per section.** Each top-level table deserializes on its own; a section that fails
  falls back to its defaults and records one error naming it. Per-key recovery inside a section
  is not worth the machinery — the largest section (`[lsp]`) has four keys. `[colors]` is the
  exception and is checked per entry, because a theme is the section most likely to be long and
  a single bad hex should not unset the whole palette.
- **Unknown keys are found without a new dependency.** A small test-and-production helper
  captures the `fields` list serde's derive passes to `deserialize_struct`, so a section's known
  keys come from the struct itself and cannot drift. Unknown top-level tables are reported the
  same way against `Config`'s section names. `[keymaps]` keeps its own `errors` list and its own
  `:health` row; it is not merged.
- **A TOML syntax error keeps what is running.** At startup there is nothing running, so it means
  defaults plus the error. On reload it means the current config stays and the error is shown —
  a half-finished edit saved with `:w` must not strip the theme mid-session. A missing file is
  not an error in either case.
- **`[copilot] enabled` is not applied live.** `lsp.copilot_enabled` is mirrored once before the
  first `didOpen` (`src/app.rs:1132`), and attaching or detaching the Copilot client on open
  buffers is its own piece of work. A reload that changes it says a restart is needed. Every
  other setting is read from `app.config` at use (`render.rs` resolves colours per frame; the
  highlight cache stores capture names, not colours), so swapping the struct applies them.
- **Reload on save is keyed on the path.** `save_active` (`src/app/save.rs:47`) is the one write
  chokepoint; when the written buffer's absolute path equals the config path, it reloads and
  appends the outcome to the note it already returns, so `:w` reads
  `"config.toml" 40L written (config reloaded)`.
- **`:config default` opens a scratch buffer** holding the annotated defaults, taken from a
  checked-in `src/default_config.toml` (already inside Cargo.toml's `/src/**/*` include). Tests
  hold that file to `Config::default()` and to every struct field, so it is the one copy rather
  than a second one. The scratch buffer has no path, so it gets no TOML highlighting —
  `Lang::detect` works from paths only — which is acceptable for a reference to yank from.
- **`:config` on a missing file** creates `~/.config/binvim/` (so `:w` succeeds) and opens
  `config.toml` as a new, empty buffer, with a status hint pointing at `:config default`.

## Relevant lore

None found — the repo has no `docs/solutions`. The applicable prior art is recorded in `CLAUDE.md`
(the `keymap.rs` entry) and followed below: collect problems instead of failing the load, catch
unknown names explicitly because serde drops them, sort errors so the startup notice names the
same one every launch, and surface them twice — a startup status notice for the first, and a
durable `:health` listing for all of them.

## Acceptance criteria

- With `[whitespace]\nshow = "yes"` and a valid `[colors]` table, binvim starts with the user's
  colours applied, whitespace markers at their default, and a status notice naming
  `[whitespace]`.
- A misspelled key (`[lsp] semantic_token = false`) and an unknown table (`[linenumbers]`) are
  each reported by name; neither is silently ignored.
- A `[colors]` entry with an unparsable value (`keyword = "#12345"`) is reported and skipped; the
  other colours still apply.
- A TOML syntax error at startup gives the default config plus a notice quoting the parser's
  line/column message.
- `:health`'s ENVIRONMENT box shows `[N problems]` on the config row and one line per problem,
  alongside the existing keymaps row.
- `:config` opens `config.toml` (creating the directory when absent); `:config reload` re-reads
  it; `:w` on that buffer reloads it and says so in the write message, including the first
  problem when there are any.
- Saving a config with a syntax error leaves the running config in effect and reports the error.
- Changing `[copilot] enabled` and reloading reports that a restart is needed.
- `:config default` opens a scratch buffer with every section and key at its default, commented.
- `:con<Tab>` completes `config`; README documents the three forms; CHANGELOG has an Unreleased
  entry.

## Tasks

- [ ] **Lenient config parse.** Add `Config::parse(text: &str) -> Result<Config, String>` (Err
  only for a TOML syntax error) that parses to a `toml::Table`, deserializes each known section
  separately, falls back per section, validates `[colors]` per entry with `parse_color`, and
  reports unknown tables and keys via the `deserialize_struct` field-capture helper. Store
  problems on `Config.errors` (sorted). `Config::load` becomes: missing file → default; syntax
  error → default with that one error; otherwise the parse. Drop `Deserialize` from `Config`
  itself; move `osc52_accepts_a_bool_or_a_mode_name` onto `Config::parse`. Replace the
  keymaps-only startup notice (`src/app.rs:1161`) with one summary over `config.errors` then
  `keymaps.errors`. Verify with new `config::tests`: bad section falls back and is named; unknown
  key and unknown table are named; bad colour skipped while others apply; syntax error is `Err`;
  empty text is default with no errors. `cargo test config::tests keymap::tests`.
- [ ] **`:health` lists config problems.** Add `config_errors: Vec<String>` to `HealthSnapshot`
  (filled in `build_health_snapshot`), and in `render.rs`'s ENVIRONMENT box add a yellow
  `[N problems]` tag to the config row plus one indented line per problem, mirroring the
  keymaps `skipped` rows. Verify manually: `XDG_CONFIG_HOME=<scratch> cargo run` with a broken
  config, then `:health`.
- [ ] **`:config` and reload.** Add `ExCommand::Config(ConfigSubCmd { Open, Reload, Default })`
  parsed like `:copilot` (`src/command.rs:883`; bare `:config` → `Open`, unknown sub → `Unknown`),
  dispatched from `src/app/input.rs` next to `ExCommand::Copilot`, implemented in a new
  `src/app/config_glue.rs` (`pub(super)` methods; register in `src/app.rs`). Make `config_path`
  `pub`. `Open`: create the config dir, `open_buffer(path)`. `Reload`: read the file and call a
  pure `apply_config_text(Option<&str>) -> String` (None = file missing) that swaps
  `self.config` unless the parse is `Err`, flags a changed `copilot.enabled`, and returns the
  message. Hook `save_active` after `self.buffer.save()?` to call it when the buffer path equals
  the config path, joining its message into `format_note`. Add `"config"` to the command list in
  `src/app/cmdline_complete.rs`. Verify with tests: `command.rs` parses the three forms and
  rejects `:config bogus`; `config_glue.rs` tests on `apply_config_text` — a good text swaps and
  reports "config reloaded", a syntax error keeps the previous colours, a copilot change reports
  a restart. `cargo test command::tests app::config_glue::tests`.
- [ ] **`:config default`.** Write `src/default_config.toml`: every section and key at its
  default value, commented out, each with a one-line why taken from the struct's doc comment;
  `[colors]` lists the chrome palette keys commented. Expose it as
  `pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml")`. `Default` opens an
  empty buffer, fills it with the text, leaves it clean, and sets `display_name` to
  `[config defaults]` (the `[Command Line]` pattern in `src/app/cmdline_history.rs:129`). Verify
  with `config::tests`: the file with every `# key = value` line uncommented parses with no
  errors and equals `Config::default()` section by section; every field of every section struct
  (from the field-capture helper) appears in it. `cargo test config::tests`.
- [ ] **Docs.** README `## Configuration`: a short paragraph on `:config`, `:config reload`,
  reload on `:w`, `:config default`, how problems are reported, and that `[copilot] enabled`
  needs a restart; add `:config` to wherever README lists ex commands. CHANGELOG `[Unreleased]`
  gets an `Added` entry for `:config` and a `Fixed`/`Changed` entry for the no-longer-silent
  config. Verify by reading the rendered sections; `cargo fmt --check`.

## Files

- `src/config.rs`: `Config::parse`, `Config.errors`, rewritten `Config::load`, field-capture
  helper, `DEFAULT_CONFIG`, `pub fn config_path`; reuses `parse_color` and each section's
  existing `Default`.
- `src/default_config.toml`: new, the annotated defaults.
- `src/keymap.rs`: unchanged behaviour; its `errors` stays the keymaps source.
- `src/command.rs`: `ConfigSubCmd` + parse arm, modelled on `CopilotSubCmd`.
- `src/app/config_glue.rs` (new) + `src/app.rs` (module line, startup notice): open / reload /
  default; reuses `open_buffer` and `open_empty_buffer` (`src/app/buffers.rs`).
- `src/app/save.rs`: reload hook in `save_active`.
- `src/app/input.rs`: dispatch arm.
- `src/app/health.rs`, `src/render.rs`: config problems row, modelled on `HealthKeymaps`.
- `src/app/cmdline_complete.rs`: `"config"` candidate.
- `README.md`, `CHANGELOG.md`.

## Verification

1. `cargo fmt --check && cargo test -- --test-threads=1` (parallel runs flake on macOS).
2. `cargo build --release`, then with a scratch config dir:
   `XDG_CONFIG_HOME=$SCRATCH target/release/binvim` against a `binvim/config.toml` containing a
   valid `[colors] background`, `[whitespace] show = "yes"`, `[lsp] semantic_token = false` and
   `[linenumbers]`. Expect the background applied, a startup notice naming the first problem, and
   all three listed under `:health` → ENVIRONMENT.
3. `:config`, fix the whitespace line, `:w` — the write message says the config reloaded with
   the remaining problems; markers toggle without a restart. Change `background`, `:w`, and the
   colour changes live.
4. Introduce a stray `]`, `:w` — the error is shown and the colours stay.
5. `:config default` — a `[config defaults]` buffer opens, clean, with every section.
6. Remove the scratch `binvim/` dir, `:config` — the directory is created and `:w` writes the
   new file.
