// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

//! Utilities to scan new store paths for buildids as they appear and populate the cache with them

use crate::db::{Cache, Entry, Id};
use crate::log::ResultExt;
use crate::store::{get_store_path, index_store_path};
use anyhow::Context;
use futures_util::{future::join_all, stream::FuturesOrdered, FutureExt, StreamExt};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::{mpsc::Sender, Semaphore};
use tokio::task::JoinHandle;

/// enqueue indexing of this many store paths at the same time
const BATCH_SIZE: usize = 100;
/// index at most thie many store paths at the same time
const N_WORKERS: usize = 8;

#[derive(Clone)]
/// A helper to examine all new store paths in parallel.
///
/// Cloning this structure returns a structure referring to the same internal state.
pub struct StoreWatcher {
    cache: Cache,
    /// semaphore to prevent indexing too many store path at the same time
    ///
    /// this prevents too many open file errors
    semaphore: Arc<Semaphore>,
    /// Locked when self.index_new_paths is running.
    working: Arc<Mutex<()>>,
}

impl StoreWatcher {
    /// Creates a [`StoreWatcher`] that populates the specified cache.
    ///
    /// To start it call [StoreWatcher::watch_store].
    pub fn new(cache: Cache) -> Self {
        Self {
            cache,
            semaphore: Arc::new(Semaphore::new(N_WORKERS)),
            working: Arc::new(Mutex::new(())),
        }
    }

    /// Index new store paths if there are new store paths.
    ///
    /// If there are none, returns Ok(None).
    /// If there are some, starts a future to index them, and returns a JoinHandle to
    /// optionnally wait for completion of the indexation.
    pub async fn maybe_index_new_paths(&self) -> anyhow::Result<Option<JoinHandle<()>>> {
        let start = self
            .cache
            .get_next_id()
            .await
            .context("reading cache next id")?;
        let (paths, end) = get_new_store_path_batch(start)
            .await
            .context("looking for new paths registered in the nix store")?;
        if paths.is_empty() {
            Ok(None)
        } else {
            let cloned_self = self.clone();
            Ok(Some(tokio::spawn(async move {
                let guard = cloned_self.working.lock().await;
                // it's possible that we had to wait a lot for this lock and that more indexation
                // was done in between.
                match cloned_self.cache.get_next_id().await {
                    Err(e) => tracing::warn!(
                        "reading next id from sqlite db: {:#}, dropping indexation request",
                        e
                    ),
                    Ok(new_start) => {
                        if new_start == start {
                            cloned_self.index_new_paths(paths, end).await;
                        } else {
                            // indexation was already in progress when we started waiting for the
                            // lock. Now that we got the lock, indexation is complete and there is
                            // nothing to do.
                            tracing::info!("indexation already complete");
                        }
                    }
                }
                drop(guard);
            })))
        }
    }

    /// Indexes a single store path, and sends found buildids to this sender
    async fn index_store_path(&self, path: PathBuf, sendto: Sender<Entry>) {
        let path2 = path.clone();
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("closed semaphore");
        tokio::task::spawn_blocking(move || {
            index_store_path(path.as_path(), sendto, true);
            drop(permit);
        })
        .await
        .with_context(|| format!("examining {} failed", path2.as_path().display()))
        .or_warn();
    }

    /// Indexes all new store paths in the store by batches.
    ///
    /// Arguments are the first batch, as returned by [get_new_store_path_batch]
    async fn index_new_paths(&self, paths: Vec<PathBuf>, id: Id) {
        if paths.is_empty() {
            return;
        };
        tracing::info!("Starting indexation of new store paths");
        let start = self.cache.get_next_id().await.unwrap_or(0);
        if start >= id {
            tracing::error!(
                size = paths.len(),
                end = id,
                start = start,
                "impossible batch"
            );
            return;
        }
        tracing::debug!(size = paths.len(), end = id, start = start, "First batch");
        let (entries_tx, mut entries_rx) = tokio::sync::mpsc::channel(3 * BATCH_SIZE);
        let batch: Vec<_> = paths
            .into_iter()
            .map(|path| self.index_store_path(path, entries_tx.clone()))
            .collect();
        let batch_handle = join_all(batch).map(move |_| id).boxed();
        let mut max_id = id;
        let mut unfinished_batches = FuturesOrdered::new();
        unfinished_batches.push_back(batch_handle);
        let mut entry_buffer = Vec::with_capacity(BATCH_SIZE);
        let mut get_new_batches = true;
        loop {
            tokio::select! {
                entry = entries_rx.recv() => {
                    match entry {
                        Some(entry) => {
                            entry_buffer.push(entry);
                            if entry_buffer.len() >= BATCH_SIZE {
                                match self.cache.register(&entry_buffer).await {
                                    Ok(()) => entry_buffer.clear(),
                                    Err(e) => tracing::warn!("cannot write entries to sqlite db: {:#}", e),
                                }
                            }
                        },
                        None => tracing::warn!("entries_rx closed"),
                    }
                }
                id = unfinished_batches.next() => {
                    match id {
                        Some(id) => {
                            match self.cache.register(&entry_buffer).await {
                                Ok(()) => {
                                    entry_buffer.clear();
                                    self.cache.set_next_id(id).await.context("writing next id").or_warn();
                                    tracing::debug!("batch {} complete", id);
                                },
                                Err(e) => tracing::warn!("cannot write entries to sqlite db: {:#}", e),
                            }
                        },
                        None => {
                            // there are no more running batches
                            self.cache.register(&entry_buffer).await.context("registering entries").or_warn();
                            entry_buffer.clear();
                            tracing::info!("Done indexing new store paths");
                            return;
                        },
                    }
                }
            }
            if get_new_batches && self.semaphore.available_permits() > 0 {
                tracing::debug!("considering starting a new batch of store paths to index");
                let (paths, id) = match get_new_store_path_batch(max_id).await {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!("cannot read nix store db: {:#}", e);
                        continue;
                    }
                };
                let batch: Vec<_> = paths
                    .into_iter()
                    .map(|path| self.index_store_path(path, entries_tx.clone()))
                    .collect();
                if batch.is_empty() {
                    tracing::debug!("batch is empty");
                    get_new_batches = false;
                } else {
                    tracing::debug!(
                        size = batch.len(),
                        start = max_id,
                        end = id,
                        "Indexing new batch of paths"
                    );
                    let batch_handle = join_all(batch).map(move |_| id).boxed();
                    max_id = id;
                    unfinished_batches.push_back(batch_handle);
                }
            }
        }
    }

    /// starts a task that periodically indexes new store paths in the store.
    ///
    /// Returns immediately.
    pub fn watch_store(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                match self_clone.maybe_index_new_paths().await {
                    Ok(None) => tokio::time::sleep(Duration::from_secs(60)).await,
                    Ok(Some(handle)) => {
                        handle.await.context("waiting for indexation").or_warn();
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    Err(e) => {
                        tracing::warn!("while watching store for new paths: {:#}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }
}

/// Reads the nix db to find new store paths.
///
/// New store paths are paths of id greater or equal to `from_id`.
///
/// Returns the id you should call this function with for the "next" paths.
async fn get_new_store_path_batch(from_id: Id) -> anyhow::Result<(Vec<PathBuf>, Id)> {
    // note: this is a hack. One cannot open a sqlite db read only with WAL if the underlying
    // file is not writable. So we promise sqlite that the db will not be modified with
    // immutable=1, but it's false.
    let mut db = SqliteConnectOptions::new()
        .filename("/nix/var/nix/db/db.sqlite")
        .immutable(true)
        .read_only(true)
        .connect()
        .await
        .context("opening nix db")?;
    let rows =
        sqlx::query("select path, id from ValidPaths where id >= $1 order by id asc limit $2")
            .bind(from_id)
            .bind(BATCH_SIZE as u32)
            .fetch_all(&mut db)
            .await
            .context("reading nix db")?;
    let mut paths = Vec::new();
    let mut max_id = 0;
    for row in rows {
        let path: &str = row.try_get("path").context("parsing path in nix db")?;
        let path = match get_store_path(Path::new(path)) {
            Some(path) => path,
            None => anyhow::bail!(
                "read corrupted stuff from nix db: {}, concurrent write?",
                path
            ),
        };
        paths.push(PathBuf::from(path));
        let id: Id = row.try_get("id").context("parsing id in nix db")?;
        max_id = id.max(max_id);
    }
    // As we lie about the database being immutable let's not keep the connection open
    db.close().await.context("closing nix db").or_warn();
    if (max_id == 0) ^ paths.is_empty() {
        anyhow::bail!("read paths with id == 0...");
    }
    Ok((paths, max_id + 1))
}

/// Index this path, but harder than automatic indexation
///
/// Specifically, this is allowed to download the .drv file from a cache.
pub async fn index_store_path_online(cache: &Cache, path: &Path) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(BATCH_SIZE);
    let path = path.to_path_buf();
    let handle = tokio::task::spawn_blocking(move || index_store_path(&path, tx, false));
    let mut batch = Vec::new();
    while let Some(entry) = rx.recv().await {
        batch.push(entry);
        if batch.len() > BATCH_SIZE {
            cache
                .register(&batch)
                .await
                .context("registering new entries")?;
            batch.clear();
        }
    }
    cache
        .register(&batch)
        .await
        .context("registering new entries")?;
    handle.await?;
    Ok(())
}
