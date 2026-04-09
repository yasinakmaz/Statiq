use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::cache::{NoCache, RedisCache};
use crate::config::AppConfig;
use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::pool::Pool;
use crate::service::SqlService;
use crate::sproc::SprocService;

/// Builder for `SqlService<T>`.
pub struct SqlServiceFactory {
    config_path: Option<String>,
    config: Option<AppConfig>,
    shutdown: Option<CancellationToken>,
    /// When true, initialise the global tracing subscriber from `AppConfig::logging`.
    /// Defaults to `false` — libraries should not initialise global state.
    /// The application is responsible for setting up its own subscriber.
    init_logging: bool,
}

impl SqlServiceFactory {
    pub fn new() -> Self {
        Self {
            config_path: None,
            config: None,
            shutdown: None,
            init_logging: false,
        }
    }

    /// Opt-in: let the factory initialise the global tracing subscriber.
    ///
    /// Useful in simple binaries that do not set up their own subscriber.
    /// In Axum/Tauri applications that configure their own tracing, leave
    /// this at the default (`false`) to avoid a silent double-init conflict.
    pub fn with_logging(mut self, enable: bool) -> Self {
        self.init_logging = enable;
        self
    }

    pub fn config_path(mut self, path: impl Into<String>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    pub fn config(mut self, cfg: AppConfig) -> Self {
        self.config = Some(cfg);
        self
    }

    pub fn shutdown(mut self, token: CancellationToken) -> Self {
        self.shutdown = Some(token);
        self
    }

    /// Build with Redis cache.
    pub async fn build_with_cache<T: SqlEntity>(
        self,
    ) -> Result<SqlService<T, RedisCache>, SqlError> {
        let shutdown = self.shutdown.clone();
        let init_logging = self.init_logging;
        let cfg = self.load_config()?;

        if init_logging {
            crate::logging::init(&cfg.logging);
        }

        let pool = Pool::new(cfg.mssql.connection_string.clone(), cfg.mssql.pool.clone())?;
        pool.spawn_validator(shutdown.unwrap_or_else(CancellationToken::new));

        let cache = RedisCache::new(&cfg.redis).await?;
        info!(table = T::TABLE_NAME, "SqlService (Redis) ready");
        Ok(SqlService::new(pool, cache, cfg.mssql.query))
    }

    /// Build without Redis cache.
    pub async fn build<T: SqlEntity>(self) -> Result<SqlService<T, NoCache>, SqlError> {
        let shutdown = self.shutdown.clone();
        let init_logging = self.init_logging;
        let cfg = self.load_config()?;

        if init_logging {
            crate::logging::init(&cfg.logging);
        }

        let pool = Pool::new(cfg.mssql.connection_string.clone(), cfg.mssql.pool.clone())?;
        pool.spawn_validator(shutdown.unwrap_or_else(CancellationToken::new));

        info!(table = T::TABLE_NAME, "SqlService (no cache) ready");
        Ok(SqlService::new(pool, NoCache, cfg.mssql.query))
    }

    /// Build a [`SprocService`] (stored-procedure execution, no entity type required).
    pub async fn build_sproc(self) -> Result<SprocService, SqlError> {
        let shutdown = self.shutdown.clone();
        let init_logging = self.init_logging;
        let cfg = self.load_config()?;

        if init_logging {
            crate::logging::init(&cfg.logging);
        }

        let pool = Pool::new(cfg.mssql.connection_string.clone(), cfg.mssql.pool.clone())?;
        pool.spawn_validator(shutdown.unwrap_or_else(CancellationToken::new));

        info!("SprocService ready");
        Ok(SprocService::new(pool, cfg.mssql.query))
    }

    fn load_config(self) -> Result<AppConfig, SqlError> {
        if let Some(cfg) = self.config {
            return Ok(cfg);
        }
        let path = self.config_path.unwrap_or_else(|| "config.json".to_string());
        AppConfig::from_file(&path)
    }
}

impl Default for SqlServiceFactory {
    fn default() -> Self {
        Self::new()
    }
}
