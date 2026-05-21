{
  description = "Integration tests for logos-rust-sdk — builds a minimal provider+caller module pair and verifies IPC via logoscore";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/c_ffi";
    logos-module-builder.inputs.logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk/c_ffi";
    # CI overrides this with --override-input logos-rust-sdk path:.
    # Keeping a real GitHub URL here lets the lock file record a valid narHash.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk";
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
  };

  outputs = inputs@{ self, logos-module-builder, logos-rust-sdk, logos-logoscore-cli, nixpkgs, ... }:
    let
      mkModule = logos-module-builder.lib.mkLogosModule;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

      # ── Module A: sdk_test_provider_module ─────────────────────────────────
      # Pure Rust, no external deps. Exposes add(a, b) -> a + b via c-ffi codegen.
      provider = mkModule {
        src = ./provider;
        configFile = ./provider/metadata.json;
        flakeInputs = inputs;
        preConfigure = ''
          export HOME=$TMPDIR
          export CARGO_HOME=$TMPDIR/cargo
          mkdir -p $CARGO_HOME

          pushd rust-lib
          cargo build --release --offline
          popd

          mkdir -p lib
          cp rust-lib/target/release/libsdk_test_provider.a lib/
          cp rust-lib/include/sdk_test_provider.h lib/
        '';
      };

    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          # callerBuildSupport bundles logos-module-client (extraBuildInputs + setupHook)
          # so the caller flake never needs to know about logos-module-client directly.
          rustSdkBuild = logos-rust-sdk.lib.callerBuildSupport.${system};

          # ── Rust caller staticlib ─────────────────────────────────────────────
          # Assemble a source tree matching the Cargo.toml path layout:
          #   rust-lib/              (Cargo.toml, Cargo.lock, src/, include/)
          #   logos-rust-sdk-src/    (the SDK crate, used as path dep)
          callerRustSrc = pkgs.runCommand "sdk-test-caller-rust-src" {} ''
            mkdir -p $out
            cp -r ${./caller/rust-lib} $out/rust-lib
            cp -r ${logos-rust-sdk} $out/logos-rust-sdk-src
          '';

          callerRustLib = pkgs.rustPlatform.buildRustPackage {
            pname = "sdk_test_caller";
            version = "0.1.0";
            src = callerRustSrc;
            sourceRoot = "sdk-test-caller-rust-src/rust-lib";
            # Hash covers vendored deps (serde, serde_json, etc.) — same tree as
            # logos-rust-example-module's caller, so the same hash applies.
            cargoHash = "sha256-6r17qKn4l1SWNac+3/8/4/YxGlGY2QEI3eAbznxyBAI=";
            doCheck = false;
          };

          # ── Module B: sdk_test_caller_module ──────────────────────────────────
          # Calls sdk_test_provider_module.add() via IPC using logos-rust-sdk.
          caller = mkModule {
            src = ./caller;
            configFile = ./caller/metadata.json;
            flakeInputs = { sdk_test_provider_module = provider; } // inputs;

            extraBuildInputs = rustSdkBuild.extraBuildInputs;

            preConfigure = ''
              mkdir -p lib
              cp ${callerRustLib}/lib/libsdk_test_caller.a lib/
              cp rust-lib/include/sdk_test_caller.h lib/

              ${rustSdkBuild.setupHook}
            '';
          };

        in
        let
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
          rustSdkBuild = logos-rust-sdk.lib.callerBuildSupport.${system};
        in
        {
          ipc-test = pkgs.runCommand "rust-sdk-ipc-test" {
            nativeBuildInputs = [ logoscore ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.qt6.qtbase ];
          } ''
            mkdir -p $out
            export QT_QPA_PLATFORM=offscreen
            export LD_LIBRARY_PATH="${rustSdkBuild.runtimeLibPath}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

            logoscore --quit-on-finish \
              -m ${modulesDir} \
              -l sdk_test_caller_module \
              -c "sdk_test_caller_module.call_add(5, 3)"

            echo "IPC test passed: sdk_test_provider_module.add(5,3) returned via IPC" > $out/result.txt
          '';
        }
      );
    };
}
