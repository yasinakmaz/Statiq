use thiserror::Error;

#[derive(Debug, Error)]
pub enum SqlError {
    #[error("ODBC error [{code}]: {message}")]
    Odbc { code: i32, message: String },

    #[error("Connection pool exhausted (timeout: {timeout_ms}ms)")]
    PoolExhausted { timeout_ms: u64 },

    #[error("Query timeout after {elapsed_ms}ms")]
    QueryTimeout { elapsed_ms: u64 },

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Redis error: {0}")]
    Cache(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("Deadlock detected, retries exhausted ({attempts})")]
    DeadlockRetryExhausted { attempts: u8 },

    #[error("Transaction already committed or rolled back")]
    InvalidTransactionState,

    #[error("Config error: {0}")]
    Config(String),

    #[error("Row mapping error on column '{column}': {reason}")]
    RowMapping {
        column: String,
        reason: String,
    },

    #[error("Not found: {table} with pk={pk}")]
    NotFound { table: &'static str, pk: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "encrypted-config")]
    #[error("Crypto error: {0}")]
    Crypto(String),
}

impl SqlError {
    pub fn odbc(code: i32, message: impl Into<String>) -> Self {
        Self::Odbc { code, message: message.into() }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Constructor for static column names (compile-time known columns from proc-macro).
    pub fn row_mapping(column: &'static str, reason: impl Into<String>) -> Self {
        Self::RowMapping { column: column.to_string(), reason: reason.into() }
    }

    /// Constructor for dynamic column names (runtime-generated column aliases, query_raw, etc.).
    pub fn row_mapping_dynamic(column: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::RowMapping { column: column.into(), reason: reason.into() }
    }

    /// Returns true if this error is a SQL Server deadlock (error 1205).
    pub fn is_deadlock(&self) -> bool {
        matches!(self, Self::Odbc { code: 1205, .. })
    }

    /// Short machine-readable error code for structured API / IPC responses.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Odbc { .. }                        => "odbc_error",
            Self::PoolExhausted { .. }               => "pool_exhausted",
            Self::QueryTimeout { .. }                => "query_timeout",
            Self::Cancelled                          => "cancelled",
            Self::Cache(_)                           => "cache_error",
            Self::Serialize(_)                       => "serialization_error",
            Self::DeadlockRetryExhausted { .. }      => "deadlock_retry_exhausted",
            Self::InvalidTransactionState            => "invalid_transaction_state",
            Self::Config(_)                          => "config_error",
            Self::RowMapping { .. }                  => "row_mapping_error",
            Self::NotFound { .. }                    => "not_found",
            Self::Io(_)                              => "io_error",
            #[cfg(feature = "encrypted-config")]
            Self::Crypto(_)                          => "crypto_error",
        }
    }

    /// A client-safe message that does not expose ODBC internals or connection strings.
    pub fn safe_message(&self) -> String {
        match self {
            Self::NotFound { table, pk }             => format!("{table} with pk={pk} not found"),
            Self::Cancelled                          => "Operation cancelled".to_string(),
            Self::PoolExhausted { timeout_ms }       => format!("Service busy, pool timeout after {timeout_ms}ms"),
            Self::QueryTimeout { elapsed_ms }        => format!("Query timed out after {elapsed_ms}ms"),
            Self::DeadlockRetryExhausted { attempts} => format!("Deadlock retries exhausted ({attempts})"),
            Self::InvalidTransactionState            => "Invalid transaction state".to_string(),
            // Do not expose ODBC connection strings, Redis URLs, or internal details
            _                                        => "An internal database error occurred".to_string(),
        }
    }
}

// ── axum integration ──────────────────────────────────────────────────────────

#[cfg(feature = "axum")]
impl axum::response::IntoResponse for SqlError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;
        use serde_json::json;

        let status = match &self {
            SqlError::NotFound { .. }                    => StatusCode::NOT_FOUND,
            SqlError::Cancelled                          => StatusCode::from_u16(499).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            SqlError::PoolExhausted { .. }
            | SqlError::DeadlockRetryExhausted { .. }    => StatusCode::SERVICE_UNAVAILABLE,
            SqlError::QueryTimeout { .. }                => StatusCode::GATEWAY_TIMEOUT,
            _                                            => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = Json(json!({
            "error_code": self.error_code(),
            "message":    self.safe_message(),
        }));

        (status, body).into_response()
    }
}

// ── Tauri IPC integration ─────────────────────────────────────────────────────

/// `serde::Serialize` for `SqlError` is only available under the `tauri` feature.
/// Tauri's `#[tauri::command]` requires `E: Serialize` for `Result<T, E>` returns.
#[cfg(feature = "tauri")]
impl serde::Serialize for SqlError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("SqlError", 2)?;
        s.serialize_field("error_code", self.error_code())?;
        s.serialize_field("message", &self.safe_message())?;
        s.end()
    }
}
