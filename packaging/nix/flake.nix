{
  description = "gwae: niri's scrolling tiling for your CLI agents, in any terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = nixpkgs.legacyPackages.${system};
      in {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "gwae";
          version = "1.2.0";
          src = self;
          nativeBuildInputs = with pkgs; [ cargo rustc pkg-config ];
          buildPhase = "cargo build --release";
          installPhase = "install -Dm755 target/release/gwae $out/bin/gwae";
          meta = {
            description = "niri's scrolling tiling for your CLI agents, in any terminal";
            license = pkgs.lib.licenses.mit;
            homepage = "https://github.com/hongnoul/gwae";
          };
        };
      });
}
