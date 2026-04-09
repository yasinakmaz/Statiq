pub mod attrs;
pub mod parse;
pub mod sql_gen;
pub mod mapper_gen;
pub mod binder_gen;

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;
use parse::StructInfo;
use sql_gen::GeneratedSql;
use mapper_gen::generate_from_row;
use binder_gen::{generate_pk_value, generate_to_params};

pub fn derive_sql_entity(input: DeriveInput) -> syn::Result<TokenStream> {
    let info = StructInfo::parse(&input)?;
    let sql = GeneratedSql::from_struct(&info);

    let struct_name = &info.struct_name;

    let _table_name_lit  = &sql.table_name;
    let schema_lit       = &sql.schema;
    let fq_table_lit     = &sql.fq_table;
    let select_cols_lit  = &sql.select_cols;
    let select_sql_lit   = &sql.select_sql;
    let select_pk_lit    = &sql.select_by_pk_sql;
    let insert_sql_lit   = &sql.insert_sql;
    let update_sql_lit   = &sql.update_sql;
    let delete_sql_lit   = &sql.delete_sql;
    let upsert_sql_lit   = &sql.upsert_sql;
    let count_sql_lit    = &sql.count_sql;
    let exists_sql_lit   = &sql.exists_sql;
    let cache_prefix_lit = &sql.cache_prefix;

    // parse.rs guarantees pk_field() is always Some at this point.
    let pk_field = info.pk_field().expect("SqlEntity: missing PK — parse.rs should have caught this");
    let pk_col = pk_field.sql_name.as_str();
    let pk_is_identity = pk_field.pk_is_identity;
    let col_count = info.active_fields().count();

    let from_row_fn  = generate_from_row(&info);
    let to_params_fn = generate_to_params(&info);
    let pk_value_fn  = generate_pk_value(&info);

    Ok(quote! {
        impl statiq::entity::SqlEntity for #struct_name {
            const TABLE_NAME:      &'static str = #fq_table_lit;
            const SCHEMA:          &'static str = #schema_lit;
            const SELECT_COLS:     &'static str = #select_cols_lit;
            const SELECT_SQL:      &'static str = #select_sql_lit;
            const SELECT_BY_PK_SQL:&'static str = #select_pk_lit;
            const INSERT_SQL:      &'static str = #insert_sql_lit;
            const UPDATE_SQL:      &'static str = #update_sql_lit;
            const DELETE_SQL:      &'static str = #delete_sql_lit;
            const UPSERT_SQL:      &'static str = #upsert_sql_lit;
            const COUNT_SQL:       &'static str = #count_sql_lit;
            const EXISTS_SQL:      &'static str = #exists_sql_lit;
            const PK_COLUMN:       &'static str = #pk_col;
            const PK_IS_IDENTITY:  bool         = #pk_is_identity;
            const CACHE_PREFIX:    &'static str = #cache_prefix_lit;
            const COLUMN_COUNT:    usize        = #col_count;

            #from_row_fn
            #to_params_fn
            #pk_value_fn
        }
    })
}
