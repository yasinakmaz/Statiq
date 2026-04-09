use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::OdbcParam;
use crate::row::OdbcRow;

/// Full CRUD + batch + raw interface.
///
/// Implemented by `SqlService<T>` — kept as a trait so you can mock it in tests.
#[async_trait]
pub trait SqlRepository<T: SqlEntity>: Send + Sync {
    // ── Single ────────────────────────────────────────────────────────────────
    async fn get_by_id(&self, id: impl Into<crate::params::PkValue> + Send, token: &CancellationToken) -> Result<Option<T>, SqlError>;
    async fn get_all(&self, token: &CancellationToken) -> Result<Vec<T>, SqlError>;
    async fn get_where(&self, filter: &str, params: &[OdbcParam], token: &CancellationToken) -> Result<Vec<T>, SqlError>;
    async fn get_paged(&self, page: i64, page_size: i64, token: &CancellationToken) -> Result<Vec<T>, SqlError>;
    async fn count(&self, token: &CancellationToken) -> Result<i64, SqlError>;
    async fn exists(&self, id: impl Into<crate::params::PkValue> + Send, token: &CancellationToken) -> Result<bool, SqlError>;

    async fn insert(&self, entity: &T, token: &CancellationToken) -> Result<i64, SqlError>;
    async fn update(&self, entity: &T, token: &CancellationToken) -> Result<(), SqlError>;
    async fn delete(&self, id: impl Into<crate::params::PkValue> + Send, token: &CancellationToken) -> Result<(), SqlError>;
    async fn upsert(&self, entity: &T, token: &CancellationToken) -> Result<(), SqlError>;

    // ── Batch ─────────────────────────────────────────────────────────────────
    async fn batch_insert(&self, entities: &[T], token: &CancellationToken) -> Result<Vec<i64>, SqlError>;
    async fn batch_update(&self, entities: &[T], token: &CancellationToken) -> Result<(), SqlError>;
    async fn batch_delete(&self, ids: &[crate::params::PkValue], token: &CancellationToken) -> Result<(), SqlError>;

    // ── Raw ───────────────────────────────────────────────────────────────────
    async fn query_raw(&self, sql: &str, params: &[OdbcParam], token: &CancellationToken) -> Result<Vec<OdbcRow>, SqlError>;
    async fn execute_raw(&self, sql: &str, params: &[OdbcParam], token: &CancellationToken) -> Result<usize, SqlError>;
    async fn scalar<S: TryFrom<String> + Send>(&self, sql: &str, params: &[OdbcParam], token: &CancellationToken) -> Result<S, SqlError>
    where
        <S as TryFrom<String>>::Error: std::fmt::Display;
}
