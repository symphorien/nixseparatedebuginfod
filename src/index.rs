use crate::db::{Cache, Entry, Timestamp};
use crate::log::ResultExt;
use crate::store::index_store_path;
use anyhow::Context;
use futures_util::{future::join_all, stream::FuturesOrdered, FutureExt, StreamExt};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::{mpsc::Sender, Semaphore};
use tokio::task::JoinHandle;

const BATCH_SIZE: usize = 100;
const N_WORKERS: usize = 8;

#[derive(Clone)]
/// A helper to examine all new store paths in parallel.
pub struct StoreWatcher {
    cache: &'static Cache,
    /// semaphore to prevent indexing too many store path at the same time
    ///
    /// this prevents too many open file errors
    semaphore: Arc<Semaphore>,
    /// Locked when self.index_new_paths is running.
    working: Arc<Mutex<()>>,
}

impl StoreWatcher {
    pub fn new(cache: &'static Cache) -> Self {
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
        let timestamp = self
            .cache
            .get_registration_timestamp()
            .await
            .context("reading cache timestamp")?;
        let (paths, timestamp) = get_new_store_path_batch(timestamp)
            .await
            .context("looking for new paths registered in the nix store")?;
        if paths.is_empty() {
            Ok(None)
        } else {
            let cloned_self = self.clone();
            Ok(Some(tokio::spawn(async move {
                let guard = cloned_self.working.lock().await;
                cloned_self.index_new_paths(paths, timestamp).await;
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
            index_store_path(path.as_path(), sendto);
            drop(permit);
        })
        .await
        .with_context(|| format!("examining {} failed", path2.as_path().display()))
        .or_warn();
    }

    /// Indexes all new store paths in the store by batches.
    ///
    /// Arguments are the first batch, as returned by [get_new_store_path_batch]
    async fn index_new_paths(&self, paths: Vec<PathBuf>, timestamp: Timestamp) {
        let (entries_tx, mut entries_rx) = tokio::sync::mpsc::channel(3 * BATCH_SIZE);
        let batch: Vec<_> = paths
            .into_iter()
            .map(|path| self.index_store_path(path, entries_tx.clone()))
            .collect();
        let batch_handle = join_all(batch).map(move |_| timestamp).boxed();
        let mut last_timestamp = timestamp;
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
                                    Err(e) => log::warn!("cannot write entries to sqlite db: {:#}", e),
                                }
                            }
                        },
                        None => log::warn!("entries_rx closed"),
                    }
                }
                timestamp = unfinished_batches.next() => {
                    match timestamp {
                        Some(timestamp) => {
                            match self.cache.register(&entry_buffer).await {
                                Ok(()) => {
                                    entry_buffer.clear();
                                    self.cache.set_registration_timestamp(timestamp).await.context("writing registration timestamp").or_warn();
                                    log::debug!("batch {} ok", timestamp);
                                },
                                Err(e) => log::warn!("cannot write entries to sqlite db: {:#}", e),
                            }
                        },
                        None => {
                            // there are no more running batches
                            self.cache.register(&entry_buffer).await.context("registering entries").or_warn();
                            entry_buffer.clear();
                            log::info!("Done registering new store paths");
                            return;
                        },
                    }
                }
            }
            if get_new_batches && self.semaphore.available_permits() > 0 {
                log::debug!("starting a new batch of store paths to index");
                let (paths, timestamp) = match get_new_store_path_batch(last_timestamp).await {
                    Ok(x) => x,
                    Err(e) => {
                        log::warn!("cannot read nix store db: {:#}", e);
                        continue;
                    }
                };
                let batch: Vec<_> = paths
                    .into_iter()
                    .map(|path| self.index_store_path(path, entries_tx.clone()))
                    .collect();
                if batch.is_empty() {
                    log::debug!("batch is empty");
                    get_new_batches = false;
                } else {
                    let batch_handle = join_all(batch).map(move |_| timestamp).boxed();
                    last_timestamp = timestamp;
                    unfinished_batches.push_back(batch_handle);
                }
            }
        }
    }

    /// starts a task that periodically indexes new store paths in the store
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
                        log::warn!("while watching store for new paths: {:#}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }
}

/// Reads the nix db to find new store paths register from this timestamp on
///
/// Returns the timestamp you should call this function with for the "next" paths.
async fn get_new_store_path_batch(
    from_timestamp: Timestamp,
) -> anyhow::Result<(Vec<PathBuf>, Timestamp)> {
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
        sqlx::query("select path, registrationTime from ValidPaths where registrationTime >= $1 and registrationTime <= (with candidates(registrationTime) as (select registrationTime from ValidPaths where registrationTime >= $1 order by registrationTime asc limit 100) select max(registrationTime) from candidates)")
            .bind(from_timestamp)
            .fetch_all(&mut db)
            .await
            .context("reading nix db")?;
    let mut paths = Vec::new();
    let mut max_time = 0;
    for row in rows {
        let path: &str = row.try_get("path").context("parsing path in nix db")?;
        if !path.starts_with("/nix/store") || path.chars().filter(|&x| x == '/').count() != 3 {
            anyhow::bail!(
                "read corrupted stuff from nix db: {}, concurrent write?",
                path
            );
        }
        paths.push(PathBuf::from(path));
        let time: Timestamp = row
            .try_get("registrationTime")
            .context("parsing timestamp in nix db")?;
        max_time = time.max(max_time);
    }
    // As we lie about the database being immutable let's not keep the connection open
    db.close().await.context("closing nix db").or_warn();
    if (max_time == 0) ^ paths.is_empty() {
        anyhow::bail!("read paths with 0 registration time...");
    }
    Ok((paths, max_time + 1))
}
