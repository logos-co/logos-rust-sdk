{
  description = "Example Logos module with logic implemented in pure Rust";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/c_ffi";
    logos-module-builder.inputs.logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk/c_ffi";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
      preConfigure = ''
        echo "=== Building Rust provider library ==="
        export HOME=$TMPDIR
        export CARGO_HOME=$TMPDIR/cargo
        mkdir -p $CARGO_HOME

        pushd rust-lib
        cargo build --release --offline
        popd

        mkdir -p lib
        cp rust-lib/target/release/librust_provider.a lib/
        cp rust-lib/include/rust_provider.h lib/
        echo "=== Rust provider library built and staged ==="
      '';
    };
}
