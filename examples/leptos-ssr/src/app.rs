//! Leptos SSR bileşenleri ve server function'ları
//!
//! Server function'lar sunucu tarafında çalışır → doğrudan MSSQL'e erişir.
//! İstemciye sadece JSON serialize edilmiş sonuçlar gider.

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
// SqlRepository trait'i scope'a alınmazsa .get_where(), .insert() vs. derlenmez
use statiq::SqlRepository;

use crate::Product; // crate kökünde (main.rs) tanımlı

// ── Server Functions ───────────────────────────────────────────────────────────
// #[server] → sunucu tarafında gerçek DB kodu çalışır,
//              istemci tarafında Leptos otomatik HTTP çağrısı üretir.

/// Tüm aktif ürünleri getir
#[server]
pub async fn fetch_products() -> Result<Vec<Product>, ServerFnError> {
    use crate::db;
    db::products()
        .get_where("Active = 1", &[], &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Yeni ürün ekle — yeni ID'yi döndürür
#[server]
pub async fn add_product(
    name: String,
    price: f64,
    stock: i32,
) -> Result<i64, ServerFnError> {
    use crate::db;
    let entity = Product { id: 0, name, price, stock, active: true };
    db::products()
        .insert(&entity, &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Ürün sil
#[server]
pub async fn remove_product(id: i32) -> Result<(), ServerFnError> {
    use crate::db;
    db::products()
        .delete(id, &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Toplam ürün sayısı
#[server]
pub async fn product_count() -> Result<i64, ServerFnError> {
    use crate::db;
    db::products()
        .count(&db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Stored procedure — en pahalı N ürün
#[server]
pub async fn top_products(top_n: i32) -> Result<Vec<Product>, ServerFnError> {
    use crate::db;
    use statiq::SprocParams;
    let params = SprocParams::new().add("@TopN", top_n);
    db::sproc()
        .query("dbo.sp_GetTopProducts", params, &db::token())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ── Shell ve App bileşenleri ──────────────────────────────────────────────────

/// Uygulama shell'i — SSR için tam HTML belgesi
pub fn shell(options: leptos::config::LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="tr">
          <head>
            <meta charset="utf-8"/>
            <meta name="viewport" content="width=device-width, initial-scale=1"/>
            <AutoReload options=options.clone()/>
            <HydrationScripts options/>
            <MetaTags/>
          </head>
          <body>
            <App/>
          </body>
        </html>
    }
}

/// Kök bileşen
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="main" href="/style.css"/>
        <Title text="Statiq Leptos SSR Demo"/>
        <main>
            <h1>"Statiq — Leptos SSR + MSSQL"</h1>
            <ProductStats/>
            <hr/>
            <ProductForm/>
            <hr/>
            <ProductList/>
            <hr/>
            <TopProducts/>
        </main>
    }
}

// ── İç bileşenler ─────────────────────────────────────────────────────────────

/// Özet: toplam ürün sayısı
#[component]
fn ProductStats() -> impl IntoView {
    let count = Resource::new(|| (), |_| product_count());

    view! {
        <section>
            <h2>"Özet"</h2>
            <Suspense fallback=|| view! { <p>"Yükleniyor..."</p> }>
                {move || count.get().map(|r| match r {
                    Ok(n)  => view! { <p><strong>{n}</strong>" ürün kayıtlı"</p> }.into_any(),
                    Err(e) => view! { <p class="error">"Hata: "{e.to_string()}</p> }.into_any(),
                })}
            </Suspense>
        </section>
    }
}

/// Yeni ürün ekleme formu — ActionForm → add_product server fn
#[component]
fn ProductForm() -> impl IntoView {
    let add_action = ServerAction::<AddProduct>::new();
    let result = add_action.value();

    view! {
        <section>
            <h2>"Yeni Ürün Ekle"</h2>
            <ActionForm action=add_action>
                <input type="text"   name="name"  placeholder="Ürün adı" required/>
                <input type="number" name="price" placeholder="Fiyat (₺)" step="0.01" required/>
                <input type="number" name="stock" placeholder="Stok" value="0" required/>
                <button type="submit">"Ekle"</button>
            </ActionForm>
            {move || result.get().map(|r| match r {
                Ok(id)  => view! { <p class="success">"✓ Eklendi — ID: "{id}</p> }.into_any(),
                Err(e)  => view! { <p class="error">"✗ Hata: "{e.to_string()}</p> }.into_any(),
            })}
        </section>
    }
}

/// Aktif ürün listesi — SSR'da tam HTML olarak render edilir
#[component]
fn ProductList() -> impl IntoView {
    let products = Resource::new(|| (), |_| fetch_products());
    let delete_action = ServerAction::<RemoveProduct>::new();

    view! {
        <section>
            <h2>"Aktif Ürünler"</h2>
            <Suspense fallback=|| view! { <p>"Yükleniyor..."</p> }>
                {move || products.get().map(|r| match r {
                    Err(e) => view! {
                        <p class="error">"Hata: "{e.to_string()}</p>
                    }.into_any(),
                    Ok(list) if list.is_empty() => view! {
                        <p>"Henüz ürün yok."</p>
                    }.into_any(),
                    Ok(list) => view! {
                        <table>
                            <thead>
                                <tr>
                                    <th>"ID"</th><th>"Ad"</th>
                                    <th>"Fiyat"</th><th>"Stok"</th><th>"İşlem"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || list.clone()
                                    key=|p| p.id
                                    children=move |p| {
                                        let pid = p.id;
                                        view! {
                                            <tr>
                                                <td>{p.id}</td>
                                                <td>{p.name.clone()}</td>
                                                <td>{format!("{:.2} ₺", p.price)}</td>
                                                <td>{p.stock}</td>
                                                <td>
                                                    <ActionForm action=delete_action>
                                                        <input type="hidden" name="id" value=pid/>
                                                        <button type="submit" class="danger">"Sil"</button>
                                                    </ActionForm>
                                                </td>
                                            </tr>
                                        }
                                    }
                                />
                            </tbody>
                        </table>
                    }.into_any(),
                })}
            </Suspense>
        </section>
    }
}

/// Stored procedure sonuçları — en pahalı 5 ürün
#[component]
fn TopProducts() -> impl IntoView {
    let top = Resource::new(|| (), |_| top_products(5));

    view! {
        <section>
            <h2>"En Pahalı 5 Ürün (Stored Procedure)"</h2>
            <Suspense fallback=|| view! { <p>"Çalışıyor..."</p> }>
                {move || top.get().map(|r| match r {
                    Err(e) => view! {
                        <p class="error">"Hata: "{e.to_string()}</p>
                    }.into_any(),
                    Ok(list) => view! {
                        <table>
                            <thead>
                                <tr><th>"ID"</th><th>"Ad"</th><th>"Fiyat"</th><th>"Stok"</th></tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || list.clone()
                                    key=|p| p.id
                                    children=|p| view! {
                                        <tr>
                                            <td>{p.id}</td>
                                            <td>{p.name.clone()}</td>
                                            <td>{format!("{:.2} ₺", p.price)}</td>
                                            <td>{p.stock}</td>
                                        </tr>
                                    }
                                />
                            </tbody>
                        </table>
                    }.into_any(),
                })}
            </Suspense>
        </section>
    }
}
