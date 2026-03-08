use std::time::Duration;
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use crate::error::SqlError;
use super::CacheLayer;

/// No-op cache — all reads return `None`, all writes succeed silently.
/// Used when Redis is disabled.
pub struct NoCache;

#[async_trait]
impl CacheLayer for NoCache {
    fn default_ttl(&self) -> Duration { Duration::from_secs(300) }
    fn count_ttl(&self) -> Duration { Duration::from_secs(60) }

    async fn get<T: DeserializeOwned + Send>(&self, _key: &str) -> Result<Option<T>, SqlError> {
        Ok(None)
    }

    async fn set<T: Serialize + Send + Sync>(&self, _key: &str, _value: &T, _ttl: Duration) -> Result<(), SqlError> {
        Ok(())
    }

    async fn get_vec<T: DeserializeOwned + Send>(&self, _key: &str) -> Result<Option<Vec<T>>, SqlError> {
        Ok(None)
    }

    async fn set_vec<T: Serialize + Send + Sync>(&self, _key: &str, _values: &[T], _ttl: Duration) -> Result<(), SqlError> {
        Ok(())
    }

    async fn get_scalar<T: DeserializeOwned + Send>(&self, _key: &str) -> Result<Option<T>, SqlError> {
        Ok(None)
    }

    async fn set_scalar<T: Serialize + Send + Sync>(&self, _key: &str, _value: T, _ttl: Duration) -> Result<(), SqlError> {
        Ok(())
    }

    async fn invalidate_entry(&self, _prefix: &str, _id: &str) -> Result<(), SqlError> {
        Ok(())
    }

    async fn invalidate_table(&self, _prefix: &str) -> Result<(), SqlError> {
        Ok(())
    }
}
