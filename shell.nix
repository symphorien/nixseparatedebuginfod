# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

{ pkgs ? import <nixpkgs> {} }:
with pkgs;
mkShell {
  nativeBuildInputs = [
    cargo
    rustc
    rustfmt
    rust-analyzer
    sqlite
    openssl
    pkg-config
    reuse
    cargo-license
    cargo-outdated
  ]
  ++ lib.optionals (!gdb.meta.unsupported) [gdb];
  buildInputs = [ libarchive ];
  RUST_BACKTRACE="full";
}
