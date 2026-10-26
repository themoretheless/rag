//! Bounded exact stored-text mentions. No source execution or runtime binding.
use super::*;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Options {
    /// Defaults true. Disabling does not disable typed import/resource evidence.
    pub enabled: Option<bool>,
    /// Candidate targets (default 500, clamped 1..5000).
    pub max_targets: Option<usize>,
    /// Stored source reads (default 2000, clamped 1..100000).
    pub max_sources: Option<usize>,
    /// Decoded source bytes (default 8 MiB, maximum 64 MiB).
    pub max_bytes: Option<usize>,
    /// Per-source decoded bytes (default 2 MiB, maximum 8 MiB).
    pub max_source_bytes: Option<usize>,
}
pub(super) struct Scan {
    pub evidence: Vec<(String, serde_json::Value)>,
    pub report: serde_json::Value,
    pub counts: HashMap<String, usize>,
}
impl Scan {
    pub fn skipped(reason: &str) -> Self {
        Self {
            evidence: vec![],
            counts: HashMap::new(),
            report: serde_json::json!({"contract_version":1,"status":reason,"complete":false,"binding_proven":false,"runtime_activity_proven":false,"absence_proven":false}),
        }
    }
}
struct Source {
    id: String,
    path: String,
    project: Option<String>,
    revision: i64,
    preserved: Option<String>,
    bytes: Option<usize>,
    metadata_valid: bool,
}
fn boundary(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | ':' | '$' | '@')
}
fn occurrence(source: &str, needle: &str) -> Option<usize> {
    source
        .match_indices(needle)
        .find(|(offset, _)| {
            !source[..*offset].chars().next_back().is_some_and(boundary)
                && !source[*offset + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(boundary)
        })
        .map(|(offset, _)| offset)
}
struct Identities<'a> {
    snapshot: &'a CodeSnapshot,
    bad: HashSet<&'a str>,
    attachments: HashMap<&'a str, &'a resource_targets::ResourceTarget>,
}
impl<'a> Identities<'a> {
    fn fresh(
        &self,
        id: &str,
        path: &str,
        project: &Option<String>,
        revision: i64,
        hash: &str,
    ) -> bool {
        if self.bad.contains(id) {
            return false;
        }
        self.snapshot
            .document_scopes
            .get(id)
            .is_some_and(|scope| scope.path == path && &scope.project == project)
            && self
                .snapshot
                .document_fingerprints
                .get(id)
                .is_some_and(|(r, h)| *r == revision && h == hash)
            || self.attachments.get(id).is_some_and(|t| {
                t.fresh()
                    && t.path == path
                    && &t.project == project
                    && t.revision == revision
                    && t.content_hash.as_deref() == Some(hash)
            })
    }
}
pub(super) fn scan(
    conn: &duckdb::Connection,
    snapshot: &CodeSnapshot,
    targets: &[&File],
    project: Option<&str>,
    options: Option<&Options>,
) -> Result<Scan> {
    if targets.is_empty() {
        return Ok(Scan::skipped("no_candidate_targets"));
    }
    let identities = Identities {
        snapshot,
        bad: snapshot
            .stale_documents
            .iter()
            .chain(&snapshot.unindexed_documents)
            .chain(&snapshot.parse_errors)
            .map(String::as_str)
            .collect(),
        attachments: snapshot
            .file_resource_targets
            .iter()
            .map(|t| (t.document_id.as_str(), t))
            .collect(),
    };
    let default = Options::default();
    let options = options.unwrap_or(&default);
    if options.enabled == Some(false) {
        return Ok(Scan::skipped("disabled"));
    }
    let target_cap = options.max_targets.unwrap_or(500).clamp(1, 5000);
    let source_cap = options.max_sources.unwrap_or(2000).clamp(1, 100000);
    let bytes_cap = options
        .max_bytes
        .unwrap_or(8 * 1024 * 1024)
        .clamp(1, 64 * 1024 * 1024);
    let per_source = options
        .max_source_bytes
        .unwrap_or(2 * 1024 * 1024)
        .clamp(1, 8 * 1024 * 1024);
    let sql=format!("SELECT d.id,d.source_file,d.wing,COALESCE(d.revision,1),json_extract_string(try_cast(d.metadata_json AS JSON),'$.code_source_hash'),octet_length(encode({text})),json_valid(d.metadata_json) {sources} ORDER BY d.source_file,d.id",text=super::super::dead_files::TEXT,sources=super::super::dead_files::SOURCES);
    let sources = {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params![project, project], |r| {
            Ok(Source {
                id: r.get(0)?,
                path: r.get(1)?,
                project: r.get(2)?,
                revision: r.get(3)?,
                preserved: r.get(4)?,
                bytes: r
                    .get::<_, Option<i64>>(5)?
                    .and_then(|v| usize::try_from(v).ok()),
                metadata_valid: r.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut basenames: BTreeMap<(Option<String>, String), usize> = BTreeMap::new();
    let mut roots: BTreeMap<Option<String>, PathBuf> = BTreeMap::new();
    let mut path_counts: BTreeMap<(Option<String>, String), usize> = BTreeMap::new();
    for source in &sources {
        let path = Path::new(&source.path);
        let base = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        *basenames.entry((source.project.clone(), base)).or_default() += 1;
        *path_counts
            .entry((source.project.clone(), source.path.clone()))
            .or_default() += 1;
        let parent = path.parent().unwrap_or(Path::new(""));
        roots
            .entry(source.project.clone())
            .and_modify(|root| {
                while !parent.starts_with(root.as_path()) {
                    if !root.pop() {
                        break;
                    }
                }
            })
            .or_insert_with(|| parent.into());
    }
    let mut sorted = targets.to_vec();
    sorted.sort_by(|a, b| {
        (&a.project, &a.path, &a.document_id).cmp(&(&b.project, &b.path, &b.document_id))
    });
    let requested = sorted.len();
    let mut selected = vec![];
    let mut rejected = 0;
    for file in sorted.into_iter().take(target_cap) {
        if identities.fresh(
            &file.document_id,
            &file.path,
            &file.project,
            file.revision,
            &file.content_hash,
        ) {
            selected.push(file);
        } else {
            rejected += 1;
        }
    }
    let mut needles: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut ambiguous_basenames = 0;
    for (index, file) in selected.iter().enumerate() {
        let path = Path::new(&file.path);
        let base = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut values = BTreeSet::from([file.path.clone()]);
        if let Some(relative) = roots
            .get(&file.project)
            .and_then(|root| path.strip_prefix(root).ok())
        {
            let relative = relative.to_string_lossy().into_owned();
            values.insert(relative.clone());
            values.insert(format!("./{relative}"));
        }
        if basenames.get(&(file.project.clone(), base.clone())) == Some(&1) {
            values.insert(base);
        } else {
            ambiguous_basenames += 1;
        }
        for value in values
            .into_iter()
            .filter(|v| v.len() >= 4 && v.len() <= 4096)
        {
            needles.entry(value).or_default().push(index);
        }
    }
    let patterns: Vec<_> = needles.keys().cloned().collect();
    if patterns.iter().map(String::len).sum::<usize>() > 256 * 1024 {
        return Ok(Scan::skipped("pattern_byte_budget_exceeded"));
    }
    let matcher = match regex::RegexSetBuilder::new(patterns.iter().map(|p| regex::escape(p)))
        .size_limit(2 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
    {
        Ok(matcher) => matcher,
        Err(_) => return Ok(Scan::skipped("pattern_automaton_budget_exceeded")),
    };
    let mut covered: HashSet<(String, String)> = HashSet::new();
    for edge in &snapshot.file_imports {
        for target in &edge.candidates {
            covered.insert((edge.document_id.clone(), target.clone()));
        }
    }
    for edge in &snapshot.file_resources {
        for target in &edge.candidates {
            covered.insert((edge.document_id.clone(), target.clone()));
        }
    }
    let source_sql=format!("SELECT CASE WHEN octet_length(encode({text}))<=? THEN {text} ELSE NULL END {sources} AND d.id=?",text=super::super::dead_files::TEXT,sources=super::super::dead_files::SOURCES);
    let mut evidence = vec![];
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total_bytes = 0;
    let mut reads = 0;
    let mut skipped = 0;
    let mut stale = 0;
    let mut covered_mentions = 0;
    let mut hit_targets = BTreeSet::new();
    for source in &sources {
        let Some(size) = source.bytes else {
            skipped += 1;
            continue;
        };
        if !source.metadata_valid
            || size > per_source
            || size > bytes_cap.saturating_sub(total_bytes)
            || reads >= source_cap
        {
            skipped += 1;
            continue;
        }
        // A prior stale observation is not turned into a new positive text role.
        let identity = snapshot
            .document_fingerprints
            .get(&source.id)
            .map(|(_, h)| h.as_str())
            .or(source.preserved.as_deref())
            .or_else(|| {
                identities
                    .attachments
                    .get(source.id.as_str())
                    .and_then(|t| t.content_hash.as_deref())
            });
        if !identity.is_some_and(|hash| {
            identities.fresh(
                &source.id,
                &source.path,
                &source.project,
                source.revision,
                hash,
            )
        }) {
            stale += 1;
            continue;
        }
        reads += 1;
        let text: Option<String> = conn.query_row(
            &source_sql,
            params![per_source as i64, project, project, source.id],
            |r| r.get(0),
        )?;
        let Some(text) = text else {
            skipped += 1;
            continue;
        };
        total_bytes += text.len();
        let hash = content_hash(&text);
        if !identities.fresh(
            &source.id,
            &source.path,
            &source.project,
            source.revision,
            &hash,
        ) {
            stale += 1;
            continue;
        }
        let mut hits: BTreeMap<usize, (usize, &str)> = BTreeMap::new();
        for pattern in matcher.matches(&text).iter() {
            let needle = &patterns[pattern];
            let Some(offset) = occurrence(&text, needle) else {
                continue;
            };
            for &target in &needles[needle] {
                let file = selected[target];
                if source.project != file.project
                    || source.id == file.document_id
                    || source.path == file.path
                {
                    continue;
                }
                if covered.contains(&(source.id.clone(), file.document_id.clone())) {
                    covered_mentions += 1;
                    continue;
                }
                let candidate = (offset, needle.as_str());
                if hits.get(&target).is_none_or(|old| candidate < *old) {
                    hits.insert(target, candidate);
                }
            }
        }
        for (target, (offset, needle)) in hits {
            let file = selected[target];
            hit_targets.insert(file.document_id.clone());
            let count = counts.entry(file.document_id.clone()).or_default();
            *count += 1;
            if *count <= 20 {
                evidence.push((file.document_id.clone(),serde_json::json!({"kind":"literal_text_path_mention_candidate","needle":needle,"document_id":source.id,"path":source.path,"project":source.project,"revision":source.revision,"content_hash":hash,"line":text[..offset].bytes().filter(|b|*b==b'\n').count()+1,"byte_start":offset,"byte_end":offset+needle.len(),"target_document_id":file.document_id,"target_path":file.path,"target_revision":file.revision,"target_content_hash":file.content_hash,"path_identity_ambiguous":path_counts.get(&(file.project.clone(),file.path.clone())).copied().unwrap_or(0)>1,"resolution":"unbound_exact_text_mention","binding_proven":false,"runtime_usage_proven":false,"provenance":"observed_stored_text"})));
            }
        }
    }
    let complete = requested <= target_cap && rejected == 0 && skipped == 0 && stale == 0;
    let report = serde_json::json!({"contract_version":1,"status":"scanned","complete":complete,"targets_requested":requested,"targets_checked":selected.len(),"targets_unchecked":requested.saturating_sub(selected.len()),"sources_inventory":sources.len(),"source_reads":reads,"decoded_bytes_loaded":total_bytes,"sources_skipped_by_budget_or_unavailable":skipped,"sources_stale_unindexed_or_unqualified":stale,"ambiguous_basename_targets":ambiguous_basenames,"typed_relation_mentions_suppressed":covered_mentions,"evidence_total":counts.values().sum::<usize>(),"evidence_retained":evidence.len(),"evidence_truncated":counts.values().sum::<usize>()>evidence.len(),"targets_with_mentions":hit_targets.len(),"observation_consistency":"single_read_transaction","binding_proven":false,"runtime_activity_proven":false,"absence_proven":false,"budgets":{"max_targets":target_cap,"max_sources":source_cap,"max_decoded_bytes":bytes_cap,"max_source_bytes":per_source,"pattern_bytes":256*1024,"pattern_automaton_bytes":2*1024*1024,"index_validation_included":false,"metadata_inventory_included":false,"evidence_per_target_limit":20},"case_sensitive":true,"string_escapes_or_HTML_entities_decoded":false,"comments_and_data_can_match":true});
    Ok(Scan {
        evidence,
        report,
        counts,
    })
}
