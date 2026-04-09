//! # Statiq
//!
//! Zero-overhead, compile-time MSSQL service for Rust.
//!
//! ## Quick start
//! ```ignore
//! use statiq::{SqlServiceFactory, SqlEntity, params};
//!
//! #[derive(SqlEntity)]
//! #[sql_table("Users", schema = "dbo")]
//! pub struct User {
//!     #[sql_primary_key(identity)]
//!     pub id: i32,
//!     #[sql_column("UserName")]
//!     pub name: String,
//!     pub active: bool,
//! }
//!
//! let svc = SqlServiceFactory::new()
//!     .config_path("config.json")
//!     .build::<User>()
//!     .await?;
//!
//! let token = tokio_util::sync::CancellationToken::new();
//! let user = svc.get_by_id(1, &token).await?;
//! ```

pub mod cache;
pub mod config;
pub mod entity;
pub mod error;
pub mod factory;
pub mod logging;
pub mod params;
pub mod pool;
pub mod query;
pub mod repository;
pub mod row;
pub mod service;
pub mod sproc;
pub mod transaction;

#[cfg(feature = "testing")]
pub mod testing;

// ── Re-exports (public API surface) ──────────────────────────────────────────
pub use cache::{CacheLayer, NoCache, RedisCache};
pub use config::AppConfig;
pub use entity::SqlEntity;
pub use error::SqlError;
pub use factory::SqlServiceFactory;
pub use params::{OdbcParam, ParamValue, PkValue};
pub use pool::Pool;
pub use repository::SqlRepository;
pub use row::OdbcRow;
pub use service::SqlService;
pub use sproc::{
    FromResultSet, MultiReader, Required, Scalar, Single, SprocPagedResult, SprocParams,
    SprocResult, SprocService,
};
pub use transaction::Transaction;

// Re-export the derive macro
pub use statiq_macros::SqlEntity;