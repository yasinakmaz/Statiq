/// Runtime query builder — wraps static SQL templates with dynamic extensions.

use crate::error::SqlError;

/// Validate a WHERE filter string against a strict allowlist.
///
/// Blocks SQL injection by rejecting dangerous keywords and characters.
/// Only column comparisons, logical operators, and parameterized `@name`
/// placeholders are permitted.
pub fn validate_filter(filter: &str) -> Result<(), SqlError> {
    let upper = filter.to_ascii_uppercase();
    static BANNED: &[&str] = &[
        ";", "--", "/*", "*/", "EXEC", "EXECUTE", "XP_", "SP_",
        "DROP", "TRUNCATE", "ALTER", "CREATE", "INSERT", "UPDATE",
        "DELETE", "OPENROWSET", "BULK", "WAITFOR", "SHUTDOWN",
    ];
    for banned in BANNED {
        if upper.contains(banned) {
            return Err(SqlError::config(format!("Disallowed token in filter: {banned}")));
        }
    }
    for ch in filter.chars() {
        if !matches!(ch,
            'A'..='Z' | 'a'..='z' | '0'..='9' | ' ' | '\t' | '\n' |
            '@' | '_' | '.' | '[' | ']' | '(' | ')' |
            '=' | '<' | '>' | '!' | '\'' | '%' | ',' | '-' | '+' | '/'
        ) {
            return Err(SqlError::config(format!("Disallowed character in filter: '{ch}'")));
        }
    }
    Ok(())
}

pub struct QueryBuilder {
    base_sql: String,
    where_clause: Option<String>,
    order_by: Option<String>,
    offset: Option<i64>,
    fetch: Option<i64>,
}

impl QueryBuilder {
    pub fn new(base_sql: impl Into<String>) -> Self {
        Self {
            base_sql: base_sql.into(),
            where_clause: None,
            order_by: None,
            offset: None,
            fetch: None,
        }
    }

    pub fn where_clause(mut self, clause: impl Into<String>) -> Self {
        self.where_clause = Some(clause.into());
        self
    }

    pub fn order_by(mut self, col: impl Into<String>) -> Self {
        self.order_by = Some(col.into());
        self
    }

    pub fn paged(mut self, page: i64, page_size: i64) -> Self {
        self.offset = Some((page - 1) * page_size);
        self.fetch = Some(page_size);
        self
    }

    pub fn build(self) -> String {
        let mut sql = self.base_sql;
        let has_order_by = self.order_by.is_some();

        if let Some(w) = self.where_clause {
            sql.push_str(" WHERE ");
            sql.push_str(&w);
        }

        if let Some(ref ord) = self.order_by {
            sql.push_str(" ORDER BY ");
            sql.push_str(ord);
        }

        if let (Some(offset), Some(fetch)) = (self.offset, self.fetch) {
            if !has_order_by {
                sql.push_str(" ORDER BY (SELECT NULL)");
            }
            sql.push_str(&format!(
                " OFFSET {offset} ROWS FETCH NEXT {fetch} ROWS ONLY"
            ));
        }

        sql
    }
}

/// Build a paged SELECT from a static base SELECT SQL.
pub fn paged_sql(select_sql: &str, pk_col: &str, page: i64, page_size: i64) -> String {
    QueryBuilder::new(select_sql)
        .order_by(pk_col)
        .paged(page, page_size)
        .build()
}

/// Build a WHERE-filtered SELECT.
pub fn filtered_sql(select_sql: &str, filter: &str) -> String {
    QueryBuilder::new(select_sql)
        .where_clause(filter)
        .build()
}

/// Build batch INSERT SQL strings (one per row, to be executed in a transaction).
pub fn batch_insert_sqls(insert_sql: &str, count: usize) -> Vec<String> {
    (0..count).map(|_| insert_sql.to_owned()).collect()
}
