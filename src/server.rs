use anyhow::Context;
use axum::body::StreamBody;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{routing::get, Router};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::io::ReaderStream;

use crate::db::Cache;
use crate::index::StoreWatcher;
use crate::store::{get_file_for_source, realise, SourceLocation};
use crate::Options;

/// The only status code in the client code of debuginfod in elfutils that prevents
/// creation of a negative cache entry.
///
/// 503 Not Available also works, but only for the section request
const NON_CACHING_ERROR_STATUS: StatusCode = StatusCode::NOT_ACCEPTABLE;

async fn unwrap_file<T: AsRef<std::path::Path>>(
    path: anyhow::Result<Option<T>>,
    ready: bool,
) -> impl IntoResponse {
    match path {
        Ok(Some(p)) => {
            let exists = realise(p.as_ref()).await;
            match exists {
                Ok(()) => match tokio::fs::File::open(p.as_ref()).await {
                    Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
                    Ok(file) => {
                        // convert the `AsyncRead` into a `Stream`
                        let stream = ReaderStream::new(file);
                        // convert the `Stream` into an `axum::body::HttpBody`
                        let body = StreamBody::new(stream);
                        Ok(body)
                    }
                },
                Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
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
        Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
    }
}

/// Start indexation, and wait for it to complete until timeout.
///
/// Returns wether indexation is complete.
async fn start_indexation_and_wait(watcher: &StoreWatcher, timeout: Duration) -> bool {
    match watcher.maybe_index_new_paths().await {
        Err(e) => {
            log::warn!("cannot start registration of new store path: {:#}", e);
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

const INDEXING_TIMEOUT: Duration = Duration::from_secs(1);

async fn get_debuginfo(
    Path(buildid): Path<String>,
    State((cache, watcher)): State<(&'static Cache, &'static StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let res = cache.get_debuginfo(&buildid).await;
    unwrap_file(res, ready).await
}

async fn get_executable(
    Path(buildid): Path<String>,
    State((cache, watcher)): State<(&'static Cache, &'static StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let res = cache.get_executable(&buildid).await;
    unwrap_file(res, ready).await
}

async fn fetch_and_get_source(
    buildid: String,
    request: PathBuf,
    cache: &'static Cache,
) -> anyhow::Result<Option<SourceLocation>> {
    let source = cache
        .get_source(&buildid)
        .await
        .with_context(|| format!("getting source of {} from cache", &buildid))?;
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

async fn uncompress_archive_file_to_http_body(
    archive: PathBuf,
    member: PathBuf,
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
    let decompressor_future = async move {
        if let Err(e) = compress_tools::tokio_support::uncompress_archive_file(
            archive_file,
            asyncwriter,
            &member_path,
        )
        .await
        {
            log::error!(
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
    State((cache, watcher)): State<(&'static Cache, &'static StoreWatcher)>,
) -> impl IntoResponse {
    let ready = start_indexation_and_wait(watcher, INDEXING_TIMEOUT).await;
    let path: &str = &param.1;
    let request = PathBuf::from(path);
    let sourcefile = fetch_and_get_source(param.0.to_owned(), request, &cache).await;
    match sourcefile {
        Ok(Some(SourceLocation::File(path))) => match tokio::fs::File::open(&path).await {
            Err(e) => Err((
                StatusCode::NOT_FOUND,
                format!("opening {}: {:#}", path.display(), e),
            )),
            Ok(file) => {
                // convert the `AsyncRead` into a `Stream`
                let stream = ReaderStream::new(file);
                // convert the `Stream` into an `axum::body::HttpBody`
                let body = StreamBody::new(stream);
                Ok(body.into_response())
            }
        },
        Ok(Some(SourceLocation::Archive { archive, member })) => {
            match uncompress_archive_file_to_http_body(archive, member).await {
                Ok(r) => Ok(r.into_response()),
                Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
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
        Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
    }
}

async fn get_section(Path(_param): Path<(String, String)>) -> impl IntoResponse {
    StatusCode::NOT_IMPLEMENTED
}

pub async fn run_server(args: Options) -> anyhow::Result<()> {
    let cache = Cache::open().await.context("opening global cache")?;
    let cache: &'static Cache = Box::leak(Box::new(cache));
    let watcher = StoreWatcher::new(cache);
    let watcher: &'static StoreWatcher = Box::leak(Box::new(watcher));
    if args.index_only {
        match watcher.maybe_index_new_paths().await? {
            None => (),
            Some(handle) => handle.await?,
        }
    } else {
        watcher.watch_store();
        let app = Router::new()
            .route("/buildid/:buildid/section/:section", get(get_section))
            .route("/buildid/:buildid/source/*path", get(get_source))
            .route("/buildid/:buildid/executable", get(get_executable))
            .route("/buildid/:buildid/debuginfo", get(get_debuginfo))
            .with_state((cache, watcher));
        axum::Server::bind(&args.listen_address)
            .serve(app.into_make_service())
            .await?;
    }
    Ok(())
}
