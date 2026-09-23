# tree-sitter-markdown (block grammar), vendored

The block grammar from [tree-sitter-md](https://github.com/tree-sitter-grammars/tree-sitter-markdown)
0.3.2 (tag `v0.3.2`), compiled by binvim's `build.rs` in place of the crate. MIT, see `LICENSE`.

It is vendored for one change, in `src/scanner.c`'s `parse_ordered_list_marker`: the ordered-list
digit loop compares against `'0'..='9'` instead of calling `isdigit` on a Unicode scalar, which
could abort binvim on glibc when a digit at the start of a line is followed by a codepoint at or
above U+0100 (`1€`). Upstream closed the same fix (tree-sitter-markdown#252) unmerged, and 0.5.3
still has the call.

Everything else is the tag's files unchanged: `src/parser.c`, `src/tree_sitter/*.h`,
`queries/highlights.scm`. When upstream fixes it, drop this directory and `build.rs` and go back
to the crate.
