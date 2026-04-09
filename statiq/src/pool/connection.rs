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

// SAFETY: odbc-api Connection is not Send by default, but we enforce
// single-threaded access via the pool checkout/return protocol.
// Each connection is held by exactly one task at a time.
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
    /// Parameters are embedded into the SQL string at call time.
    /// The `@name` placeholders in the SQL are replaced with their typed literals.
    ///
    /// `max_text_bytes` controls the TextRowSet per-cell limit. Cells wider than
    /// this value are silently truncated. Use `QueryConfig::max_text_bytes` from
    /// the application config; defaults to 65536 (64 KiB) when called directly.
    pub fn execute_query_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
        max_text_bytes: usize,
    ) -> Result<Vec<OdbcRow>, SqlError> {
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let final_sql = embed_params(sql, params);

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&final_sql, ())
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
    /// The DML and `SELECT @@ROWCOUNT` are batched into a **single ODBC round-trip**
    /// by appending them to the same SQL batch. This halves the network traffic
    /// compared to the old two-statement approach.
    pub fn execute_non_query_sync(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
    ) -> Result<usize, SqlError> {
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let dml_sql = embed_params(sql, params);
        // Batch DML + row-count query into a single server round-trip.
        // @@ROWCOUNT reflects the rows affected by the immediately preceding
        // statement, so batching is safe here.
        let batch_sql = format!("{dml_sql};\nSELECT @@ROWCOUNT AS r");

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&batch_sql, ())
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
        use odbc_api::buffers::TextRowSet;
        use odbc_api::Cursor;

        self.reset_session_if_needed();

        let final_sql = embed_params(sql, params);

        let mut stmt = self
            .inner
            .preallocate()
            .map_err(|e| SqlError::odbc(0, e.to_string()))?;

        let cursor = stmt
            .execute(&final_sql, ())
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
            // next result set. `unbind()` re-releases the buffer binding on
            // the ODBC statement before we call `more_results()`.
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

/// Replace `@name` placeholders in `sql` with SQL-literal values.
///
/// Parameter names are matched longest-first to avoid `@id` accidentally
/// matching inside `@id_prefix`. Uses a single-pass O(sql_len + N) scanner
/// with one pre-sized allocation — avoids the O(N²) allocations of the naive
/// multi-replace approach.
fn embed_params(sql: &str, params: &[OdbcParam]) -> String {
    if params.is_empty() {
        return sql.to_owned();
    }

    // Sort descending by name length so longer names are tried first.
    let mut sorted: Vec<&crate::params::OdbcParam> = params.iter().collect();
    sorted.sort_unstable_by(|a, b| b.name.len().cmp(&a.name.len()));

    // Pre-compute all SQL literals once (not once per occurrence).
    let literals: Vec<(&str, String)> = sorted
        .iter()
        .map(|p| (p.name, param_to_sql_literal(&p.value)))
        .collect();

    // Estimate output capacity: original SQL + ~16 extra chars per param slot.
    let mut out = String::with_capacity(sql.len() + params.len() * 16);

    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'@' {
            let rest = &sql[i + 1..];
            // Try each param name (longest first) at this position.
            let matched = literals.iter().find(|(name, _)| {
                rest.starts_with(name) && {
                    // Ensure the match ends at a word boundary (not part of a
                    // longer identifier). A word boundary is end-of-string or a
                    // non-alphanumeric, non-underscore character.
                    let after = rest[name.len()..].as_bytes().first().copied();
                    !matches!(after, Some(c) if c.is_ascii_alphanumeric() || c == b'_')
                }
            });

            if let Some((name, literal)) = matched {
                out.push_str(literal);
                i += 1 + name.len(); // consume '@' + name
            } else {
                // Not a known param — emit '@' as-is.
                out.push('@');
                i += 1;
            }
        } else {
            // SAFETY: bytes[i] is within a valid UTF-8 string; since it is not
            // '@' (0x40), it may be a multi-byte sequence. Push the full char.
            let ch = sql[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    out
}

fn param_to_sql_literal(value: &crate::params::ParamValue) -> String {
    use crate::params::ParamValue;

    match value {
        // ── Integer ───────────────────────────────────────────────────────────
        ParamValue::Bool(v)    => if *v { "1".into() } else { "0".into() },
        ParamValue::U8(v)      => v.to_string(),
        ParamValue::I16(v)     => v.to_string(),
        ParamValue::I32(v)     => v.to_string(),
        ParamValue::I64(v)     => v.to_string(),

        // ── Float ─────────────────────────────────────────────────────────────
        // Use repr to avoid scientific notation for very small/large values
        ParamValue::F32(v)     => format!("{v:.10}"),
        ParamValue::F64(v)     => format!("{v:.17}"),

        // ── Fixed-precision ───────────────────────────────────────────────────
        ParamValue::Decimal(v) => v.to_string(),

        // ── String (escape single quotes) ────────────────────────────────────
        ParamValue::Str(v)     => format!("N'{}'", v.replace('\'', "''")),

        // ── Binary → 0x hex literal ───────────────────────────────────────────
        // Lookup-table approach: single pre-sized allocation, no per-byte format!().
        ParamValue::Bytes(v) => {
            if v.is_empty() {
                "0x".into()
            } else {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut out = String::with_capacity(2 + v.len() * 2);
                out.push_str("0x");
                for &b in v {
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0x0F) as usize] as char);
                }
                out
            }
        }

        // ── Date / Time ───────────────────────────────────────────────────────
        // date           → 'YYYY-MM-DD'
        ParamValue::NaiveDate(v) => format!("'{}'", v.format("%Y-%m-%d")),
        // time           → 'HH:MM:SS.nnnnnnn'
        ParamValue::NaiveTime(v) => format!("'{}'", v.format("%H:%M:%S%.7f")),
        // datetime/datetime2 → 'YYYY-MM-DD HH:MM:SS.nnnnnnn'
        ParamValue::DateTime(v) => format!("'{}'", v.format("%Y-%m-%d %H:%M:%S%.7f")),
        // datetimeoffset → 'YYYY-MM-DD HH:MM:SS.nnnnnnn +HH:MM'
        ParamValue::DateTimeOffset(v) => format!("'{}'", v.format("%Y-%m-%d %H:%M:%S%.7f %:z")),

        // ── uniqueidentifier → '{GUID}' ────────────────────────────────────
        ParamValue::Guid(v)    => format!("'{v}'"),

        // ── NULL ──────────────────────────────────────────────────────────────
        ParamValue::Null       => "NULL".into(),
    }
}
