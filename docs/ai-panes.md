# AI side panes

binvim runs terminal AI tools in a pane on the right of the editor: Claude Code, Codex, opencode,
openclaw, hermes and binai. Each one is a real terminal running the tool you already have
installed, in tabs, beside the code it is working on.

## Opening a pane

| Tool | Command | Keys |
|------|---------|------|
| Claude Code | `:claude` | `<space>jc` |
| Codex | `:codex` | `<space>jx` |
| opencode | `:opencode` | `<space>jo` |
| openclaw | `:openclaw` | `<space>jw` |
| hermes | `:hermes` | `<space>jh` |
| binai | `:binai` | `<space>jb` |

Each one opens a new tab, even when a tab for that tool is already open, so `<space>jc` twice
gives you two Claude sessions. The tab header names the tool.

| Keys | Does |
|------|------|
| `<space>jf` | Focus the pane, showing it if it was hidden |
| `<space>jp` | Hide or show the pane. A hidden tool keeps running and its output keeps arriving |
| `<space>jq` | Close the active tab, ending its tool |

The pane takes 40% of the terminal's width, at least 30 columns, and always leaves the editor 40.
In a terminal narrower than 70 columns it is hidden. There is no setting for its width.

## Working in the pane

While the pane has focus, keys go to the tool, except these:

| Key | Does |
|-----|------|
| `Esc` | Back to Normal mode in the editor. The tool keeps running |
| `Ctrl-w` | Back to the editor and start a window command (`Ctrl-w h`, `Ctrl-w q`, …) |
| `Ctrl-h` / `Ctrl-Left` | Previous tab |
| `Ctrl-l` / `Ctrl-Right` | Next tab |
| `Shift-PageUp` / `Shift-PageDown` | Scroll back a page / forward a page |
| `Shift-Up` / `Shift-Down` | Scroll one line |

Because binvim takes `Esc`, a tool that uses `Esc` itself (Claude Code interrupts with it) needs
`Ctrl-[` instead. That works only in a terminal with the Kitty keyboard protocol; in others,
`Ctrl-[` arrives as `Esc` and leaves the pane too — see
[Known issues](known-issues.md#ctrl--leaves-the-terminal-pane-on-a-terminal-without-the-kitty-keyboard-protocol)
and [Terminal compatibility](terminals.md).

The pane keeps 10,000 lines of scrollback. Typing while scrolled back jumps to the live screen.

With the mouse: click a tab header to switch to it, drag to select text and copy it to the system
clipboard (over SSH that needs OSC 52, see [Terminal compatibility](terminals.md)), double-click
to select a word, and use the wheel to scroll. A tool that turns on mouse reporting gets clicks
and the wheel instead; drags and double-clicks stay binvim's, for selecting text.

## How the tool is started

The tool is started by name in your login shell, so your shell's startup files run first and
whatever they set up is there: nvm, asdf, direnv, Homebrew's `PATH`. A shell alias or function
with the tool's name is used, so an alias `claude` that adds flags applies in the pane too. The
tool starts in binvim's working directory, with binvim's environment and `TERM=xterm-256color`.

If the tool isn't installed, its tab shows your shell's "command not found" message. When the tool exits,
its tab stays open with its last output until you close it with `<space>jq`.

On Windows the tool is started through your shell as well; under `cmd.exe` it is looked up on
`PATH` first. See [Windows](windows.md).

## Edits the tool makes

The tool writes files on disk; binvim does not pass it anything. The active buffer is checked
against its file about once a second: if the tool changed it and the buffer has no unsaved
changes, it reloads and the status line says `reloaded <file> (changed on disk)`. A buffer with
unsaved changes is left alone, and `:w` then refuses to write over the tool's version until you
use `:w!` (or `:e!` to take the tool's version). A file in another split or buffer is checked once
you move to it.

## What it doesn't do

- binvim sends nothing to the tool: not the current file, not a selection, not diagnostics. Tell
  the tool what to look at the way you would in any terminal.
- There is no setting per tool: not its command, arguments or width. Use a shell alias for flags.
- Quitting binvim with `:q` or `:q!` ends every tool running in the pane without asking.

## Copilot

GitHub Copilot's inline completions are a separate feature, a language server rather than a
pane. Turn it on with `[copilot] enabled = true`; see [Configuration](configuration.md).

All keys are listed in [Keys](keys.md#leader-bindings).
