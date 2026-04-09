pub mod noop;
pub mod redis;

use std::time::Duration;
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use crate::error::SqlError;

pub use noop::NoCache;
pub use redis::RedisCache;

/// Strategy interface for caching — implemented by `RedisCache` and `NoCache`.
#[async_trait]
pub trait CacheLayer: Send + Sync + 'static {
    fn default_ttl(&self) -> Duration;
    fn count_ttl(&self) -> Duration;

    async fn get<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, SqlError>;
    async fn set<T: Serialize + Send + Sync>(&self, key: &str, value: &T, ttl: Duration) -> Result<(), SqlError>;

    async fn get_vec<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<Vec<T>>, SqlError>;
    async fn set_vec<T: Serialize + Send + Sync>(&self, key: &str, values: &[T], ttl: Duration) -> Result<(), SqlError>;

    async fn get_scalar<T: DeserializeOwned + Send>(&self, key: &str) -> Result<Option<T>, SqlError>;
    async fn set_scalar<T: Serialize + Send + Sync>(&self, key: &str, value: T, ttl: Duration) -> Result<(), SqlError>;

    /// Delete a single entity entry (e.g. `GetById::42`).
    async fn invalidate_entry(&self, prefix: &str, id: &str) -> Result<(), SqlError>;

    /// Delete all keys under a table prefix (Count, GetAll, etc.).
    async fn invalidate_table(&self, prefix: &str) -> Result<(), SqlError>;
}
