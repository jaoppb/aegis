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
      pkgsFor = system: import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      workspaceVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      # The CLIs the adapters shell out to: pinned by flake.lock, shared by the devShell and the
      # container images so a release ships the versions CI tested against.
      runtimeClis = pkgs: with pkgs; [
        docker-client
        docker-compose
        sops
        tailscale
        git
        openssh
      ];
      # The static (musl) workspace binaries: one package per binary crate.
      binaries = system:
        let
          pkgs = pkgsFor system;
          static = pkgs.pkgsStatic;
          toolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            targets = [ static.stdenv.hostPlatform.rust.rustcTarget ];
          };
          rustPlatform = static.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./clippy.toml
              ./rust-toolchain.toml
              ./crates
              ./tests
            ];
          };
          mkBinary = crate: rustPlatform.buildRustPackage {
            pname = crate;
            version = workspaceVersion;
            inherit src;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--package" crate ];
            # `just check` runs the tests; a release build only has to compile.
            doCheck = false;
            # pkgsStatic exports `-static` in NIX_CFLAGS_LINK, which the build-platform (glibc)
            # compiler also reads: cargo then links every build script statically against glibc
            # and the binary jumps to a null `__libc_start_main` (SIGSEGV in whichever build script
            # runs first, e.g. `icu_normalizer_data`). The musl target links static through
            # `-C target-feature=+crt-static`, so the variable is not needed.
            preBuild = "unset NIX_CFLAGS_LINK";
            meta = {
              description = "Aegis: GitOps for Docker Compose stacks (${crate})";
              license = pkgs.lib.licenses.asl20;
              mainProgram = crate;
            };
          };
        in
        {
          aegis = mkBinary "aegis";
          aegis-controller = mkBinary "aegis-controller";
          aegis-agent = mkBinary "aegis-agent";
        };
      # A container image for one daemon: the static binary plus the pinned CLIs. Streamed
      # (`nix build .#aegis-controller-image && ./result | docker load`), no tarball in the store.
      image = system: name: binary:
        let
          pkgs = pkgsFor system;
        in
        pkgs.dockerTools.streamLayeredImage {
          inherit name;
          tag = workspaceVersion;
          contents = [ binary pkgs.cacert ] ++ runtimeClis pkgs;
          # sops and compose write temporary files.
          extraCommands = "mkdir -m 1777 tmp; mkdir -p -m 0700 var/lib/aegis/home";
          config = {
            Entrypoint = [ "/bin/${name}" ];
            Env = [
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "HOME=/var/lib/aegis/home"
            ];
            Labels = {
              "org.opencontainers.image.source" = "https://github.com/jaoppb/aegis";
              "org.opencontainers.image.licenses" = "Apache-2.0";
              "org.opencontainers.image.version" = workspaceVersion;
            };
          };
        };
    in
    {
      packages = forAllSystems (system:
        let
          bins = binaries system;
        in
        bins // {
          aegis-controller-image = image system "aegis-controller" bins.aegis-controller;
          aegis-agent-image = image system "aegis-agent" bins.aegis-agent;
          default = bins.aegis;
        }
      );

      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          default = pkgs.mkShell {
            packages = runtimeClis pkgs ++ (with pkgs; [
              rustToolchain
              cargo-nextest
              cargo-deny
              just
              lefthook
              rumdl
              gh
              # Issue workflow and repository config (tools.toml).
              tools.packages.${system}.tools
              jq
              # The adapters' `docker`, `sops` and `tailscale` CLIs come from `runtimeClis`, the
              # same pinned versions the container images ship; the devShell adds the engine.
              docker
              age
              # Workflow lint (actionlint runs shellcheck on `run:` blocks).
              actionlint
              shellcheck
              # build-release checks the binaries are static.
              file
            ]);
            # Checked by the justfile's `_devshell` guard.
            AEGIS_DEVSHELL = "1";
          };
        }
      );
    };
}
