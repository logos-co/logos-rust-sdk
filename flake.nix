{
  description = "Logos Rust SDK — Rust crate for calling other Logos modules via IPC";

  inputs = {
    logos-nix.url = "github:logos-co/logos-nix";
    nixpkgs.follows = "logos-nix/nixpkgs";
    # The SDK's FFI binds the lp_* C ABI; the chain logos-module-client shared
    # library exports it (it links logos-protocol statically). Extraction-chain
    # branch pin — temporary, re-point at master when the qt-split chain merges.
    logos-module-client.url = "github:logos-co/logos-module-client/2bf380e0684c2467796a999fa7e569bb36eb4780";
    # Test-only: module builder + logoscore are needed for the integration test
    # suite. The c-ffi module interface (codegen.c_header) lives on the legacy
    # c_ffi builder branch; the qt-split chain replaces it with the cdylib
    # authoring path, to be adopted here as a follow-up.
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
      # know about logos-module-client or any env vars. The SDK's extern "C"
      # lp_* symbols resolve against liblogos_module_client.so, which links
      # logos-protocol statically and re-exports the lp_* C ABI. (Linking the
      # protocol archive directly into a c_ffi-era plugin would duplicate the
      # transport/token symbols its SDK already carries.)
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
            # Use importCargoLock (fetchurl-based) instead of cargoHash
            # (fetchCargoVendor's Python fetcher). crates.io now returns 403 to the
            # Python fetcher's generic User-Agent; Nix's own downloader (fetchurl) is
            # accepted. Only bites in CI on a cachix miss, where the vendor FOD is
            # actually built rather than substituted. Mirrors tests/flake.nix.
            cargoLock.lockFile = ./tests/caller/rust-lib/Cargo.lock;
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

            # Inline (`-c`) mode is legacy; drive a logoscore daemon and call via
            # the `call` client subcommand. A persistent daemon keeps the Qt event
            # loop running so the cross-module IPC reply is delivered reliably.
            export LOGOSCORE_CONFIG_DIR="$(mktemp -d)"
            DAEMON_PID=""
            cleanup() {
              logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" stop >/dev/null 2>&1 || true
              [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
              rm -rf "$LOGOSCORE_CONFIG_DIR"
            }
            trap cleanup EXIT

            logoscore -D --config-dir "$LOGOSCORE_CONFIG_DIR" -m ${modulesDir} \
              >"$LOGOSCORE_CONFIG_DIR/daemon.log" 2>&1 &
            DAEMON_PID=$!
            # `status` is the definitive readiness probe; no need to poke at the
            # daemon's internal state file.
            ready=0
            for _i in $(seq 1 100); do
              if logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" status >/dev/null 2>&1; then
                ready=1; break
              fi
              kill -0 "$DAEMON_PID" 2>/dev/null || break
              sleep 0.2
            done
            [ "$ready" = 1 ] || { echo "logoscore daemon failed to start:" >&2; cat "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2; exit 1; }

            # load-module does not auto-resolve dependencies; load provider, then caller.
            logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" load-module sdk_test_provider_module
            logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" load-module sdk_test_caller_module

            # Fail loudly if the call errors (don't swallow its exit code), and
            # match the value exactly so "8" isn't satisfied by e.g. "80".
            if ! result=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
                 call sdk_test_caller_module call_add 5 3 2>caller.err); then
              echo "logoscore call failed:" >&2
              cat caller.err "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2 || true
              exit 1
            fi
            echo "call result: $result"
            if ! printf '%s' "$result" | grep -qE '"result"[[:space:]]*:[[:space:]]*8[[:space:]]*[,}]'; then
              echo "IPC test FAILED (expected sdk_test_caller_module.call_add(5,3) == 8): $result" >&2
              cat "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2
              exit 1
            fi

            echo "IPC test passed: sdk_test_provider_module.add(5,3) returned 8 via IPC" > $out/result.txt
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
