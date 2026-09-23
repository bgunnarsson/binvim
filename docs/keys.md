# Keys

Leader bindings, buffer and tab navigation, and window splits. The motions, operators and text objects are under [Editing](editing.md); the `:` commands under [Ex commands](ex-commands.md).

## Leader bindings

| Keys        | Action                                |
|-------------|---------------------------------------|
| `<space><space>` | File picker                      |
| `<space>?`  | Recent files                          |
| `<space>G`  | Live grep                             |
| `<space>gg` | Open lazygit — suspends the editor, hands the terminal to lazygit, refreshes every buffer's git gutter on exit (same as `:lazygit` / `:lg`) |
| `<space>e`  | File explorer — built-in sidebar tree by default; shells out to yazi instead when `[file_explorer] yazi = true` |
| `<space>a`  | Code actions                          |
| `<space>r`  | Rename (LSP-aware) — opens a modal preview overlay (per-edit checkboxes, before/after snippet per occurrence) before anything touches disk. `j`/`k` move, `<Space>` toggles, `a`/`n` flip all on/off, `o` jumps to the edit site (cancels), `<Enter>` applies only the enabled edits, `<Esc>` cancels |
| `<space>R`  | Replace all (literal-string in buffer)|
| `<space>l`  | Run the code lens under the cursor — the run / debug row a server (or binvim's own vitest lenses) puts above a test |
| `<space>f`  | Format active buffer                  |
| `<space>i`  | Set up the toolchain for this language — opens `:install` preselected to the current buffer's language bundle (LSP + formatter + DAP), so you review and confirm with `y`. When a file's LSP or formatter is missing on open, a popup (same style as the file picker) lists what's missing — `Enter` opens the installer, `Esc` skips (disable the popup with `[install] prompt_on_open = false`) |
| `<space>/`  | Toggle line comment(s) — current line in Normal, every selected line in Visual. Per-language prefix (`//`, `#`, `--`); block-only languages (HTML / Markdown / CSS / XML / Razor) wrap with their pair |
| `<space>bd` | Delete buffer (refuses dirty)         |
| `<space>bD` | Delete buffer (force)                 |
| `<space>ba` | Delete all buffers (refuses dirty)    |
| `<space>bA` | Delete all buffers (force)            |
| `<space>bo` | Close other buffers                   |
| `<space>hp` | Preview the git hunk under the cursor in a hover popup |
| `<space>hs` | Stage the hunk (`git apply --cached`)  |
| `<space>hu` | Unstage the hunk                       |
| `<space>hr` | Discard the hunk — refuses while the buffer is dirty |
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
| `<space>tt` | Spawn a new terminal tab (`<space>tt` again to add another) |
| `<space>tp` | Toggle terminal pane visibility — PTYs stay alive in the background while hidden |
| `<space>tf` | Focus the terminal pane (drop into `Mode::Terminal` — typing flows to the shell again) |
| `<space>tq` | Close the active terminal tab (pane hides when the last one goes) |
| `<space>mm` | Task picker — discover + run a workspace task (same as `:task` / `:tasks`) |
| `<space>ml` | Re-run the most recent task (same as `:tasklast` / `:trun`) |
| `<space>ss` | Test picker (same as `:test`)         |
| `<space>sn` | Run the nearest test (same as `:testnearest`) |
| `<space>sf` | Run every test in the active file (same as `:testfile`) |
| `<space>sl` | Re-run the most recent test (same as `:testlast`) |
| `<space>sq` | Cancel the running test adapter (same as `:testcancel`) |
| `<space>sr` | Toggle the streaming results overlay (same as `:testresults`) |
| `<space>jc` / `<space>jC` | Spawn a new Claude tab in the right-side pane — uppercase variant additionally pre-types `@<active-buffer cwd-relative path>` into the input once the tool is ready. Same shift-pair pattern for `<space>jx` / `<space>jX` (Codex), `<space>jo` / `<space>jO` (opencode), `<space>jw` / `<space>jW` (openclaw) and `<space>jh` / `<space>jH` (hermes). Each invocation always opens a fresh instance; use `<space>jf` to focus an existing pane and `<space>jp` to toggle visibility (PTYs keep draining hidden). `<space>jq` closes the active side tab. |
| `<space>pi` | Package manager — manage installed packages: pick a project manifest when the workspace has more than one (`.csproj` / `package.json` / `Cargo.toml` / `go.mod` / `requirements.txt`), then an installed package, then a version to change to. The installed version is highlighted; `Tab` toggles prereleases; type to narrow the version list. |
| `<space>ps` | Package manager — search & add: pick a manifest (when there's more than one), type to search the registry, pick a package, then a version to add. |
| `<space>Al` | Android — pick a defined AVD and launch the emulator |
| `<space>Ac` | Android — create an AVD: pick a system image, `sdkmanager` fetches it |
| `<space>Ad` | Android — list the devices `adb` sees |
| `<space>Ab` | Android — attach a debug session to the app of the enclosing Gradle project, through jdtls and the java-debug plugin |

The package manager detects the ecosystem from the active buffer's workspace. Five backends are wired up:

- **.NET / NuGet** — requires the `dotnet` SDK on `PATH` (`dotnet package search` needs SDK 8.0.4xx+; the full per-version list is reliable on the .NET 10 SDK).
- **npm** — requires `npm` on `PATH` (`npm view` / `npm search` / `npm install`, honouring a project-local `.npmrc` for private registries).
- **Cargo / crates.io** — requires `cargo` on `PATH` for search & add; the version list is fetched from the crates.io API (so it needs `curl` and network access).
- **Go modules** — requires `go` on `PATH` for the version list & add; search scrapes pkg.go.dev (so it needs `curl` and network access).
- **Python / PyPI** — no Python tooling required at all; it reads and edits `requirements.txt` directly (so `add` just rewrites the pin — it never runs `pip`, sidestepping which-virtualenv ambiguity) and pulls the version list from PyPI's JSON API. Because PyPI retired package search (its search page is now bot-walled), "search" resolves an exact package name instead. Needs `curl` and network access.

The Cargo, Go, and Python backends shell out to `curl` for the steps their toolchain can't do (crates.io has no `cargo` command for listing all versions; the Go toolchain has no search; PyPI is HTTP-only), so `curl` must be on `PATH` for those.

Hold `<space>` (or `<space>A` / `<space>b` / `<space>d` / `<space>g` / `<space>h` / `<space>j` / `<space>m` / `<space>p` / `<space>s` / `<space>t`) for ~250 ms and a which-key popup lists the available next keys.

## Buffer / tab navigation

| Keys                  | Action                                       |
|-----------------------|----------------------------------------------|
| `H` / `L`             | Previous / next buffer (same as `:bp`/`:bn`) |
| `gt` / `gT`           | Same as `L` / `H` (Vim aliases)              |
| `Ctrl-^`              | Alternate buffer — the file active before this one (same as `:e#` / `:b#`); `N Ctrl-^` goes to buffer N |
| `Ctrl-O` / `Ctrl-I`   | Jumplist back / forward — persists across sessions per-buffer |
| Click a tab           | Switch to it                                 |
| Middle-click a tab    | Close it (refuses dirty, same as `:bd`)      |
| Click `×` on a tab    | Close it (refuses dirty)                     |
| Click `‹` / `›`       | Switch to the first hidden tab on that side, which scrolls the slice |

## Window splits

| Keys              | Action                                                       |
|-------------------|--------------------------------------------------------------|
| `<C-w> v`         | Split vertically + open the file picker for the new pane     |
| `<C-w> s`         | Split horizontally + open the file picker for the new pane   |
| `<C-w> V`         | Split vertically with the same buffer (Vim's `:vsplit`)      |
| `<C-w> S`         | Split horizontally with the same buffer (Vim's `:split`)     |
| `<C-w> h/j/k/l`   | Focus the neighbouring window on the left/down/up/right      |
| `<C-w> q` / `c`   | Close the active window (refuses if it's the last one)       |
| `<C-w> o`         | Close every window except the active one                     |
| `<C-w> =`         | Reset every split ratio back to 50/50                        |
| `<C-w> [N] >` / `<` | Widen / narrow the active window by N columns (default 1)  |
| `<C-w> [N] +` / `-` | Grow / shrink the active window's height by N rows (default 1) |
| `<C-w> T`         | Promote the focused pane's buffer to its own tab (non-destructive — the split stays) |

By default `<C-w>v` / `<C-w>s` create a split *and* open the file
picker so the new pane lands on a different file straight away —
typical case is "show me file A on the left and file B on the right."
The uppercase `<C-w>V` / `<C-w>S` keep Vim's classic behaviour of
opening the *same* buffer in both panes (useful for viewing two parts
of one long file with independent cursors). `:e other.txt` swaps the
focused pane's buffer without disturbing other panes. Moving focus
into a pane that points at a different buffer swaps the live buffer
state under you, so each window keeps its own cursor, viewport,
syntax highlighting, fold state, git stripe, blame, and markdown
concealed render.

Splits are scoped to the tab they were created in. `H` / `L` / `:b N`
cycle between *tabs*; each tab carries its own layout, so splitting
in one tab doesn't bleed into the others. A file picked into a split
via `<C-w>v` lives in that tab's layout but stays out of the tabline
until you promote it — `<C-w>T` from its focused pane adds it as a
tab (the split stays intact), or `:b <name>` from anywhere does the
same as a side effect of jumping to it.
