{
    description = "binvim playground — minimal Nix flake";

    inputs = {
        nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
        flake-utils.url = "github:numtide/flake-utils";
    };

    outputs = { self, nixpkgs, flake-utils }:
        flake-utils.lib.eachDefaultSystem (system:
            let
                pkgs = import nixpkgs { inherit system; };
                name = "binvim-playground";
            in {
                packages.default = pkgs.stdenv.mkDerivation {
                    pname = name;
                    version = "0.1.0";
                    src = ./.;
                    buildPhase = ''
                        echo "building ${name}"
                    '';
                    installPhase = ''
                        mkdir -p $out
                        cp -r . $out
                    '';
                };

                devShells.default = pkgs.mkShell {
                    name = "${name}-shell";
                    buildInputs = with pkgs; [
                        rustc
                        cargo
                        nodejs_20
                        python3
                    ];

                    shellHook = ''
                        echo "welcome to ${name}"
                    '';
                };
            });
}
