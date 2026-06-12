{
  description = "Rust-first Logos module: contract declared in Rust, typed cross-language deps";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/7ed223ebd3f86f2163443b04771ecdd9f7400dcf";
    # The dependency's flake. Placeholder — locked to the real checkout in
    # the build step via --override-input (nix rejects relative paths here).
    cpp_counter_module.url = "path:/path/to/cpp-counter";
  };

  outputs = inputs@{ self, logos-module-builder, cpp_counter_module, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;

      rustLib = system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in pkgs.rustPlatform.buildRustPackage {
          pname = "rust_orchestrator";
          version = "1.0.0";
          src = ./rust-lib;
          cargoLock = {
            lockFile = ./rust-lib/Cargo.lock;
            allowBuiltinFetchGit = true;
          };
          doCheck = false;
        };

      module = system:
        logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
          preConfigure = ''
            mkdir -p lib
            cp ${rustLib system}/lib/librust_orchestrator.a lib/
          '';
        };
    in
    {
      packages = forAllSystems (system: (module system).packages.${system});
    };
}
