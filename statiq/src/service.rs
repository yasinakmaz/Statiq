use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::cache::CacheLayer;
use crate::config::QueryConfig;
use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::{OdbcParam, PkValue};
use crate::pool::{Pool, PooledConn};
use crate::query::{filtered_sql, paged_sql};
use crate::repository::SqlRepository;
use crate::row::OdbcRow;
use crate::transaction::Transaction;

/// Main entry point — wraps a pool + cache for entity `T`.
pub struct SqlService<T: SqlEntity, C: CacheLayer = crate::cache::NoCache> {
    pub(crate) pool: Pool,
    pub(crate) cache: Arc<C>,
    pub(crate) query_cfg: QueryConfig,
    _marker: std::marker::PhantomData<T>,
}

impl<T: SqlEntity, C: CacheLayer> SqlService<T, C> {
    pub(crate) fn new(pool: Pool, cache: C, query_cfg: QueryConfig) -> Self {
        Self {
            pool,
            cache: Arc::new(cache),
            query_cfg,
            _marker: std::marker::PhantomData,
        }
    }

    async fn checkout(&self, token: &CancellationToken) -> Result<PooledConn, SqlError> {
        self.pool.checkout(token).await
    }

    fn slow_warn(&self, sql: &str, elapsed_ms: u64) {
        if elapsed_ms >= self.query_cfg.slow_query_threshold_ms {
            warn!(
                elapsed_ms,
                sql = &sql[..sql.len().min(120)],
                "Slow query"
            );
        }
    }

    async fn run_query(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<Vec<OdbcRow>, SqlError> {
        let mut conn = self.checkout(token).await?;
        let start = Instant::now();
        let sql_owned = sql.to_owned();
        let params_owned: Vec<OdbcParam> = params.to_vec();
        let max_text_bytes = self.query_cfg.max_text_bytes;

        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_query_sync(&sql_owned, &params_owned, max_text_bytes)
            }) => res.map_err(|e| SqlError::config(e.to_string()))?,
        };

        self.slow_warn(sql, start.elapsed().as_millis() as u64);
        debug!(elapsed_ms = start.elapsed().as_millis() as u64, sql = &sql[..sql.len().min(80)], "Query executed");
        result
    }

    async fn run_non_query(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<usize, SqlError> {
        let mut conn = self.checkout(token).await?;
        let start = Instant::now();
        let sql_owned = sql.to_owned();
        let params_owned: Vec<OdbcParam> = params.to_vec();

        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_non_query_sync(&sql_owned, &params_owned)
            }) => res.map_err(|e| SqlError::config(e.to_string()))?,
        };

        self.slow_warn(sql, start.elapsed().as_millis() as u64);
        result
    }

    async fn run_insert(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<i64, SqlError> {
        let mut conn = self.checkout(token).await?;
        let sql_owned = sql.to_owned();
        let params_owned: Vec<OdbcParam> = params.to_vec();

        tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_insert_sync(&sql_owned, &params_owned)
            }) => res.map_err(|e| SqlError::config(e.to_string()))?,
        }
    }

    /// Begin a transaction on this service's pool.
    pub async fn begin_transaction<'a>(
        &'a self,
        token: &CancellationToken,
    ) -> Result<Transaction<'a>, SqlError> {
        let conn = self.checkout(token).await?;
        Transaction::begin(conn)
    }

    /// Pool metrics snapshot.
    pub fn pool_metrics(&self) -> crate::pool::metrics::MetricsSnapshot {
        self.pool.metrics()
    }
}

#[async_trait]
impl<T: SqlEntity, C: CacheLayer> SqlRepository<T> for SqlService<T, C> {
    // ── get_by_id ─────────────────────────────────────────────────────────────
    async fn get_by_id(
        &self,
        id: impl Into<PkValue> + Send,
        token: &CancellationToken,
    ) -> Result<Option<T>, SqlError> {
        let pk = id.into();
        let cache_key = format!("{}::GetById::{pk}", T::CACHE_PREFIX);
        if let Ok(Some(cached)) = self.cache.get::<T>(&cache_key).await {
            return Ok(Some(cached));
        }

        let params = [OdbcParam::new(T::PK_COLUMN, pk.as_param())];
        let rows = self.run_query(T::SELECT_BY_PK_SQL, &params, token).await?;
        let entity = rows.into_iter().next().map(|r| T::from_row(&r)).transpose()?;

        if let Some(ref e) = entity {
            let _ = self.cache.set(&cache_key, e, self.cache.default_ttl()).await;
        }
        Ok(entity)
    }

    // ── get_all ───────────────────────────────────────────────────────────────
    async fn get_all(&self, token: &CancellationToken) -> Result<Vec<T>, SqlError> {
        let cache_key = format!("{}::GetAll", T::CACHE_PREFIX);
        if let Ok(Some(cached)) = self.cache.get_vec::<T>(&cache_key).await {
            return Ok(cached);
        }

        let rows = self.run_query(T::SELECT_SQL, &[], token).await?;
        let entities: Result<Vec<T>, _> = rows.iter().map(T::from_row).collect();
        let entities = entities?;

        let _ = self.cache.set_vec(&cache_key, &entities, self.cache.default_ttl()).await;
        Ok(entities)
    }

    // ── get_where ─────────────────────────────────────────────────────────────
    async fn get_where(
        &self,
        filter: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<Vec<T>, SqlError> {
        let sql = filtered_sql(T::SELECT_SQL, filter);
        let rows = self.run_query(&sql, params, token).await?;
        rows.iter().map(T::from_row).collect()
    }

    // ── get_paged ─────────────────────────────────────────────────────────────
    async fn get_paged(
        &self,
        page: i64,
        page_size: i64,
        token: &CancellationToken,
    ) -> Result<Vec<T>, SqlError> {
        let sql = paged_sql(T::SELECT_SQL, T::PK_COLUMN, page, page_size);
        let rows = self.run_query(&sql, &[], token).await?;
        rows.iter().map(T::from_row).collect()
    }

    // ── count ─────────────────────────────────────────────────────────────────
    async fn count(&self, token: &CancellationToken) -> Result<i64, SqlError> {
        let cache_key = format!("{}::Count", T::CACHE_PREFIX);
        if let Ok(Some(v)) = self.cache.get_scalar::<i64>(&cache_key).await {
            return Ok(v);
        }

        let rows = self.run_query(T::COUNT_SQL, &[], token).await?;
        let count: i64 = rows
            .into_iter()
            .next()
            .and_then(|r| r.get_first_string().ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        let _ = self.cache.set_scalar(&cache_key, count, self.cache.count_ttl()).await;
        Ok(count)
    }

    // ── exists ────────────────────────────────────────────────────────────────
    async fn exists(
        &self,
        id: impl Into<PkValue> + Send,
        token: &CancellationToken,
    ) -> Result<bool, SqlError> {
        let pk = id.into();
        let params = [OdbcParam::new(T::PK_COLUMN, pk.as_param())];
        let rows = self.run_query(T::EXISTS_SQL, &params, token).await?;
        Ok(!rows.is_empty())
    }

    // ── insert ────────────────────────────────────────────────────────────────
    async fn insert(&self, entity: &T, token: &CancellationToken) -> Result<i64, SqlError> {
        let params = entity.to_params();
        let id = if T::PK_IS_IDENTITY {
            self.run_insert(T::INSERT_SQL, &params, token).await?
        } else {
            self.run_non_query(T::INSERT_SQL, &params, token).await?;
            match entity.pk_value() {
                PkValue::I32(v)  => v as i64,
                PkValue::I64(v)  => v,
                PkValue::Str(_)  => 0,
                PkValue::Guid(_) => 0, // Guid PKs don't return a numeric identity
            }
        };
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(id)
    }

    // ── update ────────────────────────────────────────────────────────────────
    async fn update(&self, entity: &T, token: &CancellationToken) -> Result<(), SqlError> {
        let params = entity.to_params();
        self.run_non_query(T::UPDATE_SQL, &params, token).await?;
        let pk = entity.pk_value();
        let _ = self.cache.invalidate_entry(T::CACHE_PREFIX, &pk.to_string()).await;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(())
    }

    // ── delete ────────────────────────────────────────────────────────────────
    async fn delete(
        &self,
        id: impl Into<PkValue> + Send,
        token: &CancellationToken,
    ) -> Result<(), SqlError> {
        let pk = id.into();
        let params = [OdbcParam::new(T::PK_COLUMN, pk.as_param())];
        self.run_non_query(T::DELETE_SQL, &params, token).await?;
        let _ = self.cache.invalidate_entry(T::CACHE_PREFIX, &pk.to_string()).await;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(())
    }

    // ── upsert ────────────────────────────────────────────────────────────────
    async fn upsert(&self, entity: &T, token: &CancellationToken) -> Result<(), SqlError> {
        let params = entity.to_params();
        self.run_non_query(T::UPSERT_SQL, &params, token).await?;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(())
    }

    // ── batch_insert ──────────────────────────────────────────────────────────
    async fn batch_insert(
        &self,
        entities: &[T],
        token: &CancellationToken,
    ) -> Result<Vec<i64>, SqlError> {
        let mut ids = Vec::with_capacity(entities.len());
        let mut tx = self.begin_transaction(token).await?;
        for entity in entities {
            let id = tx.insert::<T>(entity, token).await?;
            ids.push(id);
        }
        tx.commit().await?;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(ids)
    }

    // ── batch_update ──────────────────────────────────────────────────────────
    async fn batch_update(
        &self,
        entities: &[T],
        token: &CancellationToken,
    ) -> Result<(), SqlError> {
        let mut tx = self.begin_transaction(token).await?;
        for entity in entities {
            tx.update::<T>(entity, token).await?;
        }
        tx.commit().await?;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(())
    }

    // ── batch_delete ──────────────────────────────────────────────────────────
    async fn batch_delete(
        &self,
        ids: &[PkValue],
        token: &CancellationToken,
    ) -> Result<(), SqlError> {
        let mut tx = self.begin_transaction(token).await?;
        for id in ids {
            tx.delete::<T>(id.clone(), token).await?;
        }
        tx.commit().await?;
        let _ = self.cache.invalidate_table(T::CACHE_PREFIX).await;
        Ok(())
    }

    // ── query_raw ─────────────────────────────────────────────────────────────
    async fn query_raw(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<Vec<OdbcRow>, SqlError> {
        self.run_query(sql, params, token).await
    }

    // ── execute_raw ───────────────────────────────────────────────────────────
    async fn execute_raw(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<usize, SqlError> {
        self.run_non_query(sql, params, token).await
    }

    // ── scalar ────────────────────────────────────────────────────────────────
    async fn scalar<S: TryFrom<String> + Send>(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<S, SqlError>
    where
        <S as TryFrom<String>>::Error: std::fmt::Display,
    {
        let rows = self.run_query(sql, params, token).await?;
        let raw = rows
            .into_iter()
            .next()
            .and_then(|r| r.get_first_string().ok())
            .ok_or_else(|| SqlError::config("scalar query returned no rows"))?;

        S::try_from(raw).map_err(|e| SqlError::config(e.to_string()))
    }
}
