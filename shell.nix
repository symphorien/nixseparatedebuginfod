with import <nixpkgs> {};
mkShell {
  nativeBuildInputs = [
    cargo
    rustc
    rustfmt
    rust-analyzer
    sqlite
    (gdb.override { enableDebuginfod = true; })
    pkg-config
  ];
  buildInputs = [ libarchive ];
  RUST_BACKTRACE="full";
}
