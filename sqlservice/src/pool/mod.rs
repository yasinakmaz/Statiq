pub mod connection;
pub mod metrics;
pub mod pooled;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::PoolConfig;
use crate::error::SqlError;
use connection::OdbcConn;
use metrics::PoolMetrics;
pub use pooled::PooledConn;

/// The shared pool state, wrapped in Arc for clone-ability.
struct PoolInner {
    connection_string: String,
    config: PoolConfig,
    idle_tx: Sender<OdbcConn>,
    idle_rx: Receiver<OdbcConn>,
    metrics: PoolMetrics,
    /// Notified whenever a connection is returned to the idle queue.
    /// Used by `checkout_inner` to replace the 5ms spin-sleep.
    notify: Arc<Notify>,
}

#[derive(Clone)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

impl Pool {
    /// Build a new pool, eagerly opening `min_size` connections.
    pub fn new(connection_string: String, config: PoolConfig) -> Result<Self, SqlError> {
        let (idle_tx, idle_rx) = bounded::<OdbcConn>(config.max_size as usize);

        let pool = Self {
            inner: Arc::new(PoolInner {
                connection_string,
                config,
                idle_tx,
                idle_rx,
                metrics: PoolMetrics::default(),
                notify: Arc::new(Notify::new()),
            }),
        };

        pool.populate_min()?;
        Ok(pool)
    }

    fn populate_min(&self) -> Result<(), SqlError> {
        for _ in 0..self.inner.config.min_size {
            let conn = self.create_connection()?;
            self.inner
                .idle_tx
                .send(conn)
                .map_err(|_| SqlError::config("Pool channel closed during init"))?;
            self.inner.metrics.idle_count.fetch_add(1, Ordering::Relaxed);
        }
        info!(
            min_size = self.inner.config.min_size,
            "Pool initialised"
        );
        Ok(())
    }

    fn create_connection(&self) -> Result<OdbcConn, SqlError> {
        // We lazily initialise a static ODBC Environment.
        static ENV: std::sync::OnceLock<odbc_api::Environment> = std::sync::OnceLock::new();
        let env: &'static odbc_api::Environment = ENV.get_or_init(|| {
            odbc_api::Environment::new().expect("Failed to create ODBC environment")
        });

        let conn = OdbcConn::new(env, &self.inner.connection_string)?;
        self.inner.metrics.total_created.fetch_add(1, Ordering::Relaxed);
        debug!("Created new ODBC connection");
        Ok(conn)
    }

    /// Check out a connection. Blocks (async) until one is available or timeout.
    pub async fn checkout(&self, token: &CancellationToken) -> Result<PooledConn, SqlError> {
        let timeout = Duration::from_millis(self.inner.config.checkout_timeout_ms);
        self.inner.metrics.waiters.fetch_add(1, Ordering::Relaxed);

        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            result = self.checkout_inner(timeout) => result,
        };

        self.inner.metrics.waiters.fetch_sub(1, Ordering::Relaxed);

        if result.is_ok() {
            self.inner.metrics.active_count.fetch_add(1, Ordering::Relaxed);
            self.inner.metrics.idle_count.fetch_sub(1, Ordering::Relaxed);
            self.inner.metrics.total_checkouts.fetch_add(1, Ordering::Relaxed);
        }

        result
    }

    async fn checkout_inner(&self, timeout: Duration) -> Result<PooledConn, SqlError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let notify = self.inner.notify.clone();

        loop {
            // 1. Try idle queue
            match self.inner.idle_rx.try_recv() {
                Ok(conn) => {
                    return Ok(PooledConn::new(conn, self.inner.idle_tx.clone(), notify));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    return Err(SqlError::config("Pool channel disconnected"));
                }
            }

            // 2. If under max, create a new one
            let active = self.inner.metrics.active_count.load(Ordering::Relaxed);
            let idle   = self.inner.metrics.idle_count.load(Ordering::Relaxed);
            if active + idle < self.inner.config.max_size as u64 {
                let conn = self.create_connection()?;
                return Ok(PooledConn::new(conn, self.inner.idle_tx.clone(), notify));
            }

            // 3. Check timeout
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                self.inner.metrics.total_timeouts.fetch_add(1, Ordering::Relaxed);
                warn!("Pool checkout timeout after {}ms", self.inner.config.checkout_timeout_ms);
                return Err(SqlError::PoolExhausted {
                    timeout_ms: self.inner.config.checkout_timeout_ms,
                });
            }

            // 4. Wait for a connection to be returned (no busy-spin).
            //    Cap the wait at `remaining` so we always honour the deadline.
            let _ = tokio::time::timeout(remaining, notify.notified()).await;
        }
    }

    pub fn metrics(&self) -> metrics::MetricsSnapshot {
        self.inner.metrics.snapshot()
    }

    /// Increment the deadlock counter (called by `with_retry` on each deadlock detection).
    pub fn record_deadlock(&self) {
        self.inner.metrics.total_deadlocks.fetch_add(1, Ordering::Relaxed);
    }

    /// Spawn the background validation task.
    pub fn spawn_validator(&self, shutdown: CancellationToken) {
        let pool = self.clone();
        let interval_secs = self.inner.config.validation_interval_secs;
        let idle_timeout = Duration::from_secs(self.inner.config.idle_timeout_secs);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        pool.validate_idle(idle_timeout);
                    }
                }
            }
            info!("Pool validator stopped");
        });
    }

    fn validate_idle(&self, idle_timeout: Duration) {
        let max_lifetime = if self.inner.config.max_lifetime_secs > 0 {
            Some(Duration::from_secs(self.inner.config.max_lifetime_secs))
        } else {
            None
        };

        let mut keep = Vec::new();
        let idle_count = self.inner.metrics.idle_count.load(Ordering::Relaxed) as usize;

        for _ in 0..idle_count {
            match self.inner.idle_rx.try_recv() {
                Ok(mut conn) => {
                    let exceeded_lifetime = max_lifetime
                        .map(|lt| conn.created_at.elapsed() > lt)
                        .unwrap_or(false);
                    if exceeded_lifetime {
                        self.inner.metrics.total_destroyed.fetch_add(1, Ordering::Relaxed);
                        debug!("Closing connection (max_lifetime exceeded)");
                    } else if conn.last_used_at.elapsed() > idle_timeout {
                        self.inner.metrics.total_destroyed.fetch_add(1, Ordering::Relaxed);
                        debug!("Closing idle connection (idle_timeout exceeded)");
                    } else if conn.validate() {
                        keep.push(conn);
                    } else {
                        self.inner.metrics.total_destroyed.fetch_add(1, Ordering::Relaxed);
                        error!("Dropping invalid connection");
                    }
                }
                Err(_) => break,
            }
        }

        let kept = keep.len();
        for conn in keep {
            let _ = self.inner.idle_tx.send(conn);
        }
        self.inner.metrics.idle_count.store(kept as u64, Ordering::Relaxed);
    }
}
