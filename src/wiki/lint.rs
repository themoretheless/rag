//! Wiki lint: link health, orphans, missing summaries.

use super::*;

/// Lint wiki catalog and graph link health.
pub fn lint_wiki(store: &Store) -> Result<LintReport> {
    let mut issues = Vec::new();
    let documents = store.list_documents()?;
    let wiki_docs = store.list_documents_by_layer("wiki")?;
    let index = store.list_wiki_index()?;
    let graph = store.get_graph_view(crate::models::GraphFilter {
        max_nodes: Some(u32::MAX),
        ..Default::default()
    })?;
    let all_edges = store.list_graph_edges()?;
    let node_by_id: HashMap<&str, &crate::models::GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut incident_nodes = HashSet::new();
    let mut duplicate_wikilinks: BTreeMap<(&str, &str), Vec<&crate::models::GraphEdge>> =
        BTreeMap::new();
    let mut health = LinkHealthCounts {
        documents: documents.len(),
        wiki_pages: wiki_docs.len(),
        graph_nodes: graph.nodes.len(),
        graph_edges: all_edges.len(),
        ..Default::default()
    };

    for edge in &all_edges {
        let source = node_by_id.get(edge.source_id.as_str()).copied();
        let target = node_by_id.get(edge.target_id.as_str()).copied();
        if source.is_some() {
            incident_nodes.insert(edge.source_id.as_str());
        }
        if target.is_some() {
            incident_nodes.insert(edge.target_id.as_str());
        }
        if edge.rel_type != crate::graph::REL_WIKILINK {
            continue;
        }
        health.wikilinks += 1;
        if source.is_none() || target.is_none() {
            health.broken_wikilinks += 1;
            let missing = match (source.is_none(), target.is_none()) {
                (true, true) => "source and target nodes",
                (true, false) => "source node",
                (false, true) => "target node",
                (false, false) => unreachable!(),
            };
            issues.push(LintIssue {
                code: LintCode::BrokenWikilink,
                severity: crate::models::IssueSeverity::Error,
                message: format!(
                    "wikilink edge {} references missing {}; rebuild or remove this edge",
                    edge.id, missing
                ),
                entity_id: Some(edge.id.clone()),
                source_id: Some(edge.source_id.clone()),
                target_id: Some(edge.target_id.clone()),
                rel_type: Some(edge.rel_type.clone()),
                ..Default::default()
            });
        }
        if edge.source_id == edge.target_id {
            health.self_links += 1;
            let label = source.map(|n| n.label.as_str()).unwrap_or(&edge.source_id);
            issues.push(LintIssue {
                code: LintCode::SelfLink,
                severity: crate::models::IssueSeverity::Warn,
                message: format!("'{label}' links to itself; remove the redundant wikilink"),
                entity_id: Some(edge.id.clone()),
                source_id: Some(edge.source_id.clone()),
                target_id: Some(edge.target_id.clone()),
                rel_type: Some(edge.rel_type.clone()),
                ..Default::default()
            });
        }
        duplicate_wikilinks
            .entry((edge.source_id.as_str(), edge.target_id.as_str()))
            .or_default()
            .push(edge);
    }

    for ((source_id, target_id), edges) in duplicate_wikilinks {
        if edges.len() < 2 {
            continue;
        }
        health.duplicate_link_groups += 1;
        health.duplicate_link_occurrences += edges.len() - 1;
        let source_label = node_by_id
            .get(source_id)
            .map(|n| n.label.as_str())
            .unwrap_or(source_id);
        let target_label = node_by_id
            .get(target_id)
            .map(|n| n.label.as_str())
            .unwrap_or(target_id);
        issues.push(LintIssue {
            code: LintCode::DuplicateWikilink,
            severity: crate::models::IssueSeverity::Warn,
            message: format!(
                "'{source_label}' links to '{target_label}' {} times; keep one occurrence unless repetition is intentional",
                edges.len()
            ),
            entity_id: Some(edges[0].id.clone()),
            source_id: Some(source_id.to_string()),
            target_id: Some(target_id.to_string()),
            rel_type: Some(crate::graph::REL_WIKILINK.into()),
            occurrences: Some(edges.len()),
            ..Default::default()
        });
    }

    if wiki_docs.is_empty() {
        issues.push(LintIssue {
            code: LintCode::WikiEmpty,
            severity: crate::models::IssueSeverity::Info,
            message: "no wiki pages yet".into(),
            ..Default::default()
        });
    }

    // Pages missing from index
    for d in &wiki_docs {
        let in_index = index
            .iter()
            .any(|e| e.page_id.as_deref() == Some(d.id.as_str()));
        if !in_index {
            issues.push(LintIssue {
                code: LintCode::MissingIndex,
                severity: crate::models::IssueSeverity::Warn,
                message: format!("wiki page {} not in wiki_index", d.uri),
                entity_id: Some(d.id.clone()),
                ..Default::default()
            });
        }
    }

    // Index entries pointing nowhere
    for e in &index {
        if let Some(pid) = &e.page_id {
            if store.get_document(pid)?.is_none() {
                issues.push(LintIssue {
                    code: LintCode::StaleIndex,
                    severity: crate::models::IssueSeverity::Warn,
                    message: format!("index entry {} points to missing page {}", e.slug, pid),
                    entity_id: Some(e.id.clone()),
                    expected_label: Some(e.slug.clone()),
                    ..Default::default()
                });
            }
        }
    }

    // Unresolved stubs in graph (include who links + expected label)
    for n in graph.nodes.iter().filter(|n| crate::models::NodeKind::Stub.is(&n.kind)) {
        if !n.resolved {
            health.unresolved_targets += 1;
            let bl = store
                .backlinks(&n.id)
                .unwrap_or_else(|_| crate::models::GraphView {
                    nodes: vec![],
                    edges: vec![],
                });
            let referenced_by: Vec<String> = bl
                .nodes
                .iter()
                .filter(|src| src.id != n.id)
                .map(|src| {
                    if src.label.is_empty() {
                        src.id.clone()
                    } else {
                        src.label.clone()
                    }
                })
                .collect();
            let refs_note = if referenced_by.is_empty() {
                "no incoming edges".to_string()
            } else {
                format!("referenced by: {}", referenced_by.join(", "))
            };
            issues.push(LintIssue {
                code: LintCode::UnresolvedStub,
                severity: crate::models::IssueSeverity::Info,
                message: format!(
                    "stub node '{}' unresolved ({}); write wiki/document with title or wiki:// slug matching this label",
                    n.label, refs_note
                ),
                entity_id: Some(n.id.clone()),
                expected_label: Some(n.label.clone()),
                referenced_by,
                ..Default::default()
            });
        }
    }

    // Documents with no graph node, or document nodes with no incident edges.
    let node_by_document: HashMap<&str, &crate::models::GraphNode> = graph
        .nodes
        .iter()
        .filter_map(|node| node.document_id.as_deref().map(|id| (id, node)))
        .collect();
    for doc in &documents {
        let node = node_by_document.get(doc.id.as_str()).copied();
        if node
            .map(|n| incident_nodes.contains(n.id.as_str()))
            .unwrap_or(false)
        {
            continue;
        }
        let is_wiki = doc.layer == LAYER_WIKI;
        health.orphan_documents += 1;
        if is_wiki {
            health.orphan_wiki_pages += 1;
        }
        issues.push(LintIssue {
            code: if is_wiki {
                LintCode::OrphanWikiPage
            } else {
                LintCode::OrphanDocument
            },
            severity: crate::models::IssueSeverity::Info,
            message: format!(
                "{} has no incoming or outgoing graph links; add a relevant wikilink or archive it",
                doc.uri
            ),
            entity_id: Some(doc.id.clone()),
            ..Default::default()
        });
    }

    // Raw without related wiki (compile debt). Prefer status/doctor counts for
    // the full number; lint ranks the (wing, room) shelves by true debt so a bounded
    // sample cannot pass itself off as the queue, and names the filter that pages one.
    const UNCOMPILED_SAMPLE_LIMIT: usize = 25;
    const UNCOMPILED_SHELF_LIMIT: usize = 8;
    let layer_health = store.layer_health_counts()?;
    health.uncompiled_raw_count = layer_health.uncompiled_raw_count as usize;
    if health.uncompiled_raw_count > 0 {
        let sample = store.list_uncompiled_raw_sample(UNCOMPILED_SAMPLE_LIMIT)?;
        health.uncompiled_raw_sample_count = sample.len();
        let queue = store.uncompiled_raw_shelves(UNCOMPILED_SHELF_LIMIT)?;
        let shelves: Vec<String> = queue
            .shelves
            .iter()
            .map(|shelf| {
                format!(
                    "{}/{}={}",
                    shelf.wing.as_deref().unwrap_or("_"),
                    shelf.room.as_deref().unwrap_or("_"),
                    shelf.docs
                )
            })
            .collect();
        issues.push(LintIssue {
            code: LintCode::RawUncompiled,
            severity: crate::models::IssueSeverity::Info,
            message: format!(
                "{} uncompiled raw source(s) in {} wing/room shelf(ves); biggest first: {}. Page a shelf with list_documents(layer=raw, wing, room, compiled=false) and close it via bounded compile / file_answer, not a full corpus pass.",
                health.uncompiled_raw_count,
                queue.total_shelves,
                if shelves.is_empty() {
                    "n/a".into()
                } else {
                    shelves.join(", ")
                }
            ),
            entity_id: sample.first().map(|r| r.document_id.clone()),
            referenced_by: sample
                .iter()
                .map(|r| {
                    format!(
                        "{} ({} / {})",
                        r.uri,
                        r.wing.as_deref().unwrap_or("_"),
                        r.room.as_deref().unwrap_or("_")
                    )
                })
                .collect(),
            ..Default::default()
        });
    }

    let healthy = !issues
        .iter()
        .any(|issue| issue.severity >= crate::models::IssueSeverity::Warn);
    Ok(LintReport {
        issue_count: issues.len(),
        issues,
        healthy,
        health,
    })
}
