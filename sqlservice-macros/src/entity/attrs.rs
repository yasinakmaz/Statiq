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
            // Parse comma-separated: first arg is a string literal (table name),
            // optional `schema = "..."` named arg.
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("schema") {
                    let value = meta.value()?;
                    let s: syn::LitStr = value.parse()?;
                    result.schema = Some(s.value());
                }
                Ok(())
            });

            // The table name is the first bare string literal in the attr args.
            if result.table_name.is_none() {
                if let Ok(args) = attr.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
                ) {
                    for arg in &args {
                        if let syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(s),
                            ..
                        }) = arg
                        {
                            result.table_name = Some(s.value());
                            break;
                        }
                    }
                }
            }
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
