with import <nixpkgs> {};
mkShell {
  nativeBuildInputs = [
    cargo
    rustc
    rustfmt
    rust-analyzer
    sqlite
    (gdb.override { enableDebuginfod = true; })
  ];
  RUST_BACKTRACE="full";
}
