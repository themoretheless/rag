//! Aggregate row codecs. SQL projection order remains owned by each repository.

use crate::error::{AppError, Result};
use crate::models::{Chunk, Document, EmbeddingManifest};
use crate::util::parse_db_timestamp;

fn timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    parse_db_timestamp(value)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", value.trim())))
}

/// Rows per multi-row `INSERT ... VALUES (..), (..)` statement.
const INSERT_BATCH_ROWS: usize = 64;

/// Insert `items` with multi-row `VALUES` statements of up to
/// [`INSERT_BATCH_ROWS`] rows each.
///
/// `head` is the `INSERT INTO t (cols) VALUES` prefix, `row` one row's
/// placeholder tuple, and `bind` pushes that row's values in tuple order.
/// DuckDB executes each statement as a full query, so one statement per 64
/// rows is several times faster than re-executing a one-row statement.
pub(crate) fn insert_rows<'a, T>(
    conn: &duckdb::Connection,
    head: &str,
    row: &str,
    items: &'a [T],
    mut bind: impl FnMut(&'a T, &mut Vec<&'a dyn duckdb::ToSql>),
) -> Result<()> {
    let mut values: Vec<&'a dyn duckdb::ToSql> = Vec::new();
    for batch in items.chunks(INSERT_BATCH_ROWS) {
        values.clear();
        for item in batch {
            bind(item, &mut values);
        }
        let tuples = vec![row; batch.len()].join(", ");
        let mut stmt = conn.prepare(&format!("{head} {tuples}"))?;
        stmt.execute(duckdb::params_from_iter(values.iter()))?;
    }
    Ok(())
}

/// Run `stmt` with `params` and map every row with `map`.
///
/// Replaces the hand-written `while let Some(row) = rows.next()?` push loops.
pub(crate) fn query_vec<T, P: duckdb::Params>(
    stmt: &mut duckdb::Statement<'_>,
    params: P,
    mut map: impl FnMut(&duckdb::Row<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map(row)?);
    }
    Ok(out)
}

/// Call `visit` for every remaining row of an open query, in order.
pub(crate) fn visit_rows(
    rows: &mut duckdb::Rows<'_>,
    mut visit: impl FnMut(&duckdb::Row<'_>) -> Result<()>,
) -> Result<()> {
    while let Some(row) = rows.next()? {
        visit(row)?;
    }
    Ok(())
}

/// Like [`visit_rows`], but `visit` can stop early with `ControlFlow::Break`.
pub(crate) fn try_visit_rows(
    rows: &mut duckdb::Rows<'_>,
    mut visit: impl FnMut(&duckdb::Row<'_>) -> Result<std::ops::ControlFlow<()>>,
) -> Result<()> {
    while let Some(row) = rows.next()? {
        if visit(row)?.is_break() {
            break;
        }
    }
    Ok(())
}

/// Run `stmt` with `params` and call `visit` for every row, in order.
///
/// For loops that filter, fill maps or aggregate; collecting into a `Vec`
/// uses [`query_vec`]. `return Ok(())` inside `visit` skips to the next row.
pub(crate) fn for_each_row<P: duckdb::Params>(
    stmt: &mut duckdb::Statement<'_>,
    params: P,
    mut visit: impl FnMut(&duckdb::Row<'_>) -> Result<()>,
) -> Result<()> {
    let mut rows = stmt.query(params)?;
    while let Some(row) = rows.next()? {
        visit(row)?;
    }
    Ok(())
}

pub(crate) fn document(row: &duckdb::Row<'_>) -> Result<Document> {
    let created_raw: String = row.get(11)?;
    let updated_raw: String = row.get(12)?;
    Ok(Document {
        id: row.get(0)?, uri: row.get(1)?, title: row.get(2)?, content: row.get(3)?,
        metadata_json: row.get(4)?, content_hash: row.get(5)?, wing: row.get(6)?,
        room: row.get(7)?, source_file: row.get(8)?,
        layer: row.get::<_, Option<String>>(9)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "raw".into()),
        kind: row.get::<_, Option<String>>(10)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "document".into()).into(),
        created_at: timestamp(&created_raw)?, updated_at: timestamp(&updated_raw)?,
        status: row.get::<_, Option<String>>(13)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "active".into()),
        pinned: row.get::<_, Option<bool>>(14)?.unwrap_or(false),
        boost: row.get::<_, Option<f64>>(15)?.unwrap_or(1.0),
        revision: row.get::<_, Option<i64>>(16)?.unwrap_or(1),
    })
}

pub(crate) fn chunk(row: &duckdb::Row<'_>) -> Result<Chunk> {
    let embedding_json: String = row.get(4)?;
    Ok(Chunk {
        id: row.get(0)?, document_id: row.get(1)?, chunk_index: row.get(2)?, content: row.get(3)?,
        embedding: serde_json::from_str(&embedding_json)?, char_start: row.get(5)?, char_end: row.get(6)?,
        metadata_json: row.get::<_, Option<String>>(7)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "{}".into()),
    })
}

pub(crate) fn embedding_manifest(row: &duckdb::Row<'_>) -> Result<EmbeddingManifest> {
    let updated_raw: String = row.get(6)?;
    Ok(EmbeddingManifest {
        id: row.get(0)?, provider: row.get(1)?, model: row.get(2)?, dims: row.get(3)?,
        base_url: row.get(4)?, content_fingerprint: row.get(5)?, updated_at: timestamp(&updated_raw)?,
    })
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;

    #[test]
    fn row_helpers_visit_in_order_and_stop_on_break() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM range(5) AS t(n) ORDER BY n")
            .unwrap();
        let all = super::query_vec(&mut stmt, [], |row| Ok(row.get::<_, i64>(0)?)).unwrap();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);

        let mut seen = Vec::new();
        let mut rows = stmt.query([]).unwrap();
        super::try_visit_rows(&mut rows, |row| {
            let n: i64 = row.get(0)?;
            if n == 1 {
                return Ok(ControlFlow::Continue(()));
            }
            seen.push(n);
            Ok(if n == 3 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
        })
        .unwrap();
        assert_eq!(seen, vec![0, 2, 3]);
    }
}
