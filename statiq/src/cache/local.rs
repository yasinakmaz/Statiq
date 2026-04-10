//! In-process cache backed by [Moka](https://docs.rs/moka).
//!
//! Enable with the `local-cache` feature flag.
//!
//! Unlike `RedisCache`, `LocalCache` is process-local — it does not survive
//! restarts and is not shared across server instances. Useful for single-process
//! deployments or as a fast L1 layer in front of Redis.

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use moka::future::Cache;
use serde::{Serialize, de::DeserializeOwned};

use crate::cache::CacheLayer;
use crate::error::SqlError;

/// In-process cache backed by Moka.
#[derive(Clone)]
pub struct LocalCache {
    inner: Cache<String, Bytes>,
    default_ttl: Duration,
    count_ttl: Duration,
}

impl LocalCache {
    /// Create a new `LocalCache`.
    ///
    /// - `max_capacity`: maximum number of entries to hold in memory.
    /// - `default_ttl`: TTL for entity entries.
    /// - `count_ttl`: TTL for scalar/count entries.
    pub fn new(max_capacity: u64, default_ttl: Duration, count_ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(default_ttl)
            .build();

        Self { inner, default_ttl, count_ttl }
    }

    fn serialize<T: Serialize>(value: &T) -> Result<Bytes, SqlError> {
        let json = serde_json::to_vec(value)?;
        Ok(Bytes::from(json))
    }

    fn deserialize<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, SqlError> {
        serde_json::from_slice(bytes).map_err(SqlError::from)
    }
}

#[async_trait]
impl CacheLayer for LocalCache {
    fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    fn count_ttl(&self) -> Duration {
        self.count_ttl
    }

    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, SqlError> {
        match self.inner.get(key).await {
            Some(b) => Ok(Some(Self::deserialize(&b)?)),
            None => Ok(None),
        }
    }

    async fn set<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        _ttl: Duration,
    ) -> Result<(), SqlError> {
        let bytes = Self::serialize(value)?;
        self.inner.insert(key.to_owned(), bytes).await;
        Ok(())
    }

    async fn get_vec<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<Vec<T>>, SqlError> {
        self.get::<Vec<T>>(key).await
    }

    async fn set_vec<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        values: &[T],
        _ttl: Duration,
    ) -> Result<(), SqlError> {
        let bytes = Self::serialize(&values)?;
        self.inner.insert(key.to_owned(), bytes).await;
        Ok(())
    }

    async fn get_scalar<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, SqlError> {
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
        let key = format!("{prefix}::{id}");
        self.inner.invalidate(&key).await;
        Ok(())
    }

    async fn invalidate_table(&self, prefix: &str) -> Result<(), SqlError> {
        // Moka does not support prefix scan — invalidate known patterns.
        for suffix in &["GetAll", "Count", "GetById"] {
            self.inner.invalidate(&format!("{prefix}::{suffix}")).await;
        }
        // Run pending invalidation tasks.
        self.inner.run_pending_tasks().await;
        Ok(())
    }
}
