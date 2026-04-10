//! Type-safe ORDER BY builder.
//!
//! Column names are validated against a strict allowlist of characters —
//! only alphanumeric, underscore, dot, and bracket characters are accepted.
//! This prevents SQL injection through dynamically constructed sort clauses.

use crate::error::SqlError;

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Asc  => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// Type-safe ORDER BY clause builder.
///
/// ```rust
/// # use statiq::query::order::{OrderBy, SortDir};
/// let clause = OrderBy::new()
///     .asc("LastName")
///     .desc("CreatedAt")
///     .to_sql()
///     .unwrap();
/// assert_eq!(clause, "LastName ASC, CreatedAt DESC");
/// ```
#[derive(Debug, Default, Clone)]
pub struct OrderBy {
    clauses: Vec<(String, SortDir)>,
}

impl OrderBy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an ascending sort column.
    pub fn asc(mut self, col: impl Into<String>) -> Self {
        self.clauses.push((col.into(), SortDir::Asc));
        self
    }

    /// Add a descending sort column.
    pub fn desc(mut self, col: impl Into<String>) -> Self {
        self.clauses.push((col.into(), SortDir::Desc));
        self
    }

    /// Build the SQL ORDER BY clause string (without the `ORDER BY` keyword).
    ///
    /// Returns `Err` if any column name contains disallowed characters.
    pub fn to_sql(&self) -> Result<String, SqlError> {
        if self.clauses.is_empty() {
            return Ok(String::new());
        }

        let mut parts = Vec::with_capacity(self.clauses.len());
        for (col, dir) in &self.clauses {
            Self::validate_column(col)?;
            parts.push(format!("{col} {}", dir.as_sql()));
        }
        Ok(parts.join(", "))
    }

    fn validate_column(col: &str) -> Result<(), SqlError> {
        if col.is_empty() {
            return Err(SqlError::config("ORDER BY column name must not be empty"));
        }
        for ch in col.chars() {
            if !matches!(ch,
                'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '.' | '[' | ']'
            ) {
                return Err(SqlError::config(format!(
                    "Disallowed character in ORDER BY column '{col}': '{ch}'"
                )));
            }
        }
        Ok(())
    }
}
