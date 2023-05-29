//! Access to debuginfo indexed in binary caches created with
//! `?index-debug-info=true`, for example Hydra.
//!
//! This is the API that dwarffs uses. When nars are copied to the binary cache,
//! file in the for `$out/lib/debug/.build-id/sh/a1` are symlinked into `debuginfo/sha1`
//! (on hydra) or `debuginfo/sha1.debug` (for file:/// caches crated with nix-copy).
//! The actual nature of the symnlink can vary: it may be a json file.

use std::{
    collections::hash_map::DefaultHasher,
    ffi::OsStr,
    hash::{Hash, Hasher},
    io::{BufReader, Read},
    os::unix::prelude::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::Context;
use async_recursion::async_recursion;
use async_trait::async_trait;
use futures_util::StreamExt;
use http::StatusCode;
use reqwest::Url;
use serde::Deserialize;
use tempfile::TempDir;
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::store::{get_buildid, get_store_path};

#[derive(Deserialize)]
struct DebuginfoMetadata {
    /// the relative path of the nar.xz in this substituter
    archive: String,
    /// the file inside the nar that holds the debuginfo
    #[allow(dead_code)]
    member: String,
}

/// Returns the 32 first bytes of the specified file
async fn magic(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut res = vec![b'\0'; 32];
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("reading magic of {}", path.display()))?;
    file.read_exact(&mut res[..])
        .with_context(|| format!("reading start of {} to determine magic", path.display()))?;
    Ok(res)
}

const NAR_MAGIC: &'static [u8] = b"\x0d\x00\x00\x00\x00\x00\x00\x00nix-archive-1";
const ELF_MAGIC: &'static [u8] = b"\x7fELF";

/// API to fetch debuginfo indices from substituters
#[async_trait]
pub trait Substituter: Send + Sync {
    /// Fetches a file from the substituter indexed by its relative path
    /// to the root
    ///
    /// Returns None in case of missing file.
    async fn fetch(&self, path: &Path) -> anyhow::Result<Option<PathBuf>>;

    /// the url used to construct this substituter
    fn url(&self) -> &str;
}

/// returns a store path containing the requested debuginfo in
/// `/lib/debug/.build-id`
pub async fn fetch_debuginfo<T: Substituter + ?Sized>(
    substituter: &T,
    buildid: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let mut res = Ok(None);
    for path in [
        // for hydra
        PathBuf::from(format!("debuginfo/{buildid}")),
        // for file:///path?index-debug-info=true created with nix copy
        PathBuf::from(format!("debuginfo/{buildid}.debug")),
    ]
    .into_iter()
    {
        res = fetch_debuginfo_from(substituter, path.as_path(), 2).await;
        if let Ok(Some(path)) = &res {
            tracing::info!(
                "downloaded debuginfo for {} from {} into {}",
                buildid,
                substituter.url(),
                path.display()
            );
            break;
        }
    }
    res
}

/// attempt to fetch debuginfo in this relative path inside the substituter
///
/// returns a store path containing it
#[async_recursion]
async fn fetch_debuginfo_from<T: Substituter + ?Sized>(
    substituter: &T,
    path: &Path,
    max_redirects: usize,
) -> anyhow::Result<Option<PathBuf>> {
    tracing::debug!(
        "attempting to fetch {} from {}",
        path.display(),
        substituter.url()
    );
    let file = substituter
        .fetch(path)
        .await
        .with_context(|| format!("fetching {} from {}", path.display(), substituter.url()))?;
    let file = match file {
        None => return Ok(None),
        Some(f) => f,
    };
    let tempdir;
    let temppath;
    let target;
    // the logic below is taken from dwarffs, but hydra only uses json redirection -> nar.xz
    let dir_to_add = match &magic(file.as_path()).await? {
        m if m.starts_with(ELF_MAGIC) => {
            /* This is the debuginfo file we want.
             * Let's create the expected hierarchy `lib/debug/.buildid/aa/bbbbbbbb`
             */
            // sync code
            let buildid = match get_buildid(file.as_path()).with_context(|| {
                format!(
                    "buildid of elf file fetched from {} in {}",
                    path.display(),
                    substituter.url()
                )
            })? {
                None => anyhow::bail!(
                    "fetched elf file from {} in {} but it has no build id",
                    path.display(),
                    substituter.url()
                ),
                Some(x) => x,
            };
            let dir = TempDir::new().context("tempdir")?;
            target = dir.path().join("target-nar");
            let mut parent = target.join("lib/debug/.build-id");
            parent.push(&buildid[..2]);
            tokio::fs::create_dir_all(parent.as_path())
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
            parent.push(format!("{}.debug", &buildid[2..]));
            tokio::fs::copy(file.as_path(), parent.as_path())
                .await
                .context("copying debuginfo file")?;
            target.as_path()
        }
        m if m.starts_with(b"{") => {
            /*****************
             * this is a json redirect
             *****************/
            if max_redirects == 0 {
                anyhow::bail!("too many redirects");
            }
            // sync code
            let file = std::fs::File::open(file.as_path())
                .with_context(|| format!("opening {} to deserialize as json", path.display()))?;
            let bufread = BufReader::new(file);
            let metadata: DebuginfoMetadata = serde_json::from_reader(bufread)
                .with_context(|| format!("parsing {} as json", path.display()))?;
            let mut redirect_path = match path.parent() {
                None => PathBuf::from("."),
                Some(p) => p.to_path_buf(),
            };
            redirect_path.push(&metadata.archive);
            anyhow::ensure!(
                redirect_path.is_relative(),
                "debuginfo metadata {} from {} features an absolute path {}",
                path.display(),
                substituter.url(),
                &metadata.archive
            );
            return fetch_debuginfo_from(substituter, redirect_path.as_path(), max_redirects - 1)
                .await;
        }
        m => {
            let nar_file = if m.starts_with(NAR_MAGIC) {
                /***********
                 * this is the nar file containing the debuginfo
                 **********/
                file.as_path() // a nar file
            } else {
                /***********
                 * this is a compressed nar probably
                 **********/
                temppath = tempfile::NamedTempFile::new()
                    .context("temppath")?
                    .into_temp_path();
                let out = tokio::fs::File::create(&temppath)
                    .await
                    .context("opening temppath")?;
                let fd = tokio::fs::File::open(&file).await.context("unxz")?;
                compress_tools::tokio_support::uncompress_data(fd, out)
                    .await
                    .with_context(|| {
                        format!("unpacking {} from {}", file.display(), substituter.url())
                    })?;
                if magic(temppath.as_ref())
                    .await
                    .context("magic of uncompressed nar")?
                    .starts_with(NAR_MAGIC)
                {
                    temppath.as_ref()
                } else {
                    anyhow::bail!("nar {} was not a compressed nar", path.display());
                }
            };
            // unpack the nar
            let fd = tokio::fs::File::open(nar_file).await?;
            let mut cmd = tokio::process::Command::new("nix-store");
            cmd.arg("--restore");
            tempdir = tempfile::TempDir::new().context("tempdir")?;
            // FIXME: the indexer should probably not take the name of the store path into account
            target = tempdir.as_ref().join("nar-debug");
            cmd.arg(target.as_path());
            cmd.stdin(fd.into_std().await);
            let status = cmd.status().await.with_context(|| {
                format!(
                    "running nix-store --import to unpack nar from {} in {}",
                    nar_file.display(),
                    substituter.url()
                )
            })?;
            anyhow::ensure!(status.success(), "nix-store --import failed: {:?}", status);
            anyhow::ensure!(
                target.exists(),
                "nix-store --import failed to create {}",
                target.display()
            );

            target.as_path()
        }
    };

    // add it to the store
    let mut cmd = tokio::process::Command::new("nix-store");
    cmd.arg("--add");
    cmd.arg(dir_to_add);
    let output = cmd.output().await.context("nix-store --add")?;
    anyhow::ensure!(
        output.status.success(),
        "nix-store --add failed: {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let mut storepath = &output.stdout[..];
    if storepath.ends_with(b"\n") {
        storepath = &storepath[..(storepath.len() - 1)];
    }
    let storepath = Path::new::<OsStr>(OsStrExt::from_bytes(storepath));
    match get_store_path(storepath) {
        None => anyhow::bail!(
            "nix-store --add did not return a store path but «{}»",
            storepath.display()
        ),
        Some(s) => {
            anyhow::ensure!(s.exists(), "nix-store --add failed to produce a storepath");
            Ok(Some(s.to_path_buf()))
        }
    }
}

/// A file:/// substituter
#[derive(PartialEq, Eq, Debug)]
pub struct FileSubstituter {
    // root path of the substituter
    path: PathBuf,
    // url of the substituter
    url: String,
}

impl FileSubstituter {
    /// If this url starts with file:/// and is a real path then returns an instance, otherwise
    /// None
    pub async fn from_url(url: &str) -> anyhow::Result<Option<Self>> {
        let parsed_url =
            Url::parse(url).with_context(|| format!("parsing binary cache url {url}"))?;
        if parsed_url.scheme() != "file" {
            return Ok(None);
        }
        let path = parsed_url
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("cannot convert {} to file path", url))?;
        let path = path.canonicalize().with_context(|| {
            format!(
                "resolving directory {} of substituter {}",
                path.display(),
                url
            )
        })?;
        Ok(Some(FileSubstituter {
            path,
            url: url.to_owned(),
        }))
    }
}

#[async_trait]
impl Substituter for FileSubstituter {
    async fn fetch(&self, path: &Path) -> anyhow::Result<Option<PathBuf>> {
        anyhow::ensure!(
            path.is_relative(),
            "substituter path {} should be relative",
            path.display()
        );
        let path = self.path.join(path);
        if path.exists() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn url(&self) -> &str {
        &self.url
    }
}

#[tokio::test]
async fn file_substituter_from_url() {
    let d = TempDir::new().unwrap();
    assert!(matches!(
        FileSubstituter::from_url("https://cache.nixos.rg").await,
        Ok(None)
    ));
    assert!(matches!(
        FileSubstituter::from_url(&format!("file://{}/doesnotexist", d.path().display())).await,
        Err(_)
    ));
    let ok = FileSubstituter::from_url(&format!(
        "file://{}/./?with_query_string=true",
        d.path().display()
    ))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&ok.path, d.path());
}

#[tokio::test]
async fn file_substituter_fetch() {
    let d = TempDir::new().unwrap();
    let ok = FileSubstituter::from_url(&format!("file://{}/./?yay=bar", d.path().display()))
        .await
        .unwrap()
        .unwrap();
    let path = d.path().join("file");
    std::fs::write(&path, "yay").unwrap();
    assert_eq!(ok.fetch(Path::new("./file")).await.unwrap().unwrap(), path);
}

/// A https:/// substituter
#[derive(Debug)]
pub struct HttpSubstituter {
    // The url to contact the cache, without its nix-specific query string, and with a trailing
    // slash
    http_url: Url,
    // url of the substituter, as passed to from_url
    url: String,
    client: reqwest::Client,
    cache: TempDir,
}

impl HttpSubstituter {
    /// If this url starts with file:/// and is a real path then returns an instance, otherwise
    /// None
    pub async fn from_url(url: &str) -> anyhow::Result<Option<Self>> {
        let mut http_url =
            Url::parse(url).with_context(|| format!("parsing binary cache url {url}"))?;
        match http_url.scheme() {
            "http" | "https" => (),
            _ => return Ok(None),
        };

        http_url.set_query(None);
        if !http_url.path().ends_with("/") {
            let mut path = http_url.path().to_owned();
            path.push('/');
            http_url.set_path(&path);
        }

        let cache = TempDir::new().context("tempdir")?;
        let client = reqwest::Client::new();

        Ok(Some(HttpSubstituter {
            http_url,
            url: url.to_owned(),
            cache,
            client,
        }))
    }
}

#[async_trait]
impl Substituter for HttpSubstituter {
    async fn fetch(&self, path: &Path) -> anyhow::Result<Option<PathBuf>> {
        anyhow::ensure!(
            path.is_relative(),
            "substituter path {} should be relative",
            path.display()
        );
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid path {}", path.display()))?;
        let url = self
            .http_url
            .join(path_str)
            .with_context(|| format!("cannot join {} to {}", path_str, &self.http_url))?;

        let mut hasher = DefaultHasher::default();
        url.hash(&mut hasher);
        let hash = hasher.finish();
        let cache_path = self.cache.path().join(format!("{hash:x}"));

        if cache_path.exists() {
            return Ok(Some(cache_path));
        }

        let tmp = tempfile::TempPath::from_path(self.cache.path().join(format!("{hash:x}.part")));
        let fd = tokio::fs::File::create(&tmp).await.context("temp file")?;
        let mut write = BufWriter::new(fd);

        tracing::debug!("getting {}", &url);
        let response = match self.client.get(url.as_str()).send().await {
            Ok(r) if r.status() == StatusCode::NOT_FOUND => {
                tracing::debug!("{} not found in {}", path.display(), self.url());
                return Ok(None);
            }
            Err(e) if e.status() == Some(StatusCode::NOT_FOUND) => {
                tracing::debug!("{} not found in {}", path.display(), self.url());
                return Ok(None);
            }
            Ok(r) if r.status() != StatusCode::OK => {
                tracing::warn!("unexpected status {} for {}", r.status(), &url);
                anyhow::bail!("{} returned status {}", self.url(), r.status());
            }
            Ok(r) => r,
            Err(e) => anyhow::bail!(
                "cannot fetch {} for {} in {}: {:#}",
                &url,
                path.display(),
                self.url(),
                e
            ),
        };
        let mut body = response.bytes_stream();

        while let Some(chunk) = body.next().await {
            let chunk = chunk.with_context(|| {
                format!(
                    "downloading from {} for {} in {}",
                    &url,
                    path.display(),
                    self.url()
                )
            })?;
            write
                .write_all(&chunk)
                .await
                .context("writing to tmp file")?;
        }

        write.flush().await.context("writing to disk")?;
        write.into_inner().sync_data().await.context("syncing")?;

        tmp.persist(&cache_path).context("renaming temp file")?;

        Ok(Some(cache_path))
    }

    fn url(&self) -> &str {
        &self.url
    }
}
