{
  description = "Integration tests for logos-rust-sdk — builds a minimal provider+caller module pair and verifies IPC via logoscore";

  inputs = {
    # The c-ffi module interface (codegen.c_header → glue + consumer api
    # headers) lives on the legacy c_ffi builder branch; the qt-split chain
    # replaces it with the cdylib authoring path (logos_module_impl.h), to be
    # adopted here as a follow-up. Until then the fixtures build with the
    # c_ffi builder, while the Rust SDK inside them binds the lp_* C ABI
    # (resolved by the logos-protocol static lib via callerBuildSupport).
    logos-module-builder.url = "github:logos-co/logos-module-builder/c_ffi";
    logos-module-builder.inputs.logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk/c_ffi";
    # CI overrides this with --override-input logos-rust-sdk path:.
    # Keeping a real GitHub URL here lets the lock file record a valid narHash.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk";
    # Extraction-chain branch pin — temporary, re-point at master when the
    # qt-split chain merges.
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli/f409cffc0a762c0e376268f349baaa09217e4059";
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
          # so the caller flake never needs to know where the lp_* C ABI comes from.
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
            # Use importCargoLock (fetchurl-based) instead of cargoHash
            # (fetchCargoVendor's Python fetcher). crates.io now returns 403 to the
            # Python fetcher's generic User-Agent; Nix's own downloader (fetchurl) is
            # accepted. Only bites in CI on a cachix miss, where the vendor FOD is
            # actually built rather than substituted.
            cargoLock.lockFile = ./caller/rust-lib/Cargo.lock;
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
    };
}
