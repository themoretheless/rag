//! Exact/regex multi-string search and evidence-backed duplicate families over stored text.
use super::*;
mod family_groups;
pub use family_groups::Grouping as DuplicateGrouping;
mod literal_mentions;
mod primary_comparison;
mod source_bytes;
pub use literal_mentions::Options as LiteralMentionOptions;
pub use primary_comparison::Options as PrimaryComparisonOptions;
use rmcp::schemars;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CorpusScope {
    pub project: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MultiSearch {
    pub queries: Vec<String>,
    pub project: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<usize>,
    /// all (default) requires every query in one document, any returns the union.
    pub match_mode: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_insensitive: bool,
    /// Bounded exact stored-text path mention evidence; not runtime path binding.
    pub literal_mentions: Option<LiteralMentionOptions>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct DuplicateQuery {
    /// similarity_components (default), exact_and_name_families (including dissimilar siblings), or source_byte_copies (qualified captured source digests).
    pub grouping: Option<DuplicateGrouping>,
    pub project: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<usize>,
    pub min_similarity: Option<f64>,
    pub max_comparisons: Option<usize>,
    pub max_members: Option<usize>,
    /// Bounded normalized-line comparisons for returned members against their primary.
    pub primary_comparison: Option<PrimaryComparisonOptions>,
    /// Bounded exact stored-text path mention evidence; not runtime path binding.
    pub literal_mentions: Option<LiteralMentionOptions>,
}
#[derive(Debug, Serialize)]
struct File {
    document_id: String,
    path: String,
    project: Option<String>,
    revision: i64,
    content_hash: String,
    source_encoding: Option<String>,
    source_encoding_basis: Option<String>,
    source_bytes_hash: Option<String>,
    source_byte_capture: source_bytes::Evidence,
    #[serde(skip)]
    content: String,
}
fn in_scope(path: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|p| Path::new(path).starts_with(Path::new(p)))
}
fn file_family(path: &str) -> String {
    let file = Path::new(path);
    let mut stem = file
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    stem = stem
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', '_', ' '])
        .into();
    for suffix in ["copy", "backup", "old"] {
        if stem.ends_with(suffix) {
            stem.truncate(stem.len() - suffix.len());
            stem = stem.trim_end_matches(['-', '_', ' ']).into();
            break;
        }
    }
    format!(
        "{}.{}",
        stem,
        file.extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    )
}
fn shingles(source: &str, words: &regex::Regex) -> (BTreeSet<String>, bool) {
    let tokens: Vec<_> = words
        .find_iter(source)
        .take(20003)
        .map(|m| m.as_str())
        .collect();
    let truncated = tokens.len() > 20000;
    let mut out = BTreeSet::new();
    if tokens.len() < 3 {
        out.insert(source.to_string());
    } else {
        for parts in tokens[..tokens.len().min(20000)].windows(3) {
            out.insert(parts.join(" "));
        }
    }
    (out, truncated)
}
fn root(parent: &mut [usize], i: usize) -> usize {
    if parent[i] != i {
        parent[i] = root(parent, parent[i]);
    }
    parent[i]
}
// Index roles are observations, never proof that an unreferenced file is unused.

#[derive(Default)]
struct RoleEvidence {
    score: u64,
    kept: Vec<(String, serde_json::Value)>,
    total: usize,
}
fn search_roles(
    snapshot: &CodeSnapshot,
    files: &[File],
) -> HashMap<String, (u64, Vec<serde_json::Value>, usize)> {
    let targets: HashMap<_, _> = files.iter().map(|f| (f.document_id.as_str(), f)).collect();
    let bad: HashSet<_> = snapshot
        .stale_documents
        .iter()
        .chain(&snapshot.unindexed_documents)
        .chain(&snapshot.parse_errors)
        .map(String::as_str)
        .collect();
    let source_valid =
        |id: &str, path: &str, project: &Option<String>, revision: i64, hash: &str| {
            !bad.contains(id)
                && snapshot
                    .document_scopes
                    .get(id)
                    .is_some_and(|s| s.path == path && &s.project == project)
                && snapshot
                    .document_fingerprints
                    .get(id)
                    .is_some_and(|(r, h)| *r == revision && h == hash)
                && targets.get(id).is_none_or(|f| {
                    f.path == path
                        && &f.project == project
                        && f.revision == revision
                        && f.content_hash == hash
                })
        };
    let own: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            targets.get(s.document_id.as_str()).is_some_and(|f| {
                s.path == f.path
                    && s.content_hash == f.content_hash
                    && s.revision == f.revision
                    && source_valid(
                        &s.document_id,
                        &f.path,
                        &f.project,
                        f.revision,
                        &f.content_hash,
                    )
            })
        })
        .map(|s| (s.id.as_str(), s.document_id.as_str()))
        .collect();
    let mut roles: HashMap<String, RoleEvidence> = HashMap::new();
    let mut add = |doc: &str, score: u64, evidence: serde_json::Value| {
        let role = roles.entry(doc.into()).or_default();
        role.score = role.score.max(score);
        role.total += 1;
        let key = evidence.to_string();
        let at = role
            .kept
            .partition_point(|(key_before, _)| key_before <= &key);
        if at < 100 {
            role.kept.insert(at, (key, evidence));
            if role.kept.len() > 100 {
                role.kept.pop();
            }
        }
    };
    for entry in &snapshot.entrypoints {
        if let Some(doc) = entry.symbol_id.as_deref().and_then(|id| own.get(id)) {
            let file = targets[doc];
            add(
                doc,
                if entry.kind.contains("main") || entry.kind == "http_route_candidate" {
                    4
                } else {
                    1
                },
                serde_json::json!({"kind":entry.kind,"path":entry.path,"line":entry.line,"symbol_id":entry.symbol_id,"document_id":doc,"project":file.project,"revision":file.revision,"content_hash":file.content_hash,"resolution":entry.resolution,"provenance":"derived"}),
            );
        }
    }
    for edge in &snapshot.bound_references {
        let r = &edge.reference;
        let Some(scope) = snapshot.document_scopes.get(&r.document_id) else {
            continue;
        };
        if !source_valid(
            &r.document_id,
            &r.path,
            &scope.project,
            r.revision,
            &r.content_hash,
        ) {
            continue;
        }
        let docs: BTreeSet<_> = edge
            .candidates
            .iter()
            .filter_map(|id| own.get(id.as_str()).copied())
            .filter(|doc| {
                *doc != r.document_id
                    && targets.get(doc).is_some_and(|f| f.project == scope.project)
            })
            .collect();
        for doc in docs {
            add(
                doc,
                if edge.resolution == "resolved_static" {
                    3
                } else {
                    2
                },
                serde_json::json!({"kind":if r.is_call{"incoming_call_candidate"}else{"incoming_reference_candidate"},"document_id":r.document_id,"path":r.path,"project":scope.project,"revision":r.revision,"content_hash":r.content_hash,"line":r.line,"column":r.column,"byte_start":r.start_byte,"resolution":edge.resolution,"provenance":"derived","runtime_usage_proven":false}),
            );
        }
    }
    let attachments: HashMap<_, _> = snapshot
        .file_resource_targets
        .iter()
        .map(|t| (t.document_id.as_str(), t))
        .collect();
    let target_valid = |id: &str, project: &Option<String>| {
        !bad.contains(id)
            && targets.get(id).is_some_and(|f| {
                &f.project == project
                    && (source_valid(id, &f.path, &f.project, f.revision, &f.content_hash)
                        || attachments.get(id).is_some_and(|t| {
                            t.fresh()
                                && t.path == f.path
                                && t.project == f.project
                                && t.revision == f.revision
                                && t.content_hash.as_deref() == Some(f.content_hash.as_str())
                        }))
            })
    };
    for edge in &snapshot.file_imports {
        if !source_valid(
            &edge.document_id,
            &edge.path,
            &edge.project,
            edge.revision,
            &edge.content_hash,
        ) {
            continue;
        }
        for target in edge
            .candidates
            .iter()
            .filter(|id| *id != &edge.document_id && target_valid(id, &edge.project))
        {
            add(
                target,
                if edge.resolution == "ambiguous" { 2 } else { 3 },
                serde_json::json!({"kind":"incoming_file_import_candidate","import_kind":edge.kind,"module":edge.module,"document_id":edge.document_id,"path":edge.path,"project":edge.project,"revision":edge.revision,"content_hash":edge.content_hash,"line":edge.line,"byte_start":edge.start_byte,"resolution":edge.resolution,"resolution_basis":edge.resolution_basis,"provenance":"derived","runtime_usage_proven":false}),
            );
        }
    }
    for edge in &snapshot.file_resources {
        if !source_valid(
            &edge.document_id,
            &edge.path,
            &edge.project,
            edge.revision,
            &edge.content_hash,
        ) {
            continue;
        }
        for target in edge
            .candidates
            .iter()
            .filter(|id| *id != &edge.document_id && target_valid(id, &edge.project))
        {
            add(
                target,
                if edge.resolution == "ambiguous" { 2 } else { 3 },
                serde_json::json!({"kind":"incoming_file_resource_candidate","reference_kind":edge.reference_kind,"target":edge.target,"choice":edge.choice,"inline":edge.inline,"document_id":edge.document_id,"path":edge.path,"project":edge.project,"revision":edge.revision,"content_hash":edge.content_hash,"line":edge.line,"byte_start":edge.start_byte,"resolution":edge.resolution,"resolution_basis":edge.resolution_basis,"provenance":"derived","runtime_usage_proven":false}),
            );
        }
    }
    roles
        .into_iter()
        .map(
            |(
                id,
                RoleEvidence {
                    score,
                    kept: evidence,
                    total,
                },
            )| {
                (
                    id,
                    (
                        score,
                        evidence.into_iter().map(|(_, value)| value).collect(),
                        total,
                    ),
                )
            },
        )
        .collect()
}
fn merge_literal_roles(
    roles: &mut HashMap<String, (u64, Vec<serde_json::Value>, usize)>,
    scan: &literal_mentions::Scan,
) {
    for (target, count) in &scan.counts {
        let role = roles.entry(target.clone()).or_default();
        role.0 = role.0.max(2);
        role.2 += count;
    }
    for (target, evidence) in &scan.evidence {
        let role = roles.entry(target.clone()).or_default();

        let key = evidence.to_string();
        let at = role
            .1
            .partition_point(|item| serde_json::to_string(item).unwrap_or_default() <= key);
        if at < 100 {
            role.1.insert(at, evidence.clone());
            if role.1.len() > 100 {
                role.1.pop();
            }
        }
    }
}
fn corpus_files_locked(
    conn: &duckdb::Connection,
    project: Option<&str>,
    prefix: Option<&str>,
) -> Result<Vec<File>> {
    let selected = {
        let mut statement=conn.prepare("SELECT id,COALESCE(source_file,uri) FROM documents WHERE COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated') AND (? IS NULL OR wing=?) ORDER BY 2,id")?;
        let rows = statement.query_map(params![project, project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, path)| in_scope(path, prefix))
            .collect::<Vec<_>>()
    };
    let mut files = Vec::new();
    for batch in selected.chunks(512) {
        let placeholders = crate::db::sql::in_placeholders(batch.len());
        let sql=format!("SELECT id,COALESCE(source_file,uri),wing,COALESCE(revision,1),content,metadata_json FROM documents WHERE id IN ({placeholders}) ORDER BY 2,id");
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(
            duckdb::params_from_iter(batch.iter().map(|(id, _)| id)),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )?;
        let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        for row in rows {
            let (id, path, project, revision, content, metadata) = row;
            if in_scope(&path, prefix) {
                let content = source::read(conn, &content, &metadata)?.into_owned();
                let hash = content_hash(&content);
                let meta = serde_json::from_str::<serde_json::Value>(&metadata).ok();
                let verified = meta.as_ref().filter(|m| {
                    m["source_decoding_version"] == 1
                        && m["source_text_hash"].as_str() == Some(hash.as_str())
                });
                let field = |key: &str| verified.and_then(|m| m[key].as_str()).map(str::to_string);
                files.push(File {
                    document_id: id,
                    path,
                    project,
                    revision,
                    content_hash: hash.clone(),
                    source_encoding: field("source_encoding"),
                    source_encoding_basis: field("source_encoding_basis"),
                    source_bytes_hash: field("source_bytes_hash"),
                    source_byte_capture: source_bytes::Evidence::read(meta.as_ref(), &hash),
                    content,
                });
            }
        }
    }
    Ok(files)
}
impl<'a> CodeIndex<'a> {
    #[cfg(test)]
    fn corpus_files(&self, project: Option<&str>, prefix: Option<&str>) -> Result<Vec<File>> {
        self.store
            .with_conn(|conn| corpus_files_locked(conn, project, prefix))
    }
    pub fn search_multi(&self, q: &MultiSearch) -> Result<serde_json::Value> {
        if q.queries.is_empty()
            || q.queries.len() > 32
            || q.queries
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 4096)
        {
            return Err(AppError::config(
                "search_multi requires 1..32 nonblank queries of at most 4096 bytes",
            ));
        }
        let mode = q.match_mode.as_deref().unwrap_or("all");
        if !matches!(mode, "all" | "any") {
            return Err(AppError::config("match_mode must be all or any"));
        }
        let patterns: Vec<_> = q
            .queries
            .iter()
            .map(|s| {
                let pattern = if q.regex { s.clone() } else { regex::escape(s) };
                let regex = regex::RegexBuilder::new(&pattern)
                    .case_insensitive(q.case_insensitive)
                    .size_limit(2 * 1024 * 1024)
                    .dfa_size_limit(2 * 1024 * 1024)
                    .build()
                    .map_err(|e| AppError::config(format!("invalid search pattern: {e}")))?;
                if regex.is_match("") {
                    return Err(AppError::config(
                        "patterns matching empty text are not supported",
                    ));
                }
                Ok(regex)
            })
            .collect::<Result<_>>()?;
        self.store.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let files =
                corpus_files_locked(&transaction, q.project.as_deref(), q.path_prefix.as_deref())?;
            let limit = q.limit.unwrap_or(50).clamp(1, 500);
            let mut items = Vec::new();
            let mut counts = vec![0usize; patterns.len()];
            let mut matched = 0;
            // Independent query windows: conjunction filtering must not hide a query's hits.
            let mut query_items = vec![Vec::new(); patterns.len()];
            let mut families: BTreeMap<(Option<String>, String), Vec<serde_json::Value>> =
                BTreeMap::new();
            let mut union_total = 0usize;
            for file in &files {
                let per_query:Vec<_>=patterns.iter().enumerate().map(|(i,p)|{
                    let matches:Vec<_>=p.find_iter(&file.content).take(4).collect();if !matches.is_empty(){counts[i]+=1;}
                    let hits:Vec<_>=matches.iter().take(3).map(|m|{
                        let line_start=file.content[..m.start()].rfind('\n').map_or(0,|p|p+1);let line_end=file.content[m.start()..].find('\n').map_or(file.content.len(),|p|m.start()+p);
                        serde_json::json!({"byte_start":m.start(),"byte_end":m.end(),"line":file.content[..m.start()].bytes().filter(|b|*b==b'\n').count()+1,"column":file.content[line_start..m.start()].chars().count()+1,"snippet":file.content[line_start..line_end].chars().take(240).collect::<String>()})
                    }).collect();serde_json::json!({"query_index":i,"hits":hits,"more_occurrences":matches.len()>3})
                }).collect();
                let hits = per_query
                    .iter()
                    .filter(|p| !p["hits"].as_array().unwrap().is_empty())
                    .count();
                if hits > 0 {
                    union_total += 1;
                    let mut retained = false;
                    for (i, found) in per_query.iter().enumerate() {
                        if !found["hits"].as_array().unwrap().is_empty() && query_items[i].len() < limit
                        {
                            query_items[i].push(serde_json::json!({"document":file,"hits":found["hits"],"more_occurrences":found["more_occurrences"],"encoding":"stored_utf8","source_encoding":file.source_encoding,"source_encoding_basis":file.source_encoding_basis,"source_bytes_hash":file.source_bytes_hash}));
                            retained = true;
                        }
                    }
                    if retained {
                        families.entry((file.project.clone(), file_family(&file.path))).or_default()
                            .push(serde_json::json!({"document":file,"query_indices":per_query.iter().filter(|p| !p["hits"].as_array().unwrap().is_empty()).map(|p|p["query_index"].clone()).collect::<Vec<_>>(),"role":"unverified","role_evidence":[]}));
                    }
                }
                if (mode == "all" && hits == patterns.len()) || (mode == "any" && hits > 0) {
                    matched += 1;
                    if items.len() < limit {
                        items.push(serde_json::json!({"document":file,"matches":per_query,"encoding":"stored_utf8"}));
                    }
                }
            }
            let per_query: Vec<_> = query_items.into_iter().enumerate().map(|(i, hits)| {
                serde_json::json!({"query_index":i,"query":q.queries[i],"items":hits,"matching_documents_total":counts[i],"truncated":counts[i]>limit,"occurrences_per_document_limit":3})
            }).collect();
            let snapshot = if union_total > 0 {
                Some(super::snapshot_locked(&transaction, q.project.as_deref())?)
            } else {
                None
            };
            let mut roles = snapshot
                .as_ref()
                .map(|s| search_roles(s, &files))
                .unwrap_or_default();
            let retained: HashSet<_> = families
                .values()
                .flatten()
                .filter_map(|m| m["document"]["document_id"].as_str())
                .collect();
            let literal_targets: Vec<_> = files
                .iter()
                .filter(|f| retained.contains(f.document_id.as_str()))
                .collect();
            let literal = if let Some(snapshot) = &snapshot {
                literal_mentions::scan(
                    &transaction,
                    snapshot,
                    &literal_targets,
                    q.project.as_deref(),
                    q.literal_mentions.as_ref(),
                )?
            } else {
                literal_mentions::Scan::skipped("no_retained_hits")
            };
            merge_literal_roles(&mut roles, &literal);
            let family_total = families.len();
            let mut ranked = Vec::new();
            for ((project, name), mut members) in families {
                let mut best = 0;
                for member in &mut members {
                    let id = member["document"]["document_id"].as_str().unwrap();
                    let (score, evidence, total) = roles.get(id).cloned().unwrap_or_default();
                    best = best.max(score);
                    member["usage_score"] = score.into();
                    member["role"] = match score {
                        4 => "entrypoint_candidate",
                        3 => "referenced_candidate",
                        2 => "ambiguous_reference_candidate",
                        1 => "entrypoint_candidate",
                        _ => "no_observed_usage",
                    }
                    .into();
                    member["role_evidence"] = serde_json::json!(evidence);
                    member["role_evidence_total"] = total.into();
                    member["role_evidence_truncated"] = (total > 100).into();
                }
                members.sort_by(|a, b| {
                    b["usage_score"]
                        .as_u64()
                        .cmp(&a["usage_score"].as_u64())
                        .then_with(|| {
                            b["role_evidence_total"]
                                .as_u64()
                                .cmp(&a["role_evidence_total"].as_u64())
                        })
                        .then_with(|| {
                            a["document"]["path"]
                                .as_str()
                                .cmp(&b["document"]["path"].as_str())
                        })
                });
                ranked.push(serde_json::json!({"project":project,"name":name,"usage_score":best,"members":members.iter().take(limit).collect::<Vec<_>>(),"retained_members_total":members.len(),"truncated":members.len()>limit,"grouping_basis":"normalized_filename_only","duplicate_content_proven":false}));
            }
            ranked.sort_by(|a, b| {
                b["usage_score"]
                    .as_u64()
                    .cmp(&a["usage_score"].as_u64())
                    .then_with(|| a["project"].as_str().cmp(&b["project"].as_str()))
                    .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
            });
            let family_items: Vec<_> = ranked.into_iter().take(limit).collect();
            let role_analysis = snapshot.as_ref().map(|s| serde_json::json!({"index_rev":s.index_revision,"basis":"fresh_static_index_candidates","runtime_activity_verified":false,"stale_documents":s.stale_documents,"unindexed_documents":s.unindexed_documents,"parse_errors":s.parse_errors,"role_contract_version":2,"markup_references_included":true,"file_import_references_included":true,"literal_path_mentions_included":true,"role_source_scope":"fresh_selected_project_index_before_target_path_prefix","observation_consistency":"single_read_transaction"}));
            let result = serde_json::json!({"items":items,"per_query":per_query,"families":family_items,"families_retained_total":family_total,"families_truncated":family_total>limit,"family_coverage":"retained_per_query_windows","family_roles_verified":false,"family_role_analysis":role_analysis,"union_matching_documents_total":union_total,"matching_documents_total":matched,"documents_scanned":files.len(),"truncated":matched>limit,"per_query_document_counts":counts,"match_mode":mode,"regex":q.regex,"case_insensitive":q.case_insensitive,"snapshot":"current_stored_documents","literal_path_analysis":literal.report,"observation_consistency":"single_read_transaction","chunk_boundary_independent":true,"filesystem_read":false});
            transaction.commit()?;
            Ok(result)
        })
    }
    pub fn duplicates(&self, q: &DuplicateQuery) -> Result<serde_json::Value> {
        let family_view = q.grouping == Some(DuplicateGrouping::ExactAndNameFamilies);
        let byte_view = q.grouping == Some(DuplicateGrouping::SourceByteCopies);
        let threshold = q
            .min_similarity
            .unwrap_or(if family_view { 0.5 } else { 0.8 });
        if !threshold.is_finite() || !(0.5..=1.0).contains(&threshold) {
            return Err(AppError::config("min_similarity must be between 0.5 and 1"));
        }
        self.store.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let files =
                corpus_files_locked(&transaction, q.project.as_deref(), q.path_prefix.as_deref())?;
            let snapshot = super::snapshot_locked(&transaction, q.project.as_deref())?;
            let mut roles = search_roles(&snapshot, &files);
            let mut exact: BTreeMap<(Option<String>, &str), Vec<usize>> = BTreeMap::new();
            let mut families: BTreeMap<(Option<String>, String), Vec<usize>> = BTreeMap::new();
            for (i, f) in files.iter().enumerate() {
                exact
                    .entry((f.project.clone(), &f.content_hash))
                    .or_default()
                    .push(i);
                families
                    .entry((f.project.clone(), file_family(&f.path)))
                    .or_default()
                    .push(i);
            }
            let mut parent: Vec<_> = (0..files.len()).collect();
            let mut similarity = Vec::new();
            for indices in exact.values().filter(|v| v.len() > 1) {
                for pair in indices.windows(2) {
                    let a = root(&mut parent, pair[0]);
                    let b = root(&mut parent, pair[1]);
                    parent[b] = a;
                    similarity.push(serde_json::json!({"a":files[pair[0]].document_id,"b":files[pair[1]].document_id,"kind":"exact_stored_text","similarity":1.0}));
                }
            }
            let words =
                regex::Regex::new(r"[\pL\pN_]+").map_err(|e| AppError::config(e.to_string()))?;
            let mut cache = HashMap::new();
            let mut sampled = BTreeSet::new();
            let mut comparisons = 0;
            let cap = q.max_comparisons.unwrap_or(10000).clamp(1, 100000);
            let mut truncated = false;
            if !family_view && !byte_view {
                'families: for indices in families.values().filter(|v| v.len() > 1) {
                    for (a_pos, a) in indices.iter().enumerate() {
                        for b in indices.iter().skip(a_pos + 1) {
                            if files[*a].content_hash == files[*b].content_hash {
                                continue;
                            }
                            if comparisons >= cap {
                                truncated = true;
                                break 'families;
                            }
                            comparisons += 1;
                            for i in [*a, *b] {
                                cache.entry(i).or_insert_with(|| {
                                    let (set, cut) = shingles(&files[i].content, &words);
                                    if cut {
                                        sampled.insert(i);
                                    }
                                    set
                                });
                            }
                            let a_set = &cache[a];
                            let b_set = &cache[b];
                            let intersection = a_set.intersection(b_set).count();
                            let union = a_set.len() + b_set.len() - intersection;
                            let score = if union == 0 {
                                1.0
                            } else {
                                intersection as f64 / union as f64
                            };
                            if score >= threshold {
                                let ra = root(&mut parent, *a);
                                let rb = root(&mut parent, *b);
                                parent[rb] = ra;
                                similarity.push(serde_json::json!({"a":files[*a].document_id,"b":files[*b].document_id,"kind":"name_family_and_token_shingles","similarity":score}));
                            }
                        }
                    }
                }
            }
            let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for i in 0..files.len() {
                let r = root(&mut parent, i);
                groups.entry(r).or_default().push(i);
            }
            let candidates = if byte_view {
                source_bytes::candidates(&files)
            } else if family_view {
                family_groups::candidates(&files)?
            } else {
                groups
                    .into_values()
                    .filter(|v| v.len() > 1)
                    .map(|indices| family_groups::Candidate {
                        indices,
                        kind: "similarity_component",
                        family: None,
                    })
                    .collect()
            };
            let target_indices: BTreeSet<_> = candidates
                .iter()
                .flat_map(|c| c.indices.iter().copied())
                .collect();
            let literal_targets: Vec<_> = target_indices.iter().map(|i| &files[*i]).collect();
            let literal = literal_mentions::scan(
                &transaction,
                &snapshot,
                &literal_targets,
                q.project.as_deref(),
                q.literal_mentions.as_ref(),
            )?;
            merge_literal_roles(&mut roles, &literal);
            let mut results = Vec::new();
            for candidate in &candidates {
                let group = &candidate.indices;
                let mut members = Vec::new();
                for i in group {
                    let file = &files[*i];
                    let (score, evidence, evidence_total) =
                        roles.get(&file.document_id).cloned().unwrap_or_default();
                    members.push(serde_json::json!({"document":file,"usage_score":score,"role":match score{4=>"entrypoint_candidate",3=>"referenced_candidate",2=>"ambiguous_reference_candidate",1=>"public_api_candidate",_=>"no_observed_usage"},"evidence":evidence.iter().take(100).collect::<Vec<_>>(),"evidence_total":evidence_total,"evidence_truncated":evidence_total>evidence.len()}));
                }
                let ids: HashSet<_> = group
                    .iter()
                    .map(|i| files[*i].document_id.as_str())
                    .collect();
                let links: Vec<_> = similarity
                    .iter()
                    .filter(|e| {
                        e["a"].as_str().is_some_and(|id| ids.contains(id))
                            && e["b"].as_str().is_some_and(|id| ids.contains(id))
                    })
                    .collect();
                let member_limit = q.max_members.unwrap_or(100).clamp(1, 500);
                members.sort_by(|a, b| {
                    let a_path = a["document"]["path"].as_str().unwrap_or("");
                    let b_path = b["document"]["path"].as_str().unwrap_or("");
                    b["usage_score"]
                        .as_u64()
                        .cmp(&a["usage_score"].as_u64())
                        .then_with(|| {
                            b["evidence_total"]
                                .as_u64()
                                .cmp(&a["evidence_total"].as_u64())
                        })
                        .then_with(|| a_path.len().cmp(&b_path.len()))
                        .then_with(|| a_path.cmp(b_path))
                        .then_with(|| {
                            a["document"]["document_id"]
                                .as_str()
                                .cmp(&b["document"]["document_id"].as_str())
                        })
                });
                let primary = members[0]["document"]["document_id"].clone();
                let score = members[0]["usage_score"].clone();
                members.truncate(member_limit);
                results.push(serde_json::json!({"kind":candidate.kind,"family_key":candidate.family,"primary_candidate":primary,"primary_is_proven":false,"usage_score":score,"members":members,"members_total":group.len(),"members_truncated":group.len()>member_limit,"similarity_evidence":links.iter().take(1000).collect::<Vec<_>>(),"similarity_evidence_total":links.len(),"similarity_evidence_truncated":links.len()>1000,"deletion_safe":false}));
            }
            results.sort_by(|a, b| {
                let exact_order = if family_view {
                    (a["kind"] != "exact_stored_text").cmp(&(b["kind"] != "exact_stored_text"))
                } else {
                    std::cmp::Ordering::Equal
                };
                exact_order
                    .then_with(|| {
                        b["usage_score"]
                            .as_u64()
                            .cmp(&a["usage_score"].as_u64())
                            .then_with(|| {
                                a["primary_candidate"]
                                    .as_str()
                                    .cmp(&b["primary_candidate"].as_str())
                            })
                    })
                    .then_with(|| a["family_key"].as_str().cmp(&b["family_key"].as_str()))
            });
            let total = results.len();
            let limit = q.limit.unwrap_or(50).clamp(1, 500);
            results.truncate(limit);
            let primary_comparison =
                primary_comparison::enrich(&mut results, &files, q.primary_comparison.as_ref());
            if family_view {
                family_groups::classify(&mut results, threshold);
            }
            let source_byte_identity = source_bytes::enrich(&mut results, &files);
            let result = serde_json::json!({"source_byte_identity":source_byte_identity,"primary_comparison":primary_comparison,"groups":results,"groups_total":total,"documents_checked":files.len(),"truncated":total>limit,"comparison_limit_reached":truncated,"comparisons":comparisons,"sampled_documents":sampled.iter().map(|i|&files[*i].document_id).collect::<Vec<_>>(),"index_rev":snapshot.index_revision,"group_semantics":if byte_view{"captured_source_byte_digest_partitions"}else if family_view{"independent_exact_partitions_and_directory_sensitive_numbered_name_families"}else{"connected_similarity_components"},"grouping":if byte_view{"source_byte_copies"}else if family_view{"exact_and_name_families"}else{"similarity_components"},"grouping_contract_version":1,"pairwise_token_grouping_performed":!family_view && !byte_view,"family_classification_threshold":if family_view{Some(threshold)}else{None},"overlapping_groups_possible":family_view,"exactness":if byte_view{"captured_source_byte_digest_and_count_not_live_filesystem"}else{"stored_utf8_text_not_original_binary"},"role_contract_version":2,"literal_path_analysis":literal.report,"role_order":["usage_score_desc","observed_evidence_count_desc","path_length_asc","path_asc","document_id_asc"],"role_relations":["entrypoints","bound_calls","bound_values_and_types","file_imports","file_resources"],"role_source_scope":"fresh_selected_project_index_before_target_path_prefix","role_observation_consistency":"single_read_transaction","liveness_semantics":"observed_static_evidence_not_runtime_proof","stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors});
            transaction.commit()?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests;
