# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

{ pkgs, lib, config, ... }:
let
  cfg = config.services.nixseparatedebuginfod;
  url = "127.0.0.1:${toString cfg.port}";
  maybeAdd = x: list: if builtins.elem x list then list else list ++ [ x ];
  recentNix = lib.lists.findFirst
    (nix: nix != null && lib.versionAtLeast
      nix.version "2.18")
    config.nix.package [
    config.nix.package
    pkgs.nix
    ((pkgs.nixVersions or { }).nix_2_18 or null)
    pkgs.nixUnstable
  ];
in
{
  imports = [
    (lib.mkRemovedOptionModule [ "services" "nixseparatedebuginfod" "allowUser" ] "this option is not necessary anymore")
  ];

  options = {
    services.nixseparatedebuginfod = {
      enable = lib.mkEnableOption "separatedebuginfod, a debuginfod server providing source and debuginfo for nix packages";
      port = lib.mkOption {
        description = "port to listen";
        default = 1949;
        type = lib.types.port;
      };
    };
  };
  config = lib.mkIf cfg.enable {
    systemd.services.nixseparatedebuginfod = {
      wantedBy = [ "multi-user.target" ];
      wants = [ "nix-daemon.service" ];
      after = [ "nix-daemon.service" ];
      path = [ recentNix ];
      serviceConfig = {
        ExecStart = [ "${pkgs.nixseparatedebuginfod}/bin/nixseparatedebuginfod -l ${url}" ];
        Restart = "on-failure";
        ProtectHome = "yes";
        ProtectSystem = "strict";
        CacheDirectory = "nixseparatedebuginfod";
        PrivateTmp = true;
        # nix does not like DynamicUsers in allowed-users
        User = "nixseparatedebuginfod";
        Group = "nixseparatedebuginfod";
      };
    };

    users.users.nixseparatedebuginfod = {
      isSystemUser = true;
      group = "nixseparatedebuginfod";
    };

    users.groups.nixseparatedebuginfod = { };

    nix.settings.extra-allowed-users = [ "nixseparatedebuginfod" ];

    environment.variables.DEBUGINFOD_URLS = "http://${url}";

    nixpkgs.overlays = [
      (self: super: {
        nixseparatedebuginfod = super.callPackage ./nixseparatedebuginfod.nix { };
        gdb-debuginfod = (super.gdb.override { enableDebuginfod = true; }).overrideAttrs (old: {
          configureFlags = maybeAdd "--with-system-gdbinit-dir=/etc/gdb/gdbinit.d" old.configureFlags;
        });
      })
    ];

    environment.systemPackages = [
      (lib.hiPrio pkgs.gdb-debuginfod)
      # valgrind support requires debuginfod-find on PATH
      (lib.hiPrio (lib.getBin (pkgs.elfutils.override { enableDebuginfod = true; })))
    ];

    environment.etc."gdb/gdbinit.d/nixseparatedebuginfod.gdb".text = "set debuginfod enabled on";

  };
}
