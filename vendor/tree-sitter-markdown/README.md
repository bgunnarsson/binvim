# tree-sitter-markdown (block grammar), vendored

The block grammar from [tree-sitter-md](https://github.com/tree-sitter-grammars/tree-sitter-markdown)
0.3.2 (tag `v0.3.2`), compiled by binvim's `build.rs` in place of the crate. MIT, see `LICENSE`.

It is vendored for two changes to `src/scanner.c`, each marked `binvim:` in the source. Both
were ways for a Markdown file to abort binvim, and 0.5.3 still has both.

- **`parse_ordered_list_marker`:** the ordered-list digit loop compares against `'0'..='9'` instead
  of calling `isdigit` on a Unicode scalar. On glibc, `isdigit` could read past its table when a
  digit at the start of a line was followed by a codepoint at or above U+0100 (`1€`). Upstream
  closed the same fix (tree-sitter-markdown#252) unmerged.
- **`scan`:** once the block stack holds as many blocks as `serialize` can fit in tree-sitter's
  1024-byte state buffer, no token that opens a block is offered, so a deeper `>`, list marker,
  fence or HTML block reads as text. `serialize` never checked the size, and the runtime aborts
  once the buffer overflows.

Everything else is the tag's files unchanged: `src/parser.c`, `src/tree_sitter/*.h`,
`queries/highlights.scm`. When upstream fixes both, drop this directory and `build.rs` and go
back to the crate.
