use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use crossbeam_channel::Sender;
use tokio::sync::Notify;
use super::connection::OdbcConn;

/// RAII guard — returns the connection to the pool on drop
/// and wakes a waiting `checkout` via the shared `Notify`.
pub struct PooledConn {
    pub(crate) conn: Option<OdbcConn>,
    pub(crate) return_tx: Sender<OdbcConn>,
    pub(crate) notify: Arc<Notify>,
}

impl PooledConn {
    pub(crate) fn new(conn: OdbcConn, return_tx: Sender<OdbcConn>, notify: Arc<Notify>) -> Self {
        Self { conn: Some(conn), return_tx, notify }
    }

    /// Consume the guard and return ownership of the inner connection
    /// (used by Transaction to take ownership).
    pub(crate) fn take(mut self) -> (OdbcConn, Sender<OdbcConn>, Arc<Notify>) {
        (self.conn.take().unwrap(), self.return_tx.clone(), self.notify.clone())
    }
}

impl Deref for PooledConn {
    type Target = OdbcConn;
    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().unwrap()
    }
}

impl DerefMut for PooledConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().unwrap()
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            conn.last_used_at = std::time::Instant::now();
            // Mark for session reset so the next checkout starts with clean state.
            conn.needs_reset = true;
            // Best-effort return; if pool is gone, the connection is simply dropped.
            let _ = self.return_tx.send(conn);
            // Wake one task waiting in checkout_inner so it can claim the returned connection.
            self.notify.notify_one();
        }
    }
}
