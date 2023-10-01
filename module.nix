# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

{ pkgs, lib, config, ... }:
let
  cfg = config.services.nixseparatedebuginfod;
  url = "127.0.0.1:${toString cfg.port}";
  maybeAdd = x: list: if builtins.elem x list then list else list ++ [ x ];
  recentNix = lib.lists.findFirst (nix: nix != null && lib.versionAtLeast
  nix.version "2.18") config.nix.package [
    config.nix.package
    pkgs.nix
    ((pkgs.nixVersions or {}).nix_2_18 or null)
    pkgs.nixUnstable
  ];
in
{
  options = {
    services.nixseparatedebuginfod = {
      enable = lib.mkEnableOption "separatedebuginfod, a debuginfod server providing source and debuginfo for nix packages";
      port = lib.mkOption {
        description = "port to listen";
        default = 1949;
        type = lib.types.port;
      };
      allowUser = lib.mkOption {
        description = "set this option to true when you set the nix configuration option `allowed-users` to something else than `*`.";
        default = false;
        type = lib.types.bool;
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
        # nix does not like DynamicUsers in allowed-users
        DynamicUser = !cfg.allowUser;
        ExecStart = [ "${pkgs.nixseparatedebuginfod}/bin/nixseparatedebuginfod -l ${url}" ];
        Restart = "on-failure";
        ProtectHome = "yes";
        ProtectSystem = "strict";
        CacheDirectory = "nixseparatedebuginfod";
        PrivateTmp = true;
      } // lib.optionalAttrs cfg.allowUser {
        User = "nixseparatedebuginfod";
        Group = "nixseparatedebuginfod";
      };
    };

    users.users = lib.mkIf cfg.allowUser {
      nixseparatedebuginfod = {
        isSystemUser = true;
        group = "nixseparatedebuginfod";
      };
    };
    users.groups = lib.mkIf cfg.allowUser {
      nixseparatedebuginfod = { };
    };

    # unfortunately we cannot do that unconditionally, because that would
    # overwrite the default of *. Hence the indirection through cfg.allowUser
    # and the assertion.
    nix.settings = lib.mkIf cfg.allowUser {
      allowed-users = [ "nixseparatedebuginfod" ];
    };

    assertions = [{
      assertion = cfg.allowUser == !(lib.lists.all (elt: elt == "*" || elt == "nixseparatedebuginfod") (config.nix.settings.allowed-users or [ ]));
      message = "nix.settings.allowed-users is ${if cfg.allowUser then "unset" else "set"}. you must set services.nixseparatedebuginfod.allowUser to ${if cfg.allowUser then "false" else "true"}.";
    }];

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
