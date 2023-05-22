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
use std::os::unix::prelude::MetadataExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio_util::io::ReaderStream;

use crate::db::Cache;
use crate::index::{index_store_path_online, StoreWatcher};
use crate::store::{get_file_for_source, get_store_path, realise, SourceLocation};
use crate::Options;

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
            let exists = realise(p.as_ref()).await;
            match exists {
                Ok(()) => match tokio::fs::File::open(p.as_ref()).await {
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
                },
                Err(e) => Err((StatusCode::NOT_FOUND, format!("{:#}", e))),
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
    index_store_path_online(cache, storepath)
        .await
        .with_context(|| format!("indexing {} online", exe.display()))?;
    Ok(())
}

/// How long to wait for indexation to complete before serving the cache
const INDEXING_TIMEOUT: Duration = Duration::from_secs(1);

async fn get_debuginfo(
    Path(buildid): Path<String>,
    State((cache, watcher)): State<(Cache, StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let res = cache.get_debuginfo(&buildid).await;
    let res = match res {
        Ok(None) => {
            // try again harder
            match maybe_reindex_by_build_id(&cache, &buildid).await {
                Ok(()) => cache.get_debuginfo(&buildid).await,
                Err(e) => Err(e),
            }
        }
        res => res,
    };
    unwrap_file(res, ready).await
}

async fn get_executable(
    Path(buildid): Path<String>,
    State((cache, watcher)): State<(Cache, StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let res = cache.get_executable(&buildid).await;
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
    let source = match source {
        Ok(None) => {
            // try again harder
            match maybe_reindex_by_build_id(&cache, &buildid).await {
                Ok(()) => cache.get_source(&buildid).await,
                Err(e) => Err(e),
            }
        }
        source => source,
    };
    let source = source.with_context(|| format!("getting source of {} from cache", &buildid))?;
    let source = match source {
        None => return Ok(None),
        Some(x) => PathBuf::from(x),
    };
    realise(source.as_ref())
        .await
        .with_context(|| format!("realizing source {}", source.display()))?;
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
async fn get_source(
    Path(param): Path<(String, String)>,
    State((cache, watcher)): State<(Cache, StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let path: &str = &param.1;
    let request = PathBuf::from(path);
    let sourcefile = fetch_and_get_source(param.0.to_owned(), request, cache).await;
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
        let app = Router::new()
            .route("/buildid/:buildid/section/:section", get(get_section))
            .route("/buildid/:buildid/source/*path", get(get_source))
            .route("/buildid/:buildid/executable", get(get_executable))
            .route("/buildid/:buildid/debuginfo", get(get_debuginfo))
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .with_state((cache, watcher));
        axum::Server::bind(&args.listen_address)
            .serve(app.into_make_service())
            .await?;
        Ok(ExitCode::SUCCESS)
    }
}
