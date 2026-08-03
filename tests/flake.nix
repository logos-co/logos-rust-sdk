{
  description = "Integration tests for logos-rust-sdk — builds a minimal provider+caller module pair and verifies IPC via logoscore";

  inputs = {
    # The cdylib authoring interface (interface = "cdylib" + codegen.lidl ->
    # uniform Qt glue over the module-impl C ABI) lives on the builder's
    # feat/cdylib-interface branch, stacked on the qt-split chain. Temporary
    # pin — re-point at master when the chain merges.
    logos-module-builder.url = "github:logos-co/logos-module-builder/c849834b9d7b7eff1f94624c9126d7fdb77a3c48";
    # CI overrides this with --override-input logos-rust-sdk path:.
    # Keeping a real GitHub URL here lets the lock file record a valid narHash.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk";
    # Extraction-chain branch pin — temporary, re-point at master when the
    # qt-split chain merges.
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli/616cb079a5828caecfafd6d4e432519c864e3fb1";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
  };

  outputs = inputs@{ self, logos-module-builder, logos-rust-sdk, logos-logoscore-cli, nixpkgs, ... }:
    let
      mkModule = logos-module-builder.lib.mkLogosModule;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

      # The logos-protocol semver the whole stack links — stamped into the
      # generated Rust scaffold (logos_module_get_protocol_version).
      protocolVersion =
        let
          header = builtins.readFile
            "${logos-module-builder.inputs.logos-protocol}/cpp/logos_protocol.h";
          parts = builtins.split "LOGOS_PROTOCOL_VERSION_STRING \"([^\"]*)\"" header;
        in
          if builtins.length parts < 2 then "0.1.0"
          else builtins.head (builtins.elemAt parts 1);

      # Assemble the source layout the fixtures' Cargo.toml path deps expect
      # (rust-lib/ + logos-rust-sdk-src/ side by side) and build the staticlib.
      mkFixtureRustLib = { pkgs, name, dir }:
        let
          src = pkgs.runCommand "${name}-rust-src" {} ''
            mkdir -p $out
            cp -r ${dir} $out/rust-lib
            cp -r ${logos-rust-sdk} $out/logos-rust-sdk-src
          '';
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = name;
          version = "0.1.0";
          inherit src;
          sourceRoot = "${name}-rust-src/rust-lib";
          # importCargoLock (fetchurl-based) instead of cargoHash: crates.io
          # 403s fetchCargoVendor's Python fetcher; Nix's own downloader is
          # accepted. Only bites in CI on a cachix miss.
          cargoLock.lockFile = "${dir}/Cargo.lock";
          env.LOGOS_PROTOCOL_VERSION = protocolVersion;
          doCheck = false;
        };
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          # ── Module A: sdk_test_provider_module ───────────────────────────────
          # Pure Rust on the cdylib path: lidl-gen generates the module-impl
          # C ABI scaffold at cargo-build time; the builder generates the
          # uniform Qt glue from the same .lidl contract.
          provider = mkModule {
            src = ./provider;
            configFile = ./provider/metadata.json;
            flakeInputs = inputs;
            preConfigure = ''
              mkdir -p lib
              cp ${mkFixtureRustLib { inherit pkgs; name = "sdk_test_provider"; dir = ./provider/rust-lib; }}/lib/libsdk_test_provider.a lib/
            '';
          };

          # ── Module B: sdk_test_caller_module ─────────────────────────────────
          # Calls sdk_test_provider_module.add() via IPC using logos-rust-sdk.
          # One protocol stack (via logos-qt-sdk) shared by the glue and the
          # SDK — the host token forwarded through logos_module_accept_token
          # authenticates the outbound call.
          caller = mkModule {
            src = ./caller;
            configFile = ./caller/metadata.json;
            flakeInputs = { sdk_test_provider_module = provider; } // inputs;
            preConfigure = ''
              mkdir -p lib
              cp ${mkFixtureRustLib { inherit pkgs; name = "sdk_test_caller"; dir = ./caller/rust-lib; }}/lib/libsdk_test_caller.a lib/
            '';
          };

          providerInstall = provider.packages.${system}.install;
          callerInstall   = caller.packages.${system}.install;

          # Merge both modules into a single directory in LGPM layout so logoscore
          # can discover them with a single -m flag.
          modulesDir = pkgs.runCommand "sdk-test-modules-dir" {} ''
            mkdir -p $out
            for src in ${providerInstall} ${callerInstall}; do
              cp -rL "$src"/modules/* $out/ 2>/dev/null || true
            done
          '';
        in
        {
          sdk_test_provider_module = provider.packages.${system}.default;
          sdk_test_caller_module   = caller.packages.${system}.default;
          modules = modulesDir;

          default = pkgs.symlinkJoin {
            name = "logos-rust-sdk-test-modules";
            paths = [
              provider.packages.${system}.default
              caller.packages.${system}.default
            ];
          };
        }
      );

      checks = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          logoscore = logos-logoscore-cli.packages.${system}.default;
          modulesDir = self.packages.${system}.modules;
        in
        {
          # Same assertions as `nix flake check` on the root flake — one script,
          # two derivations, so CI and the local check cannot drift.
          ipc-test = pkgs.runCommand "rust-sdk-ipc-test" {
            nativeBuildInputs = [ logoscore pkgs.bash ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.qt6.qtbase ];
            MODULES_DIR = modulesDir;
          } ''
            bash ${./ipc-test.sh}
          '';
        }
      );
    };
}
