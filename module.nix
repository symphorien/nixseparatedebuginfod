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
        CacheDirectory = "nixseparatedebuginfod";
        # nix does not like DynamicUsers in allowed-users
        User = "nixseparatedebuginfod";
        Group = "nixseparatedebuginfod";

        # hardening
        # Filesystem stuff
        ProtectSystem = "strict"; # Prevent writing to most of /
        ProtectHome = true; # Prevent accessing /home and /root
        PrivateTmp = true; # Give an own directory under /tmp
        PrivateDevices = true; # Deny access to most of /dev
        ProtectKernelTunables = true; # Protect some parts of /sys
        ProtectControlGroups = true; # Remount cgroups read-only
        RestrictSUIDSGID = true; # Prevent creating SETUID/SETGID files
        PrivateMounts = true; # Give an own mount namespace
        RemoveIPC = true;
        UMask = "0077";

        # Capabilities
        CapabilityBoundingSet = ""; # Allow no capabilities at all
        NoNewPrivileges = true; # Disallow getting more capabilities. This is also implied by other options.

        # Kernel stuff
        ProtectKernelModules = true; # Prevent loading of kernel modules
        SystemCallArchitectures = "native"; # Usually no need to disable this
        ProtectKernelLogs = true; # Prevent access to kernel logs
        ProtectClock = true; # Prevent setting the RTC

        # Networking
        RestrictAddressFamilies = "AF_UNIX AF_INET AF_INET6";

        # Misc
        LockPersonality = true; # Prevent change of the personality
        ProtectHostname = true; # Give an own UTS namespace
        RestrictRealtime = true; # Prevent switching to RT scheduling
        MemoryDenyWriteExecute = true; # Maybe disable this for interpreters like python
        RestrictNamespaces = true;
      };
    };

    users.users.nixseparatedebuginfod = {
      isSystemUser = true;
      group = "nixseparatedebuginfod";
    };

    users.groups.nixseparatedebuginfod = { };

    # extra- settings were introduced in nix 2.4
    # sorry for those who use 2.3
    nix.settings = lib.optionalAttrs (lib.versionAtLeast config.nix.package.version "2.4") {
      extra-allowed-users = [ "nixseparatedebuginfod" ];
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
