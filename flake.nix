{
  description = "phux — a terminal multiplexer built on libghostty-vt";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        # Read channel/components from rust-toolchain.toml. No hash needed —
        # rust-overlay derives it from the rustup metadata.
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
            # libghostty-vt-sys builds the C library via Zig at build time.
            pkgs.zig_0_15
            pkgs.pkg-config
            # Developer ergonomics.
            pkgs.just
            pkgs.cargo-nextest
            pkgs.cargo-deny
            pkgs.cargo-watch
            pkgs.cargo-insta
            pkgs.cargo-mutants
            # Debugging.
            pkgs.lldb
          ];

          env.RUST_BACKTRACE = "1";

          shellHook = ''
            echo "phux dev shell — $(rustc --version)"
          '';
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}
