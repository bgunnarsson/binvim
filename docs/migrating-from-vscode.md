# Migrating from VS Code

For VS Code users, with or without its Vim extension. binvim runs in a terminal and is modal: keys move and edit rather than type, until you ask to type. Git, a terminal and the support for language servers, formatters and debuggers are built in, so there are no extensions to find; binvim offers to install the servers and tools themselves. This page maps what you do in VS Code to the keys here.

## Modal editing in five minutes

If you've used the Vim extension, skip to [What carries over](#what-carries-over).

- **binvim starts in Normal mode**, where keys are commands. `i` enters Insert mode, where keys type text, and `Esc` goes back. The mode shows in the status line.
- **Move** with `h` / `j` / `k` / `l` (left, down, up, right), `w` / `b` by word, `0` / `$` to the line's start / end, `gg` / `G` to the file's top / bottom. The arrow keys and the mouse work too.
- **Edit** with an operator and a motion: `dw` deletes a word, `cw` changes it, `yy` copies the line, `p` pastes, `u` undoes and `U` redoes. `.` repeats the last change.
- **Select** with `v` (characters), `V` (lines) or `Ctrl-V` (a block), then `d`, `c` or `y`.
- **Commands** start with `:`. `:w` saves, `:q` quits, `:wq` does both, `:q!` quits without saving.

Copying and pasting go through the system clipboard: `y` copies to it, and `p` pastes what you copied in another app. Over SSH that clipboard is the remote machine's: a yank still reaches your own through the terminal, but `p` doesn't read it. [Editing](editing.md) covers the rest.

## What carries over

- **Many cursors.** `Ctrl-click` adds a cursor. In Visual mode, `Ctrl-N` selects the next occurrence of the selection, as `Ctrl-D` does in VS Code.
- **A tab per open file**, with a `+` on unsaved ones; a file opened into a split gets one with `<C-w>T`. Click a tab to switch, or use `H` / `L`.
- **Language features** from each language's server: completion, hover, go to definition, rename, code actions, inlay hints and diagnostics. See [LSP](lsp.md).
- **GitHub Copilot** ghost text, with `[copilot] enabled = true`.
- **The integrated terminal**, a file tree, a debugger, a test runner and a task runner.

## What's different

The leader key is Space: `<space>` then a key runs a command, and holding `<space>` lists what comes next.

| In VS Code | In binvim |
|------------|-----------|
| Quick Open (`Ctrl-P`) | `<space><space>`; `<space>?` for recent files |
| Find in files (`Ctrl-Shift-F`) | `<space>G` |
| Replace in files | `:S/pat/repl/` |
| Command palette (`Ctrl-Shift-P`) | `:` with `Tab` completion, or hold `<space>` for the which-key menu |
| Go to definition (`F12`) | `gd` |
| Find all references (`Shift-F12`) | `gr` |
| Hover | `K` |
| Rename symbol (`F2`) | `<space>r`, which previews every edit first |
| Quick fix (`Ctrl-.`) | `<space>a` |
| Go to symbol in file (`Ctrl-Shift-O`) | `<space>do` |
| Go to symbol in workspace (`Ctrl-T`) | `<space>dS` |
| Next / previous problem (`F8`) | `]d` / `[d` |
| Format document (`Shift-Alt-F`) | `<space>f`, and on every save |
| Toggle line comment (`Ctrl-/`) | `<space>/` |
| Move line up / down (`Alt-↑` / `Alt-↓`) | `Ctrl-K` / `Ctrl-J` |
| Explorer (`Ctrl-Shift-E`) | `<space>e` |
| Terminal (`` Ctrl-` ``) | `<space>tt` for a new tab, `<space>tp` to hide or show |
| Split editor (`Ctrl-\`) | `<C-w>v` (opens the file picker for the new side) |
| Switch editor tab (`Ctrl-Tab`) | `H` / `L` |
| Close editor (`Ctrl-W`) | `<space>bd` |
| Source control | The gutter stripe, `]h` / `[h`, `<space>hs` to stage a hunk, `<space>gg` for lazygit |
| Run and Debug (`F5`, `F9`, `F10`, `F11`) | The same F-keys, and `<space>ds` / `<space>db` ([Debugger](debugging.md)) |
| Testing view | `<space>ss`, or `:test` |
| Run task | `<space>mm`, or `:task` |
| Problems panel | `:health` for the servers; diagnostics sit in the gutter and the line |
| Chat / AI extensions | `<space>jc` for Claude and siblings for Codex, opencode and others, in a side pane ([Keys](keys.md#leader-bindings)) |

## Your config

binvim doesn't read `settings.json` or `keybindings.json`. Its one config file is `~/.config/binvim/config.toml`: `:config` opens it, and saving it applies it without a restart (except `[copilot] enabled`, which takes effect on the next launch). `:config default` lists every setting at its default. [Configuration](configuration.md) covers it section by section.

- **Key bindings** go under `[keymaps.normal]`, `[keymaps.visual]`, `[keymaps.insert]` and `[keymaps.command]`. `"<C-s>" = ":w<CR>"` under `[keymaps.normal]` makes `Ctrl-S` save, and `"<leader>w" = { keys = ":w<CR>", desc = "Save" }` adds a row to the which-key menu. The leader is always Space.
- **Themes.** The palette is Catppuccin Mocha. [Theme presets](configuration.md#theme-presets) include `visual-studio`, `github-dark`, `github-light`, `one-dark` and more, as `[colors]` blocks to copy into the file.
- **Indentation and whitespace** come from the project's `.editorconfig`.

## Getting language support

Where VS Code suggests an extension, binvim offers to install the language server and formatter the first time you open a file that needs them, and `:health` shows what's attached. See [Install](install.md#binvim-install--set-up-lsps-formatters-and-dap-adapters) and [External tools](external-tools.md).

## What isn't there

- **No extensions marketplace.** Everything binvim does is built in or set in `config.toml`; there is no plugin API.
- **No settings screen.** Settings are the TOML file, and `:health` names a setting it doesn't understand.
- **No GUI.** binvim runs in a terminal; [Terminals](terminals.md) lists how each one fares.

## Next

- [Keys](keys.md) for every leader chord, and [Editing](editing.md) for what each feature does.
- [Debugger](debugging.md) for the adapters, breakpoints and the debug pane.
- [binvim vs VS Code](https://www.binvim.dev/binvim-vs-vscode.html) for when each is the better choice.
