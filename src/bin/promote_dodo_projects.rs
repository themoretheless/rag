use std::path::Path;

use anyhow::{Context, Result};
use rag_mcp::Store;
use rag_mcp::db::lin_engine::{LinReader, LinWriter, RecBuilder, opt_str, req_str};

/// One-off data migration: promote `wing = 'Dodo'` rooms to projects.
#[derive(clap::Parser)]
#[command(name = "promote_dodo_projects")]
struct Args {
    /// Database file to migrate.
    db_path: String,
}

fn main() -> Result<()> {
    let Args { db_path } = <Args as clap::Parser>::parse();
    let store = Store::open(Path::new(&db_path)).context("open database")?;
    let rows = {
        let r = store.read().context("read handle")?;
        let mut out = Vec::new();
        for rec in r.find_by_eq("documents", "wing", "Dodo")? {
            let id = req_str(&rec, "id")?.to_string();
            let room = opt_str(&rec, "room").unwrap_or("").to_string();
            let source_file = opt_str(&rec, "source_file").unwrap_or("").to_string();
            let metadata_json = opt_str(&rec, "metadata_json").unwrap_or("{}").to_string();
            out.push((id, room, source_file, metadata_json));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    };

    {
        let mut w = store.write().context("write handle")?;
        let mut tx = w.begin_tx().context("begin transaction")?;
        for (id, old_room, source_file, metadata_json) in &rows {
            let project = if old_room == "overview" {
                "Dodo-overview".to_string()
            } else {
                old_room.clone()
            };
            let room = nested_room(source_file, &project);
            let metadata_json = add_tag(metadata_json, "Dodo")?;
            tx.update_where(
                "documents",
                lin::query::pred::eq("id", id.as_str()),
                RecBuilder::new()
                    .str("wing", &project)
                    .str("room", &room)
                    .str("metadata_json", &metadata_json)
                    .build(),
            )?;
        }
        tx.commit().context("commit migration")?;
    }
    println!("migrated {} Dodo documents", rows.len());
    let (tagged, remaining) = {
        let r = store.read().context("read handle")?;
        let mut tagged = 0i64;
        let mut remaining = 0i64;
        r.scan_each("documents", |rec| {
            if opt_str(rec, "wing") == Some("Dodo") {
                remaining += 1;
            }
            if let Some(meta_str) = opt_str(rec, "metadata_json")
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(meta_str)
            {
                let has_dodo = val
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("Dodo")));
                if has_dodo {
                    tagged += 1;
                }
            }
            Ok(true)
        })?;
        (tagged, remaining)
    };
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
