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
        let placeholders = std::iter::repeat_n("?", batch.len())
            .collect::<Vec<_>>()
            .join(",");
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
impl Store {
    #[cfg(test)]
    fn corpus_files(&self, project: Option<&str>, prefix: Option<&str>) -> Result<Vec<File>> {
        let conn = self.lock()?;
        corpus_files_locked(&conn, project, prefix)
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
        let mut connection = self.lock()?;
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
        let mut connection = self.lock()?;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, content: &str, project: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file://{path}"),
                source_file: Some(path.into()),
                content: content.into(),
                wing: Some(project.into()),
                ..Default::default()
            })
            .unwrap();
    }
    fn search(queries: &[&str]) -> MultiSearch {
        MultiSearch {
            queries: queries.iter().map(|q| q.to_string()).collect(),
            project: None,
            path_prefix: None,
            limit: None,
            match_mode: None,
            regex: false,
            case_insensitive: false,
            literal_mentions: None,
        }
    }
    #[test]
    fn captured_byte_copy_groups_omit_legacy_and_invalidate_changed_text_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("bytes.duckdb")).unwrap();
        for (id, raw) in [
            ("a", b"hello".as_slice()),
            ("b", b"hello".as_slice()),
            ("bom", b"\xef\xbb\xbfhello".as_slice()),
        ] {
            let path = dir.path().join(format!("{id}.txt"));
            std::fs::write(&path, raw).unwrap();
            let extracted = crate::file_ingest::extract_file(&path).unwrap();
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file://{}", path.display()),
                    source_file: Some(path.display().to_string()),
                    content: extracted.text,
                    metadata_json: serde_json::to_string(&extracted.metadata).unwrap(),
                    wing: Some("p".into()),
                    ..Default::default()
                })
                .unwrap();
        }
        put(&store, "legacy", "/p/legacy.txt", "hello", "p");
        let q = DuplicateQuery {
            project: Some("p".into()),
            grouping: Some(DuplicateGrouping::SourceByteCopies),
            literal_mentions: Some(LiteralMentionOptions {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let report = store.duplicates(&q).unwrap();
        assert_eq!(report["groups_total"], 1, "{report}");
        assert_eq!(report["groups"][0]["kind"], "exact_captured_source_bytes");
        assert_eq!(report["groups"][0]["members_total"], 2);
        assert_eq!(
            report["source_byte_identity"]["capture_status_counts"]["legacy_or_missing_capture"],
            1
        );
        let mut doc = store.get_document("b").unwrap().unwrap();
        doc.content = "changed".into();
        store.upsert_document(&doc).unwrap();
        let changed = store.duplicates(&q).unwrap();
        assert_eq!(changed["groups_total"], 0);
        assert_eq!(
            changed["source_byte_identity"]["capture_status_counts"]["source_text_mismatch"],
            1
        );
    }
    #[test]
    fn duplicate_family_inventory_keeps_independent_exact_and_dissimilar_groups() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("families.duckdb")).unwrap();
        for (id, path, body) in [
            ("a", "/p/Files.txt", "PRIVATE_ALPHA"),
            ("b", "/p/Files2.txt", "PRIVATE_ALPHA"),
            ("c", "/p/Files3.txt", "PRIVATE_OMEGA"),
            ("clone", "/p/unrelated.txt", "PRIVATE_ALPHA"),
            ("d", "/p/Other.txt", "PRIVATE_DELTA"),
            ("e", "/p/Other2.txt", "PRIVATE_EPSILON"),
            ("nested", "/p/nested/Other3.txt", "PRIVATE_NESTED"),
            ("case", "/p/other3.txt", "PRIVATE_CASE"),
            ("x", "/p/Exact.txt", "PRIVATE_EXACT"),
            ("y", "/p/Exact2.txt", "PRIVATE_EXACT"),
        ] {
            put(&store, id, path, body, "p");
        }
        put(&store, "foreign", "/p/Other4.txt", "PRIVATE_FOREIGN", "q");
        let query = DuplicateQuery {
            grouping: Some(DuplicateGrouping::ExactAndNameFamilies),
            project: Some("p".into()),
            literal_mentions: Some(LiteralMentionOptions {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let report = store.duplicates(&query).unwrap();
        assert_eq!(report["groups_total"], 4, "{report}");
        assert_eq!(report["comparisons"], 0);
        let groups = report["groups"].as_array().unwrap();
        assert!(groups[..2].iter().all(|g| g["kind"] == "exact_stored_text"));
        let family = groups
            .iter()
            .find(|g| g["family_key"] == "p/Files.txt")
            .unwrap();
        assert_eq!(family["kind"], "family");
        assert_eq!(family["members_total"], 3);
        assert_eq!(family["family_classification"]["complete"], true);
        let different = groups
            .iter()
            .find(|g| g["family_key"] == "p/Other.txt")
            .unwrap();
        assert_eq!(different["kind"], "same_name");
        assert_eq!(different["members_total"], 2);
        assert!(!report.to_string().contains("PRIVATE_"));
        assert_eq!(report, store.duplicates(&query).unwrap());
        let scoped = store
            .duplicates(&DuplicateQuery {
                path_prefix: Some("/p/nested".into()),
                ..query
            })
            .unwrap();
        assert_eq!(scoped["groups_total"], 0);
    }
    #[test]
    fn duplicate_family_limits_do_not_manufacture_same_name_classification() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("limits.duckdb")).unwrap();
        put(&store, "a", "/p/Files.txt", "alpha", "p");
        put(&store, "b", "/p/Files2.txt", "beta", "p");
        let mut q = DuplicateQuery {
            grouping: Some(DuplicateGrouping::ExactAndNameFamilies),
            project: Some("p".into()),
            literal_mentions: Some(LiteralMentionOptions {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        q.max_members = Some(1);
        let r = store.duplicates(&q).unwrap();
        assert_eq!(r["groups"][0]["kind"], "not_checked");
        assert_eq!(r["primary_comparison"]["pairs_requested"], 0);
        q.max_members = None;
        q.primary_comparison = Some(PrimaryComparisonOptions {
            enabled: Some(false),
            ..Default::default()
        });
        let r = store.duplicates(&q).unwrap();
        assert_eq!(r["groups"][0]["kind"], "not_checked");
        q.primary_comparison = Some(PrimaryComparisonOptions {
            max_file_bytes: Some(1),
            ..Default::default()
        });
        let r = store.duplicates(&q).unwrap();
        assert_eq!(r["groups"][0]["kind"], "not_checked");
        q.primary_comparison = None;
        let r = store.duplicates(&q).unwrap();
        assert_eq!(r["groups"][0]["kind"], "same_name");
        q.grouping = None;
        let r = store.duplicates(&q).unwrap();
        assert_eq!(r["groups_total"], 0);
    }
    #[test]
    fn duplicate_primary_comparison_uses_returned_window_and_current_text() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("comparison.duckdb")).unwrap();
        let shared = (0..10)
            .map(|i| format!("shared private content line {i}\n"))
            .collect::<String>();
        put(
            &store,
            "a",
            "/p/sample.txt",
            &format!("{shared}PRIVATE_ALPHA\n"),
            "p",
        );
        put(
            &store,
            "b",
            "/p/sample2.txt",
            &format!("{shared}PRIVATE_BETA\n"),
            "p",
        );
        let mut query = DuplicateQuery {
            project: Some("p".into()),
            min_similarity: Some(0.5),
            literal_mentions: Some(LiteralMentionOptions {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let report = store.duplicates(&query).unwrap();
        assert_eq!(report["groups_total"], 1, "{report}");
        assert_eq!(report["primary_comparison"]["pairs_checked"], 1);
        let group = &report["groups"][0];
        let member = group["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["document"]["document_id"] != group["primary_candidate"])
            .unwrap();
        assert_eq!(member["similarity_to_primary"], 0.833);
        assert_eq!(member["lines_only_here"], 1);
        assert_eq!(member["lines_only_in_primary"], 1);
        assert!(!report.to_string().contains("PRIVATE_"));
        query.max_members = Some(1);
        let limited = store.duplicates(&query).unwrap();
        assert_eq!(limited["primary_comparison"]["pairs_requested"], 0);
        assert_eq!(
            limited["primary_comparison"]["decoded_input_bytes_charged"],
            0
        );
    }
    #[test]
    fn multi_search_exact_conjunction_regex_scope_utf8_and_no_chunks_needed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("search.duckdb")).unwrap();
        put(
            &store,
            "a",
            "/p/a.txt",
            "Привет 🦀\nalpha|beta\nSECOND",
            "p",
        );
        put(&store, "b", "/p/ab/b.txt", "alpha|beta", "q");
        let result = store
            .search_multi(&search(&["alpha|beta", "SECOND"]))
            .unwrap();
        assert_eq!(result["matching_documents_total"], 1);
        assert_eq!(result["items"][0]["matches"][0]["hits"][0]["line"], 2);
        let mut q = search(&["alpha|beta", "SECOND"]);
        q.match_mode = Some("any".into());
        assert_eq!(
            store.search_multi(&q).unwrap()["matching_documents_total"],
            2
        );
        q.project = Some("p".into());
        assert_eq!(store.search_multi(&q).unwrap()["documents_scanned"], 1);
        q = search(&["alpha\\|b.ta", "second"]);
        q.regex = true;
        q.case_insensitive = true;
        assert_eq!(
            store.search_multi(&q).unwrap()["matching_documents_total"],
            1
        );
        q = search(&["🦀\nalpha"]);
        assert_eq!(
            store.search_multi(&q).unwrap()["items"][0]["matches"][0]["hits"][0]["column"],
            8
        );
        q.path_prefix = Some("/p/a".into());
        assert_eq!(store.search_multi(&q).unwrap()["documents_scanned"], 0);
        q.regex = true;
        q.queries = vec!["a*".into()];
        assert!(store.search_multi(&q).is_err());
        q.queries = vec!["(".into()];
        assert!(store.search_multi(&q).is_err());
    }
    #[test]
    fn independent_query_windows_and_families_preserve_scope_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("multi.duckdb")).unwrap();
        put(&store, "z", "/p/Files.php", "alpha alpha alpha alpha", "p");
        put(&store, "y", "/p/Files2.php", "beta", "p");
        put(&store, "x", "/q/Files3.php", "alpha", "q");
        let mut q = search(&["alpha", "beta", "absent"]);
        q.limit = Some(1);
        let result = store.search_multi(&q).unwrap();
        assert_eq!(result["matching_documents_total"], 0);
        assert_eq!(result["union_matching_documents_total"], 3);
        assert_eq!(result["per_query"][0]["matching_documents_total"], 2);
        assert_eq!(result["per_query"][0]["truncated"], true);
        assert_eq!(
            result["per_query"][0]["items"][0]["hits"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(result["per_query"][0]["items"][0]["more_occurrences"], true);
        assert_eq!(
            result["per_query"][1]["items"][0]["document"]["path"],
            "/p/Files2.php"
        );
        assert_eq!(result["per_query"][2]["items"], serde_json::json!([]));
        assert_eq!(result["families"][0]["duplicate_content_proven"], false);
        assert_eq!(result["families"][0]["truncated"], true);
        q.limit = Some(10);
        let result = store.search_multi(&q).unwrap();
        assert_eq!(result["families_retained_total"], 2);
        assert_eq!(
            result["families"][0]["members"].as_array().unwrap().len(),
            2
        );
        assert_eq!(result, store.search_multi(&q).unwrap());
        q.project = Some("q".into());
        let result = store.search_multi(&q).unwrap();
        assert_eq!(result["union_matching_documents_total"], 1);
        assert_eq!(result["per_query"][1]["matching_documents_total"], 0);
        assert_eq!(result["families"][0]["project"], "q");
        assert_eq!(
            result["per_query"][0]["items"][0]["source_encoding"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn source_encoding_labels_require_matching_decoded_source_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("encoding.duckdb")).unwrap();
        put(&store, "doc", "/p/code.py", "needle", "p");
        let mut doc = store.get_document("doc").unwrap().unwrap();
        doc.metadata_json = serde_json::json!({"source_decoding_version":1,"source_encoding":"windows-1251","source_encoding_basis":"heuristic_cyrillic_share","source_text_hash":content_hash("needle"),"source_bytes_hash":"raw"}).to_string();
        store.upsert_document(&doc).unwrap();
        let query = search(&["needle"]);
        let found = store.search_multi(&query).unwrap();
        assert_eq!(
            found["per_query"][0]["items"][0]["source_encoding"],
            "windows-1251"
        );
        doc.content = "needle changed".into();
        store.upsert_document(&doc).unwrap();
        let found = store.search_multi(&query).unwrap();
        assert_eq!(
            found["per_query"][0]["items"][0]["source_encoding"],
            serde_json::Value::Null
        );
        assert_eq!(
            found["per_query"][0]["items"][0]["source_bytes_hash"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn search_family_roles_use_bound_values_and_omit_stale_callers_and_targets() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("roles.duckdb")).unwrap();
        put(
            &store,
            "api",
            "/p/api.py",
            "def work():\n    return 'needle'\n",
            "p",
        );
        put(
            &store,
            "copy",
            "/p/api2.py",
            "def other():\n    return 'needle'\n",
            "p",
        );
        put(
            &store,
            "caller",
            "/p/use.py",
            "from api import work\nregistered = work\n",
            "p",
        );
        put(
            &store,
            "unreferenced",
            "/p/aaa.py",
            "def unused():\n    return 'needle'\n",
            "p",
        );
        put(
            &store,
            "caller",
            "/p/use.py",
            &format!(
                "from api import work\n{}",
                "registered = work\n".repeat(105)
            ),
            "p",
        );
        store.index_code().unwrap();
        let q = search(&["needle"]);
        let result = store.search_multi(&q).unwrap();
        let members = &result["families"][0]["members"];
        assert_eq!(members[0]["document"]["path"], "/p/api.py");
        assert_eq!(members[0]["role"], "referenced_candidate");
        assert!(members[0]["role_evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "incoming_reference_candidate"
                && e["path"] == "/p/use.py"
                && e["content_hash"].is_string()));
        assert_eq!(members[0]["role_evidence"].as_array().unwrap().len(), 100);
        assert_eq!(members[0]["role_evidence_truncated"], true);
        assert!(members[0]["role_evidence_total"].as_u64().unwrap() >= 105);
        put(&store, "caller", "/p/use.py", "registered = None\n", "p");
        let stale = store.search_multi(&q).unwrap();
        assert!(!stale["family_role_analysis"]["stale_documents"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(stale["families"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|f| f["members"].as_array().unwrap().iter())
            .all(|m| m["usage_score"].as_u64().unwrap() < 3));
        put(
            &store,
            "caller",
            "/p/use.py",
            "from api import work\nregistered = work\n",
            "p",
        );
        store.index_code().unwrap();
        put(
            &store,
            "api",
            "/p/api.py",
            "def changed():\n    return 'needle'\n",
            "p",
        );
        let stale = store.search_multi(&q).unwrap();
        assert!(stale["families"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|f| f["members"].as_array().unwrap().iter())
            .all(|m| m["usage_score"].as_u64().unwrap() < 3));
    }

    #[test]
    fn exact_and_numbered_copies_have_static_liveness_evidence_and_no_delete_claim() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("dups.duckdb")).unwrap();
        put(
            &store,
            "base",
            "/p/copies/files.ts",
            "export function work() { return 1; }",
            "p",
        );
        put(
            &store,
            "active",
            "/p/copies/files7.ts",
            "export function work() { return 1; }",
            "p",
        );
        put(
            &store,
            "caller",
            "/p/main.ts",
            "import {work} from './copies/files7'; function main() { work(); }",
            "p",
        );
        put(
            &store,
            "other",
            "/q/files.ts",
            "export function work() { return 1; }",
            "q",
        );
        store.index_code().unwrap();
        let result = store
            .duplicates(&DuplicateQuery {
                project: Some("p".into()),
                path_prefix: Some("/p/copies".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result["groups_total"], 1);
        assert_eq!(result["groups"][0]["primary_candidate"], "active");
        assert_eq!(result["groups"][0]["deletion_safe"], false);
        assert!(result["groups"][0]["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |m| m["document"]["document_id"] == "active" && m["role"] == "referenced_candidate"
            ));
        assert!(!result.to_string().contains("return 1"));
    }
    #[test]
    fn approximate_families_have_threshold_evidence_and_explicit_work_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("dups.duckdb")).unwrap();
        for i in 0..4 {
            put(
                &store,
                &i.to_string(),
                &format!("/p/files{i}.txt"),
                &format!("one two three four five six seven eight nine ten number{i}"),
                "p",
            );
        }
        let q = DuplicateQuery {
            min_similarity: Some(0.5),
            max_comparisons: Some(1),
            ..Default::default()
        };
        let result = store.duplicates(&q).unwrap();
        assert_eq!(result["comparison_limit_reached"], true);
        assert_eq!(result["comparisons"], 1);
        assert_eq!(result["groups_total"], 1);
        assert_eq!(
            result["groups"][0]["similarity_evidence"][0]["kind"],
            "name_family_and_token_shingles"
        );
    }
    #[test]
    fn duplicate_roles_include_imports_and_resources_from_outside_target_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("roles.duckdb")).unwrap();
        for (id, path, body) in [
            ("css1", "/p/copies/theme.css", "body { color:red; }"),
            ("css2", "/p/copies/theme2.css", "body { color:red; }"),
            ("js1", "/p/copies/mod.js", "const value=1;"),
            ("js2", "/p/copies/mod2.js", "const value=1;"),
            (
                "html",
                "/p/index.html",
                "<link rel='stylesheet' href='copies/theme2.css'>",
            ),
            (
                "import",
                "/p/use.js",
                "import {missing} from './copies/mod2.js';",
            ),
        ] {
            put(&store, id, path, body, "p");
        }
        put(
            &store,
            "foreign",
            "/q/index.html",
            "<link rel='stylesheet' href='../p/copies/theme.css'>",
            "q",
        );
        store.index_code().unwrap();
        let q = DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/copies".into()),
            ..Default::default()
        };
        let report = store.duplicates(&q).unwrap();
        let groups = report["groups"].as_array().unwrap();
        let css = groups
            .iter()
            .find(|g| g["primary_candidate"] == "css2")
            .unwrap_or_else(|| panic!("{report}"));
        assert!(css["members"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |e| e["kind"] == "incoming_file_resource_candidate" && e["path"] == "/p/index.html"
            ));
        let js = groups
            .iter()
            .find(|g| g["primary_candidate"] == "js2")
            .unwrap_or_else(|| panic!("{report}"));
        assert!(js["members"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "incoming_file_import_candidate" && e["path"] == "/p/use.js"));
        assert_eq!(report["role_contract_version"], 2);
        assert_eq!(css["primary_is_proven"], false);
        assert_eq!(js["deletion_safe"], false);
    }
    #[test]
    fn duplicate_roles_include_function_values_and_drop_stale_sources() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("values.duckdb")).unwrap();
        for (id, path) in [("base", "/p/copies/api.py"), ("used", "/p/copies/api2.py")] {
            put(
                &store,
                id,
                path,
                "def work():\n return 'PRIVATE_BODY'\n",
                "p",
            );
        }
        put(
            &store,
            "source",
            "/p/use.py",
            "from copies.api2 import work\nregistered=work\n",
            "p",
        );
        store.index_code().unwrap();
        let q = DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/copies".into()),
            ..Default::default()
        };
        let before = store.duplicates(&q).unwrap();
        assert_eq!(before["groups"][0]["primary_candidate"], "used", "{before}");
        assert!(before["groups"][0]["members"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "incoming_reference_candidate"));
        assert!(!before.to_string().contains("PRIVATE_BODY"));
        put(&store, "source", "/p/use.py", "registered=None\n", "p");
        let after = store.duplicates(&q).unwrap();
        assert_eq!(after["groups"][0]["usage_score"], 0, "{after}");
        store.index_code().unwrap();
        assert_eq!(store.duplicates(&q).unwrap()["groups"][0]["usage_score"], 0);
    }
    #[test]
    fn duplicate_resource_target_identity_and_parsed_target_fingerprints_are_required() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("identity.duckdb")).unwrap();
        for (id, path) in [("a", "/p/icon.svg"), ("b", "/p/icon2.svg")] {
            put(&store, id, path, "<svg><path d='M0 0'/></svg>", "p");
        }
        put(
            &store,
            "html",
            "/p/index.html",
            "<img src='icon2.svg'>",
            "p",
        );
        put(&store, "module", "/p/mod.js", "const value=1;", "p");
        put(&store, "import", "/p/use.js", "import './mod.js';", "p");
        store.index_code().unwrap();
        let files = store.corpus_files(Some("p"), None).unwrap();
        let snap = store.code_snapshot().unwrap();
        let roles = search_roles(&snap, &files);
        assert_eq!(roles.get("b").unwrap().0, 3);
        assert_eq!(roles.get("module").unwrap().0, 3);
        let mut changed = files;
        for file in &mut changed {
            if file.document_id == "b" || file.document_id == "module" {
                file.content_hash = content_hash("changed-between-observations");
            }
        }
        let roles = search_roles(&snap, &changed);
        assert!(!roles.contains_key("b"));
        assert!(!roles.contains_key("module"));
    }
    #[test]
    fn duplicate_evidence_counts_and_bounded_retention_are_order_independent() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("cap.duckdb")).unwrap();
        put(&store, "a", "/p/mod.js", "const value=1;", "p");
        put(&store, "b", "/p/mod2.js", "const value=1;", "p");
        put(
            &store,
            "source",
            "/p/use.js",
            &"import './mod2.js';\n".repeat(105),
            "p",
        );
        store.index_code().unwrap();
        let q = DuplicateQuery {
            project: Some("p".into()),
            max_members: Some(1),
            ..Default::default()
        };
        let report = store.duplicates(&q).unwrap();
        let group = &report["groups"][0];
        assert_eq!(group["primary_candidate"], "b");
        assert_eq!(group["members_total"], 2);
        assert_eq!(group["members"][0]["evidence_total"], 105);
        assert_eq!(
            group["members"][0]["evidence"].as_array().unwrap().len(),
            100
        );
        assert_eq!(group["members"][0]["evidence_truncated"], true);
        let files = store.corpus_files(Some("p"), None).unwrap();
        let mut snap = store.code_snapshot().unwrap();
        let before = search_roles(&snap, &files);
        snap.file_imports.reverse();
        assert_eq!(before, search_roles(&snap, &files));
    }
    #[test]
    fn duplicate_primary_remains_in_member_window_and_counts_break_role_ties() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("order.duckdb")).unwrap();
        put(
            &store,
            "long",
            "/p/a/deep/theme.css",
            "body{color:red}",
            "p",
        );
        put(&store, "short", "/p/z/theme2.css", "body{color:red}", "p");
        store.index_code().unwrap();
        let query = DuplicateQuery {
            project: Some("p".into()),
            max_members: Some(1),
            ..Default::default()
        };
        let report = store.duplicates(&query).unwrap();
        assert_eq!(report["groups"][0]["primary_candidate"], "short");
        assert_eq!(
            report["groups"][0]["members"][0]["document"]["document_id"],
            "short"
        );
        put(&store,"html","/p/index.html","<link href='a/deep/theme.css'><link href='a/deep/theme.css'><link href='z/theme2.css'>","p");
        store.index_code().unwrap();
        let report = store.duplicates(&query).unwrap();
        assert_eq!(report["groups"][0]["primary_candidate"], "long", "{report}");
        assert_eq!(report["groups"][0]["members"][0]["evidence_total"], 2);
        assert_eq!(
            report["groups"][0]["members"][0]["document"]["document_id"],
            "long"
        );
    }
    #[test]
    fn search_roles_include_imports_resources_and_sources_outside_target_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("roles.duckdb")).unwrap();
        for (id, path, body) in [
            ("css1", "/p/copies/theme.css", "/*needle*/body{color:red}"),
            ("css2", "/p/copies/theme2.css", "/*needle*/body{color:red}"),
            ("js1", "/p/copies/mod.js", "//needle\nconst value=1;"),
            ("js2", "/p/copies/mod2.js", "//needle\nconst value=1;"),
            (
                "svg1",
                "/p/copies/icon.svg",
                "<svg><text>needle</text></svg>",
            ),
            (
                "svg2",
                "/p/copies/icon2.svg",
                "<svg><text>needle</text></svg>",
            ),
            (
                "html",
                "/p/index.html",
                "<link href='copies/theme2.css'><img src='copies/icon2.svg'>",
            ),
            (
                "import",
                "/p/use.js",
                "import {missing} from './copies/mod2.js';",
            ),
        ] {
            put(&store, id, path, body, "p");
        }
        put(
            &store,
            "foreign",
            "/q/index.html",
            "<link href='../p/copies/theme.css'>",
            "q",
        );
        store.index_code().unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.path_prefix = Some("/p/copies".into());
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["documents_scanned"], 6);
        assert_eq!(result["matching_documents_total"], 6);
        assert_eq!(result["families_retained_total"], 3);
        for (family, used, kind) in [
            ("theme.css", "css2", "incoming_file_resource_candidate"),
            ("mod.js", "js2", "incoming_file_import_candidate"),
            ("icon.svg", "svg2", "incoming_file_resource_candidate"),
        ] {
            let group = result["families"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["name"] == family)
                .unwrap();
            let first = &group["members"][0];
            assert_eq!(first["document"]["document_id"], used, "{result}");
            assert_eq!(first["usage_score"], 3);
            assert!(first["role_evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == kind && e["path"].as_str().unwrap().starts_with("/p/")));
        }
        assert_eq!(result["family_role_analysis"]["role_contract_version"], 2);
        assert_eq!(
            result["family_role_analysis"]["markup_references_included"],
            true
        );
        assert_eq!(result["observation_consistency"], "single_read_transaction");
        assert_eq!(result, store.search_multi(&query).unwrap());
    }
    #[test]
    fn search_resource_roles_exclude_unindexed_and_stale_sources_and_targets() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("stale.duckdb")).unwrap();
        for (id, path) in [("a", "/p/copies/theme.css"), ("b", "/p/copies/theme2.css")] {
            put(&store, id, path, "/*needle*/body{color:red}", "p");
        }
        store.index_code().unwrap();
        put(
            &store,
            "html",
            "/p/index.html",
            "<link href='copies/theme2.css'>",
            "p",
        );
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.path_prefix = Some("/p/copies".into());
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            0
        );
        store.index_code().unwrap();
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["members"][0]["document"]
                ["document_id"],
            "b"
        );
        put(
            &store,
            "b",
            "/p/copies/theme2.css",
            "/*needle*/body{color:blue}",
            "p",
        );
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            0
        );
        store.index_code().unwrap();
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            3
        );
        put(&store, "html", "/p/index.html", "<main>removed</main>", "p");
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            0
        );
    }
    #[test]
    fn search_scopes_index_reads_and_skips_role_index_for_empty_windows() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("scope.duckdb")).unwrap();
        put(&store, "p", "/p/a.py", "def work(): return 'needle'", "p");
        put(&store, "q", "/q/a.py", "def work(): return 'needle'", "q");
        store.index_code().unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload='not-json' WHERE document_id='q'",
                [],
            )
            .unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        assert_eq!(
            store.search_multi(&query).unwrap()["matching_documents_total"],
            1
        );
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload='not-json' WHERE document_id='p'",
                [],
            )
            .unwrap();
        assert!(store.search_multi(&query).is_err());
        query.queries = vec!["not-present".into()];
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["matching_documents_total"], 0);
        assert!(result["family_role_analysis"].is_null());
    }
    #[test]
    fn corpus_read_transaction_keeps_roles_consistent_across_a_concurrent_publication() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("snapshot.duckdb");
        let reader = Store::open(&path).unwrap();
        let mut writer = reader.lock().unwrap().try_clone().unwrap();
        put(
            &reader,
            "a",
            "/p/copies/mod.js",
            "//needle\nconst value=1;",
            "p",
        );
        put(
            &reader,
            "b",
            "/p/copies/mod2.js",
            "//needle\nconst value=1;",
            "p",
        );
        put(
            &reader,
            "source",
            "/p/use.js",
            "import './copies/mod2.js';",
            "p",
        );
        reader.index_code().unwrap();
        let mut conn = reader.lock().unwrap();
        let transaction = conn.transaction().unwrap();
        let files = corpus_files_locked(&transaction, Some("p"), Some("/p/copies")).unwrap();
        // A cloned connection shares this DuckDB instance, unlike a second open.
        // Publish real parsed facts/history while the first read view stays open.
        let write = writer.transaction().unwrap();
        write.execute("UPDATE documents SET content='const removed=1;',revision=revision+1 WHERE id='source'",[]).unwrap();
        let docs = {
            let mut statement=write.prepare("SELECT id,source_file,content,COALESCE(revision,1),wing,source_file IS NOT NULL FROM documents ORDER BY id").unwrap();
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, bool>(5)?,
                    ))
                })
                .unwrap();
            rows.collect::<std::result::Result<Vec<_>, _>>().unwrap()
        };
        let analyses: Vec<_> = docs
            .iter()
            .map(|(id, path, source, revision, project, _)| {
                let mut parsed = parse(id, path, source, *revision).unwrap();
                parsed.project = project.clone();
                (id.clone(), serde_json::to_string(&parsed).unwrap())
            })
            .collect();
        for (id, payload) in &analyses {
            write
                .execute(
                    "UPDATE code_index_documents SET payload=? WHERE document_id=?",
                    params![payload, id],
                )
                .unwrap();
        }
        let published = history::publish(&write, &docs, &analyses, false).unwrap();
        assert!(published.1);
        assert_eq!(published.0, 2);
        write.commit().unwrap();
        let old = super::super::snapshot_locked(&transaction, Some("p")).unwrap();
        assert_eq!(old.index_revision, Some(1));
        let roles = search_roles(&old, &files);
        assert_eq!(roles.get("b").unwrap().0, 3);
        transaction.commit().unwrap();
        drop(conn);
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.path_prefix = Some("/p/copies".into());
        assert_eq!(
            reader.search_multi(&query).unwrap()["families"][0]["usage_score"],
            0
        );
        let duplicate = reader
            .duplicates(&DuplicateQuery {
                project: Some("p".into()),
                path_prefix: Some("/p/copies".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(duplicate["groups"][0]["usage_score"], 0);
        assert_eq!(
            duplicate["role_observation_consistency"],
            "single_read_transaction"
        );
    }
    #[test]
    fn scoped_corpus_body_reads_cross_parameter_batches_without_losing_order() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("batches.duckdb")).unwrap();
        store.lock().unwrap().execute("INSERT INTO documents(id,uri,title,content,source_file,wing,created_at,updated_at) SELECT 'd'||i,'file://d'||i,'doc','needle','/p/copies/f'||i||'.txt','p',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP FROM range(513) t(i)",[]).unwrap();
        put(&store, "quoted'?)", "/p/copies/quoted.txt", "needle", "p");
        put(&store, "outside", "/p/other.txt", "outside", "p");
        let mut connection = store.lock().unwrap();
        let tx = connection.transaction().unwrap();
        let files = corpus_files_locked(&tx, Some("p"), Some("/p/copies")).unwrap();
        assert_eq!(files.len(), 514);
        assert!(files
            .windows(2)
            .all(|pair| (&pair[0].path, &pair[0].document_id)
                <= (&pair[1].path, &pair[1].document_id)));
        assert!(files.iter().any(|f| f.document_id == "quoted'?)"));
        assert!(files.iter().all(|f| f.content == "needle"));
        tx.commit().unwrap();
    }
    #[test]
    fn literal_path_roles_find_endpoint_constants_outside_target_scope() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("literal.duckdb")).unwrap();
        for (id, path) in [("base", "/p/php/Files.php"), ("used", "/p/php/Files2.php")] {
            put(
                &store,
                id,
                path,
                "<?php function work(){return 'needle';}",
                "p",
            );
        }
        put(
            &store,
            "js",
            "/p/app.js",
            "// café\nconst ENDPOINT='php/Files2.php';",
            "p",
        );
        put(
            &store,
            "foreign",
            "/q/app.js",
            "const ENDPOINT='../p/php/Files.php';",
            "q",
        );
        store.index_code().unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.path_prefix = Some("/p/php".into());
        let result = store.search_multi(&query).unwrap();
        let member = &result["families"][0]["members"][0];
        assert_eq!(member["document"]["document_id"], "used", "{result}");
        assert_eq!(member["usage_score"], 2);
        let evidence = member["role_evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["kind"] == "literal_text_path_mention_candidate")
            .unwrap();
        assert_eq!(evidence["line"], 2);
        assert_eq!(evidence["binding_proven"], false);
        assert!(evidence["byte_start"].as_u64().unwrap() > 8);
        assert_eq!(evidence["needle"], "php/Files2.php");
        let dup = store
            .duplicates(&DuplicateQuery {
                project: Some("p".into()),
                path_prefix: Some("/p/php".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(dup["groups"][0]["primary_candidate"], "used");
        assert_eq!(dup["groups"][0]["deletion_safe"], false);
        assert_eq!(dup["literal_path_analysis"]["evidence_total"], 1);
    }
    #[test]
    fn literal_path_basename_uniqueness_uses_the_whole_project_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("ambiguous.duckdb")).unwrap();
        put(&store, "a", "/p/a/Files.php", "<?php echo 'needle';", "p");
        put(&store, "b", "/p/b/Files.php", "<?php echo 'needle';", "p");
        put(&store, "js", "/p/app.js", "const path='Files.php';", "p");
        store.index_code().unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.path_prefix = Some("/p/a".into());
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["families"][0]["usage_score"], 0);
        assert_eq!(
            result["literal_path_analysis"]["ambiguous_basename_targets"],
            1
        );
        put(&store, "js", "/p/app.js", "const path='a/Files.php';", "p");
        store.index_code().unwrap();
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            2
        );
    }
    #[test]
    fn literal_path_boundaries_reject_substrings_unrelated_paths_and_external_urls() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("boundaries.duckdb")).unwrap();
        put(
            &store,
            "target",
            "/p/copies/Files2.php",
            "<?php echo 'needle';",
            "p",
        );
        put(&store,"source","/p/app.js","const a='Files2.php.bak', b='OtherFiles2.php', c='other/Files2.php', d='https://remote/copies/Files2.php';","p");
        store.index_code().unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["families"][0]["usage_score"], 0, "{result}");
        put(
            &store,
            "source",
            "/p/app.js",
            "const path='./copies/Files2.php';",
            "p",
        );
        store.index_code().unwrap();
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            2
        );
    }
    #[test]
    fn literal_path_scan_reports_budgets_disabled_mode_and_stale_sources() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("budgets.duckdb")).unwrap();
        put(
            &store,
            "target",
            "/p/copies/Files2.php",
            "<?php echo 'needle';",
            "p",
        );
        put(
            &store,
            "source",
            "/p/app.js",
            "const path='copies/Files2.php';",
            "p",
        );
        store.index_code().unwrap();
        let mut query = search(&["needle"]);
        query.project = Some("p".into());
        query.literal_mentions = Some(LiteralMentionOptions {
            enabled: Some(false),
            ..Default::default()
        });
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["literal_path_analysis"]["status"], "disabled");
        assert_eq!(result["families"][0]["usage_score"], 0);
        query.literal_mentions = Some(LiteralMentionOptions {
            max_bytes: Some(1),
            ..Default::default()
        });
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["literal_path_analysis"]["complete"], false);
        assert_eq!(result["families"][0]["usage_score"], 0);
        assert_eq!(result["literal_path_analysis"]["absence_proven"], false);
        query.literal_mentions = None;
        put(
            &store,
            "source",
            "/p/app.js",
            "const path='copies/Files2.php'; // changed",
            "p",
        );
        let result = store.search_multi(&query).unwrap();
        assert_eq!(result["families"][0]["usage_score"], 0);
        assert!(
            result["literal_path_analysis"]["sources_stale_unindexed_or_unqualified"]
                .as_u64()
                .unwrap()
                > 0
        );
        store.index_code().unwrap();
        assert_eq!(
            store.search_multi(&query).unwrap()["families"][0]["usage_score"],
            2
        );
    }
    #[test]
    fn literal_path_scan_caps_retention_but_keeps_complete_unique_source_counts() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("retention.duckdb")).unwrap();
        for (id, path) in [("a", "/p/copies/Files.php"), ("b", "/p/copies/Files2.php")] {
            put(&store, id, path, "<?php echo 'needle';", "p");
        }
        for i in 0..25 {
            put(
                &store,
                &format!("s{i}"),
                &format!("/p/ref{i:02}.js"),
                "const path='copies/Files2.php'; // copies/Files2.php",
                "p",
            );
        }
        store.index_code().unwrap();
        let query = DuplicateQuery {
            project: Some("p".into()),
            path_prefix: Some("/p/copies".into()),
            ..Default::default()
        };
        let result = store.duplicates(&query).unwrap();
        let member = &result["groups"][0]["members"][0];
        assert_eq!(member["document"]["document_id"], "b");
        assert_eq!(member["evidence_total"], 25);
        assert_eq!(member["evidence"].as_array().unwrap().len(), 20);
        assert_eq!(member["evidence_truncated"], true);
        assert_eq!(result["literal_path_analysis"]["evidence_total"], 25);
        assert_eq!(result["literal_path_analysis"]["evidence_retained"], 20);
        assert_eq!(result, store.duplicates(&query).unwrap());
    }
}
