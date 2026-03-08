use std::time::Duration;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands, Client};
use serde::{de::DeserializeOwned, Serialize};
use tracing::debug;

use crate::config::RedisConfig;
use crate::error::SqlError;
use super::CacheLayer;

pub struct RedisCache {
    manager: ConnectionManager,
    default_ttl: Duration,
    count_ttl: Duration,
}

impl RedisCache {
    pub async fn new(cfg: &RedisConfig) -> Result<Self, SqlError> {
        let client = Client::open(cfg.url.as_str())?;
        let manager = ConnectionManager::new(client).await?;
        Ok(Self {
            manager,
            default_ttl: Duration::from_secs(cfg.default_ttl_secs),
            count_ttl: Duration::from_secs(cfg.count_ttl_secs),
        })
    }

    fn get_manager(&self) -> ConnectionManager {
        self.manager.clone()
    }
}

#[async_trait]
impl CacheLayer for RedisCache {
    fn default_ttl(&self) -> Duration { self.default_ttl }
    fn count_ttl(&self) -> Duration { self.count_ttl }

    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, SqlError> {
        let mut conn = self.get_manager();
        let raw: Option<String> = conn.get(key).await?;
        match raw {
            None => Ok(None),
            Some(json) => {
                debug!(key, "Cache hit");
                let v: T = serde_json::from_str(&json)?;
                Ok(Some(v))
            }
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<(), SqlError> {
        let mut conn = self.get_manager();
        let json = serde_json::to_string(value)?;
        let secs = ttl.as_secs();
        let _: () = conn.set_ex(key, json, secs).await?;
        debug!(key, ttl_secs = secs, "Cache set");
        Ok(())
    }

    async fn get_vec<T: DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> Result<Option<Vec<T>>, SqlError> {
        self.get::<Vec<T>>(key).await
    }

    async fn set_vec<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        values: &[T],
        ttl: Duration,
    ) -> Result<(), SqlError> {
        self.set(key, &values, ttl).await
    }

    async fn get_scalar<T: DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> Result<Option<T>, SqlError> {
        self.get::<T>(key).await
    }

    async fn set_scalar<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: T,
        ttl: Duration,
    ) -> Result<(), SqlError> {
        self.set(key, &value, ttl).await
    }

    async fn invalidate_entry(&self, prefix: &str, id: &str) -> Result<(), SqlError> {
        let mut conn = self.get_manager();
        let key = format!("{prefix}::GetById::{id}");
        let _: () = conn.del(&key).await?;
        debug!(key, "Cache invalidated entry");
        Ok(())
    }

    async fn invalidate_table(&self, prefix: &str) -> Result<(), SqlError> {
        let mut conn = self.get_manager();
        let pattern = format!("{prefix}::*");

        // Use SCAN cursor loop instead of KEYS to avoid blocking the Redis event loop.
        // SCAN is O(1) per call and incremental, safe under any keyspace size.
        let mut cursor: u64 = 0;
        let mut total_deleted = 0usize;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100u64)
                .query_async(&mut conn)
                .await?;

            if !keys.is_empty() {
                let mut pipe = redis::pipe();
                for key in &keys {
                    pipe.del(key);
                }
                let _: () = pipe.query_async(&mut conn).await?;
                total_deleted += keys.len();
            }

            cursor = next_cursor;
            if cursor == 0 {
                break; // full scan complete
            }
        }

        if total_deleted > 0 {
            debug!(pattern, count = total_deleted, "Cache invalidated table (SCAN)");
        }

        Ok(())
    }
}
