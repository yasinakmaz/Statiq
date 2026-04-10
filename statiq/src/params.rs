use std::borrow::Cow;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

/// A single named parameter value for an ODBC query.
#[derive(Debug, Clone)]
pub struct OdbcParam {
    /// Parameter name — `Cow::Borrowed` for compile-time literals (zero-cost),
    /// `Cow::Owned` for runtime-generated names (e.g. SprocParams, no Box::leak).
    pub name: Cow<'static, str>,
    pub value: ParamValue,
}

impl OdbcParam {
    /// Compile-time constant names — used by `params!{}` macro and `#[derive(SqlEntity)]`.
    /// `&'static str` is stored as `Cow::Borrowed` at zero cost.
    #[inline]
    pub const fn new(name: &'static str, value: ParamValue) -> Self {
        Self {
            name: Cow::Borrowed(name),
            value,
        }
    }

    /// Runtime-generated names — used by `SprocParams::add()`.
    /// Eliminates the previous `Box::leak` memory leak.
    #[inline]
    pub fn new_dynamic(name: String, value: ParamValue) -> Self {
        Self {
            name: Cow::Owned(name),
            value,
        }
    }
}

/// All MSSQL-supported parameter types.
///
/// | SQL Server type                        | Rust variant         |
/// |----------------------------------------|----------------------|
/// | bit                                    | Bool                 |
/// | tinyint                                | U8                   |
/// | smallint                               | I16                  |
/// | int                                    | I32                  |
/// | bigint                                 | I64                  |
/// | real                                   | F32                  |
/// | float                                  | F64                  |
/// | decimal / numeric / money / smallmoney | Decimal              |
/// | char / varchar / nchar / nvarchar      | Str                  |
/// | text / ntext / xml                     | Str                  |
/// | binary / varbinary / image / timestamp | Bytes                |
/// | date                                   | NaiveDate            |
/// | time                                   | NaiveTime            |
/// | datetime / datetime2 / smalldatetime   | DateTime(Utc)        |
/// | datetimeoffset                         | DateTimeOffset       |
/// | uniqueidentifier                       | Guid                 |
/// | NULL                                   | Null                 |
#[derive(Debug, Clone)]
pub enum ParamValue {
    // ── Integer ───────────────────────────────────────────────────────────────
    Bool(bool),         // bit
    U8(u8),             // tinyint
    I16(i16),           // smallint
    I32(i32),           // int
    I64(i64),           // bigint

    // ── Float ─────────────────────────────────────────────────────────────────
    F32(f32),           // real
    F64(f64),           // float

    // ── Fixed-precision ───────────────────────────────────────────────────────
    Decimal(Decimal),   // decimal, numeric, money, smallmoney

    // ── String ────────────────────────────────────────────────────────────────
    Str(String),        // char, varchar, nchar, nvarchar, text, ntext, xml

    // ── Binary ────────────────────────────────────────────────────────────────
    Bytes(Vec<u8>),     // binary, varbinary, image, rowversion, timestamp

    // ── Date / Time ───────────────────────────────────────────────────────────
    NaiveDate(NaiveDate),               // date
    NaiveTime(NaiveTime),               // time
    DateTime(DateTime<Utc>),            // datetime, datetime2, smalldatetime
    DateTimeOffset(DateTime<FixedOffset>), // datetimeoffset

    // ── Other ─────────────────────────────────────────────────────────────────
    Guid(Uuid),         // uniqueidentifier
    Null,
}

// ── From conversions ─────────────────────────────────────────────────────────

impl From<bool>             for ParamValue { fn from(v: bool)             -> Self { Self::Bool(v) } }
impl From<u8>               for ParamValue { fn from(v: u8)               -> Self { Self::U8(v) } }
impl From<i8>               for ParamValue { fn from(v: i8)               -> Self { Self::I16(v as i16) } }
impl From<i16>              for ParamValue { fn from(v: i16)              -> Self { Self::I16(v) } }
impl From<i32>              for ParamValue { fn from(v: i32)              -> Self { Self::I32(v) } }
impl From<i64>              for ParamValue { fn from(v: i64)              -> Self { Self::I64(v) } }
impl From<f32>              for ParamValue { fn from(v: f32)              -> Self { Self::F32(v) } }
impl From<f64>              for ParamValue { fn from(v: f64)              -> Self { Self::F64(v) } }
impl From<Decimal>          for ParamValue { fn from(v: Decimal)          -> Self { Self::Decimal(v) } }
impl From<String>           for ParamValue { fn from(v: String)           -> Self { Self::Str(v) } }
impl From<&str>             for ParamValue { fn from(v: &str)             -> Self { Self::Str(v.to_owned()) } }
impl From<Vec<u8>>          for ParamValue { fn from(v: Vec<u8>)          -> Self { Self::Bytes(v) } }
impl From<NaiveDate>        for ParamValue { fn from(v: NaiveDate)        -> Self { Self::NaiveDate(v) } }
impl From<NaiveTime>        for ParamValue { fn from(v: NaiveTime)        -> Self { Self::NaiveTime(v) } }
impl From<DateTime<Utc>>    for ParamValue { fn from(v: DateTime<Utc>)    -> Self { Self::DateTime(v) } }
impl From<DateTime<FixedOffset>> for ParamValue {
    fn from(v: DateTime<FixedOffset>) -> Self { Self::DateTimeOffset(v) }
}
impl From<Uuid>             for ParamValue { fn from(v: Uuid)             -> Self { Self::Guid(v) } }

impl<T: Into<ParamValue>> From<Option<T>> for ParamValue {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(inner) => inner.into(),
            None => Self::Null,
        }
    }
}

// ── PkValue ──────────────────────────────────────────────────────────────────

/// Primary-key value — used for get_by_id / delete / exists.
#[derive(Debug, Clone)]
pub enum PkValue {
    I32(i32),
    I64(i64),
    Str(String),
    Guid(Uuid),
}

impl From<i32>    for PkValue { fn from(v: i32)    -> Self { Self::I32(v) } }
impl From<i64>    for PkValue { fn from(v: i64)    -> Self { Self::I64(v) } }
impl From<&str>   for PkValue { fn from(v: &str)   -> Self { Self::Str(v.to_owned()) } }
impl From<String> for PkValue { fn from(v: String) -> Self { Self::Str(v) } }
impl From<Uuid>   for PkValue { fn from(v: Uuid)   -> Self { Self::Guid(v) } }

impl std::fmt::Display for PkValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::I32(v)  => write!(f, "{v}"),
            Self::I64(v)  => write!(f, "{v}"),
            Self::Str(v)  => write!(f, "{v}"),
            Self::Guid(v) => write!(f, "{v}"),
        }
    }
}

impl PkValue {
    pub fn as_param(&self) -> ParamValue {
        match self {
            Self::I32(v)  => ParamValue::I32(*v),
            Self::I64(v)  => ParamValue::I64(*v),
            Self::Str(v)  => ParamValue::Str(v.clone()),
            Self::Guid(v) => ParamValue::Guid(*v),
        }
    }
}

// ── params!{} macro ──────────────────────────────────────────────────────────

/// Tip-güvenli parametre slice'ı oluşturur — heap allocation yok.
///
/// ```rust
/// let p = params!{ active: true, name: "Ali", amount: Decimal::new(1999, 2) };
/// ```
#[macro_export]
macro_rules! params {
    ($($key:ident : $val:expr),* $(,)?) => {
        &[
            $( $crate::params::OdbcParam::new(stringify!($key), $crate::params::ParamValue::from($val)) ),*
        ]
    };
}
