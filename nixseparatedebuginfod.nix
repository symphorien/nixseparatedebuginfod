{ callPackage, libarchive, pkg-config, lib }:
let
  customBuildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      compress-tools = attrs: {
        buildInputs = [ libarchive ];
        nativeBuildInputs = [ pkg-config ];
      };
    };
  };
  generatedBuild = callPackage ./Cargo.nix {
    buildRustCrateForPkgs = customBuildRustCrateForPkgs;
  };
in generatedBuild.rootCrate.build

