{
  description = "Logos Rust SDK — Rust crate for calling other Logos modules via IPC";

  inputs = {
    logos-nix.url = "github:logos-co/logos-nix";
    nixpkgs.follows = "logos-nix/nixpkgs";
    # The canonical, language-neutral LIDL frontend. lidl-gen reaches its
    # parser/serializer/validator over the C ABI (lidl_ffi) instead of
    # reimplementing the grammar; build.rs links the C archives from here.
    logos-lidl = {
      url = "github:logos-co/logos-lidl";
      inputs.logos-nix.follows = "logos-nix";
    };
    # The SDK's FFI binds the lp_* C ABI; the chain logos-module-client shared
    # library exports it (it links logos-protocol statically).
    logos-module-client.url = "github:logos-co/logos-module-client";
    # Test-only: module builder + logoscore drive the integration test suite.
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli";
  };

  outputs = inputs@{ self, nixpkgs, logos-nix, logos-lidl, logos-module-client, logos-module-builder, logos-logoscore-cli }:
    let
      mkModule = logos-module-builder.lib.mkLogosModule;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        inherit system;
        pkgs = import nixpkgs { inherit system; };
      });

      # The logos-protocol semver the whole stack links — stamped into the
      # generated Rust scaffold (logos_module_get_protocol_version), forwarded
      # from the protocol header, never minted here.
      protocolVersion =
        let
          header = builtins.readFile
            "${logos-module-builder.inputs.logos-protocol}/cpp/logos_protocol.h";
          parts = builtins.split "LOGOS_PROTOCOL_VERSION_STRING \"([^\"]*)\"" header;
        in
          if builtins.length parts < 2 then "0.1.0"
          else builtins.head (builtins.elemAt parts 1);

      # Both test fixtures are Rust crates with a path dep on this repo. The Rust
      # C-ABI scaffold is generated from the .lidl contract HERE, by the prebuilt
      # logos-lidl-gen CLI, and written into the crate's `src/` as a real source
      # file the build then compiles.
      #
      # Why not generate it in the fixture's own build.rs (as a build-dependency
      # on lidl-gen)? Under nixpkgs, CARGO_BUILD_TARGET makes a build-dependency a
      # HOST unit, and lidl-gen's build.rs there cannot link logos-lidl's C ABI:
      # the host build script sees no LOGOS_LIDL_ROOT / RUSTFLAGS, no
      # nativeBuildInputs/depsBuildBuild on PATH, and no files written after
      # unpack (only `src/*.rs` and committed crate files survive into the build).
      # So the C frontend runs out-of-process in the CLI (it links the archives
      # fine as a target unit), and only its generated `.rs` output crosses into
      # the build. This matches how module-builder generates Rust scaffolds.
      mkFixtureRustLib = { pkgs, name, dir }:
        pkgs.rustPlatform.buildRustPackage {
          pname = name;
          version = "0.1.0";
          src = pkgs.runCommand "${name}-rust-src" {} ''
            set -euo pipefail
            mkdir -p $out
            cp -r ${dir} $out/rust-lib
            cp -r ${self} $out/logos-rust-sdk-src
            chmod -R u+w $out/rust-lib
            ${self.packages.${pkgs.system}.lidl-gen}/bin/logos-lidl-gen \
              $out/rust-lib/*.lidl --provider --protocol-version ${protocolVersion} \
              -o $out/rust-lib/src/provider_gen.rs
          '';
          sourceRoot = "${name}-rust-src/rust-lib";
          # importCargoLock (fetchurl-based) instead of cargoHash: crates.io
          # 403s fetchCargoVendor's Python fetcher; Nix's own downloader is
          # accepted. Only bites in CI on a cachix miss.
          cargoLock.lockFile = "${dir}/Cargo.lock";
          doCheck = false;
        };

      # ── Test modules ──────────────────────────────────────────────────────────
      # Both fixtures author on the common cdylib path: lidl-gen generates the
      # Rust C-ABI scaffold at cargo-build time; the builder generates the
      # uniform Qt glue from the same .lidl (interface = "cdylib"). The plugin
      # links one logos-protocol stack (via logos-qt-sdk) shared by the glue
      # and the Rust SDK, so the host token forwarded through
      # logos_module_accept_token authenticates the caller's outbound calls.
      mkProvider = { pkgs, ... }:
        mkModule {
          src = ./tests/provider;
          configFile = ./tests/provider/metadata.json;
          flakeInputs = inputs;
          preConfigure = ''
            mkdir -p lib
            cp ${mkFixtureRustLib { inherit pkgs; name = "sdk_test_provider"; dir = ./tests/provider/rust-lib; }}/lib/libsdk_test_provider.a lib/
          '';
        };

      mkCaller = { pkgs, provider, ... }:
        mkModule {
          src = ./tests/caller;
          configFile = ./tests/caller/metadata.json;
          flakeInputs = { sdk_test_provider_module = provider; } // inputs;
          preConfigure = ''
            mkdir -p lib
            cp ${mkFixtureRustLib { inherit pkgs; name = "sdk_test_caller"; dir = ./tests/caller/rust-lib; }}/lib/libsdk_test_caller.a lib/
          '';
        };
    in
    {
      # The lidl-gen CLI: derive .lidl contracts from Rust traits
      # (--from-rust), generate typed clients / provider scaffolds / Modules
      # aggregates from .lidl — for tooling and doctests that need the
      # generator outside a cargo build script.
      packages = forAllSystems ({ pkgs, system, ... }: {
        lidl-gen = pkgs.rustPlatform.buildRustPackage {
          pname = "logos-lidl-gen";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "logos-lidl-gen" ];
          # logos-lidl: build.rs links its C ABI archives (the binary embeds the
          # canonical frontend, so the published CLI is self-contained).
          buildInputs = [ logos-lidl.packages.${system}.logos-lidl ];
          env.LOGOS_LIDL_ROOT = "${logos-lidl.packages.${system}.logos-lidl}";
          doCheck = false;
        };
      });

      # Opaque build support for Rust binaries that call other modules via IPC
      # from OUTSIDE a module plugin (no qt-sdk/protocol link of their own).
      # Provides extraBuildInputs and a setupHook over logos-module-client,
      # whose shared library links logos-protocol statically and re-exports
      # the lp_* C ABI. Modules built on the cdylib path don't need this —
      # their lp_* resolve against the protocol archive already in the plugin.
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
          # in test derivations so subprocesses can find the library.
          runtimeLibPath = "${mc}/lib";
        }
      );

      checks = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; };

          provider = mkProvider { inherit pkgs; };
          caller   = mkCaller { inherit pkgs provider; };

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
