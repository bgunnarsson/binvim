{
  description = "binvim — Vim-grammar TUI editor with batteries included.";

  inputs = {
    # nixpkgs-unstable carries a recent enough rustc for edition-2024 +
    # `rust-version = 1.85` in Cargo.toml. If your channel pins older
    # nixpkgs, override this input with `--override-input nixpkgs <ref>`
    # or pin a fresher branch in your own flake.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # Source-available, redistribution prohibited — see LICENSE. The
      # nixpkgs predefined `unfree` tag is the closest fit but loses the
      # specifics; declaring it inline keeps the actual terms visible to
      # `nix-env -qa --json` consumers.
      binvimLicense = {
        fullName = "binvim Source-Available License (BSAL) v1.0";
        url = "https://github.com/bgunnarsson/binvim/blob/main/LICENSE";
        free = false;
        redistributable = false;
      };
      # Read the version out of Cargo.toml so the flake never drifts from
      # the crates.io / Homebrew / GitHub Release source of truth.
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs { inherit system; };

          binvim = pkgs.rustPlatform.buildRustPackage {
            pname = "binvim";
            version = cargoToml.package.version;
            src = ./.;

            # `lockFile` re-uses the committed Cargo.lock so the
            # nix-built binary matches what `cargo install --locked
            # binvim` produces on a host system.
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            # Tree-sitter grammars compile from C at build time. The
            # default rust stdenv brings a cc, but we add pkg-config
            # because a couple of transitive deps probe for it.
            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            # `arboard` with `default-features = false` uses the bare
            # X11 clipboard on Linux (libxcb), Cocoa on macOS (auto-
            # linked), nothing on minimal targets. Conditionalised so
            # the macOS build doesn't drag in xorg.
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.xorg.libxcb
            ];

            # The 440-strong test suite is happy in a clean sandbox,
            # but the panic-hardening crash test
            # (`test_crash_writes_log_on_panic`) writes to a temp
            # `~/.cache` path. The sandbox has no $HOME by default,
            # so skip tests at flake-build time — `cargo test` on the
            # host is the source of truth for CI anyway.
            doCheck = false;

            meta = with pkgs.lib; {
              description = cargoToml.package.description;
              homepage = "https://binvim.dev";
              license = binvimLicense;
              mainProgram = "binvim";
              # Both binaries from the same crate land in $out/bin —
              # `binvim` (the editor) and `binvim-install` (the
              # toolchain installer). `nix run github:bgunnarsson/binvim`
              # uses mainProgram for the default.
              platforms = platforms.unix;
            };
          };
        in
        {
          packages = {
            default = binvim;
            binvim = binvim;
          };

          # `nix run github:bgunnarsson/binvim` → opens the editor.
          # `nix run github:bgunnarsson/binvim#binvim-install` → runs
          # the toolchain installer. Both share the same derivation;
          # only the `program` path differs.
          apps = {
            default = {
              type = "app";
              program = "${binvim}/bin/binvim";
            };
            binvim = {
              type = "app";
              program = "${binvim}/bin/binvim";
            };
            binvim-install = {
              type = "app";
              program = "${binvim}/bin/binvim-install";
            };
          };

          # `nix develop` drops you into a shell with the toolchain
          # needed to hack on binvim itself (cargo + rustfmt +
          # clippy + the tree-sitter C build deps).
          devShells.default = pkgs.mkShell {
            buildInputs = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              pkg-config
            ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.xorg.libxcb
            ];
          };
        }) // {
      # System-agnostic outputs. Overlays let downstream system
      # configs do `nixpkgs.overlays = [ inputs.binvim.overlays.default ];`
      # and then reference `pkgs.binvim` like any other nixpkgs
      # package — useful for NixOS/home-manager users who want
      # binvim available as a global package alongside everything
      # else they install.
      overlays.default = final: prev: {
        binvim = self.packages.${prev.stdenv.hostPlatform.system}.default;
      };
    };
}
