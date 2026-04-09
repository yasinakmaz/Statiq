//! Leptos SSR uygulaması — axum backend, doğrudan MSSQL erişimi
//!
//! Çalıştırmak için:
//!   cargo run -p leptos-ssr
//!
//! Leptos server function'ları sunucu tarafında MSSQL'e doğrudan erişir.
//! İstemciye sadece serialize edilmiş veri gider — SQL asla açığa çıkmaz.

mod app;

use std::sync::OnceLock;

use axum::{routing::get, Router};
use leptos::config::get_configuration;
use leptos_axum::{generate_route_list, LeptosRoutes};
use serde::{Deserialize, Serialize};
use statiq::{NoCache, SqlService, SqlServiceFactory, SprocService};
use tokio_util::sync::CancellationToken;

use crate::app::{shell, App};

// ── Entity (crate kökünde tanımlı — hem main.rs hem app.rs kullanır) ──────────

/// dbo.Products tablosuna karşılık gelen entity.
/// Server function'lar serialize eder → istemciye JSON gider.
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

// ── Global DB erişimi ─────────────────────────────────────────────────────────
// Server function'ların DB'ye erişmesi için global OnceLock kullanılır.
// Üretimde leptos_axum::extract ile axum State üzerinden da sağlanabilir.

pub(crate) mod db {
    use super::*;

    static PRODUCTS: OnceLock<SqlService<Product, NoCache>> = OnceLock::new();
    static SPROC: OnceLock<SprocService> = OnceLock::new();
    static TOKEN: OnceLock<CancellationToken> = OnceLock::new();

    pub fn init(
        products: SqlService<Product, NoCache>,
        sproc: SprocService,
        token: CancellationToken,
    ) {
        PRODUCTS.set(products).ok();
        SPROC.set(sproc).ok();
        TOKEN.set(token).ok();
    }

    pub fn products() -> &'static SqlService<Product, NoCache> {
        PRODUCTS.get().expect("DB servisi başlatılmamış")
    }

    pub fn sproc() -> &'static SprocService {
        SPROC.get().expect("SprocService başlatılmamış")
    }

    pub fn token() -> CancellationToken {
        TOKEN.get().expect("Token başlatılmamış").clone()
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("leptos_ssr=debug,statiq=info,leptos=info")
        .init();

    // ── Leptos konfigürasyonu ────────────────────────────────────────────────
    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;

    // ── DB servisleri ────────────────────────────────────────────────────────
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

    // Global OnceLock'a yaz — server function'lar buradan okur
    db::init(products, sproc, token);

    // ── Leptos route listesi ─────────────────────────────────────────────────
    let routes = generate_route_list(App);

    // ── Axum router ─────────────────────────────────────────────────────────
    // LeptosRoutes<S> trait'i `LeptosOptions: FromRef<S>` bound'u gerektirir.
    // Router'ı açıkça LeptosOptions state ile başlatıyoruz.
    let app = Router::<leptos::config::LeptosOptions>::new()
        .route("/style.css", get(serve_css))
        .leptos_routes(&leptos_options, routes, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Dinleniyor: http://{addr}");
    // axum 0.7 API: axum::Server yerine axum::serve (0.7.9+)
    axum::serve(listener, app.into_make_service()).await.unwrap();
}

/// Temel CSS
async fn serve_css() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [("content-type", "text/css")],
        r#"
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body { font-family: system-ui, sans-serif; background: #0f172a; color: #e2e8f0; padding: 2rem; max-width: 900px; margin: 0 auto; }
        h1 { color: #38bdf8; margin-bottom: 1.5rem; }
        h2 { color: #94a3b8; font-size: 0.875rem; text-transform: uppercase; letter-spacing: 0.05em; margin: 1.5rem 0 0.75rem; }
        section { background: #1e293b; border-radius: 0.75rem; padding: 1.5rem; margin-bottom: 1.5rem; }
        hr { border: none; border-top: 1px solid #334155; margin: 0; }
        input { padding: 0.5rem 1rem; border-radius: 0.5rem; border: 1px solid #334155; background: #0f172a; color: #e2e8f0; font-size: 0.875rem; margin-right: 0.5rem; }
        button { padding: 0.5rem 1rem; border-radius: 0.5rem; border: none; background: #0284c7; color: white; cursor: pointer; font-size: 0.875rem; }
        button.danger { background: #dc2626; }
        table { width: 100%; border-collapse: collapse; font-size: 0.875rem; margin-top: 0.75rem; }
        th { text-align: left; color: #64748b; font-weight: 600; padding: 0.5rem 0.75rem; border-bottom: 1px solid #334155; }
        td { padding: 0.5rem 0.75rem; border-bottom: 1px solid #0f172a; }
        .success { color: #4ade80; margin-top: 0.5rem; }
        .error { color: #f87171; margin-top: 0.5rem; }
        form { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
        "#,
    )
        .into_response()
}
