//! Streaming cursor support for large result sets.
//!
//! Yields entity rows one batch at a time via `async_stream`, avoiding the need
//! to materialise the entire result set in memory before processing.
//!
//! Enable with the `streaming` feature flag.

use tokio_util::sync::CancellationToken;

use crate::entity::SqlEntity;
use crate::error::SqlError;
use crate::params::OdbcParam;
use crate::pool::Pool;

/// Stream rows from a SELECT query as typed entities.
///
/// Rows are fetched in a `spawn_blocking` task (ODBC is synchronous), then
/// yielded item-by-item to the async consumer. The entire result set is
/// fetched in one blocking call before streaming begins — for true lazy
/// server-side cursors a future version will refactor the ODBC layer.
///
/// # Example
/// ```ignore
/// let stream = stream_entities::<User>(
///     pool.clone(), "SELECT * FROM Users WHERE Active = @active".into(),
///     vec![OdbcParam::new("active", ParamValue::Bool(true))],
///     65536, ct.clone(),
/// );
/// pin_mut!(stream);
/// while let Some(user) = stream.try_next().await? { ... }
/// ```
pub fn stream_entities<T>(
    pool: Pool,
    sql: String,
    params: Vec<OdbcParam>,
    max_text_bytes: usize,
    token: CancellationToken,
) -> impl futures::Stream<Item = Result<T, SqlError>>
where
    T: SqlEntity + Send + 'static,
{
    async_stream::try_stream! {
        let mut conn = pool.checkout(&token).await?;
        let sql_owned = sql.clone();
        let params_owned = params.clone();

        let rows: Vec<crate::row::OdbcRow> = tokio::select! {
            biased;
            _ = token.cancelled() => Err(SqlError::Cancelled),
            res = tokio::task::spawn_blocking(move || {
                conn.execute_query_sync(&sql_owned, &params_owned, max_text_bytes)
            }) => res.map_err(|e| SqlError::config(e.to_string()))
                .and_then(|r| r),
        }?;

        for row in rows {
            yield T::from_row(&row)?;
        }
    }
}
