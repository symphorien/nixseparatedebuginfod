# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

let
  compiling_on_stable_and_unstable = map
    (channel:
      let
        pkgs = import (builtins.fetchTarball ("channel:" + channel)) { };
        nixseparatedebuginfod = pkgs.callPackage ./nixseparatedebuginfod.nix { };
      in
      nixseparatedebuginfod.overrideAttrs ({ name, ... }: {
        name = name + "-" + channel;
      })
    ) [ "nixos-unstable" "nixos-24.05" ];
  nixos_tests = builtins.attrValues (import ./tests/nixos-tests.nix { });
in
compiling_on_stable_and_unstable ++ nixos_tests



