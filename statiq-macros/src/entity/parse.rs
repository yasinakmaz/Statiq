use syn::{Data, DeriveInput, Fields};
use super::attrs::{ColumnAttr, ComputedAttr, IgnoreAttr, MaskAttr, PkAttr, ServerDefaultAttr, SoftDeleteAttr, TableAttr, TenantIdAttr};

/// One field of the struct, fully resolved.
pub struct FieldInfo {
    pub rust_name: String,
    pub sql_name: String,
    pub ty: syn::Type,
    pub is_pk: bool,
    pub pk_is_identity: bool,
    pub is_ignored: bool,
    /// `#[sql_computed]` — DB-computed column; SELECT-only, excluded from all writes.
    pub is_computed: bool,
    /// `#[sql_default]` — server-side default; excluded from INSERT, included in UPDATE.
    pub is_server_default: bool,
    /// `#[sql_mask]` — value replaced with `"***"` in `from_row`.
    pub is_masked: bool,
}

pub struct StructInfo {
    pub struct_name: syn::Ident,
    pub table_name: String,
    pub schema: String,
    pub fields: Vec<FieldInfo>,
    /// Column name for soft-delete flag (from `#[sql_soft_delete("IsDeleted")]`).
    pub soft_delete_col: Option<String>,
    /// Column name for tenant isolation (from `#[sql_tenant_id("TenantId")]`).
    pub tenant_id_col: Option<String>,
}

impl StructInfo {
    pub fn parse(input: &DeriveInput) -> syn::Result<Self> {
        let table_attr = TableAttr::from_attrs(&input.attrs);
        let table_name = table_attr
            .table_name
            .clone()
            .unwrap_or_else(|| input.ident.to_string());
        let schema = table_attr.schema.clone().unwrap_or_else(|| "dbo".to_string());
        let soft_delete_col = SoftDeleteAttr::from_attrs(&input.attrs).column;
        let tenant_id_col = TenantIdAttr::from_attrs(&input.attrs).column;

        let named_fields = match &input.data {
            Data::Struct(ds) => match &ds.fields {
                Fields::Named(f) => &f.named,
                _ => return Err(syn::Error::new_spanned(&input.ident, "SqlEntity requires named fields")),
            },
            _ => return Err(syn::Error::new_spanned(&input.ident, "SqlEntity can only be derived on structs")),
        };

        let mut fields = Vec::new();
        for field in named_fields {
            let rust_name = field.ident.as_ref().unwrap().to_string();
            let ignore   = IgnoreAttr::from_attrs(&field.attrs);
            let pk       = PkAttr::from_attrs(&field.attrs);
            let col      = ColumnAttr::from_attrs(&field.attrs);
            let computed = ComputedAttr::from_attrs(&field.attrs);
            let default  = ServerDefaultAttr::from_attrs(&field.attrs);
            let mask     = MaskAttr::from_attrs(&field.attrs);

            let sql_name = col.column_name.clone().unwrap_or_else(|| rust_name.clone());

            fields.push(FieldInfo {
                rust_name,
                sql_name,
                ty: field.ty.clone(),
                is_pk: field.attrs.iter().any(|a| a.path().is_ident("sql_primary_key")),
                pk_is_identity: pk.is_identity,
                is_ignored: ignore.ignore,
                is_computed: computed.is_computed,
                is_server_default: default.is_server_default,
                is_masked: mask.is_masked,
            });
        }

        // Every SqlEntity must have exactly one #[sql_primary_key] field.
        // Catch this at compile-time instead of producing a runtime panic.
        let has_pk = fields.iter().any(|f| f.is_pk);
        if !has_pk {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "SqlEntity requires exactly one field marked with \
                 `#[sql_primary_key]` or `#[sql_primary_key(identity)]`. \
                 Add the attribute to your primary-key field.",
            ));
        }

        Ok(StructInfo {
            struct_name: input.ident.clone(),
            table_name,
            schema,
            fields,
            soft_delete_col,
            tenant_id_col,
        })
    }

    /// Non-ignored fields only.
    pub fn active_fields(&self) -> impl Iterator<Item = &FieldInfo> {
        self.fields.iter().filter(|f| !f.is_ignored)
    }

    /// Non-ignored, non-identity-PK, non-computed, non-server-default fields (for INSERT VALUES).
    pub fn insert_fields(&self) -> impl Iterator<Item = &FieldInfo> {
        self.fields.iter().filter(|f| {
            !f.is_ignored && !(f.is_pk && f.pk_is_identity) && !f.is_computed && !f.is_server_default
        })
    }

    /// Non-ignored, non-PK, non-computed fields (for UPDATE SET clause).
    /// Server-default fields ARE included — they can be updated explicitly.
    pub fn update_fields(&self) -> impl Iterator<Item = &FieldInfo> {
        self.fields.iter().filter(|f| !f.is_ignored && !f.is_pk && !f.is_computed)
    }

    /// PK field (first one).
    pub fn pk_field(&self) -> Option<&FieldInfo> {
        self.fields.iter().find(|f| f.is_pk)
    }
}
