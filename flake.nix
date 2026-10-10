{
  description = "Bombay controlled Zenoh verification toolchains";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      nixpkgs,
      utils,
      fenix,
      ...
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        toolchain =
          (fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "sha256-zm3dyIY2T414ZRR3EhLOvptzG6gta4WZUcawzMUWtqI=";
          }).overrideAttrs
            (previous: {
              nativeBuildInputs =
                (previous.nativeBuildInputs or [ ])
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.darwin.cctools ];
            });
        miriToolchain =
          (fenix.packages.${system}.toolchainOf {
            channel = "nightly";
            date = "2026-06-15";
            sha256 = "sha256-oXipquOa/9M0uuo8wGuRaY2+ZqLGywZOOnRK05Mm0a0=";
          }).withComponents
            [
              "cargo"
              "rustc"
              "rust-src"
              "rust-std"
              "miri"
            ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
            pkgs.python3
            pkgs.pkg-config
            pkgs.openssl
          ];
        };
        devShells.miri = pkgs.mkShell { packages = [ miriToolchain ]; };
        formatter = pkgs.nixfmt;
      }
    );
}
