use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::params::ParamValue;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::{OdbcParam, PkValue};
use crate::repository::SqlRepository;
use crate::row::OdbcRow;

/// In-memory `SqlRepository<T>` test double. Does not require a database.
///
/// Items are stored in a `HashMap<String, T>` keyed by `T::pk_value().to_string()`.
///
/// # Limitations
/// - `get_where` and `query_raw` return all items (SQL filter is not evaluated).
/// - `get_paged` slices the full item list; ordering is hash-map order.
/// - `scalar` and `execute_raw` always return an error unless overridden.
///
/// # Example
/// ```ignore
/// let repo = MockRepository::<User>::with_data([user1, user2]);
/// let all = repo.get_all(&token).await.unwrap();
/// assert_eq!(repo.insert_call_count(), 0);
/// ```
pub struct MockRepository<T: SqlEntity + Clone> {
    store:         Arc<Mutex<HashMap<String, T>>>,
    next_id:       Arc<AtomicI64>,
    insert_calls:  Arc<AtomicI64>,
    update_calls:  Arc<AtomicI64>,
    delete_calls:  Arc<AtomicI64>,
    upsert_calls:  Arc<AtomicI64>,
}

impl<T: SqlEntity + Clone> Default for MockRepository<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: SqlEntity + Clone> MockRepository<T> {
    /// Create an empty repository.
    pub fn new() -> Self {
        Self {
            store:        Arc::new(Mutex::new(HashMap::new())),
            next_id:      Arc::new(AtomicI64::new(1)),
            insert_calls: Arc::new(AtomicI64::new(0)),
            update_calls: Arc::new(AtomicI64::new(0)),
            delete_calls: Arc::new(AtomicI64::new(0)),
            upsert_calls: Arc::new(AtomicI64::new(0)),
        }
    }

    /// Create a repository pre-populated with the given items.
    pub fn with_data(items: impl IntoIterator<Item = T>) -> Self {
        let map: HashMap<String, T> = items
            .into_iter()
            .map(|item| (item.pk_value().to_string(), item))
            .collect();
        Self {
            store:        Arc::new(Mutex::new(map)),
            next_id:      Arc::new(AtomicI64::new(1)),
            insert_calls: Arc::new(AtomicI64::new(0)),
            update_calls: Arc::new(AtomicI64::new(0)),
            delete_calls: Arc::new(AtomicI64::new(0)),
            upsert_calls: Arc::new(AtomicI64::new(0)),
        }
    }

    /// Number of times `insert` was called.
    pub fn insert_call_count(&self) -> i64 {
        self.insert_calls.load(Ordering::Relaxed)
    }

    /// Number of times `update` was called.
    pub fn update_call_count(&self) -> i64 {
        self.update_calls.load(Ordering::Relaxed)
    }

    /// Number of times `delete` was called.
    pub fn delete_call_count(&self) -> i64 {
        self.delete_calls.load(Ordering::Relaxed)
    }

    /// Number of times `upsert` was called.
    pub fn upsert_call_count(&self) -> i64 {
        self.upsert_calls.load(Ordering::Relaxed)
    }

    /// Current number of items in the store.
    pub async fn len(&self) -> usize {
        self.store.lock().await.len()
    }

    /// All items currently in the store.
    pub async fn all_items(&self) -> Vec<T> {
        self.store.lock().await.values().cloned().collect()
    }

    /// Directly insert or replace an item (bypassing call counters).
    pub async fn seed(&self, item: T) {
        self.store.lock().await.insert(item.pk_value().to_string(), item);
    }

    /// Remove all items (bypassing call counters).
    pub async fn clear(&self) {
        self.store.lock().await.clear();
    }
}

#[async_trait]
impl<T: SqlEntity + Clone + Send + Sync + 'static> SqlRepository<T> for MockRepository<T> {
    async fn get_by_id(
        &self,
        id: impl Into<PkValue> + Send,
        _token: &CancellationToken,
    ) -> Result<Option<T>, SqlError> {
        let key = id.into().to_string();
        Ok(self.store.lock().await.get(&key).cloned())
    }

    async fn get_all(&self, _token: &CancellationToken) -> Result<Vec<T>, SqlError> {
        Ok(self.store.lock().await.values().cloned().collect())
    }

    /// Returns items filtered by a simple `column = @param` expression.
    ///
    /// Supports single equality conditions like `Active = @active`.
    /// For unsupported filter patterns, returns all items as a safe fallback.
    async fn get_where(
        &self,
        filter: &str,
        params: &[OdbcParam],
        _token: &CancellationToken,
    ) -> Result<Vec<T>, SqlError> {
        // Attempt to parse a simple "col = @param" or "col=@param" filter.
        // If parsing fails, fall back to returning all items.
        let items: Vec<T> = self.store.lock().await.values().cloned().collect();

        if let Some((col, param_name)) = parse_simple_eq_filter(filter) {
            if let Some(odbc_param) = params.iter().find(|p| p.name.as_ref().eq_ignore_ascii_case(&param_name)) {
                let expected_str = param_value_to_str(&odbc_param.value);
                return Ok(items.into_iter().filter(|item| {
                    // Try to match via OdbcRow serialisation — check the column value as string.
                    let params = item.to_params();
                    params.iter().any(|p| {
                        let col_match = p.name.as_ref().eq_ignore_ascii_case(&col)
                            || p.name.as_ref().to_lowercase() == col.to_lowercase().trim_matches(|c| c == '[' || c == ']');
                        col_match && expected_str.as_deref() == Some(param_value_to_str(&p.value).unwrap_or_default().as_str())
                    })
                }).collect());
            }
        }

        Ok(items)
    }

    async fn get_paged(
        &self,
        page: i64,
        page_size: i64,
        _token: &CancellationToken,
    ) -> Result<Vec<T>, SqlError> {
        let all: Vec<T> = self.store.lock().await.values().cloned().collect();
        let page = page.max(1) as usize;
        let size = page_size.max(1) as usize;
        let start = (page - 1) * size;
        Ok(all.into_iter().skip(start).take(size).collect())
    }

    async fn count(&self, _token: &CancellationToken) -> Result<i64, SqlError> {
        Ok(self.store.lock().await.len() as i64)
    }

    async fn exists(
        &self,
        id: impl Into<PkValue> + Send,
        _token: &CancellationToken,
    ) -> Result<bool, SqlError> {
        let key = id.into().to_string();
        Ok(self.store.lock().await.contains_key(&key))
    }

    async fn insert(&self, entity: &T, _token: &CancellationToken) -> Result<i64, SqlError> {
        self.insert_calls.fetch_add(1, Ordering::Relaxed);
        let id = if T::PK_IS_IDENTITY {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            match entity.pk_value() {
                PkValue::I32(v) => v as i64,
                PkValue::I64(v) => v,
                _ => 0,
            }
        };
        self.store.lock().await.insert(entity.pk_value().to_string(), entity.clone());
        Ok(id)
    }

    async fn update(&self, entity: &T, _token: &CancellationToken) -> Result<(), SqlError> {
        self.update_calls.fetch_add(1, Ordering::Relaxed);
        let key = entity.pk_value().to_string();
        let mut store = self.store.lock().await;
        if store.contains_key(&key) {
            store.insert(key, entity.clone());
            Ok(())
        } else {
            Err(SqlError::NotFound { table: T::TABLE_NAME, pk: key })
        }
    }

    async fn delete(
        &self,
        id: impl Into<PkValue> + Send,
        _token: &CancellationToken,
    ) -> Result<(), SqlError> {
        self.delete_calls.fetch_add(1, Ordering::Relaxed);
        let key = id.into().to_string();
        self.store.lock().await.remove(&key);
        Ok(())
    }

    async fn upsert(&self, entity: &T, _token: &CancellationToken) -> Result<(), SqlError> {
        self.upsert_calls.fetch_add(1, Ordering::Relaxed);
        self.store.lock().await.insert(entity.pk_value().to_string(), entity.clone());
        Ok(())
    }

    async fn batch_insert(
        &self,
        entities: &[T],
        token: &CancellationToken,
    ) -> Result<Vec<i64>, SqlError> {
        let mut ids = Vec::with_capacity(entities.len());
        for e in entities {
            ids.push(self.insert(e, token).await?);
        }
        Ok(ids)
    }

    async fn batch_update(
        &self,
        entities: &[T],
        token: &CancellationToken,
    ) -> Result<(), SqlError> {
        for e in entities {
            self.update(e, token).await?;
        }
        Ok(())
    }

    async fn batch_delete(
        &self,
        ids: &[PkValue],
        token: &CancellationToken,
    ) -> Result<(), SqlError> {
        for id in ids {
            self.delete(id.clone(), token).await?;
        }
        Ok(())
    }

    /// Returns all items as rows (SQL is not executed).
    async fn query_raw(
        &self,
        _sql: &str,
        _params: &[OdbcParam],
        _token: &CancellationToken,
    ) -> Result<Vec<OdbcRow>, SqlError> {
        Ok(Vec::new())
    }

    /// Always returns `Ok(0)` — no SQL is executed in the mock.
    async fn execute_raw(
        &self,
        _sql: &str,
        _params: &[OdbcParam],
        _token: &CancellationToken,
    ) -> Result<usize, SqlError> {
        Ok(0)
    }

    /// Always returns an error — implement a custom mock for scalar queries.
    async fn scalar<S: TryFrom<String> + Send>(
        &self,
        _sql: &str,
        _params: &[OdbcParam],
        _token: &CancellationToken,
    ) -> Result<S, SqlError>
    where
        <S as TryFrom<String>>::Error: std::fmt::Display,
    {
        Err(SqlError::config(
            "MockRepository::scalar is not supported — use a custom mock for scalar queries",
        ))
    }
}

// ── filter helpers ────────────────────────────────────────────────────────────

/// Parse a simple `column = @param` filter into `(column_name, param_name)`.
/// Returns `None` for unsupported patterns.
fn parse_simple_eq_filter(filter: &str) -> Option<(String, String)> {
    let filter = filter.trim();
    // Accept: `Col = @p`, `[Col] = @p`, `Col=@p`
    let (lhs, rhs) = filter.split_once('=')?;
    let col = lhs.trim().trim_matches(|c| c == '[' || c == ']').to_string();
    let param = rhs.trim().strip_prefix('@')?.to_string();

    // Only accept single-token names (no spaces in param name).
    if col.contains(' ') || param.contains(' ') {
        return None;
    }
    Some((col, param))
}

/// Convert a `ParamValue` to its string representation for comparison.
fn param_value_to_str(value: &ParamValue) -> Option<String> {
    Some(match value {
        ParamValue::Bool(v)     => (if *v { "1" } else { "0" }).to_string(),
        ParamValue::U8(v)       => v.to_string(),
        ParamValue::I16(v)      => v.to_string(),
        ParamValue::I32(v)      => v.to_string(),
        ParamValue::I64(v)      => v.to_string(),
        ParamValue::F32(v)      => v.to_string(),
        ParamValue::F64(v)      => v.to_string(),
        ParamValue::Decimal(v)  => v.to_string(),
        ParamValue::Str(v)      => v.clone(),
        ParamValue::Guid(v)     => v.to_string(),
        ParamValue::Null        => return None,
        _                       => return None,
    })
}
