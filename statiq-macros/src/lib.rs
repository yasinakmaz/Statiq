mod entity;
mod params;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

/// Derive macro that implements `statiq::entity::SqlEntity` for a struct.
///
/// Attributes:
/// - `#[sql_table("TableName", schema = "dbo")]` — on the struct
/// - `#[sql_primary_key(identity)]` — on one field
/// - `#[sql_column("ColName")]` — override column name
/// - `#[sql_ignore]` — exclude field from all SQL (not even SELECT)
/// - `#[sql_computed]` — DB-computed column; SELECT only, excluded from INSERT/UPDATE/MERGE
/// - `#[sql_default]` — server-side default; excluded from INSERT, included in SELECT/UPDATE
///
/// # Example
/// ```ignore
/// #[derive(SqlEntity)]
/// #[sql_table("Users", schema = "dbo")]
/// pub struct User {
///     #[sql_primary_key(identity)]
///     pub id: i32,
///     #[sql_column("UserName")]
///     pub name: String,
///     pub active: bool,
///     #[sql_computed]
///     pub full_name: String,  // DB-computed, never written
///     #[sql_default]
///     pub created_at: chrono::DateTime<chrono::Utc>,  // GETDATE() default, not in INSERT
/// }
/// ```
#[proc_macro_derive(SqlEntity, attributes(sql_table, sql_column, sql_primary_key, sql_ignore, sql_computed, sql_default))]
pub fn derive_sql_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    entity::derive_sql_entity(input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
