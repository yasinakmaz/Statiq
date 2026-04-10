//! Named query registry — compile-time SQL string lookup.
//!
//! Provides a lightweight map from static names to static SQL strings,
//! useful for centralising all query definitions in one place.

use std::collections::HashMap;

/// Registry that maps named identifiers to SQL strings.
///
/// # Example
/// ```rust
/// # use statiq::query::registry::QueryRegistry;
/// let registry = QueryRegistry::new()
///     .register("active_users", "SELECT * FROM [dbo].[Users] WHERE Active = @active")
///     .register("admin_users",  "SELECT * FROM [dbo].[Users] WHERE Role = 'Admin'");
///
/// assert_eq!(registry.get("active_users"), Some("SELECT * FROM [dbo].[Users] WHERE Active = @active"));
/// ```
#[derive(Debug, Default, Clone)]
pub struct QueryRegistry {
    queries: HashMap<&'static str, &'static str>,
}

impl QueryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named SQL string.
    pub fn register(mut self, name: &'static str, sql: &'static str) -> Self {
        self.queries.insert(name, sql);
        self
    }

    /// Look up a registered SQL string by name.
    pub fn get(&self, name: &str) -> Option<&'static str> {
        self.queries.get(name).copied()
    }
}
