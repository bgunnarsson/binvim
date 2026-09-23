# Configuration

Optional config file at `~/.config/binvim/config.toml` — `:config` opens it, and saving it applies it:

```toml
schema_version = 1

[colors]
# Editor surface
background        = "#1e1e2e"   # buffer body bg; unset = inherit terminal default
chrome_bg         = "#181825"   # tabs, popups, status segments, side panes

# Chrome neutrals
foreground        = "#cdd6f4"   # main chrome text
dim               = "#6c7086"   # muted (line numbers, hints, comments)
emphasis          = "#b4befe"   # active tab fg, multi-cursor block, picker title
surface           = "#45475a"   # active tab bg, picker selection
border            = "#585b70"   # popup borders, dividers

# Chrome accents
accent            = "#fab387"   # debug chip, breakpoint, dirty-tab dot
accent_secondary  = "#a6e3a1"   # terminal chip, active debug sub-tab, git added
chip_fg           = "#1e1e2e"   # fg on coloured chips

# Diagnostic / severity
error             = "#f38ba8"
warning           = "#f9e2af"
info              = "#89b4fa"
hint              = "#89dceb"

# Syntax captures (tree-sitter / LSP semantic tokens)
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

[copilot]
enabled = false   # GitHub Copilot via copilot-language-server (npm). Off by default.

[file_explorer]
yazi = false      # `<space>e` opens the built-in sidebar tree by default; set true to shell out to yazi.

[lsp]
semantic_tokens = true     # `textDocument/semanticTokens/full` layered over tree-sitter.
document_highlight = true  # Surface2 bg on every occurrence of the symbol under the cursor.

[install]
prompt_on_open = true      # Hint (once/language/session) when a file's LSP or formatter is missing.

[update]
check = true               # Ask crates.io once a day whether a newer binvim is out.

[clipboard]
osc52 = "auto"             # Emit OSC 52 over SSH so a remote yank reaches your local clipboard.

[keymaps.normal]
H = "^"                    # A key → the keys it types instead. Unlisted keys keep their defaults.
L = "$"
```

**`[colors]`** — values may be hex (`#rrggbb`) or a named crossterm colour. The section drives both **chrome** and **syntax** colouring.

*Chrome palette.* The neutrals + accents above (`background`, `chrome_bg`, `foreground`, `dim`, `emphasis`, `surface`, `border`, `accent`, `accent_secondary`, `chip_fg`, `error`, `warning`, `info`, `hint`) paint every chrome surface in the editor: tab bar, status line, popups (whichkey / hover / signature / notification / floating cmdline / picker / completion), terminal pane, debug pane, gutter signs, severity glyphs, buffer overlays (search / yank / multi-cursor / match-pair / doc-highlight), `:health`, `:messages`, and the start page. Set only `background` and binvim auto-derives the four neutrals (`chrome_bg`, `surface`, `border`, `foreground`, `dim`) by luminance-aware mixing — a one-line theme yields a coherent chrome. Each accent has a baked-in Catppuccin Mocha default that you can override.

*Namespaced overrides.* Every chrome role also has a dotted-namespace key that overrides only one surface, falling back to its broad theme key (and through to the Catppuccin default). Use these when you want one specific tint without re-tinting everything else:

| Family | Keys |
| --- | --- |
| Notifications | `notification.{info,warning,success,error}` |
| Git stripe | `git.{added,modified,deleted}` |
| Diagnostics | `diagnostic.{error,warning,info,hint}` |
| Gutter | `gutter.{breakpoint,pc_marker}` |
| File tree | `file_tree.folder` |
| Tab bar | `tab.{active_bg,active_fg,inactive_fg,dirty,close}` |
| Terminal pane | `terminal.{chip_bg,chip_fg,active_tab_bg}` |
| Debug pane | `debug.{chip_bg,active_tab_bg}` |
| Mode chip | `mode.{normal,insert,visual,command,search,picker,prompt,terminal,debug}` |
| Buffer overlays | `search.highlight_bg`, `yank.flash_bg`, `multi_cursor.bg`, `match_pair.bg`, `doc_highlight.bg` |

*Syntax captures.* Capture names follow tree-sitter conventions (`keyword`, `string`, `function`, `type`, …); a dotted suffix matches more specifically before falling back to the head (`keyword.return` overrides `keyword`).

**`[start_page]`** — `lines` overrides the baked-in ASCII logo shown when binvim is launched with no path. Each entry renders on its own row, horizontally centered; the block as a whole is vertically centered. Omit it (or leave it empty) to keep the default logo.

**`[whitespace]`** — `show = true` (the default) renders every space as `·`, every tab as `→` plus space-fill to the tab width, every non-breaking space (U+00A0) as `⎵`, and the end-of-line as `¬`. All in the muted overlay colour. Set `show = false` to disable.

**`[line_numbers]`** — `relative = true` (the default) renders the gutter Vim-style: the cursor's row shows its absolute (1-indexed) line in a brighter Subtext1 tone, every other row shows the count of lines away from the cursor. Pairs naturally with count-prefixed motions like `5j` / `12k` / `3dd`. Set `relative = false` to fall back to plain 1-indexed numbering on every row.

**`[clipboard]`** — `osc52` controls the terminal OSC 52 escape that binvim can emit alongside its usual `arboard` clipboard write. arboard only ever reaches the machine binvim runs on; OSC 52 asks the *terminal* to write the **local** clipboard, so a `yy` inside a binvim running over SSH lands on your own desktop.

The default is `"auto"` — emit it over SSH, skip it locally. Locally arboard has already done the job, and the sequence isn't free: it puts every yank, base64'd, into the terminal's output stream, which is where `script`, asciinema and tmux logging will keep it. Force it with `osc52 = true` (always) or `osc52 = false` (never).

**Inside tmux or screen it needs one line of their config.** Both swallow an application's OSC 52 by default. binvim sends the sequence raw *and* wrapped in the multiplexer's DCS passthrough, so enabling either route is enough — for tmux, `set -g set-clipboard on` **or** `set -g allow-passthrough on` in `~/.tmux.conf`. Your terminal emulator also has to support OSC 52 (most do; Terminal.app does not).

**`[keymaps]`** — remap keys in Normal (`[keymaps.normal]`), Visual (`[keymaps.visual]`) and Insert (`[keymaps.insert]`) mode and on the `:` / `/` command line (`[keymaps.command]`), the way Vim's `nnoremap` / `vnoremap` / `inoremap` / `cnoremap` do. Each entry maps a key, or a sequence of keys, to the keys it should type instead, in Vim notation: `H = "^"`, `J = "10j"`, `gh = "^"`, `"<C-s>" = ":w<CR>"`, `"<leader>w" = ":w<CR>"`. Keys you don't list keep their defaults. Keys that start a longer mapping wait for the next one. If they already mean something by themselves — `x` with `xx` mapped, or `J` with both `J` and `Jk` mapped — they wait at most `[keymaps] timeout` milliseconds (default 1000, Vim's `timeoutlen`) and then run; a prefix that's unfinished anyway, like `g` or `<leader>`, waits as long as it would with nothing mapped. When the next key rules the longer mapping out, the held keys run as typed — or as the shorter mapping, when they're mapped on their own. In Insert mode and on the command line a held key always means something — it's text — so with `jk = "<Esc>"` a lone `j` is still typed, once the wait runs out or as soon as the next key isn't `k`. A value can also be a table with a description, `"<leader>x" = { keys = ":w<CR>", desc = "Save" }`: `<leader>` mappings show in the which-key popup under their `desc` (or their keys), replacing a built-in row on the same key. `<Space>`, `<CR>`, `<Esc>`, `<Tab>`, `<BS>`, `<Del>`, the arrows, `<Home>` / `<End>`, `<PageUp>` / `<PageDown>`, `<F1>`–`<F24>`, `<leader>` and the `<C-…>` / `<A-…>` / `<S-…>` modifiers are understood; `<lt>` is a literal `<`, and mapping a key to `"<Nop>"` switches it off.

A mapping fires wherever a command or a motion starts — `3H`, `"aH` and `dH` all use it — but never where the key is an argument: `fH` still finds an `H`, `rH` still replaces with one, and `gH` or `<space>bH` are left alone. That holds in Insert and on the command line too, where any key may otherwise start a mapping: the register name after `Ctrl-R`, the key after `Ctrl-V`, both keys of a `Ctrl-K` digraph and the source key after `Ctrl-X` are always read as themselves. Expansions aren't remapped, so `j = "k"` next to `k = "j"` swaps the two instead of looping. A count you type multiplies a count inside the mapping: with `J = "10j"`, `3J` moves 30 lines. Macros record the keys you pressed, so replaying one applies your mappings the same way. An entry that doesn't parse, or a table or setting under `[keymaps]` that binvim doesn't know (`[keymaps.operator]`, a mistyped `timout`), is skipped, named in the status line at startup and listed under `keymaps` in `:health` — which also counts each mode's mappings; the rest of the config still loads.
**`[lsp]`** — both toggles default `true`. `semantic_tokens = false` gates the `textDocument/semanticTokens/full` request and the highlight-cache overlay off entirely (no wire traffic, no render delta). `document_highlight = false` gates `textDocument/documentHighlight` similarly. Useful if your LSP's semantic-token output collides badly with the tree-sitter pass, or if the on-every-cursor-settle highlight echo is more distracting than useful for your workflow.

**`[install]`** — `prompt_on_open = true` (the default) is the first-run toolchain nudge: when you open a file whose language is missing its primary LSP or formatter (probed on `$PATH`), a popup (the same overlay style as the file picker, so a competing notification like Copilot sign-in can't paint over it) lists what's missing — `Enter` opens `:install` preselected to that language's bundle so you review and confirm with `y`, `Esc` dismisses. It fires at most once per language per session, is skipped for large files (which don't attach a server anyway) and for languages binvim can't auto-install (Razor's OmniSharp), never opens over another overlay or mid-edit, and never nags about DAP adapters — only the LSP + formatter that make a language feel "set up." Set `prompt_on_open = false` to silence it; `<leader>i` still works on demand.

**`[update]`** — `check = true` (the default) asks crates.io on launch whether a newer binvim has been published. The result is cached in `~/.cache/binvim/update-check.json` for 24 hours, so the network call happens at most once a day no matter how often you launch; every other launch answers from the cache file. When a newer version exists it shows up in three places: a notification on startup, a line under the start-page logo (`▲ update available — binvim x.y.z (you have a.b.c)`), and the `version` row in `:health`. Nothing is uploaded — it's a plain GET for the crate's published version list, via `curl` (same as the `<space>p` package manager; no HTTP client is linked in). Failures are silent: offline, no `curl`, or a flaky network leaves the editor exactly as it was. Set `check = false` to skip it entirely.

**`[file_explorer]`** — `yazi = false` (the default) points `<leader>e` at the built-in left-side sidebar tree pane; setting `yazi = true` switches it to the yazi shell-out. In the tree pane: `j` / `k` navigate, `Enter` / `l` opens a file (or expands a folder), `h` collapses (or jumps to the parent), `g` / `G` top / bottom, `R` rebuilds after external file changes, `<space>e` (or `q` / `Esc`) from inside the pane closes it. Three-state `<leader>e` toggle from the editor: closed → focused → unfocused-but-visible → closed, so clicking into a buffer drops focus without losing the pane. The file currently open in the focused window renders in the accent colour + bold so it stays identifiable even after the j/k cursor moves elsewhere; double-click in the pane opens a file. File operations: `a` creates a new entry (a trailing `/` makes it a folder, and missing parent folders are created along the way), `r` renames the cursor entry through a prompt pre-filled with its current name — a buffer that has the file open follows the rename, so saves keep landing in the right file — and `d` deletes the cursor entry after a `y` confirmation; any other key cancels.

**`[copilot]`** — `enabled = true` opts into GitHub Copilot. binvim attaches `copilot-language-server` (npm package `@github/copilot-language-server`, install with `npm i -g @github/copilot-language-server`) as an auxiliary LSP for every buffer. Authentication happens through the language server itself: on first launch the server emits a device-flow prompt with a verification URL + user code, which binvim surfaces in the status line. Visit the URL, enter the code, and the token persists at `~/.config/github-copilot/hosts.json` for the next session. The status auto-polls every 3 s while you complete the device flow so the editor flips to "signed in" within seconds of you clicking through. binvim itself doesn't carry an HTTP client or talk to GitHub directly — the language server handles all networking and auth. Once signed in, ghost completions appear inline as muted italic text after the cursor in Insert mode (~250 ms idle pause to trigger). Accept / dismiss split: `<Tab>` accepts the Copilot ghost (it wins over the LSP popup when both are visible — the popup auto-closes on accept), `<Enter>` accepts the LSP completion popup item, any other key dismisses the ghost. Default is `enabled = false`.

**Editing it.** `:config` opens the file (creating `~/.config/binvim/` if it isn't there yet), and writing it with `:w` applies it straight away — colours, whitespace markers, keymaps and the rest, with no restart; the write message says `config reloaded`. `:config reload` does the same after an edit made outside binvim. `:config default` opens a scratch buffer listing every section and setting at its default value, commented, to copy from. The one setting a reload can't apply is `[copilot] enabled`, which takes effect on the next launch — the reload message says so.

**When something's wrong.** A missing config uses the defaults. A problem inside the file costs only the part it's in: a value of the wrong type (`show = "yes"`) leaves its section at the defaults, a colour that doesn't parse skips just that entry, and a misspelled setting or an unknown section (`[lsp] semantic_token`, `[linenumbers]`) is named rather than silently ignored — everything else still applies. The first problem shows in the status line at startup, or in the message after a reload, and `:health` lists all of them under the `config` row. A file that isn't valid TOML loads as the defaults at startup, with the line and column in the status line; on a reload it leaves the running config as it was, so saving a half-finished edit doesn't strip your theme.

## Theme presets

Ready-made `[colors]` blocks live in [`themes/`](../themes/) — one folder per theme, each containing a `theme.toml`. Every preset ships the full chrome palette (the 12 neutrals + accents), so switching theme flips every chrome surface — tab bar, popups, status line, panes — to that theme's own tones rather than leaking Catppuccin defaults.

| Dark themes | Light themes |
| --- | --- |
| `catppuccin-mocha`, `dracula`, `tokyo-night`, `night-owl`, `one-dark`, `gruvbox`, `nord`, `github-dark`, `monokai`, `visual-studio` | `catppuccin-latte`, `light-owl`, `solarized-light`, `ayu-light`, `github-light` |

There is no built-in theme loader — copy the file contents into your `~/.config/binvim/config.toml`, e.g.:

```sh
cat themes/tokyo-night/theme.toml >> ~/.config/binvim/config.toml
```

The baked-in default is Catppuccin Mocha; `themes/catppuccin-mocha/theme.toml` mirrors it explicitly with annotated comments as a copy-paste starting point.
