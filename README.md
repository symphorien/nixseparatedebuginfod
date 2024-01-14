<!--
SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>

SPDX-License-Identifier: CC0-1.0
-->

# `nixseparatedebuginfod`

Downloads and provides debug symbols and source code for nix derivations to `gdb` and other `debuginfod`-capable debuggers as needed.

## Overview

Most software in `nixpkgs` is stripped, so hard to debug. But some key packages are built with `separateDebugInfo = true`: debug symbols are put in a separate output `debug` which is not downloaded by default (and that's for the best, debug symbols can be huge). But when you do need the debug symbols, for example for `gdb`, you need to download this `debug` output and point `gdb` to it. This can be done manually, but is quite cumbersome. `nixseparatedebuginfod` does that for you on the fly, for separate debug outputs and even for the source!

## Setup

[![Packaging status](https://repology.org/badge/vertical-allrepos/nixseparatedebuginfod.svg)](https://repology.org/project/nixseparatedebuginfod/versions)

### On NixOS

A NixOS module is provided for your convenience:
- directly upstream in NixOS &ge; 24.05
- in `./module.nix` in this repo for older versions of NixOS.

The module provides the following main option:
```nix
services.nixseparatedebuginfod.enable = true;
```

On NixOS &lt; 23.05, this option installs a version of `gdb` compiled with `debuginfod` support, so you should uninstall `gdb` from other sources (`nix-env`, `home-manager`).
As the module sets an environment variable, you need to log out/log in again or reboot for it to work.

#### NixOS &ge; 24.05

Modify `/etc/nixos/configuration.nix` as follows:

```nix
{config, pkgs, lib, ...}: {
  config = {
    /*
    ... existing options ...
    */
    services.nixseparatedebuginfod.enable = true;
  };
}
```

#### Pure stable nix, NixOS &lt; 24.05

Add the module to the `imports` section of `/etc/nixos/configuration.nix`:
```nix
{config, pkgs, lib, ...}: {
  imports = [
    ((builtins.fetchTarball {
      url = "https://github.com/symphorien/nixseparatedebuginfod/archive/9b7a087a98095c26d1ad42a05102e0edfeb49b59.tar.gz";
      sha256 = "sha256:1jbkv9mg11bcx3gg13m9d1jmg4vim7prny7bqsvlx9f78142qrlw";
    }) + "/module.nix")
  ];
  config = {
    services.nixseparatedebuginfod.enable = true;
  };
}
```
(adapt the revision and sha256 to a recent one).

#### With [niv](https://github.com/nmattia/niv); NixOS &lt; 24.05

Run `niv add github symphorien/nixseparatedebuginfod` and add to the `imports` section of `/etc/nixos/configuration.nix`:
```nix
{config, pkgs, lib, ...}: {
  imports = [
    ((import nix/sources.nix {}).nixseparatedebuginfod + "/module.nix")
  ];
  config = {
    services.nixseparatedebuginfod.enable = true;
  };
}
```

#### With flakes; NixOS &lt 24.05

If you use flakes, modify your `/etc/nixos/flake.nix` as in this example:

```nix
{
    inputs = {
        # ...
        nixseparatedebuginfod.url = "github:symphorien/nixseparatedebuginfod";
    };
    outputs = {
        nixpkgs,
        # ...
        nixseparatedebuginfod
    }: {
      nixosConfigurations.XXXXX = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
            # ...
            nixseparatedebuginfod.nixosModules.default
        ];
        config = {
          services.nixseparatedebuginfod.enable = true;
        };
      };
    };
}
```

### Manual installation without the module

If you cannot use the provided NixOS module, here are steps to set up `nixseparatedebuginfod` manually.

- Compile `nixseparatedebuginfod`
  - `nix-build ./default.nix`, or
  - `cargo build --release`, inside the provided `nix-shell` (which provides `libarchive` and `pkg-config`).
- Run `nixseparatedebuginfod`.
- Set the environment variable `DEBUGINFOD_URLS` to `http://127.0.0.1:1949`

Most software with `debuginfod` support should now use `nixseparatedebuginfod`. Some software needs to be configured further:

#### `gdb`
- In `~/.gdbinit` put
```
set debuginfod enabled on
```
otherwise, it will ask for confirmation every time.
- With `nixpkgs` 22.11 or earlier, `gdb` is not compiled with `debuginfod` support in `nixpkgs` by default. To install a suitable version of `gdb`, replace the `pkgs.gdb` entry in `home.packages` or `environment.systemPackages` by `(gdb.override { enableDebuginfod = true })` in `/etc/nixos/configuration.nix` or `~/.config/nixpkgs/home.nix`. Don't use an overlay, as `gdb` is a mass rebuild.

#### `valgrind`

`valgrind` needs `debuginfod-find` on `$PATH` to use `nixseparatedebuginfod`.
Add `(lib.getBin (pkgs.elfutils.override { enableDebuginfod = true; }))` to
`environment.systemPackages` or `home.packages`.

### Check that it works

`nix` is built with `separateDebugInfo`.
```commands
$  gdb $(command -v nix)
GNU gdb (GDB) 12.1
Copyright (C) 2022 Free Software Foundation, Inc.
License GPLv3+: GNU GPL version 3 or later <http://gnu.org/licenses/gpl.html>
This is free software: you are free to change and redistribute it.
There is NO WARRANTY, to the extent permitted by law.
Type "show copying" and "show warranty" for details.
This GDB was configured as "x86_64-unknown-linux-gnu".
Type "show configuration" for configuration details.
For bug reporting instructions, please see:
<https://www.gnu.org/software/gdb/bugs/>.
Find the GDB manual and other documentation resources online at:
    <http://www.gnu.org/software/gdb/documentation/>.

For help, type "help".
Type "apropos word" to search for commands related to "word"...
Reading symbols from /run/current-system/sw/bin/nix...
Downloading 22.56 MB separate debug info for /run/current-system/sw/bin/nix
Reading symbols from /home/symphorien/.cache/debuginfod_client/6d059e78eed4c79126bc9be93c612f9149a8deea/debuginfo...
(gdb) start
Downloading 0.01 MB source file /build/source/src/nix/main.cc
Temporary breakpoint 1 at 0x97670: file src/nix/main.cc, line 401.
Starting program: /nix/store/hfnwjdjm5h45pm64414hv2fh4kcx76zi-system-path/bin/nix
Downloading 0.70 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/ld-linux-x86-64.so.2
Downloading 0.77 MB separate debug info for /nix/store/adyrxq2jq1jq3116jxhbcsb13wi6nzpq-libsodium-1.0.18/lib/libsodium.so.23
Downloading 14.16 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixexpr.so
Downloading 0.23 MB separate debug info for /nix/store/688nq54x7kks8y7dn52nwsklnww19fxa-boehm-gc-8.2.2/lib/libgc.so.1
Downloading 0.01 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libpthread.so.0
Downloading 0.01 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libdl.so.2
Downloading 1.27 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixmain.so
Downloading 4.97 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixfetchers.so
Downloading 21.73 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixstore.so
Downloading 5.69 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixutil.so
Downloading 3.83 MB separate debug info for /nix/store/xb66g3x4iv7m95mja9zzi1ghhraxmpws-nix-2.11.1/lib/libnixcmd.so
Downloading 0.89 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libm.so.6
Downloading 5.36 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libc.so.6
[Thread debugging using libthread_db enabled]
Using host libthread_db library "/nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libthread_db.so.1".
Downloading 5.27 MB separate debug info for /nix/store/bprhh8afhvz27b051y8j451fyp6mkk38-openssl-3.0.7/lib/libcrypto.so.3
Downloading 2.25 MB separate debug info for /nix/store/brmjip0wviknyi75bqddyda45m1rnw2i-sqlite-3.39.4/lib/libsqlite3.so.0
Downloading 1.73 MB separate debug info for /nix/store/xryxkg022p5vnlyyyx58csbmfc7ydsdp-curl-7.86.0/lib/libcurl.so.4
Downloading 0.01 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/librt.so.1
Downloading 1.07 MB separate debug info for /nix/store/bprhh8afhvz27b051y8j451fyp6mkk38-openssl-3.0.7/lib/libssl.so.3
Downloading 0.11 MB separate debug info for /nix/store/9xfad3b5z4y00mzmk2wnn4900q0qmxns-glibc-2.35-224/lib/libresolv.so.2

Temporary breakpoint 1, main (BFD: reopening /home/symphorien/.cache/debuginfod_client/3d6884d200ead572b7b89a4133f645c7a3c039ed/debuginfo: No such file or directory

BFD: reopening /home/symphorien/.cache/debuginfod_client/3d6884d200ead572b7b89a4133f645c7a3c039ed/debuginfo: No such file or directory

BFD: reopening /home/symphorien/.cache/debuginfod_client/3d6884d200ead572b7b89a4133f645c7a3c039ed/debuginfo: No such file or directory

warning: Can't read data for section '.debug_loc' in file '/home/symphorien/.cache/debuginfod_client/3d6884d200ead572b7b89a4133f645c7a3c039ed/debuginfo'
argc=1, argv=0x7fffffffd128) at src/nix/main.cc:401
Downloading 0.01 MB source file /build/source/src/nix/main.cc
401     {
(gdb) l
396	}
397
398	}
399
400	int main(int argc, char * * argv)
401	{
402	    // Increase the default stack size for the evaluator and for
403	    // libstdc++'s std::regex.
404	    nix::setStackSize(64 * 1024 * 1024);
405
(gdb)
```

## Limitations
- `nixseparatedebuginfod` only provides debug symbols for derivations built with `separateDebugInfo` set to `true`, obviously.
- GDB only queries source files to `debuginfod` servers if the debug symbols were also provided via `debuginfod`, so `nixseparatedebuginfod` does not provide source for store paths with non-separate debug symbols (e.g. produced with `enableDebugging`).
- `nixseparatedebuginfod` only finds the debug outputs of store paths if either a binary cache has indexed it (the same technique as `dwarffs`) or the `.drv` file is present on the system or substitutable. This should cover most cases, however.
- Source fetching does not work when only the `dwarffs` can be used.
- If a derivation patches a source file before compiling it, `nixseparatedebuginfod` will serve the unpatched source file straight from the `src` attribute of the derivation.
- The `section` endpoint of the `debuginfod` protocol is not implemented. (If you know of some client that uses it, tell me).
- Nix &gt;= 2.18 is required to fetch sources successfully in some situations (notably
when the program was fetched from hydra long after it was built).
- Software compiled with the `stdenv` of NixOS 23.11 has mangled debug symbols where the store path of the source of in-lined functions/template instantiations is replaced by `/nix/store/eeeeee...`. These source files will not be fetched by `nixseparatedebuginfod`. The issue will be fixed in NixOS 24.05.

## Comparison to other ways to provide debug symbols
- the `environment.enableDebugInfo = true;` NixOS option only provides debug symbols for software installed in `environment.systemPackages`, but not inner libraries. As a result you will get debug symbols for `qemu`, but not for the `glibc` it uses. It also downloads debug symbols even if you end up not using them, and `qemu` debug symbols take very long to download...
- [`dwarffs`](https://github.com/edolstra/dwarffs) downloads debug symbols on the fly from a custom API provided by hydra. You won't get debug symbols for derivations compiled locally or on a custom binary cache. It also does not point `gdb` to the right place to find source files.
- `nixseparatedebuginfod` supports all binary caches because it just uses the `nix-store` command line tool. It can serve sources files as well (see the section about limitations, though). This relies on `.drv` files being present or substitutable, but when this is not the case `nixseparatedebuginfod` can fall back to the same mechanism as `dwarffs` (no source).

## Security

Normal operation uses `nix-*` commands and is subject to the normal nix control of substituter trust and NAR signing. However, anything that can connect to `nixseparatedebuginfod` gets some of the privilege of `nixseparatedebuginfod`: if you prohibit some users from using nix with the `allowed-users` option, these users can use `nixseparatedebuginfod` to
- add files from binary caches into your store,
- build existing `.drv` files, but not create new ones.
When the `.drv` file of a store path is not found, `nixseparatedebuginfod` will fall back to same API as `dwarffs`. It serves NARs with debug symbols without signatures. This means that `nixseparatedebuginfod` may add NARs from any `file`, `http` and `https` substituters (trusted or not) in the output of `nix show-config` to your store without checking signatures.

## Notes

An indexation step is needed on first startup, and then periodically. It happens automatically but can take a few minutes. A cache is stored somewhere in `~/.cache/nixseparatedebuginfod`, and currently this cache can only grow. You can safely remove it, it will be recreated on next startup.

The `debuginfod` client provided by `elfutils` (used in `gdb`) caches `debuginfod` misses, and the only way to prevent this is to return `406 File too big`. If `gdb` requests something during initial indexation you will see spurious complaints about `File too big`. You can ignore them, and retry later is debug symbols are missing.
(For development, it is useful to disable this cache altogether:
write 0 to `~/.cache/debuginfod_client/cache_miss_s` and `~/.cache/debuginfod_client/max_unused_age_s` and `~/.cache/debuginfod_client/cache_clean_interval_s`. However, this breaks `gdb` back traces in weird ways.)

To make `nixseparatedebuginfod` less verbose, export `RUST_LOG=warn` or `RUST_LOG=error`.

## Troubleshooting

If you do not use the provided NixOS module and `nixseparatedebuginfod` fails to start because the nix daemon resets the connection like this:
```
2023-09-25T21:48:52.750 5006851216 nix-daemon.service nix-daemon[216134] INFO error: error processing connection: user 'nixseparatedebuginfod' is not allowed to connect to the Nix daemon
2023-09-25T21:48:52.752 5006852674 nixseparatedebuginfod.service nixseparatedebuginfod[216414] INFO 2023-09-25T19:48:52.752187Z ERROR nixseparatedebuginfod: nix is not available: checking nix install by getting deriver of /nix/store/9krlzvny65gdc8s7kpb6lkx8cd02c25b-default-builder.sh: "nix-store" "--query" "--deriver" "/nix/store/9krlzvny65gdc8s7kpb6lkx8cd02c25b-default-builder.sh" failed: error: cannot open connection to remote store 'daemon': error: reading from file: Connection reset by peer
```
then you probably have restricted access to the daemon to specific users, for example
with `nix.settings.allowed-users = [ "@somegroup" ];`. Add the user `nixseparatedebuginfod` runs as
to this list. You can check that the setting had effect with `nix show-config`.

## References
Protocol: <https://www.mankier.com/8/debuginfod#Webapi>
Client cache: <https://www.mankier.com/7/debuginfod-client-config#Cache>
