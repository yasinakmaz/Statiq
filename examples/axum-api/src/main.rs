//! Axum REST API örneği — statiq kütüphanesi ile MSSQL CRUD + sproc + transaction
//!
//! Çalıştırmak için:
//!   cargo run -p axum-api
//!
//! Endpoint'ler:
//!   GET    /products           → tüm ürünler
//!   GET    /products/:id       → tekil ürün
//!   POST   /products           → yeni ürün ekle
//!   PUT    /products/:id       → ürün güncelle
//!   DELETE /products/:id       → ürün sil
//!   GET    /products/search    → ?name=.. ile arama (get_where)
//!   POST   /products/bulk      → toplu ekleme (transaction)
//!   GET    /sproc/top-products → sproc örneği
//!   GET    /health             → pool metrik bilgisi

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
use tracing::info;

// ── Entity ────────────────────────────────────────────────────────────────────

/// dbo.Products tablosuna karşılık gelen entity.
/// #[derive(SqlEntity)] tüm SQL sorgularını compile-time'da üretir.
#[derive(statiq::SqlEntity, Serialize, Deserialize, Clone, Debug)]
#[sql_table("Products", schema = "dbo")]
pub struct Product {
    #[sql_primary_key(identity)]  // IDENTITY kolonu — INSERT'e dahil edilmez
    pub id: i32,
    pub name: String,
    pub price: f64,
    pub stock: i32,
    pub active: bool,
}

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    /// Compile-time SQL, no-cache varyantı
    products: Arc<SqlService<Product, NoCache>>,
    /// Stored procedure servisi (entity type'a bağlı değil)
    sproc: Arc<SprocService>,
    /// Graceful shutdown sinyali
    token: CancellationToken,
}

// ── Request/response DTO'ları ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateProductRequest {
    name: String,
    price: f64,
    stock: i32,
    active: bool,
}

#[derive(Deserialize)]
struct SearchQuery {
    name: String,
}

#[derive(Deserialize)]
struct BulkRequest {
    products: Vec<CreateProductRequest>,
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Kütüphane varsayılan olarak global subscriber başlatmaz.
    // Uygulama kendi subscriber'ını kurar.
    tracing_subscriber::fmt()
        .with_env_filter("axum_api=debug,statiq=info,tower_http=debug")
        .init();

    let token = CancellationToken::new();

    // ── Servisleri oluştur ───────────────────────────────────────────────────
    // config.json workspace kökünde beklenmektedir.
    // with_logging(false) → global subscriber'a dokunmaz.
    let products = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .with_logging(false)
        .build::<Product>()
        .await
        .expect("Product servisi başlatılamadı — config.json ve MSSQL bağlantısını kontrol edin");

    let sproc = SqlServiceFactory::new()
        .config_path("config.json")
        .shutdown(token.clone())
        .build_sproc()
        .await
        .expect("SprocService başlatılamadı");

    let state = AppState {
        products: Arc::new(products),
        sproc: Arc::new(sproc),
        token,
    };

    // ── Router ───────────────────────────────────────────────────────────────
    let app = Router::new()
        .route("/products", get(list_products).post(create_product))
        .route(
            "/products/:id",
            get(get_product).put(update_product).delete(delete_product),
        )
        .route("/products/search", get(search_products))
        .route("/products/bulk", post(bulk_create))
        .route("/sproc/top-products", get(top_products_sproc))
        .route("/health", get(health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("Dinleniyor: http://{addr}");
    axum::serve(listener, app).await.unwrap();
}

// ── Handler'lar ───────────────────────────────────────────────────────────────
// Tüm handler'lar `axum::response::Response` döndürür → type inference sorunu yok.
// SqlError, `axum` feature ile `IntoResponse` implement eder → JSON hata yanıtı.

/// GET /products — tüm ürünler
async fn list_products(State(s): State<AppState>) -> axum::response::Response {
    match s.products.get_all(&s.token).await {
        Ok(list) => Json(list).into_response(),
        Err(e)   => e.into_response(),
    }
}

/// GET /products/:id — tekil ürün
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

/// POST /products — yeni ürün ekle
async fn create_product(
    State(s): State<AppState>,
    Json(req): Json<CreateProductRequest>,
) -> axum::response::Response {
    let entity = Product {
        id: 0, // IDENTITY — DB tarafından atanır
        name: req.name,
        price: req.price,
        stock: req.stock,
        active: req.active,
    };
    // insert() yeni kaydın PK değerini döndürür (OUTPUT INSERTED.Id)
    match s.products.insert(&entity, &s.token).await {
        Ok(new_id) => {
            info!(new_id, "Ürün oluşturuldu");
            let created = Product { id: new_id as i32, ..entity };
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// PUT /products/:id — güncelle
async fn update_product(
    State(s): State<AppState>,
    Path(id): Path<i32>,
    Json(req): Json<CreateProductRequest>,
) -> axum::response::Response {
    let entity = Product {
        id,
        name: req.name,
        price: req.price,
        stock: req.stock,
        active: req.active,
    };
    match s.products.update(&entity, &s.token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /products/:id — sil
async fn delete_product(
    State(s): State<AppState>,
    Path(id): Path<i32>,
) -> axum::response::Response {
    match s.products.delete(id, &s.token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /products/search?name=.. — WHERE ile filtreleme
/// `params!` makrosu stack'te `&[OdbcParam]` dilimi oluşturur (heap allocation yok)
async fn search_products(
    State(s): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> axum::response::Response {
    let p = statiq::params! { name: format!("%{}%", q.name) };
    // params! zaten &[OdbcParam] döndürür — ekstra & işareti eklemeyin
    match s.products.get_where("Name LIKE @name", p, &s.token).await {
        Ok(list) => Json(list).into_response(),
        Err(e)   => e.into_response(),
    }
}

/// POST /products/bulk — toplu ekleme
async fn bulk_create(
    State(s): State<AppState>,
    Json(req): Json<BulkRequest>,
) -> axum::response::Response {
    let entities: Vec<Product> = req
        .products
        .into_iter()
        .map(|r| Product { id: 0, name: r.name, price: r.price, stock: r.stock, active: r.active })
        .collect();
    // batch_insert — tüm kayıtları ekler, yeni ID listesini döndürür
    match s.products.batch_insert(&entities, &s.token).await {
        Ok(ids) => (StatusCode::CREATED, Json(ids)).into_response(),
        Err(e)  => e.into_response(),
    }
}

/// GET /sproc/top-products — stored procedure örneği
/// Veritabanında şu stored proc'un olduğunu varsayar:
///   CREATE PROCEDURE dbo.sp_GetTopProducts @TopN INT AS
///   BEGIN SELECT TOP (@TopN) * FROM dbo.Products ORDER BY Price DESC END
async fn top_products_sproc(State(s): State<AppState>) -> axum::response::Response {
    let params = SprocParams::new().add("@TopN", 5i32);
    match s.sproc.query::<Vec<Product>>("dbo.sp_GetTopProducts", params, &s.token).await {
        Ok(products) => Json(products).into_response(),
        Err(e)       => e.into_response(),
    }
}

/// GET /health — pool metrikleri
async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let count = s.products.count(&s.token).await.unwrap_or(-1);
    Json(serde_json::json!({
        "status": "ok",
        "product_count": count,
    }))
}
