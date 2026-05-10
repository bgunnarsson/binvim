# Contributing to binvim

Thanks for considering a contribution. binvim is a small project with a small surface area, and the bar for merging is "the code matches the existing style and the change is something the maintainer wants in the editor." This document covers what you need to know before opening a PR.

## Licence and what "contribution" means here

binvim is **source-available, not open source** — see [LICENSE](LICENSE) for the full text. The short version that matters for contributors:

- You may clone, build, and modify binvim for your own use on hardware you control.
- You may **not** publish a public fork. A private fork on a code-hosting platform that exists solely to prepare a PR is fine, but it has to be deleted or kept private once the PR is merged or abandoned.
- By submitting a PR you grant the maintainer a perpetual, irrevocable, sublicensable licence to use your contribution as part of binvim, including under different licence terms in the future (LICENSE §4). You also represent that the work is yours to grant.

If you can't agree to that, please don't open a PR.

## Before you start

For anything bigger than a one-line fix, **open an issue first**. binvim is opinionated about what it includes — pre-agreed scope avoids the awkward case where a working PR gets closed because the feature isn't wanted. Good things to flag up front:

- New language / LSP support — see [LSP_ADOPTION.md](LSP_ADOPTION.md), which already tiers the obvious candidates.
- New keybindings or operators — the parser is a Vim-grammar state machine; new verbs need to fit it, not bolt onto it.
- New configuration surface — `~/.config/binvim/config.toml` is intentionally minimal.
- New external-tool dependencies — every external binary in the README install table is one more thing that can be missing on a user's machine.

Bug fixes, missing-LSP arms, and tree-sitter additions don't need a pre-discussion — just open the PR.

## Development setup

```sh
cargo build                                  # debug build
cargo build --release                        # release build (target/release/binvim)
cargo test                                   # full suite, ~45 unit tests
cargo test motion::tests                     # one module
cargo test motion::tests::word_forward_basic # one test
cargo run -- path/to/file                    # debug-build run
```

There is no CI. There is no enforced `cargo fmt` or `clippy` config. Run them locally if you like; they are not gating.

If you're testing changes by running `binvim` interactively, remember that **the install/alias path is `target/release/binvim`** — a debug build will not be picked up. Run `cargo build --release` after the change you want to exercise.

## Repo conventions

These are not stylistic preferences — they are how the codebase is structured, and PRs that fight them tend to get bounced.

- **Flat `src/` layout.** Every module is a single file. Don't introduce `src/lsp/` directories or sub-modules without a real reason; "the file is getting big" is not one (`app.rs` is ~5k lines on purpose — the state machine is centralised so the rest of the modules can stay pure-ish).
- **No new files unless necessary.** Prefer extending an existing module. New top-level files need to justify themselves.
- **Tests live inline, in `#[cfg(test)] mod tests` at the bottom of the file under test.** No separate `tests/` directory, no `tests/integration/`. `motion.rs` and `text_object.rs` have the densest coverage and are the model.
- **Comments explain *why*, not *what*.** The existing comments in `lang.rs` (priority resolution), `lsp.rs` (debounce window), and `app.rs` (BufferStash shape) are the pattern: load-bearing context that isn't obvious from the code. Don't add what-comments. Don't add multi-paragraph docstrings.
- **No backwards-compatibility shims, feature flags, or `// removed` markers** for code that's been deleted. Just delete it.
- **Don't over-abstract.** Three similar lines is better than a premature abstraction. Don't design for hypothetical future requirements.
- **LF line endings only.** No CRLF.

## Architecture quick reference

For a longer tour see [CLAUDE.md](CLAUDE.md). The 30-second version:

- `app.rs` owns the event loop, active buffer, per-buffer stashes, and all transient UI state. Action dispatch lives here.
- `parser.rs` turns `KeyEvent`s into `Action` values via the Vim-grammar state machine. Operators, motions, text-objects, counts, registers, leader, surround — all resolved here before `app.rs` sees them.
- `motion.rs` and `text_object.rs` are pure functions over `(buffer, cursor)`. New motions or text objects belong here, with tests inline.
- `lang.rs` owns tree-sitter. The non-obvious bit: **highlight captures resolve by pattern_index priority — later patterns win**. JSON ships its own embedded query because the upstream pattern order is incompatible with that scheme. If you change the priority logic, the JSON block at `lang.rs:88` is the canary.
- `lsp.rs` is a from-scratch JSON-RPC client. Multiple servers per buffer is supported and used (e.g. tsserver + Tailwind on `.tsx`). `didChange` is debounced with a 50ms burst window in `app.rs`.
- `render.rs` is the only module that talks to crossterm for drawing.

## Adding a new LSP

[LSP_ADOPTION.md](LSP_ADOPTION.md) is the authoritative recipe. The four-file change is always:

1. New arm in `primary_spec_for_path` (`src/lsp.rs`).
2. Extension → `Lang` mapping in `src/lang.rs`.
3. `tree-sitter-<lang>` crate in `Cargo.toml` (only if you also want highlighting).
4. New row in the README install table.

There is no plugin system. Every server is hard-wired in `lsp.rs`. That is a deliberate choice; please don't propose a plugin loader as part of an LSP PR.

## Adding tree-sitter highlighting for an existing LSP

Add the crate to `Cargo.toml`, then a `Lang` variant + `ts_language()` arm + `highlights_query()` arm. If the upstream highlights query is wrong under "later pattern wins" priority (see JSON), embed a corrected query inline rather than patching the priority logic.

## Verifying a change

Before opening a PR:

- `cargo test` is green.
- `cargo build --release` succeeds.
- For LSP / language changes: open a representative file, run `:health`, and confirm the server appears under **LSP servers** with the expected `key`, `language_id`, and detected `root`. Trigger completion and hover on a known symbol.
- For UI / rendering / keybinding changes: actually run the release binary and use the feature. Type-checks and tests verify code correctness, not feature correctness.

## Pull requests

- One logical change per PR. If you're tempted to write "and also fixed X" in the description, X is a separate PR.
- Branch from `main`. Rebase on top of `main` before opening; no merge commits.
- PR description should explain the *why* — what the user-visible behaviour was before, what it is after, and what motivated the change. The maintainer can read the diff for the *what*.
- No Claude / AI / "Co-Authored-By" attribution in commit messages, branch names, or PR descriptions.
- Reference the issue number if you opened one.

## Reporting bugs

Open a GitHub issue with:

- binvim version (`binvim --version` or the commit SHA you built from).
- OS and terminal emulator.
- A minimal reproduction — file contents (or a path to a public repo), exact keystrokes, what you expected, what happened. `:health` output is often the fastest way to tell whether an LSP-shaped bug is a binvim issue or a missing server.

For LSP-specific bugs, the server logs go to stderr; running `binvim 2> /tmp/binvim.log` and attaching the relevant section is the most useful thing you can include.

## Contact

For licensing questions outside the scope of LICENSE — redistribution, commercial use, hosted-service provision — contact the maintainer on Twitter/X at [@bgunnarssonis](https://twitter.com/bgunnarssonis). For everything else, the issue tracker is the right place.
