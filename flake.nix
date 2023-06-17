{
  description = "Downloads and provides debug symbols and source code for nix derivations to gdb and other debuginfod-capable debuggers as needed.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    ...
  }:
  let
    packagesWith = pkgs: {
      nixseparatedebuginfod = pkgs.callPackage ./nixseparatedebuginfod.nix {};
    };
  in
  flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [self.overlays.default];
      };
    in rec {
      packages = packagesWith pkgs // {
        default = packages.nixseparatedebuginfod;
      };
      devShells.default = pkgs.callPackage ./shell.nix {};
    }
  ) // {
    nixosModules.default = import ./module.nix;
    overlays.default = final: prev: packagesWith final;
  };
}
