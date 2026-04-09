use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::SqlError;

// ── CellValue ─────────────────────────────────────────────────────────────────

/// Raw cell storage — mirrors every MSSQL column type.
///
/// When `TextRowSet` is used (scaffold mode), all cells arrive as `Text`.
/// Each typed getter parses the text string into the target type.
#[derive(Debug, Clone)]
pub enum CellValue {
    // ── Integer ───────────────────────────────────────────────────────────────
    Bool(bool),
    U8(u8),
    I16(i16),
    I32(i32),
    I64(i64),

    // ── Float ─────────────────────────────────────────────────────────────────
    F32(f32),
    F64(f64),

    // ── Fixed-precision ───────────────────────────────────────────────────────
    Decimal(Decimal),

    // ── String ────────────────────────────────────────────────────────────────
    Text(String),

    // ── Binary ────────────────────────────────────────────────────────────────
    Bytes(Vec<u8>),

    // ── Date / Time ───────────────────────────────────────────────────────────
    NaiveDate(NaiveDate),
    NaiveTime(NaiveTime),
    DateTime(DateTime<Utc>),
    DateTimeOffset(DateTime<FixedOffset>),

    // ── Other ─────────────────────────────────────────────────────────────────
    Guid(Uuid),
    Null,
}

// ── OdbcRow ──────────────────────────────────────────────────────────────────

/// A single result row with named columns.
///
/// `columns` is shared via `Arc` across all rows in the same result set —
/// cloning a row only increments a reference count, not the column list.
#[derive(Debug, Clone)]
pub struct OdbcRow {
    pub(crate) columns: Arc<Vec<String>>,
    pub(crate) values: Vec<CellValue>,
}

impl OdbcRow {
    pub fn new(columns: Arc<Vec<String>>, values: Vec<CellValue>) -> Self {
        Self { columns, values }
    }

    /// Direct positional value access — O(1), no column-name lookup.
    /// Used by macro-generated `from_row` implementations.
    pub fn value_at(&self, idx: usize) -> Result<&CellValue, SqlError> {
        self.values.get(idx).ok_or_else(|| {
            SqlError::row_mapping("__index__", "column index out of range")
        })
    }

    fn index_of(&self, name: &str) -> Result<usize, SqlError> {
        if name.is_empty() {
            return Ok(0); // first-column fallback for scalar queries
        }
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            .ok_or_else(|| SqlError::row_mapping_dynamic(
                name,
                format!("column '{name}' not found in result set"),
            ))
    }

    /// Return the first column as a raw string (COUNT, scalar queries, etc.).
    pub fn get_first_string(&self) -> Result<String, SqlError> {
        match self.values.first() {
            Some(CellValue::Text(s))    => Ok(s.clone()),
            Some(CellValue::I64(v))     => Ok(v.to_string()),
            Some(CellValue::I32(v))     => Ok(v.to_string()),
            Some(CellValue::Decimal(v)) => Ok(v.to_string()),
            Some(CellValue::Guid(v))    => Ok(v.to_string()),
            Some(CellValue::Null) | None => {
                Err(SqlError::row_mapping("__first__", "NULL or empty row"))
            }
            Some(other) => Err(SqlError::row_mapping(
                "__first__",
                format!("unexpected cell type: {other:?}"),
            )),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn cell(&self, col: &str) -> Result<&CellValue, SqlError> {
        Ok(&self.values[self.index_of(col)?])
    }

    fn parse_err(col: &str, msg: impl std::fmt::Display) -> SqlError {
        SqlError::row_mapping_dynamic(col, msg.to_string())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // bool  (bit)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_bool(&self, col: &str) -> Result<bool, SqlError> {
        match self.cell(col)? {
            CellValue::Bool(v)  => Ok(*v),
            CellValue::I32(v)   => Ok(*v != 0),
            CellValue::I64(v)   => Ok(*v != 0),
            CellValue::U8(v)    => Ok(*v != 0),
            CellValue::Text(s)  => Ok(matches!(s.trim(), "1" | "true" | "True" | "TRUE")),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected bool, got {other:?}"))),
        }
    }

    pub fn get_bool_opt(&self, col: &str) -> Result<Option<bool>, SqlError> {
        match self.cell(col)? {
            CellValue::Null    => Ok(None),
            CellValue::Bool(v) => Ok(Some(*v)),
            CellValue::I32(v)  => Ok(Some(*v != 0)),
            CellValue::I64(v)  => Ok(Some(*v != 0)),
            CellValue::U8(v)   => Ok(Some(*v != 0)),
            CellValue::Text(s) => Ok(Some(matches!(s.trim(), "1" | "true" | "True" | "TRUE"))),
            other => Err(Self::parse_err(col, format!("expected bool, got {other:?}"))),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // u8  (tinyint)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_u8(&self, col: &str) -> Result<u8, SqlError> {
        match self.cell(col)? {
            CellValue::U8(v)   => Ok(*v),
            CellValue::I32(v)  => Ok(*v as u8),
            CellValue::I64(v)  => Ok(*v as u8),
            CellValue::Text(s) => s.trim().parse::<u8>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected u8, got {other:?}"))),
        }
    }

    pub fn get_u8_opt(&self, col: &str) -> Result<Option<u8>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_u8(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // i16  (smallint)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_i16(&self, col: &str) -> Result<i16, SqlError> {
        match self.cell(col)? {
            CellValue::I16(v)  => Ok(*v),
            CellValue::I32(v)  => Ok(*v as i16),
            CellValue::I64(v)  => Ok(*v as i16),
            CellValue::Text(s) => s.trim().parse::<i16>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected i16, got {other:?}"))),
        }
    }

    pub fn get_i16_opt(&self, col: &str) -> Result<Option<i16>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_i16(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // i32  (int)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_i32(&self, col: &str) -> Result<i32, SqlError> {
        match self.cell(col)? {
            CellValue::I32(v)  => Ok(*v),
            CellValue::I16(v)  => Ok(*v as i32),
            CellValue::I64(v)  => Ok(*v as i32),
            CellValue::U8(v)   => Ok(*v as i32),
            CellValue::Text(s) => s.trim().parse::<i32>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected i32, got {other:?}"))),
        }
    }

    pub fn get_i32_opt(&self, col: &str) -> Result<Option<i32>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_i32(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // i64  (bigint)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_i64(&self, col: &str) -> Result<i64, SqlError> {
        match self.cell(col)? {
            CellValue::I64(v)  => Ok(*v),
            CellValue::I32(v)  => Ok(*v as i64),
            CellValue::I16(v)  => Ok(*v as i64),
            CellValue::U8(v)   => Ok(*v as i64),
            CellValue::Text(s) => s.trim().parse::<i64>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected i64, got {other:?}"))),
        }
    }

    pub fn get_i64_opt(&self, col: &str) -> Result<Option<i64>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_i64(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // f32  (real)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_f32(&self, col: &str) -> Result<f32, SqlError> {
        match self.cell(col)? {
            CellValue::F32(v)  => Ok(*v),
            CellValue::F64(v)  => Ok(*v as f32),
            CellValue::Text(s) => s.trim().parse::<f32>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected f32, got {other:?}"))),
        }
    }

    pub fn get_f32_opt(&self, col: &str) -> Result<Option<f32>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_f32(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // f64  (float)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_f64(&self, col: &str) -> Result<f64, SqlError> {
        match self.cell(col)? {
            CellValue::F64(v)  => Ok(*v),
            CellValue::F32(v)  => Ok(*v as f64),
            CellValue::Text(s) => s.trim().parse::<f64>().map_err(|e| Self::parse_err(col, e)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected f64, got {other:?}"))),
        }
    }

    pub fn get_f64_opt(&self, col: &str) -> Result<Option<f64>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_f64(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Decimal  (decimal, numeric, money, smallmoney)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_decimal(&self, col: &str) -> Result<Decimal, SqlError> {
        match self.cell(col)? {
            CellValue::Decimal(v) => Ok(*v),
            CellValue::Text(s)    => {
                // Money columns come back with a currency symbol in some locales — strip it
                let clean = s.trim().trim_start_matches(['$', '€', '£', '¥']);
                clean.parse::<Decimal>().map_err(|e| Self::parse_err(col, e))
            }
            CellValue::F64(v) => Decimal::try_from(*v).map_err(|e| Self::parse_err(col, e)),
            CellValue::I64(v) => Ok(Decimal::from(*v)),
            CellValue::I32(v) => Ok(Decimal::from(*v)),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected Decimal, got {other:?}"))),
        }
    }

    pub fn get_decimal_opt(&self, col: &str) -> Result<Option<Decimal>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_decimal(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // String  (varchar, nvarchar, char, nchar, text, ntext, xml, sql_variant)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_string(&self, col: &str) -> Result<String, SqlError> {
        match self.cell(col)? {
            CellValue::Text(v) => Ok(v.clone()),
            CellValue::I32(v)  => Ok(v.to_string()),
            CellValue::I64(v)  => Ok(v.to_string()),
            CellValue::Decimal(v) => Ok(v.to_string()),
            CellValue::Guid(v) => Ok(v.to_string()),
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected String, got {other:?}"))),
        }
    }

    pub fn get_string_opt(&self, col: &str) -> Result<Option<String>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_string(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Vec<u8>  (binary, varbinary, image, rowversion, timestamp)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_bytes(&self, col: &str) -> Result<Vec<u8>, SqlError> {
        match self.cell(col)? {
            CellValue::Bytes(v) => Ok(v.clone()),
            CellValue::Text(s)  => {
                // ODBC sometimes returns binary as "0x..." hex string
                let s = s.trim();
                if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16)
                            .map_err(|e| Self::parse_err(col, e)))
                        .collect()
                } else {
                    Ok(s.as_bytes().to_vec())
                }
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected Bytes, got {other:?}"))),
        }
    }

    pub fn get_bytes_opt(&self, col: &str) -> Result<Option<Vec<u8>>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_bytes(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // NaiveDate  (date)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_naive_date(&self, col: &str) -> Result<NaiveDate, SqlError> {
        match self.cell(col)? {
            CellValue::NaiveDate(v) => Ok(*v),
            CellValue::Text(s) => {
                // SQL Server: "YYYY-MM-DD"
                NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
                    .map_err(|e| Self::parse_err(col, e))
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected NaiveDate, got {other:?}"))),
        }
    }

    pub fn get_naive_date_opt(&self, col: &str) -> Result<Option<NaiveDate>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_naive_date(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // NaiveTime  (time)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_naive_time(&self, col: &str) -> Result<NaiveTime, SqlError> {
        match self.cell(col)? {
            CellValue::NaiveTime(v) => Ok(*v),
            CellValue::Text(s) => {
                let s = s.trim();
                // Try up to 7 fractional digits: "HH:MM:SS.nnnnnnn"
                NaiveTime::parse_from_str(s, "%H:%M:%S%.f")
                    .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
                    .map_err(|e| Self::parse_err(col, e))
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected NaiveTime, got {other:?}"))),
        }
    }

    pub fn get_naive_time_opt(&self, col: &str) -> Result<Option<NaiveTime>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_naive_time(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // DateTime<Utc>  (datetime, datetime2, smalldatetime)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_datetime(&self, col: &str) -> Result<DateTime<Utc>, SqlError> {
        match self.cell(col)? {
            CellValue::DateTime(v) => Ok(*v),
            CellValue::Text(s) => {
                let s = s.trim();
                // Try RFC3339, then MSSQL formats
                s.parse::<DateTime<Utc>>()
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                            .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                            .map(|dt| dt.and_utc())
                    })
                    .map_err(|e| Self::parse_err(col, e))
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected DateTime<Utc>, got {other:?}"))),
        }
    }

    pub fn get_datetime_opt(&self, col: &str) -> Result<Option<DateTime<Utc>>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_datetime(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // DateTime<FixedOffset>  (datetimeoffset)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_datetime_offset(&self, col: &str) -> Result<DateTime<FixedOffset>, SqlError> {
        match self.cell(col)? {
            CellValue::DateTimeOffset(v) => Ok(*v),
            CellValue::Text(s) => {
                let s = s.trim();
                // SQL Server format: "YYYY-MM-DD HH:MM:SS.nnnnnnn +HH:MM"
                DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f %:z")
                    .or_else(|_| DateTime::parse_from_rfc3339(s))
                    .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S %:z"))
                    .map_err(|e| Self::parse_err(col, e))
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected DateTimeOffset, got {other:?}"))),
        }
    }

    pub fn get_datetime_offset_opt(&self, col: &str) -> Result<Option<DateTime<FixedOffset>>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_datetime_offset(col).map(Some),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Uuid  (uniqueidentifier)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn get_uuid(&self, col: &str) -> Result<Uuid, SqlError> {
        match self.cell(col)? {
            CellValue::Guid(v) => Ok(*v),
            CellValue::Text(s) => {
                Uuid::parse_str(s.trim()).map_err(|e| Self::parse_err(col, e))
            }
            CellValue::Null => Err(Self::parse_err(col, "unexpected NULL")),
            other => Err(Self::parse_err(col, format!("expected Uuid, got {other:?}"))),
        }
    }

    pub fn get_uuid_opt(&self, col: &str) -> Result<Option<Uuid>, SqlError> {
        match self.cell(col)? {
            CellValue::Null => Ok(None),
            _ => self.get_uuid(col).map(Some),
        }
    }
}
