{
  description = "Basic Rust provider with a typed event";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/c47cff9b3195d33d2abad5f59768a84d8aa5bc16";
    # Provides logos-lidl-gen (the contract->scaffold generator the builder
    # runs) and the SDK the crate links. One extra input vs a C++ module.
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/441b936f2bcb309ea8f42f017f4612c139a97297";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        (logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}
