use assert_cmd::prelude::*;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use tempfile::{tempdir, TempDir};

fn nixseparatedebuginfod(t: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("nixseparatedebuginfod").unwrap();
    cmd.env("XDG_CACHE_HOME", t.path());
    cmd
}

fn populate_cache(t: &TempDir) {
    let mut cmd = nixseparatedebuginfod(t);
    cmd.arg("-i");
    cmd.assert().success();
}

fn gdb(exe: &Path, port: u16, commands: &str) -> String {
    let mut cmd = Command::new("gdb");
    let t = tempdir().unwrap();
    cmd.env("HOME", t.path());
    cmd.env("NIX_DEBUG_INFO_DIRS", "");
    cmd.env("DEBUGINFOD_URLS", format!("http://127.0.0.1:{port}"));
    let mut tmpfile = t.path().to_path_buf();
    tmpfile.push("gdb");
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

fn suicide(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| prctl::set_death_signal(9).map_err(std::io::Error::from_raw_os_error));
    }
}

#[test]
fn test() {
    let t = tempfile::tempdir().unwrap();

    populate_cache(&t);

    let mut cmd = Command::new("nix-build");
    cmd.arg("<nixpkgs>");
    cmd.arg("-A");
    cmd.arg("gnumake");
    cmd.arg("-o");
    let mut output = t.path().to_owned();
    output.push("result");
    cmd.arg(output.as_path());
    cmd.assert().success();

    let port: u16 = 3000 + rand::random::<u8>() as u16;
    let mut cmd = nixseparatedebuginfod(&t);
    cmd.arg("-l");
    cmd.arg(format!("127.0.0.1:{port}"));
    suicide(&mut cmd);
    let mut handle = cmd.spawn().unwrap();

    let mut exe = output;
    exe.push("bin");
    exe.push("make");
    let out = gdb(&exe, port, "start\nl");
    assert!(dbg!(out).contains("1051\tmain (int argc, char **argv)"));

    handle.kill().unwrap();
}
