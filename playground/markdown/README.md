# binvim playground

A workspace for exercising binvim's language support.

## Features

- **Modal editing** — Vim-style operators and motions.
- **Tree-sitter highlighting** — 30+ grammars bundled.
- **LSP everywhere** — auto-discovers the right server per file.

## Quickstart

```sh
cargo install binvim
binvim path/to/file
```

## Example code

```rust
fn fib(n: u32) -> u32 {
    match n {
        0 => 0,
        1 => 1,
        _ => fib(n - 1) + fib(n - 2),
    }
}
```

```ts
const greet = (name: string) => `Hello, ${name}!`;
```

## Comparison

| Feature       | binvim | Other |
| ------------- | ------ | ----- |
| Modal editing | yes    | maybe |
| LSP           | yes    | yes   |
| Plugins       | no     | yes   |

## Quotes

> "It's just a TUI editor, but it's *my* TUI editor."
> — me, 2026

## Links

- [Homepage](https://example.com)
- [Issues](https://example.com/issues)

### Task list

- [x] Set up the playground
- [ ] Run binvim against each folder
- [ ] File a bug if something looks off

---

Footnote-style[^1] references work too.

[^1]: like this.
