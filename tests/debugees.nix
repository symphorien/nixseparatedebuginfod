let
  nixpkgs = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/8c54d842d95.tar.gz";
    sha256 = "sha256:14hk2lyxy8ajczn77363vw05w24fyx9889q3b89riqgs28acyz87";
  };
  pkgs = import nixpkgs { };
in
{
  inherit (pkgs)
    gnumake # has source in archive
    nix; # has source in flat files
}
