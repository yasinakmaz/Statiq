use std::sync::Arc;
use std::time::Instant;
use odbc_api::{Connection, Environment, ResultSetMetadata};
use crate::error::SqlError;
use crate::row::{CellValue, OdbcRow};
use crate::params::OdbcParam;

/// State of a pooled ODBC connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Active,
    Validating,
    Closing,
}

/// An owned ODBC connection with lifecycle metadata.
pub struct OdbcConn {
    pub(crate) inner: Connection<'static>,
    pub state: ConnState,
    pub created_at: Instant,
    pub last_used_at: Instant,
    /// Set to `true` when the connection is returned to the pool.
    /// The next execute call will run `EXEC sp_reset_connection` first to clear
    /// any leftover session state (SET options, implicit transactions, etc.).
    /// Only effective when `reset_on_reuse` is also `true`.
    pub needs_reset: bool,
    /// Mirrors `PoolConfig::reset_connection_on_reuse`. When `false` (default),
    /// `sp_reset_connection` is never executed regardless of `needs_reset`.
    pub reset_on_reuse: bool,
}

// SAFETY: Pool checkout/return protokolü her OdbcConn'un aynı anda yalnızca
// bir task tarafından tutulduğunu garanti eder:
// 1. PooledConn tek erişim noktasıdır (DerefMut + Drop auto-return).
// 2. Pool::checkout en fazla bir PooledConn/connection döndürür.
// 3. Transaction::begin PooledConn'u consume eder, aliasing imkansızdır.
// Send: spawn_blocking'e move edilmesi için gerekli.
// Sync: Arc<Pool>'un thread'ler arası paylaşımı için gerekli.
unsafe impl Send for OdbcConn {}
unsafe impl Sync for OdbcConn {}

impl OdbcConn {
    pub fn new(env: &'static Environment, connection_string: &str) -> Result<Self, SqlError> {
        let inner = env
            .connect_with_connection_string(connection_string, Default::default())
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        Ok(Self {
            inner,
            state: ConnState::Idle,
            created_at: Instant::now(),
            last_used_at: Instant::now(),
            needs_reset: false,
            reset_on_reuse: false,
        })
    }

    pub fn validate(&mut self) -> bool {
        self.state = ConnState::Validating;
        let ok = self.execute_scalar_sync("SELECT 1").is_ok();
        self.state = if ok { ConnState::Idle } else { ConnState::Closing };
        ok
    }

    /// If this connection was previously returned to the pool, run
    /// `EXEC sp_reset_connection` to clear leftover session state before reuse.
    /// This is the same mechanism ADO.NET uses for SQL Server connection pooling.
    fn reset_session_if_needed(&mut self) {
        if !self.needs_reset || !self.reset_on_reuse {
            self.needs_reset = false;
            return;
        }
        self.needs_reset = false;
        // Best-effort: ignore errors — if the reset fails the validator will
        // eventually discard the connection.
        if let Ok(mut stmt) = self.inner.preallocate() {
            let _ = stmt.execute("EXEC sp_reset_connection", ());
        }
    }

    /// Execute a SELECT and return rows (synchronous).
    ///
    /// Parameters are bound as true ODBC positional `?` parameters — no string
    /// embedding. This prevents SQL injection and enables execution plan caching.
    ///
    /// `max_text_bytes` controls the TextRowSet per-cell limit. Cells wider than
    /// this value are silently truncated by the ODBC driver.
    pub fn execute_query_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
        max_text_bytes: usize,
    ) -> Result<Vec<OdbcRow>, SqlError> {
        use crate::pool::binding::params_to_positional;
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let (positional_sql, bound_params) = params_to_positional(sql, params);

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&positional_sql, bound_params.as_slice())
            .map_err(|e| SqlError::odbc(extract_odbc_code(&e), e.to_string()))?;

        let mut rows = Vec::new();
        if let Some(mut cursor) = cursor {
            let col_names: Arc<Vec<String>> = Arc::new(
                cursor
                    .column_names()
                    .map_err(|e: odbc_api::Error| SqlError::odbc(0, e.to_string()))?
                    .collect::<Result<_, _>>()
                    .map_err(|e: odbc_api::Error| SqlError::odbc(0, e.to_string()))?,
            );

            let col_count = col_names.len();

            let mut row_set = TextRowSet::for_cursor(100, &mut cursor, Some(max_text_bytes))
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;

            let mut row_set_cursor = cursor
                .bind_buffer(&mut row_set)
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;

            while let Some(batch) = row_set_cursor
                .fetch()
                .map_err(|e| SqlError::odbc(0, e.to_string()))?
            {
                for row_idx in 0..batch.num_rows() {
                    let mut values = Vec::with_capacity(col_count);
                    for col_idx in 0..col_count {
                        let val = match batch.at(col_idx, row_idx) {
                            None => CellValue::Null,
                            Some(bytes) => {
                                CellValue::Text(String::from_utf8_lossy(bytes).into_owned())
                            }
                        };
                        values.push(val);
                    }
                    // Arc::clone is a reference-count increment — no Vec/String copy.
                    rows.push(OdbcRow::new(Arc::clone(&col_names), values));
                }
            }
        }

        self.last_used_at = Instant::now();
        Ok(rows)
    }

    /// Execute a non-query (INSERT/UPDATE/DELETE). Returns actual rows affected.
    ///
    /// The DML and `SELECT @@ROWCOUNT` are batched into a **single ODBC round-trip**.
    /// Parameters are bound as true positional ODBC `?` bindings (no string embedding).
    pub fn execute_non_query_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
    ) -> Result<usize, SqlError> {
        use crate::pool::binding::params_to_positional;
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let (positional_dml, bound_params) = params_to_positional(sql, params);
        // Batch DML + row-count query into a single server round-trip.
        // @@ROWCOUNT reflects the rows affected by the immediately preceding
        // statement, so batching is safe here.
        // Note: the @@ROWCOUNT part has no ? placeholders — bound_params covers
        // only the DML's ? markers, which is correct.
        let batch_sql = format!("{positional_dml};\nSELECT @@ROWCOUNT AS r");

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&batch_sql, bound_params.as_slice())
            .map_err(|e| SqlError::odbc(extract_odbc_code(&e), e.to_string()))?;

        let mut affected = 0usize;
        if let Some(mut cursor) = cursor {
            let mut row_set = TextRowSet::for_cursor(1, &mut cursor, Some(64))
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;
            let mut rsc = cursor
                .bind_buffer(&mut row_set)
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;
            if let Some(batch) = rsc.fetch().map_err(|e| SqlError::odbc(0, e.to_string()))? {
                if batch.num_rows() > 0 {
                    if let Some(bytes) = batch.at(0, 0) {
                        if let Ok(s) = std::str::from_utf8(bytes) {
                            affected = s.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
        }

        self.last_used_at = Instant::now();
        Ok(affected)
    }

    /// Execute INSERT with OUTPUT INSERTED and return the generated identity value.
    pub fn execute_insert_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
    ) -> Result<i64, SqlError> {
        // reset_session_if_needed is called inside execute_query_sync
        let rows = self.execute_query_sync(sql, params, 256)?;
        rows.into_iter()
            .next()
            .and_then(|r| r.get_first_string().ok())
            .and_then(|s| s.trim().parse::<i64>().ok())
            .ok_or_else(|| SqlError::odbc(0, "INSERT OUTPUT returned no rows or non-numeric id"))
    }

    pub fn commit_sync(&mut self) -> Result<(), SqlError> {
        self.inner
            .commit()
            .map_err(|e| SqlError::odbc(0, e.to_string()))
    }

    pub fn rollback_sync(&mut self) {
        let _ = self.inner.rollback();
    }

    pub fn execute_scalar_sync(&mut self, sql: &str) -> Result<Option<String>, SqlError> {
        let rows = self.execute_query_sync(sql, &[], 256)?;
        Ok(rows.into_iter().next().and_then(|r| r.get_first_string().ok()))
    }

    /// Execute a stored procedure (or any SQL) and return **all** result sets.
    ///
    /// Parameters are bound as true positional ODBC `?` bindings.
    /// Iterates through every result set produced by the query using
    /// `more_results()` on the bound cursor, collecting each into a
    /// `Vec<OdbcRow>`. The outer `Vec` index corresponds to the result-set
    /// index (0 = first, 1 = second, …).
    pub fn execute_multiple_query_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
        max_text_bytes: usize,
    ) -> Result<Vec<Vec<OdbcRow>>, SqlError> {
        use crate::pool::binding::params_to_positional;
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let (positional_sql, bound_params) = params_to_positional(sql, params);

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&positional_sql, bound_params.as_slice())
            .map_err(|e| SqlError::odbc(extract_odbc_code(&e), e.to_string()))?;

        let mut result_sets: Vec<Vec<OdbcRow>> = Vec::new();
        let mut maybe_cursor = cursor;

        while let Some(mut cursor) = maybe_cursor {
            let col_names: Arc<Vec<String>> = Arc::new(
                cursor
                    .column_names()
                    .map_err(|e: odbc_api::Error| SqlError::odbc(0, e.to_string()))?
                    .collect::<Result<_, _>>()
                    .map_err(|e: odbc_api::Error| SqlError::odbc(0, e.to_string()))?,
            );

            let col_count = col_names.len();
            let mut rows = Vec::new();

            let mut row_set =
                TextRowSet::for_cursor(100, &mut cursor, Some(max_text_bytes))
                    .map_err(|e| SqlError::odbc(0, e.to_string()))?;

            let mut rsc = cursor
                .bind_buffer(&mut row_set)
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;

            while let Some(batch) =
                rsc.fetch().map_err(|e| SqlError::odbc(0, e.to_string()))?
            {
                for row_idx in 0..batch.num_rows() {
                    let mut values = Vec::with_capacity(col_count);
                    for col_idx in 0..col_count {
                        let val = match batch.at(col_idx, row_idx) {
                            None => CellValue::Null,
                            Some(bytes) => {
                                CellValue::Text(String::from_utf8_lossy(bytes).into_owned())
                            }
                        };
                        values.push(val);
                    }
                    rows.push(OdbcRow::new(Arc::clone(&col_names), values));
                }
            }

            result_sets.push(rows);

            // Unbind the buffer to recover the cursor, then advance to the
            // next result set.
            let (next_cursor, _buf) = rsc
                .unbind()
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;

            maybe_cursor = next_cursor
                .more_results()
                .map_err(|e| SqlError::odbc(0, e.to_string()))?;
        }

        self.last_used_at = Instant::now();
        Ok(result_sets)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn extract_odbc_code(e: &odbc_api::Error) -> i32 {
    if e.to_string().contains("1205") { 1205 } else { 0 }
}
