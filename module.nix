# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

{ pkgs, lib, config, ... }:
let
  cfg = config.services.nixseparatedebuginfod;
  url = "127.0.0.1:${toString cfg.port}";
  maybeAdd = x: list: if builtins.elem x list then list else list ++ [ x ];
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
    };
  };
  config = lib.mkIf cfg.enable {
    systemd.sockets.nixseparatedebuginfod = {
      listenStreams = [ url ];
      wantedBy = [ "sockets.target" ];
    };
    systemd.services.nixseparatedebuginfod = {
      wants = [ "nix-daemon.service" ];
      after = [ "nix-daemon.service" ];
      requires = [ "nixseparatedebuginfod.socket" ];
      path = [ config.nix.package ];
      serviceConfig = {
        DynamicUser = true;
        ExecStart = [ "${pkgs.nixseparatedebuginfod}/bin/nixseparatedebuginfod --socket-activated" ];
        Restart = "on-failure";
        ProtectHome = "yes";
        ProtectSystem = "strict";
        CacheDirectory = "nixseparatedebuginfod";
      };
    };

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
