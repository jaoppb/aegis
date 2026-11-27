{
  description = "Aegis developer environment";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    tools = {
      url = "github:jaoppb/tools";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, tools }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ];
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              cargo-nextest
              just
              lefthook
              rumdl
              gh
              # Issue workflow and repository config (tools.toml).
              tools.packages.${system}.tools
              jq
              git
              # The CLIs the adapters shell out to; pinned to guard against output drift.
              docker
              docker-compose
              sops
              age
              tailscale
              # Workflow lint (actionlint runs shellcheck on `run:` blocks).
              actionlint
              shellcheck
            ];
            # Checked by the justfile's `_devshell` guard.
            AEGIS_DEVSHELL = "1";
          };
        }
      );
    };
}
