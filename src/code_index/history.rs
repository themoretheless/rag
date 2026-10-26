//! Immutable source/index revisions with content-addressed source storage.
use super::*;
use duckdb::Connection;
use rmcp::schemars;
use std::collections::{BTreeMap, BTreeSet};

type SourceRow = (String, String, String, i64, Option<String>, bool);
#[derive(Debug, Clone, Serialize)]
pub(super) struct File {
    pub(super) id: String,
    pub(super) path: String,
    pub(super) project: Option<String>,
    pub(super) revision: i64,
    pub(super) hash: String,
    #[serde(skip)]
    pub(super) source: String,
    #[serde(skip)]
    pub(super) analysis: Option<String>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct HistoryQuery {
    pub limit: Option<usize>,
    pub before_revision: Option<i64>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffQuery {
    pub before: i64,
    pub after: Option<i64>,
    pub project: Option<String>,
    pub limit: Option<usize>,
}
pub(super) fn publish(
    conn: &Connection,
    docs: &[SourceRow],
    analyses: &[(String, String)],
    force_full: bool,
) -> Result<(i64, bool, rag_mcp_core::IndexResolutionStats)> {
    let payloads: HashMap<_, _> = analyses
        .iter()
        .map(|(id, p)| (id.as_str(), p.as_str()))
        .collect();
    let mut files: Vec<_> = docs
        .iter()
        .map(|(id, path, source, rev, project, _)| File {
            id: id.clone(),
            path: path.clone(),
            project: project.clone(),
            revision: *rev,
            hash: content_hash(source),
            source: source.clone(),
            analysis: payloads.get(id.as_str()).map(|p| (*p).into()),
        })
        .collect();
    files.sort_by(|a, b| a.id.cmp(&b.id));
    let source_file_ids: BTreeSet<_> = docs
        .iter()
        .filter(|(_, _, _, _, _, source_file)| *source_file)
        .map(|(id, _, _, _, _, _)| id.as_str())
        .collect();
    let fingerprint = content_hash(&serde_json::to_string(&(
        INDEX_FORMAT_VERSION,
        &files,
        &source_file_ids,
    ))?);
    let current: Option<(i64, String)> = conn
        .query_row(
            "SELECT id,fingerprint FROM code_index_revisions ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let unchanged = current
        .as_ref()
        .is_some_and(|(_, previous)| previous == &fingerprint);
    if unchanged && !force_full {
        return Ok((
            current.as_ref().unwrap().0,
            false,
            resolution_cache::noop_stats(),
        ));
    }
    // A return to earlier content is a new chronological revision, not a time rewind.
    let revision = current.as_ref().map_or(1, |v| v.0 + 1);
    let mut indexed = HashMap::new();
    let mut symbols = Vec::new();
    for file in &files {
        if let Some(payload) = &file.analysis {
            let doc: IndexedDocument = serde_json::from_str(payload)?;
            symbols.extend(doc.symbols.clone());
            indexed.insert(file.id.clone(), doc);
        }
    }
    let configured = rust_cfg::apply(&mut indexed);
    symbols = indexed.values().flat_map(|d| d.symbols.clone()).collect();
    for file in &mut files {
        if let Some(doc) = indexed.get(&file.id) {
            file.analysis = Some(serde_json::to_string(doc)?);
        }
    }
    let previous = if let Some((id, _)) = &current {
        Some(frozen_graph(conn, *id)?)
    } else {
        None
    };
    let mut resolved = resolution_cache::resolve(
        &symbols,
        &indexed,
        previous.as_ref().and_then(|v| v.get("resolution_cache")),
        force_full,
    )?;
    if unchanged {
        let mut old = previous.as_ref().unwrap()["references"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        old.sort_by_key(serde_json::Value::to_string);
        let mut full: Vec<_> = resolved
            .edges
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<_, _>>()?;
        full.sort_by_key(serde_json::Value::to_string);
        if old != full {
            return Err(AppError::Invalid("full binding recomputation differs from immutable revision; cache equivalence failed".into()));
        }
        resolved.stats.equivalent_to_existing = Some(true);
        return Ok((current.as_ref().unwrap().0, false, resolved.stats));
    }
    let references = resolved.edges;
    let calls: Vec<_> = references.iter().filter(|r| r.reference.is_call).collect();
    let targets: Vec<_> = files
        .iter()
        .filter(|f| source_file_ids.contains(f.id.as_str()) && !indexable(&f.path))
        .map(|f| {
            resource_targets::ResourceTarget::frozen(
                &f.id,
                &f.path,
                f.project.clone(),
                f.revision,
                f.hash.clone(),
            )
        })
        .collect();
    let graph = serde_json::json!({"client_endpoints_format":1,"translation_keys_format":1,"translation_keys":translation_keys::snapshot(&indexed),"powershell_syntax_format":1,"perl_syntax_format":1,"csharp_syntax_format":1,"ruby_syntax_format":1,"inline_style_resources_format":1,"responsive_resources_format":1,"file_resource_targets":targets,"file_resources":file_resources::edges_with_targets(&indexed,&targets),"file_imports":file_imports::edges(&indexed),"references":references,"calls":calls,"inheritance":inheritance::edges(&symbols,&indexed),"overrides":inheritance::overrides(&symbols,&indexed),"entrypoints":analysis::entrypoints(&symbols,&indexed),"sql":sql::snapshot(&indexed),"rust_build_contexts":configured.contexts,"format_version":INDEX_FORMAT_VERSION});
    let mut graph = graph;
    graph["resolution_cache"] = resolved.cache;
    conn.execute(
        "INSERT INTO code_index_revisions(id,fingerprint,graph_json) VALUES (?,?,?)",
        params![revision, fingerprint, serde_json::to_string(&graph)?],
    )?;
    for file in &files {
        conn.execute(
            "INSERT INTO code_index_source_blobs VALUES (?,?) ON CONFLICT DO NOTHING",
            params![file.hash, file.source],
        )?;
        conn.execute(
            "INSERT INTO code_index_revision_files VALUES (?,?,?,?,?,?,?)",
            params![
                revision,
                file.id,
                file.path,
                file.project,
                file.revision,
                file.hash,
                file.analysis
            ],
        )?;
    }
    if let Some((previous, _)) = current {
        let old = files_at(conn, previous)?;
        let incoming: HashMap<_, _> = files.iter().map(|f| (f.id.as_str(), f)).collect();
        for previous_file in &old {
            match incoming.get(previous_file.id.as_str()) {
                None => {
                    drift::observe(
                        conn,
                        previous,
                        previous_file,
                        "removed_from_index",
                        None,
                        "stored_sync",
                    )?;
                }
                Some(next) if previous_file.hash != next.hash => {
                    drift::observe(
                        conn,
                        previous,
                        previous_file,
                        "modified",
                        Some(&next.hash),
                        "stored_sync",
                    )?;
                }
                Some(next)
                    if previous_file.path != next.path || previous_file.project != next.project =>
                {
                    drift::observe(
                        conn,
                        previous,
                        previous_file,
                        "metadata_changed",
                        Some(&next.hash),
                        "stored_sync",
                    )?;
                }
                _ => {}
            }
        }
        let old_ids: HashSet<_> = old.iter().map(|f| f.id.as_str()).collect();
        for next in &files {
            if !old_ids.contains(next.id.as_str()) {
                drift::observe(
                    conn,
                    previous,
                    next,
                    "added_to_index",
                    Some(&next.hash),
                    "stored_sync",
                )?;
            }
        }
    }
    Ok((revision, true, resolved.stats))
}
fn files_at(conn: &Connection, rev: i64) -> Result<Vec<File>> {
    files(conn, rev, None)
}
pub(super) fn files(conn: &Connection, rev: i64, project: Option<&str>) -> Result<Vec<File>> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM code_index_revisions WHERE id=?",
        [rev],
        |r| r.get(0),
    )?;
    if exists != 1 {
        return Err(AppError::not_found(format!("unknown index revision {rev}")));
    }
    let mut stmt=conn.prepare("SELECT f.document_id,f.path,f.project,f.document_revision,f.content_hash,b.content,f.analysis_json FROM code_index_revision_files f LEFT JOIN code_index_source_blobs b ON b.content_hash=f.content_hash WHERE f.index_revision=? AND (? IS NULL OR f.project=?) ORDER BY f.document_id")?;
    let rows = stmt.query_map(params![rev, project, project], |r| {
        Ok(File {
            id: r.get(0)?,
            path: r.get(1)?,
            project: r.get(2)?,
            revision: r.get(3)?,
            hash: r.get(4)?,
            source: r.get(5)?,
            analysis: r.get(6)?,
        })
    })?;
    let files = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    if files.iter().any(|f| content_hash(&f.source) != f.hash) {
        return Err(AppError::Db(
            "immutable source blob checksum mismatch".into(),
        ));
    }
    Ok(files)
}
fn symbol_key(s: &Symbol) -> String {
    let base = format!("{}::{}::{}", s.document_id, s.qualified_name, s.kind);
    s.build_context.as_ref().map_or_else(
        || base.clone(),
        |context| format!("{base}::build:{context}"),
    )
}
fn symbols(files: &[File]) -> Result<BTreeMap<String, Vec<serde_json::Value>>> {
    let mut result: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for file in files {
        if let Some(payload) = &file.analysis {
            let doc: IndexedDocument = serde_json::from_str(payload)?;
            for s in &doc.symbols {
                let body = file
                    .source
                    .get(s.start_byte..s.end_byte)
                    .ok_or_else(|| AppError::config("invalid immutable symbol span"))?;
                result.entry(symbol_key(s)).or_default().push(
                    serde_json::json!({"signature":s.signature,"body_hash":content_hash(body)}),
                );
            }
        }
    }
    for values in result.values_mut() {
        values.sort_by_key(serde_json::Value::to_string);
    }
    Ok(result)
}
fn edges(conn: &Connection, rev: i64, files: &[File]) -> Result<BTreeSet<String>> {
    let graph: String = conn.query_row(
        "SELECT graph_json FROM code_index_revisions WHERE id=?",
        [rev],
        |r| r.get(0),
    )?;
    let graph: serde_json::Value = serde_json::from_str(&graph)?;
    let mut keys = HashMap::new();
    let ids: HashSet<_> = files.iter().map(|f| f.id.as_str()).collect();
    for file in files {
        if let Some(payload) = &file.analysis {
            let doc: IndexedDocument = serde_json::from_str(payload)?;
            for s in doc.symbols {
                keys.insert(s.id.clone(), symbol_key(&s));
            }
        }
    }
    let mut out = BTreeSet::new();
    for call in graph["calls"].as_array().into_iter().flatten() {
        let reference = &call["reference"];
        if !reference["document_id"]
            .as_str()
            .is_some_and(|id| ids.contains(id))
        {
            continue;
        }
        let caller = reference["caller"]
            .as_str()
            .and_then(|s| keys.get(s))
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{}::<module>",
                    reference["document_id"].as_str().unwrap_or("")
                )
            });
        for target in call["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str())
            .filter_map(|s| keys.get(s))
        {
            out.insert(format!("{caller} -> {target}"));
        }
    }
    Ok(out)
}
fn frozen_graph(conn: &Connection, rev: i64) -> Result<serde_json::Value> {
    let raw: String = conn.query_row(
        "SELECT graph_json FROM code_index_revisions WHERE id=?",
        [rev],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&raw)?)
}
fn schema_changes(
    before: &serde_json::Value,
    after: &serde_json::Value,
    project: Option<&str>,
    limit: usize,
) -> serde_json::Value {
    if !before["sql"]["tables"].is_array() || !after["sql"]["tables"].is_array() {
        return serde_json::json!({"available":false,"reason":"historical_schema_snapshot_missing","replay_performed":false});
    }
    let expressions_available = before["format_version"].as_u64().is_some_and(|v| v >= 19)
        && after["format_version"].as_u64().is_some_and(|v| v >= 19);
    let constraints_available = before["format_version"].as_u64().is_some_and(|v| v >= 20)
        && after["format_version"].as_u64().is_some_and(|v| v >= 20);
    let tables = |graph: &serde_json::Value| -> BTreeMap<String, serde_json::Value> {
        graph["sql"]["tables"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| project.is_none_or(|p| t["project"] == p))
            .map(|t| {
                (
                    serde_json::to_string(&(t["project"].clone(), t["name"].clone())).unwrap(),
                    {
                        let mut table = t.clone();
                        if !expressions_available {
                            if let Some(columns) = table["columns"].as_object_mut() {
                                for column in columns.values_mut() {
                                    if let Some(fields) = column.as_object_mut() {
                                        for key in [
                                            "default_expression",
                                            "generated_expression",
                                            "generation_clause",
                                        ] {
                                            fields.remove(key);
                                        }
                                    }
                                }
                            }
                        }
                        if !constraints_available {
                            table.as_object_mut().unwrap().remove("constraints");
                            if let Some(columns) = table["columns"].as_object_mut() {
                                for column in columns.values_mut() {
                                    if let Some(fields) = column.as_object_mut() {
                                        fields.remove("constraints");
                                    }
                                }
                            }
                        }
                        table
                    },
                )
            })
            .collect()
    };
    let old = tables(before);
    let new = tables(after);
    let keys: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    let mut items = Vec::new();
    for key in keys {
        let a = old.get(key);
        let b = new.get(key);
        if a.map(|t| (&t["columns"], &t["complete"], &t["constraints"]))
            == b.map(|t| (&t["columns"], &t["complete"], &t["constraints"]))
        {
            continue;
        }
        let table = b.or(a).unwrap();
        let ca = a.and_then(|t| t["columns"].as_object());
        let cb = b.and_then(|t| t["columns"].as_object());
        let columns: BTreeSet<_> = ca
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(cb.into_iter().flat_map(|m| m.keys()))
            .collect();
        let mut changes = Vec::new();
        for column in columns {
            let x = ca.and_then(|m| m.get(column));
            let y = cb.and_then(|m| m.get(column));
            if x != y {
                let bounded = |value: Option<&serde_json::Value>| {
                    value.map(|v| {
                        let mut v = v.clone();
                        if let Some(values) = v["constraints"].as_array().cloned() {
                            v["constraints_total"] = values.len().into();
                            v["constraints_truncated"] = (values.len() > limit).into();
                            v["constraints"] = serde_json::json!(values
                                .into_iter()
                                .take(limit)
                                .collect::<Vec<_>>());
                        }
                        v
                    })
                };
                changes.push(serde_json::json!({"column":column,"kind":if x.is_none(){"added"}else if y.is_none(){"removed"}else{"changed"},"before":bounded(x),"after":bounded(y),"constraints_truncated":x.into_iter().chain(y).any(|v|v["constraints"].as_array().is_some_and(|a|a.len()>limit))}));
            }
        }
        let evidence = |table: Option<&serde_json::Value>| {
            table
                .and_then(|t| t["evidence"].as_array())
                .cloned()
                .unwrap_or_default()
        };
        let old_evidence = evidence(a);
        let new_evidence = evidence(b);
        let constraints = |table: Option<&serde_json::Value>| {
            table
                .and_then(|t| t["constraints"].as_array())
                .cloned()
                .unwrap_or_default()
        };
        let old_constraints = constraints(a);
        let new_constraints = constraints(b);
        let keyed = |values: &Vec<serde_json::Value>| {
            values
                .iter()
                .map(|v| (v.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let old_keys = keyed(&old_constraints);
        let new_keys = keyed(&new_constraints);
        let added_constraints: Vec<_> = new_keys
            .iter()
            .filter(|(k, _)| !old_keys.contains_key(*k))
            .map(|(_, v)| v)
            .collect();
        let removed_constraints: Vec<_> = old_keys
            .iter()
            .filter(|(k, _)| !new_keys.contains_key(*k))
            .map(|(_, v)| v)
            .collect();
        items.push(serde_json::json!({"project":table["project"],"table":table["name"],"kind":if a.is_none(){"added"}else if b.is_none(){"removed"}else{"changed"},"constraints_added":added_constraints.iter().take(limit).collect::<Vec<_>>(),"constraints_added_total":added_constraints.len(),"constraints_removed":removed_constraints.iter().take(limit).collect::<Vec<_>>(),"constraints_removed_total":removed_constraints.len(),"before_constraints":old_constraints.iter().take(limit).collect::<Vec<_>>(),"after_constraints":new_constraints.iter().take(limit).collect::<Vec<_>>(),"constraints_truncated":old_constraints.len()>limit||new_constraints.len()>limit||added_constraints.len()>limit||removed_constraints.len()>limit||changes.iter().any(|c|c["constraints_truncated"]==true),"column_changes":changes.iter().take(limit).collect::<Vec<_>>(),"column_changes_total":changes.len(),"columns_truncated":changes.len()>limit,"before_complete":a.map(|t|&t["complete"]),"after_complete":b.map(|t|&t["complete"]),"before_evidence":old_evidence.iter().take(limit).collect::<Vec<_>>(),"after_evidence":new_evidence.iter().take(limit).collect::<Vec<_>>(),"evidence_truncated":old_evidence.len()>limit||new_evidence.len()>limit}));
    }
    let diagnostics = |graph: &serde_json::Value| {
        graph["sql"]["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| project.is_none_or(|p| d["evidence"]["project"] == p))
            .cloned()
            .collect::<Vec<_>>()
    };
    let old_diagnostics = diagnostics(before);
    let new_diagnostics = diagnostics(after);
    serde_json::json!({"available":true,"items":items.iter().take(limit).collect::<Vec<_>>(),"total":items.len(),"truncated":items.len()>limit || items.iter().any(|v|v["columns_truncated"]==true||v["evidence_truncated"]==true||v["constraints_truncated"]==true)||old_diagnostics.len()>limit||new_diagnostics.len()>limit,"scope":"derived_table_columns_and_completeness","expression_changes_available":expressions_available,"constraint_changes_available":constraints_available,"expression_comparison":"canonical_sql_ast_text_not_semantic_equivalence","before_diagnostics":old_diagnostics.iter().take(limit).collect::<Vec<_>>(),"after_diagnostics":new_diagnostics.iter().take(limit).collect::<Vec<_>>(),"before_diagnostics_total":old_diagnostics.len(),"after_diagnostics_total":new_diagnostics.len(),"replay_performed":false,"live_database_verified":false})
}
fn translation_comparison(
    before: &serde_json::Value,
    after: &serde_json::Value,
    before_files: &[File],
    after_files: &[File],
    limit: usize,
) -> (HashSet<String>, serde_json::Value) {
    let available = before["translation_keys_format"] == 1 && after["translation_keys_format"] == 1;
    if !available {
        return (
            HashSet::new(),
            serde_json::json!({"available":false,"reason":"legacy_missing_translation_key_capture","absence_proven":false}),
        );
    }
    let all_before: HashSet<_> = before_files.iter().map(|f| f.id.as_str()).collect();
    let all_after: HashSet<_> = after_files.iter().map(|f| f.id.as_str()).collect();
    let qualified = |g: &serde_json::Value| {
        g["translation_keys"]["coverage"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["complete"] == true)
            .filter_map(|c| c["document_id"].as_str().map(str::to_owned))
            .collect::<HashSet<_>>()
    };
    let before_ok = qualified(before);
    let after_ok = qualified(after);
    let relevant: BTreeSet<_> = before_files
        .iter()
        .chain(after_files)
        .filter(|f| translation_keys::supported(&f.path))
        .map(|f| f.id.as_str())
        .collect();
    let mut eligible = HashSet::new();
    let mut excluded = Vec::new();
    for id in relevant {
        if (!all_before.contains(id) || before_ok.contains(id))
            && (!all_after.contains(id) || after_ok.contains(id))
        {
            eligible.insert(id.to_string());
        } else {
            excluded.push(id);
        }
    }
    let report = serde_json::json!({"available":true,"eligible_documents":eligible.len(),"excluded_documents":excluded.iter().take(limit).collect::<Vec<_>>(),"excluded_documents_total":excluded.len(),"excluded_documents_truncated":excluded.len()>limit,"complete_for_supported_documents":excluded.is_empty(),"comparison":"source_owner_key_syntax_kind_and_occurrence_count_ignore_position_only_changes","absence_proven":false});
    (eligible, report)
}
struct RelationCoverage {
    client_endpoints: bool,
    translation_documents: HashSet<String>,
    overrides: bool,
    file_imports: bool,
    file_resources: bool,
    resource_targets: bool,
    responsive: bool,
    css_urls: bool,
    inline_styles: bool,
    ruby: bool,
    csharp: bool,
    perl: bool,
    powershell: bool,
}
fn relations(
    graph: &serde_json::Value,
    files: &[File],
    coverage: &RelationCoverage,
) -> Result<BTreeSet<String>> {
    let mut keys = HashMap::new();
    let ids: HashSet<_> = files.iter().map(|f| f.id.as_str()).collect();
    for file in files {
        if let Some(payload) = &file.analysis {
            let doc: IndexedDocument = serde_json::from_str(payload)?;
            for symbol in doc.symbols {
                keys.insert(symbol.id.clone(), symbol_key(&symbol));
            }
        }
    }
    let mut out = BTreeSet::new();
    let mut key_counts = BTreeMap::<String, usize>::new();
    for occurrence in graph["translation_keys"]["items"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(document) = occurrence["document_id"]
            .as_str()
            .filter(|id| ids.contains(id) && coverage.translation_documents.contains(*id))
        else {
            continue;
        };
        let owner = occurrence["owner"].as_str().and_then(|id| keys.get(id));
        let key = serde_json::to_string(
            &serde_json::json!({"kind":"uses_key","source_document":document,"project":occurrence["project"],"owner":owner,"target_key":occurrence["key"],"usage_kind":occurrence["usage_kind"],"syntax":occurrence["syntax"],"runtime_binding_proven":false}),
        )?;
        *key_counts.entry(key).or_default() += 1;
    }
    for (key, count) in key_counts {
        let mut value: serde_json::Value = serde_json::from_str(&key)?;
        value["occurrences"] = count.into();
        out.insert(serde_json::to_string(&value)?);
    }
    if coverage.file_imports {
        for e in graph["file_imports"].as_array().into_iter().flatten() {
            if e["kind"]
                .as_str()
                .is_some_and(|s| s.starts_with("powershell_"))
                && !coverage.powershell
            {
                continue;
            }
            if e["kind"].as_str().is_some_and(|s| s.starts_with("perl_")) && !coverage.perl {
                continue;
            }
            if e["kind"].as_str().is_some_and(|s| s.starts_with("ruby_")) && !coverage.ruby {
                continue;
            }
            let Some(source) = e["document_id"].as_str().filter(|id| ids.contains(id)) else {
                continue;
            };
            for target in e["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .filter(|id| ids.contains(id))
            {
                out.insert(serde_json::to_string(&serde_json::json!({"kind":"file_import_candidate","source":source,"target":target,"module":e["module"],"import_kind":e["kind"],"resolution":e["resolution"]}))?);
            }
        }
    }
    if coverage.file_resources {
        let parsed_ids: HashSet<_> = files
            .iter()
            .filter(|f| f.analysis.is_some())
            .map(|f| f.id.as_str())
            .collect();
        for e in graph["file_resources"].as_array().into_iter().flatten() {
            if e["inline"].is_object() && !coverage.inline_styles {
                continue;
            }
            if e["choice"].is_object() && !coverage.responsive {
                continue;
            }
            if e["reference_kind"] == "css:url" && !coverage.css_urls {
                continue;
            }
            let Some(source) = e["document_id"].as_str().filter(|id| ids.contains(id)) else {
                continue;
            };
            for target in e["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .filter(|id| ids.contains(id))
            {
                if !coverage.resource_targets && !parsed_ids.contains(target) {
                    continue;
                }
                let mut key = serde_json::json!({"kind":"file_resource_candidate","source":source,"target":target,"url":e["target"],"reference_kind":e["reference_kind"],"resolution":e["resolution"]});
                if e["choice"].is_object() {
                    let c = &e["choice"];
                    key["choice"] = serde_json::json!({"syntax":c["syntax"],"descriptors":c["descriptors"],"descriptors_hash":c["descriptors_hash"],"descriptors_truncated":c["descriptors_truncated"],"type_hint":c["type_hint"],"descriptor_validation":c["descriptor_validation"]});
                }
                if e["inline"].is_object() {
                    key["inline"] = serde_json::json!({"property":e["inline"]["property"],"property_hash":e["inline"]["property_hash"],"property_truncated":e["inline"]["property_truncated"],"css_reference_kind":e["inline"]["css_reference_kind"]});
                }
                out.insert(serde_json::to_string(&key)?);
            }
        }
    }
    for edge in graph["references"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["reference"]["is_call"] == false)
    {
        let r = &edge["reference"];
        if !r["document_id"].as_str().is_some_and(|id| ids.contains(id)) {
            continue;
        }
        let caller = r["caller"]
            .as_str()
            .and_then(|id| keys.get(id))
            .cloned()
            .unwrap_or_else(|| format!("{}::<module>", r["document_id"].as_str().unwrap_or("")));
        for target in edge["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter_map(|id| keys.get(id))
        {
            out.insert(serde_json::to_string(&serde_json::json!({"kind":"non_call_reference","source":caller,"target":target,"resolution":edge["resolution"]}))?);
        }
    }
    for edge in graph["inheritance"].as_array().into_iter().flatten() {
        if edge["relation"] == "powershell_base_type_candidate" && !coverage.powershell {
            continue;
        }
        if edge["relation"] == "perl_parent_package_candidate" && !coverage.perl {
            continue;
        }
        if edge["relation"] == "csharp_base_type_candidate" && !coverage.csharp {
            continue;
        }
        if edge["relation"] == "ruby_superclass_candidate" && !coverage.ruby {
            continue;
        }
        let Some(derived) = edge["derived"].as_str().and_then(|id| keys.get(id)) else {
            continue;
        };
        for target in edge["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter_map(|id| keys.get(id))
        {
            out.insert(serde_json::to_string(&serde_json::json!({"kind":"inheritance","source":derived,"target":target,"relation":edge["relation"],"resolution":edge["resolution"],"hierarchy_status":edge["hierarchy_status"]}))?);
        }
    }
    for edge in graph["overrides"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|_| coverage.overrides)
    {
        let Some(derived) = edge["derived"].as_str().and_then(|id| keys.get(id)) else {
            continue;
        };
        for target in edge["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter_map(|id| keys.get(id))
        {
            out.insert(serde_json::to_string(&serde_json::json!({"kind":"method_override_candidate","source":derived,"target":target,"resolution":edge["resolution"],"semantic_override_proven":false}))?);
        }
    }
    for entry in graph["entrypoints"].as_array().into_iter().flatten() {
        if entry["kind"] == "http_route_candidate" && entry["symbol_id"].is_null() {
            if let Some(document) = entry["document_id"].as_str().filter(|id| ids.contains(id)) {
                out.insert(serde_json::to_string(&serde_json::json!({
                    "kind":"unbound_route_registration", "source_document":document,
                    "route":entry["route"], "method":entry["method"],
                    "handler_expression":entry["handler_expression"], "resolution":entry["resolution"]
                }))?);
            }
        }
        if entry["kind"] == "http_endpoint_reference" {
            if entry["reference_kind"]
                .as_str()
                .is_some_and(|k| k.starts_with("js:jquery:") || k.starts_with("js:axios:"))
                && !coverage.client_endpoints
            {
                continue;
            }
            if let Some(document) = entry["document_id"].as_str().filter(|id| ids.contains(id)) {
                out.insert(serde_json::to_string(&serde_json::json!({
                    "kind":"endpoint_reference", "source_document":document,
                    "target":entry["route"], "reference_kind":entry["reference_kind"],
                    "resolution":entry["resolution"], "request_method":entry["request_method"]
                }))?);
            }
        }
        if let Some(symbol) = entry["symbol_id"].as_str().and_then(|id| keys.get(id)) {
            out.insert(serde_json::to_string(&serde_json::json!({"kind":"entrypoint","source":symbol,"entrypoint_kind":entry["kind"],"route":entry["route"],"method":entry["method"],"resolution":entry["resolution"]}))?);
        }
    }
    Ok(out)
}
impl Store {
    pub fn code_index_history(&self, q: &HistoryQuery) -> Result<serde_json::Value> {
        let conn = self.lock()?;
        let limit = q.limit.unwrap_or(20).clamp(1, 100);
        let mut stmt=conn.prepare("SELECT r.id,CAST(r.created_at AS VARCHAR),r.fingerprint,(SELECT COUNT(*) FROM code_index_revision_files f WHERE f.index_revision=r.id) FROM code_index_revisions r WHERE (? IS NULL OR r.id < ?) ORDER BY r.id DESC LIMIT ?")?;
        let rows=stmt.query_map(params![q.before_revision,q.before_revision,(limit+1) as i64],|r|Ok(serde_json::json!({"index_rev":r.get::<_,i64>(0)?,"created_at":r.get::<_,String>(1)?,"fingerprint":r.get::<_,String>(2)?,"file_count":r.get::<_,i64>(3)?})))?;
        let mut items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let more = items.len() > limit;
        items.truncate(limit);
        Ok(
            serde_json::json!({"items":items,"has_more":more,"next_before_revision":if more{items.last().and_then(|v|v["index_rev"].as_i64())}else{None},"immutable":true}),
        )
    }
    pub fn code_index_diff(&self, q: &DiffQuery) -> Result<serde_json::Value> {
        let conn = self.lock()?;
        let after = match q.after {
            Some(id) => id,
            None => conn
                .query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| {
                    r.get::<_, Option<i64>>(0)
                })?
                .ok_or_else(|| AppError::not_found("index has no revisions"))?,
        };
        let before_files = files(&conn, q.before, q.project.as_deref())?;
        let after_files = files(&conn, after, q.project.as_deref())?;
        let before: BTreeMap<_, _> = before_files.iter().map(|f| (f.id.as_str(), f)).collect();
        let next: BTreeMap<_, _> = after_files.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut changes = Vec::new();
        for (id, file) in &before {
            match next.get(id) {
                None => changes.push(serde_json::json!({"kind":"removed","document":file})),
                Some(new)
                    if file.hash != new.hash
                        || file.path != new.path
                        || file.project != new.project
                        || file.revision != new.revision =>
                {
                    changes.push(serde_json::json!({"kind":"changed","before":file,"after":new}))
                }
                _ => {}
            }
        }
        for (id, file) in &next {
            if !before.contains_key(id) {
                changes.push(serde_json::json!({"kind":"added","document":file}));
            }
        }
        let before_symbols = symbols(&before_files)?;
        let after_symbols = symbols(&after_files)?;
        let symbol_keys: BTreeSet<_> = before_symbols.keys().chain(after_symbols.keys()).collect();
        let mut symbol_changes = Vec::new();
        for key in symbol_keys {
            if before_symbols.get(key) != after_symbols.get(key) {
                symbol_changes.push(serde_json::json!({"symbol_key":key,"kind":if !before_symbols.contains_key(key){"added"}else if !after_symbols.contains_key(key){"removed"}else{"changed"}}));
            }
        }
        let old_edges = edges(&conn, q.before, &before_files)?;
        let new_edges = edges(&conn, after, &after_files)?;
        let added: Vec<_> = new_edges.difference(&old_edges).collect();
        let removed: Vec<_> = old_edges.difference(&new_edges).collect();
        let old_graph = frozen_graph(&conn, q.before)?;
        let new_graph = frozen_graph(&conn, after)?;
        let relations_available = [&old_graph, &new_graph].iter().all(|g| {
            ["references", "inheritance", "entrypoints"]
                .iter()
                .all(|k| g[*k].is_array())
        });
        let method_relations_available =
            old_graph["overrides"].is_array() && new_graph["overrides"].is_array();
        let file_import_relations_available =
            old_graph["file_imports"].is_array() && new_graph["file_imports"].is_array();
        let file_resource_relations_available =
            old_graph["file_resources"].is_array() && new_graph["file_resources"].is_array();
        let resource_target_relations_available = old_graph["file_resource_targets"].is_array()
            && new_graph["file_resource_targets"].is_array();
        let responsive_resource_relations_available = old_graph["responsive_resources_format"] == 1
            && new_graph["responsive_resources_format"] == 1;
        let css_url_classification_comparable =
            old_graph["responsive_resources_format"] == new_graph["responsive_resources_format"];
        let inline_style_relations_available = old_graph["inline_style_resources_format"] == 1
            && new_graph["inline_style_resources_format"] == 1;
        let ruby_relations_available =
            old_graph["ruby_syntax_format"] == 1 && new_graph["ruby_syntax_format"] == 1;
        let csharp_relations_available =
            old_graph["csharp_syntax_format"] == 1 && new_graph["csharp_syntax_format"] == 1;
        let perl_relations_available =
            old_graph["perl_syntax_format"] == 1 && new_graph["perl_syntax_format"] == 1;
        let powershell_relations_available = old_graph["powershell_syntax_format"] == 1
            && new_graph["powershell_syntax_format"] == 1;
        let limit = q.limit.unwrap_or(100).clamp(1, 500);
        let (translation_documents, translation_key_comparison) =
            translation_comparison(&old_graph, &new_graph, &before_files, &after_files, limit);
        let translation_key_relations_available = translation_key_comparison["available"] == true;
        let client_endpoint_relations_available =
            old_graph["client_endpoints_format"] == 1 && new_graph["client_endpoints_format"] == 1;
        let relation_coverage = RelationCoverage {
            client_endpoints: client_endpoint_relations_available,
            translation_documents,
            overrides: method_relations_available,
            file_imports: file_import_relations_available,
            file_resources: file_resource_relations_available,
            resource_targets: resource_target_relations_available,
            responsive: responsive_resource_relations_available,
            css_urls: css_url_classification_comparable,
            inline_styles: inline_style_relations_available,
            ruby: ruby_relations_available,
            csharp: csharp_relations_available,
            perl: perl_relations_available,
            powershell: powershell_relations_available,
        };
        let old_relations = if relations_available {
            relations(&old_graph, &before_files, &relation_coverage)?
        } else {
            BTreeSet::new()
        };
        let new_relations = if relations_available {
            relations(&new_graph, &after_files, &relation_coverage)?
        } else {
            BTreeSet::new()
        };
        let relation_added: Vec<_> = new_relations
            .difference(&old_relations)
            .map(|s| serde_json::from_str::<serde_json::Value>(s))
            .collect::<std::result::Result<_, _>>()?;
        let relation_removed: Vec<_> = old_relations
            .difference(&new_relations)
            .map(|s| serde_json::from_str::<serde_json::Value>(s))
            .collect::<std::result::Result<_, _>>()?;
        let schema_diff = schema_changes(&old_graph, &new_graph, q.project.as_deref(), limit);
        let mut report = serde_json::json!({"before_format_version":old_graph["format_version"],"after_format_version":new_graph["format_version"],"analysis_format_changed":old_graph["format_version"]!=new_graph["format_version"],"schema_diff":schema_diff,"relations_available":relations_available,"client_endpoint_relations_available":client_endpoint_relations_available,"method_relations_available":method_relations_available,"file_import_relations_available":file_import_relations_available,"file_resource_relations_available":file_resource_relations_available,"resource_target_relations_available":resource_target_relations_available,"responsive_resource_relations_available":responsive_resource_relations_available,"inline_style_relations_available":inline_style_relations_available,"ruby_relations_available":ruby_relations_available,"csharp_relations_available":csharp_relations_available,"perl_relations_available":perl_relations_available,"powershell_relations_available":powershell_relations_available,"css_url_classification_comparable":css_url_classification_comparable,"translation_key_relations_available":translation_key_relations_available,"translation_key_comparison":translation_key_comparison,"relations_scope":"uses_key_candidates_non_call_candidates_file_import_candidates_file_resource_candidates_inheritance_method_override_candidates_entrypoints_unbound_routes_and_endpoint_references","relations_added":relation_added.iter().take(limit).collect::<Vec<_>>(),"relations_added_total":relation_added.len(),"relations_removed":relation_removed.iter().take(limit).collect::<Vec<_>>(),"relations_removed_total":relation_removed.len(),"relations_truncated":relation_added.len()>limit||relation_removed.len()>limit,"before":q.before,"after":after,"project":q.project,"files":changes.iter().take(limit).collect::<Vec<_>>(),"file_changes_total":changes.len(),"symbols":symbol_changes.iter().take(limit).collect::<Vec<_>>(),"symbol_changes_total":symbol_changes.len(),"edges_added":added.iter().take(limit).collect::<Vec<_>>(),"edges_added_total":added.len(),"edges_removed":removed.iter().take(limit).collect::<Vec<_>>(),"edges_removed_total":removed.len(),"truncated":changes.len()>limit||symbol_changes.len()>limit||added.len()>limit||removed.len()>limit||relation_added.len()>limit||relation_removed.len()>limit||schema_diff["truncated"]==true,"edge_semantics":"resolved_static_candidate_links","bodies_included":false,"immutable":true});
        let contexts = |graph: &serde_json::Value| {
            graph["rust_build_contexts"].as_array().map(|items| {
                items
                    .iter()
                    .filter(|item| q.project.as_ref().is_none_or(|p| item["project"] == *p))
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        let old_contexts = contexts(&old_graph);
        let new_contexts = contexts(&new_graph);
        report["rust_build_contexts"] = serde_json::json!({"available":old_contexts.is_some()&&new_contexts.is_some(),"before":old_contexts.as_ref().map(|v|v.iter().take(50).collect::<Vec<_>>()),"after":new_contexts.as_ref().map(|v|v.iter().take(50).collect::<Vec<_>>()),"before_total":old_contexts.as_ref().map(Vec::len),"after_total":new_contexts.as_ref().map(Vec::len),"truncated":old_contexts.as_ref().is_some_and(|v|v.len()>50)||new_contexts.as_ref().is_some_and(|v|v.len()>50),"changed":if old_contexts.is_some()&&new_contexts.is_some(){Some(old_contexts!=new_contexts)}else{None}});
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn doc(id: &str, path: &str, source: &str) -> Document {
        Document {
            id: id.into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            content: source.into(),
            title: path.into(),
            ..Default::default()
        }
    }
    #[test]
    fn frozen_method_relations_diff_and_legacy_missing_coverage_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("overrides.duckdb")).unwrap();
        let mut d = doc(
            "a",
            "/p/models.py",
            "class Base:\n    def work(self): pass\nclass Child(Base):\n    def work(self): pass\n",
        );
        store.upsert_document(&d).unwrap();
        store.index_code().unwrap();
        assert_eq!(
            frozen_graph(&store.lock().unwrap(), 1).unwrap()["overrides"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        d.content = "class Base:\n    def work(self): pass\nclass Child(Base): pass\n".into();
        store.upsert_document(&d).unwrap();
        store.index_code().unwrap();
        let q = DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let result = store.code_index_diff(&q).unwrap();
        assert_eq!(result["method_relations_available"], true);
        assert!(result["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "method_override_candidate"));
        let mut graph = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        graph.as_object_mut().unwrap().remove("overrides");
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
                [graph.to_string()],
            )
            .unwrap();
        let legacy = store.code_index_diff(&q).unwrap();
        assert_eq!(legacy["method_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "method_override_candidate"));
    }
    #[test]
    fn translation_key_history_ignores_offsets_but_keeps_counts_and_legacy_gaps() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("key-history.duckdb")).unwrap();
        let mut view = doc("view", "/p/view.html", "%{key} %{key}");
        view.wing = Some("p".into());
        let mut code = doc(
            "code",
            "/p/render.php",
            "<?php function render(){Translate('key');}",
        );
        code.wing = Some("p".into());
        for d in [&view, &code] {
            store.upsert_document(d).unwrap();
        }
        store.index_code().unwrap();
        let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        view.content = format!("\n\n{}", view.content);
        code.content = format!("\n{}", code.content);
        for d in [&view, &code] {
            store.upsert_document(d).unwrap();
        }
        store.index_code().unwrap();
        let diff = store
            .code_index_diff(&DiffQuery {
                before: 1,
                after: Some(2),
                project: Some("p".into()),
                limit: None,
            })
            .unwrap();
        assert_eq!(diff["translation_key_relations_available"], true);
        assert!(
            !diff["relations_added"]
                .as_array()
                .unwrap()
                .iter()
                .chain(diff["relations_removed"].as_array().unwrap())
                .any(|r| r["kind"] == "uses_key"),
            "{diff}"
        );
        view.content = "%{key}".into();
        store.upsert_document(&view).unwrap();
        store.index_code().unwrap();
        let q = DiffQuery {
            before: 2,
            after: Some(3),
            project: Some("p".into()),
            limit: None,
        };
        let changed = store.code_index_diff(&q).unwrap();
        assert!(changed["relations_added"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "uses_key" && r["occurrences"] == 1));
        assert!(changed["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "uses_key" && r["occurrences"] == 2));
        let mut legacy = frozen_graph(&store.lock().unwrap(), 2).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("translation_keys_format");
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revisions SET graph_json=? WHERE id=2",
                [legacy.to_string()],
            )
            .unwrap();
        let unknown = store.code_index_diff(&q).unwrap();
        assert_eq!(unknown["translation_key_relations_available"], false);
        assert!(!unknown["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "uses_key"));
        assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), before);
    }
    #[test]
    fn limited_translation_captures_are_not_compared_as_complete_negative_facts() {
        let file = File {
            id: "a".into(),
            path: "/p/view.html".into(),
            project: Some("p".into()),
            revision: 1,
            hash: "hash".into(),
            source: String::new(),
            analysis: None,
        };
        let before = serde_json::json!({"translation_keys_format":1,"translation_keys":{"coverage":[{"document_id":"a","complete":true}]}});
        let after = serde_json::json!({"translation_keys_format":1,"translation_keys":{"coverage":[{"document_id":"a","complete":false}]}});
        let (eligible, report) = translation_comparison(
            &before,
            &after,
            std::slice::from_ref(&file),
            std::slice::from_ref(&file),
            10,
        );
        assert!(eligible.is_empty());
        assert_eq!(report["excluded_documents_total"], 1);
        assert_eq!(report["complete_for_supported_documents"], false);
        let (eligible, _) =
            translation_comparison(&before, &before, std::slice::from_ref(&file), &[], 10);
        assert!(eligible.contains("a"));
    }
    #[test]
    fn frozen_schema_and_relations_diff_ignore_line_moves_and_project_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("semantic.duckdb")).unwrap();
        let mut migration = doc(
            "ddl",
            "/p/migrations/001.sql",
            "CREATE TABLE users(name TEXT NOT NULL);",
        );
        migration.wing = Some("p".into());
        let mut code = doc("code","/p/models.py","class Base: pass\nclass Other: pass\nclass Child(Base): pass\ndef work(): pass\nregistered = work\ndef test_old(): pass\n");
        code.wing = Some("p".into());
        let mut foreign = doc(
            "foreign",
            "/q/migrations/001.sql",
            "ALTER TABLE unknown ADD COLUMN x INT;",
        );
        foreign.wing = Some("q".into());
        for d in [&migration, &code, &foreign] {
            store.upsert_document(d).unwrap();
        }
        store.index_code().unwrap();
        let frozen = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        assert!(frozen["sql"]["tables"].is_array());
        migration.content = "CREATE TABLE users(name TEXT NOT NULL, age INT NOT NULL);".into();
        code.content="class Base: pass\nclass Other: pass\nclass Child(Other): pass\ndef work(): pass\nregistered = None\ndef plain(): pass\n".into();
        store.upsert_document(&migration).unwrap();
        store.upsert_document(&code).unwrap();
        store.index_code().unwrap();
        let q = DiffQuery {
            before: 1,
            after: Some(2),
            project: Some("p".into()),
            limit: None,
        };
        let result = store.code_index_diff(&q).unwrap();
        assert_eq!(result["schema_diff"]["total"], 1, "{result}");
        assert_eq!(
            result["schema_diff"]["items"][0]["column_changes"][0]["column"],
            "age"
        );
        assert_eq!(
            result["schema_diff"]["after_diagnostics"],
            serde_json::json!([])
        );
        let added = result["relations_added"].as_array().unwrap();
        let removed = result["relations_removed"].as_array().unwrap();
        assert!(added.iter().any(|e| e["kind"] == "inheritance"));
        assert!(removed.iter().any(|e| e["kind"] == "non_call_reference"));
        assert!(removed.iter().any(|e| e["kind"] == "entrypoint"));
        assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), frozen);
        code.content = format!("# moved\n{}", code.content);
        migration.content = format!("-- moved\n{}", migration.content);
        store.upsert_document(&migration).unwrap();
        store.upsert_document(&code).unwrap();
        store.index_code().unwrap();
        let moved = store
            .code_index_diff(&DiffQuery {
                before: 2,
                after: Some(3),
                ..q
            })
            .unwrap();
        assert_eq!(moved["schema_diff"]["total"], 0, "{moved}");
        assert_eq!(moved["relations_added_total"], 0);
        assert_eq!(moved["relations_removed_total"], 0);
    }
    #[test]
    fn schema_diff_captures_expression_only_changes_and_marks_legacy_coverage() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("expressions.duckdb")).unwrap();
        let mut migration = doc(
            "ddl",
            "/p/migrations/001.sql",
            "CREATE TABLE calc(a INT DEFAULT 1, b INT GENERATED ALWAYS AS (a + 1) STORED);",
        );
        store.upsert_document(&migration).unwrap();
        store.index_code().unwrap();
        let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        migration.content =
            "CREATE TABLE calc(a INT DEFAULT 2, b INT GENERATED ALWAYS AS (a + 2) STORED);".into();
        store.upsert_document(&migration).unwrap();
        store.index_code().unwrap();
        let after = frozen_graph(&store.lock().unwrap(), 2).unwrap();
        let diff = store
            .code_index_diff(&DiffQuery {
                before: 1,
                after: Some(2),
                project: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(diff["schema_diff"]["expression_changes_available"], true);
        assert_eq!(
            diff["schema_diff"]["items"][0]["column_changes_total"], 2,
            "{diff}"
        );
        let mut legacy = before.clone();
        legacy["format_version"] = 18.into();
        let compatibility = schema_changes(&legacy, &after, None, 100);
        assert_eq!(compatibility["expression_changes_available"], false);
        assert_eq!(compatibility["total"], 0, "{compatibility}");
        assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), before);
    }

    #[test]
    fn frozen_constraints_diff_and_legacy_coverage_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("constraints.duckdb")).unwrap();
        let mut d = doc(
            "ddl",
            "/p/migrations/001.sql",
            "CREATE TABLE x(a INT CONSTRAINT positive CHECK(a>0), CONSTRAINT uq UNIQUE(a));",
        );
        store.upsert_document(&d).unwrap();
        store.index_code().unwrap();
        let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        d.content="CREATE TABLE x(a INT CONSTRAINT positive CHECK(a>1), CONSTRAINT uq UNIQUE(a), CONSTRAINT ceiling CHECK(a<100));".into();
        store.upsert_document(&d).unwrap();
        store.index_code().unwrap();
        let after = frozen_graph(&store.lock().unwrap(), 2).unwrap();
        let result = store
            .code_index_diff(&DiffQuery {
                before: 1,
                after: Some(2),
                project: None,
                limit: Some(1),
            })
            .unwrap();
        assert_eq!(result["schema_diff"]["constraint_changes_available"], true);
        assert_eq!(result["schema_diff"]["total"], 1);
        assert_eq!(result["schema_diff"]["items"][0]["column_changes_total"], 1);
        assert_eq!(
            result["schema_diff"]["items"][0]["constraints_truncated"],
            true
        );
        assert_eq!(result["truncated"], true);
        let mut legacy = before;
        legacy["format_version"] = 19.into();
        let compatibility = schema_changes(&legacy, &after, None, 100);
        assert_eq!(compatibility["constraint_changes_available"], false);
        assert_eq!(compatibility["total"], 0, "{compatibility}");
    }

    #[test]
    fn missing_historical_schema_does_not_invent_a_replayed_snapshot() {
        let result = schema_changes(
            &serde_json::json!({}),
            &serde_json::json!({"sql":{"tables":[]}}),
            None,
            1,
        );
        assert_eq!(result["available"], false);
        assert_eq!(result["replay_performed"], false);
    }

    #[test]
    fn immutable_history_reuses_noop_and_content_blobs_and_diffs_semantic_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.duckdb");
        let store = Store::open(&path).unwrap();
        store
            .upsert_document(&doc(
                "a",
                "/p/a.rs",
                "fn main() { foo(); let callback = foo; } fn foo() {}",
            ))
            .unwrap();
        store
            .upsert_document(&doc("text", "/p/readme.md", "unchanged text"))
            .unwrap();
        let first = store.index_code().unwrap();
        assert_eq!(first.index_revision, Some(1));
        assert!(first.revision_created);
        assert_eq!(store.code_snapshot().unwrap().index_revision, Some(1));
        let graph: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let frozen: serde_json::Value = serde_json::from_str(&graph).unwrap();
        assert_eq!(frozen["format_version"], INDEX_FORMAT_VERSION);
        assert!(frozen["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reference"]["name"] == "foo"
                && r["reference"]["is_call"] == false
                && r["resolution"] == "resolved_static"));
        let warm = store.index_code().unwrap();
        assert_eq!(warm.index_revision, Some(1));
        assert!(!warm.revision_created);
        let mut a = store.get_document("a").unwrap().unwrap();
        a.content="// moved definitions\nfn main() { bar(); let secret_value=1; } fn foo() {} fn bar() {}".into();
        store.upsert_document(&a).unwrap();
        assert_eq!(store.index_code().unwrap().index_revision, Some(2));
        let diff = store
            .code_index_diff(&DiffQuery {
                before: 1,
                after: None,
                project: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(diff["file_changes_total"], 1);
        assert_eq!(diff["symbol_changes_total"], 2);
        assert_eq!(diff["edges_added_total"], 1);
        assert_eq!(diff["edges_removed_total"], 1);
        assert!(!diff.to_string().contains("secret_value"));
        assert_eq!(
            store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT graph_json FROM code_index_revisions WHERE id=1",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            graph
        );
        let blobs: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM code_index_source_blobs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(blobs, 3);
        let page = store
            .code_index_history(&HistoryQuery {
                limit: Some(1),
                before_revision: None,
            })
            .unwrap();
        assert_eq!(page["items"][0]["index_rev"], 2);
        assert_eq!(page["has_more"], true);
        assert_eq!(
            store
                .code_index_history(&HistoryQuery {
                    limit: Some(1),
                    before_revision: Some(2)
                })
                .unwrap()["items"][0]["index_rev"],
            1
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .code_index_diff(&DiffQuery {
                    before: 1,
                    after: Some(2),
                    project: None,
                    limit: Some(1)
                })
                .unwrap()["truncated"],
            true
        );
    }
    #[test]
    fn deleting_every_document_makes_a_new_chronological_empty_revision() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.duckdb")).unwrap();
        assert_eq!(store.index_code().unwrap().index_revision, Some(1));
        store
            .upsert_document(&doc("a", "/p/a.rs", "fn a() {}"))
            .unwrap();
        assert_eq!(store.index_code().unwrap().index_revision, Some(2));
        store.delete_document("a").unwrap();
        assert_eq!(store.index_code().unwrap().index_revision, Some(3));
        let diff = store
            .code_index_diff(&DiffQuery {
                before: 2,
                after: Some(3),
                project: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(diff["files"][0]["kind"], "removed");
        assert_eq!(store.index_code().unwrap().index_revision, Some(3));
        assert!(store
            .code_index_diff(&DiffQuery {
                before: 999,
                after: None,
                project: None,
                limit: None
            })
            .is_err());
    }
    #[test]
    fn late_publication_failure_rolls_back_current_cache_and_revision_together() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.duckdb")).unwrap();
        store
            .upsert_document(&doc("a", "/p/a.rs", "fn old() {}"))
            .unwrap();
        store.index_code().unwrap();
        let old_payload: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.lock().unwrap().execute_batch("ALTER TABLE code_index_source_blobs RENAME TO original_blobs; CREATE TABLE code_index_source_blobs(content_hash VARCHAR PRIMARY KEY,content VARCHAR NOT NULL CHECK(content NOT LIKE '%new%')); INSERT INTO code_index_source_blobs SELECT * FROM original_blobs; DROP TABLE original_blobs;").unwrap();
        let mut a = store.get_document("a").unwrap().unwrap();
        a.content = "fn new() {}".into();
        store.upsert_document(&a).unwrap();
        assert!(store.index_code().is_err());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT payload FROM code_index_documents WHERE document_id='a'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            old_payload
        );
        let history = store.code_index_history(&HistoryQuery::default()).unwrap();
        assert_eq!(history["items"].as_array().unwrap().len(), 1);
        assert_eq!(store.code_snapshot().unwrap().stale_documents, vec!["a"]);
    }
    #[test]
    fn client_endpoints_history_is_frozen_and_guards_legacy_capture() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("clients.duckdb")).unwrap();
        store
            .upsert_document(&doc(
                "client",
                "/p/client.js",
                "$.ajax({url:'/target',method:'GET'});",
            ))
            .unwrap();
        store.index_code().unwrap();
        store
            .upsert_document(&doc(
                "client",
                "/p/client.js",
                "$.ajax({url:'/target',method:'POST'});",
            ))
            .unwrap();
        store.index_code().unwrap();
        let q = DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let diff = store.code_index_diff(&q).unwrap();
        assert_eq!(diff["client_endpoint_relations_available"], true);
        assert!(diff["relations_added"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reference_kind"] == "js:jquery:ajax" && r["request_method"] == "POST"));
        assert!(diff["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["request_method"] == "GET"));
        let mut graph = frozen_graph(&store.lock().unwrap(), 1).unwrap();
        graph
            .as_object_mut()
            .unwrap()
            .remove("client_endpoints_format");
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
                [graph.to_string()],
            )
            .unwrap();
        let legacy = store.code_index_diff(&q).unwrap();
        assert_eq!(legacy["client_endpoint_relations_available"], false);
        assert!(!legacy["relations_added"]
            .as_array()
            .unwrap()
            .iter()
            .chain(legacy["relations_removed"].as_array().unwrap())
            .any(|r| r["reference_kind"] == "js:jquery:ajax"));
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    #[test]
    fn missing_or_corrupt_history_blobs_are_errors_not_silent_file_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.duckdb")).unwrap();
        store
            .upsert_document(&crate::models::Document {
                id: "a".into(),
                uri: "file:///p/a.rs".into(),
                source_file: Some("/p/a.rs".into()),
                content: "fn a() {}".into(),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let q = DiffQuery {
            before: 1,
            after: Some(1),
            project: None,
            limit: None,
        };
        store
            .lock()
            .unwrap()
            .execute("UPDATE code_index_source_blobs SET content='corrupt'", [])
            .unwrap();
        assert!(store.code_index_diff(&q).is_err());
        store
            .lock()
            .unwrap()
            .execute("DELETE FROM code_index_source_blobs", [])
            .unwrap();
        assert!(store.code_index_diff(&q).is_err());
    }
}
