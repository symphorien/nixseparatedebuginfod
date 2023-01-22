// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

use anyhow::{bail, Context};
use directories::ProjectDirs;
use sha2::Digest;
use sqlx::{sqlite::SqlitePool, Row};

/// Store path id
pub type Id = u32;

/// An entry stored in the cache.
///
/// `executable` is the full path to the executable of this buildid (executable includes .so).
/// `debuginfo` is the full path to an elf object containing debuginfo.
/// `source` is the store path of the source, either directory or archive.
#[derive(Debug, Clone)]
pub struct Entry {
    pub buildid: String,
    pub executable: Option<String>,
    pub debuginfo: Option<String>,
    pub source: Option<String>,
}

/// A cache storing the executable and debuginfo location for each buildid.
#[derive(Clone)]
pub struct Cache {
    /// A connection to a backing sqlite db.
    sqlite: SqlitePool,
}
/// The schema of the sqlite db backing `Cache`.
const SCHEMA: &'static str = include_str!("./schema.sql");

fn get_schema_version() -> u32 {
    let mut hasher = sha2::Sha256::new();
    hasher.update(SCHEMA.as_bytes());
    let hash = hasher.finalize();
    u32::from_le_bytes(hash[0..4].try_into().unwrap())
}

/// Checks wether this db has the right schema version
async fn pool_is_valid(pool: &SqlitePool) -> anyhow::Result<()> {
    let row = sqlx::query("select version from version")
        .fetch_one(pool)
        .await
        .context("reading schema version")?;
    let version: u32 = row
        .try_get("version")
        .context("reading schema version first row")?;
    if version != get_schema_version() {
        bail!("incompatible cache version {}", version);
    }
    Ok(())
}

/// Sets the schema on a empty db, and populate single row tables.
async fn populate_pool(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut transaction = pool
        .begin()
        .await
        .context("opening transaction to set schema on cache db")?;
    sqlx::query(SCHEMA)
        .execute(&mut transaction)
        .await
        .context("setting schema on cache db")?;
    sqlx::query("insert into version values ($1);")
        .bind(get_schema_version())
        .execute(&mut transaction)
        .await
        .context("setting schema version on cache db")?;
    sqlx::query("insert into gc values (0);")
        .execute(&mut transaction)
        .await
        .context("setting schema default timestamps on cache db")?;
    sqlx::query("insert into id values (0);")
        .execute(&mut transaction)
        .await
        .context("setting schema default next id on cache db")?;
    transaction.commit().await?;
    Ok(())
}

impl Cache {
    /// Attempts to open the cache from disk. Does not try very hard.
    async fn open_weak() -> anyhow::Result<Cache> {
        let dirs = ProjectDirs::from("eu", "xlumurb", "nixseparatedebuginfod");
        let dirs = match dirs {
            Some(d) => d,
            None => bail!("could not determine cache dir in $HOME"),
        };
        let mut path = dirs.cache_dir().to_owned();
        std::fs::create_dir_all(&path)
            .with_context(|| format!("creating cache directory {}", path.display()))?;
        path.push("cache.sqlite3");
        let path_utf8 = match path.to_str() {
            Some(p) => p,
            None => bail!("cache path {} is not utf8", path.display()),
        };
        let url = format!("file:{}?mode=rwc", path_utf8);
        let pool = SqlitePool::connect(&url)
            .await
            .with_context(|| format!("failed to connect to {} with sqlite3", &url))?;
        let pool = match pool_is_valid(&pool).await {
            Ok(()) => pool,
            Err(e) => {
                tracing::warn!("cache {} is invalid, wiping it. {:#}", path.display(), e);
                pool.close().await;
                std::fs::remove_file(&path).unwrap_or_else(|e| {
                    tracing::warn!("error removing corrupted cache {}: {:#}", path.display(), e)
                });
                let pool = SqlitePool::connect(&url)
                    .await
                    .with_context(|| format!("failed to connect to {} with sqlite3", &url))?;
                populate_pool(&pool)
                    .await
                    .context("populating empty cache")?;
                pool
            }
        };
        Ok(Cache { sqlite: pool })
    }

    /// Opens a cache, either from disk, or it it fails, in memory.
    pub async fn open() -> anyhow::Result<Cache> {
        match Cache::open_weak().await {
            Err(e) => {
                tracing::warn!("could not use on disk cache ({:#}), running cache in memory", e);
                let pool = SqlitePool::connect(":memory:")
                    .await
                    .context("opening in memory sql db")?;
                populate_pool(&pool)
                    .await
                    .context("populating empty cache")?;
                Ok(Cache { sqlite: pool })
            }
            Ok(cache) => Ok(cache),
        }
    }

    /// Get the path of an elf object containing debuginfo for this buildid.
    ///
    /// The path may have been gc-ed, you are responsible to ensure it exists.
    pub async fn get_debuginfo(&self, buildid: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("select debuginfo from builds where buildid = $1;")
            .bind(buildid)
            .fetch_optional(&self.sqlite)
            .await
            .context("reading debuginfo from cache db")?;
        Ok(match row {
            None => None,
            Some(r) => r.try_get("debuginfo")?,
        })
    }

    /// Get the path of an elf object containing text for this buildid.
    ///
    /// The path may have been gc-ed, you are responsible to ensure it exists.
    pub async fn get_executable(&self, buildid: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("select executable from builds where buildid = $1;")
            .bind(buildid)
            .fetch_optional(&self.sqlite)
            .await
            .context("reading executable from cache db")?;
        Ok(match row {
            None => None,
            Some(r) => r.try_get("executable")?,
        })
    }

    /// Get the store path where the source of this buildid is.
    ///
    /// The path may have been gc-ed, you are responsible to ensure it exists.
    pub async fn get_source(&self, buildid: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query("select source from builds where buildid = $1;")
            .bind(buildid)
            .fetch_optional(&self.sqlite)
            .await
            .context("reading executable from cache db")?;
        Ok(match row {
            None => None,
            Some(r) => r.try_get("source")?,
        })
    }

    /// Register information for a buildid
    ///
    /// Only one of the each entry fields is stored for each buildid, if register is called serval times
    /// for a single buildid, only the latest `Some` provided one is retained.
    pub async fn register(&self, entries: &[Entry]) -> anyhow::Result<()> {
        if entries.len() == 0 {
            return Ok(());
        }
        let mut transaction = self.sqlite.begin().await.context("transaction sqlite")?;
        for entry in entries {
            sqlx::query(
                "insert into builds
                    values ($1, $2, $3, $4)
                    on conflict(buildid) do update set
                    executable = coalesce(excluded.executable, executable),
                    debuginfo = coalesce(excluded.debuginfo, debuginfo),
                    source = coalesce(excluded.source, source)
                    ;",
            )
            .bind(&entry.buildid)
            .bind(&entry.executable)
            .bind(&entry.debuginfo)
            .bind(&entry.source)
            .execute(&mut transaction)
            .await
            .context("inserting build")?;
        }
        transaction
            .commit()
            .await
            .context("committing entry insert")?;
        Ok(())
    }

    /// Store the next store path id to read from the nix db
    pub async fn set_next_id(&self, id: Id) -> anyhow::Result<()> {
        sqlx::query("update id set next = max(next, $1);")
            .bind(id)
            .execute(&self.sqlite)
            .await
            .context("advancing next registered id in cache db")?;
        Ok(())
    }

    /// get the next store path id to read from the nix db
    pub async fn get_next_id(&self) -> anyhow::Result<Id> {
        let row = sqlx::query("select next from id")
            .fetch_one(&self.sqlite)
            .await
            .context("reading next registered id in cache db")?;
        Ok(row
            .try_get("next")
            .context("parsing next registered id from cache db")?)
    }
}
