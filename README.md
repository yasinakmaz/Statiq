# Statiq

Zero-overhead, compile-time MSSQL service library for Rust.

All SQL (SELECT, INSERT, UPDATE, DELETE, MERGE) is generated at compile time by the `#[derive(SqlEntity)]` proc-macro. There is no runtime string building, no reflection, no ORM magic. The library is async-first (Tokio), uses static dispatch throughout, and exposes `CancellationToken` on every operation for clean shutdown and request cancellation.

Targets: Axum REST APIs, Leptos SSR applications, and Tauri desktop apps.

---

## Table of Contents

1. [Crates](#1-crates)
2. [Requirements](#2-requirements)
3. [Cargo.toml Setup](#3-cargotoml-setup)
4. [config.json](#4-configjson)
5. [Defining an Entity](#5-defining-an-entity)
6. [Building a Service](#6-building-a-service)
7. [CRUD Operations (SqlRepository)](#7-crud-operations-sqlrepository)
8. [params! Macro](#8-params-macro)
9. [Transactions](#9-transactions)
10. [Stored Procedures (SprocService)](#10-stored-procedures-sprocservice)
11. [Multiple Result Sets from a Sproc](#11-multiple-result-sets-from-a-sproc)
12. [Redis Cache](#12-redis-cache)
13. [Raw Queries](#13-raw-queries)
14. [Error Handling](#14-error-handling)
15. [Testing with MockRepository](#15-testing-with-mockrepository)
16. [Axum Integration — Full Example](#16-axum-integration--full-example)
17. [Leptos SSR Integration — Full Example](#17-leptos-ssr-integration--full-example)
18. [Type Mappings (SQL Server ↔ Rust)](#18-type-mappings-sql-server--rust)
19. [Attribute Reference](#19-attribute-reference)
20. [config.json Field Reference](#20-configjson-field-reference)

---

## 1. Crates

The workspace has two published crates:

- `statiq` — the main library (connection pool, services, transactions, cache, sproc, testing).
- `statiq-macros` — the proc-macro crate. You only need to add it explicitly if you use `#[derive(SqlEntity)]` without the `statiq` re-export (very rare).

---

## 2. Requirements

- Rust 1.75+
- An ODBC driver for SQL Server installed on the machine.
  - Windows: "ODBC Driver 17 for SQL Server" or "ODBC Driver 18 for SQL Server" (download from Microsoft).
  - Linux/macOS: same drivers via Microsoft packages, plus `unixODBC`.
- A reachable SQL Server instance (2016+ recommended).

---

## 3. Cargo.toml Setup

### Minimal (no cache, no axum feature)

```toml
[dependencies]
statiq      = "0.2"
tokio       = { version = "1", features = ["full"] }
tokio-util  = { version = "0.7", features = ["full"] }
serde       = { version = "1", features = ["derive"] }
```

### With Axum

```toml
[dependencies]
statiq     = { version = "0.2", features = ["axum"] }
axum       = "0.8"
tokio      = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["full"] }
serde      = { version = "1", features = ["derive"] }
```

The `axum` feature makes `SqlError` implement `axum::response::IntoResponse`, so you can return `Result<Json<T>, SqlError>` directly from handlers.

### With Tauri

```toml
[dependencies]
statiq    = { version = "0.2", features = ["tauri"] }
tauri     = { version = "2", features = ["devtools"] }
tokio     = { version = "1", features = ["full"] }
serde     = { version = "1", features = ["derive"] }
serde_json = "1"
```

The `tauri` feature makes `SqlError` implement `serde::Serialize`, so Tauri commands can return `Result<T, SqlError>` and the error will reach the JavaScript frontend as a structured JSON object.

### With Redis Cache

```toml
[dependencies]
statiq = { version = "0.2", features = ["redis"] }
```

(Redis support is compiled in by default — no separate feature flag needed. `RedisCache` is always available, but only activated when you call `build_with_cache` and set `"enabled": true` in config.)

### For Unit Tests

```toml
[dev-dependencies]
statiq = { version = "0.2", features = ["testing"] }
```

---

## 4. config.json

Place `config.json` at the working directory root (usually the workspace root when running `cargo run`).

```json
{
  "mssql": {
    "connection_string": "Driver={ODBC Driver 17 for SQL Server};Server=localhost;Database=MyDb;UID=sa;PWD=YourPassword;",
    "pool": {
      "max_size": 100,
      "min_size": 5,
      "idle_timeout_secs": 300,
      "max_lifetime_secs": 1800,
      "checkout_timeout_ms": 5000,
      "validation_interval_secs": 60,
      "max_deadlock_retries": 3,
      "reset_connection_on_reuse": false
    },
    "query": {
      "default_command_timeout_secs": 30,
      "slow_query_threshold_ms": 1000,
      "max_text_bytes": 65536
    }
  },
  "redis": {
    "url": "redis://127.0.0.1:6379",
    "pool_size": 20,
    "default_ttl_secs": 300,
    "count_ttl_secs": 60,
    "enabled": false
  },
  "logging": {
    "level": "INFO",
    "format": "json"
  }
}
```

You can also supply a programmatic `AppConfig` instead of a file path. See [Building a Service](#6-building-a-service).

---

## 5. Defining an Entity

An entity is a plain Rust struct that mirrors a database table. Add `#[derive(SqlEntity)]` and the proc-macro generates all SQL constants and the row-mapping code at compile time.

### Basic Entity

```rust
use serde::{Deserialize, Serialize};
use statiq::SqlEntity;

#[derive(SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Users", schema = "dbo")]
pub struct User {
    #[sql_primary_key(identity)]  // IDENTITY column — excluded from INSERT
    pub id: i32,

    #[sql_column("UserName")]     // override the SQL column name
    pub name: String,

    pub email: String,
    pub active: bool,
}
```

What the macro generates (you never write this yourself):

- `User::TABLE_NAME` — `"dbo.Users"`
- `User::SELECT_SQL` — `"SELECT Id, UserName, Email, Active FROM dbo.Users"`
- `User::INSERT_SQL` — `"INSERT INTO dbo.Users (UserName, Email, Active) OUTPUT INSERTED.Id VALUES (@UserName, @Email, @Active)"`
- `User::UPDATE_SQL` — `"UPDATE dbo.Users SET UserName = @UserName, Email = @Email, Active = @Active WHERE Id = @Id"`
- `User::DELETE_SQL` — `"DELETE FROM dbo.Users WHERE Id = @Id"`
- `User::MERGE_SQL` — full `MERGE` statement for upsert
- `User::PK_COLUMN` — `"Id"`
- `User::PK_IS_IDENTITY` — `true`
- `impl User { fn from_row(row: &OdbcRow) -> Result<Self, SqlError> }` — typed row mapper
- `impl User { fn to_params(&self) -> Vec<OdbcParam> }` — parameter binder
- `impl User { fn pk_value(&self) -> PkValue }` — primary key extractor

### Entity with Optional Fields

```rust
#[derive(SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Orders", schema = "sales")]
pub struct Order {
    #[sql_primary_key(identity)]
    pub id: i64,

    pub customer_id: i32,

    pub notes: Option<String>,        // nullable nvarchar — NULL is handled automatically
    pub shipped_at: Option<chrono::DateTime<chrono::Utc>>,

    #[sql_default]                    // server-side DEFAULT GETDATE() — excluded from INSERT
    pub created_at: chrono::DateTime<chrono::Utc>,

    #[sql_computed]                   // DB computed column — SELECT only, never written
    pub total_amount: rust_decimal::Decimal,

    #[sql_ignore]                     // completely excluded from all SQL
    pub cached_display: String,
}
```

### Entity with Non-Identity Primary Key

```rust
#[derive(SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Currencies", schema = "ref")]
pub struct Currency {
    #[sql_primary_key]   // no `identity` — key is set by the application
    pub code: String,    // e.g. "USD", "EUR"

    pub name: String,
    pub symbol: String,
}
```

### Entity with UUID Primary Key

```rust
use uuid::Uuid;

#[derive(SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Documents", schema = "dbo")]
pub struct Document {
    #[sql_primary_key]
    pub id: Uuid,       // uniqueidentifier in SQL Server

    pub title: String,
    pub body: String,
}
```

---

## 6. Building a Service

`SqlServiceFactory` is the entry point. It's a builder — call `.config_path()`, `.shutdown()`, and `.with_logging()` as needed, then call one of the terminal `.build*()` methods.

### No Cache (most common)

```rust
use statiq::{SqlServiceFactory, NoCache, SqlService};
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();

let user_svc: SqlService<User, NoCache> = SqlServiceFactory::new()
    .config_path("config.json")
    .shutdown(token.clone())
    .with_logging(false)   // don't touch global tracing subscriber
    .build::<User>()
    .await?;
```

### With Redis Cache

```rust
use statiq::{SqlServiceFactory, RedisCache, SqlService};

let user_svc: SqlService<User, RedisCache> = SqlServiceFactory::new()
    .config_path("config.json")
    .shutdown(token.clone())
    .build_with_cache::<User>()
    .await?;
```

Cache is only active when `"enabled": true` in `config.json`. If disabled, `RedisCache` silently becomes a pass-through.

### Programmatic Config (no config.json file)

```rust
use statiq::{SqlServiceFactory, AppConfig};

let cfg = AppConfig {
    mssql: statiq::config::MssqlConfig {
        connection_string: std::env::var("DATABASE_URL")?,
        pool: Default::default(),
        query: Default::default(),
    },
    redis: Default::default(),
    logging: Default::default(),
};

let svc = SqlServiceFactory::new()
    .config(cfg)
    .shutdown(token.clone())
    .build::<User>()
    .await?;
```

### SprocService (for stored procedures)

```rust
use statiq::{SqlServiceFactory, SprocService};

let sproc: SprocService = SqlServiceFactory::new()
    .config_path("config.json")
    .shutdown(token.clone())
    .build_sproc()
    .await?;
```

`SprocService` does not have a type parameter — it is entity-agnostic. You can share one instance across the entire application.

### Graceful Shutdown

All services accept a `CancellationToken`. When the token is cancelled, in-flight operations are interrupted and the background pool validator stops. Wire this to your OS signal handler:

```rust
let token = CancellationToken::new();

// spawn signal handler
let shutdown_token = token.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    shutdown_token.cancel();
});

// all services share the same token
let svc = SqlServiceFactory::new()
    .config_path("config.json")
    .shutdown(token.clone())
    .build::<User>()
    .await?;
```

---

## 7. CRUD Operations (SqlRepository)

All operations are on the `SqlRepository<T>` trait, which `SqlService<T, C>` implements. Every method takes a `&CancellationToken` as the last argument.

Import the trait when you use a `dyn SqlRepository` or a generic bound:

```rust
use statiq::SqlRepository;
```

When working directly with `SqlService<T, C>` (not behind a trait object), no import is needed.

### get_by_id

```rust
// Returns Option<T> — None when no row with that PK exists.
let user: Option<User> = svc.get_by_id(42, &token).await?;
```

### get_all

```rust
// SELECT all rows.
let users: Vec<User> = svc.get_all(&token).await?;
```

### get_where

```rust
use statiq::params;

// Filter with a raw WHERE clause + typed parameters.
// params! creates a &[OdbcParam] on the stack — zero heap allocation.
let p = params! { active: true };
let users: Vec<User> = svc.get_where("Active = @active", p, &token).await?;

// Multiple parameters:
let p = params! { name: format!("%{}%", q), active: true };
let users = svc.get_where("UserName LIKE @name AND Active = @active", p, &token).await?;
```

### get_paged

```rust
// Page 1, 20 items per page. Pages are 1-indexed.
let page: Vec<User> = svc.get_paged(1, 20, &token).await?;
```

### count

```rust
let n: i64 = svc.count(&token).await?;
```

### exists

```rust
let found: bool = svc.exists(42, &token).await?;
```

### insert

```rust
// Returns the new PK value as i64.
// For IDENTITY columns the DB assigns the value; for non-identity keys it echoes entity.pk_value().
let entity = User { id: 0, name: "Alice".into(), email: "alice@example.com".into(), active: true };
let new_id: i64 = svc.insert(&entity, &token).await?;
```

### update

```rust
let updated = User { id: 42, name: "Alice Updated".into(), ..user };
svc.update(&updated, &token).await?;
```

### delete

```rust
svc.delete(42, &token).await?;
// Also accepts i64, String, Uuid depending on the PK type.
```

### upsert

```rust
// MERGE INTO — inserts if PK not found, updates if found.
svc.upsert(&entity, &token).await?;
```

### batch_insert

```rust
// Inserts a slice; returns a Vec<i64> of new IDs.
let ids: Vec<i64> = svc.batch_insert(&entities, &token).await?;
```

### batch_update

```rust
svc.batch_update(&entities, &token).await?;
```

### batch_delete

```rust
use statiq::PkValue;

let ids = vec![PkValue::I32(1), PkValue::I32(2), PkValue::I32(3)];
svc.batch_delete(&ids, &token).await?;
```

---

## 8. params! Macro

`params!` creates a stack-allocated array of `OdbcParam` values. It is zero-cost — no `Vec`, no heap allocation.

```rust
use statiq::params;

// Single param
let p = params! { name: "Alice" };

// Multiple params
let p = params! { min_price: 10.0f64, max_price: 100.0f64, active: true };

// Using an expression
let p = params! { name: format!("%{}%", search_term) };

// Optional (NULL-safe)
let maybe_email: Option<String> = Some("x@y.com".into());
let p = params! { email: maybe_email };  // sends NULL when None

// Passing to get_where
svc.get_where("Price BETWEEN @min_price AND @max_price AND Active = @active", p, &token).await?;
```

The macro strips the `@` prefix automatically when matching parameter names in the SQL string.

---

## 9. Transactions

Get a `Transaction` from the pool, do work, then call `.commit()`. If you drop the transaction without committing, it automatically rolls back.

```rust
use statiq::Pool;

// The pool is accessible via svc.pool() or store it separately.
// Here we show the pattern using a transaction obtained from the service's pool.
let conn = svc.pool().checkout(&token).await?;
let mut tx = statiq::transaction::Transaction::begin(conn)?;

tx.insert::<Product>(&product_a, &token).await?;
tx.insert::<Product>(&product_b, &token).await?;
tx.execute_raw("UPDATE dbo.Stock SET Reserved = Reserved + 1 WHERE Id = @id", &params!{ id: 99i32 }, &token).await?;

tx.commit().await?;  // if this line is never reached, Drop auto-rolls back
```

### Deadlock-Safe Retry

```rust
use statiq::transaction::with_retry;

let result = with_retry(svc.pool(), &token, 3, |tx| async move {
    tx.update::<Order>(&order, &token).await?;
    tx.execute_raw("UPDATE dbo.Inventory SET Qty = Qty - 1 WHERE ProductId = @id",
                   &params! { id: order.product_id }, &token).await?;
    Ok(())
}).await?;
```

`with_retry` detects SQL Server deadlock error 1205 and retries up to `max_retries` times with exponential back-off (50 ms, 100 ms, 200 ms, …).

---

## 10. Stored Procedures (SprocService)

`SprocService` executes stored procedures with compile-time generic return types. The dispatch is monomorphised at compile time — no virtual dispatch, no allocations beyond the final result.

### Parameters

```rust
use statiq::SprocParams;

let params = SprocParams::new()
    .add("@UserId", 42i32)
    .add("@Active", true)
    .add("@Name", "Alice".to_string())
    .add_nullable("@Notes", None::<String>);  // sends NULL
```

### Single Result Set

```rust
// Returns Vec<T: SqlEntity>
let users: Vec<User> = sproc
    .query::<Vec<User>>("dbo.sp_GetUsers", SprocParams::new().add("@Active", true), &token)
    .await?;

// Returns Option<T> — None if the proc returned no rows
use statiq::Single;
let Single(user) = sproc
    .query::<Single<User>>("dbo.sp_GetUserById", SprocParams::new().add("@Id", 42i32), &token)
    .await?;

// Returns T or error if no rows
use statiq::Required;
let Required(user) = sproc
    .query::<Required<User>>("dbo.sp_GetUserById", SprocParams::new().add("@Id", 42i32), &token)
    .await?;

// Returns a single scalar value
use statiq::Scalar;
let Scalar(count) = sproc
    .query::<Scalar<i64>>("dbo.sp_CountActiveUsers", SprocParams::new(), &token)
    .await?;
let total = count.unwrap_or(0);
```

### execute (no result set)

```rust
// For procs that only perform DML and return no rows.
sproc.execute("dbo.sp_ArchiveOrders", SprocParams::new().add("@OlderThanDays", 90i32), &token).await?;
```

---

## 11. Multiple Result Sets from a Sproc

When a stored procedure returns more than one result set, use `query2`, `query3`, `query4`, or `query_multiple`.

```rust
// Two result sets: total count + a page of rows
let (Scalar(total), dealers) = sproc
    .query2::<Scalar<i64>, Vec<Dealer>>(
        "dbo.sp_DealerList",
        SprocParams::new()
            .add("@PageNumber", 1i32)
            .add("@PageSize", 20i32),
        &token,
    )
    .await?;

let total_count = total.unwrap_or(0);
println!("Total: {total_count}, this page: {}", dealers.len());
```

```rust
// Three result sets
let (orders, items, totals) = sproc
    .query3::<Vec<Order>, Vec<OrderItem>, Scalar<rust_decimal::Decimal>>(
        "dbo.sp_OrderDetail",
        SprocParams::new().add("@OrderId", 123i32),
        &token,
    )
    .await?;
```

```rust
// Four result sets
let (a, b, c, d) = sproc
    .query4::<Vec<TypeA>, Vec<TypeB>, Scalar<i64>, Single<Summary>>(
        "dbo.sp_Dashboard",
        SprocParams::new(),
        &token,
    )
    .await?;
```

For more than four result sets, use `query_multiple` with a `MultiReader`:

```rust
use statiq::MultiReader;

let mut reader = sproc
    .query_multiple("dbo.sp_BigReport", SprocParams::new(), &token)
    .await?;

let totals: Vec<ReportTotal>   = reader.read_list()?;
let details: Vec<ReportDetail> = reader.read_list()?;
let Scalar(grand_total)        = reader.read_scalar::<rust_decimal::Decimal>()?;
```

`MultiReader` methods:

| Method | Return type |
|--------|-------------|
| `read_list::<T>()` | `Result<Vec<T>, SqlError>` |
| `read_single::<T>()` | `Result<Single<T>, SqlError>` |
| `read_required::<T>()` | `Result<Required<T>, SqlError>` |
| `read_scalar::<S>()` | `Result<Scalar<S>, SqlError>` |
| `read_raw()` | `Result<Vec<OdbcRow>, SqlError>` |

---

## 12. Redis Cache

Redis caching is transparent — the same `SqlRepository` trait is used. `SqlService<T, RedisCache>` automatically caches results under keys derived from the table name and primary key.

```rust
let svc: SqlService<User, RedisCache> = SqlServiceFactory::new()
    .config_path("config.json")
    .shutdown(token.clone())
    .build_with_cache::<User>()
    .await?;

// First call goes to SQL Server, populates Redis.
let user = svc.get_by_id(42, &token).await?;

// Second call is served from Redis (TTL from config.json: default_ttl_secs).
let user = svc.get_by_id(42, &token).await?;
```

When `"enabled": false` in `config.json`, `RedisCache` becomes a no-op pass-through — no Redis connection is established and all calls go directly to SQL Server.

Cache invalidation (insert/update/delete/upsert) automatically removes affected keys.

---

## 13. Raw Queries

For queries that don't map to a single entity, use the raw methods on `SqlRepository`.

### query_raw — returns untyped rows

```rust
use statiq::OdbcRow;

let rows: Vec<OdbcRow> = svc
    .query_raw(
        "SELECT p.Name, c.Name AS Category FROM dbo.Products p JOIN dbo.Categories c ON c.Id = p.CategoryId WHERE p.Active = @active",
        &params! { active: true },
        &token,
    )
    .await?;

for row in &rows {
    let name: String      = row.get_string("Name")?;
    let category: String  = row.get_string("Category")?;
    println!("{name} — {category}");
}
```

### execute_raw — DML without an entity

```rust
let affected: usize = svc
    .execute_raw(
        "UPDATE dbo.Products SET Active = 0 WHERE CategoryId = @cat_id",
        &params! { cat_id: 5i32 },
        &token,
    )
    .await?;
```

### scalar — single value query

```rust
let max_price: rust_decimal::Decimal = svc
    .scalar("SELECT MAX(Price) FROM dbo.Products", &[], &token)
    .await?;
```

### OdbcRow field accessors

```rust
row.get_string("ColumnName")?
row.get_i32("ColumnName")?
row.get_i64("ColumnName")?
row.get_bool("ColumnName")?
row.get_f64("ColumnName")?
row.get_decimal("ColumnName")?
row.get_datetime("ColumnName")?
row.get_naive_date("ColumnName")?
row.get_bytes("ColumnName")?
row.get_uuid("ColumnName")?

// Optional variants (return None when the cell is NULL):
row.get_string_opt("ColumnName")?   // Option<String>
row.get_i32_opt("ColumnName")?      // Option<i32>
// ... and so on for every type
```

---

## 14. Error Handling

`SqlError` is the single error type. All methods return `Result<_, SqlError>`.

```rust
use statiq::SqlError;

match svc.get_by_id(99, &token).await {
    Ok(Some(user)) => { /* use user */ }
    Ok(None)       => { /* not found */ }
    Err(SqlError::PoolExhausted { timeout_ms }) => eprintln!("DB pool timeout after {timeout_ms}ms"),
    Err(SqlError::Cancelled)                   => { /* request was cancelled */ }
    Err(e)                                     => eprintln!("DB error: {e}"),
}
```

### Client-safe messages

Never expose internal error details (ODBC error text, connection strings) to clients. Use `safe_message()`:

```rust
.map_err(|e| e.safe_message())   // returns a String safe to send to the frontend
```

### Error codes for structured responses

```rust
let code = err.error_code();   // "odbc_error", "pool_exhausted", "not_found", etc.
```

### axum feature — automatic HTTP status mapping

With the `axum` feature, `SqlError: IntoResponse`:

```rust
// In an axum handler — no match needed, errors become proper HTTP responses
async fn get_user(State(s): State<AppState>, Path(id): Path<i32>) -> axum::response::Response {
    match s.users.get_by_id(id, &s.token).await {
        Ok(Some(u)) => Json(u).into_response(),
        Ok(None)    => StatusCode::NOT_FOUND.into_response(),
        Err(e)      => e.into_response(),  // SqlError → JSON body + correct HTTP status
    }
}
```

HTTP status mapping:

| Error | HTTP Status |
|-------|-------------|
| `NotFound` | 404 |
| `Cancelled` | 499 |
| `PoolExhausted`, `DeadlockRetryExhausted` | 503 |
| `QueryTimeout` | 504 |
| everything else | 500 |

JSON body:

```json
{ "error_code": "pool_exhausted", "message": "Service busy, pool timeout after 5000ms" }
```

### tauri feature — IPC-safe error serialization

With the `tauri` feature, `SqlError: serde::Serialize`:

```rust
#[tauri::command]
async fn get_user(id: i32, state: tauri::State<'_, DbState>) -> Result<User, String> {
    state.users.get_by_id(id, &state.token)
        .await
        .map_err(|e| e.safe_message())
}
```

Or, to send structured JSON to the JS frontend:

```rust
#[tauri::command]
async fn get_user(id: i32, state: tauri::State<'_, DbState>) -> Result<User, SqlError> {
    state.users.get_by_id(id, &state.token).await
    // SqlError serializes as { "error_code": "...", "message": "..." }
}
```

---

## 15. Testing with MockRepository

Enable the `testing` feature in `[dev-dependencies]`:

```toml
[dev-dependencies]
statiq = { version = "0.2", features = ["testing"] }
```

`MockRepository<T>` is an in-memory implementation of `SqlRepository<T>`. It requires no database.

```rust
use statiq::testing::MockRepository;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_insert_and_get() {
    let token = CancellationToken::new();
    let repo = MockRepository::<User>::new();

    let user = User { id: 0, name: "Alice".into(), email: "alice@test.com".into(), active: true };
    let id = repo.insert(&user, &token).await.unwrap();
    assert_eq!(id, 1);

    let found = repo.get_by_id(1i32, &token).await.unwrap();
    assert!(found.is_some());
    assert_eq!(repo.insert_call_count(), 1);
}

#[tokio::test]
async fn test_with_pre_seeded_data() {
    let token = CancellationToken::new();
    let users = vec![
        User { id: 1, name: "Alice".into(), email: "a@test.com".into(), active: true },
        User { id: 2, name: "Bob".into(),   email: "b@test.com".into(), active: false },
    ];
    let repo = MockRepository::with_data(users);

    let all = repo.get_all(&token).await.unwrap();
    assert_eq!(all.len(), 2);

    repo.delete(2i32, &token).await.unwrap();
    assert_eq!(repo.len().await, 1);
    assert_eq!(repo.delete_call_count(), 1);
}
```

### MockRepository API summary

| Method | Description |
|--------|-------------|
| `MockRepository::new()` | Empty store |
| `MockRepository::with_data(iter)` | Pre-populated store |
| `repo.seed(item).await` | Add/replace an item (bypasses counters) |
| `repo.clear().await` | Remove all items |
| `repo.all_items().await` | `Vec<T>` snapshot |
| `repo.len().await` | Current item count |
| `repo.insert_call_count()` | How many times `insert` was called |
| `repo.update_call_count()` | How many times `update` was called |
| `repo.delete_call_count()` | How many times `delete` was called |
| `repo.upsert_call_count()` | How many times `upsert` was called |

Limitations:
- `get_where` and `query_raw` return all items; the SQL filter string is not evaluated.
- `get_paged` slices the full list; ordering is hash-map insertion order.
- `scalar` always returns an error — supply a custom mock type for scalar tests.

### Testing with trait objects

`SqlRepository<T>` is object-safe. You can inject either a real service or a mock through `Arc<dyn SqlRepository<T>>`:

```rust
// In production:
let repo: Arc<dyn SqlRepository<User>> = Arc::new(
    SqlServiceFactory::new().config_path("config.json").build::<User>().await?
);

// In tests:
let repo: Arc<dyn SqlRepository<User>> = Arc::new(MockRepository::<User>::new());
```

---

## 16. Axum Integration — Full Example

### SQL

```sql
CREATE TABLE dbo.Products (
    Id    INT IDENTITY(1,1) PRIMARY KEY,
    Name  NVARCHAR(200) NOT NULL,
    Price FLOAT         NOT NULL,
    Stock INT           NOT NULL DEFAULT 0,
    Active BIT          NOT NULL DEFAULT 1
);
```

### Cargo.toml

```toml
[package]
name    = "my-api"
version = "0.1.0"
edition = "2021"

[dependencies]
statiq     = { version = "0.2", features = ["axum"] }
axum       = "0.8"
tower-http = { version = "0.6", features = ["cors", "trace"] }
tokio      = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["full"] }
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
tracing    = "0.1"
tracing-subscriber = "0.3"
```

### main.rs

```rust
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use statiq::{NoCache, SqlRepository, SqlService, SqlServiceFactory, SprocParams, SprocService};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

// ── Entity ────────────────────────────────────────────────────────────────────

#[derive(statiq::SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Products", schema = "dbo")]
pub struct Product {
    #[sql_primary_key(identity)]
    pub id: i32,
    pub name: String,
    pub price: f64,
    pub stock: i32,
    pub active: bool,
}

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    products: Arc<SqlService<Product, NoCache>>,
    sproc:    Arc<SprocService>,
    token:    CancellationToken,
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateProduct { name: String, price: f64, stock: i32, active: bool }

#[derive(Deserialize)]
struct SearchQuery { name: String }

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // The library does NOT touch the global subscriber. Set up your own here.
    tracing_subscriber::fmt()
        .with_env_filter("my_api=debug,statiq=info,tower_http=debug")
        .init();

    let token = CancellationToken::new();

    // Graceful shutdown on Ctrl-C
    let shutdown_token = token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_token.cancel();
    });

    let products = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .with_logging(false)
        .build::<Product>()
        .await
        .expect("Failed to start DB service");

    let sproc = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .build_sproc()
        .await
        .expect("Failed to start SprocService");

    let state = AppState {
        products: Arc::new(products),
        sproc:    Arc::new(sproc),
        token,
    };

    let app = Router::new()
        .route("/products",          get(list_products).post(create_product))
        .route("/products/:id",      get(get_product).put(update_product).delete(delete_product))
        .route("/products/search",   get(search_products))
        .route("/sproc/top",         get(top_products))
        .route("/health",            get(health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("Listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}

// ── Handlers ──────────────────────────────────────────────────────────────────
// Every handler returns axum::response::Response.
// SqlError implements IntoResponse (axum feature) — no manual status mapping needed.

async fn list_products(State(s): State<AppState>) -> axum::response::Response {
    match s.products.get_all(&s.token).await {
        Ok(list) => Json(list).into_response(),
        Err(e)   => e.into_response(),
    }
}

async fn get_product(
    State(s): State<AppState>,
    Path(id): Path<i32>,
) -> axum::response::Response {
    match s.products.get_by_id(id, &s.token).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None)    => StatusCode::NOT_FOUND.into_response(),
        Err(e)      => e.into_response(),
    }
}

async fn create_product(
    State(s): State<AppState>,
    Json(req): Json<CreateProduct>,
) -> axum::response::Response {
    let entity = Product { id: 0, name: req.name, price: req.price, stock: req.stock, active: req.active };
    match s.products.insert(&entity, &s.token).await {
        Ok(new_id) => {
            let created = Product { id: new_id as i32, ..entity };
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn update_product(
    State(s): State<AppState>,
    Path(id): Path<i32>,
    Json(req): Json<CreateProduct>,
) -> axum::response::Response {
    let entity = Product { id, name: req.name, price: req.price, stock: req.stock, active: req.active };
    match s.products.update(&entity, &s.token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn delete_product(
    State(s): State<AppState>,
    Path(id): Path<i32>,
) -> axum::response::Response {
    match s.products.delete(id, &s.token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn search_products(
    State(s): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> axum::response::Response {
    let p = statiq::params! { name: format!("%{}%", q.name) };
    match s.products.get_where("Name LIKE @name", p, &s.token).await {
        Ok(list) => Json(list).into_response(),
        Err(e)   => e.into_response(),
    }
}

// Requires: CREATE PROCEDURE dbo.sp_GetTopProducts @TopN INT AS
//           SELECT TOP (@TopN) * FROM dbo.Products ORDER BY Price DESC
async fn top_products(State(s): State<AppState>) -> axum::response::Response {
    let params = SprocParams::new().add("@TopN", 5i32);
    match s.sproc.query::<Vec<Product>>("dbo.sp_GetTopProducts", params, &s.token).await {
        Ok(list) => Json(list).into_response(),
        Err(e)   => e.into_response(),
    }
}

async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let count = s.products.count(&s.token).await.unwrap_or(-1);
    Json(serde_json::json!({ "status": "ok", "product_count": count }))
}
```

### Key points for Axum

1. Store `SqlService` and `SprocService` in `Arc<_>` so they can be cloned into `AppState`.
2. `AppState` must implement `Clone` — axum's `State<S>` extractor clones it per request.
3. With `features = ["axum"]`, return type can be `axum::response::Response` from all handlers and call `.into_response()` on both the success and the error value.
4. Call `with_logging(false)` — axum apps set up their own tracing subscriber before calling `SqlServiceFactory::build`.
5. Pass the same `CancellationToken` to all services and to the signal handler.

---

## 17. Leptos SSR Integration — Full Example

Leptos SSR runs your components server-side (Rust) and hydrates them in the browser. DB access happens inside `#[server]` functions which are compiled for the server target only.

### SQL (same table as above)

```sql
CREATE TABLE dbo.Products (
    Id    INT IDENTITY(1,1) PRIMARY KEY,
    Name  NVARCHAR(200) NOT NULL,
    Price FLOAT         NOT NULL,
    Stock INT           NOT NULL DEFAULT 0,
    Active BIT          NOT NULL DEFAULT 1
);
```

### Cargo.toml

```toml
[package]
name    = "my-leptos-app"
version = "0.1.0"
edition = "2021"

[dependencies]
statiq       = { version = "0.2" }
leptos       = { version = "0.7", features = ["ssr"] }
leptos_axum  = { version = "0.7" }
leptos_meta  = { version = "0.7", features = ["ssr"] }
axum         = { version = "0.7", features = ["macros"] }  # leptos_axum 0.7 requires axum 0.7
tokio        = { version = "1", features = ["full"] }
tokio-util   = { version = "0.7", features = ["full"] }
serde        = { version = "1", features = ["derive"] }
tracing-subscriber = "0.3"
```

Note: `leptos_axum` 0.7 depends on `axum` 0.7. Do not mix with axum 0.8 in the same binary.

### src/main.rs

```rust
mod app;

use std::sync::OnceLock;

use axum::{routing::get, Router};
use leptos::config::get_configuration;
use leptos_axum::{generate_route_list, LeptosRoutes};
use serde::{Deserialize, Serialize};
use statiq::{NoCache, SqlService, SqlServiceFactory, SprocService};
use tokio_util::sync::CancellationToken;

use crate::app::{shell, App};

// ── Entity (defined at crate root so both main.rs and app.rs can use it) ──────

#[derive(statiq::SqlEntity, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[sql_table("Products", schema = "dbo")]
pub struct Product {
    #[sql_primary_key(identity)]
    pub id: i32,
    pub name: String,
    pub price: f64,
    pub stock: i32,
    pub active: bool,
}

// ── Global singletons accessed by server functions ────────────────────────────
// In a larger application, prefer leptos_axum::extract with axum State.
// OnceLock works well for simple apps.

pub(crate) mod db {
    use super::*;

    static PRODUCTS: OnceLock<SqlService<Product, NoCache>> = OnceLock::new();
    static SPROC:    OnceLock<SprocService>                 = OnceLock::new();
    static TOKEN:    OnceLock<CancellationToken>            = OnceLock::new();

    pub fn init(products: SqlService<Product, NoCache>, sproc: SprocService, token: CancellationToken) {
        PRODUCTS.set(products).ok();
        SPROC.set(sproc).ok();
        TOKEN.set(token).ok();
    }

    pub fn products() -> &'static SqlService<Product, NoCache> {
        PRODUCTS.get().expect("DB not initialised")
    }
    pub fn sproc() -> &'static SprocService {
        SPROC.get().expect("SprocService not initialised")
    }
    pub fn token() -> CancellationToken {
        TOKEN.get().expect("Token not initialised").clone()
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("my_leptos_app=debug,statiq=info,leptos=info")
        .init();

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;

    let token = CancellationToken::new();

    let products = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .with_logging(false)
        .build::<Product>()
        .await
        .expect("Failed to start DB service");

    let sproc = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .build_sproc()
        .await
        .expect("Failed to start SprocService");

    db::init(products, sproc, token);

    let routes = generate_route_list(App);

    let app = Router::<leptos::config::LeptosOptions>::new()
        .leptos_routes(&leptos_options, routes, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Listening on http://{addr}");
    axum::serve(listener, app.into_make_service()).await.unwrap();
}
```

### src/app.rs

```rust
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Title};
// SqlRepository must be in scope for .insert(), .get_where(), etc. to compile
use statiq::SqlRepository;

use crate::Product;

// ── Server Functions ───────────────────────────────────────────────────────────
// #[server] compiles the function body only for the server target.
// On the client, Leptos replaces it with an HTTP call automatically.

#[server]
pub async fn fetch_products() -> Result<Vec<Product>, ServerFnError> {
    use crate::db;
    db::products()
        .get_where("Active = 1", &[], &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn add_product(name: String, price: f64, stock: i32) -> Result<i64, ServerFnError> {
    use crate::db;
    let entity = Product { id: 0, name, price, stock, active: true };
    db::products()
        .insert(&entity, &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn remove_product(id: i32) -> Result<(), ServerFnError> {
    use crate::db;
    db::products()
        .delete(id, &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ── Shell and root component ──────────────────────────────────────────────────

pub fn shell(options: leptos::config::LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
          <head>
            <meta charset="utf-8"/>
            <meta name="viewport" content="width=device-width, initial-scale=1"/>
            <AutoReload options=options.clone()/>
            <HydrationScripts options/>
            <MetaTags/>
          </head>
          <body><App/></body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Title text="My App"/>
        <main>
            <ProductForm/>
            <ProductList/>
        </main>
    }
}

// ── Components ────────────────────────────────────────────────────────────────

#[component]
fn ProductForm() -> impl IntoView {
    let add_action = ServerAction::<AddProduct>::new();
    view! {
        <ActionForm action=add_action>
            <input type="text"   name="name"  placeholder="Product name" required/>
            <input type="number" name="price" placeholder="Price" step="0.01" required/>
            <input type="number" name="stock" placeholder="Stock" value="0"   required/>
            <button type="submit">"Add"</button>
        </ActionForm>
    }
}

#[component]
fn ProductList() -> impl IntoView {
    let products     = Resource::new(|| (), |_| fetch_products());
    let delete_action = ServerAction::<RemoveProduct>::new();

    view! {
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || products.get().map(|r| match r {
                Err(e)   => view! { <p>"Error: "{e.to_string()}</p> }.into_any(),
                Ok(list) => view! {
                    <table>
                        <For
                            each=move || list.clone()
                            key=|p| p.id
                            children=move |p| {
                                let pid = p.id;
                                view! {
                                    <tr>
                                        <td>{p.id}</td>
                                        <td>{p.name.clone()}</td>
                                        <td>{format!("{:.2}", p.price)}</td>
                                        <td>
                                            <ActionForm action=delete_action>
                                                <input type="hidden" name="id" value=pid/>
                                                <button type="submit">"Delete"</button>
                                            </ActionForm>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </table>
                }.into_any(),
            })}
        </Suspense>
    }
}
```

### Key points for Leptos SSR

1. The entity struct must implement `PartialEq` (Leptos `Resource` needs it to detect changes) and `Serialize + Deserialize` (sent over the wire as JSON).
2. Inside a `#[server]` function, bring `use statiq::SqlRepository` into scope — without it, `.insert()`, `.get_where()`, and the other methods are not in scope.
3. `db::products()` returns `&'static SqlService<...>` via `OnceLock`. This is appropriate for simple apps. For production apps with per-request context, use `leptos_axum::extract::<State<AppState>>()` inside server functions.
4. `leptos_axum` 0.7 uses `axum` 0.7 — do not upgrade to axum 0.8 in the same binary.
5. Leptos server functions are HTTP endpoints under the hood. `ServerFnError` is the only allowed error type. Wrap `SqlError` with `.map_err(|e| ServerFnError::new(e.to_string()))`.

---

## 18. Type Mappings (SQL Server ↔ Rust)

| SQL Server type | Rust field type | `params!` sends | `OdbcRow` getter |
|----------------|-----------------|-----------------|------------------|
| `bit` | `bool` / `Option<bool>` | `Bool` | `get_bool` / `get_bool_opt` |
| `tinyint` | `u8` / `Option<u8>` | `U8` | `get_u8` / `get_u8_opt` |
| `smallint` | `i16` / `Option<i16>` | `I16` | `get_i16` / `get_i16_opt` |
| `int` | `i32` / `Option<i32>` | `I32` | `get_i32` / `get_i32_opt` |
| `bigint` | `i64` / `Option<i64>` | `I64` | `get_i64` / `get_i64_opt` |
| `real` | `f32` / `Option<f32>` | `F32` | `get_f32` / `get_f32_opt` |
| `float` | `f64` / `Option<f64>` | `F64` | `get_f64` / `get_f64_opt` |
| `decimal`, `numeric`, `money`, `smallmoney` | `rust_decimal::Decimal` | `Decimal` | `get_decimal` |
| `char`, `varchar`, `nchar`, `nvarchar` | `String` / `Option<String>` | `Str` | `get_string` / `get_string_opt` |
| `text`, `ntext`, `xml`, `sql_variant` | `String` | `Str` | `get_string` |
| `binary`, `varbinary`, `image`, `rowversion` | `Vec<u8>` | `Bytes` | `get_bytes` |
| `date` | `chrono::NaiveDate` | `NaiveDate` | `get_naive_date` |
| `time` | `chrono::NaiveTime` | `NaiveTime` | `get_naive_time` |
| `datetime`, `datetime2`, `smalldatetime` | `chrono::DateTime<chrono::Utc>` | `DateTime(Utc)` | `get_datetime` |
| `datetimeoffset` | `chrono::DateTime<chrono::FixedOffset>` | `DateTimeOffset` | `get_datetime_offset` |
| `uniqueidentifier` | `uuid::Uuid` | `Guid` | `get_uuid` |
| `NULL` (any) | `Option<T>` | `Null` | `get_*_opt` |

---

## 19. Attribute Reference

### Struct-level

`#[sql_table("TableName", schema = "dbo")]`

Required. Sets the SQL table name and schema. If `schema` is omitted, no schema prefix is emitted.

### Field-level

| Attribute | Effect |
|-----------|--------|
| `#[sql_primary_key]` | Marks this field as the PK. Required on exactly one field. Included in SELECT, UPDATE WHERE, DELETE WHERE, and MERGE match. |
| `#[sql_primary_key(identity)]` | Same as above, but also excludes the field from INSERT (SQL Server assigns the value). |
| `#[sql_column("ColName")]` | Overrides the SQL column name used in all generated SQL. The struct field name is used as the `@param` name in `params!`. |
| `#[sql_ignore]` | Completely excludes the field from all generated SQL. Not in SELECT, not in INSERT, not in params. |
| `#[sql_computed]` | DB-computed column. Included in SELECT only. Excluded from INSERT, UPDATE, MERGE, and `to_params()`. |
| `#[sql_default]` | Server-side DEFAULT. Included in SELECT and UPDATE. Excluded from INSERT and MERGE-insert clause. |

---

## 20. config.json Field Reference

### mssql.connection_string

ODBC connection string. Common format:

```
Driver={ODBC Driver 17 for SQL Server};Server=HOST\INSTANCE;Database=DBNAME;UID=USER;PWD=PASS;
Driver={ODBC Driver 17 for SQL Server};Server=HOST,1433;Database=DBNAME;Trusted_Connection=yes;
```

### mssql.pool

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_size` | u32 | 100 | Maximum total connections in the pool. |
| `min_size` | u32 | 5 | Minimum connections kept open (warm pool). |
| `idle_timeout_secs` | u64 | 300 | Close a connection that has been idle this long. |
| `max_lifetime_secs` | u64 | 1800 | Close and replace a connection older than this (0 = no limit). |
| `checkout_timeout_ms` | u64 | 5000 | How long a caller waits for an available connection before `PoolExhausted`. |
| `validation_interval_secs` | u64 | 60 | How often the background validator runs to cull idle/stale connections. |
| `max_deadlock_retries` | u8 | 3 | Used by `with_retry` when not overridden explicitly. |
| `reset_connection_on_reuse` | bool | false | Run `EXEC sp_reset_connection` before reusing a connection. Mirrors ADO.NET behaviour. |

### mssql.query

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_command_timeout_secs` | u64 | 30 | ODBC statement timeout. |
| `slow_query_threshold_ms` | u64 | 1000 | Queries exceeding this are logged at WARN level. |
| `max_text_bytes` | usize | 65536 | Maximum byte size per text/binary cell. Increase for `nvarchar(max)` / `xml` / `varbinary(max)` columns. |

### redis

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | String | `"redis://127.0.0.1:6379"` | Redis connection URL. |
| `pool_size` | u32 | 20 | Redis connection pool size. |
| `default_ttl_secs` | u64 | 300 | Default TTL for cached records. |
| `count_ttl_secs` | u64 | 60 | TTL for cached `count()` results. |
| `enabled` | bool | false | When false, `RedisCache` is a no-op pass-through. |

### logging

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `level` | String | `"INFO"` | Log level: `"ERROR"`, `"WARN"`, `"INFO"`, `"DEBUG"`, `"TRACE"`. |
| `format` | String | `"json"` | `"json"` for structured JSON, `"text"` for human-readable. |

Only used when `SqlServiceFactory::with_logging(true)` is set. In Axum/Leptos/Tauri applications, call `with_logging(false)` and set up your own subscriber.
