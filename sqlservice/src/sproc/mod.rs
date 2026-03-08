//! # SprocService — Stored Procedure Execution Layer
//!
//! Rust equivalent of C#'s `ISprocService` — compile-time generic dispatch,
//! zero reflection, zero overhead.
//!
//! ## Key design
//!
//! [`FromResultSet`] is the central trait. Implement it for any type that can
//! be constructed from a single ODBC result set. Built-in wrappers:
//!
//! | Wrapper | Meaning |
//! |---------|---------|
//! | `Vec<T: SqlEntity>` | All rows |
//! | [`Single<T>`] | First row, `Option<T>` |
//! | [`Required<T>`] | First row, error if missing |
//! | [`Scalar<S>`] | First column of first row, parsed via `FromStr` |
//!
//! ## Quick example
//!
//! ```ignore
//! // Two result sets: total count + page of dealers
//! let (Scalar(total), dealers) = sproc
//!     .query2::<Scalar<i64>, Vec<VwDealers>>(
//!         "Dealer.sp_DealerList",
//!         SprocParams::new()
//!             .add("@PageNumber", 1i32)
//!             .add("@PageSize", 20i32),
//!         &ct,
//!     )
//!     .await?;
//! ```

use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::config::QueryConfig;
use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::{OdbcParam, ParamValue};
use crate::pool::{Pool, PooledConn};
use crate::pool::metrics::MetricsSnapshot;
use crate::row::OdbcRow;

// ── Result-set wrappers ───────────────────────────────────────────────────────

/// First row of a result set — `None` if the set is empty.
///
/// Destructure with `let Single(value) = ...`.
#[derive(Debug, Clone)]
pub struct Single<T>(pub Option<T>);

/// First row of a result set — error if the set is empty.
///
/// Destructure with `let Required(value) = ...`.
#[derive(Debug, Clone)]
pub struct Required<T>(pub T);

/// First column of the first row, parsed via [`std::str::FromStr`].
///
/// `None` if the result set is empty.
/// Destructure with `let Scalar(value) = ...`.
#[derive(Debug, Clone)]
pub struct Scalar<T>(pub Option<T>);

// ── FromResultSet trait ───────────────────────────────────────────────────────

/// Convert one ODBC result set (a `Vec<OdbcRow>`) into `Self`.
///
/// This is the compile-time dispatch hook used by all `SprocService::query*`
/// methods. All dispatch happens at monomorphisation time — zero runtime cost.
pub trait FromResultSet: Sized {
    fn from_result_set(rows: Vec<OdbcRow>) -> Result<Self, SqlError>;
}

impl<T: SqlEntity> FromResultSet for Vec<T> {
    #[inline]
    fn from_result_set(rows: Vec<OdbcRow>) -> Result<Self, SqlError> {
        rows.iter().map(T::from_row).collect()
    }
}

impl<T: SqlEntity> FromResultSet for Single<T> {
    #[inline]
    fn from_result_set(rows: Vec<OdbcRow>) -> Result<Self, SqlError> {
        let val = rows
            .into_iter()
            .next()
            .map(|r| T::from_row(&r))
            .transpose()?;
        Ok(Single(val))
    }
}

impl<T: SqlEntity> FromResultSet for Required<T> {
    #[inline]
    fn from_result_set(rows: Vec<OdbcRow>) -> Result<Self, SqlError> {
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| SqlError::config("query_required: sproc returned no rows"))?;
        Ok(Required(T::from_row(&row)?))
    }
}

impl<S> FromResultSet for Scalar<S>
where
    S: std::str::FromStr,
    S::Err: std::fmt::Display,
{
    #[inline]
    fn from_result_set(rows: Vec<OdbcRow>) -> Result<Self, SqlError> {
        let val = rows
            .into_iter()
            .next()
            .and_then(|r| r.get_first_string().ok())
            .map(|s| {
                s.trim()
                    .parse::<S>()
                    .map_err(|e| SqlError::config(e.to_string()))
            })
            .transpose()?;
        Ok(Scalar(val))
    }
}

// ── SprocParams ───────────────────────────────────────────────────────────────

/// Fluent parameter builder for stored procedure calls.
///
/// ```ignore
/// SprocParams::new()
///     .add("@Name",    "Acme Corp")
///     .add("@Active",  true)
///     .add_nullable("@TaxId", tax_id.as_deref())
/// ```
///
/// Parameter names may include or omit the leading `@` — both forms are accepted.
#[derive(Debug, Default, Clone)]
pub struct SprocParams {
    // (name_without_at, value)
    params: Vec<(String, ParamValue)>,
}

impl SprocParams {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a required parameter. `value` must implement `Into<ParamValue>`.
    #[inline]
    pub fn add(mut self, name: &str, value: impl Into<ParamValue>) -> Self {
        let key = strip_at(name);
        self.params.push((key, value.into()));
        self
    }

    /// Add an optional parameter. `None` maps to SQL `NULL`.
    #[inline]
    pub fn add_nullable<V: Into<ParamValue>>(mut self, name: &str, value: Option<V>) -> Self {
        let key = strip_at(name);
        let pv = match value {
            Some(v) => v.into(),
            None => ParamValue::Null,
        };
        self.params.push((key, pv));
        self
    }

    /// Build the `EXEC sproc_name @p = @p, …` SQL string and the param slice.
    ///
    /// Names are leaked to produce `&'static str` required by [`OdbcParam`].
    /// This is bounded (one small allocation per unique param name) and safe.
    pub(crate) fn into_exec(self, sproc_name: &str) -> (String, Vec<OdbcParam>) {
        let sql = if self.params.is_empty() {
            format!("EXEC {sproc_name}")
        } else {
            let list = self
                .params
                .iter()
                .map(|(n, _)| format!("@{n} = @{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("EXEC {sproc_name} {list}")
        };

        let odbc_params = self
            .params
            .into_iter()
            .map(|(name, value)| {
                // SAFETY: The leaked string is a short param name literal.
                // Leaking is bounded to the number of unique param names used
                // across the process lifetime — acceptable in server applications.
                let static_name: &'static str =
                    Box::leak(name.into_boxed_str());
                OdbcParam::new(static_name, value)
            })
            .collect();

        (sql, odbc_params)
    }
}

fn strip_at(name: &str) -> String {
    name.strip_prefix('@').unwrap_or(name).to_owned()
}

// ── MultiReader ───────────────────────────────────────────────────────────────

/// Manual reader for 5+ result sets — or when typed tuples are insufficient.
///
/// Each `read_*` call consumes the next result set in order. Calling more
/// times than there are result sets returns an empty result (not an error).
#[derive(Debug)]
pub struct MultiReader {
    sets: Vec<Vec<OdbcRow>>,
    idx: usize,
}

impl MultiReader {
    pub(crate) fn new(sets: Vec<Vec<OdbcRow>>) -> Self {
        Self { sets, idx: 0 }
    }

    fn next_set(&mut self) -> Vec<OdbcRow> {
        if self.idx < self.sets.len() {
            let set = std::mem::take(&mut self.sets[self.idx]);
            self.idx += 1;
            set
        } else {
            Vec::new()
        }
    }

    /// Read all rows of the next result set as `Vec<T>`.
    pub fn read_list<T: SqlEntity>(&mut self) -> Result<Vec<T>, SqlError> {
        let rows = self.next_set();
        rows.iter().map(T::from_row).collect()
    }

    /// Read the first row of the next result set as `Option<T>`.
    pub fn read_single<T: SqlEntity>(&mut self) -> Result<Option<T>, SqlError> {
        let rows = self.next_set();
        rows.into_iter().next().map(|r| T::from_row(&r)).transpose()
    }

    /// Read the first row of the next result set, error if missing.
    pub fn read_required<T: SqlEntity>(&mut self) -> Result<T, SqlError> {
        let rows = self.next_set();
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| SqlError::config("read_required: result set is empty"))?;
        T::from_row(&row)
    }

    /// Read a scalar from the first column of the first row of the next set.
    pub fn read_scalar<S>(&mut self) -> Result<Option<S>, SqlError>
    where
        S: std::str::FromStr,
        S::Err: std::fmt::Display,
    {
        let rows = self.next_set();
        rows.into_iter()
            .next()
            .and_then(|r| r.get_first_string().ok())
            .map(|s| {
                s.trim()
                    .parse::<S>()
                    .map_err(|e| SqlError::config(e.to_string()))
            })
            .transpose()
    }

    /// Read raw rows of the next result set without mapping.
    pub fn read_raw(&mut self) -> Vec<OdbcRow> {
        self.next_set()
    }
}

// ── SprocResult ───────────────────────────────────────────────────────────────

/// Business-level result wrapper — mirrors the common `SprocResult<T>` pattern.
///
/// Stored procedures often return a first result set with `Success`, `ErrorCode`,
/// `ErrorMessage` columns followed by data result sets. `SprocResult<T>` is a
/// typed envelope for that pattern.
///
/// For procedures that return no data, use `SprocResult` (= `SprocResult<()>`).
#[derive(Debug, Clone)]
pub struct SprocResult<T = ()> {
    pub success: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub data: Option<T>,
}

impl<T> SprocResult<T> {
    /// Successful result with data.
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            error_code: None,
            error_message: None,
            data: Some(data),
        }
    }

    /// Failed result with optional error codes.
    pub fn fail(
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Self {
        Self {
            success: false,
            error_code,
            error_message,
            data: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.success
    }
}

impl SprocResult<()> {
    /// Successful result with no data payload.
    pub fn ok_unit() -> Self {
        Self {
            success: true,
            error_code: None,
            error_message: None,
            data: None,
        }
    }
}

// ── SprocPagedResult ──────────────────────────────────────────────────────────

/// Paged result from a stored procedure.
///
/// Convention: the sproc returns two result sets — data rows first,
/// then a single row with a `TotalCount` column.
#[derive(Debug, Clone)]
pub struct SprocPagedResult<T> {
    pub items: Vec<T>,
    pub total_count: i64,
    pub page_number: i32,
    pub page_size: i32,
}

// ── SprocService ──────────────────────────────────────────────────────────────

/// Stored procedure execution service.
///
/// Obtain via [`crate::factory::SqlServiceFactory::build_sproc`].
pub struct SprocService {
    pool: Pool,
    query_cfg: QueryConfig,
}

impl SprocService {
    pub fn new(pool: Pool, query_cfg: QueryConfig) -> Self {
        Self { pool, query_cfg }
    }

    pub fn pool_metrics(&self) -> MetricsSnapshot {
        self.pool.metrics()
    }

    async fn checkout(&self, token: &CancellationToken) -> Result<PooledConn, SqlError> {
        self.pool.checkout(token).await
    }

    fn slow_warn(&self, sql: &str, elapsed_ms: u64) {
        if elapsed_ms >= self.query_cfg.slow_query_threshold_ms {
            warn!(
                elapsed_ms,
                sql = &sql[..sql.len().min(120)],
                "Slow sproc"
            );
        }
    }

    /// Core: execute SQL and return all result sets via `spawn_blocking`.
    async fn run_multiple(
        &self,
        sql: &str,
        params: &[OdbcParam],
        token: &CancellationToken,
    ) -> Result<Vec<Vec<OdbcRow>>, SqlError> {
        let mut conn = self.checkout(token).await?;
        let start = Instant::now();
        let sql_owned = sql.to_owned();
        let params_owned: Vec<OdbcParam> = params.to_vec();
        let max_text_bytes = self.query_cfg.max_text_bytes;

        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_multiple_query_sync(&sql_owned, &params_owned, max_text_bytes)
            }) => res.map_err(|e| SqlError::config(e.to_string()))?,
        };

        let elapsed = start.elapsed().as_millis() as u64;
        self.slow_warn(sql, elapsed);
        debug!(elapsed_ms = elapsed, sql = &sql[..sql.len().min(80)], "Sproc executed");
        result
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Execute a sproc and read the **first** result set as `R`.
    ///
    /// ```ignore
    /// // List
    /// let dealers: Vec<VwDealers> = sproc.query("Dealer.sp_List", params, &ct).await?;
    ///
    /// // Single nullable
    /// let Single(d) = sproc.query::<Single<VwDealers>>("Dealer.sp_GetById", params, &ct).await?;
    ///
    /// // Required (errors if empty)
    /// let Required(d) = sproc.query::<Required<VwDealers>>("Dealer.sp_GetById", params, &ct).await?;
    ///
    /// // Scalar
    /// let Scalar(n) = sproc.query::<Scalar<i64>>("dbo.sp_Count", params, &ct).await?;
    /// ```
    pub async fn query<R: FromResultSet>(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<R, SqlError> {
        let (sql, odbc_params) = params.into_exec(name);
        let mut sets = self.run_multiple(&sql, &odbc_params, token).await?;
        let first = if sets.is_empty() { Vec::new() } else { sets.remove(0) };
        R::from_result_set(first)
    }

    /// Execute a sproc and read the first **two** result sets as `(R1, R2)`.
    ///
    /// ```ignore
    /// // Scalar total + list (paging pattern)
    /// let (Scalar(total), dealers) =
    ///     sproc.query2::<Scalar<i64>, Vec<VwDealers>>("Dealer.sp_List", params, &ct).await?;
    /// ```
    pub async fn query2<R1: FromResultSet, R2: FromResultSet>(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<(R1, R2), SqlError> {
        let (sql, odbc_params) = params.into_exec(name);
        let mut sets = self.run_multiple(&sql, &odbc_params, token).await?;
        let s0 = pop_front(&mut sets);
        let s1 = pop_front(&mut sets);
        Ok((R1::from_result_set(s0)?, R2::from_result_set(s1)?))
    }

    /// Execute a sproc and read the first **three** result sets as `(R1, R2, R3)`.
    pub async fn query3<R1: FromResultSet, R2: FromResultSet, R3: FromResultSet>(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<(R1, R2, R3), SqlError> {
        let (sql, odbc_params) = params.into_exec(name);
        let mut sets = self.run_multiple(&sql, &odbc_params, token).await?;
        let s0 = pop_front(&mut sets);
        let s1 = pop_front(&mut sets);
        let s2 = pop_front(&mut sets);
        Ok((
            R1::from_result_set(s0)?,
            R2::from_result_set(s1)?,
            R3::from_result_set(s2)?,
        ))
    }

    /// Execute a sproc and read the first **four** result sets as `(R1, R2, R3, R4)`.
    pub async fn query4<
        R1: FromResultSet,
        R2: FromResultSet,
        R3: FromResultSet,
        R4: FromResultSet,
    >(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<(R1, R2, R3, R4), SqlError> {
        let (sql, odbc_params) = params.into_exec(name);
        let mut sets = self.run_multiple(&sql, &odbc_params, token).await?;
        let s0 = pop_front(&mut sets);
        let s1 = pop_front(&mut sets);
        let s2 = pop_front(&mut sets);
        let s3 = pop_front(&mut sets);
        Ok((
            R1::from_result_set(s0)?,
            R2::from_result_set(s1)?,
            R3::from_result_set(s2)?,
            R4::from_result_set(s3)?,
        ))
    }

    /// Execute a sproc and return a [`MultiReader`] for manual result-set access.
    ///
    /// Use when you need 5+ result sets, or when `read_scalar` / `read_single` /
    /// `read_list` calls need to be interleaved with business logic.
    ///
    /// ```ignore
    /// let mut reader = sproc.query_multiple("Dealer.sp_GetById", params, &ct).await?;
    /// let result_row = reader.read_single::<SprocResultRow>()?;
    /// let dealer     = reader.read_single::<VwDealers>()?;
    /// let allocs     = reader.read_list::<VwPoolAllocations>()?;
    /// let licenses   = reader.read_list::<VwUnassignedLicenses>()?;
    /// let sub_dealers = reader.read_list::<VwDealers>()?;
    /// ```
    pub async fn query_multiple(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<MultiReader, SqlError> {
        let (sql, odbc_params) = params.into_exec(name);
        let sets = self.run_multiple(&sql, &odbc_params, token).await?;
        Ok(MultiReader::new(sets))
    }

    /// Execute a non-query sproc (INSERT / UPDATE / DELETE). Returns rows affected.
    pub async fn execute(
        &self,
        name: &str,
        params: SprocParams,
        token: &CancellationToken,
    ) -> Result<usize, SqlError> {
        let mut conn = self.checkout(token).await?;
        let (sql, odbc_params) = params.into_exec(name);
        let sql_owned = sql.clone();

        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_non_query_sync(&sql_owned, &odbc_params)
            }) => res.map_err(|e| SqlError::config(e.to_string()))?,
        };

        self.slow_warn(&sql, 0);
        result
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn pop_front(sets: &mut Vec<Vec<OdbcRow>>) -> Vec<OdbcRow> {
    if sets.is_empty() {
        Vec::new()
    } else {
        sets.remove(0)
    }
}
