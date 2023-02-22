{
  description = "Downloads and provides debug symbols and source code for nix derivations to gdb and other debuginfod-capable debuggers as needed.";

  outputs = { ... }: {
    nixosModules.default = import ./module.nix;
  };
}
