# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

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

