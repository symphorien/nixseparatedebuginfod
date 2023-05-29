# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

with import <nixpkgs> {};
mkShell {
  nativeBuildInputs = [
    cargo
    rustc
    rustfmt
    rust-analyzer
    sqlite
    openssl
    (gdb.override { enableDebuginfod = true; })
    pkg-config
    reuse
    cargo-license
    cargo-outdated
  ];
  buildInputs = [ libarchive ];
  RUST_BACKTRACE="full";
}
