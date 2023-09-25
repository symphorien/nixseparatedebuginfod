<!--
SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>

SPDX-License-Identifier: CC0-1.0
-->

# `master`
- switch to jemalloc for significantly decrease peak RSS during indexation
- use nix-store --query --valid-derivers when possible
- actually implement substituting `.drv` files
- implement support for the same API as `dwarffs` (does not provide source)

# v0.1.0
- initial release
