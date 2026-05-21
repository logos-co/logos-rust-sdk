{
  description = "Logos Rust SDK — Rust crate for calling other Logos modules via IPC";

  inputs = {
    logos-nix.url = "github:logos-co/logos-nix";
    nixpkgs.follows = "logos-nix/nixpkgs";
    logos-module-client.url = "github:logos-co/logos-module-client/new_api_test";
    logos-module-client.inputs.logos-nix.follows = "logos-nix";
    logos-module-client.inputs.logos-cpp-sdk.follows = "logos-module-builder/logos-cpp-sdk";
    # Test-only: module builder + logoscore are needed for the integration test suite.
    logos-module-builder.url = "github:logos-co/logos-module-builder/c_ffi";
    logos-module-builder.inputs.logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk/c_ffi";
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli";
  };

  outputs = inputs@{ self, nixpkgs, logos-nix, logos-module-client, logos-module-builder, logos-logoscore-cli }:
    let
      mkModule = logos-module-builder.lib.mkLogosModule;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        inherit system;
        pkgs = import nixpkgs { inherit system; };
      });

      # ── Test modules ──────────────────────────────────────────────────────────
      # These are built only when checks are evaluated (ws test / nix flake check).
      # Source lives in tests/; tests/flake.nix is also a standalone flake for the
      # same test suite.
      provider = mkModule {
        src = ./tests/provider;
        configFile = ./tests/provider/metadata.json;
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
      # Opaque build support for Rust modules that call other modules via IPC.
      # Provides extraBuildInputs and a setupHook — consumers don't need to
      # know about logos-module-client or any env vars.
      lib.callerBuildSupport = nixpkgs.lib.genAttrs systems (system:
        let
          mc    = logos-module-client.packages.${system}.logos-module-client;
          mcLib = logos-module-client.packages.${system}.logos-module-client-lib;
        in
        {
          extraBuildInputs = [ mcLib ];
          setupHook = ''
            export LOGOS_MODULE_CLIENT_ROOT="${mc}"
          '';
          # Path to liblogos_module_client.so — set LD_LIBRARY_PATH to this
          # in test derivations so logoscore subprocesses can find the library.
          runtimeLibPath = "${mc}/lib";
        }
      );

      checks = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          rustSdkBuild = self.lib.callerBuildSupport.${system};

          callerRustSrc = pkgs.runCommand "sdk-test-caller-rust-src" {} ''
            mkdir -p $out
            cp -r ${./tests/caller/rust-lib} $out/rust-lib
            cp -r ${self} $out/logos-rust-sdk-src
          '';

          callerRustLib = pkgs.rustPlatform.buildRustPackage {
            pname = "sdk_test_caller";
            version = "0.1.0";
            src = callerRustSrc;
            sourceRoot = "sdk-test-caller-rust-src/rust-lib";
            cargoHash = "sha256-6r17qKn4l1SWNac+3/8/4/YxGlGY2QEI3eAbznxyBAI=";
            doCheck = false;
          };

          caller = mkModule {
            src = ./tests/caller;
            configFile = ./tests/caller/metadata.json;
            flakeInputs = { sdk_test_provider_module = provider; } // inputs;
            extraBuildInputs = rustSdkBuild.extraBuildInputs;
            preConfigure = ''
              mkdir -p lib
              cp ${callerRustLib}/lib/libsdk_test_caller.a lib/
              cp rust-lib/include/sdk_test_caller.h lib/
              ${rustSdkBuild.setupHook}
            '';
          };

          providerInstall = provider.packages.${system}.install;
          callerInstall   = caller.packages.${system}.install;

          modulesDir = pkgs.runCommand "sdk-test-modules-dir" {} ''
            mkdir -p $out
            for src in ${providerInstall} ${callerInstall}; do
              cp -rL "$src"/modules/* $out/ 2>/dev/null || true
            done
          '';

          logoscore = logos-logoscore-cli.packages.${system}.default;
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

      devShells = forAllSystems ({ pkgs, ... }: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rustc
            cargo
            rust-analyzer
            rustfmt
            clippy
          ];
        };
      });
    };
}
