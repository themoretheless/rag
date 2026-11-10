//! DuckDB conversions for domain types.
//!
//! The domain modules (`models`, `error`) stay free of `duckdb`; the store owns
//! the impls that bind them to SQL. Both types are local to this crate, so the
//! impls may live here without a newtype.

use duckdb::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use crate::error::AppError;
use crate::models::DocumentKind;

impl ToSql for DocumentKind {
    fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for DocumentKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value).map(Self::from)
    }
}

impl From<duckdb::Error> for AppError {
    fn from(err: duckdb::Error) -> Self {
        AppError::Db(err.to_string())
    }
}
