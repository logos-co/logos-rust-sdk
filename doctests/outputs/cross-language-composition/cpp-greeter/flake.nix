{
  description = "Contract-first C++ cdylib module: a greeter";
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/113b2e1228d059393f12050db9eeaa57a5123536";
  };
  outputs = inputs@{ self, logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
