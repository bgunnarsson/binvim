# Install

## macOS — Homebrew

```sh
brew install bgunnarsson/binvim/binvim
```

The tap lives at [github.com/bgunnarsson/homebrew-binvim](https://github.com/bgunnarsson/homebrew-binvim). The formula compiles from source (`depends_on "rust" => :build`) — first install takes a minute or two while the tree-sitter grammars compile.

## Linux — install script

```sh
curl -fsSL https://binvim.dev/install.sh | sh
```

Pulls the matching musl-static tarball (`x86_64` or `aarch64`) from the latest GitHub Release, verifies its SHA-256, and drops the binary at `~/.local/bin/binvim`. Override with `BINVIM_VERSION=v0.1.0` or `BINVIM_INSTALL_DIR=/opt/bin` if needed.

## Windows — PowerShell installer

```powershell
iwr https://binvim.dev/install.ps1 -UseBasicParsing | iex
```

Pulls the `x86_64-pc-windows-msvc` zip from the latest GitHub Release and drops `binvim.exe` (+ `binvim-install.exe`) into `%LOCALAPPDATA%\binvim\bin\`. The script doesn't mutate `PATH` — it prints the one-liner to do that yourself, so the install stays reversible. Override with `$env:BINVIM_VERSION = 'v0.1.0'` or `$env:BINVIM_INSTALL_DIR = 'C:\bin'`.

`:terminal` on Windows requires Windows 10 1809+ (ConPTY). Older versions will fail to open a PTY.

## Windows — Scoop

```powershell
scoop bucket add binvim https://github.com/bgunnarsson/binvim
scoop install binvim
```

Uses the manifest at [`scoop/binvim.json`](../scoop/binvim.json) in this repo — the repo doubles as the bucket. The release script points it at each new release's Windows zip, so `scoop update binvim` picks up a release as soon as it's out.

## crates.io

```sh
cargo install --locked binvim
```

Builds from source against the version published on crates.io. `--locked` uses the `Cargo.lock` shipped with the crate so the dep set matches what was tested for the release. Both `binvim` and `binvim-install` land in `~/.cargo/bin/`.

## Nix flake

```sh
nix run github:bgunnarsson/binvim              # one-shot, in a temporary store path
nix profile install github:bgunnarsson/binvim  # install permanently to your profile
nix run github:bgunnarsson/binvim#binvim-install  # run the toolchain installer
```

For NixOS / home-manager system configs, add binvim as a flake input and use the default overlay:

```nix
{
  inputs.binvim.url = "github:bgunnarsson/binvim";
  outputs = { self, nixpkgs, binvim, ... }: {
    nixosConfigurations.<hostname> = nixpkgs.lib.nixosSystem {
      modules = [
        { nixpkgs.overlays = [ binvim.overlays.default ]; }
        ({ pkgs, ... }: { environment.systemPackages = [ pkgs.binvim ]; })
      ];
    };
  };
}
```

`nix develop github:bgunnarsson/binvim` drops you into a shell with the toolchain (cargo, rustfmt, clippy, pkg-config + the tree-sitter C build deps) for hacking on binvim itself.

## From source

```sh
cargo build --release
```

The binary lands at `target/release/binvim`. Requires a stable Rust toolchain.

## `binvim-install` — set up LSPs, formatters, and DAP adapters

A second binary, `binvim-install`, ships alongside `binvim` from every install path (Homebrew, the Linux tarball, `cargo build --release`). Run it once to bring up the toolchains binvim drives:

```sh
binvim-install
```

It opens a checkbox list of every language binvim supports. Pick the ones you care about, and it'll detect which package managers you have on `$PATH` (`brew`, `apt-get`, `npm`, `cargo`, `rustup`, `go`, `pipx`, `pip`, `gem`, `dotnet`, `nix`, `composer`), pick the right installer per tool, dedupe shared tools across languages (`prettier`, `lldb-dap`, `vscode-langservers-extracted`, …), show you the plan, and run the installs once you confirm. Anything that can't be auto-installed (`netcoredbg`, OmniSharp) prints the manual steps instead. The full per-tool reference table is under [External tools](external-tools.md) for users who'd rather install by hand.

The same flow is available **inside the editor** — `:install` (or `:installer`) opens a full-screen overlay with the identical three-stage UX (bundles → optional Node.js versions → plan review). `y` on the plan stage suspends binvim lazygit-style and runs the installs against the host terminal, then drops back into the editor with a status-line summary. Both entry points share the catalog + runner in `binvim::install`, so adding a language only requires touching one place.

`:update` runs the same overlay but only upgrades tools you already have on `$PATH` to the catalog's pinned (or newest) versions — handy after a binvim release bumps its pins. Tools that aren't installed are left untouched and flagged "not installed — run :install to add it". Managers that own their own version (`brew`, `apt`, `nix`) upgrade via their native upgrade command; pinned managers (`npm`, `cargo`, `go`, `gem`, `pipx`, `dotnet`, `composer`) re-run their install at the pin.

The **first checkbox in the `:update` list is binvim itself**. It detects how the running binary was installed — Homebrew, cargo, the install script, Scoop, or Nix — from the executable's path and runs the matching upgrade (`brew upgrade …`, `cargo install --locked --force binvim`, re-running `install.sh`, `scoop update binvim`, `nix profile upgrade binvim`). A source/dev build is detected too and shown with manual instructions instead. The new binary takes effect on the next launch.

## Run

```sh
binvim [path]
```

If `path` is omitted and a session exists for this cwd, the session restores (start page + tab row above it). Otherwise the start page renders alone. Press `:` for a command (`:e <path>`, `:q`) or `<space>` to open the file picker.
