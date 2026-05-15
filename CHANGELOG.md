# Changelog

All notable changes to binvim are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Window splits — `<C-w>v` / `<C-w>s` / `<C-w>h/j/k/l` / `<C-w>q` /
  `<C-w>o` / `<C-w>=`.** Vertical and horizontal splits with
  independent cursors and viewports per pane. Focus moves
  geometrically — `<C-w>l` picks the right-side neighbour with the
  largest vertical overlap, matching Vim's spatial intuition. Closing
  the active window collapses its space into the sibling that absorbed
  it; `<C-w>o` collapses everything down to just the active pane.
  Splits currently share the same buffer across all panes — per-window
  buffer switching is the next step. The split tree lives in
  `src/layout.rs` (binary tree of `WindowId`s, leaves carry no inline
  state); per-pane view state lives in `src/window.rs` and is stashed
  in `App.windows` for inactive panes, swapped onto `App.window` when
  focus moves.
- **`<leader>ba` / `<leader>bA` — close all buffers.** New buffer-prefix
  bindings that drop every open buffer in one keystroke. Lowercase
  refuses if any buffer is dirty (mirrors `<leader>bd`); uppercase forces
  through unsaved changes. Lands on the start page with a single
  `[No Name]` slot, identical terminal state to closing the last buffer
  via `:bd`. Which-key popup and README updated to match.

## [0.1.7] - 2026-05-15

### Added
- **Markdown rendering polish.** Five additions on top of the
  existing concealed-render mode:
  - **Strikethrough.** GFM `~~text~~` hides the markers and renders
    the inner span with the terminal's strikethrough attribute in
    Overlay0 (so it reads as "deprecated / done"). Same flanking
    rules as bold — opener can't precede whitespace, closer can't
    follow whitespace.
  - **Code-fence chrome.** ` ```rust ` (or ` ~~~ `) opener now
    hides the fence chars and styles the language tag in
    bold-Peach. Opener + body + closer rows all paint with a
    Mantle background extended to the right edge so the block
    reads as a unified dark slab; the closer row hides its
    backticks and shows as a solid bottom-of-block bar. EOL
    `¬` whitespace markers are suppressed inside the slab so
    the dark background isn't broken by chrome glyphs.
  - **Setext headings.** `Title\n=====` becomes a bold-Lavender
    H1; `Title\n-----` becomes bold-Lavender H2. The underline
    row collapses. CommonMark's prose-only constraint is honoured
    (so `# ATX heading\n---` stays as ATX heading + horizontal
    rule, not a setext promotion).
  - **Horizontal rules.** Standalone `---` / `***` / `___` (3+
    of the same char) renders as a continuous `─` line in
    Overlay0 across the buffer width.
  - **Frontmatter.** YAML (`---`…`---`) and TOML (`+++`…`+++`)
    blocks at the top of file render in muted Overlay0 italic so
    they read as metadata chrome, not content.
  - **Inline HTML.** A handful of common tags fold into native
    styling: `<strong>` / `<b>` → bold, `<em>` / `<i>` → italic,
    `<u>` → underline, `<code>` → inline-code Green; `<br>` /
    `<br/>` / `<br />` collapse entirely; `<!--…-->` comments
    collapse on the line they appear on.
  - **Hidden rows truly collapse.** Previously rows marked
    `Hidden` (setext underlines, `<details>`/`</details>` chrome,
    standalone HTML comments) painted as blank rows — they took
    a vertical row of viewport space. They now skip the render
    walk entirely so subsequent rows shift up. A `<details>`
    block that used to span ~4 chrome rows of breathing room
    now collapses tight against the surrounding prose. Cursor
    placement, mouse-click → source-line mapping, and `j`/`k`
    motion all walk past the same hidden runs so navigation
    stays consistent. Tradeoff (accepted): the gutter's
    line-number now reflects the source line painted at that
    visible row, so `:N` jumps still land on the right source
    line but the visible row of N may shift compared to the
    raw-source layout.
  - **`<details>` disclosure blocks.** Standalone `<details>` /
    `</details>` rows hide as chrome; `<summary>X</summary>`
    becomes a bold-Peach `▼ X` disclosure title (always-expanded
    in a TUI). Inline tags inside the summary are stripped, so
    `<summary><strong>Setup the frontend</strong></summary>`
    reads cleanly as `▼ Setup the frontend`. Body content
    between the open/close tags renders as ordinary markdown.
  - **Six distinct heading colours.** Each ATX level now lands on
    its own Catppuccin accent so an outline reads as a six-tone
    hierarchy at a glance: H1 Red, H2 Peach, H3 Yellow, H4 Green,
    H5 Sky, H6 Mauve. Previously H1+H2 collapsed into Lavender
    and H4-H6 all rendered Sky.
  - **Tables.** GFM tables (header + `|---|---|` separator + body
    rows) render as box-drawn `│ … │ … │` with `├─┼─┤` separator;
    cells column-pad to the widest entry per column. Header bold
    + Lavender, separator dim Overlay0, body normal text.
    Alignment markers (`:---`, `---:`, `:---:`) are accepted but
    everything renders left-aligned in v1. Cursor on a table row
    keeps source-byte semantics so editing via `i` / Visual still
    works on the raw `| … |` pipes.
- **Markdown "concealed render" mode.** `.md` buffers in Normal mode
  paint with structural markers folded into prettier glyphs:
  headings drop their `#`s and render bold (Lavender for H1/H2,
  Sapphire for H3, Sky for H4+); `**bold**` and `*italic*` / `_x_`
  hide their markers and style the inner span; `` `inline code` ``
  hides the backticks and tints the body Catppuccin Green;
  `[text](url)` collapses to underlined-Blue `text` (the URL and
  brackets vanish); `- ` / `* ` / `+ ` bullets become `•` (Peach);
  `> ` blockquotes become `▎ ` with the body in muted Overlay0
  italic. The buffer text is never mutated — entering Insert or
  Visual mode flips back to raw markdown source instantly so you
  edit what's actually on disk. Cursor placement and mouse clicks
  walk the same per-line transforms the renderer uses, so navigating
  hidden ranges and clicking on rendered glyphs both land on the
  expected source position. Fenced code blocks (` ``` ` / `~~~`)
  are tracked across lines so inline transforms are suppressed
  inside them — `_API_` inside `ANTHROPIC_API_KEY=…` no longer
  renders as italic-`API`. Italic / bold flanking rules also
  follow CommonMark: underscore can't open or close emphasis if
  it sits between two word chars (so `f_o_o` and `FOO__BAR__BAZ`
  stay literal); a `*` or `_` opener can't precede whitespace,
  a closer can't follow whitespace.
- **Scroll the `:health` dashboard.** `j`/`k` (or arrow keys) scroll
  one row, `Ctrl-D`/`Ctrl-U` half a page, `Ctrl-F`/`Ctrl-B` and
  PageDown/PageUp a full page, `g`/`G` (or Home/End) jump to top /
  bottom, mouse wheel also scrolls. Footer now hints what's clipped:
  "more above" / "more below" / "↑ k ↓ j to scroll" when both
  directions have content offscreen. Useful on short terminals where
  the LSP / BUFFERS / TAILWIND sections used to fall off the bottom.
- **`:health` dashboard: five new sections after TAILWIND.**
  FORMATTER (on-save tool for the active buffer + binary resolution,
  including project-local `node_modules` resolution and the
  ruff→black / nixfmt→alejandra fallback chains), EDITORCONFIG
  (effective indent / tab width / trim-trailing-ws / final-newline
  + the chain of `.editorconfig` files that produced them, or
  "defaults — no .editorconfig found" if none), TREE-SITTER (detected
  language + highlight-cache state and byte-count, "no parser
  configured" when the extension isn't wired up), SESSION (whether
  the launch restored a session, the cwd-keyed session file path,
  recent-files count), TERMINAL ($TERM, $COLORTERM, truecolor
  detection, $TERM_PROGRAM, current size). Useful for diagnosing
  "why didn't save reformat" / "where is this indent coming from" /
  "are my colours actually 24-bit" without grepping the source.

## [0.1.6] - 2026-05-14

### Added
- **Replace selected text across the current buffer.** Visual-select
  the literal text, `<leader>R`, type the replacement in the prompt,
  Enter. The selection text pre-fills the cmdline so you can edit
  instead of retyping; the prompt's top border carries a "Replace in
  buffer" title. Multi-line selections are rejected with a status
  hint (the underlying substitute is a single-line literal match);
  empty selections show "replace: empty selection". Leader chord
  also now activates in Visual mode generally, not just Normal —
  other actions (file picker, format, rename) work from Visual too.

### Changed
- **`/` search is case-insensitive.** `/foo` matches `Foo` / `FOO` /
  `fOo` / etc. Applies to `n`, `N`, `*`, `#` and the search-result
  highlight painter as well; positions stay correct on Unicode text
  because the lowercase fold is ASCII-only (only A-Z get touched).

## [0.1.5] - 2026-05-14

### Added
- **`:health` is now a full-screen TUI dashboard.** Replaces the
  previous scratch-buffer with an ASCII-banner header and bordered
  PROCESS / RESOURCES / ENVIRONMENT / ACTIVE BUFFER / LSP SERVERS /
  GIT / BUFFERS / TAILWIND sections. New data the old plain-text
  report didn't have: detected language / line count / indent style
  / cursor in ACTIVE BUFFER, diagnostics chips coloured per
  severity, GIT panel with ahead / behind / modified / untracked
  counts. Auto-refreshes every second so resource numbers actually
  move. Esc / `q` / `:q` dismiss.
- **Smart Enter splits HTML / JSX / TSX tag pairs.** `<div>|</div>`
  + Enter expands to three lines with the body indented, matching
  the existing `{|}` / `[|]` / `(|)` behaviour.
- **Tier 1 language coverage — Python, C / C++, Bash, YAML, Lua.** Each
  gets the full stack: LSP (`pyright`-langserver with `basedpyright`
  fallback, `clangd` with `language_id` flipping on extension,
  `bash-language-server`, `yaml-language-server`, `lua-language-server`),
  tree-sitter highlighting (Python, C, C++, Lua — Bash and YAML
  grammars were already wired), and a format-on-save formatter
  (`ruff format` with `black` fallback, `clang-format`, `shfmt`,
  `stylua`). YAML LSP + highlight only at this point; formatter
  landed in the prettier-coverage commit below.
- **Tier 2 language coverage — Vue, Svelte, Markdown, TOML, Ruby, PHP,
  Java.** LSPs (`vue-language-server`, `svelteserver`, `marksman`,
  `taplo lsp stdio`, `ruby-lsp`, `intelephense`, `jdtls` with a
  per-project workspace data dir hashed under
  `~/.cache/binvim/jdtls/<hash>`). Tree-sitter via `tree-sitter-java`,
  `tree-sitter-ruby`, `tree-sitter-php` (`LANGUAGE_PHP` —
  `<?php … ?>`-inside-HTML shape), `tree-sitter-toml-ng` (the `-ng`
  fork — upstream `tree-sitter-toml` is archived),
  `tree-sitter-svelte-ng` (same situation). Vue grammar skipped —
  `tree-sitter-vue` 0.0.3 is alpha. Formatters via Prettier (Markdown
  / Vue / Svelte), `taplo format`, `rufo`, `php-cs-fixer` (temp-file
  dance — no stdin mode), `google-java-format`.
- **Tier 3 language coverage — Zig, Nix, Elixir, Kotlin, Dockerfile /
  Containerfile, SQL.** LSPs (`zls`, `nil` with `nixd` fallback,
  `elixir-ls` with `language_server.sh` fallback,
  `kotlin-language-server`, `docker-langserver`, `sqls`). Dockerfile
  is filename-based: `Dockerfile`, `Containerfile`,
  `Dockerfile.<suffix>`, `Containerfile.<suffix>`, or `*.dockerfile`.
  Tree-sitter via `tree-sitter-zig`, `tree-sitter-nix`,
  `tree-sitter-elixir`, `tree-sitter-containerfile` (the maintained
  successor to `tree-sitter-dockerfile`, which is stuck on
  tree-sitter 0.20), `tree-sitter-sequel` (successor to
  `tree-sitter-sql`); Kotlin's parser via `tree-sitter-kotlin-ng` but
  the crate ships no highlights query so `.kt` files render as plain
  text (LSP carries semantic info). Formatters via `zig fmt`,
  `nixfmt` with `alejandra` fallback, `mix format`, `ktfmt`
  (temp-file dance), `sql-formatter`.
- **Emmet auxiliary LSP.** `emmet-ls` layers alongside the primary
  server for HTML / CSS / JSX / TSX / Vue / Svelte / Astro / Razor
  buffers. Typing `ul>li*3>a[href]` and accepting the completion
  expands to the nested markup; mirrors the auxiliary shape used for
  Tailwind. Razor reports as `html` since emmet-ls has no Razor
  mode — the markup half of a Razor file is HTML.
- **Snippet continuation-line indent.** Multi-line LSP snippet bodies
  (emmet expansions, function templates, …) arrive with continuation
  lines starting at column 0 — the server has no view of where the
  buffer sits. Prepend the current line's leading whitespace to every
  line after the first, mirroring VS Code / Neovim. Tab stops shift
  forward for each indent inserted before them, so cycling lands at
  the right column.
- **`Ctrl-J` / `Ctrl-K` move lines down / up.** Single line in Normal
  mode, the full selected line range in Visual mode (any kind).
  Cursor and visual anchor follow the moving block so the selection
  stays attached to the same content across the shift. Count prefix
  works: `3<C-J>`. File-boundary clamps prevent the cursor from
  drifting past row 0 / EOF; trailing-newline correctness preserves
  whether the file ends with `\n` or not. Ropey's phantom empty
  trailing line is detected and skipped so the last real line doesn't
  produce a stray blank row when moved down at the edge.
- **Esc strips whitespace-only lines on Insert→Normal.** Vim
  convention: after Enter on an indented line you land at an
  auto-indented column; if you Esc without typing content, the
  indent is left behind. Now stripped — cursor parks at column 0
  rather than the standard col-1 step-back.
- **Notification box auto-dismisses after 10 seconds.** Status
  messages (save confirms, format results, LSP errors, hunk-op
  outcomes) used to linger until the next keypress. Keypress still
  dismisses instantly; the timeout is a fallback for the no-input
  case. Tracked via a new `status_msg_at: Option<Instant>` field;
  the event loop's poll budget shrinks to the deadline so the box
  clears on time without any input.
- **Prettier as the cross-format fallback.** Format-on-save dispatch
  routes everything biome doesn't yet cover to Prettier — Markdown,
  MDX, Vue, Svelte, HTML, CSS preprocessor variants (`.scss`,
  `.less`), YAML, GraphQL. Project-local
  `node_modules/.bin/prettier` wins over a global install, but
  global works too (no node_modules required), so a `.yaml` or
  `.md` file in any random project gets the same canonical
  formatter on `<leader>f`.

### Changed
- **`:w` status shows the basename, not the full path.** The save
  confirmation used to render the buffer's absolute path
  (`"/Users/…/binvim/CLAUDE.md" 51L written`), which wrapped the
  notification box across multiple rows on any deep tree. Now prints
  just the basename — disambiguating between two same-named files
  isn't this message's job; the tab bar already carries that signal.
- **Notifications park below the tab bar.** `draw_notification`
  hard-coded `top = 0`, which sat the box on top of the active tab's
  label. Now uses `app.buffer_top()` (which is 1 when tabs are
  showing, 0 otherwise) so the notification anchors to the first row
  of buffer content instead of overlapping tab labels.

### Fixed
- **Auxiliary LSPs (emmet, Tailwind, …) never received `didOpen` when
  the primary server's binary wasn't installed.** `LspManager::
  ensure_for_path` returned the primary client specifically. When the
  primary failed to spawn (vscode-html-language-server missing on a
  `.html` file is the canonical case), the function returned `None`
  and the two callers (`lsp_attach_active` / `lsp_sync_active`)
  early-returned on `None` — `didOpen` never went out, and emmet-ls
  / Tailwind appeared as "running" in `:health` but produced empty
  responses to every textDocument/* request. Specific user-facing
  symptom: typing `!` in a blank `.html` file should pop the HTML5
  boilerplate via emmet-ls, but nothing appeared. Fixed by changing
  `ensure_for_path` to return `bool` indicating "at least one client
  is running" rather than the primary client specifically.
- **CSS custom properties rendered identical to regular properties.**
  The CSS query's `((property_name) @variable (#match? @variable
  "^--"))` override was meant to flip `--color-primary` away from the
  Lavender `@property` tone toward the default-text `@variable`. But
  `@variable` resolves to `None` colour in `config.rs`, and the
  highlight priority loop only updated `byte_priority` inside the
  `if let Some(color) = …` branch — a more-specific later match
  meaning "this should be uncoloured" silently failed to clear the
  earlier general match's colour. Resolve the colour outside the
  `Option` branch; `None` captures now also bump priority, so the
  override visibly takes effect. Safe because the queries we use
  follow the standard "general first, specific later" convention
  (the one outlier — the JSON bundled query — already got rewritten
  inline when this priority system was introduced).
- **Git gutter: stripe landed on the wrong block when adding a duplicate
  alongside an existing one.** Adding a structurally-similar block (e.g.
  a second `defineField({...})` next to an existing one) made the green
  Added stripe paint on the *original* block instead of the new one.
  This was Myers diff being clever — two equally-valid edit scripts
  produce the same final file, and Myers tie-breaks toward the earlier
  position. Switched all three `git diff` invocations to
  `--diff-algorithm=histogram`, which anchors hunks on landmark lines
  and lines up with what humans expect for additions like this.

### Added (earlier in cycle)
- **Git gutter (stage 1 of git integration).** A coloured stripe at the
  leftmost gutter column shows working-tree changes against the index:
  Green `▎` for added lines, Yellow `▎` for modified lines, Red `▁` for
  deletions (painted on the line that now sits at the deletion point).
  Hunks are refreshed on save, on buffer switch, and on initial open.
  Implementation shells out to `git diff --no-color --unified=0` and
  parses the hunk headers; no libgit2 dependency. Gutter widens by one
  column to make room (`digits + 3`).
- **Inline git blame (stage 4 — git integration complete).** `:Gblame`
  toggles per-line virtual text at the end of every row: author •
  relative age (`3d`, `2w`, `4mo`) • short SHA, in muted italic so the
  text reads as metadata not content. Sourced from `git blame
  --porcelain`, parsed locally. Each buffer toggles independently, so
  blame can be on for one file and off for another. Re-fetched on
  reload (e.g. after `<leader>hr`) since line numbers shift; cleared
  when the toggle is off so memory doesn't grow. Suppressed on rows
  that already show an inline diagnostic to avoid stacking overlays.
- **Stage / unstage / reset hunk (stage 3 of git integration).**
  `<leader>hs` stages the hunk under the cursor by building a one-file
  unified diff and piping it through `git apply --cached --unidiff-zero`.
  `<leader>hu` unstages a staged hunk via the reverse patch. `<leader>hr`
  discards the working-tree change — applies the reversed patch to the
  working tree (not the index) and reloads the buffer from disk. Reset
  refuses to run while the buffer is dirty, so unsaved edits can't be
  silently overwritten. Each action refreshes the gutter and reports
  success / failure in the status line. Reuses the existing reload-from-
  disk path (now exposed as `force_reload_from_disk` for cases that
  bypass the dirty guard).
- **Hunk navigation + preview (stage 2 of git integration).** `]h` /
  `[h` jump to the next / previous git hunk in the active buffer (bonks
  at the end, no wrap-around — matches `]q` / `[q`). `<leader>hp`
  previews the hunk under the cursor in a hover popup, rendered from
  `git diff -U3` so three lines of surrounding context come along.
  Status line gains a `+A ~M -D` counter next to the branch name when
  the working tree has any added / modified / deleted hunks. New
  `<leader>h` which-key entry advertises the sub-menu (`s` / `u` / `r`
  reserved for stage 3).
- **Word-aware drag after double-click.** Double-click a word to select
  it, then drag forward or backward to extend the visual selection
  word-by-word. The anchor pins to the side of the original word
  opposite the drag direction; the cursor snaps to the word boundary
  under the mouse. Dragging through whitespace holds the selection at
  the previous word boundary so it only grows when a new word is
  crossed. Falls back to the existing char-by-char drag if the drag
  didn't start from a double-click.

## [0.1.4] - 2026-05-11

### Added
- **Alt / Ctrl + Backspace deletes a word; Cmd / Super + Backspace
  deletes to start of line.** Insert-mode Backspace now reads
  `key.modifiers` and routes to one of three paths: SUPER/META wipes
  from `line_start` to the cursor; ALT/CONTROL peels trailing
  whitespace then one homogeneous run (word chars or punctuation),
  matching macOS Option-Delete; otherwise the existing single-char
  delete with auto-pair / line-join logic. None of the three span
  lines, so an accidental Cmd-Backspace can't eat into the row above.

### Fixed
- **Cursor placement skewed by inlay hints.** `place_cursor` walked
  only buffer chars + tab widths, so when an inlay hint took N visual
  cells before the cursor's buffer column, the terminal cursor landed
  N cells short of where the buffer actually sat. Backspace then
  removed a char that visually looked like it was "ahead" of the
  cursor. Mouse-click mapping had the inverse error: clicks inside a
  hint label scattered into the next visible token. A shared helper
  `inlay_hint_widths_for_line` now feeds both: cursor placement adds
  hint widths for cols *strictly before* `cursor.col` (the at-cursor
  hint renders on the cursor's far side so the cursor sits on the
  near edge of the hint), and click mapping snaps clicks-inside-a-hint
  to the buffer col at the hint's anchor.
- **C# / Razor file-type icon was tofu on Nerd Fonts v3.** The
  previous codepoint `\u{f81a}` was a v2-only glyph. Switched to
  `\u{e648}` (`nf-seti-c_sharp`), stable across v2 and v3. Affects
  both the picker icon and the status-line "razor" tag.

## [0.1.3] - 2026-05-11

### Added
- **YAML / XML syntax highlighting.** New `Lang::Yaml` and `Lang::Xml`
  variants wired to `tree-sitter-yaml` and `tree-sitter-xml`. XML
  detection covers the MSBuild + .NET family — `.xml`, `.csproj`,
  `.fsproj`, `.vbproj`, `.props`, `.targets`, `.config`, `.manifest`,
  `.nuspec`, `.resx`, `.xaml`, `.xhtml`, `.xsd`, `.xsl`, `.xslt`,
  `.plist`.
- **`.editorconfig` highlighting.** Byte-level scanner: `#`/`;`
  comments in Overlay1, `[*.cs]`-style section headers in Pink,
  `key = value` pairs with the key in Lavender, the `=` in Sky, and
  the value in Green. TOML grammar can't parse the bracket-pattern
  section headers so it's a custom pass.
- **`.gitignore` family highlighting.** Covers `.gitignore`,
  `.gitattributes`, `.dockerignore`, `.npmignore`. `#` comments,
  `!`-negation prefix in Mauve, pattern lines in Lavender.

### Changed
- **CSS highlight query replaces the bundled tree-sitter-css one.**
  Upstream paints `class_name`, `id_name`, `namespace_name`,
  `property_name`, and `feature_name` all as `@property` — so in
  `.foo { color: red; }` the selector `.foo` and the property
  `color` rendered in the same Lavender. The replacement query
  splits them: `.class-name` is `@constructor` (Yellow),
  `#id-name` is `@label` (Sapphire), `property:` stays `@property`
  (Lavender), `--custom-prop` is `@variable`, at-rules
  (`@media`/`@keyframes`/…) are `@keyword` (Mauve).

### Added (continued)
- **DAP multi-project workspace support.** `<leader>ds` works from
  any file now — not just `.cs`. When the workspace has more than
  one `.csproj` (or `.fsproj` / `.vbproj`), a picker opens listing
  every project relative to the cwd. Walking up looks for `.sln` or
  `.git` first to find the workspace root, then enumerates projects
  beneath it (skipping `bin/`, `obj/`, `node_modules/`, etc., with
  a 6-deep bound). The buffer's path doesn't need to be inside the
  picked project — opens fine from a README at the repo root.
- **DAP reads `Properties/launchSettings.json`.** Every profile
  with `commandName == "Project"` (Kestrel hosting via `dotnet run`)
  becomes a launchable option:
  - 0 profiles → start with framework defaults (no overrides).
  - 1 profile → use it directly.
  - >1 profiles → open the launch-profile picker, one row per
    profile, displaying the profile name + its applicationUrl
    (e.g. `Umbraco.Web.UI  (https://localhost:44317, http://…)`).
  The chosen profile's `applicationUrl` becomes `ASPNETCORE_URLS`
  on the launched process so the app binds to the user's configured
  port instead of the framework default `http://localhost:5000` —
  fixes the "Failed to bind to 127.0.0.1:5000: address already in
  use" case when another local service squats on :5000. The
  profile's `environmentVariables` pass through as `env` on the
  launch payload (e.g. `ASPNETCORE_ENVIRONMENT=Local`).
- **Picker file-type icons.** Files / Recents / Buffers / Grep /
  References rows get a Nerd Font icon per row from `Lang::detect`
  on the basename. Symbol / CodeAction pickers stay unchanged (rows
  aren't files). Grep / references rows now correctly split off the
  trailing `:LN:COL:…` suffix instead of treating it as part of the
  filename.
- **Picker fuzzy-match char highlighting.** `fuzzy_match` returns
  match positions alongside the score; matched chars render in
  Catppuccin Yellow + Bold so it's obvious which query letters
  produced the row's rank.
- **Inlay-hint kind styling.** Parameter hints (LSP `kind == 2`)
  render in a warmer Overlay2 tone; type hints (`kind == 1` or
  unknown) keep the muted Overlay1 they had — both categories scan
  apart on a mixed line.
- **JSX overlay: `{expr}` braces tagged as JSX-template syntax.**
  JSX expression containers get `@operator` on their braces so they
  read as JSX-template syntax instead of being mistaken for object
  literals. (Originally also covered `<>…</>` fragments via a
  `jsx_fragment` capture, but that node type isn't in
  tree-sitter-typescript / tree-sitter-javascript 0.23 — adding it
  made `Query::new` fail and wiped the entire highlight cache for
  every .tsx / .jsx file. Reverted, regression-tested.)
- **`<leader>do` / `<leader>dS` for Doc / Workspace symbols.** Moved
  from top-level `<leader>o` / `<leader>S` so all "navigate around
  code while debugging" pickers cluster in the debug sub-menu.
  Existing debug bindings shift: `dq` is Stop session, `dO` is Step
  out (was `dS` / `do`).
- **`gt` / `gT` Vim aliases for next / previous buffer.** Same path
  as `H` / `L`.
- **Middle-click closes a tab.** Subject to the same dirty-buffer
  guard as `:bd`. Faster than aiming for the `×`.
- **Clickable tab-bar overflow chevrons.** `‹` / `›` shift the
  visible slice by one tab when clicked — sets the active buffer to
  one step before the first visible tab (`‹`) or one step after the
  last visible tab (`›`).
- **Multi-cursor Enter mirroring.** Pressing Enter with additional
  cursors active inserts a literal `\n` at every cursor. Smart
  indent at the secondaries is non-trivial (neighbouring context can
  disagree across positions) so v1 keeps them in basic sync.
- **Persistent jumplist.** Each `SessionBuffer` now serialises its
  per-buffer `jumplist` + `jump_idx`; on launch the values restore
  alongside cursor/viewport. Entries are clamped against the
  current buffer's bounds so a file shortened since the last session
  doesn't carry an out-of-range jump.
- **`:s` / `:S` regex flag.** Add `r` to the flag tail (e.g.
  `:s/foo.*bar/x/gr` or `:%S/^let /var /r`) to interpret the pattern
  as a regex. Replacement honours `$1`/`$2`/… capture references. No
  flag → fixed-string substitution, same as before. Project-wide
  `:S` also drops `--fixed-strings` from the ripgrep candidate
  search when `r` is set.
- **Proper visual-block surround.** `Vb`-selected rectangle now
  wraps each row's column slice independently — `(c1, c2+1)` on
  every row in the rectangle — instead of falling back to a coarse
  anchor-to-cursor span across the whole region.
- **Razor / `.cshtml` syntax highlighting.** New `Lang::Razor` variant
  routed via `tree-sitter-razor`, paired with the bundled C# highlights
  query plus a Razor overlay that tags every `@`-marker directive
  (`@inject`, `@using`, `@{`, `@if`, `@(expr)`, `@Identifier`, `@*…*@`,
  …) as `@keyword.directive` and HTML element delimiters as
  `@punctuation.bracket`. Tag names and attribute names — anonymous
  lexer rules in the grammar — get coloured by a byte-level overlay so
  they still light up when the grammar's parser produces ERROR nodes
  (typical for Tailwind `class="…[16px]…"` attribute values, BOM
  headers, etc.). C# keywords inside broken regions get a regex-based
  fallback paint so an `else { if (x) {…} }` block keeps its colour
  even when the tree-sitter pass can't reach the tokens.
- **`<leader>f` formats the active buffer.** Same path as `:fmt` /
  `:format`. Picks the right tool per extension:
- **csharpier integration for `.cs` (and `.cshtml` / `.razor` once a
  future csharpier supports them).** Resolved from `$PATH` or
  `~/.dotnet/tools/csharpier`. The buffer goes through a sibling temp
  file so the project's `.csharpierrc` / `.editorconfig` resolution
  still works. csharpier 1.x prints `Is an unsupported file type` and
  exits 0 for Razor; the dispatch detects that signal and falls back.
- **`.editorconfig`-driven indent normaliser for `.cshtml` / `.razor`.**
  Reflows leading whitespace per `indent_style` and `indent_size` —
  tabs → spaces, spaces → tabs, with intermediate columns preserved.
  Inner whitespace (column-aligned attributes, mid-line tabs) is left
  alone. Wired into the csharpier fallback so saving a Razor file
  applies the indent settings even though csharpier itself doesn't
  format it.
- **gofmt / goimports for `.go`.** Prefers `goimports` when on `$PATH`
  (formats + organises imports), falls back to plain `gofmt`. Both read
  stdin and write to stdout so no temp file is needed.
- **Visual-mode `p` / `P` replaces the selection.** Previously only
  Normal-mode parsed the put keys. Now `viw` → `p`, `V<motion>` →
  `p`, and block-select → `p` all swap the visual span with the
  register's contents. Block paste deletes the rectangle and inserts
  at the corner; charwise + linewise behave like Vim's `cpoptions-=>`
  with a linewise-over-charwise trim.
- **Paste from the OS clipboard.** `p` / `P` against the unnamed /
  `+` / `*` registers consult the clipboard first — anything you
  copied in another app wins over the in-memory yank. Falls back to
  the in-memory register when the clipboard is empty, locked, or
  non-text. Linewise heuristic: trailing newline *and* interior
  newline → linewise paste; single line with trailing newline stays
  charwise.
- **.NET Core debugger via DAP.** New `src/dap/` module spawns Samsung's
  netcoredbg (looked up on `$PATH`), drives the full Debug Adapter
  Protocol handshake (initialize → launch → setBreakpoints →
  configurationDone), and surfaces stop / step / output events in a new
  bottom debug pane. `:debug` (or `<leader>ds`) auto-builds the project
  via `dotnet build` and auto-resolves the dll under `bin/Debug/net*/`,
  preferring `<project>.dll` from the most recently built target. The
  adapter registry in `src/dap/specs.rs` is adapter-agnostic — additional
  adapters (delve, debugpy, lldb-dap) plug in with one struct entry.
  Tested against .NET 10 on Apple Silicon.
- **Bottom debug pane.** `:dappane` / `<leader>dp` toggles a split below
  the editor (painted in the tab bar's Mantle shade so it reads as
  chrome rather than a buffer split). The pane silently no-ops on
  terminals too short to keep the editor usable. Header shows
  `DEBUG │ <adapter> · <status>`; body splits into call stack +
  locals on the left, debug-console tail on the right.
- **`Mode::DebugPane` for in-pane navigation.** `<leader>df` focuses
  the pane; `j`/`k`/`g`/`G` move the locals selection (with auto-follow
  scroll), `Enter` / `Tab` / `<space>` expand a structured value,
  `Ctrl-Y` / `Ctrl-E` free-scroll the left column without losing the
  selection, `J` / `K` scroll the console column, `c`/`n`/`i`/`O` step
  without leaving the pane, `:` enters the command line, `Esc` returns
  to Normal.
- **Variable expansion in the locals tree.** Structured locals render
  with ▶ / ▼ markers; expansion lazily fetches `children` per
  `variables_reference` and caches them. Children fetched concurrently
  are routed back to the right parent via a `request_seq → vref` map.
  All maps clear on `stopped` / `continued` — DAP doesn't promise vref
  stability between stops.
- **Breakpoint and current-PC gutter markers.** User breakpoints render
  as ● in the sign column; the currently-stopped top frame renders as
  ▶ (Catppuccin Peach). Both win over LSP diagnostic severity on the
  same line.
- **Visual Studio / Rider F-key bindings.** `F5` continue (or start
  when no session), `Shift+F5` stop, `F9` toggle breakpoint, `F10`
  step over, `F11` step into, `Shift+F11` step out. Work in any
  editor mode so the muscle memory carries through Insert / DebugPane.
- **Auto-jump to stopped frame.** When a `stopped` event arrives, the
  editor opens the source of the top frame (if not already open),
  jumps the cursor onto the paused line, and the gutter ▶ marker
  lands there. Scroll positions reset so each new stop starts from a
  predictable viewport.
- **Adapter stderr surfaced in the pane.** Any line netcoredbg writes
  to stderr streams into the debug-console buffer and the pane status
  header — adapter crashes no longer look like silent hangs. An
  unexpected `Child::try_wait` exit becomes an `AdapterError` event so
  the user sees the cause instead of a frozen pane.
- **Unverified-breakpoint surfacing.** netcoredbg's `setBreakpoints`
  response includes per-breakpoint `verified` flags; unverified entries
  now stream into the console + status line so the user spots a
  missing-PDB / wrong-line / "bind by pattern" misfire instead of
  silently never hitting.

### Changed
- **`<leader>d` is the debugger sub-menu.** Doc symbols moved from
  top-level `<leader>o` to `<leader>do`; Workspace symbols from
  `<leader>S` to `<leader>dS`. The leader-pick popup lists `+Debug`
  alongside `+Buffer`. The leader-entry hints update with the new
  mapping.
- **Debug pane uses the tab-bar's Mantle shade.** Previously rendered
  in Surface0 / Surface1, which read as a second buffer split. Now
  both header and body paint as `#181825` so the pane visually sits
  in the chrome layer.

### Fixed
- **CRLF files clobbered inline diagnostics.** `Buffer::from_path` kept
  every line's trailing `\r`; the renderer stripped only `\n`, so the
  `\r` reached the terminal and reset the cursor to column 0 right
  before the Error-Lens inline diagnostic painted on top of the
  source. Files with CRLF on the line a diagnostic landed on (common
  in Vettvangur's mixed-line-ending `.cs` files) rendered as
  `● Unnecessary using directive.PublishedContent;` with the leading
  source obliterated. Now normalises `\r\n` → `\n` on load + on
  disk-watch reload, with a defensive trailing-`\r` strip in the
  renderer as belt-and-suspenders for paste / LSP-applied edits.
- **Mouse clicks on tab-indented lines landed past the click.** The
  click handler treated the post-gutter screen column as the buffer
  char column directly. A tab takes `TAB_WIDTH` visual cells but one
  buffer char, so two leading tabs (visual col 8) used to map to char
  8 — well past `<partial` on the line. The handler now replays the
  renderer's display-width walk to translate visual position back to
  char position; drag selection shares the same mapping.
- **Razor closing-tag names rendered uncoloured.** The post-pass's
  child-node skip set listed `<`, `>`, `/>`, `=`, `"` but missed
  `</`, so the scanner treated the closing-tag opener as a nested
  child and stopped before reaching `section` / `div`. Added `</`
  to the skip set so opening and closing tag names match in colour,
  plus an overlay capture so the `</` token itself paints with the
  punctuation tone like `<` / `>` / `/>`.
- **Stale frames + locals after step / breakpoint hit.** `stackTrace`,
  `scopes`, and `variables` responses mutated session state but didn't
  emit a user-facing `DapEvent`, so the main loop's `events.is_empty()`
  gate skipped the re-render and the ▶ marker + locals appeared only
  on the next keypress. `DapManager::drain` now returns
  `(events, progress)` and the main loop renders on either condition.
- **DAP polling latency.** The main loop's 100ms poll ceiling stacked
  ~500ms of perceived lag onto a single step (4-5 request/response
  round-trips, each waiting on the next poll wake). The poll budget
  now caps at 16ms while `DapManager::is_active()`; idle keeps the
  100ms cadence.
- **Deep stacks hiding locals.** With `justMyCode=false`, an ASP.NET
  breakpoint produced 10-15 framework frames that filled the left
  column and pushed locals off-screen. The renderer used to cap the
  frame list, but now every frame is in the list and the column
  scrolls — `Ctrl-Y` / `Ctrl-E` peek upward without losing the
  selection.
- **`adapterID` for netcoredbg.** The adapter keys behaviour on the
  well-known `coreclr` adapter id; passing our internal `"dotnet"` key
  produced a degraded session. The initialize request now sends
  `adapterID: "coreclr"` while the editor-side registry continues to
  use the `dotnet` key.
- **Lambdas inside minimal-API endpoints never hitting.** Default
  `justMyCode=true` caused netcoredbg to rebind breakpoints inside
  endpoint lambdas to the nearest user-code sequence point (the
  `MapGet` registration line), so they fired once during startup and
  never again on request. Switched the default to `false`; the
  `breakpoint` DAP event is now handled so adapter-side rebinds
  propagate to the gutter.
- **netcoredbg empty stackTrace immediately after stop.** Some adapters
  refuse to populate `stackTrace` until the client has called
  `threads`. The `stopped` event now fires both requests back-to-back
  so the frame list is populated by the time `stackTrace` returns.

## [0.1.2] - 2026-05-11

### Changed
- **Tab bar shows for single buffers too.** `show_tabs()` now returns
  true whenever any path-backed buffer is open (or multiple buffers
  are loaded), not only the multi-buffer case. The fresh-launch
  `[No Name]` seed still hides the bar. `open_buffer` strips the
  phantom seed buffer on the transition into the first real file —
  same shape as the session-restore cleanup — so the bar starts as a
  single tab rather than `[No Name] | foo.rs`.

### Fixed
- **`.tsx` files parsed as TypeScript by tsserver.** A single
  `typescript-language-server` client serves both `.ts` and `.tsx`
  (same `ServerSpec.key`). On `didOpen` we were reusing the client's
  stored `language_id` — whichever spec spawned it first — so a
  `.tsx` file opened after a `.ts` would get `languageId: typescript`
  and every JSX `<…>` came back as a syntax error. `LspClient::did_open`
  now takes the languageId per call; `LspManager::did_open_all` looks
  up the spec for each path and pipes the right one through.
- **Pure-TypeScript syntax highlighting regression.** The JSX overlay
  query referenced `jsx_*` node types that don't exist in
  tree-sitter-typescript's pure-TS grammar (only TSX has them), so
  `Query::new` errored out and the whole `.ts` highlight cache came
  back empty — every token rendered as plain text. The overlay is now
  only layered onto TSX and plain JS, leaving pure TS to its JS+TS
  query stack as before.
- **JSX / TSX tag highlighting.** Tree-sitter-javascript's bundled
  highlight query categorises every JSX element name as a generic
  identifier, leaving `<div>` / `<main>` / `<span>` indistinguishable
  from surrounding variables. Added an overlay query that tags
  lowercase JSX names as `@tag` (Mauve) and PascalCase names as
  `@constructor` (Yellow), with attribute names rendering as
  `@attribute`. Member-access components (`Foo.Bar`) are coloured on
  both halves. Layered onto JS / TS / TSX.
- **HTML-tag matched-pair highlight narrows to the tag name.** Cursor on
  `<main className="…">` used to highlight the whole opening run and the
  whole `</main>` run; now both halves underline just the tag name
  (`main` ↔ `main`).

## [0.1.1] - 2026-05-10

### Added
- `CONTRIBUTING.md` — repo conventions, architecture quick-reference, and a
  step-by-step PR walkthrough for the most common contribution shapes
  (new LSP, new tree-sitter grammar, motion/text-object).

### Fixed
- **Multi-selection delete cursor shift.** After `d` / `c` on a
  Ctrl-N multi-selection, secondary cursors landed at stale positions
  — they used original char indices without accounting for the
  cumulative shift left from earlier (lower-indexed) deletes. Now each
  cursor's final position subtracts the total length of every prior
  deletion, so all cursors sit exactly where the deleted ranges were.

### Changed
- **`H` / `L` cycle buffers (tabs).** Shift-h / shift-l in Normal mode
  now switch to the previous / next buffer respectively (same as
  `:bp`/`:bn` and `<leader>bp`/`<leader>bn`). They no longer fire the
  Vim viewport-top / viewport-bottom motions — use `M` (viewport
  middle) and the scroll commands for those workflows.
- **Tab bar reverted to one row.** Drops the 2-row variant. File-type
  icon column also removed; each tab is just ` label[ +]  × ` now.
- **Restored sessions land on the start page.** The saved buffers are
  loaded in the background; the welcome screen is on top until the
  user reaches for one (`H`/`L` cycle, `:bn`/`:bp`, `:b<n>`, `:e`,
  `<leader>e`, or clicking any tab). Solo-buffer sessions: `H`/`L`
  still dismiss the start page and surface the one buffer (with the
  usual "Only one buffer" hint). The tab bar IS shown above the start
  page so the user can see what was restored — no tab is highlighted
  as active until you actually pick one.

### Fixed
- **Snippet / completion accept honours `textEdit.range`.** Servers
  send an explicit replacement span (e.g. covering a trailing `.` when
  proposing `?.method`); previously we always used the client-side
  word-prefix guess, which produced `obj.?.method` instead of
  `obj?.method`. Now we honour the server's range when present and
  fall back to the prefix guess only when it's absent.
- **Session restore no longer leaves a phantom `[No Name]` buffer.**
  `App::new()` seeds `buffers[0]` with an empty stash before
  `hydrate_from_session` runs; the restore path now strips that initial
  empty slot when any session buffer is successfully opened.

### Changed
- **Tab bar polish.** Each tab now carries a 2-char language icon
  (colour-coded per file type: `rs`/`ts`/`js`/`go`/`{}`/`md`/etc.) on
  the left and a clickable `×` close button on the right. Padding
  bumped to give the label and chrome breathing room. Overflow
  chevrons render `‹` / `›` at the bar edges when tabs have scrolled
  off either side. Close-clicks honour the same dirty guard as `:bd`
  (refuse, status message); use the buffer prefix's `D` for force.
- **Tab bar replaces the buffer picker.** When 2+ buffers are open
  (outside the start page), a Catppuccin-Mantle tab row paints across
  the top of the screen — active tab in Surface1 + Lavender + bold,
  inactive in Subtext0, dirty buffers carry a Peach `+`. Left-click on
  a tab switches to it. Buffer area shifts down by one row when the
  bar is visible. The `<leader>b<space>` picker binding is gone; use
  the tab bar (click), `:bn`/`:bp`, or `<leader>bn`/`<leader>bp`.

### Added
- **Sublime-style multi-selection.** In Visual-char mode, `Ctrl-N`
  finds the next literal-text occurrence of the current selection and
  adds it as a parallel selection. Repeat to keep selecting. `d`, `c`,
  `y` then apply to every selection at once: `d` deletes all,
  `c` deletes and enters Insert mode with a mirrored cursor at every
  former selection start (typing then mirrors across all sites via
  the existing multi-cursor machinery). Wraps the buffer once; status
  message when only one occurrence exists or none are left.
- **Multi-cursor (real, mirrored).** `Ctrl-click` in Normal mode adds
  a secondary cursor at the click position; cursors render as Lavender
  blocks. Entering Insert via `i` keeps cursors anchored; via `a`
  shifts each cursor by +1 (so typing lands *after* each char). Other
  Insert-entries (`I`/`A`/`o`/`O`) collapse the multi-cursors. Inside
  Insert mode, typing and Backspace mirror at the primary cursor and
  every additional cursor (bottom-up edit order keeps indices stable).
  `Esc` exits Insert and collapses; a plain (non-Ctrl) click also
  collapses. Auto-pair is disabled while multi-cursor is active — the
  user is in mass-edit mode anyway.
- **Inlay hints.** Added `textDocument/inlayHint` request +
  response parser; hints render between buffer chars in dim italic
  (Overlay1). Refetched once per buffer-version; respects horizontal
  scroll and shares the line's width budget with the buffer chars.
  Servers that publish hints (rust-analyzer, typescript-language-server,
  gopls, …) now annotate types and parameters inline.
- **Snippet expansion.** LSP completion items marked
  `insertTextFormat == 2` get their `$N` / `${N:default}` / `$0`
  placeholders parsed and resolved. The cursor lands at `$1` (or `$0`
  if no `$1`); first-seen defaults mirror to later bare references of
  the same stop. `snippetSupport: true` advertised in the initialize
  payload so servers actually send them. Tab cycling between stops is
  a follow-up.
- **Project-wide substitute (`:S/pat/repl/[g]`).** Walks ripgrep for
  every file containing `pat` under `cwd`, opens each, applies the
  substitution buffer-wide, saves. Status reports total substitutions
  + file count + any per-file errors. Range prefix ignored.
- **Replace-all in buffer (`<leader>R`).** Literal-string counterpart
  to LSP rename: gathers the word under the cursor, opens a prompt
  pre-filled with it, on submit runs `:%s/word/new/g`. Useful when the
  symbol isn't LSP-tracked (config files, comments, prose).
- **Double-click to select word.** A second left-click at the same buffer
  position within 350ms expands to the inner word under the cursor and
  enters Visual-char mode.
- **Visual-block mode (`Ctrl-V`).** Third visual kind alongside char and
  line. Rectangular column selection across a line span; `d`/`c`/`y`
  apply column-wise per row. `>`/`<` fall back to indent/outdent over
  the line range. Selection survives drag, mode label reads `V-BLOCK`.
- **Sessions.** Open buffer set + per-buffer cursor + viewport are
  persisted to `~/.cache/binvim/sessions/<cwd-hash>.json` on clean
  shutdown and restored on launch when no file argument is passed. A
  buffer whose path no longer exists is silently dropped.
- **Better completion popup.** Each row renders a colour-coded kind chip
  (`fn`, `var`, `cls`, `if`, `fld`, `mod`, `snp`, `kw`, `K`, …) on the
  left and the server-supplied `detail` (e.g. the function signature)
  right-aligned in Overlay2 grey. Popup width caps at 80 cols; detail
  trims first when space is tight.
- **Picker scrolling.** Mouse wheel inside the picker moves the selection
  by ±3 (instead of scrolling the buffer behind it); PageUp/PageDown jump
  by a full visible page; Ctrl-U/Ctrl-D jump by half a page; Ctrl-G /
  Home and Ctrl-Shift-G / End jump to first/last result. Existing
  arrow-keys and Ctrl-J/Ctrl-K still move by one row; footer hint
  advertises `^J`/`^K` to match Vim convention. (Ctrl-N/Ctrl-P aliases
  removed — single Vim-style binding only.)

### Changed
- **Hover popup now syntax-highlights code and preserves indentation.**
  The markdown returned by an LSP is parsed into structured lines (prose,
  headings, horizontal rules, fenced code blocks); each fenced block's
  language tag drives a tree-sitter pass and the byte-colour map paints
  per-character, so signatures (e.g. `defineField<{ name: string; … }>`)
  finally render the way they would in the buffer. Code lines keep their
  leading whitespace verbatim (tabs expand to four columns); prose still
  word-wraps to the popup width but indentation carries through to wrapped
  continuations. Headings render bold + Lavender; rules render as a
  horizontal divider in the border colour.
- **Picker redesigned as a centered floating popup.** Catppuccin-Mantle
  bordered box (~80% wide, ~60% tall, clamped) replacing the full-width
  bottom panel. Title chip in the top border with a counter on the right;
  Peach `›` prompt arrow; selected row gets a Lavender `▌` accent + Surface1
  background + bold; directory part of each path renders dim while the
  basename pops bright; smart truncation prefers keeping the basename
  intact; footer hint row (`↵ open  ^N/^P navigate  esc cancel`) with a
  shorter fallback on narrow terminals.
- **Source layout: `src/app/` and `src/lsp/` are now sub-module directories.**
  The two largest files (formerly `app.rs` at ~5.6k LoC and `lsp.rs` at ~2k)
  have been split into 14 and 6 focused children respectively. Each parent
  (`src/app.rs`, `src/lsp.rs`) is now a slim entry that declares the
  children and re-exports the public API; external addresses (`crate::app::App`,
  `crate::lsp::LspManager`) are unchanged. No behaviour changes.
- `CLAUDE.md` / `CONTRIBUTING.md` / `LSP_ADOPTION.md` updated to point at
  the new file paths and document the sub-module convention.

## [0.1.0] - 2026-05-09

Initial public release. binvim is a single-binary modal TUI editor written
in Rust, drop-in usable as a daily-driver Vim alternative for codebases
covered by its bundled language stack.

### Added

#### Modal editing
- Normal / Insert / Visual (char + line) / Command / Search / Picker /
  Prompt modes.
- Motions: word / WORD / end-of-word, line start/end, first/last non-blank,
  buffer first/last, `gg` / `G`, `H` / `M` / `L`, `f` / `F` / `t` / `T` and
  `;` / `,`, `n` / `N`, marks (`m{a-z}`, `'` / `` ` ``).
- Operators: `d`, `c`, `y` over motions, text objects, and linewise.
- Text objects: word, big-word, paragraph, quotes (`'"\``), brackets
  (`()[]{}<>`) — both `i` (inner) and `a` (around) variants.
- Indent / outdent (`>`, `<`, `>>`, `<<`) including visual-mode versions
  that keep the selection alive.
- Surround: `ds{c}`, `cs{old}{new}`, visual `S{c}` (Vim-surround style).
- Counts and registers — named, unnamed, and the `+`/`*` clipboard
  registers mirroring through to the OS clipboard via `arboard`.
- Macros (`q{a-z}` record, `@{a-z}` replay, `@@` replay last).
- `.` repeats the last edit, including full insert sessions.
- Jumplist with `Ctrl-O` / `Ctrl-I`.
- Undo / redo with persistent history at `~/.cache/binvim/undo/<sha>.json`
  (content-hash keyed, so out-of-band edits invalidate stale history).
- `Ctrl-A` / `Ctrl-X` increment / decrement on the line, recognising
  decimal, `0x…`, `0o…`, `0b…`, leading-zero padding, and a leading `-`
  when standalone.
- `~` toggle-case on the current char(s).
- `I` / `A` insert shortcuts.
- Indent-based folding with `za` / `zo` / `zc` / `zR` / `zM`.
- Yank flash — yanked range pulses Peach for 200ms.
- Smart Enter — copies leading whitespace, adds an indent unit after openers,
  splits `{|}` / `[|]` / `(|)` onto three lines with the cursor double-indented.
- Auto-pair brackets, quotes, backticks; auto-close HTML tags on typing `>`;
  comparison-aware guarding so `a < b` doesn't sprout a stray `>`.
- Matched bracket / matched HTML-tag highlight under the cursor.

#### Multi-buffer
- `:e` / `:bd` / `:bd!` / `:bn` / `:bp` / `:ls` / `:b{spec}`, plus the buffer
  picker (`<leader>b<space>`).
- Per-buffer state stash (cursor, viewport, history, marks, jumplist, folds,
  highlight cache).
- Auto-reload on external disk change for non-dirty buffers.
- Recent files at `~/.cache/binvim/recents`, surfaced in the file picker
  and as its own `<leader>?` shortcut.

#### Search & ex commands
- `/` / `?` / `n` / `N` / `*` / `#` with wrap-around, search-highlight, and
  `:noh` to silence it.
- `:s/pattern/replacement/[g]`, `:d`, `:y`, `:e`, `:goto`, `:health`,
  `:fmt` / `:format`.
- Ex range syntax: `1,5d`, `%s/foo/bar/g`, `.,$y`, etc.

#### LSP (hand-written JSON-RPC client over child-process stdio)
- Multiple servers per buffer: a primary (hover / goto / refs) plus
  auxiliaries (currently Tailwind on CSS/HTML/JSX/TSX/JS/TS/Astro/etc.,
  and csharp-ls layered onto Razor).
- Bundled server specs: rust-analyzer, typescript-language-server, Biome
  (JSON), gopls, vscode-html-language-server, vscode-css-language-server,
  astro-ls, csharp-ls (preferred for .cs/.vb), OmniSharp (fallback for
  .cs/.vb/Razor).
- Async `initialize` handshake — no blocking on the first open.
- 50ms `didChange` burst debounce so rapid typing doesn't flood servers.
- Drain cap on incoming traffic so diagnostic floods (OmniSharp) don't
  starve keyboard input.
- Tailwind detection covers v3 (`tailwind.config.{js,ts,cjs,…}`) and v4
  (`tailwindcss` listed in `package.json` (dev)dependencies).
- Capabilities: hover (`K`), goto-definition (`gd`), references (`gr`),
  document symbols (`<leader>o`), workspace symbols (`<leader>S`),
  completion (auto-triggered + `Ctrl-N`/`Ctrl-P`), signature help on
  `(` / `,`, code actions (`<leader>a`) including `workspace/executeCommand`
  + `workspace/applyEdit` round-trip, rename (`<leader>r`).
- Inline Error-Lens-style diagnostics with undercurl on the offending range.
- Hover popup: markdown cleanup, word-wrap, capped height, scrolling.
- Completion popup: prefix-tier ranking with substring + subsequence
  fallback; respects server `sortText` / `filterText`; preserves the
  popup across multi-server fan-out.
- `:health` opens a scratch buffer summarising version / pid / cwd /
  branch / config / CPU+RSS / open buffers / running LSPs / per-buffer
  attachment status / Tailwind config.

#### Tree-sitter highlighting
- Bundled grammars: Rust, TypeScript, JavaScript, JSX, TSX, JSON, HTML,
  CSS, Astro, Bash / Zsh, Markdown, C#, Razor.
- Capture priority by `pattern_index` — later patterns win, matching the
  upstream highlight-query convention. JSON ships a custom query (keys
  distinct from strings) because the upstream order is incompatible with
  this scheme.
- Catppuccin Mocha default theme; theme overrides via
  `~/.config/binvim/config.toml`. Capture-name resolution is dotted-prefix
  aware (`keyword.return` falls back to `keyword`).

#### Rendering & UI
- Powerline-style status line; floating command line / search prompt;
  notifications float top-right.
- Notification box wraps multi-row, caps at half terminal width, severity
  colours its border, click-to-copy lands the message in the clipboard
  and unnamed register.
- Mouse support: click, drag-select, wheel scroll (vertical + horizontal,
  with cursor drag-along).
- Horizontal viewport that follows the cursor; `zh` / `zl` / `zH` / `zL`
  for manual nudging.
- Visible tab / trailing-whitespace / EOL / non-breaking-space markers,
  configurable.
- Modal overlays dim the buffer behind them.
- Which-key popup after a 250ms leader hold.
- Start page (centred Mocha-Blue logo + configurable text) when launched
  with no path.

#### Pickers (one fuzzy engine, multiple roles)
- Files (`<leader><space>`), Buffers (`<leader>b<space>`), Grep
  (`<leader>g`, ripgrep-backed), Recents (`<leader>?`), Doc Symbols
  (`<leader>o`), Workspace Symbols (`<leader>S`), References, Code Actions.
- Skips `node_modules` and `.git/`, shows dotfiles.
- Yazi integration (`<leader>e`) — hands the terminal over and reclaims
  it cleanly on exit.

#### Save / format
- `:w`, `:wq`, `:q`, `:q!`.
- Format on save via Biome for JS / TS / JSX / TSX / JSON variants;
  resolved from project `node_modules/.bin/biome`.
- `.editorconfig` support: `indent_style`, `indent_size`,
  `insert_final_newline`, `trim_trailing_whitespace`.

#### Distribution
- Homebrew tap (`brew install bgunnarsson/binvim/binvim`).
- Linux install.sh (mirrored to `binvim-web` on each release).
- `bim` symlink installed alongside the `binvim` binary.
- binman-style local release scripts.

[Unreleased]: https://github.com/bgunnarsson/binvim/compare/v0.1.7...HEAD
[0.1.7]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.7
[0.1.6]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.6
[0.1.5]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.5
[0.1.4]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.4
[0.1.3]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.3
[0.1.2]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.2
[0.1.1]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.1
[0.1.0]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.0
