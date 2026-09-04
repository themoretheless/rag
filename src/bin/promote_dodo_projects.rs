use std::path::Path;

use anyhow::{Context, Result};
use duckdb::{params, Connection};

fn main() -> Result<()> {
    let db_path = std::env::args()
        .nth(1)
        .context("usage: promote_dodo_projects <database.duckdb>")?;
    let mut connection = Connection::open(&db_path).context("open database")?;
    let tx = connection.transaction().context("begin transaction")?;
    let rows = {
        let mut statement = tx.prepare(
            "SELECT id, COALESCE(room, ''), COALESCE(source_file, ''), metadata_json \
             FROM documents WHERE wing = 'Dodo' ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };

    for (id, old_room, source_file, metadata_json) in &rows {
        let project = if old_room == "overview" {
            "Dodo-overview".to_string()
        } else {
            old_room.clone()
        };
        let room = nested_room(source_file, &project);
        let metadata_json = add_tag(metadata_json, "Dodo")?;
        tx.execute(
            "UPDATE documents SET wing = ?, room = ?, metadata_json = ? WHERE id = ?",
            params![project, room, metadata_json, id],
        )?;
    }
    tx.commit().context("commit migration")?;
    println!("migrated {} Dodo documents", rows.len());
    let tagged: i64 = connection.query_row(
        "SELECT COUNT(*) FROM documents WHERE json_contains(metadata_json, '\"Dodo\"')",
        [],
        |row| row.get(0),
    )?;
    let remaining: i64 = connection.query_row(
        "SELECT COUNT(*) FROM documents WHERE wing = 'Dodo'",
        [],
        |row| row.get(0),
    )?;
    println!("Dodo-tagged documents: {tagged}; remaining Dodo wing: {remaining}");
    Ok(())
}

fn nested_room(source_file: &str, project: &str) -> String {
    let components = Path::new(source_file)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components
        .windows(3)
        .find(|window| window[0].eq_ignore_ascii_case("Dodo") && window[1] == project)
        .map(|window| window[2].to_string())
        .unwrap_or_else(|| {
            if source_file.is_empty() {
                "wiki".into()
            } else {
                "root".into()
            }
        })
}

fn add_tag(metadata_json: &str, tag: &str) -> Result<String> {
    let mut metadata: serde_json::Value = serde_json::from_str(metadata_json)?;
    let object = metadata
        .as_object_mut()
        .context("document metadata must be an object")?;
    let tags = object
        .entry("tags")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()))
        .as_array_mut()
        .context("document metadata tags must be an array")?;
    if !tags.iter().any(|value| value.as_str() == Some(tag)) {
        tags.push(serde_json::Value::String(tag.into()));
    }
    Ok(serde_json::to_string(&metadata)?)
}
