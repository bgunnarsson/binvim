# Vim compatibility

binvim speaks Vim's grammar: counts, registers, operators over motions and text objects, marks, macros and `.` behave the way Vim users expect. The [Editing](editing.md) bullets describe each feature and the [Ex commands](ex-commands.md) table lists the `:` commands; this section is the map — what's there, where binvim differs from Vim on purpose, and what's left out.

## What's supported

- **Insert mode** — `Ctrl-W`, `Ctrl-U`, `Ctrl-R {reg}` (and `Ctrl-R =`), `Ctrl-A`, `Ctrl-T` / `Ctrl-D`, `Ctrl-V {key}`, `Ctrl-E` / `Ctrl-Y`, `Ctrl-O`, `Ctrl-K {a}{b}` digraphs, `Ctrl-X Ctrl-N` / `Ctrl-P` / `Ctrl-L` / `Ctrl-F` completion, and `Ctrl-C` to leave.
- **Motions** — `%`, `{` / `}`, `(` / `)`, `<CR>` / `+` / `-` / `_` / `|`, `gj` / `gk` / `g0` / `g^` / `g$` / `gm` / `gM`, `]d` / `[d`, and `[{` / `]}` / `[(` / `])`.
- **Marks and jumps** — `''`, `'.`, `` `^ ``, `'[` / `']`, `'<` / `'>`, file marks `A`–`Z` across buffers, `gv`, `gi`, the change list (`g;` / `g,`, `:changes`), `:marks`, `:jumps`, and `Ctrl-^` / `:e#` / `:b#`.
- **Operators** — `gu` / `gU` / `g~` / `g?`, `ys` / `yss` / `yS` surround, `gJ`, `R` Replace mode, `gp` / `gP` / `]p` / `[p`, `=` re-indent, `gq` / `gw` wrap, `!{motion}` / `!!` filter, and Visual `Ctrl-A` / `Ctrl-X` / `g Ctrl-A` / `g Ctrl-X`.
- **Text objects** — `ip` / `ap`, `is` / `as`, `it` / `at`, `ia` / `aa`, and tree-sitter `af` / `if` / `ac` / `ic`.
- **Search and substitute** — Vim regex with smart case, offsets (`/pat/e+1`, `/pat/s-1`, `/pat/+2`) and `//`, `gn` / `gN`, highlight while typing, `:s` with `\1`, `&` and the `c` / `i` / `I` / `n` flags, and `&` / `g&` / `:&` / `:&&` / `:~`.
- **Ex commands** — the full range grammar, `:m` / `:t` / `:j` / `:>` / `:<` / `:d x` / `:y x` / `:pu`, `:le` / `:ri` / `:ce` / `:retab`, `:normal`, `:g` / `:v`, `:sort`, `:!` / `:r !` / `:w !`, `:sp` / `:vs` / `:new` / `:vnew` / `:only` / `:close`, `:cd` / `:pwd`, `:bufdo` / `:windo` / `:cdo` / `:cfdo`, `:grep` / `:vimgrep` / `:make`, `:set`, and `ZZ` / `ZQ` / `:x` / `:wa` / `:qa` / `:wqa` / `:xa` / `:e!`.
- **Registers and the command line** — numbered `"1`–`"9`, `"-`, the read-only `".` / `"%` / `":` / `"/` / `"#`, `"A`–`"Z` append, `"=` arithmetic, `@:`, `Ctrl-R` / `Ctrl-W` / `Ctrl-U` / `Ctrl-B` / `Ctrl-E` / `Ctrl-P` / `Ctrl-N` in the `:` and `/` prompts, and the `q:` / `q/` / `q?` window.
- **Visual mode** — block `I` / `A` / `$A`, `O`, line-wise `D` / `X` / `Y` / `C` / `R`, `*` / `#`, char and line `I` / `A`, and `Ctrl-C`.
- **Undo** — an undo tree with `g-` / `g+`, `:earlier` / `:later` by steps, time or writes, and `:undolist`.
- **Tags** — `Ctrl-]`, `Ctrl-T`, `g]`, `:tag`, `:tags`, `:tselect`, `:pop` and `:tnext` / `:tprevious` / `:tfirst` / `:tlast` over a ctags `tags` file, the nearest one above the buffer. binvim reads the file and never writes one: run `ctags -R` yourself. Useful where no language server gives `gd` a definition.
- **Folds, scrolling and info** — `zj` / `zk` / `zv` / `zO` / `zC` / `zA` / `zr` / `zm` / `zx`, `z<CR>` / `z.` / `z-` / `zs` / `ze`, `Ctrl-G`, `g Ctrl-G`, `ga`, `gx`, `gf` / `<C-w>f`, `gI` and `Ctrl-L`.

## Where binvim differs on purpose

These are binvim's own keys, and they stay:

- **`H` / `L` step through buffers.** Buffers are binvim's tabs ([Buffer / tab navigation](keys.md#buffer--tab-navigation)), so they get keys on the home row. Vim's jump to the top / bottom of the screen has no key here.
- **`U` redoes**, next to `u`; `Ctrl-R` redoes too. Vim's line-undo `U` isn't implemented.
- **`Q` replays the last macro**, as in Neovim. Vim's `Q` enters Ex mode, which binvim doesn't have.
- **`<C-w>v` / `<C-w>s` open the file picker** in the new pane, since a split usually wants a second file. `<C-w>V` / `<C-w>S` and `:sp` / `:vs` split onto the same buffer, as Vim does.
- **Insert `Ctrl-N` / `Ctrl-P` open LSP completion**, the better list when a server is attached. Vim's keyword completion is `Ctrl-X Ctrl-N` / `Ctrl-X Ctrl-P`.
- **`Ctrl-J` / `Ctrl-K` move the current line down / up.** In Vim, `Ctrl-J` is a second `j` and `Ctrl-K` does nothing in Normal mode.
- **`:update` upgrades the toolchain** ([`binvim-install`](install.md#binvim-install--set-up-lsps-formatters-and-dap-adapters)), the partner of `:install`. `:w` writes, and `:x` writes only when there are changes.
- **Visual `S` wraps the selection in a pair**, as vim-surround's does; Vim's built-in `S` changes the lines. binvim's `ys` / `cs` / `ds` family is vim-surround's grammar, and the tie goes to the mistake that's recoverable: `S` typed expecting a line change waits for a pair character, which `Esc` cancels, while `S)` typed expecting surround would delete the selection.
- **Visual `K` is LSP hover at the cursor**, the same lookup as Normal `K`, and leaves Visual. Vim looks the selection up with `keywordprg`, which would be a second meaning and new configuration. A server answers a position for the whole symbol around it, so the cursor is the point sent — the selection's start could be the indentation `V` begins at.
- **Char- and line-wise Visual `I` / `A` follow Vim's documented rules.** Vim's own behaviour in the corners its help doesn't cover (`VjA` from column 0, `vkI` going backwards) differs, and isn't copied.

## Left out

- **Abbreviations** (`:ab`, `:iab`) — they'd need new configuration surface.
- **Manual folds** (`zf`, `zd`, `zE`) — folds follow indentation.
- **Vimscript** — the `"=` register evaluates arithmetic only, and `:set` knows the short list of options in the [Ex commands](ex-commands.md) table; any other gives an error.
