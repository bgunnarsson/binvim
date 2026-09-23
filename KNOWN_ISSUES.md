# Known issues

Defects binvim ships with, and what each one costs you. Every entry names where
it lives in the tree so it can be checked rather than believed. Fixed issues
move out of here and into `CHANGELOG.md`; whole areas that have never been
audited are Horizon 2 in `ROADMAP.md`, not entries here.

There is no issue tracker — this file is it.

## Highlighting

### Adversarial multibyte UTF-8 can abort binvim through the bash grammar

**Bash, `.editorconfig` and the `.gitignore` family. No fix available.**

`tree-sitter-bash`'s C external scanner has a latent out-of-bounds read on
adversarial multibyte input — supplementary-plane and combining-mark soup. Same
shape as the Markdown bug above: the bad read only lands on an unmapped page
once enough prior parses have shaped the heap, so it appeared as a
non-deterministic `SIGSEGV` in `fuzz_bash` on native x86_64 CI, never on
aarch64, never reproducible from one input. The other thirty grammars chew
through the same stream without complaint, which pins it to bash's scanner.

The three bash-backed grammars are fuzzed over printable ASCII
(`BASH_FUZZ_ALPHABET`, `lang.rs:2714`). Restore `\PC{0,400}` if a fixed
`tree-sitter-bash` lands upstream.

### Markdown nested past ~254 block levels is not highlighted

**Markdown. Bounded — degrades instead of crashing.**

A file nested about 255 block levels deep (`>>>>…`, or list items) overflows
tree-sitter's fixed scanner-state serialization buffer and aborts. binvim now
measures an upper bound on nesting depth first and skips the parse entirely when
it exceeds `MARKDOWN_MAX_OPEN_BLOCKS` (`lang.rs:888`), so such a file opens with
no Markdown highlighting rather than not opening at all.

### A grammar that will not finish loses its highlighting

**Every language. Bounded — degrades instead of hanging.**

Tree-sitter's query cursor can spin indefinitely on pathological input — a
silent 99% CPU hang with no output. Parse and query run under one shared
deadline (`TREE_SITTER_BUDGET`, 500ms, `lang.rs:836`), shared across every
injected sub-language pass so an HTML file with fifty `<script>` blocks is
bounded once overall. A file that exhausts it renders unhighlighted.

### SCSS falls back to the CSS grammar on Windows

**SCSS, MSVC targets only. Waiting on an upstream release.**

`tree-sitter-scss` 1.0.0's `build.rs` passes the GCC-only
`-Wno-unused-parameter` to whatever C compiler the host picks, which breaks
`cl.exe` (`D8021: invalid numeric argument`). The dependency is gated behind
`cfg(not(target_env = "msvc"))` (`Cargo.toml:93`), so Windows SCSS parses as
CSS: selectors and properties highlight fine, while `$var`, `@mixin`,
`@include`, `#{}`, `%placeholder` and `&` nesting lose their dedicated captures.

A fix exists on upstream master but has not been released. Re-enable everywhere
once upstream guards the flag with `flag_if_supported`.

## Recovery

### Without a cache directory, nothing is recovered and an in-place write isn't guarded

Recovery files and the copy a save takes before writing in place both live
under the cache directory (`~/.cache/binvim`, or `XDG_CACHE_HOME`). binvim goes
without one when there's no `HOME`, and when the directory or one above it
belongs to another user (`paths::cache_dir`). The second case covers `sudo
binvim`, which keeps your `HOME`, so root doesn't leave copies of its files
where you can read them. Without one, unsaved text isn't dumped, and a save that has to write in
place — a hard-linked file, a directory you can't create files in, an owner the
temp file can't take, a failed rename — goes ahead without a copy
(`write_in_place`, `src/paths.rs`), so a write failing partway leaves the file
truncated. The text is still in the buffer, so `:w` again once the cause is
fixed.

## Terminals

Each terminal's results are in `TERMINALS.md`. What is listed here is a
terminal's doing rather than binvim's, and the matrix links to it.

### `Ctrl-[` leaves the `:terminal` pane on a terminal without the Kitty keyboard protocol

**tmux, and any terminal that doesn't speak the protocol. No fix available in
binvim.**

In the `:terminal` pane `Esc` hands focus back to the editor, and `Ctrl-[` is
how an Esc reaches the program running there (a vi-mode shell, vim, `less`).
That only works where the terminal reports `Ctrl-[` apart from `Esc`, which is
what the Kitty keyboard protocol's disambiguate flag does. A legacy terminal
sends the same byte, 0x1b, for both keys, so binvim can't tell them apart and
`Ctrl-[` leaves the pane like `Esc` does (`TERMINALS.md` check 19).

Workaround: use a terminal that passes check 19, where the pane gets the Esc,
or run the program that needs it in a split of the host terminal rather than
in `:terminal`.

## Tests

### A grammar that segfaults is only identifiable when tests run sequentially

A C-side `SIGSEGV` produces no Rust panic, and per-test output is captured and
lost on crash. Run in parallel, the test that died is one of `num_cpus` in
flight and only the ones that already finished printed a result. CI therefore
runs `cargo test -- --test-threads=1` (`ci.yml:53`), where the last `... ok`
line is the test immediately before the one that crashed. It costs about five
seconds across the suite, and more than that on the Windows runners.

Run the suite the same way locally when judging a change: several unrelated
tests fail under parallel execution on macOS, so a parallel run's failures are
not evidence of anything.

## Vim compatibility

No key is left undecided. Where binvim departs from Vim on purpose, and what it
leaves out, is listed under *Where binvim differs on purpose* and *Left out* in
`docs/vim-compatibility.md`, and stays there rather than being copied here.
