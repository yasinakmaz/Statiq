//! Circuit breaker for SQL pool checkout.
//!
//! Prevents a cascade of connection failures from overwhelming the pool when
//! the database is unavailable. The breaker transitions:
//!
//! ```text
//! Closed ──(failures ≥ threshold)──► Open ──(recovery_timeout elapsed)──► HalfOpen
//!   ▲                                                                          │
//!   └────────────────────── (next checkout succeeds) ───────────────────────── ┘
//! ```
//!
//! In the `Open` state all checkout attempts immediately return
//! [`SqlError::PoolExhausted`] without touching the ODBC pool.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::SqlError;

/// Current breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Too many failures — all requests are rejected immediately.
    Open,
    /// Recovery window — the next request is allowed through as a probe.
    HalfOpen,
}

/// Thread-safe circuit breaker.
pub struct CircuitBreaker {
    failure_count: AtomicU32,
    failure_threshold: u32,
    recovery_timeout: Duration,
    /// Unix millis of the last failure.
    last_failure_ms: AtomicU64,
    state: Mutex<CircuitState>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: consecutive failures before opening.
    /// - `recovery_timeout`: how long to stay `Open` before transitioning to `HalfOpen`.
    pub fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            failure_count: AtomicU32::new(0),
            failure_threshold,
            recovery_timeout,
            last_failure_ms: AtomicU64::new(0),
            state: Mutex::new(CircuitState::Closed),
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Check whether a request may proceed.
    ///
    /// Returns `Ok(())` for `Closed` / `HalfOpen`, or
    /// `Err(SqlError::PoolExhausted)` when `Open`.
    pub fn check(&self) -> Result<(), SqlError> {
        let mut state = self.state.lock().unwrap();

        match *state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                let last = self.last_failure_ms.load(Ordering::Relaxed);
                let elapsed_ms = Self::now_ms().saturating_sub(last);
                if Duration::from_millis(elapsed_ms) >= self.recovery_timeout {
                    *state = CircuitState::HalfOpen;
                    Ok(())
                } else {
                    Err(SqlError::PoolExhausted {
                        timeout_ms: self.recovery_timeout.as_millis() as u64,
                    })
                }
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    /// Record a successful operation — resets failure count and closes the circuit.
    pub fn record_success(&self) {
        self.failure_count.store(0, Ordering::Relaxed);
        *self.state.lock().unwrap() = CircuitState::Closed;
    }

    /// Record a failed operation — increments the counter; opens the circuit when
    /// the threshold is reached.
    pub fn record_failure(&self) {
        let prev = self.failure_count.fetch_add(1, Ordering::Relaxed);
        self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);

        if prev + 1 >= self.failure_threshold {
            *self.state.lock().unwrap() = CircuitState::Open;
        }
    }

    /// Current circuit state (snapshot).
    pub fn state(&self) -> CircuitState {
        *self.state.lock().unwrap()
    }
}
