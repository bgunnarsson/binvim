# binvim

A Vim-grammar TUI editor written in Rust. Tree-sitter highlighting (Rust, TS/TSX/JSX, JS, JSON, Go, Python, C / C++, Java, Ruby, PHP, Lua, TOML, Svelte, Zig, Nix, Elixir, Dockerfile, SQL, HTML, CSS, Markdown, C#, Razor, YAML, XML / `.csproj` / `.manifest` family, Bash, `.editorconfig`, `.gitignore`), multi-server LSP fan-out (rename, code-actions, inlay hints, signature help, snippet expansion, find-references, document & workspace symbols), a built-in .NET debugger via DAP (multi-project picker, launchSettings profiles, breakpoints, stack frames, locals with lazy expansion, VS / Rider F-keys), per-language formatters dispatched by extension (biome, csharpier, gofmt/goimports, ruff, clang-format, shfmt, stylua, prettier, taplo, rufo, php-cs-fixer, google-java-format, zig fmt, nixfmt, mix format, ktfmt, sql-formatter, plus `.editorconfig` reflow on every save), real multi-cursor with Sublime-style `Ctrl-N` selections, fuzzy pickers with file-type icons and match-character highlighting, sessions with persistent per-buffer jumplists, tab bar, persistent undo, code folding, surround operations, smart-indent, OS-clipboard paste, horizontal scrolling, and a Catppuccin Mocha palette — all in one binary, no plugins.

## Features

### Editor

- **Modal editing** — normal / insert / visual (charwise, linewise, blockwise) with operators, text objects, marks, registers, dot-repeat, undo/redo, and macros.
- **Multi-cursor (mirrored, real)** — `Ctrl-click` in Normal mode adds a secondary cursor at the click position; cursors render as Lavender blocks. Enter Insert via `i` (anchored) or `a` (shifts each cursor by +1); typing and Backspace mirror at every site simultaneously (bottom-up edit order keeps indices stable). `Esc` collapses; a plain (non-Ctrl) click also collapses.
- **Multi-selection (`Ctrl-N`)** — Sublime-style. In Visual-char mode, `Ctrl-N` finds the next literal-text occurrence of the current selection and adds it as a parallel selection. Repeat to keep adding. `d`/`c`/`y` apply across every selection; `c` lands in Insert mode with a mirrored cursor at every former selection start.
- **Visual-block mode (`Ctrl-V`)** — third visual kind alongside char and line. Rectangular selection across a line span; `d`/`c`/`y` apply column-wise per row, `>`/`<` fall back to indent / outdent over the line range.
- **Surround operations** — `ds<char>` strips the surrounding pair, `cs<old><new>` swaps it, visual `S<char>` wraps the selection. Pair chars: `(/)/b`, `[/]`, `{/}/B`, `<`, `"`, `'`, `` ` ``.
- **Numeric `Ctrl-A` / `Ctrl-X`** — increment / decrement the next number on the current line. Recognises decimal, `0x…`, `0b…`, `0o…`, with optional leading `-`. Preserves leading zeros.
- **Smart indent on Enter** — copies the current line's leading whitespace and adds one indent unit after `{ [ ( :` `=>` `->`. Pressing Enter inside an auto-paired `{|}` splits the pair into three lines with the cursor on the indented middle.
- **Insert-mode word / line-start delete shortcuts.** `Alt`/`Option`+`Backspace` deletes the previous word (peels trailing whitespace, then one homogeneous run of word chars or punctuation — same as macOS Option-Delete). `Cmd`/`Super`+`Backspace` deletes from the cursor back to column 0. `Ctrl`+`Backspace` aliases to the word-delete for terminal users without a usable Option key. None of them span line boundaries.
- **HTML tag auto-completion** — typing `>` after `<div` writes `<div>|</div>`. Self-closing tags, void elements, generics (`Array<T>`), comments and declarations all skipped. Active in `.html`, `.cshtml`, `.razor`, `.jsx`, `.tsx`, `.vue`, `.svelte`, `.astro`, `.xml`, `.md`.
- **Bracket and HTML-tag matching** — when the cursor is on (or just past) a bracket or anywhere inside an HTML tag, the matching partner highlights with a Surface2 background + bold. Works through arbitrary nesting.
- **Code folding** — indent-based folds, toggled with `za`/`zo`/`zc`, with `zR`/`zM` to open/close all. Folded blocks render as `⏷ N lines`.
- **Git gutter + hunks** — leftmost gutter column shows working-tree changes against the git index: Green `▎` for added lines, Yellow `▎` for modified, Red `▁` for the line just below a deletion. Status line gains a `+A ~M -D` counter next to the branch name. `]h` / `[h` jump to next / previous hunk; `<leader>hp` previews the hunk under the cursor in a hover popup (three lines of surrounding context). `<leader>hs` stages the hunk via `git apply --cached`, `<leader>hu` unstages, `<leader>hr` discards (refuses while the buffer is dirty so unsaved edits can't be lost). Refreshed on save, on buffer switch, on initial open, and after any stage / unstage / reset.
- **Inline git blame** — `:Gblame` toggles per-line virtual text at end-of-line: author • relative age (`3d`, `2w`, `4mo`) • short SHA, in muted italic. Parsed from `git blame --porcelain`. Per-buffer toggle. Suppressed on rows that already show an inline diagnostic.
- **Persistent undo** — undo history is serialised per file under `~/.cache/binvim/undo/<hash>.json` on save and reloaded on the next session, keyed by content hash so external edits invalidate stale history.
- **System-clipboard yank + paste** — `y`, `yy`, `Y`, `:y`, visual yank, and the implicit yank on `d`/`c`/`x` mirror to the OS clipboard via `arboard` whenever they target the unnamed register. `p` / `P` (Normal *and* Visual mode) read from the OS clipboard first — anything you `Cmd-C`'d in another app wins over the in-memory yank. Named registers (`"ay`) stay local.
- **Visual-mode paste** — `p` / `P` over a selection (word / multi-line / block) swaps the selection with the register's contents. Linewise content over a charwise selection drops its trailing newline so paste doesn't open a stray blank line.
- **Yank flash** — yanked range flashes a Catppuccin Peach background for 200ms so you see what's been picked up.
- **Horizontal scrolling** — long lines scroll automatically as the cursor moves past the edge; trackpad / mouse-wheel horizontal events scroll without moving the cursor; Vim-style `zh` / `zl` (1 col) and `zH` / `zL` (half-width) work too.
- **Double-click to select word, drag to extend by words** — a second left-click at the same buffer position within 350 ms expands to the inner word and enters Visual-char. Continue holding and drag to grow or shrink the selection a word at a time; the cursor snaps to whole-word boundaries and only jumps once a new word is crossed (dragging through whitespace keeps the previous boundary).
- **Whitespace markers** — every space, tab, non-breaking space, and end-of-line surface as a muted glyph (`·`, `→`, `⎵`, `¬`). Configurable.
- **Format on save / on-demand** — `<leader>f` or `:fmt` runs the right tool per extension. biome for JS / TS / JSX / TSX / JSON / JSONC; csharpier for `.cs`; `gofmt` / `goimports` for `.go`; `ruff format` (or `black` as fallback) for `.py`; `clang-format` for `.c` / `.h` / `.cpp` / `.cc` / `.hpp` / `.cxx` / `.hxx`; `shfmt` for `.sh` / `.bash` / `.zsh`; `stylua` for `.lua`; Prettier (project-local `node_modules/.bin/prettier` first, then global) for the file types biome doesn't currently format — `.md` / `.mdx` / `.vue` / `.svelte` / `.html` / `.htm` / `.css` / `.scss` / `.less` / `.yaml` / `.yml` / `.graphql` / `.gql`; `taplo format` for `.toml`; `rufo` for `.rb`; `php-cs-fixer` for `.php` (temp-file dance — no stdin mode); `google-java-format` for `.java`; `zig fmt` for `.zig`; `nixfmt` (or `alejandra`) for `.nix`; `mix format` for `.ex` / `.exs`; `ktfmt` for `.kt` / `.kts` (temp-file dance); `sql-formatter` for `.sql`; `.editorconfig` indent reflow for `.cshtml` / `.razor` (csharpier rejects those, so we fall through). `.editorconfig` directives (final newline, trailing whitespace) apply on every save regardless of extension.
- **Auto-reload on disk change** — when an open file changes externally and the buffer isn't dirty, binvim notices via mtime poll and reloads with a status note.
- **Recents in the file picker** — most-recently-opened files surface at the top of the file picker on an empty query, persisted at `~/.cache/binvim/recents`.

### Sessions & tabs

- **Sessions** — open buffers + per-buffer cursor + viewport persist to `~/.cache/binvim/sessions/<cwd-hash>.json` on clean shutdown and restore on launch when no file argument is passed. Buffers whose paths no longer exist are silently dropped. Restored sessions drop you on the start page with the tab row above it advertising what's loaded — `H`/`L` (or `:bn`/`:bp`, `:b<n>`, a tab click) brings you into a buffer.
- **Tab bar** — every open buffer renders as a tab at the top of the screen. Active tab in Surface1 + Lavender + bold, inactive tabs in Subtext0, dirty buffers carry a Peach `+`. Click a tab to switch; click its `×` to close (refuses dirty, same as `:bd`). `‹` / `›` chevrons appear at the bar edges when tabs scroll off either side. The bar matches the editor background.

### Tree-sitter highlighting

Rust, TypeScript / TSX / JSX, JavaScript, JSON, Go, **Python**, **C / C++**, **Java**, **Ruby**, **PHP**, **Lua**, **TOML**, **Svelte**, **Zig**, **Nix**, **Elixir**, **Dockerfile** / **Containerfile**, **SQL**, HTML, CSS, Markdown, C#, **Razor** (`.cshtml` / `.razor`), **YAML**, **XML** (including `.csproj` / `.fsproj` / `.vbproj` / `.props` / `.targets` / `.config` / `.manifest` / `.nuspec` / `.resx` / `.xaml`), Bash, **`.editorconfig`**, **`.gitignore`** family (`.gitignore`, `.gitattributes`, `.dockerignore`, `.npmignore`).

Pattern-priority resolution so `(method_declaration name: (identifier) @function)` deterministically beats the catch-all `(identifier) @variable`.

A few language-specific tweaks on top of the bundled queries:

- **JSX / TSX** — overlay tags lowercase elements (`<div>`) as `@tag` (Pink) and PascalCase components (`<Foo>`, `<Foo.Bar>`) as `@constructor` (Yellow). `{expr}` braces inside JSX get treated as JSX-template syntax (`@operator`) instead of falling through to the object-literal punctuation tone.
- **Razor** — `@inject` / `@using` / `@{…}` / `@if` / `@(…)` / `@*…*@` etc. paint as `@keyword.directive`; C# inside the blocks is highlighted by the C# query. A byte-level overlay handles HTML tag / attribute names + C# keywords inside broken-parse regions (BOM headers, Tailwind `class="…[16px]…"` bracket attributes, …).
- **CSS** — replacement query so selectors and properties don't collide: `.class-name` is `@constructor` (Yellow), `#id-name` is `@label` (Sapphire), `property:` is `@property` (Lavender), `--custom-prop` is `@variable`, at-rules (`@media`/`@keyframes`/…) are `@keyword` (Mauve).
- **`.editorconfig`** — comments, `[*.cs]` section headers in Pink, `key = value` pairs with the key in Lavender, `=` in Sky, value in Green.
- **`.gitignore` family** — `#` comments, `!`-negation prefix in Mauve, patterns in Lavender.

### LSP

Per-language servers with `initializationOptions`, project-root detection, and a debounced `didChange` (50ms burst window) so rapid typing doesn't flood the server.

| Capability                  | Binding                  | Notes                                                                                                                                  |
|-----------------------------|--------------------------|----------------------------------------------------------------------------------------------------------------------------------------|
| Completion                  | auto + `Ctrl-N`/`Ctrl-P` | Multi-server fan-out — items from primary + auxiliary servers (e.g. Tailwind alongside tsserver) merge in the popup. Each row shows a colour-coded kind chip and the server-supplied `detail`. |
| Snippet expansion           | on accept                | LSP items with `insertTextFormat == 2` get their `$N` / `${N:default}` / `$0` placeholders parsed; cursor lands at `$1`, defaults mirror to later bare references. |
| Hover                       | `K`                      | Markdown parsed into structured lines — fenced code blocks tree-sitter-highlighted with the language tag's grammar.                     |
| Inlay hints                 | inline                   | `textDocument/inlayHint` annotations render between buffer chars in dim italic. Respects horizontal scroll.                            |
| Goto-definition             | `gd`                     |                                                                                                                                        |
| Find references             | `gr`                     | Results open in a fuzzy picker; Enter jumps.                                                                                           |
| Document symbols            | `<space>o`               | File outline. Hierarchy preserved with `›` separators.                                                                                  |
| Workspace symbols           | `<space>S`               | Live server-side filter as you type.                                                                                                   |
| Signature help              | auto on `(` / `,`        | Parameter being typed gets a Catppuccin Yellow highlight inside the popup.                                                             |
| Code actions                | `<leader>a`              | Picks render with kind tag. Supports both `WorkspaceEdit` and command-shaped actions; round-trips `workspace/applyEdit` from the server. |
| Rename                      | `<leader>r`              | LSP-aware. Prompt pre-fills the current word; submission applies the `WorkspaceEdit` across every affected file.                        |
| Diagnostics                 | inline + sign column     | Undercurl on the offending range, severity glyph in the gutter.                                                                        |

**Multi-server fan-out** — primary servers (rust-analyzer, tsserver, gopls, biome, OmniSharp, csharp-ls, pyright, clangd, jdtls, intelephense, …) plus auxiliaries layered on top. Tailwind class-name completion attaches alongside CSS / HTML / JSX / TSX / JS / TS / Astro / Vue / Svelte / Razor whenever Tailwind is detected (v3 `tailwind.config.*` or v4 CSS-first via a `tailwindcss` dependency in `package.json`). Emmet abbreviation expansion (`emmet-ls`) attaches to the same markup-flavoured file set, surfacing `ul>li*3>a[href]`-style snippets in the completion popup; the snippet inserter prepends the current line's leading whitespace to every continuation line so closing tags line up with the opener.

### Debugger (DAP)

Built-in .NET debugger via [netcoredbg](https://github.com/Samsung/netcoredbg). Driven by an adapter-agnostic DAP client — netcoredbg is the first entry in the registry; delve, debugpy, lldb-dap, etc. plug in with one struct.

| Capability               | Binding              | Notes                                                                                                                                                                          |
|--------------------------|----------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Start                    | `<leader>ds` / `F5`  | Walks up from the active buffer looking for `.sln` / `.git`, enumerates every `.csproj` / `.fsproj` / `.vbproj` underneath. 0 → error, 1 → straight through, >1 → project picker. Auto-restarts an active session (collapses the old `dq → ds` round-trip into one keystroke; waits up to 1.5 s for the previous debuggee to release its listening port). |
| Launch profile           | (after project pick) | Reads `Properties/launchSettings.json`. Profiles with `commandName: "Project"` (Kestrel hosting) are runnable. 0 → framework defaults; 1 → use directly; >1 → profile picker. The chosen profile's `applicationUrl` becomes `ASPNETCORE_URLS`; its `environmentVariables` flow into the launched process env. |
| Stop                     | `<leader>dq` / `Shift+F5` | Sends `disconnect terminateDebuggee:true`; closes the bottom pane.                                                                                                              |
| Continue                 | `<leader>dc` / `F5` (while paused) |                                                                                                                                                                                |
| Step over / into / out   | `<leader>dn` / `di` / `dO` / `F10` / `F11` / `Shift+F11` |                                                                                                                                            |
| Toggle breakpoint        | `<leader>db` / `F9`  | Gutter `●` marker. Survives across sessions (kept in memory + serialised on `<leader>dB` clears them per-file).                                                                  |
| Clear breakpoints (file) | `<leader>dB`         | Drops every breakpoint in the active buffer; resends to the adapter if a session's alive.                                                                                       |
| Toggle pane              | `<leader>dp`         | Bottom split. Frames + locals on the left, debug-console on the right. Auto-opens on session start, auto-closes on session end.                                                  |
| Focus pane               | `<leader>df`         | Enters `Mode::DebugPane`. `j`/`k`/`g`/`G` move locals selection; Enter/Tab/Space expands a structured value; `Ctrl-Y`/`Ctrl-E` free-scroll the left column; `J`/`K` page the console; `c`/`n`/`i`/`O` step without leaving the pane; `:` enters the command line; `Esc` returns to Normal. |
| Doc / Workspace symbols  | `<leader>do` / `dS`  | LSP pickers, scoped under the debug menu so "navigate around code while debugging" actions cluster in one place.                                                                 |

**Variable expansion** — structured locals render with `▶`/`▼` markers; expansion lazily fetches `children` per `variables_reference` and caches them across re-renders. All caches clear on `stopped`/`continued` (DAP doesn't promise vref stability between stops).

**Diagnostic surfacing** — adapter stderr (e.g. netcoredbg's `dlopen() error: libdbgshim.dylib not found`) streams into the pane's status_line and output buffer instead of vanishing into `Stdio::null()`. Unverified breakpoints, JIT-rebinding events, and `setBreakpoints` failures show up as console-category output so a never-hits is diagnosable instead of mysterious.

### Pickers

Fuzzy file picker, live grep, recents, document / workspace symbols, code actions, references, and debug-project / debug-profile prompts — opened from leader (`<space>`).

- **File-type icons** — path-based rows (Files, Recents, Buffers, Grep, References) get a Nerd Font icon per row derived from `Lang::detect` on the basename; unknown extensions fall back to a generic document glyph. Symbol / Code-action pickers stay icon-free (rows aren't files).
- **Match-character highlighting** — fuzzy-matched chars render in Catppuccin Yellow + Bold so it's obvious which letters of your query produced the row's rank.
- **Navigation** — mouse wheel moves the selection by ±3; PageUp/PageDown jump a page; `Ctrl-U`/`Ctrl-D` jump a half-page; `Home`/`End` jump to first/last; `^J`/`^K` (and arrows) move by one.

Each picker is a centered floating popup with the directory part of paths dimmed and the basename bright.

### Catppuccin Mocha defaults

Colours overridable via `~/.config/binvim/config.toml`.

## Install

### macOS — Homebrew

```sh
brew install bgunnarsson/binvim/binvim
```

The tap lives at [github.com/bgunnarsson/homebrew-binvim](https://github.com/bgunnarsson/homebrew-binvim). The formula compiles from source (`depends_on "rust" => :build`) — first install takes a minute or two while the tree-sitter grammars compile.

### Linux — install script

```sh
curl -fsSL https://binvim.dev/install.sh | sh
```

Pulls the matching musl-static tarball (`x86_64` or `aarch64`) from the latest GitHub Release, verifies its SHA-256, and drops the binary at `~/.local/bin/binvim`. Override with `BINVIM_VERSION=v0.1.0` or `BINVIM_INSTALL_DIR=/opt/bin` if needed.

### From source

```sh
cargo build --release
```

The binary lands at `target/release/binvim`. Requires a stable Rust toolchain.

## Run

```sh
binvim [path]
```

If `path` is omitted and a session exists for this cwd, the session restores (start page + tab row above it). Otherwise the start page renders alone. Press `:` for a command (`:e <path>`, `:q`) or `<space>` to open the file picker.

## Leader bindings

| Keys        | Action                                |
|-------------|---------------------------------------|
| `<space>`   | File picker                           |
| `<space>?`  | Recent files                          |
| `<space>g`  | Live grep                             |
| `<space>e`  | Yazi file manager                     |
| `<space>a`  | Code actions                          |
| `<space>r`  | Rename (LSP-aware)                    |
| `<space>R`  | Replace all (literal-string in buffer)|
| `<space>f`  | Format active buffer                  |
| `<space>bd` | Delete buffer (refuses dirty)         |
| `<space>bD` | Delete buffer (force)                 |
| `<space>ba` | Delete all buffers (refuses dirty)    |
| `<space>bA` | Delete all buffers (force)            |
| `<space>bo` | Close other buffers                   |
| `<space>bn` | Next buffer                           |
| `<space>bp` | Previous buffer                       |
| `<space>ds` | Start debug session                   |
| `<space>dq` | Stop debug session                    |
| `<space>db` | Toggle breakpoint                     |
| `<space>dB` | Clear breakpoints in active file      |
| `<space>dc` | Continue                              |
| `<space>dn` | Step over (next)                      |
| `<space>di` | Step into                             |
| `<space>dO` | Step out                              |
| `<space>dp` | Toggle debug pane                     |
| `<space>df` | Focus debug pane                      |
| `<space>do` | Document symbols (LSP)                |
| `<space>dS` | Workspace symbols (LSP)               |

Hold `<space>` (or `<space>b` / `<space>d`) for ~250 ms and a which-key popup lists the available next keys.

## Buffer / tab navigation

| Keys                  | Action                                       |
|-----------------------|----------------------------------------------|
| `H` / `L`             | Previous / next buffer (same as `:bp`/`:bn`) |
| `gt` / `gT`           | Same as `H` / `L` (Vim aliases)              |
| `Ctrl-O` / `Ctrl-I`   | Jumplist back / forward — persists across sessions per-buffer |
| Click a tab           | Switch to it                                 |
| Middle-click a tab    | Close it (refuses dirty, same as `:bd`)      |
| Click `×` on a tab    | Close it (refuses dirty)                     |
| Click `‹` / `›`       | Scroll the visible tab slice by one          |

## Window splits

| Keys              | Action                                                       |
|-------------------|--------------------------------------------------------------|
| `<C-w> v`         | Split the active window vertically (new pane on the right)   |
| `<C-w> s`         | Split the active window horizontally (new pane below)        |
| `<C-w> h/j/k/l`   | Focus the neighbouring window on the left/down/up/right      |
| `<C-w> q` / `c`   | Close the active window (refuses if it's the last one)       |
| `<C-w> o`         | Close every window except the active one                     |
| `<C-w> =`         | Reset every split ratio back to 50/50                        |

Each pane can show a different buffer — `:e other.txt`, `:b 2`,
`H` / `L` all operate on the active pane's file only. Moving focus
into a pane that points at a different buffer swaps the live buffer
state under you, so each window keeps its own cursor, viewport,
syntax highlighting, fold state, git stripe, and diagnostics.

## Ex commands

Beyond the standard `:w`, `:q`, `:e <path>`, `:bd`, `:s/pat/repl/g`, etc.:

| Command                   | Description                                                                                                                   |
|---------------------------|-------------------------------------------------------------------------------------------------------------------------------|
| `:health`                 | Open a scratch buffer summarising version, CPU / RAM share, buffers, attached LSPs (with binary path + running flag), and Tailwind config detection. `:checkhealth` works too. |
| `:fmt` / `:format`        | Run the configured formatter on the active buffer. Same path as `<leader>f`.                                                  |
| `:s/pat/repl/[gr]`        | Buffer-local substitute. `g` global, `r` interprets `pat` as a regex (`$1`/`$2` capture refs honoured in the replacement). Default is literal text. |
| `:S/pat/repl/[gr]`        | Project-wide substitute. ripgrep enumerates the files containing `pat`, opens each, applies the substitution buffer-wide, saves. `r` flips ripgrep into regex mode too. |
| `:debug` / `:dap`         | Start a debug session. `:dapstop`, `:dapc`, `:dapn`, `:dapi`, `:dapo`, `:dapb`, `:dapclear`, `:dappane` cover the rest of the surface. |
| `:noh`                    | Clear the search highlight.                                                                                                   |

## External tools

binvim spawns these on demand. Each is optional — when a binary isn't on `$PATH` (or in a relevant `node_modules/.bin/`) the editor just skips that capability.

| Tool                            | Purpose                                  | Install                                                                  |
|---------------------------------|------------------------------------------|--------------------------------------------------------------------------|
| `rust-analyzer`                 | Rust LSP                                 | `rustup component add rust-analyzer`                                     |
| `typescript-language-server`    | JS / TS / JSX / TSX LSP                  | `npm i -g typescript-language-server typescript`                         |
| `gopls`                         | Go LSP                                   | `go install golang.org/x/tools/gopls@latest`                             |
| `pyright-langserver`            | Python LSP (`basedpyright-langserver` is tried as a fallback) | `npm i -g pyright` (or `npm i -g basedpyright`)                          |
| `clangd`                        | C / C++ LSP                              | `brew install llvm` / `apt install clangd`                               |
| `bash-language-server`          | Bash / shell LSP                         | `npm i -g bash-language-server`                                          |
| `yaml-language-server`          | YAML LSP                                 | `npm i -g yaml-language-server`                                          |
| `lua-language-server`           | Lua LSP                                  | `brew install lua-language-server`                                       |
| `vue-language-server`           | Vue LSP                                  | `npm i -g @vue/language-server`                                          |
| `svelteserver`                  | Svelte LSP                               | `npm i -g svelte-language-server`                                        |
| `marksman`                      | Markdown LSP                             | `brew install marksman` (single Go binary)                               |
| `taplo`                         | TOML LSP + formatter                     | `cargo install taplo-cli --features lsp`                                 |
| `ruby-lsp`                      | Ruby LSP                                 | `gem install ruby-lsp`                                                   |
| `intelephense`                  | PHP LSP                                  | `npm i -g intelephense`                                                  |
| `jdtls`                         | Java LSP (Eclipse JDT-LS)                | `brew install jdtls` — binvim hashes the buffer's parent dir into `~/.cache/binvim/jdtls/<hash>` as the workspace data dir so projects don't trample each other |
| `zls`                           | Zig LSP                                  | `brew install zls`                                                       |
| `nil` (or `nixd`)               | Nix LSP                                  | `nix profile install nixpkgs#nil` (nixd via `nix profile install nixpkgs#nixd`) |
| `elixir-ls`                     | Elixir LSP                               | `brew install elixir-ls` (binvim probes `language_server.sh` as a fallback if the package only ships the shim) |
| `kotlin-language-server`        | Kotlin LSP                               | `brew install kotlin-language-server` (JVM-backed; same friction profile as jdtls) |
| `docker-langserver`             | Dockerfile LSP                           | `npm i -g dockerfile-language-server-nodejs`                              |
| `sqls`                          | SQL LSP                                  | `go install github.com/sqls-server/sqls@latest`                          |
| `vscode-css-language-server`    | CSS / SCSS / Less LSP                    | `npm i -g vscode-langservers-extracted`                                  |
| `vscode-html-language-server`   | HTML LSP                                 | `npm i -g vscode-langservers-extracted`                                  |
| `tailwindcss-language-server`   | Tailwind class-name completion           | `npm i -g @tailwindcss/language-server` (the unscoped npm package is an empty stub — use the scoped one) |
| `emmet-ls`                      | Emmet abbreviation completion in HTML / CSS / JSX / TSX / Vue / Svelte / Astro / Razor buffers | `npm i -g emmet-ls`                                                      |
| `astro-ls`                      | Astro LSP                                | `npm i -g @astrojs/language-server`                                      |
| `csharp-ls`                     | C# LSP (Roslyn-based, preferred)         | `dotnet tool install --global csharp-ls`                                 |
| `OmniSharp`                     | Razor / `.cshtml` IntelliSense (full)    | binvim probes `~/.local/bin/omnisharp/OmniSharp` plus `$PATH`. Drop the official tarball there. |
| `biome` (project-local)         | JSON LSP + JS / TS / JSON formatter      | `npm i -D @biomejs/biome` in the project                                  |
| `csharpier`                     | `.cs` formatter                          | `dotnet tool install --global csharpier`                                 |
| `gofmt` / `goimports`           | Go formatter (`goimports` preferred when on `$PATH` — it also organises imports) | Ships with Go; `go install golang.org/x/tools/cmd/goimports@latest` for the imports variant |
| `ruff` (or `black`)             | Python formatter (ruff preferred, black as fallback) | `pipx install ruff` / `pipx install black`                                |
| `clang-format`                  | C / C++ formatter                        | `brew install llvm` / `apt install clang-format`                          |
| `shfmt`                         | Shell-script formatter                   | `brew install shfmt` / `go install mvdan.cc/sh/v3/cmd/shfmt@latest`       |
| `stylua`                        | Lua formatter                            | `cargo install stylua` / `brew install stylua`                            |
| `prettier`                      | Formatter for the file types biome doesn't cover — Markdown / MDX, Vue, Svelte, HTML, CSS / SCSS / Less, YAML, GraphQL. Project-local preferred (walks up to `node_modules/.bin/prettier`), falls back to global | `npm i -g prettier` (or `-D` per project; Svelte additionally needs `prettier-plugin-svelte` in node_modules) |
| `rufo`                          | Ruby formatter                           | `gem install rufo`                                                       |
| `php-cs-fixer`                  | PHP formatter                            | `composer global require friendsofphp/php-cs-fixer`                       |
| `google-java-format`            | Java formatter                           | `brew install google-java-format`                                        |
| `zig fmt`                       | Zig formatter (ships with the toolchain) | `brew install zig`                                                       |
| `nixfmt` (or `alejandra`)       | Nix formatter                            | `nix profile install nixpkgs#nixfmt-rfc-style` (alejandra via `nix profile install nixpkgs#alejandra`) |
| `mix format`                    | Elixir formatter (ships with the toolchain) | `brew install elixir`                                                  |
| `ktfmt`                         | Kotlin formatter                         | `brew install ktfmt`                                                     |
| `sql-formatter`                 | SQL formatter (multi-dialect)            | `npm i -g sql-formatter`                                                 |
| `netcoredbg`                    | .NET debug adapter (DAP)                 | Build from [github.com/Samsung/netcoredbg](https://github.com/Samsung/netcoredbg). The binary and its `libdbgshim.dylib` / `ManagedPart.dll` / `Microsoft.CodeAnalysis.*.dll` siblings need to live in the same directory — symlink them next to the binary if you copy out of the build's install dir. |
| `rg`                            | Live grep backend                        | `brew install ripgrep`                                                   |
| `yazi`                          | `<space>e` file manager                  | `brew install yazi`                                                      |

binvim auto-discovers project-local binaries by walking up to the closest `node_modules/.bin/`, so a `devDependency` in your project takes precedence over a global install.

## Configuration

Optional config file at `~/.config/binvim/config.toml`:

```toml
schema_version = 1

[colors]
keyword = "#cba6f7"
"keyword.return" = "Magenta"
string = "#a6e3a1"

[start_page]
lines = [
    "  hello, world  ",
    "  press : to start ",
]

[whitespace]
show = true   # space=`·`, tab=`→ `, nbsp=`⎵`, eol=`¬`. On by default.

[line_numbers]
relative = true   # cursor row shows absolute, others show distance. On by default.
```

**`[colors]`** — values may be hex (`#rrggbb`) or a named crossterm colour. Capture names follow tree-sitter conventions (`keyword`, `string`, `function`, `type`, …); a dotted suffix matches more specifically before falling back to the head (`keyword.return` overrides `keyword`).

**`[start_page]`** — `lines` overrides the baked-in ASCII logo shown when binvim is launched with no path. Each entry renders on its own row, horizontally centered; the block as a whole is vertically centered. Omit it (or leave it empty) to keep the default logo.

**`[whitespace]`** — `show = true` (the default) renders every space as `·`, every tab as `→` plus space-fill to the tab width, every non-breaking space (U+00A0) as `⎵`, and the end-of-line as `¬`. All in the muted overlay colour. Set `show = false` to disable.

**`[line_numbers]`** — `relative = true` (the default) renders the gutter Vim-style: the cursor's row shows its absolute (1-indexed) line in a brighter Subtext1 tone, every other row shows the count of lines away from the cursor. Pairs naturally with count-prefixed motions like `5j` / `12k` / `3dd`. Set `relative = false` to fall back to plain 1-indexed numbering on every row.

A missing or malformed config is ignored — the baked-in Catppuccin Mocha palette is used.

### Theme presets

Ready-made `[colors]` blocks live in [`themes/`](themes/) — one folder per theme, each containing a `theme.toml`:

| Dark themes | Light themes |
| --- | --- |
| `dracula`, `tokyo-night`, `one-dark`, `github-dark`, `catppuccin-mocha`, `night-owl`, `gruvbox`, `nord` | `github-light`, `solarized-light`, `catppuccin-latte`, `ayu-light`, `light-owl` |

There is no built-in theme loader — copy the file contents into your `~/.config/binvim/config.toml`, e.g.:

```sh
cat themes/tokyo-night/theme.toml >> ~/.config/binvim/config.toml
```

The baked-in default is Catppuccin Mocha; `themes/catppuccin-mocha/theme.toml` mirrors it explicitly as a copy-paste starting point.

## Licence

Source-available, not open source. Copyright (c) 2026 B. Gunnarsson — see [LICENSE](LICENSE) for the full text. In short: you may read the source, run it locally, modify your own copy, and submit pull requests upstream. You may not redistribute, publicly fork, or run it as a hosted service. For anything outside that scope, contact the licensor on Twitter/X at [@bgunnarssonis](https://twitter.com/bgunnarssonis).

## Project layout

```
src/
  app.rs           slim entry — App struct + new/run + TerminalGuard
  app/
    state.rs       supporting types (Register, BufferStash, HoverState, …)
    pair.rs        bracket and HTML tag matching + auto-pair helpers
    view.rs        viewport, scrolling, folds, highlight cache, tab-bar geometry
    search.rs      search, jumps, per-line range queries for the renderer
    registers.rs   registers, macros, dot-repeat, OS-clipboard mirror
    buffers.rs     buffer switching, open/close, disk reload, recents, sessions
    save.rs        save flow, formatter, .editorconfig on-save, git branch
    edit.rs        primitive edits — insert / replace / surround / undo / number / multi-cursor mirror
    visual.rs      visual-mode helpers (incl. block + Ctrl-N multi-selection)
    dispatch.rs    apply_action — operator / motion / text-object glue
    input.rs       per-mode key handlers, mouse handler, `:`-command dispatch
    lsp_glue.rs    LSP event handling, request helpers, snippet expansion
    dap_glue.rs    DAP event handling, debug-pane focus mode, project / profile pickers
    picker_glue.rs picker open / handle / refilter, yazi shell-out
    health.rs      `:health` output
  buffer.rs        rope-backed text buffer
  command.rs       ex-command (`:`) parser
  config.rs        config loader and colour resolution
  cursor.rs        cursor + visual selection model
  editorconfig.rs  .editorconfig parser + on-save transforms
  format.rs        formatter dispatch (one arm per extension; stdin→stdout helper + temp-file dance for tools without stdin support)
  lang.rs          tree-sitter language detection and highlight cache
  lsp.rs           slim entry — re-exports public API
  lsp/
    types.rs       wire-side types + URI helpers
    specs.rs       per-extension server dispatch + workspace discovery
    client.rs      LspClient — spawn + send/recv frames
    io.rs          reader-thread loop + JSON-RPC dispatcher
    manager.rs     LspManager — fan-out + response routing
    parse.rs       response parsers
  dap.rs           slim entry — re-exports public API
  dap/
    types.rs       wire-side types — DapIncoming / DapEvent / breakpoint / frame / variable structs
    specs.rs       adapter registry, project / launchSettings discovery, $PATH lookup
    client.rs      DapClient — spawn + stdin / stdout / stderr fan-out
    io.rs          reader-thread loop (Content-Length framing, same as LSP)
    manager.rs     DapManager — protocol state machine + drain
  mode.rs          modes and operators
  motion.rs        motions
  parser.rs        keystroke → action parser
  picker.rs        fuzzy pickers
  render.rs        terminal rendering (incl. tab bar)
  session.rs       per-workspace session persistence
  text_object.rs   text objects (`iw`, `i"`, `ap`, …)
  undo.rs          undo/redo history (in-memory + on-disk persistence)
```
