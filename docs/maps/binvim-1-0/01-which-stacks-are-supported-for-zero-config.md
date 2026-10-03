---
title: Which stacks does 1.0 call supported, and what must each one have to count?
kind: grilling
mode: hitl
status: resolved
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar says first run works with zero manual config for the supported stacks, and the roadmap says depth is LSP + debugger + formatter + tree-sitter for every advertised language. Today only Rust, Go, Python, C/C++ and C# have all four. Decide which stacks 1.0 promises, and for each whether a debugger and formatter are required or the promise is narrower (for example LSP + formatter + highlighting, with a debugger only where advertised).

## Context

- `docs/roadmap.md` (strategic through-line, 1.0 quality bar).
- `docs/external-tools.md`, `src/install.rs` `BUNDLES`, `src/dap/specs.rs` `BUILTIN_ADAPTERS`.
- Gaps as charted: no debugger outside the five runtimes above; no formatter for Razor, TOML, Docker.

## Answer

1.0 promises six stacks, Rust, Go, Python, C/C++, C#/.NET and TypeScript/JavaScript (with HTML, CSS and JSON), each with LSP + debugger + formatter + tree-sitter installed from the first-run prompt with no manual step on macOS, Linux and Windows, shown by a scripted fresh-environment run per stack per OS; every other language is "also works", outside the promise.

Settled with the user 2026-10-03:

- **The six stacks.** These keep the roadmap's "depth, not count" rule (`docs/roadmap.md:148-150`) without spending 1.0 on debuggers for Elixir or Zig. Lua, Ruby, PHP, Java, Zig, Nix, Elixir, Kotlin, SQL, YAML, Bash, Markdown, Svelte, TOML, Docker and the rest keep what they have and are labelled "also works".
- **What a promised stack needs.** All four parts, every tool installed by the first-run prompt, on all three OSes. The narrower promise (no debugger where none ships) was turned down: it would make TS/JS a promised stack with no debugger.
- **Razor is not in the .NET promise.** It is "also works": its LSP is OmniSharp, a manual install (`src/install.rs` Razor bundle), and csharpier punts on `.cshtml`, so it falls back to an indent reflow (`src/format.rs:28-30`).
- **Evidence.** One scripted fresh-environment run per stack per OS: open a sample project, accept the prompt, then check that hover answers, that format changes the file, and that a breakpoint is hit. Linux runs in a container in CI. macOS and Windows run on the user's machines, and their rows are filled from the user's reports, as the terminal matrix is. Test runners do not count towards the bar.

Gaps against this, read from the code on 2026-10-03:

- Rust has no formatter: `format_buffer` has no `"rs"` arm (`src/format.rs:17-55`), though the Rust bundle installs `rustfmt` (`src/install.rs:420`).
- TS/JS has no debugger: `BUILTIN_ADAPTERS` is .NET, Rust, Go, Python (`src/dap/specs.rs:108`).
- C#'s netcoredbg is `Installer::Manual` only (`src/install.rs`, C# bundle).
- C/C++ is debugged only inside a Cargo project: lldb-dap's only root marker is `Cargo.toml` (`src/dap/specs.rs:513-524`).
- The first-run prompt never offers a debugger: `missing_core_tools` checks only `Role::Lsp` and `Role::Formatter` (`src/install.rs:684-702`).
- TOML does have a formatter (taplo, `src/format.rs:46`), contrary to the map's first notes.
