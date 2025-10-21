<!--
SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>

SPDX-License-Identifier: CC0-1.0
-->

# `v0.4.1`
* fix CVE-2025-61557: nixseparatedebuginfod < `0.4.1` would serve any file on the system it could read, even outside the nix store. When using the NixOS module, nixseparatedebuginfod runs as a systemd dynamic user and listens on localhost, which means that this bug only allows local users to read world-readable files, which is unlikely to be a problem. However different setups where nixseparatedebuginfod runs as a user with access to sensitive files and/or listens on the open network should upgrade. Please note that even with the fix, nixseparatedebuginfod will disclose the content of any file in the store to clients that request it. This is by design and is unlikely to ever change.

# `v0.4.0`

* fix ignoring `RUST_LOG`
* fix fetching drv files from substituters
* does not build anymore with the version of rustc shipped by NixOS 23.11

# `v0.3.4`

* fix crash on malformed ELF
* fix parsing `extra-` options in nix configuration
* module: fix using with nix 2.3
* don't emit timestamps in logs (journald does it already)

# `v0.3.3`

* handle sources inlined from another derivation (for example C++ template instantiation). Related: nixpkgs PR 279455

# `v0.3.2`

* fix version number in v0.3.1

# `v0.3.1`

* don't crash on startup with nix 2.3

# `v0.3.0`
* handle better installs with non-trivial `allowed-users` nix option
* systemd hardening

# `v0.2.0`
- switch to jemalloc for significantly decreased peak RSS during indexation
- use nix-store --query --valid-derivers when possible
- actually implement substituting `.drv` files
- implement support for the same API as `dwarffs` (does not provide source)

# v0.1.0
- initial release
