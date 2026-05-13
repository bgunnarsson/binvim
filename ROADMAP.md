# binvim roadmap

Forward-looking plan for binvim. Scope is mostly **language coverage** —
that is the gap between binvim today and a credible Neovim / VS Code
replacement for a general audience. See
[`LSP_ADOPTION.md`](./LSP_ADOPTION.md) for the per-language wiring
recipe; this file is the schedule and ordering on top of it.

Each language gets the full stack: **LSP** (completion / hover /
goto-def / diagnostics / rename / code actions / references / inlay
hints / signature help / symbols), **tree-sitter highlighting**, and a
**format-on-save formatter** dispatched from `src/format.rs`. Skipping
the formatter is only acceptable when no canonical one exists for the
language (e.g. YAML).

Today's coverage: **Rust, TS/JS/JSX/TSX, JSON (Biome), Go, Python,
C / C++, Bash, Lua, Java, Ruby, PHP, TOML, Vue, Svelte, Markdown,
HTML, CSS/SCSS/LESS, C# (.cs), Razor (.cshtml / .razor), Astro, YAML
(LSP + highlight, no formatter)**, plus Tailwind and Emmet as
auxiliaries. Tree-sitter grammars cover the same set minus Vue plus
XML, `.editorconfig`, and `.gitignore`.

---

## Milestone 1 — General-purpose language coverage (Tier 1) — **shipped**

Goal was to remove "binvim doesn't support my language" as a bounce
reason for a new user opening any file on day one. Shipped:

- **Python** — `pyright-langserver` primary (with `basedpyright-langserver`
  as a fallback in `cmd_candidates`), `tree-sitter-python`, formatter
  via `ruff format` (with `black` as fallback). Extensions `.py`,
  `.pyi`. Root markers: `pyproject.toml`, `setup.py`, `setup.cfg`,
  `requirements.txt`, `Pipfile`, `.git`.
- **C / C++** — `clangd` (one binary, `language_id` switched on
  extension), `tree-sitter-c` + `tree-sitter-cpp`, formatter via
  `clang-format`. Extensions `.c`, `.h`, `.cc`, `.cpp`, `.cxx`, `.hh`,
  `.hpp`, `.hxx`, `.c++`, `.h++`. Root markers: `compile_commands.json`,
  `compile_flags.txt`, `CMakeLists.txt`, `Makefile`, `.git`.
- **Bash / Shell** — `bash-language-server start`, formatter via
  `shfmt -filename=...`. Tree-sitter was already wired.
- **YAML** — `yaml-language-server --stdio`. Tree-sitter already wired.
  **No formatter** — no canonical YAML formatter exists outside of
  Prettier, which requires a node_modules install.
- **Lua** — `lua-language-server`, `tree-sitter-lua`, formatter via
  `stylua --search-parent-directories`. Critical for Neovim refugees.

Smoke tests for each new tree-sitter grammar live in `lang::tests` —
they assert that common tokens (`def`, `template`, `local function`,
etc.) come back coloured.

## Milestone 2 — High-leverage additions (Tier 2) — **shipped**

Covered the next-most-common stacks plus the heaviest of the LSPs.
Shipped:

- **Vue** — `vue-language-server --stdio` (the `@vue/language-server`
  npm package). Formatter via Prettier. Tree-sitter grammar skipped —
  `tree-sitter-vue` is at 0.0.3 and not production-ready; the LSP
  carries highlighting alone. Tailwind already attached as auxiliary
  for `.vue`.
- **Svelte** — `svelteserver --stdio`. Formatter via Prettier (the
  project's `node_modules` needs `prettier-plugin-svelte` alongside
  Prettier itself). Tree-sitter via `tree-sitter-svelte-ng` 1.0 (the
  maintained fork — upstream `tree-sitter-svelte` is stale).
- **Markdown** — `marksman server`. Formatter via Prettier.
  Tree-sitter already wired.
- **TOML** — `taplo lsp stdio` + `taplo format -` for the formatter
  (same binary, different subcommand — single Rust dep). Tree-sitter
  via `tree-sitter-toml-ng` 0.7 (upstream `tree-sitter-toml` is
  archived; the `-ng` fork is canonical now).
- **Ruby** — `ruby-lsp stdio` (Shopify-maintained). Formatter via
  `rufo -x` (rubocop's stdin output format mixes diagnostics with
  source — rufo's narrower scope makes it a cleaner fit for the
  save path). Tree-sitter via `tree-sitter-ruby` 0.23. Detects
  `Gemfile` / `Rakefile` / `Brewfile` / `Guardfile` / `Capfile` /
  `Vagrantfile` as Ruby.
- **PHP** — `intelephense --stdio`. Formatter via `php-cs-fixer fix`
  (no stdin mode, so the temp-file dance from csharpier is reused).
  Tree-sitter via `tree-sitter-php` 0.24's `LANGUAGE_PHP` (handles
  the `<?php … ?>`-inside-HTML shape).
- **Java** — `jdtls -data <workspace>`. Each project gets its own
  hashed slot under `~/.cache/binvim/jdtls/` so workspace state
  doesn't bleed between unrelated projects. Formatter via
  `google-java-format -`. Tree-sitter via `tree-sitter-java` 0.23.

## Milestone 3 — Niche but binvim-aligned (Tier 3)

Goal: serve the small, devoted communities that overlap heavily with
terminal-editor users. **Community contributions or one-off requests.**
Formatter still expected per language unless none is canonical.

- **Zig** — `zls`. Formatter: `zig fmt` (ships with the toolchain).
- **Nix** — `nil` (or `nixd`). Formatter: `nixfmt` or `alejandra`.
- **Elixir** — `elixir-ls` via `language_server.sh`. Formatter: `mix
  format -` (reads stdin when given `-`).
- **Kotlin** — `kotlin-language-server` (JVM, similar friction to
  jdtls). Formatter: `ktfmt` or `ktlint`.
- **Dockerfile** — `docker-langserver`. Needs filename-pattern
  detection (`Dockerfile`, `Dockerfile.*`) distinct from extension
  matching. Formatter: none widely adopted — skip.
- **SQL** — `sqls`. Quality varies by dialect; treat as optional.
  Formatter: `sql-formatter` (npm) when SQL files are common enough to
  matter.

---

## Per-LSP change shape

For each language the diff touches five files — see existing Astro and
Go arms as templates:

- **`src/lsp/specs.rs`** — new arm in `primary_spec_for_path`. Use
  `local_bin(<sub>, <bin>)` for `~/.local/bin/<sub>/<bin>` probes;
  `find_node_modules_bin(start, name)` for project-local node binaries.
- **`src/lang.rs`** — new `Lang::*` variant, extension entry in
  `Lang::detect()`, alias entry in `Lang::from_md_tag()`, and matching
  arms in `ts_language()` / `highlights_query()` (only if you also want
  syntax highlighting — orthogonal to LSP).
- **`src/render.rs`** — file-type icon + lang name (the two exhaustive
  matches on `Lang`).
- **`src/format.rs`** — new arm in `format_buffer`. Most stdin→stdout
  formatters can call `run_stdin_pipe(bin, args, source, label)`; only
  reach for a temp-file dance when the tool refuses stdin (csharpier).
- **`Cargo.toml`** — `tree-sitter-<lang>` crate (only if you're wiring
  highlighting).
- **`README.md`** — one row in the LSP install table and one row in the
  formatter install table.

`ServerSpec` template:

```rust
ServerSpec {
    key: "<stable-key>".into(),
    language_id: "<lsp-language-id>".into(),
    cmd_candidates: vec!["<binary>".into(), local_bin("<sub>", "<binary>")],
    args: vec![],   // or e.g. vec!["--stdio".into()]
    root_markers: vec!["...".into(), ".git".into()],
    initialization_options: Value::Null,
}
```

## Verification (per LSP)

- Open a representative file (e.g. `foo.py`) and run `:health`. Confirm
  the new server appears under **LSP servers (N running)** with the
  expected `key`, `language_id`, and detected `root`.
- Trigger completion (`<C-x><C-o>` or as-you-type) and hover on a known
  symbol; confirm the response renders.
- Run `<leader>f` (or `:fmt`) on a deliberately misformatted file —
  confirm the formatter rewrites it and the status line stays clean.
- `cargo build --release && cargo test` — the existing test suite stays
  green. LSP wiring is data; formatters are thin shells around external
  tools and don't need their own tests beyond the one already in
  `format.rs` for `apply_editorconfig_indent`.
- Rebuild release (`cargo build --release`); the user's `binvim` alias
  points at `target/release/binvim`.

## Out of scope

- No plugin system. Every server and formatter is hard-wired in
  `src/lsp/specs.rs` / `src/format.rs`.
- Tree-sitter highlighting is optional and independent of LSP wiring —
  adding a grammar without an LSP, or vice versa, is fine.
