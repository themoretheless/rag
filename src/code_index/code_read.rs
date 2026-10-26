//! Explicit source reads in one captured-view transaction, with shared text budgets.
use super::*;
mod cursor;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CodeReadQuery {
    #[serde(flatten)]
    pub selection: CodeQuery,
    /// Exact project; disambiguates paths and names before reading bodies.
    pub project: Option<String>,
    pub document_id: Option<String>,
    /// Symbol/current chunk IDs as array or comma/whitespace text; at most 100; cannot combine with symbol.
    #[serde(default, deserialize_with = "cursor::ids")]
    #[schemars(with = "Option<cursor::IdInput>")]
    pub entry_ids: Option<Vec<String>>,
    /// One-based inclusive source line window, intersected with selected spans.
    pub start: Option<usize>,
    pub end: Option<usize>,
    /// Inclusive UTF-8 byte start; optional exclusive end. Combined with line windows.
    pub start_byte: Option<usize>,
    pub end_byte: Option<usize>,
    /// Optimistic continuation returned by the preceding code page.
    pub cursor: Option<String>,
    /// File reads: retain nested metadata, partition parent bodies around children.
    #[serde(default)]
    pub include_containers: bool,
    /// Shared returned source character budget, 1..100000 (default 20000).
    pub max_chars: Option<usize>,
    /// stored/index/auto all read captured source; disk reads use live_read.
    pub source: Option<String>,
}
#[derive(Clone)]
struct Span {
    document: String,
    start: usize,
    end: usize,
    symbol: Option<Symbol>,
    via: String,
}
fn line(source: &str, offset: usize) -> usize {
    1 + source.as_bytes()[..offset]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}
fn window(source: &str, start: Option<usize>, end: Option<usize>) -> (usize, usize) {
    let mut a = 0;
    let mut b = source.len();
    let mut current = 1;
    if start.is_some_and(|v| v > 1) {
        a = source.len();
    }
    for (i, c) in source.bytes().enumerate() {
        if c == b'\n' {
            if end == Some(current) {
                b = i + 1;
                break;
            }
            current += 1;
            if start == Some(current) {
                a = i + 1;
            }
        }
    }
    (a, b)
}
fn subtract(a: usize, b: usize, children: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut cursor = a;
    let mut pieces = Vec::new();
    for &(x, y) in children {
        if y <= cursor || x >= b {
            continue;
        }
        if x > cursor {
            pieces.push((cursor, x.min(b)));
        }
        cursor = cursor.max(y).min(b);
    }
    if cursor < b {
        pieces.push((cursor, b));
    }
    pieces
}
impl Store {
    pub fn code_read(&self, q: &CodeReadQuery) -> Result<serde_json::Value> {
        if q.start == Some(0) || q.end == Some(0) || q.start.zip(q.end).is_some_and(|(a, b)| a > b)
        {
            return Err(AppError::config(
                "code start/end must be positive ordered source lines",
            ));
        }
        if q.start_byte.zip(q.end_byte).is_some_and(|(a, b)| a > b) {
            return Err(AppError::config("code byte bounds must be ordered"));
        }
        let ids = q.entry_ids.as_deref().unwrap_or(&[]);
        if ids.len() > 100
            || ids.iter().any(|s| s.is_empty() || s.len() > 512)
            || !ids.is_empty() && q.selection.symbol.is_some()
        {
            return Err(AppError::config(
                "code entry_ids: at most 100 nonempty IDs, cannot combine with symbol",
            ));
        }
        if q.selection
            .symbol
            .as_deref()
            .is_some_and(|s| s.trim().is_empty())
            || q.selection.symbol.is_none()
                && ids.is_empty()
                && q.selection.path.is_none()
                && q.document_id.is_none()
        {
            return Err(AppError::config(
                "code requires symbol, path, document_id or entry_ids",
            ));
        }
        if q.source
            .as_deref()
            .is_some_and(|s| !matches!(s, "stored" | "index" | "auto"))
        {
            return Err(AppError::config(
                "code source must be stored/index/auto; use live_read for disk sources",
            ));
        }
        let budget = q.max_chars.unwrap_or(20000);
        if !(1..=100000).contains(&budget) {
            return Err(AppError::config("code max_chars must be 1..100000"));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let snapshot = snapshot_locked(&transaction, q.project.as_deref())?;
        let view = cursor::View::new(q, &snapshot)?;
        let position = view.decode(q.cursor.as_deref())?;
        let eligible = |id: &str| {
            snapshot
                .document_scopes
                .get(id)
                .is_some_and(|s| q.selection.path.as_ref().is_none_or(|p| p == &s.path))
                && q.document_id.as_ref().is_none_or(|d| d == id)
                && snapshot.document_fingerprints.contains_key(id)
        };
        let mut documents = BTreeMap::<String, (String, i64, String)>::new();
        // Only selected source documents are materialized, after the scoped index.
        let load = |id: &str,
                    documents: &mut BTreeMap<String, (String, i64, String)>|
         -> Result<()> {
            if documents.contains_key(id) {
                return Ok(());
            }
            let (content,metadata,revision,path,project):(String,String,i64,String,Option<String>)=transaction.query_row("SELECT content,metadata_json,revision,COALESCE(source_file,uri),wing FROM documents WHERE id=? AND COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated')",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            let source = source::read(&transaction, &content, &metadata)?.into_owned();
            let scope = &snapshot.document_scopes[id];
            let (expected_revision, hash) = &snapshot.document_fingerprints[id];
            if revision != *expected_revision
                || content_hash(&source) != *hash
                || path != scope.path
                || project != scope.project
            {
                return Err(AppError::Conflict(
                    "source identity changed during code read".into(),
                ));
            }
            documents.insert(id.into(), (source, revision, hash.clone()));
            Ok(())
        };
        let mut spans = Vec::new();
        let mut unresolved = Vec::new();
        if !ids.is_empty() {
            for id in ids.iter().collect::<BTreeSet<_>>() {
                if let Some(symbol) = snapshot
                    .symbols
                    .iter()
                    .find(|s| &s.id == id && eligible(&s.document_id))
                {
                    spans.push(Span {
                        document: symbol.document_id.clone(),
                        start: symbol.start_byte,
                        end: symbol.end_byte,
                        symbol: Some(symbol.clone()),
                        via: "symbol_id".into(),
                    });
                    continue;
                }
                let chunk: Option<(String, i32, i32)> = transaction
                    .query_row(
                        "SELECT document_id,char_start,char_end FROM chunks WHERE id=?",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let mut resolved = false;
                if let Some((document, a, b)) = chunk.filter(|(d, _, _)| eligible(d)) {
                    let body: String = transaction.query_row(
                        "SELECT content FROM chunks WHERE id=?",
                        [id],
                        |r| r.get(0),
                    )?;
                    load(&document, &mut documents)?;
                    let source = &documents[&document].0;
                    if a >= 0 && b >= a {
                        let start =
                            source
                                .char_indices()
                                .nth(a as usize)
                                .map(|v| v.0)
                                .or_else(|| {
                                    (a as usize == source.chars().count()).then_some(source.len())
                                });
                        let end =
                            source
                                .char_indices()
                                .nth(b as usize)
                                .map(|v| v.0)
                                .or_else(|| {
                                    (b as usize == source.chars().count()).then_some(source.len())
                                });
                        if let Some((a, b)) = start
                            .zip(end)
                            .filter(|(a, b)| source.get(*a..*b) == Some(body.as_str()))
                        {
                            spans.push(Span {
                                document,
                                start: a,
                                end: b,
                                symbol: None,
                                via: format!("chunk_id:{id}"),
                            });
                            resolved = true;
                        }
                    }
                }
                if !resolved {
                    unresolved.push(id.clone());
                }
            }
        } else {
            spans = snapshot
                .symbols
                .iter()
                .filter(|s| {
                    eligible(&s.document_id)
                        && q.selection
                            .symbol
                            .as_ref()
                            .is_none_or(|n| n == &s.id || n == &s.name || n == &s.qualified_name)
                })
                .map(|s| Span {
                    document: s.document_id.clone(),
                    start: s.start_byte,
                    end: s.end_byte,
                    symbol: Some(s.clone()),
                    via: "symbol".into(),
                })
                .collect();
        }
        let single = q.selection.symbol.is_some();
        if single && spans.len() != 1 {
            return Ok(
                serde_json::json!({"found":false,"ambiguous":spans.len()>1,"candidates":spans.iter().take(100).map(|s|&s.symbol).collect::<Vec<_>>(),"candidates_total":spans.len(),"candidates_truncated":spans.len()>100,"index_rev":snapshot.index_revision,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors}),
            );
        }
        if ids.is_empty() && !single {
            let mut target_documents: Vec<_> = snapshot
                .document_scopes
                .iter()
                .filter(|(id, s)| {
                    q.selection.path.as_ref().is_none_or(|p| p == &s.path)
                        && q.document_id.as_ref().is_none_or(|d| d == *id)
                })
                .map(|(id, _)| id.clone())
                .collect();
            target_documents.sort();
            if target_documents.len() > 1 {
                return Ok(
                    serde_json::json!({"found":false,"ambiguous":true,"document_candidates":target_documents.iter().take(100).map(|id|serde_json::json!({"document_id":id,"path":snapshot.document_scopes[id].path,"project":snapshot.document_scopes[id].project})).collect::<Vec<_>>(),"document_candidates_total":target_documents.len(),"document_candidates_truncated":target_documents.len()>100,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"index_rev":snapshot.index_revision}),
                );
            }
            if spans.is_empty() {
                for id in target_documents.into_iter().filter(|id| eligible(id)) {
                    load(&id, &mut documents)?;
                    spans.push(Span {
                        start: 0,
                        end: documents[&id].0.len(),
                        document: id,
                        symbol: None,
                        via: "file".into(),
                    });
                }
            }
        }
        spans.sort_by(|a, b| {
            (&a.document, a.start, std::cmp::Reverse(a.end)).cmp(&(
                &b.document,
                b.start,
                std::cmp::Reverse(b.end),
            ))
        });
        spans.dedup_by(|a, b| a.document == b.document && a.start == b.start && a.end == b.end);
        let all = spans.clone();
        let spans_total = spans.len();
        if !q.include_containers {
            let mut document = String::new();
            let mut furthest = None;
            spans.retain(|s| {
                if document != s.document {
                    document = s.document.clone();
                    furthest = None;
                }
                if furthest.is_some_and(|end| s.end <= end) {
                    false
                } else {
                    furthest = Some(s.end);
                    true
                }
            });
        }
        let nested_dropped = spans_total - spans.len();
        let retained_total = spans.len();
        let limit = q.selection.limit.unwrap_or(50).clamp(1, 500);
        if let Some(p) = position {
            if p.span >= spans.len() || p.byte < spans[p.span].start || p.byte > spans[p.span].end {
                return Err(AppError::config("code cursor is outside selected spans"));
            }
        }
        let span_offset = position.map_or(0, |p| p.span);
        let mut remaining = budget;
        let mut items = Vec::new();
        let mut text_truncated = false;
        let mut covered = BTreeMap::<String, Vec<(usize, usize)>>::new();
        let mut segments_remaining = 1000usize;
        let mut overlap_bytes_omitted = 0usize;
        let mut segments_truncated = false;
        let mut next = None;
        for (index, span) in spans.iter().enumerate() {
            let children: Vec<_> = if q.include_containers {
                all.iter()
                    .filter(|p| {
                        p.document == span.document
                            && p.start >= span.start
                            && p.end <= span.end
                            && (p.start > span.start || p.end < span.end)
                    })
                    .map(|s| (s.start, s.end))
                    .collect()
            } else {
                Vec::new()
            };
            let parts = if q.include_containers {
                subtract(span.start, span.end, &children)
            } else {
                vec![(span.start, span.end)]
            };
            let previous = covered.entry(span.document.clone()).or_default();
            let canonical: Vec<_> = parts
                .iter()
                .flat_map(|&(a, b)| subtract(a, b, previous))
                .collect();
            previous.extend(parts.iter().copied());
            previous.sort_unstable();
            if index < span_offset {
                continue;
            }
            if items.len() >= limit || remaining == 0 || segments_remaining == 0 {
                next = Some(cursor::Position {
                    span: index,
                    byte: span.start,
                });
                break;
            }
            load(&span.document, &mut documents)?;
            let (source, revision, hash) = &documents[&span.document];
            if source.get(span.start..span.end).is_none() {
                return Err(AppError::config(
                    "invalid cached code span; rebuild index_code",
                ));
            }
            if q.start_byte.is_some_and(|b| !source.is_char_boundary(b))
                || q.end_byte.is_some_and(|b| !source.is_char_boundary(b))
            {
                return Err(AppError::config(
                    "code byte bounds must be UTF-8 boundaries within source",
                ));
            }
            let resume = position
                .filter(|p| p.span == index)
                .map_or(span.start, |p| p.byte);
            if !source.is_char_boundary(resume) {
                return Err(AppError::config("code cursor is not a UTF-8 boundary"));
            }
            let (lo, hi) = window(source, q.start, q.end);
            let a = span
                .start
                .max(lo)
                .max(q.start_byte.unwrap_or(0))
                .max(resume);
            let b = span.end.min(hi).min(q.end_byte.unwrap_or(source.len()));
            let clip = |(x, y): (usize, usize)| {
                let x = x.max(a);
                let y = y.min(b);
                (x < y).then_some((x, y))
            };
            let visible: Vec<_> = canonical.into_iter().filter_map(clip).collect();
            let before_overlap = parts
                .into_iter()
                .filter_map(clip)
                .map(|(a, b)| b - a)
                .sum::<usize>();
            overlap_bytes_omitted +=
                before_overlap - visible.iter().map(|(a, b)| b - a).sum::<usize>();
            let mut segments = Vec::new();
            let mut truncated = false;
            for (a, b) in visible {
                if segments_remaining == 0 || remaining == 0 {
                    segments_truncated |= segments_remaining == 0;
                    truncated = true;
                    next = Some(cursor::Position {
                        span: index,
                        byte: a,
                    });
                    break;
                }
                let text = &source[a..b];
                let take = text
                    .char_indices()
                    .nth(remaining)
                    .map(|v| v.0)
                    .unwrap_or(text.len());
                let output = &text[..take];
                remaining -= output.chars().count();
                segments_remaining -= 1;
                segments.push(serde_json::json!({"start_byte":a,"end_byte":a+take,"start_line":line(source,a),"end_line":line(source,a+take-1),"code":output}));
                if take < text.len() {
                    truncated = true;
                    next = Some(cursor::Position {
                        span: index,
                        byte: a + take,
                    });
                    break;
                }
            }
            text_truncated |= truncated;
            items.push(serde_json::json!({"document_id":span.document,"path":snapshot.document_scopes[&span.document].path,"project":snapshot.document_scopes[&span.document].project,"revision":revision,"content_hash":hash,"symbol":span.symbol,"via":span.via,"selected_start_byte":span.start,"selected_end_byte":span.end,"outside_window":a>=b,"segments":segments,"text_truncated":truncated}));
            if next.is_some() {
                break;
            }
        }
        let spans_truncated = span_offset + items.len() < retained_total;
        let next_cursor = next.map(|p| view.encode(p));
        let mut result = serde_json::json!({"found":!items.is_empty(),"ambiguous":false,"index_rev":snapshot.index_revision,"spans_total":spans_total,"overlap_bytes_omitted":overlap_bytes_omitted,"segments_truncated":segments_truncated,"max_segments":1000,"source_segments_do_not_overlap":true,"nested_dropped":nested_dropped,"retained_spans_total":retained_total,"spans_truncated":spans_truncated,"text_truncated":text_truncated,"truncated":next_cursor.is_some(),"next_cursor":next_cursor,"read_complete":next.is_none(),"is_continuation":position.is_some(),"span_offset":span_offset,"unresolved_entry_ids":unresolved,"start":q.start,"end":q.end,"start_byte":q.start_byte,"end_byte":q.end_byte,"max_chars":budget,"returned_chars":budget-remaining,"budget_scope":"returned_source_text_only_metadata_and_index_loading_excluded","observation_consistency":"single_project_scoped_read_transaction","text_source":"stored_source","filesystem_read":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors});
        if single {
            let item = &items[0];
            result["symbol"] = item["symbol"].clone();
            result["code"] = item["segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|s| s["code"].as_str())
                .collect::<String>()
                .into();
            result["source_ranges"]=serde_json::json!(item["segments"].as_array().unwrap().iter().map(|s|serde_json::json!({"start_byte":s["start_byte"],"end_byte":s["end_byte"],"start_line":s["start_line"],"end_line":s["end_line"]})).collect::<Vec<_>>());
            result["outside_window"] = item["outside_window"].clone();
        } else {
            result["items"] = serde_json::json!(items);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, Document};
    fn store(source: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("code.duckdb")).unwrap();
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: "file:///p/a.py".into(),
                source_file: Some("/p/a.py".into()),
                content: source.into(),
                wing: Some("alpha".into()),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        (dir, store)
    }
    fn path() -> CodeReadQuery {
        CodeReadQuery {
            selection: CodeQuery {
                path: Some("/p/a.py".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn code_read_single_window_unicode_crlf_and_budget() {
        let (_dir, s) = store("def target():\r\n    return 'αβγ'\r\n");
        let mut q = CodeReadQuery {
            selection: CodeQuery {
                symbol: Some("target".into()),
                ..Default::default()
            },
            start: Some(2),
            end: Some(2),
            ..Default::default()
        };
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["code"], "    return 'αβγ'");
        assert_eq!(r["source_ranges"][0]["start_line"], 2);
        q.max_chars = Some(13);
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["code"].as_str().unwrap().chars().count(), 13);
        assert_eq!(r["text_truncated"], true);
        q.start = Some(50);
        q.end = None;
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["outside_window"], true);
        assert_eq!(r["code"], "");
    }
    #[test]
    fn code_read_file_nested_and_container_partition_preserves_unique_bytes() {
        let body="class C:\n    def first(self):\n        return 1\n    def second(self):\n        return 2\n\ndef tail():\n    return 3\n";
        let (_dir, s) = store(body);
        let mut q = path();
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["nested_dropped"], 2);
        assert_eq!(r["items"].as_array().unwrap().len(), 2);
        q.include_containers = true;
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["nested_dropped"], 0);
        let mut positions = BTreeSet::new();
        for item in r["items"].as_array().unwrap() {
            for span in item["segments"].as_array().unwrap() {
                let a = span["start_byte"].as_u64().unwrap() as usize;
                let b = span["end_byte"].as_u64().unwrap() as usize;
                assert_eq!(span["code"], &body[a..b]);
                for i in a..b {
                    assert!(positions.insert(i), "duplicate byte {i}");
                }
            }
        }
        assert_eq!(r["items"].as_array().unwrap().len(), 4);
        q.selection.limit = Some(1);
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["spans_truncated"], true);
        assert_eq!(r["retained_spans_total"], 4);
    }
    #[test]
    fn code_read_chunks_validate_unicode_offsets_and_remove_overlap() {
        let body = "# αβγ\ndef target():\n    return 1\n";
        let (_dir, s) = store(body);
        for (id, a, b) in [("one", 0, 17), ("two", 5, 24)] {
            s.insert_chunks(&[Chunk {
                id: id.into(),
                document_id: "a".into(),
                chunk_index: 0,
                content: body.chars().skip(a).take(b - a).collect(),
                embedding: vec![],
                char_start: a as i32,
                char_end: b as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
        }
        let q = CodeReadQuery {
            entry_ids: Some(vec!["one".into(), "two".into(), "missing".into()]),
            ..Default::default()
        };
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["unresolved_entry_ids"], serde_json::json!(["missing"]));
        assert_eq!(r["overlap_bytes_omitted"].as_u64().unwrap(), 12);
        let pieces = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|i| i["segments"].as_array().unwrap())
            .map(|s| s["code"].as_str().unwrap())
            .collect::<String>();
        assert_eq!(pieces, body.chars().take(24).collect::<String>());
    }
    #[test]
    fn code_read_path_project_ambiguity_and_stale_exclusion() {
        let (_dir, s) = store("def a(): pass\n");
        s.upsert_document(&Document {
            id: "b".into(),
            uri: "file:///p/b.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def b(): pass\n".into(),
            wing: Some("beta".into()),
            ..Default::default()
        })
        .unwrap();
        s.index_code().unwrap();
        let ambiguous = s.code_read(&path()).unwrap();
        assert_eq!(ambiguous["ambiguous"], true);
        assert_eq!(ambiguous, s.code_read(&path()).unwrap());
        let mut q = path();
        q.project = Some("alpha".into());
        assert_eq!(s.code_read(&q).unwrap()["found"], true);
        s.upsert_document(&Document {
            id: "a".into(),
            uri: "file:///p/a.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def changed(): pass".into(),
            wing: Some("alpha".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(s.code_read(&q).unwrap()["found"], false);
        assert_eq!(s.code_read(&path()).unwrap()["ambiguous"], true);
    }
    #[test]
    fn code_read_validation_file_without_symbols_and_shared_budget() {
        let (_dir, s) = store("# αβγ\r\n# second\r\n");
        let mut q = path();
        q.start = Some(1);
        q.end = Some(1);
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["items"][0]["segments"][0]["code"], "# αβγ\r\n");
        q.start = Some(0);
        assert!(s.code_read(&q).is_err());
        q.start = None;
        q.max_chars = Some(0);
        assert!(s.code_read(&q).is_err());
        q.max_chars = None;
        q.source = Some("disk".into());
        assert!(s.code_read(&q).is_err());
        assert!(s.code_read(&CodeReadQuery::default()).is_err());
        let (_d, s) = store("def a(): return 'αβγ'\ndef b(): return 'δεζ'\n");
        let mut q = path();
        q.max_chars = Some(24);
        let r = s.code_read(&q).unwrap();
        let count = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|i| i["segments"].as_array().unwrap())
            .map(|s| s["code"].as_str().unwrap().chars().count())
            .sum::<usize>();
        assert_eq!(count, 24);
        assert_eq!(r["text_truncated"], true);
    }
    #[test]
    fn code_read_segment_cap_and_invalid_chunks_are_explicit() {
        let source = format!(
            "class C:\n{}",
            (0..1005)
                .map(|i| format!("    def m{i}(self): pass\n"))
                .collect::<String>()
        );
        let (_dir, s) = store(&source);
        let mut q = path();
        q.include_containers = true;
        q.max_chars = Some(100000);
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["segments_truncated"], true);
        assert_eq!(r["truncated"], true);
        let segments = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["segments"].as_array().unwrap().len())
            .sum::<usize>();
        assert_eq!(segments, 1000);
        s.insert_chunks(&[Chunk {
            id: "invalid".into(),
            document_id: "a".into(),
            chunk_index: 0,
            content: "wrong".into(),
            embedding: vec![],
            char_start: 0,
            char_end: 5,
            metadata_json: "{}".into(),
        }])
        .unwrap();
        let q = CodeReadQuery {
            entry_ids: Some(vec!["invalid".into()]),
            ..Default::default()
        };
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["found"], false);
        assert_eq!(r["unresolved_entry_ids"], serde_json::json!(["invalid"]));
        let q = CodeReadQuery {
            entry_ids: Some(vec!["invalid".into()]),
            project: Some("beta".into()),
            ..Default::default()
        };
        assert_eq!(s.code_read(&q).unwrap()["found"], false);
    }
    #[test]
    fn code_cursor_single_large_line_resumes_with_changed_page_budget() {
        let body = format!("def target(): return '{}'\n", "α".repeat(120005));
        let (_dir, s) = store(&body);
        let mut q = CodeReadQuery {
            selection: CodeQuery {
                symbol: Some("target".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut text = String::new();
        let mut pages = 0;
        loop {
            let r = s.code_read(&q).unwrap();
            pages += 1;
            assert!(pages <= 4);
            assert_eq!(r["is_continuation"], q.cursor.is_some());
            text.push_str(r["code"].as_str().unwrap());
            if r["read_complete"] == true {
                assert!(r["next_cursor"].is_null());
                break;
            }
            let next = r["next_cursor"].as_str().unwrap().to_string();
            assert_ne!(q.cursor.as_deref(), Some(next.as_str()));
            q.cursor = Some(next);
            q.max_chars = Some(100000);
        }
        assert_eq!(pages, 3);
        assert_eq!(text, body.trim_end());
    }
    #[test]
    fn code_cursor_partially_overlapping_chunks_do_not_repeat_across_pages() {
        let body = "# αβγ\ndef target():\n    return 1\n";
        let (_dir, s) = store(body);
        for (id, a, b) in [("one", 0, 17), ("two", 5, 24)] {
            s.insert_chunks(&[Chunk {
                id: id.into(),
                document_id: "a".into(),
                chunk_index: 0,
                content: body.chars().skip(a).take(b - a).collect(),
                embedding: vec![],
                char_start: a as i32,
                char_end: b as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
        }
        let mut q = CodeReadQuery {
            entry_ids: Some(vec!["one".into(), "two".into()]),
            selection: CodeQuery {
                limit: Some(1),
                ..Default::default()
            },
            max_chars: Some(4),
            ..Default::default()
        };
        let mut text = String::new();
        let mut seen = BTreeSet::new();
        let mut pages = 0;
        loop {
            let r = s.code_read(&q).unwrap();
            pages += 1;
            assert!(pages < 20);
            for i in r["items"].as_array().unwrap() {
                for p in i["segments"].as_array().unwrap() {
                    let a = p["start_byte"].as_u64().unwrap() as usize;
                    let b = p["end_byte"].as_u64().unwrap() as usize;
                    for pos in a..b {
                        assert!(seen.insert(pos));
                    }
                    text.push_str(p["code"].as_str().unwrap());
                }
            }
            let Some(next) = r["next_cursor"].as_str() else {
                break;
            };
            q.cursor = Some(next.into());
        }
        assert_eq!(text, body.chars().take(24).collect::<String>());
    }
    #[test]
    fn code_cursor_byte_windows_reject_split_utf8_and_support_empty_boundary() {
        let body = "def target(): return 'αβγ'\n";
        let (_dir, s) = store(body);
        let a = body.find('α').unwrap();
        let mut q = CodeReadQuery {
            selection: CodeQuery {
                symbol: Some("target".into()),
                ..Default::default()
            },
            start_byte: Some(a),
            end_byte: Some(a + 4),
            ..Default::default()
        };
        assert_eq!(s.code_read(&q).unwrap()["code"], "αβ");
        q.end_byte = Some(a + 1);
        assert!(s.code_read(&q).is_err());
        q.end_byte = Some(body.len() + 1);
        assert!(s.code_read(&q).is_err());
        q.start_byte = Some(a);
        q.end_byte = Some(a);
        let r = s.code_read(&q).unwrap();
        assert_eq!(r["code"], "");
        assert_eq!(r["read_complete"], true);
    }
    #[test]
    fn code_cursor_changed_queries_or_stale_sources_conflict_before_lookup() {
        let (_dir, s) = store("def target(): return 'old'\n");
        let mut q = CodeReadQuery {
            selection: CodeQuery {
                symbol: Some("target".into()),
                ..Default::default()
            },
            max_chars: Some(3),
            ..Default::default()
        };
        q.cursor = s.code_read(&q).unwrap()["next_cursor"]
            .as_str()
            .map(str::to_string);
        q.include_containers = true;
        assert!(matches!(s.code_read(&q), Err(AppError::Conflict(_))));
        q.include_containers = false;
        let original = q.cursor.clone().unwrap();
        q.cursor = Some("bad".into());
        assert!(s.code_read(&q).is_err());
        let mut parts = original.split(':').map(str::to_string).collect::<Vec<_>>();
        parts[3] = usize::MAX.to_string();
        q.cursor = Some(parts.join(":"));
        assert!(s.code_read(&q).is_err());
        q.cursor = Some(original);
        s.upsert_document(&Document {
            id: "a".into(),
            uri: "file:///p/a.py".into(),
            source_file: Some("/p/a.py".into()),
            content: "def target(): return 'new'\n".into(),
            wing: Some("alpha".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(s.code_read(&q), Err(AppError::Conflict(_))));
        s.index_code().unwrap();
        assert!(matches!(s.code_read(&q), Err(AppError::Conflict(_))));
    }
    #[test]
    fn code_cursor_delimited_ids_match_array_and_have_bounded_parsing() {
        let (_dir, s) = store("def first(): pass\ndef second(): pass\n");
        let symbols = s.code_snapshot().unwrap().symbols;
        let a = &symbols[0].id;
        let b = &symbols[1].id;
        let mut q: CodeReadQuery = serde_json::from_value(
            serde_json::json!({"entry_ids":format!("{a}, \n{b}"),"max_chars":2}),
        )
        .unwrap();
        let r = s.code_read(&q).unwrap();
        let expected = r["next_cursor"].clone();
        q.cursor = r["next_cursor"].as_str().map(str::to_string);
        let mut array: CodeReadQuery =
            serde_json::from_value(serde_json::json!({"entry_ids":[b,a,a],"max_chars":2})).unwrap();
        assert_eq!(s.code_read(&array).unwrap()["next_cursor"], expected);
        array.cursor = q.cursor.clone();
        assert_eq!(s.code_read(&q).unwrap(), s.code_read(&array).unwrap());
        assert!(serde_json::from_value::<CodeReadQuery>(
            serde_json::json!({"entry_ids":"x".repeat(513)})
        )
        .is_err());
        let many: CodeReadQuery =
            serde_json::from_value(serde_json::json!({"entry_ids":"id ".repeat(1000)})).unwrap();
        assert_eq!(many.entry_ids.as_ref().unwrap().len(), 101);
        assert!(s.code_read(&many).is_err());
    }
    #[test]
    fn code_cursor_segment_limit_and_container_metadata_resume_without_missing_bytes() {
        let body = format!(
            "class C:\n{}",
            (0..1005)
                .map(|i| format!("    def m{i}(self): pass\n"))
                .collect::<String>()
        );
        let (_dir, s) = store(&body);
        let mut q = path();
        q.include_containers = true;
        q.max_chars = Some(100000);
        q.selection.limit = Some(500);
        let expected: BTreeSet<_> = s
            .code_snapshot()
            .unwrap()
            .symbols
            .iter()
            .flat_map(|s| s.start_byte..s.end_byte)
            .collect();
        let mut seen = BTreeSet::new();
        let mut pages = 0;
        let mut previous = None;
        loop {
            let r = s.code_read(&q).unwrap();
            pages += 1;
            assert!(pages < 6);
            for i in r["items"].as_array().unwrap() {
                for p in i["segments"].as_array().unwrap() {
                    let a = p["start_byte"].as_u64().unwrap() as usize;
                    let b = p["end_byte"].as_u64().unwrap() as usize;
                    assert_eq!(p["code"], &body[a..b]);
                    for pos in a..b {
                        assert!(seen.insert(pos), "duplicate {pos}");
                    }
                }
            }
            let Some(next) = r["next_cursor"].as_str() else {
                assert_eq!(r["read_complete"], true);
                break;
            };
            assert_ne!(previous.as_deref(), Some(next));
            previous = Some(next.to_string());
            q.cursor = previous.clone();
        }
        assert_eq!(seen, expected);
    }
    #[test]
    fn code_cursor_corrupt_child_boundary_returns_error_without_poisoning_store() {
        let body = "class C:\n    'α'\n    def child(self): pass\n";
        let (_dir, s) = store(body);
        let payload: String = s
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut doc: IndexedDocument = serde_json::from_str(&payload).unwrap();
        doc.symbols
            .iter_mut()
            .find(|s| s.name == "child")
            .unwrap()
            .start_byte = body.find('α').unwrap() + 1;
        s.lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload=? WHERE document_id='a'",
                [serde_json::to_string(&doc).unwrap()],
            )
            .unwrap();
        let mut q = path();
        q.include_containers = true;
        assert!(s.code_read(&q).is_err());
        assert!(s.code_snapshot().is_ok());
    }
}
