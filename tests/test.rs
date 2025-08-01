// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

use assert_cmd::prelude::*;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
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
    dbg!(cmd).assert().success();
}

fn wait_for_port(port: u16) {
    while let Err(e) = reqwest::blocking::get(&format!("http://127.0.0.1:{port}")) {
        println!("port {} is not open yet: {:#}", port, e);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Spawns a nixseparatedebuginfod on a random port
///
/// returns the port and child handle. Don't forget to kill it.
///
/// If substituters is not None, hacks the environment so that nix show-config
/// lists those substituters. This hack should be ignore by actual nix-store commands
/// as these go through the daemon
fn spawn_server(t: &TempDir, substituters: Option<Vec<&str>>) -> (u16, std::process::Child) {
    let port: u16 = 3000 + rand::random::<u8>() as u16;
    let mut cmd = nixseparatedebuginfod(t);
    cmd.arg("-l");
    cmd.arg(format!("127.0.0.1:{port}"));
    suicide(&mut cmd);
    if let Some(substituters) = substituters {
        let nix_conf = file_in(t, "nix.conf");
        let substituters_as_str = &substituters.join(" ");
        std::fs::write(nix_conf, format!("substituters = {substituters_as_str}\ntrusted-substituters = {substituters_as_str}")).unwrap();
        cmd.env("NIX_CONF_DIR", t.path().display().to_string());
    }
    cmd.env(
        "RUST_LOG",
        "nixseparatedebuginfod=debug,actix=info,sqlx=warn,warn",
    );
    let handle = dbg!(cmd).spawn().unwrap();
    wait_for_port(port);
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
    let tmpfile = file_in(t, "gdb");
    std::fs::write(&tmpfile, commands).unwrap();
    cmd.arg(exe);
    cmd.arg("--batch");
    cmd.arg("--init-eval-command=set debuginfod verbose 10");
    cmd.arg("--init-eval-command=set debuginfod enabled on");
    cmd.arg("-n");
    cmd.arg("-x");
    cmd.arg(&tmpfile);
    let output = dbg!(cmd).output().unwrap();
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
    let mut root = std::env::current_dir().unwrap();
    root.push("tests");
    root.push(name);
    root
}

/// runs nix-build ./tests/debugees.nix -A $attr -o $output --store $store
fn nix_build(attr: &str, output: &Path, store: Option<impl AsRef<Path>>) {
    let mut cmd = Command::new("nix-build");
    cmd.arg(fixture("debugees.nix"));
    cmd.arg("-A");
    cmd.arg(attr);
    cmd.arg("-o");
    cmd.arg(output);
    if let Some(store) = store {
        cmd.arg("--store");
        cmd.arg(store.as_ref());
    }
    dbg!(cmd).assert().success();
}

/// runs nix-instantiate ./tests/debugees.nix -A $attr
fn nix_instantiate(attr: &str) -> PathBuf {
    let mut cmd = Command::new("nix-instantiate");
    cmd.arg(fixture("debugees.nix"));
    cmd.arg("-A");
    cmd.arg(attr);
    let output = dbg!(cmd).output().unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(dbg!(out.trim_matches(&['"', '\n'] as &[_])));
    assert!(path.is_absolute());
    path
}

/// runs nix copy --from ... --to ... --store ... path
fn nix_copy(
    from: Option<impl AsRef<Path>>,
    to: Option<impl AsRef<Path>>,
    path: &Path,
    store: Option<impl AsRef<Path>>,
) {
    let mut cmd = Command::new("nix");
    cmd.arg("copy");
    cmd.args([
        "--extra-experimental-features",
        "nix-command",
        "--extra-experimental-features",
        "flakes",
    ]);
    if let Some(from) = from {
        cmd.arg("--from");
        cmd.arg(from.as_ref());
    }
    if let Some(to) = to {
        cmd.arg("--to");
        cmd.arg(to.as_ref());
    }
    if let Some(store) = store {
        cmd.arg("--store");
        cmd.arg(store.as_ref());
    }
    cmd.arg(path);
    dbg!(cmd).assert().success();
}

fn delete_path(storepath: impl AsRef<Path>) {
    let path = storepath.as_ref();

    if path.exists() {
        // using xargs is a hack https://github.com/NixOS/nix/issues/6141#issuecomment-1476807193
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(path.as_os_str().as_bytes()).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut cmd = Command::new("xargs");
        cmd.arg("--verbose");
        cmd.arg("nix-store")
            .arg("--delete")
            .arg("--option")
            .arg("auto-optimise-store")
            .arg("false");
        cmd.stdin(file);
        cmd.stdout(Stdio::inherit());
        dbg!(cmd).assert().success();

        assert!(!path.exists());
        println!("removed {}", path.display());
    }
}

fn remove_drv_and_outputs(attr: &str) {
    // get drv path
    let drv_path = nix_eval_drv_path(attr);

    if !drv_path.exists() {
        return;
    }

    // get all outputs
    let mut cmd = Command::new("nix-store");
    cmd.arg("--query").arg("--outputs").arg(&drv_path);
    let out = dbg!(cmd).output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        let path = Path::new(line);
        assert!(path.is_absolute());
        // now remove the output
        delete_path(path);
    }

    delete_path(&drv_path);
}

/// obtains the store path of an output without creating the .drv file
fn nix_eval_out_path(attr: &str, output: &str) -> PathBuf {
    let mut cmd = Command::new("nix-instantiate");
    cmd.arg("--eval").arg("-E").arg(format!(
        "(import {}).{}.{}.outPath",
        fixture("debugees.nix").display(),
        attr,
        output,
    ));
    let out = dbg!(cmd).output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    let path = PathBuf::from(dbg!(out.trim_matches(&['"', '\n'] as &[_])));
    assert!(path.is_absolute());
    path
}

/// like nix_instantiate but does not create a .drv file
fn nix_eval_drv_path(attr: &str) -> PathBuf {
    let mut cmd = Command::new("nix-instantiate");
    cmd.arg("--eval").arg("-E").arg(format!(
        "(import {}).{}.drvPath",
        fixture("debugees.nix").display(),
        attr,
    ));
    let out = dbg!(cmd).output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    let path = PathBuf::from(dbg!(out.trim_matches(&['"', '\n'] as &[_])));
    assert!(path.is_absolute());
    path
}

fn realise_storepath(storepath: impl AsRef<Path>) {
    let mut cmd = Command::new("nix-store");
    cmd.arg("--realise").arg(storepath.as_ref());
    dbg!(cmd).assert().success();
}

fn remove_debug_output(attr: &str) {
    let path = nix_eval_out_path(attr, "debug");
    delete_path(path);
}

fn remove_debuginfo_for_buildid(buildid: &str) {
    let segment = format!(
        "lib/debug/.build-id/{}/{}.debug",
        &buildid[..2],
        &buildid[2..]
    );
    for entry in std::fs::read_dir("/nix/store").unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().join(&segment);
        delete_path(&path);
    }
}

#[test]
fn test_normal() {
    let t = tempfile::tempdir().unwrap();

    // gnumake has source in tar.gz files
    let output = file_in(&t, "gnumake");
    nix_build("gnumake", &output, None::<PathBuf>);

    remove_debug_output("gnumake");

    let (port, mut server) = spawn_server(&t, Some(vec![]));

    let mut exe = output;
    exe.push("bin");
    exe.push("make");
    // this is before indexation finished, and should not populate the client cache
    gdb(&t, &exe, port, "start\nl\n");

    server.kill().unwrap();

    populate_cache(&t);

    let (port, mut server) = spawn_server(&t, Some(vec![]));

    let out = gdb(&t, &exe, port, "start\nl\n");
    assert!(dbg!(out).contains("1051\tmain (int argc, char **argv)"));

    // nix has source in flat files
    let output: PathBuf = file_in(&t, "nix");
    nix_build("nix", &output, None::<PathBuf>);
    // we will test fetching the source of a template instantiation from a header-only library
    // disabled until nix bin output drops its reference to nlohmann_json
    /*
    let nlohmann_json = nix_eval_out_path("nlohmann_json", "out");
    delete_path(&nlohmann_json);
    */

    let mut exe = output;
    exe.push("bin");
    exe.push("nix");
    // test that gdb can show the source of main (in nix)
    // and the source of an inlined template (from nlohmann_json)
    let out = gdb(
        &t,
        &exe,
        port,
        "start\nl\nn\nl\nl nlohmann::detail::output_stream_adapter<char>::write_character(char)\n",
    );
    // gdb fetched the source of main()
    assert!(dbg!(&out).contains("389\tint main(int argc, char * * argv)"));
    // and fetched the source of a template instantiation in a header-only lib
    assert!(out.contains("73\t    void write_character(CharType c) override"));

    server.kill().unwrap();
}

#[test]
fn test_invalid_deriver() {
    let t = tempfile::tempdir().unwrap();

    // check that nix supports --query --valid-derivers
    let mut cmd = Command::new("nix-store");
    cmd.arg("--query").arg("--valid-derivers");
    if !dbg!(cmd.status()).unwrap().success() {
        println!("skipping test_invalid_deriver, nix is too old");
        return;
    }

    remove_drv_and_outputs("mailutils_drvhash1");
    remove_drv_and_outputs("mailutils_drvhash2");

    let outpath = nix_eval_out_path("mailutils_drvhash1", "out");
    realise_storepath(&outpath);

    // outpath has a missing deriver
    // let's create a different one
    nix_instantiate("mailutils_drvhash2");

    populate_cache(&t);

    // start a server that can't fetch from hydra
    let (port, mut server) = spawn_server(&t, Some(vec![]));

    // check that the server can use the deriver of mailutils_drvhash2 instead of
    // the deriver returned by hydra (mailutils_drvhash1)
    let out = gdb(
        &t,
        outpath.join("bin/mailutils").as_ref(),
        port,
        "start\nl\n",
    );
    assert!(dbg!(out).contains("156\tmain (int argc, char **argv)"));

    server.kill().unwrap();
}

#[test]
fn test_hydra_api_file() {
    remove_debuginfo_for_buildid("10deef1d1c1e79a27c25e9636d652ca3b99dc3f5");
    let t = tempfile::tempdir().unwrap();
    let store = file_in(&t, "store");

    let output = file_in(&t, "python");
    // build in another store so we don't have the drv file
    nix_build("python3", &output, Some(&store));
    let python = std::fs::read_link(output).unwrap();
    let output = file_in(&t, "python_debug");
    nix_build("python3.debug", &output, Some(&store));
    let real_output = output.with_file_name(format!(
        "{}-debug",
        output.file_name().unwrap().to_str().unwrap()
    ));
    let python_debug = std::fs::read_link(real_output).unwrap();

    let cache_dir = file_in(&t, "cache");
    let cache = format!("file://{}?index-debug-info=true", cache_dir.display());

    nix_copy(None::<PathBuf>, Some(&cache), &python_debug, Some(&store));
    nix_copy(Some(&store), None::<PathBuf>, &python, None::<PathBuf>);

    let (port, mut server) = spawn_server(&t, Some(vec![&cache]));

    let exe = python.join("bin/python");
    // this is before indexation finished, and should not populate the client cache
    let out = gdb(&t, &exe, port, "start\n");
    // we don't get source code but at least we get location information
    assert!(dbg!(out).contains(" at Programs/python.c:15"));

    server.kill().unwrap();
}

#[test]
fn test_hydra_api_https() {
    remove_debuginfo_for_buildid("78218dee9fd3709104f6521a2c5507fb0a5732b2");
    let t = tempfile::tempdir().unwrap();
    let store = file_in(&t, "store");

    let output = file_in(&t, "python");
    // build in another store so we don't have the drv file
    nix_build("python310", &output, Some(&store));
    let python = std::fs::read_link(output).unwrap();

    nix_copy(Some(&store), None::<PathBuf>, &python, None::<PathBuf>);

    let (port, mut server) = spawn_server(&t, None);

    let exe = python.join("bin/python");
    // this is before indexation finished, and should not populate the client cache
    let out = gdb(&t, &exe, port, "start\n");
    // we don't get source code but at least we get location information
    assert!(dbg!(out).contains(" at Programs/python.c:15"));

    server.kill().unwrap();
}

#[test]
fn test_cache_invalidation() {
    let t = tempfile::tempdir().unwrap();

    let output = file_in(&t, "sl");
    nix_build("sl", &output, None::<PathBuf>);
    let sl = std::fs::read_link(output).unwrap();
    let output = file_in(&t, "sl_debug");
    nix_build("sl.debug", &output, None::<PathBuf>);
    let real_output = output.with_file_name(format!(
        "{}-debug",
        output.file_name().unwrap().to_str().unwrap()
    ));
    let sl_debug = std::fs::read_link(&real_output).unwrap();
    std::fs::remove_file(real_output).unwrap();

    let cache_dir = file_in(&t, "cache");
    let cache = format!("file://{}?index-debug-info=true", cache_dir.display());

    nix_copy(None::<PathBuf>, Some(&cache), &sl_debug, None::<PathBuf>);

    // register the debug output
    populate_cache(&t);

    // invalidate the value in the cache
    remove_debug_output("sl");

    let (port, mut server) = spawn_server(&t, Some(vec![&cache]));

    let exe = sl.join("bin/sl");
    // the cached value does not exist anymore and cannot be recreated
    // with nix-store --realise, so fetch it with the hydra api
    let out = gdb(&t, &exe, port, "start\n");
    assert!(dbg!(out).contains("at sl.c:120"));

    server.kill().unwrap();
}
