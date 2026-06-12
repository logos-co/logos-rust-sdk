{
  description = "Universal C++ consumer of the Rust orchestrator";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/7ed223ebd3f86f2163443b04771ecdd9f7400dcf";
    # Placeholder — locked to the real checkout in the build step via
    # --override-input (nix rejects relative paths here).
    rust_orchestrator_module.url = "path:/path/to/rust-orchestrator";
  };

  outputs = inputs@{ self, logos-module-builder, rust_orchestrator_module, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
