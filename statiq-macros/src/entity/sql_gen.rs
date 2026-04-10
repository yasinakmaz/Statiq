use super::parse::StructInfo;

/// Generates all `const` SQL strings from the parsed struct metadata.
pub struct GeneratedSql {
    pub table_name: String,
    pub schema: String,
    pub fq_table: String, // [schema].[table]
    pub select_cols: String,
    pub select_sql: String,
    pub select_by_pk_sql: String,
    pub insert_sql: String,
    pub update_sql: String,
    pub delete_sql: String,
    /// Physical DELETE — only differs from `delete_sql` when soft-delete is active.
    pub hard_delete_sql: String,
    pub upsert_sql: String,
    pub count_sql: String,
    pub exists_sql: String,
    pub cache_prefix: String,
}

impl GeneratedSql {
    pub fn from_struct(info: &StructInfo) -> Self {
        let schema = &info.schema;
        let table = &info.table_name;
        let fq = format!("[{schema}].[{table}]");

        // SELECT cols
        let active: Vec<&str> = info.active_fields().map(|f| f.sql_name.as_str()).collect();
        let select_cols = active.join(", ");

        // parse.rs guarantees pk_field() is always Some after successful parse.
        let pk = info.pk_field().expect("SqlEntity: missing PK field — parse.rs should have caught this");
        let pk_col  = pk.sql_name.as_str();
        let pk_rust = pk.rust_name.as_str();

        // Build optional WHERE clauses for soft-delete and tenant-id filters
        let soft_filter = info.soft_delete_col.as_deref()
            .map(|col| format!("[{col}] = 0"))
            .unwrap_or_default();
        let tenant_filter = info.tenant_id_col.as_deref()
            .map(|col| format!("[{col}] = @__tenant_id"))
            .unwrap_or_default();

        let extra_filters: Vec<&str> = [soft_filter.as_str(), tenant_filter.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect();
        let extra_where = if extra_filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", extra_filters.join(" AND "))
        };
        let extra_and = if extra_filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", extra_filters.join(" AND "))
        };

        let select_sql = format!("SELECT {select_cols} FROM {fq}{extra_where}");
        let select_by_pk_sql = format!(
            "SELECT {select_cols} FROM {fq} WHERE {pk_col} = @{pk_col}{extra_and}"
        );

        // INSERT
        let ins_fields: Vec<_> = info.insert_fields().collect();
        let ins_cols = ins_fields.iter().map(|f| f.sql_name.as_str()).collect::<Vec<_>>().join(", ");
        let ins_params = ins_fields.iter().map(|f| format!("@{}", f.rust_name)).collect::<Vec<_>>().join(", ");
        let insert_sql = if pk.pk_is_identity {
            format!(
                "INSERT INTO {fq} ({ins_cols}) OUTPUT INSERTED.{pk_col} VALUES ({ins_params})"
            )
        } else {
            format!("INSERT INTO {fq} ({ins_cols}) VALUES ({ins_params})")
        };

        // UPDATE — excludes PK, computed, and ignored fields; server-default fields are included
        let upd_fields: Vec<_> = info.update_fields().collect();
        let upd_sets = upd_fields
            .iter()
            .map(|f| format!("{} = @{}", f.sql_name, f.rust_name))
            .collect::<Vec<_>>()
            .join(", ");
        let update_sql = format!("UPDATE {fq} SET {upd_sets} WHERE {pk_col} = @{pk_rust}");

        // DELETE (hard — always a physical DELETE)
        let hard_delete_sql = format!("DELETE FROM {fq} WHERE {pk_col} = @{pk_col}");

        // Soft DELETE — rewrite to UPDATE if soft-delete column is configured
        let delete_sql = if let Some(sd_col) = &info.soft_delete_col {
            format!("UPDATE {fq} SET [{sd_col}] = 1 WHERE {pk_col} = @{pk_col}")
        } else {
            hard_delete_sql.clone()
        };

        // UPSERT (MERGE)
        let merge_using_cols = ins_fields
            .iter()
            .map(|f| format!("@{} AS {}", f.rust_name, f.sql_name))
            .collect::<Vec<_>>()
            .join(", ");
        let merge_update = upd_fields
            .iter()
            .map(|f| format!("target.{} = source.{}", f.sql_name, f.sql_name))
            .collect::<Vec<_>>()
            .join(", ");
        let merge_insert_cols = ins_fields.iter().map(|f| f.sql_name.as_str()).collect::<Vec<_>>().join(", ");
        let merge_insert_vals = ins_fields.iter().map(|f| format!("source.{}", f.sql_name)).collect::<Vec<_>>().join(", ");
        let upsert_sql = format!(
            "MERGE {fq} AS target \
             USING (SELECT {merge_using_cols}) AS source ({merge_insert_cols}) \
             ON (target.{pk_col} = source.{pk_col}) \
             WHEN MATCHED THEN UPDATE SET {merge_update} \
             WHEN NOT MATCHED THEN INSERT ({merge_insert_cols}) VALUES ({merge_insert_vals});"
        );

        // COUNT / EXISTS — respect soft-delete / tenant-id filters
        let count_sql = format!("SELECT COUNT_BIG(*) FROM {fq}{extra_where}");
        let exists_sql = format!("SELECT CAST(1 AS BIT) FROM {fq} WHERE {pk_col} = @{pk_col}{extra_and}");

        let cache_prefix = format!("SqlService::{table}");

        Self {
            table_name: table.clone(),
            schema: schema.clone(),
            fq_table: fq,
            select_cols,
            select_sql,
            select_by_pk_sql,
            insert_sql,
            update_sql,
            delete_sql,
            hard_delete_sql,
            upsert_sql,
            count_sql,
            exists_sql,
            cache_prefix,
        }
    }
}
