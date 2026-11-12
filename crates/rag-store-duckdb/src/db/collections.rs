//! Durable collections, ordered outline membership, and prerequisite links over `lin`.

use std::collections::{HashMap, HashSet};

use super::Store;
use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_i32, cell_opt_string, cell_str, cell_string,
    composite_id_2, composite_id_3,
};
use crate::error::{AppError, Result};
use crate::models::{Collection, CollectionDetail, CollectionEntry};
use crate::util::{format_db_timestamp, parse_flexible_timestamp};

impl Store {
    /// Create a collection with its entries after validating them, in one transaction.
    pub fn create_collection(
        &self,
        collection: &Collection,
        entries: &[CollectionEntry],
    ) -> Result<CollectionDetail> {
        validate_collection(collection, entries)?;
        self.with_tx(|tx| {
            if tx.get_by_id("collections", &collection.id)?.is_some() {
                return Err(AppError::db(format!(
                    "collection already exists: {}",
                    collection.id
                )));
            }
            validate_entries(tx, entries)?;
            let created_at = normalize_ts(&collection.created_at)?;
            let updated_at = normalize_ts(&collection.updated_at)?;
            let rec = RecBuilder::new()
                .id(&collection.id)
                .str("name", &collection.name)
                .opt_str("description", collection.description.as_deref())
                .str("metadata_json", &collection.metadata_json)
                .str("created_at", &created_at)
                .str("updated_at", &updated_at)
                .build();
            tx.upsert("collections", rec)?;
            replace_entries(tx, &collection.id, entries)?;
            Ok(())
        })?;
        self.get_collection(&collection.id)?.ok_or_else(|| {
            AppError::not_found(format!(
                "collection not found after create: {}",
                collection.id
            ))
        })
    }

    /// List all collections, most recently updated first.
    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        let r = self.read()?;
        let mut out = r.filter_map_rows("collections", |row| Ok(Some(row_to_collection(row))))?;
        out.sort_by(|a, b| {
            let ta = parse_flexible_timestamp(&a.updated_at);
            let tb = parse_flexible_timestamp(&b.updated_at);
            tb.cmp(&ta)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Fetch one collection with its entries, or `None` when absent.
    pub fn get_collection(&self, id: &str) -> Result<Option<CollectionDetail>> {
        let r = self.read()?;
        let Some(col_row) = r.get_by_id("collections", id)? else {
            return Ok(None);
        };
        let collection = row_to_collection(&col_row);

        let dep_rows = r.find_by_eq("collection_dependencies", "collection_id", id)?;
        let mut deps_by_doc: HashMap<String, Vec<String>> = HashMap::new();
        for dep_row in &dep_rows {
            if let (Some(doc_id), Some(dep_id)) = (
                cell_str(dep_row, "document_id"),
                cell_str(dep_row, "depends_on_document_id"),
            ) {
                deps_by_doc
                    .entry(doc_id.to_string())
                    .or_default()
                    .push(dep_id.to_string());
            }
        }
        for deps in deps_by_doc.values_mut() {
            deps.sort();
        }

        let entry_rows = r.find_by_eq("collection_entries", "collection_id", id)?;
        let mut entries: Vec<CollectionEntry> = entry_rows
            .iter()
            .map(|row| {
                let document_id = cell_string(row, "document_id");
                let depends_on = deps_by_doc.remove(&document_id).unwrap_or_default();
                CollectionEntry {
                    document_id,
                    position: cell_i32(row, "position", 0),
                    parent_document_id: cell_opt_string(row, "parent_document_id"),
                    depends_on,
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then_with(|| a.document_id.cmp(&b.document_id))
        });

        let (dependency_order, dependency_cycle_members) = dependency_order(&entries);
        Ok(Some(CollectionDetail {
            collection,
            entries,
            dependency_order,
            dependency_cycle_members,
        }))
    }

    /// Update a collection's name, description, metadata and/or entries.
    pub fn update_collection(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<Option<&str>>,
        metadata_json: Option<&str>,
        entries: Option<&[CollectionEntry]>,
    ) -> Result<CollectionDetail> {
        let current = self
            .get_collection(id)?
            .ok_or_else(|| AppError::not_found(format!("collection not found: {id}")))?;
        let next_name = name.unwrap_or(&current.collection.name).trim();
        if next_name.is_empty() {
            return Err(AppError::config("collection name must be non-empty"));
        }
        let next_metadata = metadata_json.unwrap_or(&current.collection.metadata_json);
        validate_metadata(next_metadata)?;

        let next_description: Option<&str> = match description {
            Some(value) => value,
            None => current.collection.description.as_deref(),
        };
        let now_s = format_db_timestamp(chrono::Utc::now());

        self.with_tx(|tx| {
            if let Some(entries) = entries {
                validate_entries(tx, entries)?;
            }
            let rec = RecBuilder::new()
                .id(id)
                .str("name", next_name)
                .opt_str("description", next_description)
                .str("metadata_json", next_metadata)
                .str("created_at", &current.collection.created_at)
                .str("updated_at", &now_s)
                .build();
            tx.upsert("collections", rec)?;
            if let Some(entries) = entries {
                replace_entries(tx, id, entries)?;
            }
            Ok(())
        })?;

        self.get_collection(id)?
            .ok_or_else(|| AppError::not_found(format!("collection not found after update: {id}")))
    }
}

fn normalize_ts(raw: &str) -> Result<String> {
    let ts = parse_flexible_timestamp(raw)
        .ok_or_else(|| AppError::config(format!("invalid collection timestamp: {raw}")))?;
    Ok(format_db_timestamp(ts))
}

fn validate_collection(collection: &Collection, entries: &[CollectionEntry]) -> Result<()> {
    if collection.id.trim().is_empty() || collection.name.trim().is_empty() {
        return Err(AppError::config("collection id and name must be non-empty"));
    }
    validate_metadata(&collection.metadata_json)?;
    let _ = entries;
    Ok(())
}

fn validate_metadata(raw: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|_| ())
        .map_err(|e| AppError::config(format!("metadata_json is not valid JSON: {e}")))
}

fn validate_entries(r: &impl LinReader, entries: &[CollectionEntry]) -> Result<()> {
    let ids: HashSet<&str> = entries.iter().map(|e| e.document_id.trim()).collect();
    if ids.len() != entries.len() || ids.contains("") {
        return Err(AppError::config(
            "collection document ids must be non-empty and unique",
        ));
    }
    for entry in entries {
        if r.get_by_id("documents", &entry.document_id)?.is_none() {
            return Err(AppError::not_found(format!(
                "document not found: {}",
                entry.document_id
            )));
        }
        if let Some(parent) = entry.parent_document_id.as_deref() {
            if parent == entry.document_id || !ids.contains(parent) {
                return Err(AppError::config(format!(
                    "parent_document_id for {} must name another collection member",
                    entry.document_id
                )));
            }
        }
        let mut seen = HashSet::new();
        for dep in &entry.depends_on {
            if dep == &entry.document_id || !ids.contains(dep.as_str()) || !seen.insert(dep) {
                return Err(AppError::config(format!(
                    "depends_on for {} must contain unique other collection members",
                    entry.document_id
                )));
            }
        }
    }
    for entry in entries {
        let mut cursor = entry.parent_document_id.as_deref();
        let mut visited = HashSet::new();
        while let Some(parent) = cursor {
            if !visited.insert(parent) || parent == entry.document_id {
                return Err(AppError::config(format!(
                    "outline parent cycle includes {}",
                    entry.document_id
                )));
            }
            cursor = entries
                .iter()
                .find(|candidate| candidate.document_id == parent)
                .and_then(|candidate| candidate.parent_document_id.as_deref());
        }
    }
    Ok(())
}

fn dependency_order(entries: &[CollectionEntry]) -> (Vec<String>, Vec<String>) {
    let mut indegree: HashMap<&str, usize> = entries
        .iter()
        .map(|entry| (entry.document_id.as_str(), entry.depends_on.len()))
        .collect();
    let mut ordered = Vec::with_capacity(entries.len());
    let mut emitted = HashSet::new();
    loop {
        let next = entries.iter().find(|entry| {
            !emitted.contains(entry.document_id.as_str())
                && indegree
                    .get(entry.document_id.as_str())
                    .copied()
                    .unwrap_or(0)
                    == 0
        });
        let Some(next) = next else { break };
        emitted.insert(next.document_id.as_str());
        ordered.push(next.document_id.clone());
        for candidate in entries {
            if candidate
                .depends_on
                .iter()
                .any(|id| id == &next.document_id)
            {
                if let Some(value) = indegree.get_mut(candidate.document_id.as_str()) {
                    *value = value.saturating_sub(1);
                }
            }
        }
    }
    let cycle_members = entries
        .iter()
        .filter(|entry| !emitted.contains(entry.document_id.as_str()))
        .map(|entry| entry.document_id.clone())
        .collect::<Vec<_>>();
    if cycle_members.is_empty() {
        (ordered, cycle_members)
    } else {
        (Vec::new(), cycle_members)
    }
}

fn replace_entries(
    w: &mut impl LinWriter,
    collection_id: &str,
    entries: &[CollectionEntry],
) -> Result<()> {
    w.delete_where(
        "collection_dependencies",
        lin::query::pred::eq("collection_id", collection_id),
    )?;
    w.delete_where(
        "collection_entries",
        lin::query::pred::eq("collection_id", collection_id),
    )?;
    for (position, entry) in entries.iter().enumerate() {
        let entry_id = composite_id_2(collection_id, &entry.document_id);
        let rec = RecBuilder::new()
            .id(&entry_id)
            .str("collection_id", collection_id)
            .str("document_id", &entry.document_id)
            .i64("position", position as i64)
            .opt_str("parent_document_id", entry.parent_document_id.as_deref())
            .build();
        w.upsert("collection_entries", rec)?;
        for dependency in &entry.depends_on {
            let dep_id = composite_id_3(collection_id, &entry.document_id, dependency);
            let dep_rec = RecBuilder::new()
                .id(&dep_id)
                .str("collection_id", collection_id)
                .str("document_id", &entry.document_id)
                .str("depends_on_document_id", dependency)
                .build();
            w.upsert("collection_dependencies", dep_rec)?;
        }
    }
    Ok(())
}

fn row_to_collection(row: &lin::Row) -> Collection {
    Collection {
        id: cell_string(row, "id"),
        name: cell_string(row, "name"),
        description: cell_opt_string(row, "description"),
        metadata_json: cell_string(row, "metadata_json"),
        created_at: cell_string(row, "created_at"),
        updated_at: cell_string(row, "updated_at"),
    }
}

#[cfg(test)]
mod tests;
