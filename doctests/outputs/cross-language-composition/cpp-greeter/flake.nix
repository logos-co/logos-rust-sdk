{
  description = "Contract-first C++ cdylib module: a greeter";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/c47cff9b3195d33d2abad5f59768a84d8aa5bc16";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
