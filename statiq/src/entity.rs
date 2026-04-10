use serde::{de::DeserializeOwned, Serialize};

use crate::error::SqlError;
use crate::params::{OdbcParam, PkValue};
use crate::row::OdbcRow;

/// Implemented automatically by `#[derive(SqlEntity)]`.
///
/// Supertrait bounds:
/// - `Serialize + DeserializeOwned` — required for Redis caching
/// - `Send + Sync + 'static` — required for async pool dispatch
pub trait SqlEntity: Sized + Send + Sync + 'static + Serialize + DeserializeOwned {
    const TABLE_NAME: &'static str;
    const SCHEMA: &'static str;

    const SELECT_COLS: &'static str;
    const SELECT_SQL: &'static str;
    const SELECT_BY_PK_SQL: &'static str;
    const INSERT_SQL: &'static str;
    const UPDATE_SQL: &'static str;
    const DELETE_SQL: &'static str;
    /// Physical DELETE — always a real DELETE FROM even when soft-delete is active.
    const HARD_DELETE_SQL: &'static str;
    const UPSERT_SQL: &'static str;
    const COUNT_SQL: &'static str;
    const EXISTS_SQL: &'static str;

    const PK_COLUMN: &'static str;
    const PK_IS_IDENTITY: bool;

    const CACHE_PREFIX: &'static str;
    const COLUMN_COUNT: usize;

    fn from_row(row: &OdbcRow) -> Result<Self, SqlError>;
    fn to_params(&self) -> Vec<OdbcParam>;
    fn pk_value(&self) -> PkValue;

    /// Optional row-level security filter appended to all SELECT queries.
    ///
    /// Return a SQL fragment (without `WHERE`/`AND`) to restrict which rows are
    /// visible to the current user. The fragment may reference `@__tenant_id`
    /// or other parameters. The default implementation returns `None` (no RLS).
    ///
    /// Override this in a manual `impl SqlEntity for MyEntity` block when RLS
    /// is needed without the `#[sql_tenant_id]` macro attribute.
    fn rls_filter() -> Option<&'static str> {
        None
    }
}
