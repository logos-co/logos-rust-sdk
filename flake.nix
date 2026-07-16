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
    # The SDK's FFI binds the lp_* C ABI. Out-of-plugin callers link it from
    # logos-protocol's shared library (liblogos_protocol.{so,dylib}); the pin
    # is reached via logos-module-builder.inputs.logos-protocol below so the
    # lp_* ABI matches what the cdylib modules embed.
    # Test-only: module builder + logoscore drive the integration test suite.
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    logos-logoscore-cli.url = "github:logos-co/logos-logoscore-cli";
  };

  outputs = inputs@{ self, nixpkgs, logos-nix, logos-lidl, logos-module-builder, logos-logoscore-cli }:
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

      # Both test fixtures are Rust crates with a path dep on this repo. Their
      # module-impl C-ABI scaffold (`src/provider_gen.rs`) is generated from the
      # .lidl contract by `logos-lidl-gen --provider` and CHECKED IN — regenerate
      # with that command if the .lidl changes (see each crate's
      # provider_gen.rs header).
      #
      # Why checked in instead of generated in the fixture's own build.rs (as a
      # build-dependency on lidl-gen)? Under nixpkgs, CARGO_BUILD_TARGET makes a
      # build-dependency a HOST unit, and lidl-gen's build.rs there cannot link
      # logos-lidl's C ABI — the host build script reaches the archives by no
      # mechanism (no LOGOS_LIDL_ROOT/RUSTFLAGS, nothing on PATH, and
      # buildRustPackage strips any file the build *generates* into the crate;
      # only committed source survives). So the C frontend runs out-of-process
      # (the CLI links the archives fine as a target unit) and its output is
      # committed. This matches how module-builder ships Rust module scaffolds.
      mkFixtureRustLib = { pkgs, name, dir }:
        pkgs.rustPlatform.buildRustPackage {
          pname = name;
          version = "0.1.0";
          src = pkgs.runCommand "${name}-rust-src" {} ''
            mkdir -p $out
            cp -r ${dir} $out/rust-lib
            cp -r ${self} $out/logos-rust-sdk-src
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
          # Run the generator's unit tests as part of the build so they gate
          # `nix flake check` / `ws test`. These are pure codegen + FFI-parse
          # tests (no network, no daemon) and only need LOGOS_LIDL_ROOT, already
          # set above; without this they never execute in CI — which is how a
          # bstr-event codegen bug and a stale assertion both shipped unnoticed.
          doCheck = true;
          cargoTestFlags = [ "-p" "logos-lidl-gen" ];
        };
      });

      # Opaque build support for Rust binaries that call other modules via IPC
      # from OUTSIDE a module plugin (no qt-sdk/protocol link of their own).
      # Provides extraBuildInputs and a setupHook over logos-protocol's shared
      # library (liblogos_protocol.{so,dylib}), which exports the lp_* C ABI the
      # SDK's FFI binds. Sourced from the builder's protocol pin so it matches
      # the lp_* ABI the cdylib modules embed. Modules built on the cdylib path
      # don't need this — their lp_* resolve against the protocol archive
      # already in the plugin.
      lib.callerBuildSupport = nixpkgs.lib.genAttrs systems (system:
        let
          protocol = logos-module-builder.inputs.logos-protocol.packages.${system}.logos-protocol;
        in
        {
          extraBuildInputs = [ protocol ];
          setupHook = ''
            export LOGOS_PROTOCOL_ROOT="${protocol}"
          '';
          # Path to liblogos_protocol.{so,dylib} — set LD_LIBRARY_PATH to this
          # in test derivations so subprocesses can find the library.
          runtimeLibPath = "${protocol}/lib";
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

            # Binary EVENT round trip. The caller subscribed to the provider's
            # blobReady(seq, payload: bstr) event in on_context_ready; ask the
            # provider to emit a 4096-byte deterministic blob, then read back what
            # the subscription actually received. This exercises the generated
            # bstr-event emitter end-to-end — a payload that dropped or corrupted
            # in codegen would surface here as size 0 or a wrong checksum. (The
            # blob is byte i = (i*7+11)&0xff; the caller's checksum is
            # sum(payload[i] * (i%31 + 1)) = 8354754 for 4096 bytes.)
            if ! emitted=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
                 call sdk_test_provider_module emit_blob 4096 2>emit.err); then
              echo "logoscore emit_blob call failed:" >&2
              cat emit.err "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2 || true
              exit 1
            fi
            echo "emit_blob result: $emitted"
            if ! printf '%s' "$emitted" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]'; then
              echo "IPC test FAILED (expected emit_blob(4096) == 4096): $emitted" >&2
              exit 1
            fi

            # The event is delivered to the caller's listener thread off the Qt
            # event loop, so poll briefly for it rather than assuming it landed.
            size="" ; checksum=""
            for _i in $(seq 1 50); do
              size=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
                     call sdk_test_caller_module last_blob_size 2>/dev/null || true)
              printf '%s' "$size" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]' && break
              sleep 0.2
            done
            checksum=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
                       call sdk_test_caller_module last_blob_checksum 2>/dev/null || true)
            echo "last_blob_size: $size"
            echo "last_blob_checksum: $checksum"
            if ! printf '%s' "$size" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]'; then
              echo "IPC test FAILED (binary event payload not received intact; expected size 4096): $size" >&2
              cat "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2
              exit 1
            fi
            if ! printf '%s' "$checksum" | grep -qE '"result"[[:space:]]*:[[:space:]]*8354754[[:space:]]*[,}]'; then
              echo "IPC test FAILED (binary event payload corrupted; expected checksum 8354754): $checksum" >&2
              cat "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2
              exit 1
            fi

            {
              echo "IPC test passed: sdk_test_provider_module.add(5,3) returned 8 via IPC"
              echo "Binary event passed: blobReady payload received as 4096 bytes, checksum 8354754"
            } > $out/result.txt
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
