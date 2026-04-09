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
        let select_sql = format!("SELECT {select_cols} FROM {fq}");

        // parse.rs guarantees pk_field() is always Some after successful parse.
        let pk = info.pk_field().expect("SqlEntity: missing PK field — parse.rs should have caught this");
        let pk_col  = pk.sql_name.as_str();
        let pk_rust = pk.rust_name.as_str();

        let select_by_pk_sql = format!(
            "SELECT {select_cols} FROM {fq} WHERE {pk_col} = @{pk_col}"
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

        // DELETE
        let delete_sql = format!("DELETE FROM {fq} WHERE {pk_col} = @{pk_col}");

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

        // COUNT / EXISTS
        let count_sql = format!("SELECT COUNT_BIG(*) FROM {fq}");
        let exists_sql = format!("SELECT CAST(1 AS BIT) FROM {fq} WHERE {pk_col} = @{pk_col}");

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
            upsert_sql,
            count_sql,
            exists_sql,
            cache_prefix,
        }
    }
}
