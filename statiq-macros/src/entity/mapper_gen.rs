use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Type;
use super::parse::StructInfo;

/// Generates the `from_row` method body.
pub fn generate_from_row(info: &StructInfo) -> TokenStream {
    let mut field_assignments = Vec::new();

    for field in &info.fields {
        let rust_ident = format_ident!("{}", field.rust_name);
        let sql_name = &field.sql_name;

        if field.is_ignored {
            field_assignments.push(quote! {
                #rust_ident: Default::default()
            });
            continue;
        }

        // Masked String fields: read from DB (to advance cursor) but replace value.
        if field.is_masked {
            let ty_str = quote!(&field.ty).to_string().replace(' ', "");
            let is_optional = ty_str.starts_with("Option<");
            if is_optional {
                field_assignments.push(quote! {
                    #rust_ident: { let _ = row.get_string_opt(#sql_name)?; Some("***".to_string()) }
                });
            } else {
                field_assignments.push(quote! {
                    #rust_ident: { let _ = row.get_string(#sql_name)?; "***".to_string() }
                });
            }
            continue;
        }

        let getter = get_row_getter(&field.ty, sql_name);
        field_assignments.push(quote! {
            #rust_ident: #getter
        });
    }

    quote! {
        fn from_row(row: &statiq::row::OdbcRow) -> Result<Self, statiq::error::SqlError> {
            Ok(Self {
                #(#field_assignments),*
            })
        }
    }
}

/// Picks the correct `OdbcRow::get_*` call for a given Rust type.
fn get_row_getter(ty: &Type, col: &str) -> TokenStream {
    // Normalise spaces so "Option < i32 >" → "Option<i32>"
    let ty_str = quote!(#ty).to_string().replace(' ', "");

    if ty_str.starts_with("Option<") {
        let inner = ty_str
            .trim_start_matches("Option<")
            .trim_end_matches('>');
        return optional_getter(inner, col);
    }

    required_getter(&ty_str, col)
}

/// Required (non-nullable) getter — maps Rust type name → `get_*` call.
///
/// | Rust type                          | SQL Server types                           |
/// |------------------------------------|--------------------------------------------|
/// | bool                               | bit                                        |
/// | u8                                 | tinyint                                    |
/// | i16                                | smallint                                   |
/// | i32                                | int                                        |
/// | i64                                | bigint                                     |
/// | f32                                | real                                       |
/// | f64                                | float                                      |
/// | rust_decimal::Decimal / Decimal    | decimal, numeric, money, smallmoney        |
/// | String                             | char, varchar, nchar, nvarchar, text, ntext, xml, sql_variant |
/// | Vec<u8>                            | binary, varbinary, image, rowversion, timestamp |
/// | chrono::NaiveDate / NaiveDate      | date                                       |
/// | chrono::NaiveTime / NaiveTime      | time                                       |
/// | chrono::DateTime<Utc>              | datetime, datetime2, smalldatetime         |
/// | chrono::DateTime<FixedOffset>      | datetimeoffset                             |
/// | uuid::Uuid / Uuid                  | uniqueidentifier                           |
fn required_getter(ty_str: &str, col: &str) -> TokenStream {
    match ty_str {
        // ── Integer ───────────────────────────────────────────────────────────
        "bool"  => quote! { row.get_bool(#col)? },
        "u8"    => quote! { row.get_u8(#col)? },
        "i8"    => quote! { row.get_i16(#col)? as i8 },
        "i16"   => quote! { row.get_i16(#col)? },
        "i32"   => quote! { row.get_i32(#col)? },
        "i64"   => quote! { row.get_i64(#col)? },

        // ── Float ─────────────────────────────────────────────────────────────
        "f32"   => quote! { row.get_f32(#col)? },
        "f64"   => quote! { row.get_f64(#col)? },

        // ── Fixed-precision ───────────────────────────────────────────────────
        "Decimal"
        | "rust_decimal::Decimal"
        | "rust_decimal::decimal::Decimal" => quote! { row.get_decimal(#col)? },

        // ── String ────────────────────────────────────────────────────────────
        "String" => quote! { row.get_string(#col)? },

        // ── Binary ────────────────────────────────────────────────────────────
        "Vec<u8>" => quote! { row.get_bytes(#col)? },

        // ── Date / Time ───────────────────────────────────────────────────────
        "NaiveDate"
        | "chrono::NaiveDate" => quote! { row.get_naive_date(#col)? },

        "NaiveTime"
        | "chrono::NaiveTime" => quote! { row.get_naive_time(#col)? },

        "DateTime<Utc>"
        | "chrono::DateTime<Utc>"
        | "chrono::DateTime<chrono::Utc>" => quote! { row.get_datetime(#col)? },

        "DateTime<FixedOffset>"
        | "chrono::DateTime<FixedOffset>"
        | "chrono::DateTime<chrono::FixedOffset>" => quote! { row.get_datetime_offset(#col)? },

        // ── Uuid ──────────────────────────────────────────────────────────────
        "Uuid"
        | "uuid::Uuid" => quote! { row.get_uuid(#col)? },

        // ── Fallback: try string then parse ───────────────────────────────────
        _ => quote! {
            row.get_string(#col)?.parse().map_err(|e: Box<dyn std::error::Error + Send + Sync>| {
                statiq::error::SqlError::row_mapping(#col, e.to_string())
            })?
        },
    }
}

/// Optional (`Option<T>`) getter.
fn optional_getter(inner: &str, col: &str) -> TokenStream {
    match inner {
        // ── Integer ───────────────────────────────────────────────────────────
        "bool"  => quote! { row.get_bool_opt(#col)? },
        "u8"    => quote! { row.get_u8_opt(#col)? },
        "i8"    => quote! { row.get_i16_opt(#col)?.map(|v| v as i8) },
        "i16"   => quote! { row.get_i16_opt(#col)? },
        "i32"   => quote! { row.get_i32_opt(#col)? },
        "i64"   => quote! { row.get_i64_opt(#col)? },

        // ── Float ─────────────────────────────────────────────────────────────
        "f32"   => quote! { row.get_f32_opt(#col)? },
        "f64"   => quote! { row.get_f64_opt(#col)? },

        // ── Fixed-precision ───────────────────────────────────────────────────
        "Decimal"
        | "rust_decimal::Decimal"
        | "rust_decimal::decimal::Decimal" => quote! { row.get_decimal_opt(#col)? },

        // ── String ────────────────────────────────────────────────────────────
        "String" => quote! { row.get_string_opt(#col)? },

        // ── Binary ────────────────────────────────────────────────────────────
        "Vec<u8>" => quote! { row.get_bytes_opt(#col)? },

        // ── Date / Time ───────────────────────────────────────────────────────
        "NaiveDate"
        | "chrono::NaiveDate" => quote! { row.get_naive_date_opt(#col)? },

        "NaiveTime"
        | "chrono::NaiveTime" => quote! { row.get_naive_time_opt(#col)? },

        "DateTime<Utc>"
        | "chrono::DateTime<Utc>"
        | "chrono::DateTime<chrono::Utc>" => quote! { row.get_datetime_opt(#col)? },

        "DateTime<FixedOffset>"
        | "chrono::DateTime<FixedOffset>"
        | "chrono::DateTime<chrono::FixedOffset>" => quote! { row.get_datetime_offset_opt(#col)? },

        // ── Uuid ──────────────────────────────────────────────────────────────
        "Uuid"
        | "uuid::Uuid" => quote! { row.get_uuid_opt(#col)? },

        // ── Fallback ──────────────────────────────────────────────────────────
        _ => quote! {
            row.get_string_opt(#col)?.map(|s| s.parse().unwrap())
        },
    }
}
