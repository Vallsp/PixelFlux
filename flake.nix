{
  description = "Pixelflux — production-grade SDLC pipeline (Rust + Nix). Dev shell + distroless container.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Toolchain pinned via rust-toolchain.toml (incl. the musl target).
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Pick the musl target matching the host architecture so the
        # container runs natively (x86_64 or aarch64 / ARM).
        muslTarget =
          if pkgs.stdenv.hostPlatform.isAarch64
          then "aarch64-unknown-linux-musl"
          else "x86_64-unknown-linux-musl";

        # Keep Cargo sources, plus the embedded web UI (include_str!).
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          name = "source";
          filter = path: type:
            (pkgs.lib.hasSuffix ".html" path) || (craneLib.filterCargoSources path type);
        };

        # Arguments shared by dependency and package builds.
        commonArgs = {
          inherit src;
          strictDeps = true;

          # Fully static binary so the container needs nothing but the binary.
          CARGO_BUILD_TARGET = muslTarget;
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";

          # Integration tests need Docker; keep the Nix build hermetic.
          doCheck = false;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        bin = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });

        # Distroless image: ONLY the static binary closure. No shell, no
        # package manager. Runs as a non-root user (uid 65532, "nonroot").
        container = pkgs.dockerTools.buildLayeredImage {
          name = "pixelflux";
          tag = "latest";
          contents = [ ];
          config = {
            Entrypoint = [ "${bin}/bin/pixelflux" ];
            User = "65532:65532";
            ExposedPorts = { "3000/tcp" = { }; };
            Env = [ "PORT=3000" ];
          };
        };
      in
      {
        packages = {
          default = bin;
          inherit container;
        };

        # `nix develop` (or direnv) — the single command to a full toolbox.
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain

            # Task runner + git hooks + formatting
            pkgs.go-task
            pkgs.lefthook
            pkgs.treefmt

            # Security / supply chain
            pkgs.gitleaks
            pkgs.trivy
            pkgs.syft
            pkgs.dive

            # Testing & benchmarking
            pkgs.k6
            pkgs.hurl

            # Linters & formatters
            pkgs.taplo
            pkgs.nixpkgs-fmt
            pkgs.shfmt
            pkgs.shellcheck
            pkgs.yamllint
            pkgs.actionlint
            pkgs.prettier
            pkgs.markdownlint-cli
            pkgs.vale
            pkgs.lychee

            # Misc utilities
            pkgs.jq
            pkgs.docker-client
          ];

          shellHook = ''
            echo "Pixelflux dev shell ready — run 'task' to list every available task."
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      }
    );
}
