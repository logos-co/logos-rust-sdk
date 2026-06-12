{
  description = "Example Logos module with logic implemented in pure Rust";

  inputs = {
    # The cdylib authoring interface (interface = "cdylib" + codegen.lidl ->
    # uniform Qt glue over the module-impl C ABI) lives on this branch until
    # the protocol-extraction chain merges — then re-point at master.
    logos-module-builder.url = "github:logos-co/logos-module-builder/22c3d6b926de0187f1371f8768597e3cf3f400dd";
  };

  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;

      # Compile the Rust staticlib. The module-impl C ABI scaffold is
      # generated from the .lidl contract by lidl-gen (build.rs); the
      # committed Cargo.lock vendors the crates.io and git dependencies.
      rustLib = system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in pkgs.rustPlatform.buildRustPackage {
          pname = "rust_provider";
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
            cp ${rustLib system}/lib/librust_provider.a lib/
          '';
        };
    in
    {
      packages = forAllSystems (system: (module system).packages.${system});
    };
}
