# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

map (channel: let
  pkgs = import (builtins.fetchTarball ("channel:"+channel)) {};
  nixseparatedebuginfod = pkgs.callPackage ./nixseparatedebuginfod.nix {};
in
  nixseparatedebuginfod.overrideAttrs ({name, ...}: {
    name = name + "-" + channel;
  })
  ) [ "nixos-unstable" "nixos-23.11" ]

