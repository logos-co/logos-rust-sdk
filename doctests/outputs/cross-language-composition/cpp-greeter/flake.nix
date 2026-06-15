{
  description = "Contract-first C++ cdylib module: a greeter";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/03ad946f1928cff35373a21838f89d6fd7c8eadc";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
