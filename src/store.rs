// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

//! Lower level utilities to query the store.

use crate::db::Entry;
use crate::log::ResultExt;
use anyhow::Context;
use object::read::Object;
use once_cell::unsync::Lazy;
use std::{
    ffi::{OsStr, OsString},
    os::unix::prelude::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc::Sender;

/// Whether nix-store supports --query --valid-derivers (>= 2.18)
///
/// Set by [detect_nix].
static NIX_STORE_QUERY_VALID_DERIVERS_SUPPORTED: AtomicBool = AtomicBool::new(false);

const NIX_STORE: &str = "/nix/store";

/// attempts have this store path exist in the store
///
/// if the path already exists, do nothing
/// otherwise runs `nix-store --realise` to download it from a binary cache.
pub async fn realise(path: &Path) -> anyhow::Result<()> {
    use tokio::fs::metadata;
    use tokio::process::Command;
    if metadata(path).await.is_ok() {
        return Ok(());
    };
    let mut command = Command::new("nix-store");
    command.arg("--realise").arg(path);
    tracing::info!("Running {:?}", &command);
    let _ = command.status().await;
    if metadata(path).await.is_ok() {
        return Ok(());
    };
    anyhow::bail!("nix-store --realise {} failed", path.display());
}

/// downloads a .drv file if necessary
///
/// if the path already exists, do nothing
/// otherwise runs `nix-store --realise` to download it from a binary cache.
fn download_drv(path: &Path) -> anyhow::Result<()> {
    use std::fs::metadata;
    use std::process::Command;
    if metadata(path).is_ok() {
        return Ok(());
    };
    let mut command = Command::new("nix-store");
    command.arg("--realise");
    // nix-store --realise foo.drv downloads the drv and its default output
    // we use the following trick to only download the drv: we ask for a non existing output
    // as the narinfo does not give the list of outputs, nix has to download the drv first, and
    // then fails to download the output
    command.arg(path.with_extension("drv!outputdoesn0tex1st"));
    tracing::info!("Running {:?}", &command);
    let _ = command.status();
    if metadata(path).is_ok() {
        return Ok(());
    };
    anyhow::bail!("nix-store --realise {} failed", path.display());
}

/// Walks a store path and attempts to register everything that has a buildid in it.
/// If offline is false, may try to download the .drv file from cache.
pub fn index_store_path(storepath: &Path, sendto: Sender<Entry>, offline: bool) {
    let span = tracing::info_span!("indexing", storepath=%storepath.display()).entered();
    if storepath
        .file_name()
        .unwrap_or_default()
        .as_bytes()
        .ends_with(b".drv")
    {
        return;
    }
    if !storepath.is_dir() {
        return;
    }
    let deriver_source = Lazy::new(|| match get_deriver(storepath) {
        Err(e) => {
            tracing::warn!("no deriver for {}: {:#}", storepath.display(), e);
            (None, None)
        }
        Ok(None) => (None, None),
        Ok(Some(deriver)) => {
            if !offline && !deriver.is_file() {
                download_drv(deriver.as_ref())
                    .with_context(|| {
                        format!(
                            "downloading deriver {} of {}",
                            deriver.display(),
                            storepath.display()
                        )
                    })
                    .or_warn();
            }
            if deriver.is_file() {
                let source = match get_source(deriver.as_path()) {
                    Err(e) => {
                        tracing::info!(
                            "no source for {} (deriver of {}): {:#}",
                            deriver.display(),
                            storepath.display(),
                            e
                        );
                        None
                    }
                    Ok(s) => Some(s),
                };
                (Some(deriver), source)
            } else {
                (None, None)
            }
        }
    });
    let storepath_os: &OsStr = storepath.as_ref();
    if storepath_os.as_bytes().ends_with(b"-debug") {
        let mut root = storepath.to_owned();
        root.push("lib");
        root.push("debug");
        root.push(".build-id");
        if !root.is_dir() {
            return;
        };
        let readroot = match std::fs::read_dir(&root) {
            Err(e) => {
                tracing::warn!("could not list {}: {:#}", root.display(), e);
                return;
            }
            Ok(r) => r,
        };
        for mid in readroot {
            let mid = match mid {
                Err(e) => {
                    tracing::warn!("could not list {}: {:#}", root.display(), e);
                    continue;
                }
                Ok(mid) => mid,
            };
            if !mid.file_type().map(|x| x.is_dir()).unwrap_or(false) {
                continue;
            };
            let mid_path = mid.path();
            let mid_name_os = mid.file_name();
            let mid_name = match mid_name_os.to_str() {
                None => continue,
                Some(x) => x,
            };
            let read_mid = match std::fs::read_dir(&mid_path) {
                Err(e) => {
                    tracing::warn!("could not list {}: {:#}", mid_path.display(), e);
                    continue;
                }
                Ok(r) => r,
            };
            for end in read_mid {
                let end = match end {
                    Err(e) => {
                        tracing::warn!("could not list {}: {:#}", mid_path.display(), e);
                        continue;
                    }
                    Ok(end) => end,
                };
                if !end.file_type().map(|x| x.is_file()).unwrap_or(false) {
                    continue;
                };
                let end_name_os = end.file_name();
                let end_name = match end_name_os.to_str() {
                    None => continue,
                    Some(x) => x,
                };
                if !end_name.ends_with(".debug") {
                    continue;
                };
                let buildid = format!(
                    "{}{}",
                    &mid_name,
                    &end_name[..(end_name.len() - ".debug".len())]
                );
                let (_, source) = &*deriver_source;
                let entry = Entry {
                    debuginfo: end.path().to_str().map(|s| s.to_owned()),
                    executable: None,
                    source: source.as_ref().and_then(|path| {
                        path.as_ref()
                            .and_then(|path| path.to_str())
                            .map(|s| s.to_owned())
                    }),
                    buildid,
                };
                sendto
                    .blocking_send(entry)
                    .context("sending entry failed")
                    .or_warn();
            }
        }
    } else {
        let debug_output = Lazy::new(|| {
            let (deriver, _) = &*deriver_source;
            match deriver {
                None => None,
                Some(deriver) => match get_debug_output(deriver.as_path()) {
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(
                            "could not determine if the deriver {} of {} has a debug output: {:#}",
                            storepath.display(),
                            deriver.display(),
                            e
                        );
                        None
                    }
                    Ok(Some(d)) => Some(d),
                },
            }
        });
        for file in walkdir::WalkDir::new(storepath) {
            let file = match file {
                Err(_) => continue,
                Ok(file) => file,
            };
            if !file.file_type().is_file() {
                continue;
            };
            let path = file.path();
            let buildid = match get_buildid(path) {
                Err(e) => {
                    tracing::info!("cannot get buildid of {}: {:#}", path.display(), e);
                    continue;
                }
                Ok(Some(buildid)) => buildid,
                Ok(None) => continue,
            };
            let debuginfo = match &*debug_output {
                None => None,
                Some(storepath) => {
                    let theoretical = debuginfo_path_for(&buildid, storepath.as_path());
                    if storepath.is_dir() {
                        // the store path is available, check the prediction
                        if !theoretical.is_file() {
                            tracing::warn!(
                                "{} has buildid {}, and {} exists but not {}",
                                path.display(),
                                buildid,
                                storepath.display(),
                                theoretical.display()
                            );
                            None
                        } else {
                            Some(theoretical)
                        }
                    } else {
                        Some(theoretical)
                    }
                }
            };
            let (_, source) = &*deriver_source;
            let entry = Entry {
                buildid,
                source: source.as_ref().and_then(|path| {
                    path.as_ref()
                        .and_then(|path| path.to_str())
                        .map(|s| s.to_owned())
                }),
                executable: path.to_str().map(|s| s.to_owned()),
                debuginfo: debuginfo.and_then(|path| path.to_str().map(|s| s.to_owned())),
            };
            sendto
                .blocking_send(entry)
                .context("sending entry failed")
                .or_warn();
        }
    }
    drop(span)
}

/// Return the path where separate debuginfo is to be found in a debug output for a buildid
fn debuginfo_path_for(buildid: &str, debug_output: &Path) -> PathBuf {
    let mut res = debug_output.to_path_buf();
    res.push("lib");
    res.push("debug");
    res.push(".build-id");
    res.push(&buildid[..2]);
    res.push(format!("{}.debug", &buildid[2..]));
    res
}

/// Obtains the original deriver of a store path.
///
/// Corresponds to `nix-store --query --deriver`
///
/// The store path must exist.
fn get_original_deriver(storepath: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut cmd = std::process::Command::new("nix-store");
    cmd.arg("--query").arg("--deriver").arg(storepath);
    tracing::debug!("Running {:?}", &cmd);
    let out = cmd.output().with_context(|| format!("running {:?}", cmd))?;
    if !out.status.success() {
        anyhow::bail!("{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr));
    }
    let n = out.stdout.len();
    if n <= 1 || out.stdout[n - 1] != b'\n' {
        anyhow::bail!(
            "{:?} returned weird output: {}",
            cmd,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let path = PathBuf::from(OsString::from_vec(out.stdout[..n - 1].to_owned()));
    if path.as_path() == Path::new("unknown-deriver") {
        return Ok(None);
    }
    if !path.is_absolute() {
        // nix returns `unknown-deriver` when it does not know
        anyhow::bail!("no deriver: {}", path.display());
    };
    Ok(Some(path))
}

/// Obtains a set of local derivers for a store path.
///
/// Corresponds to `nix-store --query --valid-derivers`
///
/// The store path must exist.
///
/// Fails if nix version is < 2.18
fn get_valid_derivers(storepath: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut cmd = std::process::Command::new("nix-store");
    cmd.arg("--query").arg("--valid-derivers").arg(storepath);
    tracing::debug!("Running {:?}", &cmd);
    let out = cmd.output().with_context(|| format!("running {:?}", cmd))?;
    if !out.status.success() {
        anyhow::bail!("{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr));
    }
    let mut result = Vec::new();
    for line in out.stdout.split(|&c| c == b'\n') {
        if !line.is_empty() {
            let path = PathBuf::from(OsString::from_vec(line.to_owned()));
            if !path.is_absolute() {
                // nix returns `unknown-deriver` when it does not know
                anyhow::bail!(
                    "incorrect deriver {} for {}",
                    String::from_utf8_lossy(line),
                    path.display()
                );
            };
            result.push(path)
        }
    }
    Ok(result)
}

/// Attempts to obtain any deriver for this store path, preferably existing.
///
/// Corresponds to `nix-store --query --deriver` or `nix-store --query --valid-derivers.
///
/// The store path must exist.
fn get_deriver(storepath: &Path) -> anyhow::Result<Option<PathBuf>> {
    if NIX_STORE_QUERY_VALID_DERIVERS_SUPPORTED.load(Ordering::SeqCst) {
        for path in get_valid_derivers(storepath)
            .with_context(|| format!("getting valid deriver for {}", storepath.display()))?
        {
            if path.exists() {
                return Ok(Some(path));
            } else {
                tracing::warn!(
                    "nix-store --query --valid-derivers {} returned a non-existing path",
                    storepath.display()
                );
            }
        }
    }
    get_original_deriver(storepath)
        .with_context(|| format!("getting original deriver for {}", storepath.display()))
}

/// Checks that nix is installed.
///
/// Also stores in global state whether some features only available in recent nix
/// versions are available.
///
/// Should be called on startup.
pub fn detect_nix() -> anyhow::Result<()> {
    let mut test_path = None;
    for entry in Path::new("/nix/store")
        .read_dir()
        .context("listing directory content of /nix/store")?
    {
        let entry = entry.context("reading directory entry in /nix/store")?;
        if entry.file_name().as_bytes().starts_with(b".") {
            continue;
        }
        test_path = Some(entry.path());
        break;
    }
    let test_path = match test_path {
        Some(test_path) => test_path,
        None => anyhow::bail!("/nix/store is empty, did you really install nix?"),
    };
    if get_valid_derivers(&test_path).is_ok() {
        NIX_STORE_QUERY_VALID_DERIVERS_SUPPORTED.store(true, Ordering::SeqCst);
        tracing::info!("detected nix >= 2.18");
        return Ok(());
    }
    let _ = get_original_deriver(&test_path).with_context(|| {
        format!(
            "checking nix install by getting deriver of {}",
            test_path.display()
        )
    })?;
    tracing::warn!("detected nix < 2.18, a more recent nix is required to obtain source files in some situations.");
    Ok(())
}

/// Obtains the debug output corresponding to this derivation
///
/// The derivation must exist.
fn get_debug_output(drvpath: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut cmd = std::process::Command::new("nix-store");
    cmd.arg("--query").arg("--outputs").arg(drvpath);
    tracing::debug!("Running {:?}", &cmd);
    let out = cmd.output().with_context(|| format!("running {:?}", cmd))?;
    if !out.status.success() {
        anyhow::bail!("{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr));
    }
    for output in out.stdout.split(|&elt| elt == b'\n') {
        if output.ends_with(b"-debug") {
            return Ok(Some(PathBuf::from(OsString::from_vec(output.to_owned()))));
        }
    }
    Ok(None)
}

/// Obtains the source store path corresponding to this derivation
///
/// The derivation must exist.
///
/// Source is understood as `src = `, multiple sources or patches are not supported.
fn get_source(drvpath: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut cmd = std::process::Command::new("nix-store");
    cmd.arg("--query").arg("--binding").arg("src").arg(drvpath);
    tracing::debug!("Running {:?}", &cmd);
    let out = cmd.output().with_context(|| format!("running {:?}", cmd))?;
    if !out.status.success() {
        if out
            .stderr
            .as_slice()
            .ends_with(b"has no environment binding named 'src'\n")
        {
            return Ok(None);
        } else {
            anyhow::bail!("{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr));
        }
    }
    let n = out.stdout.len();
    if n <= 1 || out.stdout[n - 1] != b'\n' {
        anyhow::bail!(
            "{:?} returned weird output: {}",
            cmd,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let path = PathBuf::from(OsString::from_vec(out.stdout[..n - 1].to_owned()));
    if !path.is_absolute() {
        anyhow::bail!("weird source: {}", path.display());
    };
    Ok(Some(path))
}

/// Where a source file might be
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLocation {
    /// Inside an archive
    Archive {
        /// path of the archive
        archive: PathBuf,
        /// path of the file in the archive
        member: PathBuf,
    },
    /// A file directly in the store
    File(PathBuf),
}

impl SourceLocation {
    /// Get the path against which we match the requested file name
    fn member_path(&self) -> &Path {
        match self {
            SourceLocation::Archive { member, .. } => member.as_path(),
            SourceLocation::File(path) => path.as_path(),
        }
    }
}

/// Return the build id of this file.
///
/// If the file is not an executable returns Ok(None).
/// Errors are only for errors returned from the fs.
pub fn get_buildid(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {} to get its buildid", path.display()))?;
    let reader = object::read::ReadCache::new(file);
    let object = match object::read::File::parse(&reader) {
        Err(_) => {
            // object::read::Error is opaque, so no way to distinguish "this is not elf" and a real
            // error
            return Ok(None);
        }
        Ok(o) => o,
    };
    match object
        .build_id()
        .with_context(|| format!("parsing {} for buildid", path.display()))?
    {
        None => Ok(None),
        Some(data) => {
            let buildid = base16::encode_lower(&data);
            Ok(Some(buildid))
        }
    }
}

/// To remove references, gcc is patched to replace the hash part
/// of store path by an uppercase version in debug symbols.
///
/// Store paths are embedded in debug symbols for example as the location
/// of template instantiation from libraries that live in other derivations.
///
/// This function undoes the mangling.
pub fn demangle(storepath: PathBuf) -> PathBuf {
    if !storepath.starts_with(NIX_STORE) {
        return storepath;
    }
    let mut as_bytes = storepath.into_os_string().into_vec();
    let len = as_bytes.len();
    let store_len = NIX_STORE.len();
    as_bytes[len.min(store_len + 1)..len.min(store_len + 1 + 32)].make_ascii_lowercase();
    OsString::from_vec(as_bytes).into()
}

#[test]
fn test_demangle_nominal() {
    assert_eq!(demangle(PathBuf::from("/nix/store/JW65XNML1FGF4BFGZGISZCK3LFJWXG6L-GCC-12.3.0/include/c++/12.3.0/bits/vector.tcc")), PathBuf::from("/nix/store/jw65xnml1fgf4bfgzgiszck3lfjwxg6l-GCC-12.3.0/include/c++/12.3.0/bits/vector.tcc"));
}

#[test]
fn test_demangle_noop() {
    assert_eq!(demangle(PathBuf::from("/nix/store/jw65xnml1fgf4bfgzgiszck3lfjwxg6l-gcc-12.3.0/include/c++/12.3.0/bits/vector.tcc")), PathBuf::from("/nix/store/jw65xnml1fgf4bfgzgiszck3lfjwxg6l-gcc-12.3.0/include/c++/12.3.0/bits/vector.tcc"));
}

#[test]
fn test_demangle_empty() {
    assert_eq!(demangle(PathBuf::from("/")), PathBuf::from("/"));
}

#[test]
fn test_demangle_incomplete() {
    assert_eq!(
        demangle(PathBuf::from("/nix/store/JW65XNML1FGF4B")),
        PathBuf::from("/nix/store/jw65xnml1fgf4b")
    );
}

#[test]
fn test_demangle_non_storepath() {
    assert_eq!(
        demangle(PathBuf::from("/build/src/FOO.C")),
        PathBuf::from("/build/src/FOO.C")
    );
}

/// Attempts to find a file that matches the request in an existing source path.
pub fn get_file_for_source(
    source: &Path,
    request: &Path,
) -> anyhow::Result<Option<SourceLocation>> {
    tracing::info!(
        "request path {:?} in source {:?}",
        request.display(),
        source.display()
    );

    let target: Vec<&OsStr> = request.iter().collect();
    // invariant: we only keep candidates which have same path as target for components i..
    let mut candidates: Vec<_> = Vec::new();
    let source_type = source
        .metadata()
        .with_context(|| format!("stat({})", source.display()))?;
    if source_type.is_dir() {
        for file in walkdir::WalkDir::new(source) {
            match file {
                Err(e) => {
                    tracing::warn!("failed to walk source {}: {:#}", source.display(), e);
                    continue;
                }
                Ok(f) => {
                    if Some(&f.file_name()) == target.last() {
                        candidates.push(SourceLocation::File(f.path().to_path_buf()));
                    }
                }
            }
        }
    } else if source_type.is_file() {
        let mut archive = std::fs::File::open(source)
            .with_context(|| format!("opening source archive {}", source.display()))?;
        let member_list = compress_tools::list_archive_files(&mut archive)
            .with_context(|| format!("listing files in source archive {}", source.display()))?;
        for member in member_list {
            if Path::new(&member).file_name().as_ref() == target.last() {
                candidates.push(SourceLocation::Archive {
                    archive: source.to_path_buf(),
                    member: PathBuf::from(member),
                });
            }
        }
    }
    if candidates.len() < 2 {
        return Ok(candidates.pop());
    }
    let mut best_total_len = 0;
    let mut best_matching_len = 0;
    let mut best_candidates = Vec::new();
    for candidate in candidates {
        let member_path = candidate.member_path();
        let total_len = member_path.iter().count();
        let matching_len = member_path
            .iter()
            .rev()
            .zip(target.iter().rev())
            .skip(1)
            .position(|(ref c, t)| c != t)
            .unwrap_or(total_len - 1);
        if matching_len > best_matching_len
            || (matching_len == best_matching_len && total_len < best_total_len)
        {
            best_matching_len = matching_len;
            best_total_len = total_len;
            best_candidates.clear();
            best_candidates.push(candidate);
        } else if matching_len == best_matching_len {
            best_candidates.push(candidate);
        }
    }
    if best_candidates.len() > 1 {
        anyhow::bail!(
            "cannot tell {:?} apart from {} for target {}",
            &best_candidates,
            &source.display(),
            request.display()
        );
    }
    Ok(best_candidates.pop())
}

#[cfg(test)]
fn make_test_source_path(paths: Vec<&'static str>) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    for path in paths {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "content").unwrap();
    }
    dir
}

#[test]
fn get_file_for_source_simple() {
    let dir = make_test_source_path(vec!["soft-version/src/main.c", "soft-version/src/Makefile"]);
    let res = get_file_for_source(dir.path(), "/source/soft-version/src/main.c".as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(
        res,
        SourceLocation::File(dir.path().join("soft-version/src/main.c"))
    );
}

#[test]
fn get_file_for_source_different_dir() {
    let dir = make_test_source_path(vec!["lib/core-net/network.c", "lib/plat/optee/network.c"]);
    let res = get_file_for_source(dir.path(), "/build/source/lib/core-net/network.c".as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(
        res,
        SourceLocation::File(dir.path().join("lib/core-net/network.c"))
    );
}

#[test]
fn get_file_for_source_regression_pr_7() {
    let dir = make_test_source_path(vec![
        "store/source/lib/core-net/network.c",
        "store/source/lib/plat/optee/network.c",
    ]);
    let res = get_file_for_source(dir.path(), "build/source/lib/core-net/network.c".as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(
        res,
        SourceLocation::File(dir.path().join("store/source/lib/core-net/network.c"))
    );
}

#[test]
fn get_file_for_source_no_right_filename() {
    let dir = make_test_source_path(vec![
        "store/source/lib/core-net/network.c",
        "store/source/lib/plat/optee/network.c",
    ]);
    let res = get_file_for_source(
        dir.path(),
        "build/source/lib/core-net/somethingelse.c".as_ref(),
    );
    assert_eq!(res.unwrap(), None);
}

#[test]
fn get_file_for_source_glibc() {
    let dir = make_test_source_path(vec![
        "glibc-2.37/sysdeps/unix/sysv/linux/openat64.c",
        "glibc-2.37/sysdeps/mach/hurd/openat64.c",
        "glibc-2.37/io/openat64.c",
    ]);
    let res = get_file_for_source(
        dir.path(),
        "/build/glibc-2.37/io/../sysdeps/unix/sysv/linux/openat64.c".as_ref(),
    );
    assert_eq!(
        res.unwrap().unwrap(),
        SourceLocation::File(
            dir.path()
                .join("glibc-2.37/sysdeps/unix/sysv/linux/openat64.c")
        )
    );
}

#[test]
fn get_file_for_source_misleading_dir() {
    let dir = make_test_source_path(vec!["store/store/wrong/dir/file", "good/dir/store/file"]);
    let res = get_file_for_source(dir.path(), "/build/project/store/file".as_ref());
    assert_eq!(
        res.unwrap().unwrap(),
        SourceLocation::File(dir.path().join("good/dir/store/file"))
    );
}

#[test]
fn get_file_for_source_ambiguous() {
    let sources = vec![
        "glibc-2.37/sysdeps/unix/sysv/linux/openat64.c",
        "glibc-2.37/sysdeps/mach/hurd/openat64.c",
        "glibc-2.37/io/openat64.c",
    ];
    let dir = make_test_source_path(sources.clone());
    let res = get_file_for_source(
        dir.path(),
        "/build/glibc-2.37/fakeexample/openat64.c".as_ref(),
    );
    assert!(res.is_err());
    let msg = res.unwrap_err().to_string();
    assert!(dbg!(&msg).contains("cannot tell"));
    assert!(msg.contains("apart"));
    for source in sources {
        assert!(msg.contains(source));
    }
}

/// Turns a path in the store as its topmost parent in /nix/store
pub fn get_store_path(path: &Path) -> Option<&Path> {
    let mut ancestors = path.ancestors().peekable();
    while let Some(a) = ancestors.next() {
        match ancestors.peek() {
            Some(p) if p.as_os_str() == "/nix/store" => return Some(a),
            _ => (),
        }
    }
    None
}

#[test]
fn test_get_store_path() {
    assert_eq!(
        get_store_path(Path::new("/nix/store/foo")).unwrap(),
        Path::new("/nix/store/foo")
    );
    assert_eq!(
        get_store_path(Path::new("/nix/store/foo/bar")).unwrap(),
        Path::new("/nix/store/foo")
    );
    assert_eq!(get_store_path(Path::new("eq")), None);
}
