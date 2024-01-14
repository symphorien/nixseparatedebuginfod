<!--
SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>

SPDX-License-Identifier: CC0-1.0
-->

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
