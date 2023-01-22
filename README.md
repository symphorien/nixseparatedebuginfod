<!--
SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>

SPDX-License-Identifier: CC0-1.0
-->

# `nixseparatedebuginfod`

Downloads and provides debug symbols and source code for nix derivations to `gdb` and other `debuginfod`-capable debuggers as needed.

## Overview

Most software in `nixpkgs` is stripped, so hard to debug. But some key packages are built with `separateDebugInfo = true`: debug symbols are put in a separate output `debug` which is not downloaded by default (and that's for the best, debug symbols can be huge). But when you do need the debug symbols, for example for `gdb`, you need to download this `debug` output and point `gdb` to it. This can be done manually, but is quite cumbersome. `nixseparatedebuginfod` does that for you on the fly, for separate debug outputs and even for the source!

## Setup

### On NixOS

A NixOS module is provided for your convenience in `./module.nix`.
It provides a version of `gdb` compiled with `debuginfod` support, so you should uninstall `gdb` from other source (`nix-env`, `home-manager`).
As the module sets an environment variable, you need to log out/lo gin again or reboot for it to work.


### Manual installation

- Compile `nixseparatedebuginfod`
  - `nix-build ./default.nix`, or
  - `cargo build --release`, inside the provided `nix-shell` (which provides `libarchive` and `pkg-config`).
- Run `nixseparatedebuginfod`.
- Set the environment variable `DEBUGINFOD_URLS` to `http://127.0.0.1:1949`

Software with `debuginfod` support should now use `nixseparatedebuginfod`. Unfortunately `gdb` is not compiled with `debuginfod` support in `nixpkgs` by default, so some additional steps are needed:
- In `/etc/nixos/configuration.nix` or `~/.config/nixpkgs/home.nix` replace the `pkgs.gdb` entry in `home.packages` or `environment.systemPackages` by `(gdb.override { enableDebuginfod = true })`. Don't use an overlay, as `gdb` is a mass rebuild.
- In `~/.gdbinit` put
```
set `debuginfod` enabled on
```
otherwise, it will ask for confirmation every time.

## Limitations
- Only provides debug symbols for derivations built with `separateDebugInfo` set to `true`, obviously.
- Only finds the debug outputs of store paths with `.drv` file present on the system or substitutable. This should be the case in most cases, but not if you `nix-store --realise` something manually from hydra.
- If a derivation patches a source file before compiling it, `nixseparatedebuginfod` will serve the unpatched source file straight from the `src` attribute of the derivation.
- The `section` endpoint of the `debuginfod` protocol is not implemented. (If you know of some client that uses it, tell me).

## Comparison to other ways to provide debug symbols
- the `environment.enableDebugInfo = true;` NixOS option only provides debug symbols for software installed in `environment.systemPackages`, but not inner libraries. As a result you will get debug symbols for `qemu`, but not for the `glibc` it uses. It also downloads debug symbols even if you end up not using them, and `qemu` debug symbols take very long to download...
- [`dwarffs`](https://github.com/edolstra/dwarffs) downloads debug symbols on the fly from a custom API provided by hydra. You won't get debug symbols for derivations compiled locally or on a custom binary cache. It also does not point `gdb` to the right place to find source files.
- `nixseparatedebuginfod` supports all binary caches because it just uses the `nix-store` command line tool. It can serve sources files as well (see the section about limitations, though).

## Notes

An indexation step is needed on first startup, and then periodically. It happens automatically but can take a few minutes. A cache is stored somewhere in `~/.cache/nixseparatedebuginfod`, and currently this cache can only grow. You can safely remove it, it will be recreated on next startup.

The `debuginfod` client provided by `elfutils` (used in `gdb`) caches `debuginfod` misses, and the only way to prevent this is to return `406 File too big`. If `gdb` requests something during initial indexation you will see spurious complaints about `File too big`. You can ignore them, and retry later is debug symbols are missing.
(For development, it is useful to disable this cache altogether:
write 0 to `~/.cache/debuginfod_client/cache_miss_s` and `~/.cache/debuginfod_client/max_unused_age_s` and `~/.cache/debuginfod_client/cache_clean_interval_s`.)

To make `nixseparatedebuginfod` less verbose, export `RUST_LOG=warn` or `RUST_LOG=error`.

## References
Protocol: <https://www.mankier.com/8/debuginfod#Webapi>
Client cache: <https://www.mankier.com/7/debuginfod-client-config#Cache>
