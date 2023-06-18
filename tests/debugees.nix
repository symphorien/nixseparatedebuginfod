# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: GPL-3.0-only

let
  nixpkgs = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/087416863971.tar.gz";
    sha256 = "sha256:0gw5l5bj3zcgxhp7ki1jafy6sl5nk4vr43hal94lhi15kg2vfmfy";
  };
  pkgs = import nixpkgs { };
in
{
  inherit (pkgs)
    gnumake # has source in archive
    nix # has source in flat files
    python3
    python310;
  sl = pkgs.sl.overrideAttrs (_:{ separateDebugInfo = true; });
}
