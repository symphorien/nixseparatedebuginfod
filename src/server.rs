// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

//! An http server serving the content of [Cache]
//!
//! References:
//! Protocol: <https://www.mankier.com/8/debuginfod#Webapi>

use anyhow::Context;
use axum::body::StreamBody;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::get, Router};
use http::header::{HeaderMap, CONTENT_LENGTH};
use std::collections::HashSet;
use std::os::unix::prelude::MetadataExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::io::ReaderStream;

use crate::db::Cache;
use crate::index::{index_single_store_path_to_cache, StoreWatcher};
use crate::log::ResultExt;
use crate::store::{get_file_for_source, get_store_path, realise, SourceLocation};
use crate::substituter::{FileSubstituter, HttpSubstituter, Substituter};
use crate::Options;

#[derive(Clone)]
struct ServerState {
    cache: Cache,
    watcher: StoreWatcher,
    substituters: Arc<Vec<Box<dyn Substituter>>>,
}

/// The only status code in the client code of debuginfod in elfutils that prevents
/// creation of a negative cache entry.
///
/// 503 Not Available also works, but only for the section request
const NON_CACHING_ERROR_STATUS: StatusCode = StatusCode::NOT_ACCEPTABLE;

/// Serve the content of this file, or an appropriate error.
///
/// Attempts to substitute the file if necessary.
///
/// `ready` should be true if indexation is currently complete. If it is false,
/// error codes are tuned to prevent the client from caching the answer.
async fn unwrap_file<T: AsRef<std::path::Path>>(
    path: anyhow::Result<Option<T>>,
    ready: bool,
) -> impl IntoResponse {
    let response = match path {
        Ok(Some(p)) => {
            match tokio::fs::File::open(p.as_ref()).await {
                Err(e) => Err((StatusCode::NOT_FOUND, format!("{:#}", e))),
                Ok(file) => {
                    let mut headers = HeaderMap::new();
                    if let Ok(metadata) = p.as_ref().metadata() {
                        if let Ok(value) = metadata.size().to_string().parse() {
                            headers.insert(CONTENT_LENGTH, value);
                        }
                    }
                    tracing::info!("returning {}", p.as_ref().display());
                    // convert the `AsyncRead` into a `Stream`
                    let stream = ReaderStream::new(file);
                    // convert the `Stream` into an `axum::body::HttpBody`
                    let body = StreamBody::new(stream);
                    Ok((headers, body))
                }
            }
        }
        Ok(None) => Err((
            if ready {
                StatusCode::NOT_FOUND
            } else {
                NON_CACHING_ERROR_STATUS
            },
            "not found in cache".to_string(),
        )),
        Err(e) => Err((StatusCode::NOT_FOUND, format!("{:#}", e))),
    };
    if let Err((code, error)) = &response {
        tracing::info!("Responding error {}: {}", code, error);
    };
    response
}

/// Start indexation, and wait for it to complete until timeout.
///
/// Returns wether indexation is complete.
async fn start_indexation_and_wait(watcher: StoreWatcher, timeout: Duration) -> bool {
    match watcher.maybe_index_new_paths().await {
        Err(e) => {
            tracing::warn!("cannot start registration of new store path: {:#}", e);
            false
        }
        Ok(None) => true,
        Ok(Some(handle)) => {
            tokio::select! {
                _ = tokio::time::sleep(timeout) => false,
                _ = handle => true,
            }
        }
    }
}

/// Reindex harder.
///
/// If the .drv file is not in the store, automatic indexation will find the executable but not
/// the debuginfo and source. We can attempt to download this drv file during a second
/// indexation attempt.
async fn maybe_reindex_by_build_id(cache: &Cache, buildid: &str) -> anyhow::Result<()> {
    let exe = match cache
        .get_executable(buildid)
        .await
        .with_context(|| format!("getting executable of {} from cache", buildid))?
    {
        Some(exe) => exe,
        None => return Ok(()),
    };
    tracing::debug!("reindexing {}", &exe);
    let exe = PathBuf::from(exe);
    let storepath = match get_store_path(exe.as_path()) {
        Some(storepath) => storepath,
        None => anyhow::bail!(
            "executable {} for buildid {} is not a store path",
            exe.display(),
            buildid
        ),
    };
    index_single_store_path_to_cache(cache, storepath, true)
        .await
        .with_context(|| format!("indexing {} online", exe.display()))?;
    Ok(())
}

/// Ensures that the contained path exists, and if this is not the case
/// replace it by `Ok(None)`
///
/// The tag is the kind of file this should be, to be used in error messages
async fn and_realise<T: AsRef<std::path::Path>>(
    result: anyhow::Result<Option<T>>,
    tag: &str,
) -> anyhow::Result<Option<T>> {
    match result {
        Ok(Some(p)) => {
            let res = realise(p.as_ref())
                .await
                .with_context(|| format!("realising {} of type {}", p.as_ref().display(), tag));

            if res.is_err() {
                res.or_warn();
                Ok(None)
            } else {
                Ok(Some(p))
            }
        }
        other => other,
    }
}

/// attempts to fetch debuginfo from substituters via the same API as dwarffs
async fn maybe_fetch_debuginfo_from_substituter_index(
    cache: &Cache,
    substituters: &[Box<dyn Substituter>],
    buildid: &str,
) -> anyhow::Result<()> {
    for substituter in substituters.iter() {
        match crate::substituter::fetch_debuginfo(substituter.as_ref(), buildid).await {
            Err(e) => tracing::info!(
                "cannot fetch buildid {} from substituter {}: {:#}",
                buildid,
                substituter.url(),
                e
            ),
            Ok(None) => (),
            Ok(Some(path)) => {
                index_single_store_path_to_cache(cache, &path, false)
                    .await
                    .with_context(|| format!("indexing {}", path.display()))
                    .or_warn();
                if let Ok(Some(_)) =
                    and_realise(cache.get_debuginfo(buildid).await, "debuginfo").await
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// How long to wait for indexation to complete before serving the cache
const INDEXING_TIMEOUT: Duration = Duration::from_secs(1);

#[axum_macros::debug_handler]
async fn get_debuginfo(
    Path(buildid): Path<String>,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(state.watcher, INDEXING_TIMEOUT).await;
    let res = and_realise(state.cache.get_debuginfo(&buildid).await, "debuginfo").await;
    let res = match res {
        Ok(None) => {
            // try again harder
            tracing::debug!("{} was not in cache, reindexing online", buildid);
            match maybe_reindex_by_build_id(&state.cache, &buildid).await {
                Ok(()) => and_realise(state.cache.get_debuginfo(&buildid).await, "debuginfo").await,
                Err(e) => Err(e),
            }
        }
        res => res,
    };
    let res = match res {
        Ok(None) => {
            // try again harder
            tracing::debug!(
                "online reindexation failed for {}, using hydra API",
                buildid
            );
            match maybe_fetch_debuginfo_from_substituter_index(
                &state.cache,
                state.substituters.as_ref(),
                &buildid,
            )
            .await
            {
                Ok(()) => and_realise(state.cache.get_debuginfo(&buildid).await, "debuginfo").await,
                Err(e) => Err(e),
            }
        }
        res => res,
    };
    unwrap_file(res, ready).await
}

#[axum_macros::debug_handler]
async fn get_executable(
    Path(buildid): Path<String>,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(state.watcher, INDEXING_TIMEOUT).await;
    let res = and_realise(state.cache.get_executable(&buildid).await, "executable").await;
    unwrap_file(res, ready).await
}

/// queries the cache for a source file `request` corresponding to `buildid`.
///
/// may download the source if required, and returns where the requested file is on disk.
async fn fetch_and_get_source(
    buildid: String,
    request: PathBuf,
    cache: Cache,
) -> anyhow::Result<Option<SourceLocation>> {
    let source = cache.get_source(&buildid).await;
    let source = match and_realise(source, "source").await {
        Ok(None) => {
            // try again harder
            match maybe_reindex_by_build_id(&cache, &buildid).await {
                Ok(()) => and_realise(cache.get_source(&buildid).await, "source").await,
                Err(e) => Err(e),
            }
        }
        source => source,
    };
    let source = source.with_context(|| format!("getting source of {} from cache", &buildid))?;
    let source = match source {
        None => {
            tracing::debug!("no source found for buildid {}", &buildid);
            return Ok(None);
        }
        Some(x) => PathBuf::from(x),
    };
    tracing::debug!(
        "found source store path for buildid {} at {}",
        &buildid,
        source.display()
    );
    let file =
        tokio::task::spawn_blocking(move || get_file_for_source(source.as_ref(), request.as_ref()))
            .await?
            .context("looking in source")?;
    Ok(file)
}

/// reads a file inside an archive into an http response
async fn uncompress_archive_file_to_http_body(
    archive: &std::path::Path,
    member: &std::path::Path,
) -> anyhow::Result<impl IntoResponse> {
    let archive_file = tokio::fs::File::open(&archive)
        .await
        .with_context(|| format!("opening source archive {}", archive.display()))?;
    let member_path = member
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non utf8 archive name"))?
        .to_string();
    let (asyncwriter, asyncreader) = tokio::io::duplex(256 * 1024);
    let streamreader = tokio_util::io::ReaderStream::new(asyncreader);
    let archive = archive.to_path_buf();
    let member = member.to_path_buf();
    let decompressor_future = async move {
        if let Err(e) = compress_tools::tokio_support::uncompress_archive_file(
            archive_file,
            asyncwriter,
            &member_path,
        )
        .await
        {
            tracing::error!(
                "expanding {} from {}: {:#}",
                member.display(),
                archive.display(),
                e
            );
        }
    };
    tokio::spawn(decompressor_future);
    Ok(StreamBody::new(streamreader))
}

#[axum_macros::debug_handler]
async fn get_source(
    Path(param): Path<(String, String)>,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(state.watcher, INDEXING_TIMEOUT).await;
    let path: &str = &param.1;
    let request = PathBuf::from(path);
    let sourcefile = fetch_and_get_source(param.0.to_owned(), request, state.cache).await;
    let response = match sourcefile {
        Ok(Some(SourceLocation::File(path))) => match tokio::fs::File::open(&path).await {
            Err(e) => Err((
                StatusCode::NOT_FOUND,
                format!("opening {}: {:#}", path.display(), e),
            )),
            Ok(file) => {
                let mut headers = HeaderMap::new();
                if let Ok(metadata) = path.metadata() {
                    if let Ok(value) = metadata.size().to_string().parse() {
                        headers.insert(CONTENT_LENGTH, value);
                    }
                }
                tracing::info!("returning {}", path.display());
                // convert the `AsyncRead` into a `Stream`
                let stream = ReaderStream::new(file);
                // convert the `Stream` into an `axum::body::HttpBody`
                let body = StreamBody::new(stream);
                Ok((headers, body).into_response())
            }
        },
        Ok(Some(SourceLocation::Archive {
            ref archive,
            ref member,
        })) => match uncompress_archive_file_to_http_body(&archive, &member).await {
            Ok(r) => {
                tracing::info!("returning {} from {}", member.display(), archive.display());
                Ok(r.into_response())
            }
            Err(e) => Err((StatusCode::NOT_FOUND, format!("{:#}", e))),
        },
        Ok(None) => Err((
            if ready {
                StatusCode::NOT_FOUND
            } else {
                NON_CACHING_ERROR_STATUS
            },
            "not found in cache".to_string(),
        )),
        Err(e) => Err((StatusCode::NOT_FOUND, format!("{:#}", e))),
    };
    if let Err((code, error)) = &response {
        tracing::info!("Responding error {}: {}", code, error);
    };
    response
}

async fn get_section(Path(_param): Path<(String, String)>) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

async fn get_substituters() -> anyhow::Result<Vec<Box<dyn Substituter>>> {
    let config = crate::config::get_nix_config()
        .await
        .context("determining the list of substituters")?;
    let mut urls = HashSet::new();
    for key in &["substituters", "trusted-substituters"] {
        let several = config.get(*key).map(|s| s.as_str()).unwrap_or("");
        for word in several.split(" ") {
            if word.len() != 0 {
                urls.insert(word);
            }
        }
    }
    tracing::debug!("found substituters {urls:?} in nix.conf");
    let mut substituters: Vec<Box<dyn Substituter>> = vec![];
    for url in urls.iter() {
        match FileSubstituter::from_url(url).await {
            Ok(Some(s)) => {
                tracing::debug!("using substituter {} for hydra API", s.url());
                substituters.push(Box::new(s));
                continue;
            }
            Err(e) => tracing::warn!("substituter url {url} has a problem: {e:#}"),
            Ok(None) => tracing::debug!("substituter {url} is not supported by file:// backend"),
        }
        match HttpSubstituter::from_url(url).await {
            Ok(Some(s)) => {
                tracing::debug!("using substituter {} for hydra API", s.url());
                substituters.push(Box::new(s));
            }
            Err(e) => tracing::warn!("substituter url {url} has a problem: {e:#}"),
            Ok(None) => tracing::debug!("substituter {url} is not supported by https:// backend"),
        }
    }
    Ok(substituters)
}

/// If option `-i` is specified, index and exit. Otherwise starts indexation and runs the
/// debuginfod server.
pub async fn run_server(args: Options) -> anyhow::Result<ExitCode> {
    let cache = Cache::open().await.context("opening global cache")?;
    let watcher = StoreWatcher::new(cache.clone());
    if args.index_only {
        match watcher.maybe_index_new_paths().await? {
            None => (),
            Some(handle) => handle.await?,
        };
        Ok(ExitCode::SUCCESS)
    } else {
        watcher.watch_store();
        let substituters = get_substituters().await.context("listing substituters")?;
        let state = ServerState {
            watcher,
            cache,
            substituters: Arc::new(substituters),
        };
        let app = Router::new()
            .route("/buildid/:buildid/section/:section", get(get_section))
            .route("/buildid/:buildid/source/*path", get(get_source))
            .route("/buildid/:buildid/executable", get(get_executable))
            .route("/buildid/:buildid/debuginfo", get(get_debuginfo))
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .with_state(state);
        axum::Server::bind(&args.listen_address)
            .serve(app.into_make_service())
            .await?;
        Ok(ExitCode::SUCCESS)
    }
}
