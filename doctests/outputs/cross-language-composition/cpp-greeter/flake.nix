{
  description = "Contract-first C++ cdylib module: a greeter";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/19e2c133b7202ce8a8675791a7dbf136f2eeb96f";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
