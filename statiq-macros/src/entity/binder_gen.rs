use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use super::parse::StructInfo;

/// Generates `to_params()` — returns `Vec<OdbcParam>` for INSERT/UPDATE.
/// Computed columns are excluded entirely (never written).
/// Server-default columns are included (needed for UPDATE SET clause).
pub fn generate_to_params(info: &StructInfo) -> TokenStream {
    let active_non_ignored: Vec<_> = info
        .active_fields()
        .filter(|f| !f.is_computed)
        .collect();

    let param_entries: Vec<TokenStream> = active_non_ignored
        .iter()
        .map(|f| {
            let rust_ident = format_ident!("{}", f.rust_name);
            let param_name = &f.rust_name;
            quote! {
                statiq::params::OdbcParam::new(
                    #param_name,
                    statiq::params::ParamValue::from(self.#rust_ident.clone()),
                )
            }
        })
        .collect();

    quote! {
        fn to_params(&self) -> Vec<statiq::params::OdbcParam> {
            vec![
                #(#param_entries),*
            ]
        }
    }
}

/// Generates `pk_value()` — returns the `PkValue` for the primary key field.
pub fn generate_pk_value(info: &StructInfo) -> TokenStream {
    let pk = info.pk_field();

    if let Some(pk) = pk {
        let rust_ident = format_ident!("{}", pk.rust_name);
        quote! {
            fn pk_value(&self) -> statiq::params::PkValue {
                statiq::params::PkValue::from(self.#rust_ident.clone())
            }
        }
    } else {
        // parse.rs guarantees a PK field exists before we reach here.
        // This branch is unreachable in practice but kept as a safety net.
        quote! {
            fn pk_value(&self) -> statiq::params::PkValue {
                panic!("SqlEntity: no #[sql_primary_key] field — \
                        this is a proc-macro bug, please report it")
            }
        }
    }
}
