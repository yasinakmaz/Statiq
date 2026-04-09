//! Tauri masaüstü uygulaması — statiq ile MSSQL'e doğrudan erişim
//!
//! Çalıştırmak için (tauri-cli gerekir):
//!   cargo tauri dev   (geliştirme)
//!   cargo tauri build (dağıtım paketi)
//!
//! veya ham olarak:
//!   cargo run -p tauri-app
//!
//! `tauri` feature sayesinde SqlError, serde::Serialize implement eder;
//! Tauri komutları Result<T, SqlError> doğrudan döndürebilir.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use statiq::{
    NoCache, SqlEntity, SqlRepository, SqlService, SqlServiceFactory, SprocParams, SprocService,
};
use tokio_util::sync::CancellationToken;

// ── Entity ────────────────────────────────────────────────────────────────────

/// dbo.Products tablosuna karşılık gelen entity.
/// `tauri` feature → SqlError seri hale getirilebilir olur (IPC için).
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

// ── Shared state (Tauri manage ile saklanır) ──────────────────────────────────

struct DbState {
    products: SqlService<Product, NoCache>,
    sproc: SprocService,
    token: CancellationToken,
}

// ── Tauri komutları ───────────────────────────────────────────────────────────
// Her komut `tauri::State` aracılığıyla DbState'e erişir.
// `tauri` feature aktifken SqlError → serde::Serialize → IPC üzerinden JS'e gider.

/// Tüm ürünleri getir
#[tauri::command]
async fn get_all_products(
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<Vec<Product>, String> {
    state
        .products
        .get_all(&state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// ID'ye göre tekil ürün
#[tauri::command]
async fn get_product_by_id(
    id: i32,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<Option<Product>, String> {
    state
        .products
        .get_by_id(id, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Yeni ürün oluştur — yeni ID'yi döndürür
#[tauri::command]
async fn create_product(
    name: String,
    price: f64,
    stock: i32,
    active: bool,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<i64, String> {
    let entity = Product { id: 0, name, price, stock, active };
    state
        .products
        .insert(&entity, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Ürün güncelle
#[tauri::command]
async fn update_product(
    id: i32,
    name: String,
    price: f64,
    stock: i32,
    active: bool,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<(), String> {
    let entity = Product { id, name, price, stock, active };
    state
        .products
        .update(&entity, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Ürün sil
#[tauri::command]
async fn delete_product(
    id: i32,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<(), String> {
    state
        .products
        .delete(id, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// İsme göre ürün ara (LIKE)
#[tauri::command]
async fn search_products(
    name: String,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<Vec<Product>, String> {
    let params = statiq::params! { name: format!("%{}%", name) };
    state
        .products
        .get_where("Name LIKE @name", &params, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Upsert — kayıt yoksa ekle, varsa güncelle (MERGE)
#[tauri::command]
async fn upsert_product(
    id: i32,
    name: String,
    price: f64,
    stock: i32,
    active: bool,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<(), String> {
    let entity = Product { id, name, price, stock, active };
    state
        .products
        .upsert(&entity, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Sayfalı ürün listesi (sayfa numarası 1'den başlar)
#[tauri::command]
async fn get_products_paged(
    page: i64,
    page_size: i64,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<Vec<Product>, String> {
    state
        .products
        .get_paged(page, page_size, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Toplam ürün sayısı
#[tauri::command]
async fn product_count(
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<i64, String> {
    state
        .products
        .count(&state.token)
        .await
        .map_err(|e| e.safe_message())
}

/// Stored procedure — en pahalı N ürün
/// DB'de şu proc'un mevcut olduğu varsayılır:
///   CREATE PROCEDURE dbo.sp_GetTopProducts @TopN INT AS
///   BEGIN SELECT TOP (@TopN) * FROM dbo.Products ORDER BY Price DESC END
#[tauri::command]
async fn get_top_products(
    top_n: i32,
    state: tauri::State<'_, Arc<DbState>>,
) -> Result<Vec<Product>, String> {
    let params = SprocParams::new().add("@TopN", top_n);
    state
        .sproc
        .query("dbo.sp_GetTopProducts", params, &state.token)
        .await
        .map_err(|e| e.safe_message())
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("tauri_app=debug,statiq=info")
        .init();

    // Tauri kendi async runtime'ını yönetir.
    // DB servisleri tokio runtime içinde (tauri::async_runtime) başlatılır.
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();

            // Tauri'nin runtime'ında async setup
            tauri::async_runtime::block_on(async move {
                let token = CancellationToken::new();

                let products = SqlServiceFactory::new()
                    .config_path("config.json")
                    .shutdown(token.clone())
                    .with_logging(false)
                    .build::<Product>()
                    .await
                    .expect("Product servisi başlatılamadı");

                let sproc = SqlServiceFactory::new()
                    .config_path("config.json")
                    .shutdown(token.clone())
                    .build_sproc()
                    .await
                    .expect("SprocService başlatılamadı");

                let db_state = Arc::new(DbState { products, sproc, token });
                handle.manage(db_state);
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_all_products,
            get_product_by_id,
            create_product,
            update_product,
            delete_product,
            search_products,
            upsert_product,
            get_products_paged,
            product_count,
            get_top_products,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri uygulaması başlatılamadı");
}
