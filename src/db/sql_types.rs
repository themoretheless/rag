//! DuckDB conversions for domain types.
//!
//! The domain modules (`models`, `error`) stay free of `duckdb`; the store owns
//! the impls that bind them to SQL. Both types are local to this crate, so the
//! impls may live here without a newtype.

use duckdb::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use crate::error::AppError;
use crate::models::{DocStatus, DocumentKind, DocumentLayer};

/// String-coded domain enums bind and decode as their stored string.
macro_rules! sql_string_code {
    ($($name:ty),+) => {$(
        impl ToSql for $name {
            fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                String::column_result(value).map(Self::from)
            }
        }
    )+};
}

sql_string_code!(DocumentKind, DocumentLayer, DocStatus);

impl From<duckdb::Error> for AppError {
    fn from(err: duckdb::Error) -> Self {
        AppError::Db(err.to_string())
    }
}
