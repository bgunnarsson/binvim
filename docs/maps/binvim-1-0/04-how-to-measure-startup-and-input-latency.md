---
title: How are startup time, input latency and memory measured, reproducibly, against a Neovim distro?
kind: research
mode: afk
status: resolved
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar needs budgets that are met and published. Before numbers can be set, decide the method: the tools (hyperfine, typometer-style key-to-photon, a pty harness timing a key to its redraw, RSS sampling), the comparison setup (LazyVim version, files opened), what can run as a script in this repo and in CI, and what has to run by hand.

## Context

- `docs/roadmap.md` Horizon 2, "Performance budget with numbers" (render coalescing).
- `src/app.rs` run loop and `src/render.rs` for where a key becomes a frame.
- `scripts/terminal-probe.sh` and `scripts/terminal-check.rs` as the pattern for a check script.
- Web sources on measuring terminal editor latency; cite links.

## Answer

Measure with three scripts in the repo, run on the same machine and the same file for both editors. Startup uses hyperfine (`-N`, warm-up, 50+ runs) to the first frame. Key-to-redraw uses a pty harness: write one key, stop when a vt100 parse of the output shows the expected screen, 1,000+ keys, report p50/p90/p99/p99.9. RSS is sampled after load and after a fixed idle. CI runs them only as ratios against nvim in the same job. The published absolute numbers come from the user's hardware, with the terminal named.

- binvim has no timing code today: no benches, no criterion, no profiling flag. Every `Instant::now()` is a deadline (scout, over `src/`). The harness is new, and `scripts/terminal-probe.sh` is not a pattern for it: it drives `/dev/tty` by hand.
- A frame is drawn at the top of the loop once `needs_render` is set (`src/app.rs:1299`). Input is drained up to 256 events per frame (`src/app.rs:1475`), and the idle poll is 100 ms (`src/app.rs:1316`). So a key's redraw is not held to a tick: the harness measures handling plus one `render::draw`.
- Headless startup is not first frame. `nvim --headless +q` skips the UI and LazyVim's event-loaded plugins, so both editors are timed from spawn to their first painted screen in a pty, for example by waiting for the mode line.
- "Redraw done" is judged on screen contents, not on the first byte, because nvim and binvim emit their redraws differently. The building blocks are `portable-pty` + `vt100` in Rust, or `pexpect` + `pyte`. No standard published tool exists.
- For a fair LazyVim run: pin the nvim release, commit `lazy-lock.json`, use its own `NVIM_APPNAME`, and install parsers and LSPs before timing. Report `nvim --clean` beside it as the floor.
- Files: an empty buffer, about 100 lines of Rust, and a generated 10k-line Rust file, committed or reproducible.
- Typometer or a camera is an optional hardware cross-check, never CI.

Sources: [hyperfine](https://github.com/sharkdp/hyperfine), [Typing with pleasure (Typometer)](https://pavelfatin.com/typing-with-pleasure/), [Dan Luu, Terminal latency](https://danluu.com/term-latency/). The Neovim flag semantics are from the researcher's knowledge: its fetch of [the startup docs](https://neovim.io/doc/user/starting.html) returned nothing.
