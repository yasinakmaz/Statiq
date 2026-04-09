use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub mssql: MssqlConfig,
    pub redis: RedisConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MssqlConfig {
    pub connection_string: String,
    pub pool: PoolConfig,
    pub query: QueryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PoolConfig {
    pub max_size: u32,
    pub min_size: u32,
    /// Close idle connections that have been unused longer than this (seconds).
    pub idle_timeout_secs: u64,
    /// Maximum lifetime of any connection regardless of activity (seconds).
    /// Connections older than this are recycled at the next validation pass.
    /// 0 = no limit.
    #[serde(default = "PoolConfig::default_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
    pub checkout_timeout_ms: u64,
    pub validation_interval_secs: u64,
    pub max_deadlock_retries: u8,
    /// When `true`, runs `EXEC sp_reset_connection` before reusing a pooled
    /// connection to clear leftover session state (SET options, etc.).
    /// This mirrors ADO.NET connection pooling behaviour but requires that the
    /// SQL Server login has EXECUTE permission on the internal proc.
    /// Defaults to `false` — safe for all environments.
    #[serde(default)]
    pub reset_connection_on_reuse: bool,
}

impl PoolConfig {
    fn default_max_lifetime_secs() -> u64 { 1800 } // 30 minutes
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 100,
            min_size: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 1800,
            checkout_timeout_ms: 5000,
            validation_interval_secs: 60,
            max_deadlock_retries: 3,
            reset_connection_on_reuse: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueryConfig {
    pub default_command_timeout_secs: u64,
    pub slow_query_threshold_ms: u64,
    /// Maximum byte size for a single text/binary cell in TextRowSet (scaffold mode).
    /// Cells wider than this limit are silently truncated by the ODBC driver.
    /// Default: 65536 (64 KiB). Increase for nvarchar(max)/xml/varbinary(max) columns.
    #[serde(default = "QueryConfig::default_max_text_bytes")]
    pub max_text_bytes: usize,
}

impl QueryConfig {
    fn default_max_text_bytes() -> usize { 65536 } // 64 KiB
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            default_command_timeout_secs: 30,
            slow_query_threshold_ms: 1000,
            max_text_bytes: QueryConfig::default_max_text_bytes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: u32,
    pub default_ttl_secs: u64,
    pub count_ttl_secs: u64,
    pub enabled: bool,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            pool_size: 20,
            default_ttl_secs: 300,
            count_ttl_secs: 60,
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "INFO".to_string(),
            format: "json".to_string(),
        }
    }
}

impl AppConfig {
    pub fn from_file(path: &str) -> Result<Self, crate::error::SqlError> {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(crate::error::SqlError::from)
    }
}
