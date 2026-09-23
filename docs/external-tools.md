# External tools

binvim spawns these on demand. Each is optional — when a binary isn't on `$PATH` (or in a relevant `node_modules/.bin/`) the editor just skips that capability.

| Tool                            | Purpose                                  | Install                                                                  |
|---------------------------------|------------------------------------------|--------------------------------------------------------------------------|
| `rust-analyzer`                 | Rust LSP                                 | `rustup component add rust-analyzer`                                     |
| `typescript-language-server`    | JS / TS / JSX / TSX LSP                  | `npm i -g typescript-language-server typescript`                         |
| `gopls`                         | Go LSP                                   | `go install golang.org/x/tools/gopls@latest`                             |
| `pyright-langserver`            | Python LSP (`basedpyright-langserver` is tried as a fallback) | `npm i -g pyright` (or `npm i -g basedpyright`)                          |
| `clangd`                        | C / C++ LSP                              | `brew install llvm` / `apt install clangd` / `winget install LLVM.LLVM` / `scoop install llvm` |
| `bash-language-server`          | Bash / shell LSP                         | `npm i -g bash-language-server`                                          |
| `yaml-language-server`          | YAML LSP                                 | `npm i -g yaml-language-server`                                          |
| `lua-language-server`           | Lua LSP                                  | `brew install lua-language-server` / `winget install LuaLS.lua-language-server` / `scoop install lua-language-server` |
| `vue-language-server`           | Vue LSP                                  | `npm i -g @vue/language-server`                                          |
| `svelteserver`                  | Svelte LSP                               | `npm i -g svelte-language-server`                                        |
| `marksman`                      | Markdown LSP                             | `brew install marksman` (single Go binary) / `winget install Artempyanykh.Marksman` / `scoop install marksman` |
| `taplo`                         | TOML LSP + formatter                     | `cargo install taplo-cli --features lsp`                                 |
| `ruby-lsp`                      | Ruby LSP                                 | `gem install ruby-lsp`                                                   |
| `intelephense`                  | PHP LSP                                  | `npm i -g intelephense`                                                  |
| `jdtls`                         | Java LSP (Eclipse JDT-LS)                | `brew install jdtls` — binvim hashes the buffer's parent dir into `~/.cache/binvim/jdtls/<hash>` as the workspace data dir so projects don't trample each other |
| `zls`                           | Zig LSP                                  | `brew install zls` / `winget install zigtools.zls` / `scoop install zls` |
| `nil` (or `nixd`)               | Nix LSP                                  | `nix profile install nixpkgs#nil` (nixd via `nix profile install nixpkgs#nixd`) |
| `elixir-ls`                     | Elixir LSP                               | `brew install elixir-ls` (binvim probes `language_server.sh` as a fallback if the package only ships the shim) / `scoop install elixir-ls` |
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
| `clang-format`                  | C / C++ formatter                        | `brew install llvm` / `apt install clang-format` / `winget install LLVM.LLVM` / `scoop install llvm` |
| `shfmt`                         | Shell-script formatter                   | `brew install shfmt` / `go install mvdan.cc/sh/v3/cmd/shfmt@latest`       |
| `stylua`                        | Lua formatter                            | `cargo install stylua` / `brew install stylua`                            |
| `prettier`                      | Formatter for the file types biome doesn't cover — Markdown / MDX, Vue, Svelte, HTML, CSS / SCSS / Less, YAML, GraphQL. Project-local preferred (walks up to `node_modules/.bin/prettier`), falls back to global | `npm i -g prettier` (or `-D` per project; Svelte additionally needs `prettier-plugin-svelte` in node_modules) |
| `rufo`                          | Ruby formatter                           | `gem install rufo`                                                       |
| `php-cs-fixer`                  | PHP formatter                            | `composer global require friendsofphp/php-cs-fixer`                       |
| `google-java-format`            | Java formatter                           | `brew install google-java-format`                                        |
| `zig fmt`                       | Zig formatter (ships with the toolchain) | `brew install zig` / `winget install zig.zig` / `scoop install zig` |
| `nixfmt` (or `alejandra`)       | Nix formatter                            | `nix profile install nixpkgs#nixfmt-rfc-style` (alejandra via `nix profile install nixpkgs#alejandra`) |
| `mix format`                    | Elixir formatter (ships with the toolchain) | `brew install elixir` / `winget install Elixir.Elixir` / `scoop install elixir` |
| `ktfmt`                         | Kotlin formatter                         | `brew install ktfmt`                                                     |
| `sql-formatter`                 | SQL formatter (multi-dialect)            | `npm i -g sql-formatter`                                                 |
| `netcoredbg`                    | .NET debug adapter (DAP)                 | Build from [github.com/Samsung/netcoredbg](https://github.com/Samsung/netcoredbg). The binary and its `libdbgshim.dylib` / `ManagedPart.dll` / `Microsoft.CodeAnalysis.*.dll` siblings need to live in the same directory — symlink them next to the binary if you copy out of the build's install dir. |
| `dlv`                           | Go debug adapter (DAP)                   | `go install github.com/go-delve/delve/cmd/dlv@latest`                    |
| `debugpy` (Python module)       | Python debug adapter (DAP)               | `pip install debugpy` (or `pipx inject` into a venv). binvim runs it as `python3 -m debugpy.adapter`. |
| `lldb-dap`                      | Rust / C / C++ debug adapter (DAP)       | Ships with LLVM 18+: `brew install llvm` (then add `$(brew --prefix llvm)/bin` to `$PATH`). Falls back to the legacy `lldb-vscode` if `lldb-dap` isn't present. |
| `java-debug` (jdtls plugin)     | Android Java / Kotlin debug adapter (DAP) | Download `com.microsoft.java.debug.plugin-*.jar` from [github.com/microsoft/java-debug](https://github.com/microsoft/java-debug/releases) into `~/.cache/binvim/java-debug/`; jdtls loads it for `<leader>ab` attach debugging. |
| `rg`                            | Live grep backend                        | `brew install ripgrep`                                                   |
| `yazi`                          | `<space>e` file manager — optional; only needed with `[file_explorer] yazi = true`, the built-in sidebar tree is the default | `brew install yazi`                                                      |
| `sdkmanager` / `avdmanager`     | Android SDK command-line tools — emulator management (`<leader>a`), no Android Studio | `brew install --cask android-commandlinetools`, then `sdkmanager --licenses` |
| `adb`                           | Android platform-tools (device bridge)   | `brew install --cask android-platform-tools` / `apt install android-tools-adb` |
| `emulator`                      | Android emulator runtime                 | `sdkmanager emulator` — binvim locates it under `$ANDROID_HOME/emulator` |

binvim auto-discovers project-local binaries by walking up to the closest `node_modules/.bin/`, so a `devDependency` in your project takes precedence over a global install.
