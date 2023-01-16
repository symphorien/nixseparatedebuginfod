use assert_cmd::prelude::*;
use std::path::Path;
use std::process::Command;
use std::{os::unix::process::CommandExt, path::PathBuf};
use tempfile::TempDir;

fn nixseparatedebuginfod(t: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("nixseparatedebuginfod").unwrap();
    cmd.env("XDG_CACHE_HOME", t.path());
    cmd
}

/// Blocks until nixseparatedebuginfod has scanned all of the store
fn populate_cache(t: &TempDir) {
    let mut cmd = nixseparatedebuginfod(t);
    cmd.arg("-i");
    cmd.assert().success();
}

/// Spawns a nixseparatedebuginfod on a random port
///
/// returns the port and child handle. Don't forget to kill it.
fn spawn_server(t: &TempDir) -> (u16, std::process::Child) {
    let port: u16 = 3000 + rand::random::<u8>() as u16;
    let mut cmd = nixseparatedebuginfod(&t);
    cmd.arg("-l");
    cmd.arg(format!("127.0.0.1:{port}"));
    suicide(&mut cmd);
    let handle = cmd.spawn().unwrap();
    (port, handle)
}

/// Makes a PathBuf of a file in this directory
fn file_in(t: &TempDir, name: &str) -> PathBuf {
    let mut root = t.path().to_path_buf();
    root.push(name);
    root
}

// Runs gdb on this exe with these commands and configured to use debuginfod on this port
//
//  Returns its output
fn gdb(t: &TempDir, exe: &Path, port: u16, commands: &str) -> String {
    let mut cmd = Command::new("gdb");
    cmd.env("HOME", t.path());
    cmd.env("XDG_CACHE_HOME", t.path());
    cmd.env("NIX_DEBUG_INFO_DIRS", "");
    cmd.env("DEBUGINFOD_URLS", format!("http://127.0.0.1:{port}"));
    cmd.env("DEBUGINFOD_VERBOSE", "1");
    let tmpfile = file_in(&t, "gdb");
    std::fs::write(&tmpfile, commands).unwrap();
    cmd.arg(exe);
    cmd.arg("--batch");
    cmd.arg("--init-eval-command=set debuginfod verbose 10");
    cmd.arg("--init-eval-command=set debuginfod enabled on");
    cmd.arg("-n");
    cmd.arg("-x");
    cmd.arg(&tmpfile);
    let output = cmd.output().unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Marks a command to die when its parent (us) die.
fn suicide(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| prctl::set_death_signal(9).map_err(std::io::Error::from_raw_os_error));
    }
}

/// Finds a file by name in the tests folder of the repo
fn fixture(name: &str) -> PathBuf {
    let mut root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    root.push("tests");
    root.push(name);
    root
}

// runs nix-build ./tests/debugees.nix -A $attr -o $output
fn nix_build(attr: &str, output: &Path) {
    let mut cmd = Command::new("nix-build");
    cmd.arg(fixture("debugees.nix"));
    cmd.arg("-A");
    cmd.arg(attr);
    cmd.arg("-o");
    cmd.arg(output);
    cmd.assert().success();
}

#[test]
fn test() {
    let t = tempfile::tempdir().unwrap();

    // gnumake has source in tar.gz files
    let output = file_in(&t, "gnumake");
    nix_build("gnumake", &output);

    let (port, mut server) = spawn_server(&t);

    let mut exe = output;
    exe.push("bin");
    exe.push("make");
    // this is before indexation finished, and should not populate the client cache
    gdb(&t, &exe, port, "start\nl\n");

    server.kill().unwrap();

    populate_cache(&t);

    let (port, mut server) = spawn_server(&t);

    let out = gdb(&t, &exe, port, "start\nl\n");
    assert!(dbg!(out).contains("1051\tmain (int argc, char **argv)"));

    // nix has source in flat files
    let output: PathBuf = file_in(&t, "nix");
    nix_build("nix", &output);

    let mut exe = output;
    exe.push("bin");
    exe.push("nix");
    let out = gdb(&t, &exe, port, "start\nl\n");
    assert!(dbg!(out).contains("400	int main(int argc, char * * argv)"));

    server.kill().unwrap();
}
