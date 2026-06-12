{
  description = "Integration tests for logos-rust-sdk — builds a minimal provider+caller module pair and verifies IPC via logoscore";

  inputs = {
    # The cdylib authoring interface (interface = "cdylib" + codegen.lidl ->
    # uniform Qt glue over the module-impl C ABI) lives on the builder's
    # feat/cdylib-interface branch, stacked on the qt-split chain. Temporary
    # pin — re-point at master when the chain merges.
    logos-module-builder.url = "github:logos-co/logos-module-builder/c115998c95c335061715327085ea564840d98463";
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
          ipc-test = pkgs.runCommand "rust-sdk-ipc-test" {
            nativeBuildInputs = [ logoscore ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.qt6.qtbase ];
          } ''
            mkdir -p $out
            export QT_QPA_PLATFORM=offscreen

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
