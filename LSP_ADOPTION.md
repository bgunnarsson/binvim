# Additional LSPs for binvim — adoption-tiered list

## Context

binvim today wires up primary LSPs for **Rust, TS/JS/JSX/TSX, JSON (Biome), Go,
HTML, CSS/SCSS/LESS, C# (.cs), Razor (.cshtml/.razor), Astro**, plus Tailwind
as an auxiliary. Tree-sitter grammars cover the same set plus Markdown.

For binvim to reach the broader "could be my daily driver" audience —
particularly developers eyeing it as a Neovim/VS Code alternative — the
single biggest gap is **the rest of the popular language stack**. The list
below is opinionated, tiered by adoption ROI, and grounded in the existing
`ServerSpec` pattern in `src/lsp/specs.rs`.

Every entry below assumes the same wiring shape that's already there: one
arm in `primary_spec_for_path` (`src/lsp/specs.rs`), ext → `Lang` mapping in
`src/lang.rs:23–38`, a tree-sitter grammar in `Cargo.toml` (if highlight
support is wanted — orthogonal to LSP), and a row in the README install
table.

---

## Tier 1 — Must-have for any general editor (do first)

These are the languages a new user is most likely to open on day one. If
any are missing, "binvim doesn't support my language" becomes the bounce
reason.

| Language | LSP | Binary / install | Extensions | Root markers | Tree-sitter crate | Notes |
|---|---|---|---|---|---|---|
| **Python** | `pyright` *(or `basedpyright`)* | `npm i -g pyright` | `.py`, `.pyi` | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`, `Pipfile`, `.git` | `tree-sitter-python` | Spawn with `--stdio`. `basedpyright` is a drop-in fork some users prefer; let `cmd_candidates` try both. |
| **C / C++** | `clangd` | `brew install llvm` / `apt install clangd` | `.c`, `.h`, `.cc`, `.cpp`, `.cxx`, `.hh`, `.hpp`, `.hxx` | `compile_commands.json`, `compile_flags.txt`, `CMakeLists.txt`, `Makefile`, `.git` | `tree-sitter-c`, `tree-sitter-cpp` | One binary covers both languages. `language_id` switches on extension (`c` vs `cpp`). |
| **Bash / Shell** | `bash-language-server` | `npm i -g bash-language-server` | `.sh`, `.bash`, `.zsh` | `.git` | `tree-sitter-bash` | Spawn with `start`. Cheap install, very common file type. |
| **YAML** | `yaml-language-server` | `npm i -g yaml-language-server` | `.yaml`, `.yml` | `.git` | `tree-sitter-yaml` | Spawn with `--stdio`. Massive infra/CI/k8s usage. |
| **Lua** | `lua-language-server` | `brew install lua-language-server` | `.lua` | `.luarc.json`, `.luarc.jsonc`, `init.lua`, `.git` | `tree-sitter-lua` | Critical for the Neovim-refugee crowd binvim is courting. |

## Tier 2 — Strong additions (large communities, high leverage)

| Language | LSP | Binary / install | Extensions | Root markers | Notes |
|---|---|---|---|---|---|
| **Java** | `jdtls` (Eclipse JDT-LS) | manual tarball | `.java` | `pom.xml`, `build.gradle`, `build.gradle.kts`, `.git` | Heavy: needs `-data <workspace_dir>` and a JVM. Worth the friction for Java developers; consider deferring until requested. |
| **Vue** | `@vue/language-server` (Volar) | `npm i -g @vue/language-server` | `.vue` | `package.json`, `vue.config.*`, `vite.config.*`, `.git` | Already a Tailwind language ID in `lsp/specs.rs::tailwind_spec_for_path` — no primary today. |
| **Svelte** | `svelte-language-server` | `npm i -g svelte-language-server` | `.svelte` | `svelte.config.js`, `package.json`, `.git` | Same situation as Vue. Spawn with `--stdio`. |
| **Markdown** | `marksman` | `brew install marksman` | `.md`, `.markdown` | `.marksman.toml`, `.git` | Single Go binary. Tree-sitter highlight already wired. |
| **TOML** | `taplo` | `cargo install taplo-cli --features lsp` / `brew install taplo` | `.toml` | `.git` | Args: `lsp stdio`. Single Rust binary. |
| **Ruby** | `ruby-lsp` | `gem install ruby-lsp` | `.rb`, `.rake`, `.gemspec` | `Gemfile`, `.ruby-version`, `.git` | Shopify-maintained. Spawn with `stdio`. |
| **PHP** | `intelephense` | `npm i -g intelephense` | `.php` | `composer.json`, `.git` | Spawn with `--stdio`. Free tier covers most needs. |

## Tier 3 — Niche but binvim-aligned (smaller, devoted communities)

| Language | LSP | Binary / install | Notes |
|---|---|---|---|
| **Zig** | `zls` | `brew install zls` | Zig devs are exactly the audience that picks lightweight editors. |
| **Nix** | `nil` *(or `nixd`)* | `nix profile install nixpkgs#nil` | NixOS users overlap heavily with terminal-editor users. |
| **Elixir** | `elixir-ls` | precompiled release | Spawn with `language_server.sh`. |
| **Kotlin** | `kotlin-language-server` | precompiled release | JVM-based, similar friction to jdtls. |
| **Dockerfile** | `docker-langserver` | `npm i -g dockerfile-language-server-nodejs` | Filename pattern (`Dockerfile`, `Dockerfile.*`) — needs detection logic distinct from extension matching. |
| **SQL** | `sqls` | `go install github.com/sqls-server/sqls@latest` | Quality varies by dialect; treat as optional. |

---

## Files to modify (per LSP added)

For each new language the change is the same shape — see the existing
Astro and Go arms as templates:

- **`src/lsp/specs.rs`** — new arm in `primary_spec_for_path`. Use
  `local_bin(<sub>, <bin>)` for `~/.local/bin/<sub>/<bin>` probes;
  `find_node_modules_bin(start, name)` for project-local node binaries.
- **`src/lang.rs:23–38`** — extend `Lang::detect()` and the matching
  `ts_language()` / `highlights_query()` arms (only if you want syntax
  highlighting, which is independent of LSP support).
- **`Cargo.toml`** — `tree-sitter-<lang>` crate alongside the existing ones
  in the `tree-sitter-*` block (lines 17–25).
- **`README.md`** — row in the LSP install table (around line 100).

The `ServerSpec` template:

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

## Recommended first batch

Ship **Tier 1** as one PR — Python, clangd, Bash, YAML, Lua. That single
batch covers backend, systems, infra/scripting, and editor extensibility.
It moves binvim from "good for web devs and Rustaceans" to "general
terminal editor."

Tier 2 in follow-up PRs based on user demand.

Tier 3 as community contributions or one-off requests.

## Verification (per LSP)

- Open a representative file (e.g. `foo.py`) and run `:health`. Confirm the
  new server appears under **LSP servers (N running)** with the expected
  `key`, `language_id`, and detected `root`.
- Trigger completion (`<C-x><C-o>` or as-you-type) and hover on a known
  symbol; confirm the response renders.
- `cargo build --release && cargo test` — the existing 41-test suite stays
  green; LSP wiring is data, not logic, so no test churn expected.
- Rebuild release (`cargo build --release`) since the user's `binvim` alias
  points at `target/release/binvim`.
