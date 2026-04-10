use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::{OdbcParam, PkValue};
use crate::pool::connection::OdbcConn;
use crate::pool::PooledConn;

/// SQL Server transaction isolation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Snapshot,
    Serializable,
}

impl IsolationLevel {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::ReadUncommitted => "READ UNCOMMITTED",
            Self::ReadCommitted   => "READ COMMITTED",
            Self::RepeatableRead  => "REPEATABLE READ",
            Self::Snapshot        => "SNAPSHOT",
            Self::Serializable    => "SERIALIZABLE",
        }
    }
}

/// An active database transaction.
///
/// On `Drop`, if not yet committed, automatically issues a synchronous rollback.
pub struct Transaction<'pool> {
    conn: Option<OdbcConn>,
    return_tx: Sender<OdbcConn>,
    notify: Arc<Notify>,
    committed: bool,
    _phantom: std::marker::PhantomData<&'pool ()>,
}

impl<'pool> Transaction<'pool> {
    /// Begin a transaction on a checked-out connection.
    pub(crate) fn begin(guard: PooledConn) -> Result<Self, SqlError> {
        Self::begin_isolated(guard, IsolationLevel::ReadCommitted)
    }

    /// Begin a transaction with a specific isolation level.
    /// Single round-trip: SET ISOLATION LEVEL + SET IMPLICIT_TRANSACTIONS OFF + BEGIN TRANSACTION.
    pub(crate) fn begin_isolated(guard: PooledConn, level: IsolationLevel) -> Result<Self, SqlError> {
        let (mut conn, return_tx, notify) = guard.take();

        let sql = format!(
            "SET TRANSACTION ISOLATION LEVEL {}; SET IMPLICIT_TRANSACTIONS OFF; BEGIN TRANSACTION",
            level.as_sql()
        );
        conn.execute_non_query_sync(&sql, &[])
            .map_err(|e| SqlError::odbc(0, format!("BEGIN TRANSACTION failed: {e}")))?;

        debug!(isolation = ?level, "Transaction started");
        Ok(Self {
            conn: Some(conn),
            return_tx,
            notify,
            committed: false,
            _phantom: std::marker::PhantomData,
        })
    }

    fn conn_mut(&mut self) -> Result<&mut OdbcConn, SqlError> {
        self.conn.as_mut().ok_or(SqlError::InvalidTransactionState)
    }

    // ── DML helpers ───────────────────────────────────────────────────────────

    pub async fn insert<T: SqlEntity>(
        &mut self,
        entity: &T,
        _token: &CancellationToken,
    ) -> Result<i64, SqlError> {
        let params = entity.to_params();
        let conn = self.conn_mut()?;
        if T::PK_IS_IDENTITY {
            conn.execute_insert_sync(T::INSERT_SQL, &params)
        } else {
            conn.execute_non_query_sync(T::INSERT_SQL, &params)?;
            Ok(match entity.pk_value() {
                PkValue::I32(v)  => v as i64,
                PkValue::I64(v)  => v,
                PkValue::Str(_)  => 0,
                PkValue::Guid(_) => 0,
            })
        }
    }

    pub async fn update<T: SqlEntity>(
        &mut self,
        entity: &T,
        _token: &CancellationToken,
    ) -> Result<(), SqlError> {
        let params = entity.to_params();
        let conn = self.conn_mut()?;
        conn.execute_non_query_sync(T::UPDATE_SQL, &params)?;
        Ok(())
    }

    pub async fn delete<T: SqlEntity>(
        &mut self,
        id: impl Into<PkValue>,
        _token: &CancellationToken,
    ) -> Result<(), SqlError> {
        let pk = id.into();
        let params = [OdbcParam::new(T::PK_COLUMN, pk.as_param())];
        let conn = self.conn_mut()?;
        conn.execute_non_query_sync(T::DELETE_SQL, &params)?;
        Ok(())
    }

    pub async fn execute_raw(
        &mut self,
        sql: &str,
        params: &[OdbcParam],
        _token: &CancellationToken,
    ) -> Result<usize, SqlError> {
        let conn = self.conn_mut()?;
        conn.execute_non_query_sync(sql, params)
    }

    // ── Savepoints ───────────────────────────────────────────────────────────

    /// Create a savepoint within the current transaction.
    ///
    /// `name` must contain only alphanumeric characters and underscores.
    pub async fn savepoint(&mut self, name: &str) -> Result<(), SqlError> {
        Self::validate_savepoint_name(name)?;
        let conn = self.conn_mut()?;
        conn.execute_non_query_sync(&format!("SAVE TRANSACTION {name}"), &[])?;
        debug!(savepoint = name, "Savepoint created");
        Ok(())
    }

    /// Roll back to a previously created savepoint (partial rollback).
    ///
    /// `name` must contain only alphanumeric characters and underscores.
    pub async fn rollback_to(&mut self, name: &str) -> Result<(), SqlError> {
        Self::validate_savepoint_name(name)?;
        let conn = self.conn_mut()?;
        conn.execute_non_query_sync(&format!("ROLLBACK TRANSACTION {name}"), &[])?;
        debug!(savepoint = name, "Rolled back to savepoint");
        Ok(())
    }

    fn validate_savepoint_name(name: &str) -> Result<(), SqlError> {
        if name.is_empty() {
            return Err(SqlError::config("Savepoint name must not be empty"));
        }
        for ch in name.chars() {
            if !ch.is_ascii_alphanumeric() && ch != '_' {
                return Err(SqlError::config(format!(
                    "Invalid savepoint name character: '{ch}' — only alphanumeric and '_' allowed"
                )));
            }
        }
        Ok(())
    }

    // ── Commit / Rollback ─────────────────────────────────────────────────────

    pub async fn commit(mut self) -> Result<(), SqlError> {
        let conn = self.conn.as_mut().ok_or(SqlError::InvalidTransactionState)?;
        conn.commit_sync()?;
        self.committed = true;
        debug!("Transaction committed");
        Ok(())
    }

    pub fn rollback(&mut self) {
        if let Some(conn) = self.conn.as_mut() {
            conn.rollback_sync();
            warn!("Transaction rolled back");
        }
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(conn) = self.conn.as_mut() {
                conn.rollback_sync();
                debug!("Transaction auto-rolled back on drop");
            }
        }
        // Return connection to pool and wake a waiting checkout.
        if let Some(mut conn) = self.conn.take() {
            // Mark for session reset — after a transaction the connection may have
            // leftover SET options or other session state from the application code.
            conn.needs_reset = true;
            let _ = self.return_tx.send(conn);
            self.notify.notify_one();
        }
    }
}

// ── Deadlock retry helper ─────────────────────────────────────────────────────

/// Execute a closure inside a transaction, retrying on deadlock up to `max_retries`.
pub async fn with_retry<F, Fut, T>(
    pool: &crate::pool::Pool,
    token: &CancellationToken,
    max_retries: u8,
    mut f: F,
) -> Result<T, SqlError>
where
    F: FnMut(&mut Transaction<'_>) -> Fut,
    Fut: std::future::Future<Output = Result<T, SqlError>>,
{
    let mut attempts = 0u8;
    loop {
        let conn = pool.checkout(token).await?;
        let mut tx = Transaction::begin(conn)?;

        match f(&mut tx).await {
            Ok(value) => {
                tx.commit().await?;
                return Ok(value);
            }
            Err(e) if e.is_deadlock() && attempts < max_retries => {
                attempts += 1;
                pool.record_deadlock();
                warn!(attempt = attempts, "Deadlock detected, retrying");
                let backoff = Duration::from_millis(50 * (1u64 << attempts));
                tokio::time::sleep(backoff).await;
            }
            Err(e) if e.is_deadlock() => {
                pool.record_deadlock();
                return Err(SqlError::DeadlockRetryExhausted { attempts });
            }
            Err(e) => return Err(e),
        }
    }
}
