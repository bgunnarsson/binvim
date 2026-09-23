# Known issues

Defects binvim ships with, and what each one costs you. Every entry names where
it lives in the tree so it can be checked rather than believed. Fixed issues
move out of here and into `CHANGELOG.md`; whole areas that have never been
audited are Horizon 2 in `ROADMAP.md`, not entries here.

There is no issue tracker — this file is it.

## Highlighting

### An ordered-list marker followed by a non-ASCII character can abort binvim

**Markdown, glibc only. No fix available.**

`tree-sitter-md`'s block scanner reads an ordered-list marker with
`while (isdigit(lexer->lookahead))`, handing a full Unicode scalar to the
*narrow* `isdigit`, whose argument is undefined outside `0..=255`. glibc indexes
its ctype table unchecked, so a digit followed by a codepoint at or above U+0100
— `1€` at the start of a line — reads past the table. Usually that is a wrong
answer; occasionally it is an unmapped page and a `SIGSEGV` that takes the
process with it, because a C-side fault is not a Rust panic and nothing above it
can catch it.

Present through `tree-sitter-md` 0.5.3. macOS's range-checked libc hides it, and
it has never reproduced from a single captured input — it needs an allocator
shaped by enough prior parses — so it surfaced only as a non-deterministic
crash in `fuzz_markdown` on native x86_64 CI.

The call is inherent to the grammar, so closing it means an upstream fix or a
vendored grammar. Until then the fuzz suite feeds Markdown printable ASCII plus
tab and newline (`MARKDOWN_FUZZ_ALPHABET`, `lang.rs:2728`), which keeps the
byte-offset invariant and every block shape under test while never exercising
the path. That stops the random CI crash; it does not stop the crash.

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
