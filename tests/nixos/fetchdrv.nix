{ pkgs, lib, overlay }:
let
  secret-key = "key-name:/COlMSRbehSh6YSruJWjL+R0JXQUKuPEn96fIb+pLokEJUjcK/2Gv8Ai96D7JGay5gDeUTx5wdpPgNvum9YtwA==";
  public-key = "key-name:BCVI3Cv9hr/AIveg+yRmsuYA3lE8ecHaT4Db7pvWLcA=";
in
{
  name = "fetch-drv-from-cache";
  /* A binary cache with debug info, derivation, and source for sl */
  nodes.cache = { pkgs, ... }: {
    services.nix-serve = {
      enable = true;
      secretKeyFile = builtins.toFile "secret-key" secret-key;
      openFirewall = true;
    };
    system.extraDependencies = [
      pkgs.stdenv
      (lib.getDev pkgs.ncurses)
      (lib.getLib pkgs.ncurses)
      pkgs.sl.src
      pkgs.bash
      pkgs.path
    ];
  };
  /* the machine where we need the debuginfo */
  nodes.machine = {
    virtualisation.useNixStoreImage = true;
    virtualisation.writableStore = true;

    services.nixseparatedebuginfod.enable = true;
    nixpkgs.overlays = [ overlay ];
    nix.settings = {
      substituters = lib.mkForce [ "http://cache:5000" ];
      trusted-public-keys = [ public-key ];
    };
    systemd.services.nixseparatedebuginfod.environment.RUST_LOG = "nixseparatedebuginfod=debug,sqlx=warn,tower=debug,info";
    environment.systemPackages = [
      pkgs.gdb
      (pkgs.writeShellScriptBin "wait_for_indexation" ''
        set -x
        while debuginfod-find debuginfo "$1"/bin/sl |& grep 'File too large'; do
          sleep 1;
        done
        echo "debuginfod-find stopped saying File too large"
      '')
    ];
    system.extraDependencies = [
      pkgs.path
    ];
  };
  testScript = /* python */ ''
    start_all()
    cache.wait_for_unit("nix-serve.service")
    cache.wait_for_open_port(5000)
    machine.wait_for_unit("nixseparatedebuginfod.service")
    machine.wait_for_open_port(1949)

    with subtest("show the config to debug the test"):
      machine.succeed("nix --extra-experimental-features nix-command show-config |& logger")
      machine.succeed("cat /etc/nix/nix.conf |& logger")
      machine.succeed("systemctl cat nixseparatedebuginfod.service |& logger")

    with subtest("populate the cache with sl and its drv file"):
      # it's important to build in the vm to avoid a situation where
      # the deriver inherited from the substituter is not the same as
      # the one evaluated from nixpkgs locally
      sl_path = cache.succeed("nix-build -E 'with import ${pkgs.path} {}; sl.overrideAttrs (_: { separateDebugInfo = true; })'").strip()
      print(f"sl has path {sl_path}")
      cache.succeed(f"nix-store --query --deriver {sl_path} |& logger --stderr |& grep /nix/store")

    with subtest("fetch sl, but not its drv file"):
      machine.succeed(f"nix-store --realise {sl_path}")

      machine.succeed(f"nix-store --query --deriver {sl_path}/bin/sl |& logger --stderr |& grep /nix/store")
      machine.succeed(f"[ ! -e $(nix-store --query --deriver {sl_path} ) ]")

    machine.succeed(f"timeout 600 wait_for_indexation {sl_path}")

    # obtaining debuginfo requires fetching the drv file from the cache
    machine.succeed(f"debuginfod-find debuginfo {sl_path}/bin/sl")

    # test that gdb can fetch source
    out = machine.succeed(f"gdb {sl_path}/bin/sl --batch -x ${builtins.toFile "commands" ''
    start
    l
    ''}")
    print(out)
    assert 'int main(' in out
  '';
}
