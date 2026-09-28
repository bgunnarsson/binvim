# Migrating from Neovim

For Neovim users, whether on a hand-built config or a distribution like LazyVim or AstroNvim. binvim speaks Vim's grammar, so your hands already know most of it. What changes is where the rest comes from: the features your plugins added are built in, and `init.lua` gives way to one TOML file.

## What carries over

- **The grammar.** Counts, registers, operators over motions and text objects, marks, macros, `.`, the undo tree, `:s` and `:g` with ranges, the `q:` window. [Vim compatibility](vim-compatibility.md#whats-supported) lists what's supported.
- **Space as leader**, and a which-key popup: hold `<space>` (or any leader prefix) for about 250 ms and it lists the next keys.
- **`H` / `L` for the previous / next buffer**, the way LazyVim binds them. `gt` / `gT` do the same.
- **`Q` replays the last macro**, as in Neovim.
- **`gd`, `gr`, `K` and `]d` / `[d`** go to the definition, list references in a picker, show hover, and step through diagnostics.
- **`:checkhealth`** opens binvim's `:health` dashboard.

## What's different

Most of a distribution's plugins have a built-in counterpart:

| In Neovim | In binvim |
|-----------|-----------|
| telescope / fzf-lua: find files, live grep, recent files | `<space><space>`, `<space>G`, `<space>?` |
| telescope LSP pickers: document / workspace symbols | `<space>do`, `<space>dS` |
| nvim-lspconfig + mason | Servers are wired in ([LSP](lsp.md)); `:install` installs them |
| nvim-cmp / blink.cmp | Completion pops up as you type; `Ctrl-N` / `Ctrl-P` open it |
| LSP rename, code actions | `<space>r` (previews every edit first), `<space>a` |
| conform.nvim / none-ls | `<space>f` or `:fmt`, and on every save ([External tools](external-tools.md)) |
| gitsigns | The gutter stripe, `]h` / `[h`, `<space>hp` / `hs` / `hu` / `hr`, `:Gblame` |
| lazygit.nvim | `<space>gg` or `:lazygit` |
| nvim-surround / vim-surround | `ys` / `cs` / `ds` and Visual `S`, the same grammar |
| Comment.nvim / `gc` | `<space>/`, on the line or the Visual selection; there is no `gc` |
| which-key.nvim | Built in |
| neo-tree / nvim-tree | `<space>e`, or yazi with `[file_explorer] yazi = true` |
| toggleterm | `<space>tt` (a new tab each time), `<space>tp` to hide, `:terminal` |
| nvim-dap + nvim-dap-ui | `<space>ds` to start, `<space>db` for a breakpoint, and the F-keys ([Debugger](debugging.md)) |
| neotest | `<space>ss` / `sn` / `sf`, or `:test` |
| grug-far / spectre | `:S/pat/repl/` across the project |
| persistence.nvim / auto-session | Sessions save on quit and restore when you start `binvim` with no file |
| bufferline | The tab bar: one tab per buffer, except files opened into a split until `<C-w>T` |
| undotree | `g-` / `g+`, `:earlier` / `:later`, `:undolist` |
| copilot.lua | `[copilot] enabled = true` |
| AI plugins | `<space>jc` for Claude and siblings for Codex, opencode and others, in a side pane ([Keys](keys.md#leader-bindings)) |

A few keys mean something else here, on purpose. `U` redoes, `Ctrl-J` / `Ctrl-K` move the line, `<C-w>v` / `<C-w>s` open the file picker in the new split (`<C-w>V` / `<C-w>S` split onto the same buffer), and Visual `S` surrounds. `gr` opens references straight away, so Neovim 0.11's `grn` / `gra` / `grr` aren't keys here: rename is `<space>r` and code actions `<space>a`. [Where binvim differs on purpose](vim-compatibility.md#where-binvim-differs-on-purpose) has each one and why.

## Your config

binvim doesn't read `init.lua`, `init.vim` or a vimrc. Its one config file is `~/.config/binvim/config.toml`: `:config` opens it, and writing it applies it without a restart (except `[copilot] enabled`, which takes effect on the next launch). `:config default` lists every setting at its default. [Configuration](configuration.md) covers it section by section.

- **Mappings** go under `[keymaps.normal]`, `[keymaps.visual]`, `[keymaps.insert]` and `[keymaps.command]`, and work like `nnoremap` and its siblings. `vim.keymap.set("n", "<leader>w", ":w<CR>")` becomes `"<leader>w" = ":w<CR>"` under `[keymaps.normal]`, and a `desc` puts it in the which-key popup: `"<leader>w" = { keys = ":w<CR>", desc = "Save" }`.
- **The leader is Space, and can't be changed.** There is no `mapleader`; `<leader>` in a mapping means Space.
- **Options.** `:set` knows a short list for the session (`ic`, `scs`, `hls`, `tw`, `rnu`, `list`, `et` / `sw` / `ts` and a few more; [Ex commands](ex-commands.md)). Indentation comes from `.editorconfig`. Relative line numbers are on by default; `[line_numbers] relative = false` turns them off.
- **Colour schemes.** The palette is Catppuccin Mocha. [Theme presets](configuration.md#theme-presets) are `[colors]` blocks to copy into the file (Tokyo Night, Gruvbox, Nord, Dracula and more), and any colour can be set on its own.

## Getting language support

Where you'd add a server to mason, binvim offers to install the language server and formatter the first time you open a file that needs them, and `:health` shows what's attached. See [Install](install.md#binvim-install--set-up-lsps-formatters-and-dap-adapters) and [External tools](external-tools.md).

## What isn't there

- **No plugins, no Lua, no Vimscript.** Anything binvim does is built in or set in `config.toml`.
- **Abbreviations and manual folds.** Folds follow indentation, so `zf` / `zd` / `zE` have nothing to do.
- **Screen-relative `H` / `L`.** They step through buffers instead.

The rest is under [Left out](vim-compatibility.md#left-out).

## Next

- [Keys](keys.md) for every leader chord, and [Editing](editing.md) for what each feature does.
- [Debugger](debugging.md) for the adapters and the debug pane.
- [binvim vs Neovim](https://www.binvim.dev/binvim-vs-neovim.html) and [binvim vs AstroNvim / LazyVim](https://www.binvim.dev/binvim-vs-astronvim.html) for when each is the better choice.
