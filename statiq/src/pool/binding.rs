//! Gerçek ODBC parametre binding — SQL string'e gömme YOK.
//!
//! `params_to_positional` fonksiyonu, `@name` placeholder'larını pozisyonel `?`
//! marker'larına çevirir ve her parametreyi `Box<dyn InputParameter>` olarak
//! hazırlar. Bu sayede SQL Server execution plan cache çalışır ve SQL injection
//! riski tamamen ortadan kalkar.

use chrono::Datelike as _;
use chrono::Timelike as _;
use odbc_api::{
    Bit, DataType, Nullable,
    parameter::{InputParameter, VarBinaryBox, VarCharBox, WithDataType},
    sys::{Date as OdbcDate, Time as OdbcTime, Timestamp as OdbcTs},
};

use crate::params::{OdbcParam, ParamValue};

// ── Tek parametre → Box<dyn InputParameter> ───────────────────────────────────

/// `ParamValue`'u, ODBC driver'ın doğrudan alacağı heap-allocated input param'a
/// dönüştürür. `String` ve `Vec<u8>` kopyalanır; scalar'lar stack-allocation ile
/// `Nullable<T>` veya `WithDataType<T>` içinde tutulur.
///
/// ## SQL Server type mapping
///
/// | `ParamValue`    | ODBC C Type          | SQL Type               |
/// |-----------------|----------------------|------------------------|
/// | Bool            | `Nullable<Bit>`      | bit                    |
/// | U8              | `Nullable<i16>`      | smallint (0..=255 fit) |
/// | I16             | `Nullable<i16>`      | smallint               |
/// | I32             | `Nullable<i32>`      | int                    |
/// | I64             | `Nullable<i64>`      | bigint                 |
/// | F32             | `Nullable<f32>`      | real                   |
/// | F64             | `Nullable<f64>`      | float                  |
/// | Decimal         | `VarCharBox`         | decimal (string cast)  |
/// | Str             | `VarCharBox`         | varchar/nvarchar       |
/// | Bytes           | `VarBinaryBox`       | varbinary              |
/// | NaiveDate       | `Nullable<OdbcDate>` | date                   |
/// | NaiveTime       | `WithDataType<Time>` | time(0)                |
/// | DateTime        | `WithDataType<Ts>`   | datetime2(7)           |
/// | DateTimeOffset  | `VarCharBox`         | datetimeoffset (string)|
/// | Guid            | `VarCharBox`         | uniqueidentifier       |
/// | Null            | `VarCharBox::null()` | NULL                   |
pub fn param_to_box(value: &ParamValue) -> Box<dyn InputParameter> {
    match value {
        // ── Integer ────────────────────────────────────────────────────────────
        ParamValue::Bool(v) => Box::new(Nullable::new(Bit::from_bool(*v))),

        // u8 (tinyint 0-255) — u8 has no HasDataType impl so widen to i16
        ParamValue::U8(v) => Box::new(Nullable::new(*v as i16)),

        ParamValue::I16(v) => Box::new(Nullable::new(*v)),
        ParamValue::I32(v) => Box::new(Nullable::new(*v)),
        ParamValue::I64(v) => Box::new(Nullable::new(*v)),

        // ── Float ──────────────────────────────────────────────────────────────
        ParamValue::F32(v) => Box::new(Nullable::new(*v)),
        ParamValue::F64(v) => Box::new(Nullable::new(*v)),

        // ── Fixed-precision: SQL Server accepts decimal as string ──────────────
        ParamValue::Decimal(v) => Box::new(VarCharBox::from_string(v.to_string())),

        // ── String ─────────────────────────────────────────────────────────────
        ParamValue::Str(v) => Box::new(VarCharBox::from_string(v.clone())),

        // ── Binary ─────────────────────────────────────────────────────────────
        ParamValue::Bytes(v) => Box::new(VarBinaryBox::from_vec(v.clone())),

        // ── Date ───────────────────────────────────────────────────────────────
        // OdbcDate has HasDataType (DataType::Date) so Nullable<OdbcDate> works.
        ParamValue::NaiveDate(v) => Box::new(Nullable::new(OdbcDate {
            year: v.year() as i16,
            month: v.month() as u16,
            day: v.day() as u16,
        })),

        // ── Time ───────────────────────────────────────────────────────────────
        // OdbcTime has CData/CElement (Pod) but NOT HasDataType — use WithDataType.
        ParamValue::NaiveTime(v) => Box::new(WithDataType::new(
            OdbcTime {
                hour: v.hour() as u16,
                minute: v.minute() as u16,
                second: v.second() as u16,
            },
            DataType::Time { precision: 0 },
        )),

        // ── DateTime (UTC) ─────────────────────────────────────────────────────
        // OdbcTimestamp has Pod but NOT HasDataType — use WithDataType.
        // fraction is in 100-nanosecond units (SQL Server datetime2 precision).
        ParamValue::DateTime(v) => Box::new(WithDataType::new(
            OdbcTs {
                year: v.year() as i16,
                month: v.month() as u16,
                day: v.day() as u16,
                hour: v.hour() as u16,
                minute: v.minute() as u16,
                second: v.second() as u16,
                fraction: v.timestamp_subsec_nanos() / 100,
            },
            DataType::Timestamp { precision: 7 },
        )),

        // ── DateTimeOffset ─────────────────────────────────────────────────────
        // No ODBC C type for datetimeoffset — send as string in SQL Server format.
        ParamValue::DateTimeOffset(v) => {
            let s = v.format("%Y-%m-%d %H:%M:%S%.7f %:z").to_string();
            Box::new(VarCharBox::from_string(s))
        }

        // ── GUID (uniqueidentifier) ────────────────────────────────────────────
        ParamValue::Guid(v) => Box::new(VarCharBox::from_string(v.to_string())),

        // ── NULL ───────────────────────────────────────────────────────────────
        ParamValue::Null => Box::new(VarCharBox::null()),
    }
}

// ── SQL dönüştürme: @name → ? ─────────────────────────────────────────────────

/// `@name` placeholder'larını pozisyonel `?` marker'larına çevirir.
///
/// Aynı algoritma `embed_params`'taki gibi: isimler en-uzun-önce sırasıyla
/// denenir, kelime sınırları (alphanumeric/underscore olmayan karakter) kontrol
/// edilir. Döndürülen `Vec<Box<dyn InputParameter>>` pozisyonel sıradaki
/// parametre değerlerini içerir.
///
/// Aynı `@name` birden fazla geçiyorsa, her geçiş için ayrı bir Box clone'u
/// oluşturulur — ODBC her `?` için bağımsız bir binding bekler.
pub fn params_to_positional(
    sql: &str,
    params: &[OdbcParam],
) -> (String, Vec<Box<dyn InputParameter>>) {
    if params.is_empty() {
        return (sql.to_owned(), Vec::new());
    }

    // En uzun isim önce — @id_suffix'in @id ile eşleşmesini önler
    let mut sorted: Vec<&OdbcParam> = params.iter().collect();
    sorted.sort_unstable_by(|a, b| b.name.len().cmp(&a.name.len()));

    let mut out = String::with_capacity(sql.len());
    let mut bound: Vec<Box<dyn InputParameter>> = Vec::with_capacity(params.len());

    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'@' {
            let rest = &sql[i + 1..];

            // En uzun eşleşeni bul
            let matched = sorted.iter().find(|p| {
                // &*p.name works for both &'static str and Cow<'static, str>
                let name: &str = &*p.name;
                rest.starts_with(name) && {
                    // Kelime sınırı: isimden sonra alphanumeric veya _ yoksa match
                    let after = rest[name.len()..].as_bytes().first().copied();
                    !matches!(after, Some(c) if c.is_ascii_alphanumeric() || c == b'_')
                }
            });

            if let Some(p) = matched {
                out.push('?');
                bound.push(param_to_box(&p.value));
                i += 1 + p.name.len();
            } else {
                // Bilinmeyen @ — olduğu gibi bırak (sistem parametreleri vb.)
                out.push('@');
                i += 1;
            }
        } else {
            // SAFETY: UTF-8 string içindeyiz; bytes[i] '@' değilse tam karakter okuruz.
            let ch = sql[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    (out, bound)
}
