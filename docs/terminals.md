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

Start with the probe, from the repo root in the terminal you're recording:

```sh
sh scripts/terminal-probe.sh
```

It asks the terminal what the checks depend on, then asks you to look at one line, press eleven
keys, click once and paste once. It prints a report, saved to
`$TMPDIR/binvim-terminal-probe.txt`, with a verdict for checks 2–11, 13–16, 18 and 19. Over SSH,
copy the script to the remote machine and run it there.

A probe answer stands in for the check because what varies between terminals is the terminal's
half: which bytes a key sends, which modes it answers to, how wide it draws a cluster, whether it
slants, curls and pastes. binvim's half was checked by hand on the same inputs in tmux and Kitty.
A `?` means the terminal didn't answer the query (tmux answers no DECRQM), and that check is run
by hand below instead.

Then run checks 1, 12, 17, 20 and 21 in binvim itself, plus any the probe marked `?` or ✗. Over
SSH, check 18's `yy` as well. From the repo root, on the build you're recording (`:health` shows
its version):

```sh
cargo build --release
target/release/binvim docs/terminal-check.md
```

`docs/terminal-check.md` holds the text the checks look at: a line of wide characters, one of
emoji clusters, one mixing both, a styled line and a long line. `scripts/terminal-check.rs` carries
a syntax error for the undercurl check and needs `rust-analyzer` on `PATH`. Lines are named by
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
- **a link** — to the `docs/known-issues.md` entry when the terminal is at fault, or to the commit
  that fixed binvim when binvim was.
- **—** — not run yet. Alacritty, Windows Terminal, over-SSH and Terminal.app shipped in 0.7
  without a run and are filled in as they are checked.

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
    `docs/known-issues.md` entry, not a binvim bug.
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
    Paste them again, `:w /tmp/paste.txt`, and run `xxd /tmp/paste.txt` outside binvim.
    *Pass:* no `0d` byte. A terminal that sends the breaks as CR still lands them as lines.
15. **Synchronized output.** `:e src/render.rs`, hold `Ctrl-d` for two seconds, then `Ctrl-u`,
    then hold `j`.
    *Pass:* no torn frames (half old screen, half new) and no cursor flashing across the screen.
    `:bd`.
16. **Undercurl.** `:e scripts/terminal-check.rs` and wait for rust-analyzer.
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
    as `Esc`, so the pane loses focus instead: record the `docs/known-issues.md` link.
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
| | Version | 1.3.1 | 0.49.0 | 20240203-110809-5046fc22 | not yet run | 3.6a | not yet run | not yet run | not yet run |
| 1 | Launch and quit | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 2 | Re-encoded keys | ✓ | ✓ | ✓ | — | ✓ [2f81606](https://github.com/bgunnarsson/binvim/commit/2f81606) | — | — | — |
| 3 | Esc response | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 4 | `Ctrl-[` and Alt | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 5 | Shift / Ctrl arrows | n/a | n/a | n/a | — | ✓ | — | — | — |
| 6 | Cursor shape | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 7 | Truecolor | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 8 | Italic and bold | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 9 | Nerd Font glyphs | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 10 | Wide characters | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 11 | Emoji clusters | ✓ | ✓ | [KI](known-issues.md#wezterm-draws-an-emoji-made-wide-by-vs16-in-one-cell) | — | ✓ | — | — | — |
| 12 | Long line | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 13 | Mouse | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 14 | Bracketed paste | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 15 | Synchronized output | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 16 | Undercurl | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 17 | Resize | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 18 | OSC 52 | ✓ | ✓ | ✓ | — | ✓ | — | — | — |
| 19 | `:terminal` | ✓ | ✓ | [KI](known-issues.md#ctrl--leaves-the-terminal-pane-on-a-terminal-without-the-kitty-keyboard-protocol) | — | [KI](known-issues.md#ctrl--leaves-the-terminal-pane-on-a-terminal-without-the-kitty-keyboard-protocol) | — | — | — |
| 20 | lazygit round trip | ✓ | ✓ | ✓ | — | ✓ [a4ace97](https://github.com/bgunnarsson/binvim/commit/a4ace97) | — | — | — |
| 21 | Closing unsaved | ✓ | ✓ | ✓ | — | ✓ | — | — | — |

### Notes

**Ghostty** — Ghostty 1.3.1 on macOS 26, recorded on 2026-09-23 from the
maintainer's daily use of binvim in Ghostty, not from a run of the checklist or
the probe:

- 5: n/a on the same grounds as Kitty and WezTerm on this machine: macOS keeps
  Ctrl-Left and Ctrl-Right for switching Spaces.

**Kitty** — Kitty 0.49.0 on macOS 26, run on 2026-09-23 with
`macos_option_as_alt yes`. Checks 1–6 by hand, the rest from
`scripts/terminal-probe.sh`:

- Kitty takes the keyboard protocol: the flags read 0 before binvim's push and 1
  after, so `Ctrl-i`, `Ctrl-[`, `Ctrl-w`, `Ctrl-c`, Esc and Alt-Backspace all
  arrive as `CSI u` keys, and check 19 passes where tmux's needs the
  `docs/known-issues.md` entry.
- 5: Shift-Left and Shift-Right arrive as `CSI 1;2D` / `1;2C` and move one
  character. Ctrl-Left and Ctrl-Right never reach the terminal: macOS keeps
  them for switching Spaces unless that shortcut is turned off.
- 6, 7, 15, 16: Kitty answers the queries itself — DECRQSS read back the bar
  cursor, the `38;2` colour and `4:3`, and DECRQM 2026 is 2 (supported).
- 14: Kitty sends a paste's line breaks as LF. Checks 12, 17, 20 and 21 by hand in binvim.

**WezTerm** — WezTerm 20240203-110809-5046fc22 on macOS 26, run on
2026-09-23 with its default configuration, from `scripts/terminal-probe.sh`:

- WezTerm has the Kitty keyboard protocol but leaves it off: the flags read
  back as nothing after binvim's push, so keys arrive in the legacy encoding
  (`Ctrl-i` is Tab, `Ctrl-[` is Esc), which binvim reads correctly everywhere
  but check 19. With `enable_kitty_keyboard = true` the push reads back as `1`.
- 5: Shift-Left and Shift-Right arrive as `CSI 1;2D` / `1;2C`. Ctrl-Left and
  Ctrl-Right are macOS's Spaces shortcut.
- 6, 7: WezTerm answers no DECRQSS; the bar cursor and the smooth colour bar
  were judged by eye. DECRQM 2026 is 2 (supported).
- 11: `❤️` (heart + VS16) measured one cell where `unicode-width` gives two;
  the other clusters matched. `unicode_version = 14` makes it two cells.
- 1, 12, 17, 20, 21: by hand in binvim.

**tmux** — the maintainer runs binvim in tmux day to day on macOS and Linux; the
column itself is tmux 3.6a on macOS, run on 2026-09-23 against a detached server
(`tmux -L`, a 120×30 window, `TERM=tmux-256color`, the default config apart from
what is named below). With no client attached, what was checked is what tmux
parsed from binvim, not how an outer terminal then draws it. Keys went in with
`send-keys`, one call per key after `Esc`, and every check read the result back
from tmux:

- 1: the shell screen after `:q` (`capture-pane`) held only the prompt and
  `ok`, under both `TERM=tmux-256color` and `TERM=xterm-256color` with
  `extended-keys off`, so the `CSI > 1 u` binvim pushes on startup was taken as
  a control sequence and not printed.
- 2: failed first, since tmux sends `Ctrl-i` as `Tab`: fixed in `2f81606`, then
  `Ctrl-o` / `Ctrl-i` moved `cursor_y` from 1 to 12.
- 3: `Esc` to `NORMAL` in the mode line took 30–33 ms including the harness's own
  polling. An interactive tmux adds `escape-time` (10 ms here, 500 ms before
  tmux 3.5); a slow `Esc` in tmux is that option.
- 4: `Ctrl-[` arrives as the Esc byte and left Insert. `Alt-Backspace` deleted a
  word; `Esc` then `j` sent in quick succession left Insert and moved down.
  Sending the Kitty protocol's `Ctrl-[` (`CSI 91;5u`) by hand did nothing before
  `c15d6b5`, which now makes it Esc outside `:terminal`.
- 5, 10, 11, 12: `#{cursor_x}` after each key. The arrows moved one column
  with or without Shift or Ctrl; CJK and emoji steps were two columns, and
  `$` on the *Emoji* line landed where tmux's own widths put the last
  character (tmux measures 👍🏽, 👨‍👩‍👧, 🇮🇸 and ❤️ as two columns, like
  `unicode-width`).
- 6: `#{cursor_shape}` read block, bar, block, underline, block, and
  `default` at the shell afterwards.
- 7, 8, 16: `capture-pane -e` showed `38;2` / `48;2` colours, `3m` on `italic`
  and `1m` on `bold`, and `4:3` with `58;2;243;139;168` under the syntax error.
  An outer terminal only gets the undercurl if tmux's `terminal-features` gives
  it `usstyle`.
- 9: the status line and the file picker's rows carry the Nerd Font codepoints
  (`U+E0B0`, `U+E0B2`, `U+E609`, …). Whether they draw is the outer terminal's
  font.
- 13: SGR mouse reports written into the pane (`send-keys -H`): the click put
  the cursor on the cell, the drag yanked exactly `one two three`, two wheel
  notches scrolled `src/render.rs` by six lines. tmux forwarding an outer
  terminal's mouse wasn't exercised.
- 14: `paste-buffer -p` delivered the three lines intact and in `INSERT`; one
  `u` removed them. The first run misjudged this from the screen. tmux sends
  the breaks as CR, and binvim wrote them to disk as `\r`. Fixed in `ca45836`.
  Re-run against a release build, `xxd` of the saved file showed `0a` and no `0d`.
- 15: `pipe-pane -O` over six `Ctrl-d` and twenty `j` caught 30 frames, each
  inside `?2026h` … `?2026l`.
- 17: `resize-window` to 70×16 and 130×34 redrew with the status line on the
  last row each time.
- 18: needs `set -s set-clipboard on`; tmux's default (`external`) drops OSC
  52 from applications. With it, both the `printf` and a `yy` under
  `osc52 = true` landed in tmux's paste buffer. The `allow-passthrough` route
  sends the DCS-wrapped copy to the outer terminal, which a detached server
  can't show.
- 19: `Ctrl-[` left the pane, as `Esc` does; the red `printf` rendered.
- 20: from a buffer in the repo. Before `a4ace97`, `Ctrl-C` at lazygit's
  "not a git repository" prompt, which comes up when the buffer's directory is
  outside a repo, ended binvim too; after it, binvim came back with its dirty
  buffer. `Ctrl-[`, `Alt-Backspace`, the arrows and a click passed again after
  lazygit.
- 21: `kill-server` hung up the pane; the relaunch reported recovered changes
  with ` unsaved` on the *Styles* line, and `:e!` removed the recovery file.

**over SSH** — Ghostty on macOS into Linux boxes, recorded on 2026-09-25 from the
maintainer's use while making config changes on the remote machines, not from a
run of the checklist or the probe: editing and config reload worked without
issue. OSC 52 (check 18, the one thing that only differs over SSH) was not
tested. Not enough to fill in the matrix row below.
