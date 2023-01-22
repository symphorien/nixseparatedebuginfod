# SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
#
# SPDX-License-Identifier: CC0-1.0

{ pkgs, lib, config, ... }:
let
  cfg = config.services.nixseparatedebuginfod;
  url = "127.0.0.1:${toString cfg.port}";
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
    systemd.services.nixseparatedebuginfod = {
      wantedBy = [ "multi-user.target" ];
      wants = [ "nix-daemon.service" ];
      after = [ "nix-daemon.service" ];
      path = [ config.nix.package ];
      serviceConfig = {
        DynamicUser = true;
        ExecStart = [ "${pkgs.nixseparatedebuginfod}/bin/nixseparatedebuginfod -l ${url}" ];
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
          configureFlags = old.configureFlags ++ [ "--with-system-gdbinit=/etc/gdbinit" ];
        });
      })
    ];

    environment.systemPackages = [
      (lib.hiPrio pkgs.gdb-debuginfod)
    ];

    environment.etc.gdbinit.text = "set debuginfod enabled on";

  };
}
