//! Exact/regex multi-string search and evidence-backed duplicate families over stored text.
use super::*;
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
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct DuplicateQuery {
    pub project: Option<String>,
    pub path_prefix: Option<String>,
    pub limit: Option<usize>,
    pub min_similarity: Option<f64>,
    pub max_comparisons: Option<usize>,
    pub max_members: Option<usize>,
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
fn search_roles(
    snapshot: &CodeSnapshot,
    files: &[File],
) -> HashMap<String, (u64, Vec<serde_json::Value>, usize)> {
    let fresh: HashMap<_, _> = files.iter().map(|f| (f.document_id.as_str(), f)).collect();
    let own: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            fresh
                .get(s.document_id.as_str())
                .is_some_and(|f| f.content_hash == s.content_hash && f.revision == s.revision)
        })
        .map(|s| (s.id.as_str(), s.document_id.as_str()))
        .collect();
    let mut roles: HashMap<String, (u64, Vec<serde_json::Value>, usize)> = HashMap::new();
    let mut add = |doc: &str, score: u64, evidence: serde_json::Value| {
        let value = roles.entry(doc.into()).or_default();
        value.0 = value.0.max(score);
        value.2 += 1;
        if value.1.len() < 100 {
            value.1.push(evidence);
        }
    };
    for entry in &snapshot.entrypoints {
        if let Some(doc) = entry.symbol_id.as_deref().and_then(|id| own.get(id)) {
            let score = if entry.kind.contains("main") || entry.kind == "http_route_candidate" {
                4
            } else {
                1
            };
            add(
                doc,
                score,
                serde_json::json!({"kind":entry.kind,"path":entry.path,"line":entry.line,"symbol_id":entry.symbol_id,"resolution":entry.resolution,"provenance":"derived"}),
            );
        }
    }
    for edge in &snapshot.bound_references {
        let mut targets = BTreeSet::new();
        for id in &edge.candidates {
            if let Some(doc) = own.get(id.as_str()) {
                if *doc != edge.reference.document_id {
                    targets.insert(*doc);
                }
            }
        }
        for doc in targets {
            add(
                doc,
                if edge.resolution == "resolved_static" {
                    3
                } else {
                    2
                },
                serde_json::json!({
                "kind":if edge.reference.is_call {"incoming_call_candidate"} else {"incoming_reference_candidate"},
                "path":edge.reference.path,"line":edge.reference.line,"document_id":edge.reference.document_id,
                "revision":edge.reference.revision,"content_hash":edge.reference.content_hash,
                "resolution":edge.resolution,"provenance":"derived"}),
            );
        }
    }
    for (_, evidence, _) in roles.values_mut() {
        evidence.sort_by_key(serde_json::Value::to_string);
    }
    roles
}
impl Store {
    fn corpus_files(&self, project: Option<&str>, prefix: Option<&str>) -> Result<Vec<File>> {
        let conn = self.lock()?;
        let mut stmt=conn.prepare("SELECT id,COALESCE(source_file,uri),wing,COALESCE(revision,1),content,metadata_json FROM documents WHERE COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated') AND (? IS NULL OR wing=?) ORDER BY 2,id")?;
        let rows = stmt.query_map(params![project, project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let mut files = Vec::new();
        for row in rows {
            let (id, path, project, revision, content, metadata) = row;
            if in_scope(&path, prefix) {
                let content = source::read(&conn, &content, &metadata)?.into_owned();
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
                    content_hash: hash,
                    source_encoding: field("source_encoding"),
                    source_encoding_basis: field("source_encoding_basis"),
                    source_bytes_hash: field("source_bytes_hash"),
                    content,
                });
            }
        }
        Ok(files)
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
        let files = self.corpus_files(q.project.as_deref(), q.path_prefix.as_deref())?;
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
            Some(self.code_snapshot()?)
        } else {
            None
        };
        let roles = snapshot
            .as_ref()
            .map(|s| search_roles(s, &files))
            .unwrap_or_default();
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
        let role_analysis = snapshot.as_ref().map(|s| serde_json::json!({"index_rev":s.index_revision,"basis":"fresh_static_index_candidates","runtime_activity_verified":false,"stale_documents":s.stale_documents,"unindexed_documents":s.unindexed_documents,"parse_errors":s.parse_errors,"markup_references_included":false}));
        Ok(
            serde_json::json!({"items":items,"per_query":per_query,"families":family_items,"families_retained_total":family_total,"families_truncated":family_total>limit,"family_coverage":"retained_per_query_windows","family_roles_verified":false,"family_role_analysis":role_analysis,"union_matching_documents_total":union_total,"matching_documents_total":matched,"documents_scanned":files.len(),"truncated":matched>limit,"per_query_document_counts":counts,"match_mode":mode,"regex":q.regex,"case_insensitive":q.case_insensitive,"snapshot":"current_stored_documents","chunk_boundary_independent":true,"filesystem_read":false}),
        )
    }
    pub fn duplicates(&self, q: &DuplicateQuery) -> Result<serde_json::Value> {
        let threshold = q.min_similarity.unwrap_or(0.8);
        if !threshold.is_finite() || !(0.5..=1.0).contains(&threshold) {
            return Err(AppError::config("min_similarity must be between 0.5 and 1"));
        }
        let corpus = self.corpus_files(q.project.as_deref(), None)?;
        let fingerprints: HashMap<_, _> = corpus
            .iter()
            .map(|f| (f.document_id.clone(), (f.content_hash.clone(), f.revision)))
            .collect();
        let files: Vec<_> = corpus
            .into_iter()
            .filter(|f| in_scope(&f.path, q.path_prefix.as_deref()))
            .collect();
        let snapshot = self.code_snapshot()?;
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
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..files.len() {
            let r = root(&mut parent, i);
            groups.entry(r).or_default().push(i);
        }
        let mut results = Vec::new();
        for group in groups.values().filter(|v| v.len() > 1) {
            let mut members = Vec::new();
            let mut best: (usize, usize) = (0, group[0]);
            for i in group {
                let file = &files[*i];
                let own: Vec<_> = snapshot
                    .symbols
                    .iter()
                    .filter(|s| {
                        s.document_id == file.document_id
                            && s.content_hash == file.content_hash
                            && s.revision == file.revision
                    })
                    .collect();
                let own_ids: HashSet<_> = own.iter().map(|s| s.id.as_str()).collect();
                let mut evidence = Vec::new();
                let mut score = 0;
                for entry in snapshot.entrypoints.iter().filter(|e| {
                    e.symbol_id
                        .as_deref()
                        .is_some_and(|id| own_ids.contains(id))
                }) {
                    let weight = if matches!(
                        entry.kind.as_str(),
                        "conventional_main" | "http_route_candidate"
                    ) {
                        4
                    } else {
                        1
                    };
                    score = score.max(weight);
                    evidence.push(serde_json::json!({"kind":entry.kind,"path":entry.path,"line":entry.line,"provenance":"derived"}));
                }
                for call in snapshot.calls.iter().filter(|c| {
                    c.reference.document_id != file.document_id
                        && c.candidates.iter().any(|id| own_ids.contains(id.as_str()))
                }) {
                    // Validate the referring document against the same captured text corpus.
                    if !fingerprints
                        .get(&call.reference.document_id)
                        .is_some_and(|(hash, rev)| {
                            hash == &call.reference.content_hash && *rev == call.reference.revision
                        })
                    {
                        continue;
                    }
                    score = score.max(3);
                    if evidence.len() < 100 {
                        evidence.push(serde_json::json!({"kind":"incoming_call_candidate","path":call.reference.path,"line":call.reference.line,"resolution":call.resolution,"provenance":"derived"}));
                    }
                }
                if score > best.0
                    || (score == best.0
                        && (file.path.len(), &file.path)
                            < (files[best.1].path.len(), &files[best.1].path))
                {
                    best = (score, *i);
                }
                members.push(serde_json::json!({"document":file,"usage_score":score,"role":match score{4=>"entrypoint_candidate",3=>"referenced_candidate",1=>"public_api_candidate",_=>"no_observed_usage"},"evidence":evidence.iter().take(100).collect::<Vec<_>>(),"evidence_truncated":evidence.len()>100}));
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
                b["usage_score"]
                    .as_u64()
                    .cmp(&a["usage_score"].as_u64())
                    .then_with(|| {
                        a["document"]["path"]
                            .as_str()
                            .cmp(&b["document"]["path"].as_str())
                    })
            });
            members.truncate(member_limit);
            results.push(serde_json::json!({"primary_candidate":files[best.1].document_id,"primary_is_proven":false,"usage_score":best.0,"members":members,"members_total":group.len(),"members_truncated":group.len()>member_limit,"similarity_evidence":links.iter().take(1000).collect::<Vec<_>>(),"similarity_evidence_total":links.len(),"similarity_evidence_truncated":links.len()>1000,"deletion_safe":false}));
        }
        results.sort_by(|a, b| {
            b["usage_score"]
                .as_u64()
                .cmp(&a["usage_score"].as_u64())
                .then_with(|| {
                    a["primary_candidate"]
                        .as_str()
                        .cmp(&b["primary_candidate"].as_str())
                })
        });
        let total = results.len();
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        results.truncate(limit);
        Ok(
            serde_json::json!({"groups":results,"groups_total":total,"documents_checked":files.len(),"truncated":total>limit,"comparison_limit_reached":truncated,"comparisons":comparisons,"sampled_documents":sampled.iter().map(|i|&files[*i].document_id).collect::<Vec<_>>(),"index_rev":snapshot.index_revision,"group_semantics":"connected_similarity_components","exactness":"stored_utf8_text_not_original_binary","liveness_semantics":"observed_static_evidence_not_runtime_proof","stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors}),
        )
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
        }
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
}
