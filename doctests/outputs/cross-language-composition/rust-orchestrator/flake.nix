{
  description = "Rust consumer: concrete + interface deps, context, sync/async, events";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/03ad946f1928cff35373a21838f89d6fd7c8eadc";
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/8b89e562a52218af6beef6fa6e3cfa12ab52e93e";
    # The concrete dependency's flake (its published .lidl drives
    # modules().rust_calc_module). Placeholder — locked to the real checkout
    # at build time via --override-input (nix rejects relative paths here).
    rust_calc_module.url = "path:/path/to/rust-calc";
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
