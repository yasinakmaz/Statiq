use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub mssql: MssqlConfig,
    pub redis: RedisConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MssqlConfig {
    pub connection_string: String,
    pub pool: PoolConfig,
    pub query: QueryConfig,
    /// Optional read-replica connection strings.
    /// When non-empty, SELECT queries are routed to replicas (round-robin)
    /// while writes always use the primary.
    #[serde(default)]
    pub read_replicas: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryConfig {
    pub default_command_timeout_secs: u64,
    pub slow_query_threshold_ms: u64,
    /// Maximum byte size for a single text/binary cell in TextRowSet (scaffold mode).
    /// Cells wider than this limit are silently truncated by the ODBC driver.
    /// Default: 65536 (64 KiB). Increase for nvarchar(max)/xml/varbinary(max) columns.
    #[serde(default = "QueryConfig::default_max_text_bytes")]
    pub max_text_bytes: usize,
    /// Per-query timeout override (seconds). `None` uses `default_command_timeout_secs`.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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
            timeout_secs: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

    /// Load config automatically:
    /// - If path ends with `.enc` **and** `STATIQ_CONFIG_KEY` env var is set,
    ///   decrypt with AES-256-GCM then deserialise.
    /// - Otherwise fall back to plain JSON.
    pub fn from_file_auto(path: &str) -> Result<Self, crate::error::SqlError> {
        #[cfg(feature = "encrypted-config")]
        if path.ends_with(".enc") {
            if let Ok(key) = std::env::var("STATIQ_CONFIG_KEY") {
                return Self::from_encrypted_file(path, &key);
            }
        }
        Self::from_file(path)
    }

    /// Decrypt an AES-256-GCM config file.
    ///
    /// File format: `base64(12-byte nonce || ciphertext || 16-byte GCM tag)`.
    /// `key_hex` must be a 64-character hex string (32 bytes = 256-bit key).
    #[cfg(feature = "encrypted-config")]
    pub fn from_encrypted_file(path: &str, key_hex: &str) -> Result<Self, crate::error::SqlError> {
        use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
        use base64::Engine as _;

        let raw = std::fs::read_to_string(path)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|e| crate::error::SqlError::Crypto(e.to_string()))?;

        if bytes.len() < 12 {
            return Err(crate::error::SqlError::Crypto("Encrypted file too short".to_string()));
        }

        let key_bytes = hex::decode(key_hex)
            .map_err(|e| crate::error::SqlError::Crypto(format!("Invalid key hex: {e}")))?;
        if key_bytes.len() != 32 {
            return Err(crate::error::SqlError::Crypto("Key must be 32 bytes (64 hex chars)".to_string()));
        }

        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|e| crate::error::SqlError::Crypto(e.to_string()))?;
        let nonce = aes_gcm::Nonce::from_slice(&bytes[..12]);
        let plaintext = cipher.decrypt(nonce, &bytes[12..])
            .map_err(|_| crate::error::SqlError::Crypto("Decryption failed — wrong key or corrupted file".to_string()))?;

        serde_json::from_slice(&plaintext).map_err(crate::error::SqlError::from)
    }

    /// Encrypt the config and write to `path` using AES-256-GCM.
    ///
    /// File format: `base64(12-byte nonce || ciphertext || 16-byte GCM tag)`.
    /// `key_hex` must be a 64-character hex string (32 bytes = 256-bit key).
    #[cfg(feature = "encrypted-config")]
    pub fn to_encrypted_file(&self, path: &str, key_hex: &str) -> Result<(), crate::error::SqlError> {
        use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead, aead::OsRng};
        use aes_gcm::aead::rand_core::RngCore as _;
        use base64::Engine as _;

        let key_bytes = hex::decode(key_hex)
            .map_err(|e| crate::error::SqlError::Crypto(format!("Invalid key hex: {e}")))?;
        if key_bytes.len() != 32 {
            return Err(crate::error::SqlError::Crypto("Key must be 32 bytes (64 hex chars)".to_string()));
        }

        let plaintext = serde_json::to_vec(self)?;

        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|e| crate::error::SqlError::Crypto(e.to_string()))?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);

        let mut ciphertext = cipher.encrypt(nonce, plaintext.as_slice())
            .map_err(|e| crate::error::SqlError::Crypto(e.to_string()))?;

        let mut payload = nonce_bytes.to_vec();
        payload.append(&mut ciphertext);

        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
        std::fs::write(path, encoded)?;
        Ok(())
    }
}
