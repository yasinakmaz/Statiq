use syn::Attribute;

/// Parsed result of `#[sql_table("name", schema = "dbo")]`
#[derive(Default)]
pub struct TableAttr {
    pub table_name: Option<String>,
    pub schema: Option<String>,
}

/// Parsed result of `#[sql_column("ColName")]`
#[derive(Default, Clone)]
pub struct ColumnAttr {
    pub column_name: Option<String>,
}

/// Parsed from `#[sql_primary_key(identity)]`
#[derive(Default, Clone)]
pub struct PkAttr {
    pub is_identity: bool,
}

/// Marker for `#[sql_ignore]`
#[derive(Default, Clone)]
pub struct IgnoreAttr {
    pub ignore: bool,
}

/// Marker for `#[sql_computed]` — DB-computed column (read-only).
/// Included in SELECT / `from_row`; excluded from INSERT, UPDATE, MERGE, and `to_params`.
#[derive(Default, Clone)]
pub struct ComputedAttr {
    pub is_computed: bool,
}

/// Marker for `#[sql_default]` — column with a server-side default (GETDATE(), NEWID(), etc.).
/// Included in SELECT / `from_row` and UPDATE; excluded from INSERT and MERGE-insert source.
#[derive(Default, Clone)]
pub struct ServerDefaultAttr {
    pub is_server_default: bool,
}

impl TableAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let mut result = TableAttr::default();
        for attr in attrs {
            if !attr.path().is_ident("sql_table") {
                continue;
            }
            // Parse in a single pass: optional positional string literal first,
            // then zero or more `key = "value"` named args (e.g. schema = "Keycloak").
            let _ = attr.parse_args_with(|input: syn::parse::ParseStream| {
                // 1) Positional string literal: "TableName"
                if input.peek(syn::LitStr) {
                    let s: syn::LitStr = input.parse()?;
                    result.table_name = Some(s.value());
                    let _ = input.parse::<syn::Token![,]>();
                }
                // 2) Named key=value pairs: schema = "..."
                while !input.is_empty() {
                    let ident: syn::Ident = input.parse()?;
                    let _: syn::Token![=] = input.parse()?;
                    let s: syn::LitStr = input.parse()?;
                    if ident == "schema" {
                        result.schema = Some(s.value());
                    }
                    let _ = input.parse::<syn::Token![,]>();
                }
                Ok(())
            });
        }
        result
    }
}

impl ColumnAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let mut result = ColumnAttr::default();
        for attr in attrs {
            if !attr.path().is_ident("sql_column") {
                continue;
            }
            if let Ok(args) = attr.parse_args_with(
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
            ) {
                for arg in &args {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = arg
                    {
                        result.column_name = Some(s.value());
                        break;
                    }
                }
            }
        }
        result
    }
}

impl PkAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let mut result = PkAttr::default();
        for attr in attrs {
            if !attr.path().is_ident("sql_primary_key") {
                continue;
            }
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("identity") {
                    result.is_identity = true;
                }
                Ok(())
            });
        }
        result
    }
}

impl IgnoreAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let ignore = attrs.iter().any(|a| a.path().is_ident("sql_ignore"));
        Self { ignore }
    }
}

impl ComputedAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let is_computed = attrs.iter().any(|a| a.path().is_ident("sql_computed"));
        Self { is_computed }
    }
}

impl ServerDefaultAttr {
    pub fn from_attrs(attrs: &[Attribute]) -> Self {
        let is_server_default = attrs.iter().any(|a| a.path().is_ident("sql_default"));
        Self { is_server_default }
    }
}
