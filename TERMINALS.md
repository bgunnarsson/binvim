# Terminal compatibility

binvim draws everything itself through escape sequences, so what it looks like and which keys it
can tell apart depend on the terminal. This file is the checklist every terminal is held to and
the results for the ones that have been through it.

## What binvim asks of a terminal

| Feature | Where | If the terminal lacks it |
| --- | --- | --- |
| Kitty keyboard protocol, disambiguate flag (`CSI > 1 u`) | `TerminalGuard::enable` (`app.rs`), re-pushed after the picker, lazygit and installer suspends | Keys arrive in the legacy encoding: `Ctrl-[` is `Esc`, `Ctrl-i` is `Tab`, Super/Cmd isn't reported |
| Mouse capture (SGR) | `app.rs` | Clicks, drags and the wheel do nothing |
| Bracketed paste | `app.rs`, `handle_paste` (`app/input.rs`) | A paste types out key by key, and autoindent staircases it |
| Synchronized output (DEC 2026) | `render.rs` | Frames can tear while scrolling fast |
| Undercurl with an underline colour (`4:3`, `58;2`) | `render.rs` | Diagnostics show a plain underline, or none |
| Italics | `render.rs` | Markdown `*italic*`, inlay hints and ghost text lose their slant |
| Cursor shape (DECSCUSR) | `render.rs` | The cursor stays a block in Insert and Replace |
| Truecolor (`38;2`, `48;2`) | everywhere | Colours are rounded to the terminal's palette |
| OSC 52 clipboard, DCS-wrapped under tmux and screen | `app/registers.rs` | A yank over SSH doesn't reach your clipboard |
| Unicode widths matching `unicode-width` | `render.rs`, `buffer.rs` | Text after a wide character or emoji is drawn a column off |
| A Nerd Font | `render.rs` (status line, pickers, file tree) | Icons show as boxes or `?` |

## How to run it

From the repo root, on the build you're recording:

```sh
cargo build --release
target/release/binvim --version
target/release/binvim docs/terminal-check.md
```

`docs/terminal-check.md` holds the text the checks look at: a line of wide characters, one of
emoji clusters, one mixing both, a styled line and a long line. `docs/terminal-check.rs` carries a
syntax error for the undercurl check and needs `rust-analyzer` on `PATH`. Lines are named by
their first word: `:6` jumps to *Wide*, `:7` *Emoji*, `:8` *Mixed*, `:9` *Styles*, `:11` *Long*.
If a "Set up Markdown" install prompt appears, `Esc` dismisses it.

Run with your own config. When a check fails, run it again with the defaults before recording it
(`XDG_CONFIG_HOME=$(mktemp -d) target/release/binvim docs/terminal-check.md`), and record which
one failed.

On macOS, checks that use Alt need the terminal's Option key to send Alt (Meta). Most terminals
leave it off by default: Ghostty `macos-option-as-alt = true`, Kitty
`macos_option_as_alt yes`, WezTerm `send_composed_key_when_left_alt_is_pressed = false`, Alacritty
`option_as_alt = "Both"`, Terminal.app *Use Option as Meta key*.

Record each cell as:

- **✓** — passed as written.
- **n/a** — the check can't apply here, with the reason in the notes under the matrix.
- **a link** — to the `KNOWN_ISSUES.md` entry when the terminal is at fault, or to the commit
  that fixed binvim when binvim was.

A cell is never a bare "fail".

## The checklist

1. **Launch and quit.** Run `binvim` with no file. The logo draws centred. `:q` then Enter.
   *Pass:* the shell prompt comes back on a clean screen, with no `[>1u`, `[<u`, `u` or other
   stray characters, and `echo ok` prints `ok`.
2. **Keys the keyboard protocol re-encodes.** `:9` Enter, `A`, type ` one two`, `Ctrl-w`.
   *Pass:* ` two` is gone. Enter, Tab, `x`, Backspace. *Pass:* a new, indented line with no `x`.
   `Ctrl-c`. *Pass:* `NORMAL`. `:e!` Enter to drop the edit. `gg`, `G`, `Ctrl-o`. *Pass:* the
   cursor is back on line 1. `Ctrl-i`. *Pass:* it is on the last line again.
3. **Esc response time.** `i`, then `Esc`, five times in a row at typing speed.
   *Pass:* the mode chip flips to `NORMAL` with no delay you can see, every time.
4. **`Ctrl-[` and Alt.** `i`, `Ctrl-[`.
   *Pass:* `NORMAL`, as with `Esc`. Then `:9` Enter, `A`, type ` foo bar`, `Alt-Backspace`.
   *Pass:* ` bar` is gone, the chip still reads `INSERT`. `Esc`, `u`. Then `i`, `Esc` and `j`
   quickly one after the other. *Pass:* `NORMAL`, the cursor moved down a line, nothing was typed.
5. **Shift and Ctrl arrows.** `:8` Enter, `0`. Press Shift-Right, Ctrl-Right, Shift-Left,
   Ctrl-Left in Normal, then again after `i`.
   *Pass:* each press moves the cursor one character, as the plain arrow does, and nothing is
   typed (no `;2C`, `;5D`, `C` or `D`). `Esc`, and `u` if anything changed. On macOS, Ctrl-Left and
   Ctrl-Right switch Spaces unless that shortcut is off; that is n/a, not a failure.
6. **Cursor shape.** *Pass:* a block in Normal, a bar after `i`, a block after `Esc`, an
   underline after `R`, a block after `Esc`. After `:q`, the shell has its own cursor back.
7. **Truecolor.** `:health`, find the Terminal section, then `q`.
   *Pass:* `truecolor yes`. binvim reads that from `$COLORTERM`, so on a terminal that supports
   24-bit colour but doesn't set it, record the variable's value in the notes.
8. **Italic and bold.** Look at the *Styles* line in Normal.
   *Pass:* `italic` is slanted, `bold` is bold, and the `*` markers are hidden.
9. **Nerd Font glyphs.** *Pass:* the status line's segments end in solid arrow shapes, the
   language name at its right has a file icon, and `<space><space>` lists files with an icon
   each. None is a box or `?`. `Esc`. With no Nerd Font installed, n/a.
10. **Wide characters.** `:6` Enter, `0`, `W`, then `l` five times.
    *Pass:* the cursor covers each CJK character whole, two cells wide, and steps one character
    per press. `$` puts it on the final `.` with `¬` right after it, nothing overlapping.
11. **Emoji clusters.** `:8` Enter, `0`, `W`, then `l` six times.
    *Pass:* the cursor covers `漢`, `b`, `👍🏽`, `c`, `🇮🇸`, `d` in turn, each whole, and `¬` sits right
    after `d`. With the cursor on `👍🏽`, `x` deletes it whole (`a漢bc🇮🇸d`), then `u`. On `:7`, the
    family `👨‍👩‍👧` draws as one glyph and `¬` stays at the end of the line as the cursor crosses it.
    A terminal that draws a cluster at a different width than `unicode-width` gives it is a
    `KNOWN_ISSUES.md` entry, not a binvim bug.
12. **Long line.** `:11` Enter, `$`.
    *Pass:* the view scrolls right, `END¬` is visible and the line-number gutter is intact. `0`
    scrolls back with nothing left over from the scrolled view.
13. **Mouse.** Click the `b` of `bold` on the *Styles* line. *Pass:* the cursor goes there.
    Drag from `i` of `italic` to the `d` of `bold`. *Pass:* a Visual selection covering exactly
    that. `Esc`. `:e src/render.rs`, then scroll the wheel. *Pass:* the view scrolls. `:bd`.
14. **Bracketed paste.** Copy these three lines from outside binvim:
    ```
    fn paste() {
        call(1, "two");
    }
    ```
    In the fixture, `G`, `o`, then paste with the terminal's paste key.
    *Pass:* the three lines arrive as copied, with the indent unchanged (no staircase) and no
    doubled `(` or `"`; the chip still reads `INSERT`. `Esc`, then one `u` removes all three.
15. **Synchronized output.** `:e src/render.rs`, hold `Ctrl-d` for two seconds, then `Ctrl-u`,
    then hold `j`.
    *Pass:* no torn frames (half old screen, half new) and no cursor flashing across the screen.
    `:bd`.
16. **Undercurl.** `:e docs/terminal-check.rs` and wait for rust-analyzer.
    *Pass:* line 4 shows `Syntax Error: expected expression`, and the `;` has a curly underline in
    the error colour (red). A straight underline means the terminal doesn't draw styled
    underlines. No underline and no message means the LSP isn't running — check `:health`.
    `:bd`.
17. **Resize.** Make the window narrower and shorter, then wider and taller.
    *Pass:* binvim redraws at each size with the status line on the bottom row and nothing left
    over from the previous size.
18. **OSC 52.** In the shell of this terminal:
    `printf '\e]52;c;%s\a' "$(printf osc52-ok | base64)"`, then paste anywhere.
    *Pass:* `osc52-ok`. That shows the terminal accepts OSC 52. binvim's side is checked in the
    over-SSH row, where it matters: `:9` Enter, `yy`, then paste on the local machine.
    *Pass:* the *Styles* line. Locally, n/a: binvim writes the clipboard directly and skips OSC 52.
19. **`:terminal`.** `:terminal`. *Pass:* a shell pane opens at the bottom, the chip reads
    `TERMINAL`. Run `printf '\e[31mred\e[0m\n'`. *Pass:* `red` in red. Run `cat -v`, press
    `Ctrl-[`. *Pass:* `^[` is printed and the chip still reads `TERMINAL`. `Ctrl-c`, then `Esc`.
    *Pass:* the chip reads `NORMAL`. A terminal without the Kitty keyboard protocol sends `Ctrl-[`
    as `Esc`, so the pane loses focus instead: record the `KNOWN_ISSUES.md` link.
20. **lazygit round trip.** `<space>gg` (needs `lazygit`; it starts in the buffer's directory, so
    the fixture has to be the one in this repo). *Pass:* lazygit fills the screen.
    `q`. *Pass:* binvim redraws as it was, and checks 4, 5 and 13 still pass.
21. **Closing with unsaved changes.** `:9` Enter, `A`, type ` unsaved`, `Esc`. Wait five
    seconds, then close the window (over SSH, drop the connection with `Enter`, `~`, `.`).
    Open the terminal again and `binvim docs/terminal-check.md`.
    *Pass:* binvim reports recovered changes and the *Styles* line ends in ` unsaved`. `:e!`
    discards them and removes the recovery file.

## Results

| # | Check | Ghostty | Kitty | WezTerm | Alacritty | tmux | Windows Terminal | over SSH | Terminal.app |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | Version | | | | | | | | |
| 1 | Launch and quit | | | | | | | | |
| 2 | Re-encoded keys | | | | | | | | |
| 3 | Esc response | | | | | | | | |
| 4 | `Ctrl-[` and Alt | | | | | | | | |
| 5 | Shift / Ctrl arrows | | | | | | | | |
| 6 | Cursor shape | | | | | | | | |
| 7 | Truecolor | | | | | | | | |
| 8 | Italic and bold | | | | | | | | |
| 9 | Nerd Font glyphs | | | | | | | | |
| 10 | Wide characters | | | | | | | | |
| 11 | Emoji clusters | | | | | | | | |
| 12 | Long line | | | | | | | | |
| 13 | Mouse | | | | | | | | |
| 14 | Bracketed paste | | | | | | | | |
| 15 | Synchronized output | | | | | | | | |
| 16 | Undercurl | | | | | | | | |
| 17 | Resize | | | | | | | | |
| 18 | OSC 52 | | | | | | | | |
| 19 | `:terminal` | | | | | | | | |
| 20 | lazygit round trip | | | | | | | | |
| 21 | Closing unsaved | | | | | | | | |

### Notes
