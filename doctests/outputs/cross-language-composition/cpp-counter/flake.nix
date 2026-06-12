{
  description = "Contract-first C++ cdylib module: a counter";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/7ed223ebd3f86f2163443b04771ecdd9f7400dcf";
  };

  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
