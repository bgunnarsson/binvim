# Tree-sitter highlighting

Rust, TypeScript / TSX / JSX, JavaScript, JSON, Go, **Python**, **C / C++**, **Java**, **Ruby**, **PHP**, **Lua**, **TOML**, **Svelte**, **Zig**, **Nix**, **Elixir**, **Dockerfile** / **Containerfile**, **SQL**, HTML, CSS (`.less` on the same grammar), **SCSS / Sass**, Markdown, C#, **Razor** (`.cshtml` / `.razor`), **YAML**, **XML** (including `.csproj` / `.fsproj` / `.vbproj` / `.props` / `.targets` / `.config` / `.manifest` / `.nuspec` / `.resx` / `.xaml` / `.xhtml` / `.xsd` / `.xsl` / `.xslt` / `.plist`), Bash. Kotlin is parsed but not coloured — its grammar crate ships no highlights query, so colour comes from the LSP's semantic tokens. **`.editorconfig`** and the **`.gitignore`** family (`.gitignore`, `.gitattributes`, `.dockerignore`, `.npmignore`) are coloured by a small byte-level scanner rather than a grammar.

Pattern-priority resolution so `(method_declaration name: (identifier) @function)` deterministically beats the catch-all `(identifier) @variable`.

A few language-specific tweaks on top of the bundled queries:

- **JSX / TSX** — overlay tags lowercase elements (`<div>`) as `@tag` (Pink) and PascalCase components (`<Foo>`, `<Foo.Bar>`) as `@constructor` (Yellow). `{expr}` braces inside JSX get treated as JSX-template syntax (`@operator`) instead of falling through to the object-literal punctuation tone.
- **Razor** — `@inject` / `@using` / `@{…}` / `@if` / `@(…)` etc. paint as `@keyword.directive`, and `@*…*@` as `@comment`; C# inside the blocks is highlighted by the C# query. A byte-level overlay handles HTML tag / attribute names + C# keywords inside broken-parse regions (BOM headers, Tailwind `class="…[16px]…"` bracket attributes, …).
- **CSS** — replacement query so selectors and properties don't collide: `.class-name` is `@constructor` (Yellow), `#id-name` is `@label` (Sapphire), `property:` is `@property` (Lavender), `--custom-prop` is `@variable`, at-rules (`@media`/`@keyframes`/…) are `@keyword` (Mauve).
- **`.editorconfig`** — comments, `[*.cs]` section headers in Pink, `key = value` pairs with the key in Lavender, `=` in Sky, value in Green.
- **`.gitignore` family** — `#` comments, `!`-negation prefix in Mauve, patterns in Lavender.

Known issues: an upstream C-scanner bug in `tree-sitter-bash` can abort the
process rather than fail softly on hostile input, and SCSS on Windows falls back
to the CSS grammar. See
[KNOWN_ISSUES.md](../KNOWN_ISSUES.md).
