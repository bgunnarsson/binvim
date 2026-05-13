# binvim roadmap

Forward-looking plan for binvim. Scope is mostly **language coverage** —
that is the gap between binvim today (great for Rust / web / .NET) and a
credible Neovim / VS Code replacement for a general audience. See
[`LSP_ADOPTION.md`](./LSP_ADOPTION.md) for the per-language wiring
recipe; this file is the schedule and ordering on top of it.

Today's coverage: **Rust, TS/JS/JSX/TSX, JSON (Biome), Go, HTML,
CSS/SCSS/LESS, C# (.cs), Razor (.cshtml/.razor), Astro**, plus Tailwind
as an auxiliary. Tree-sitter grammars cover the same set plus Markdown,
YAML, XML, Bash, `.editorconfig`, and `.gitignore`.

---

## Milestone 1 — General-purpose language coverage (Tier 1)

Goal: remove "binvim doesn't support my language" as a bounce reason for
a new user opening any file on day one. Ship as **one PR**.

- **Python** — `pyright` (or `basedpyright` as a fallback in
  `cmd_candidates`). Extensions: `.py`, `.pyi`. Root markers:
  `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`,
  `Pipfile`, `.git`. Tree-sitter: `tree-sitter-python`.
- **C / C++** — `clangd`, one binary covers both languages with
  `language_id` switched on extension (`c` vs `cpp`). Extensions: `.c`,
  `.h`, `.cc`, `.cpp`, `.cxx`, `.hh`, `.hpp`, `.hxx`. Root markers:
  `compile_commands.json`, `compile_flags.txt`, `CMakeLists.txt`,
  `Makefile`, `.git`. Tree-sitter: `tree-sitter-c`, `tree-sitter-cpp`.
- **Bash / Shell** — `bash-language-server start`. Extensions: `.sh`,
  `.bash`, `.zsh`. Tree-sitter already wired.
- **YAML** — `yaml-language-server --stdio`. Extensions: `.yaml`,
  `.yml`. Tree-sitter already wired.
- **Lua** — `lua-language-server`. Critical for Neovim refugees.
  Extensions: `.lua`. Root markers: `.luarc.json`, `.luarc.jsonc`,
  `init.lua`, `.git`. Tree-sitter: `tree-sitter-lua`.

**Exit criteria.** `:health` lists each new server with the expected
`key`, `language_id`, and detected `root` when a representative file is
open. Completion and hover render. `cargo test` stays green.

## Milestone 2 — High-leverage additions (Tier 2)

Goal: cover the next-most-common stacks, mostly framework / DSL gaps.
Ship in **follow-up PRs based on user demand**, not as a single batch.

- **Vue** — `@vue/language-server`. Already a Tailwind language ID in
  `lsp/specs.rs::tailwind_spec_for_path`; no primary today.
- **Svelte** — `svelte-language-server --stdio`. Same situation as Vue.
- **Markdown** — `marksman`. Single Go binary; tree-sitter highlight
  already wired.
- **TOML** — `taplo lsp stdio`. Single Rust binary.
- **Ruby** — `ruby-lsp stdio` (Shopify-maintained).
- **PHP** — `intelephense --stdio`.
- **Java** — `jdtls`. Heavy: needs `-data <workspace_dir>` and a JVM.
  Worth the friction but defer until requested.

## Milestone 3 — Niche but binvim-aligned (Tier 3)

Goal: serve the small, devoted communities that overlap heavily with
terminal-editor users. **Community contributions or one-off requests.**

- **Zig** — `zls`.
- **Nix** — `nil` (or `nixd`).
- **Elixir** — `elixir-ls` via `language_server.sh`.
- **Kotlin** — `kotlin-language-server` (JVM, similar friction to
  jdtls).
- **Dockerfile** — `docker-langserver`. Needs filename-pattern detection
  (`Dockerfile`, `Dockerfile.*`) distinct from extension matching.
- **SQL** — `sqls`. Quality varies by dialect; optional.

---

## Per-LSP change shape

For each language the diff is the same four files — see existing Astro
and Go arms as templates:

- **`src/lsp/specs.rs`** — new arm in `primary_spec_for_path`. Use
  `local_bin(<sub>, <bin>)` for `~/.local/bin/<sub>/<bin>` probes;
  `find_node_modules_bin(start, name)` for project-local node binaries.
- **`src/lang.rs:23–38`** — extend `Lang::detect()` and the matching
  `ts_language()` / `highlights_query()` arms (only if syntax
  highlighting is wanted — orthogonal to LSP).
- **`Cargo.toml`** — `tree-sitter-<lang>` crate alongside the existing
  ones (lines 17–25).
- **`README.md`** — row in the LSP install table (around line 100).

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
- `cargo build --release && cargo test` — the existing test suite stays
  green; LSP wiring is data, not logic, so no test churn is expected.
- Rebuild release (`cargo build --release`); the user's `binvim` alias
  points at `target/release/binvim`.

## Out of scope

- No plugin system. Every server is hard-wired in `src/lsp/specs.rs`.
- Tree-sitter highlighting is optional and independent of LSP wiring —
  adding a grammar without an LSP, or vice versa, is fine.
