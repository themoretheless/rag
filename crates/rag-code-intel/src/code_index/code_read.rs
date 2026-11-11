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
impl<'a> CodeIndex<'a> {
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
        self.store.with_conn(|connection| {
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
                    // Segment bounds come from child spans in the cached index
                    // payload, not only from the validated span and cursor.
                    let Some(text) = source.get(a..b) else {
                        return Err(AppError::config(
                            "invalid cached code span; rebuild index_code",
                        ));
                    };
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
        })
    }
}

#[cfg(test)]
mod tests;
