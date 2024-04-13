{ pkgs ? (import (builtins.fetchTarball "channel:nixos-unstable") { }) }:
let
  overlay = self: super: {
    nixseparatedebuginfod = super.callPackage ../nixseparatedebuginfod.nix { };
  };
  lib = pkgs.lib;
  nixos-lib = import (pkgs.path + "/nixos/lib") { };
  testDir = ./nixos;
  mkTest = path: nixos-lib.runTest ({ hostPkgs = pkgs; } // ((import path) { inherit pkgs lib overlay; }));
in
lib.mapAttrs'
  (filename: _type: {
    name = lib.removeSuffix ".nix" filename;
    value = mkTest (testDir + ("/" + filename));
  })
  (builtins.readDir testDir)

