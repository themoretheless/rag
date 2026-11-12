//! Lean document catalog and project dashboard queries over `lin`.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::lin_engine::{
    LinReader, cell_bool, cell_f64, cell_nonempty_string, cell_opt_string, cell_str, cell_string,
    cell_ts_or_now,
};
use super::store::{Store, compiled_document_ids};
use crate::error::{AppError, Result};
use crate::models::DrawerListItem;

pub use rag_mcp_core::ProjectHome;

pub const DEFAULT_CATALOG_PAGE_SIZE: usize = 50;
pub const MAX_CATALOG_PAGE_SIZE: usize = 200;

/// Fast non-allocating UTC timestamp parser into epoch microseconds (`i64`) for `YYYY-MM-DD[T ]HH:MM:SS[.f][Z]`.
/// Falls back to `parse_db_timestamp` only for non-UTC offsets or unusual formats!
#[inline]
fn parse_ts_micros_fast(raw: &str, fallback_now_us: i64) -> i64 {
    let b = raw.as_bytes();
    if b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b' ' || b[10] == b'T')
        && b[13] == b':'
        && b[16] == b':'
    {
        let d2 = |i: usize| -> Option<i64> {
            let hi = b[i].wrapping_sub(b'0');
            let lo = b[i + 1].wrapping_sub(b'0');
            if hi <= 9 && lo <= 9 {
                Some(i64::from(hi) * 10 + i64::from(lo))
            } else {
                None
            }
        };
        let d4 = |i: usize| -> Option<i64> { Some(d2(i)? * 100 + d2(i + 2)?) };
        if let (Some(y), Some(m), Some(d), Some(hh), Some(mm), Some(ss)) =
            (d4(0), d2(5), d2(8), d2(11), d2(14), d2(17))
            && (1..=12).contains(&m)
            && (1..=31).contains(&d)
            && hh < 24
            && mm < 60
            && ss <= 60
        {
            let mut pos = 19usize;
            let mut micros = 0i64;
            if pos < b.len() && b[pos] == b'.' {
                pos += 1;
                let mut mul = 100_000i64;
                while pos < b.len() && b[pos].is_ascii_digit() {
                    if mul > 0 {
                        micros += i64::from(b[pos] - b'0') * mul;
                        mul /= 10;
                    }
                    pos += 1;
                }
            }
            let pure_utc = pos == b.len() || (pos + 1 == b.len() && b[pos] == b'Z');
            if pure_utc {
                // Howard Hinnant civil-from-days inverse (`days_from_civil`)
                let y_adj = y - i64::from(m <= 2);
                let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
                let yoe = y_adj - era * 400;
                let mp = m + if m > 2 { -3 } else { 9 };
                let doy = (153 * mp + 2) / 5 + d - 1;
                let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
                let days = era * 146_097 + doe - 719_468;
                let secs = days * 86_400 + hh * 3_600 + mm * 60 + ss;
                return secs.saturating_mul(1_000_000).saturating_add(micros);
            }
        }
    }
    crate::util::parse_db_timestamp(raw)
        .map(|dt| dt.timestamp_micros())
        .unwrap_or(fallback_now_us)
}

#[inline]
fn contains_ascii_case_insensitive(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let hb = haystack.as_bytes();
        let nb = needle_lower.as_bytes();
        if nb.len() > hb.len() {
            return false;
        }
        return hb
            .windows(nb.len())
            .any(|w| w.iter().zip(nb).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    haystack.to_lowercase().contains(needle_lower)
}

const CATALOG_DOC_COLS: &[&str] = &[
    "id",
    "uri",
    "title",
    "wing",
    "room",
    "source_file",
    "layer",
    "kind",
    "content_hash",
    "pinned",
    "boost",
    "status",
    "created_at",
    "updated_at",
];

#[derive(Clone)]
struct CatalogTopEntry {
    pinned: bool,
    boost: f64,
    updated_us: i64,
    /// Lazy tie-breakers allocated ONLY on exact `(pinned, boost, updated_us)` collision!
    title_tie: Option<Box<str>>,
    id_tie: Option<Box<str>>,
    /// `usize::MAX` means `MemTable` row (`row_id` = memtable index).
    seg_idx: usize,
    row_id: u32,
}

#[inline]
fn cmp_catalog_rank_fast(a: &CatalogTopEntry, b: &CatalogTopEntry) -> Ordering {
    // Returns `Ordering::Less` when `a` ranks BEFORE (better than) `b`!
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| b.boost.total_cmp(&a.boost))
        .then_with(|| b.updated_us.cmp(&a.updated_us))
}

impl PartialEq for CatalogTopEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for CatalogTopEntry {}

impl PartialOrd for CatalogTopEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CatalogTopEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap on `CatalogTopEntry`: `Greater` means WORSE rank so `.peek()` / `.pop()` evicts the worst item!
        cmp_catalog_rank_fast(self, other)
            .then_with(|| {
                let st = self.title_tie.as_deref().unwrap_or("");
                let ot = other.title_tie.as_deref().unwrap_or("");
                st.cmp(ot)
            })
            .then_with(|| {
                let si = self.id_tie.as_deref().unwrap_or("");
                let oi = other.id_tie.as_deref().unwrap_or("");
                si.cmp(oi)
            })
    }
}

fn hydrate_drawer_item(row: &lin::Row) -> DrawerListItem {
    DrawerListItem {
        id: cell_string(row, "id"),
        uri: cell_string(row, "uri"),
        title: cell_string(row, "title"),
        wing: cell_opt_string(row, "wing"),
        room: cell_opt_string(row, "room"),
        source_file: cell_opt_string(row, "source_file"),
        layer: cell_nonempty_string(row, "layer").unwrap_or_else(|| "raw".into()),
        kind: cell_nonempty_string(row, "kind").unwrap_or_else(|| "document".into()),
        content_hash: cell_opt_string(row, "content_hash"),
        pinned: cell_bool(row, "pinned", false),
        boost: cell_f64(row, "boost", 1.0),
        status: cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into()),
        created_at: cell_ts_or_now(row, "created_at"),
        updated_at: cell_ts_or_now(row, "updated_at"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct DocumentCatalogFilter {
    pub q: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub source_file: Option<String>,
    pub layer: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    /// `Some(true)` keeps only documents a resolved wiki page links to, `Some(false)`
    /// only those without that compile link - i.e. the raw-side compile queue.
    pub compiled: Option<bool>,
    pub include_archived: bool,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentCatalogPage {
    pub items: Vec<DrawerListItem>,
    pub total: u64,
}

impl Store {
    /// Return one lean catalog page without loading document bodies.
    pub fn list_document_catalog(
        &self,
        filter: &DocumentCatalogFilter,
    ) -> Result<DocumentCatalogPage> {
        let r = self.read()?;
        let compiled_ids = match filter.compiled {
            Some(_) => Some(compiled_document_ids(&r)?),
            None => None,
        };

        let wing_f = filter.wing.as_deref().and_then(clean);
        let room_f = filter.room.as_deref().and_then(clean);
        let source_file_f = filter.source_file.as_deref().and_then(clean);
        let layer_f = filter.layer.as_deref().and_then(clean);
        let kind_f = filter.kind.as_deref().and_then(clean);
        let status_f = filter.status.as_deref().and_then(clean);
        let q_lower = filter.q.as_deref().and_then(clean).map(str::to_lowercase);

        let limit = filter.limit.clamp(1, MAX_CATALOG_PAGE_SIZE);
        let top_cap = filter.offset.saturating_add(limit).max(1);

        // Ultra-fast columnar Top-K Heap path on `DiskEngineV2`:
        // Avoids allocating `DrawerListItem` (9 Strings) + `chrono` parsing for 99.7% of non-winning rows!
        if let Some(eng) = r.disk_v2()
            && eng.has_table("documents")
        {
            let now_us = Utc::now().timestamp_micros();
            let segs = eng.table_segments("documents");
            let mut total = 0u64;
            let mut heap: BinaryHeap<CatalogTopEntry> =
                BinaryHeap::with_capacity(top_cap.min(4_096) + 1);

            // Helper to lazily populate `title_tie` + `id_tie` on an existing heap entry or candidate
            let populate_ties = |entry: &mut CatalogTopEntry| -> Result<()> {
                if entry.title_tie.is_some() && entry.id_tie.is_some() {
                    return Ok(());
                }
                if entry.seg_idx == usize::MAX {
                    if let Some(row) = eng.get_memtable_row("documents", entry.row_id as usize) {
                        entry.title_tie = Some(cell_str(row, "title").unwrap_or("").into());
                        entry.id_tie = Some(cell_str(row, "id").unwrap_or("").into());
                    }
                } else if let Some(seg) = segs.get(entry.seg_idx) {
                    let mut lbt = None;
                    let mut lbi = None;
                    let pt = seg.col_pos("title");
                    let pi = seg.col_pos("id");
                    let t = seg
                        .borrow_col_str(pt, entry.row_id, &mut lbt)?
                        .unwrap_or("");
                    let i = seg
                        .borrow_col_str(pi, entry.row_id, &mut lbi)?
                        .unwrap_or("");
                    entry.title_tie = Some(t.into());
                    entry.id_tie = Some(i.into());
                }
                Ok(())
            };

            let mut push_candidate = |mut cand: CatalogTopEntry| -> Result<()> {
                total += 1;
                if heap.len() < top_cap {
                    heap.push(cand);
                    return Ok(());
                }
                let cmp_fast = heap
                    .peek()
                    .map(|worst| cmp_catalog_rank_fast(&cand, worst))
                    .unwrap_or(Ordering::Less);
                match cmp_fast {
                    Ordering::Greater => {
                        // Candidate is strictly worse than the current worst item in Top-K!
                        return Ok(());
                    }
                    Ordering::Less => {
                        heap.pop();
                        heap.push(cand);
                    }
                    Ordering::Equal => {
                        // Rare exact `(pinned, boost, updated_us)` tie: lazily hydrate `title` & `id`!
                        populate_ties(&mut cand)?;
                        let mut worst_owned = heap.pop().unwrap();
                        populate_ties(&mut worst_owned)?;
                        if cand < worst_owned {
                            heap.push(cand);
                        } else {
                            heap.push(worst_owned);
                        }
                    }
                }
                Ok(())
            };

            // 1. Scan immutable mmap segments (via secondary index when filtered on source_file / wing / layer)
            for (s_idx, seg) in segs.iter().enumerate() {
                let sid = seg.meta.seg_id;
                let p_wing = seg.col_pos("wing");
                let p_room = seg.col_pos("room");
                let p_sf = seg.col_pos("source_file");
                let p_layer = seg.col_pos("layer");
                let p_kind = seg.col_pos("kind");
                let p_status = seg.col_pos("status");
                let p_id = seg.col_pos("id");
                let p_title = seg.col_pos("title");
                let p_uri = seg.col_pos("uri");
                let p_upd = seg.col_pos("updated_at");

                let mut lb_wing = None;
                let mut lb_room = None;
                let mut lb_sf = None;
                let mut lb_layer = None;
                let mut lb_kind = None;
                let mut lb_status = None;
                let mut lb_id = None;
                let mut lb_title = None;
                let mut lb_uri = None;
                let mut lb_upd = None;

                let mut eval_seg_rid = |rid: u32| -> Result<()> {
                    if let Some(val) = wing_f
                        && seg.borrow_col_str(p_wing, rid, &mut lb_wing)? != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = room_f
                        && seg.borrow_col_str(p_room, rid, &mut lb_room)? != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = source_file_f
                        && seg.borrow_col_str(p_sf, rid, &mut lb_sf)? != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = layer_f
                        && seg.borrow_col_str(p_layer, rid, &mut lb_layer)? != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = kind_f
                        && seg.borrow_col_str(p_kind, rid, &mut lb_kind)? != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = status_f {
                        if seg.borrow_col_str(p_status, rid, &mut lb_status)? != Some(val) {
                            return Ok(());
                        }
                    } else if !filter.include_archived {
                        let st = seg
                            .borrow_col_str(p_status, rid, &mut lb_status)?
                            .unwrap_or("active");
                        if matches!(st, "archived" | "tombstone") {
                            return Ok(());
                        }
                    }

                    if let (Some(want_compiled), Some(compiled_set)) =
                        (filter.compiled, &compiled_ids)
                    {
                        let id_s = seg.borrow_col_str(p_id, rid, &mut lb_id)?.unwrap_or("");
                        if compiled_set.contains(id_s) != want_compiled {
                            return Ok(());
                        }
                    }

                    if let Some(ref ql) = q_lower {
                        let title_s = seg
                            .borrow_col_str(p_title, rid, &mut lb_title)?
                            .unwrap_or("");
                        let uri_s = seg.borrow_col_str(p_uri, rid, &mut lb_uri)?.unwrap_or("");
                        let sf_s = seg.borrow_col_str(p_sf, rid, &mut lb_sf)?.unwrap_or("");
                        if !contains_ascii_case_insensitive(title_s, ql)
                            && !contains_ascii_case_insensitive(uri_s, ql)
                            && !contains_ascii_case_insensitive(sf_s, ql)
                        {
                            // Fallback for multi-field newline-spanning query
                            let combined = format!("{title_s}\n{uri_s}\n{sf_s}").to_lowercase();
                            if !combined.contains(ql) {
                                return Ok(());
                            }
                        }
                    }

                    let pinned = seg.get_bool_opt(rid, "pinned").unwrap_or(false);
                    let boost = seg.get_f64_opt(rid, "boost").unwrap_or(1.0);
                    let updated_us = seg
                        .borrow_col_str(p_upd, rid, &mut lb_upd)?
                        .map(|s| parse_ts_micros_fast(s, now_us))
                        .unwrap_or(now_us);

                    push_candidate(CatalogTopEntry {
                        pinned,
                        boost,
                        updated_us,
                        title_tie: None,
                        id_tie: None,
                        seg_idx: s_idx,
                        row_id: rid,
                    })
                };

                if let Some(val) = source_file_f {
                    for rid in eng.segment_matching_rids(seg, "source_file", val) {
                        eval_seg_rid(rid)?;
                    }
                } else if let Some(val) = wing_f {
                    for rid in eng.segment_matching_rids(seg, "wing", val) {
                        eval_seg_rid(rid)?;
                    }
                } else if let Some(val) = layer_f {
                    for rid in eng.segment_matching_rids(seg, "layer", val) {
                        eval_seg_rid(rid)?;
                    }
                } else {
                    for rid in 0..seg.meta.row_count {
                        if !eng.is_segment_row_dead(sid, rid) {
                            eval_seg_rid(rid)?;
                        }
                    }
                }
            }

            // 2. Scan live MemTable rows
            let mut app_err: Option<AppError> = None;
            eng.for_each_memtable_row("documents", |mt_idx, row| {
                let res = (|| -> Result<()> {
                    if let Some(val) = wing_f
                        && cell_str(row, "wing") != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = room_f
                        && cell_str(row, "room") != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = source_file_f
                        && cell_str(row, "source_file") != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = layer_f
                        && cell_str(row, "layer") != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = kind_f
                        && cell_str(row, "kind") != Some(val)
                    {
                        return Ok(());
                    }
                    if let Some(val) = status_f {
                        if cell_str(row, "status") != Some(val) {
                            return Ok(());
                        }
                    } else if !filter.include_archived {
                        let st = cell_str(row, "status").unwrap_or("active");
                        if matches!(st, "archived" | "tombstone") {
                            return Ok(());
                        }
                    }
                    if let (Some(want_compiled), Some(compiled_set)) =
                        (filter.compiled, &compiled_ids)
                    {
                        let id_s = cell_str(row, "id").unwrap_or("");
                        if compiled_set.contains(id_s) != want_compiled {
                            return Ok(());
                        }
                    }
                    if let Some(ref ql) = q_lower {
                        let title_s = cell_str(row, "title").unwrap_or("");
                        let uri_s = cell_str(row, "uri").unwrap_or("");
                        let sf_s = cell_str(row, "source_file").unwrap_or("");
                        if !contains_ascii_case_insensitive(title_s, ql)
                            && !contains_ascii_case_insensitive(uri_s, ql)
                            && !contains_ascii_case_insensitive(sf_s, ql)
                        {
                            let combined = format!("{title_s}\n{uri_s}\n{sf_s}").to_lowercase();
                            if !combined.contains(ql) {
                                return Ok(());
                            }
                        }
                    }
                    let pinned = cell_bool(row, "pinned", false);
                    let boost = cell_f64(row, "boost", 1.0);
                    let updated_us = cell_str(row, "updated_at")
                        .map(|s| parse_ts_micros_fast(s, now_us))
                        .unwrap_or(now_us);
                    push_candidate(CatalogTopEntry {
                        pinned,
                        boost,
                        updated_us,
                        title_tie: None,
                        id_tie: None,
                        seg_idx: usize::MAX,
                        row_id: mt_idx as u32,
                    })
                })();
                if let Err(e) = res {
                    app_err = Some(e);
                    return Ok(false);
                }
                Ok(true)
            })?;
            if let Some(e) = app_err {
                return Err(e);
            }

            // 3. Hydrate full `DrawerListItem` ONLY for the `<= top_cap` winners, sort exact, and slice page!
            let mut winners: Vec<DrawerListItem> = Vec::with_capacity(heap.len());
            for entry in heap {
                let row = if entry.seg_idx == usize::MAX {
                    eng.get_memtable_row("documents", entry.row_id as usize)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    segs[entry.seg_idx].materialize_projected(entry.row_id, CATALOG_DOC_COLS)?
                };
                winners.push(hydrate_drawer_item(&row));
            }
            winners.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then_with(|| b.boost.total_cmp(&a.boost))
                    .then_with(|| b.updated_at.cmp(&a.updated_at))
                    .then_with(|| a.title.cmp(&b.title))
                    .then_with(|| a.id.cmp(&b.id))
            });
            let items = winners
                .into_iter()
                .skip(filter.offset)
                .take(limit)
                .collect();
            return Ok(DocumentCatalogPage { items, total });
        }

        // Legacy / `:memory:` fallback path
        let mut matched: Vec<DrawerListItem> = Vec::new();
        let mut visit = |row: &lin::Row| -> Result<()> {
            if let Some(val) = wing_f {
                if cell_str(row, "wing") != Some(val) {
                    return Ok(());
                }
            }
            if let Some(val) = room_f {
                if cell_str(row, "room") != Some(val) {
                    return Ok(());
                }
            }
            if let Some(val) = source_file_f {
                if cell_str(row, "source_file") != Some(val) {
                    return Ok(());
                }
            }
            if let Some(val) = layer_f {
                if cell_str(row, "layer") != Some(val) {
                    return Ok(());
                }
            }
            if let Some(val) = kind_f {
                if cell_str(row, "kind") != Some(val) {
                    return Ok(());
                }
            }
            if let Some(val) = status_f {
                if cell_str(row, "status") != Some(val) {
                    return Ok(());
                }
            } else if !filter.include_archived {
                let st = cell_str(row, "status").unwrap_or("active");
                if matches!(st, "archived" | "tombstone") {
                    return Ok(());
                }
            }

            let id = cell_string(row, "id");
            if let (Some(want_compiled), Some(compiled_set)) = (filter.compiled, &compiled_ids) {
                let is_compiled = compiled_set.contains(&id);
                if is_compiled != want_compiled {
                    return Ok(());
                }
            }

            let title = cell_string(row, "title");
            let uri = cell_string(row, "uri");
            let source_file = cell_opt_string(row, "source_file");

            if let Some(ref ql) = q_lower {
                let haystack = format!(
                    "{}\n{}\n{}",
                    title,
                    uri,
                    source_file.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                if !haystack.contains(ql) {
                    return Ok(());
                }
            }

            matched.push(hydrate_drawer_item(row));
            Ok(())
        };

        if let Some(val) = source_file_f {
            r.for_each_by_eq_projected(
                "documents",
                "source_file",
                val,
                Some(CATALOG_DOC_COLS),
                |row| {
                    visit(row)?;
                    Ok(true)
                },
            )?;
        } else if let Some(val) = wing_f {
            r.for_each_by_eq_projected("documents", "wing", val, Some(CATALOG_DOC_COLS), |row| {
                visit(row)?;
                Ok(true)
            })?;
        } else if let Some(val) = layer_f {
            r.for_each_by_eq_projected("documents", "layer", val, Some(CATALOG_DOC_COLS), |row| {
                visit(row)?;
                Ok(true)
            })?;
        } else {
            r.scan_each_projected("documents", CATALOG_DOC_COLS, |row| {
                visit(row)?;
                Ok(true)
            })?;
        }

        matched.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.boost.total_cmp(&a.boost))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });

        let total = matched.len() as u64;
        let items = matched
            .into_iter()
            .skip(filter.offset)
            .take(limit)
            .collect();

        Ok(DocumentCatalogPage { items, total })
    }

    /// Aggregate the useful health/freshness inventory for one project.
    pub fn project_home(&self, project_id: &str) -> Result<ProjectHome> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::config("project must be non-empty"));
        }

        let (
            documents,
            chunks,
            raw_documents,
            wiki_documents,
            diary_documents,
            archived_documents,
            rooms,
            source_files,
            graph_nodes,
            graph_edges,
            unresolved_links,
            last_updated_at,
        ) = {
            let r = self.read()?;
            let mut doc_rows = Vec::new();
            r.for_each_by_eq_projected(
                "documents",
                "wing",
                project_id,
                Some(&[
                    "id",
                    "layer",
                    "kind",
                    "status",
                    "room",
                    "source_file",
                    "updated_at",
                ]),
                |row| {
                    doc_rows.push(row.clone());
                    Ok(true)
                },
            )?;

            let mut documents = 0u64;
            let mut raw_documents = 0u64;
            let mut wiki_documents = 0u64;
            let mut diary_documents = 0u64;
            let mut archived_documents = 0u64;
            let mut distinct_rooms: HashSet<String> = HashSet::new();
            let mut distinct_files: HashSet<String> = HashSet::new();
            let mut last_updated_at: Option<DateTime<Utc>> = None;
            let mut chunks = 0u64;
            let mut graph_nodes = 0u64;
            let mut source_node_ids: Vec<String> = Vec::new();

            for row in &doc_rows {
                documents += 1;
                let layer = cell_str(row, "layer");
                let kind = cell_str(row, "kind").unwrap_or_default();
                let status = cell_str(row, "status").unwrap_or("active");

                if layer.unwrap_or("raw") == "raw" {
                    raw_documents += 1;
                }
                if layer == Some("wiki") {
                    wiki_documents += 1;
                }
                if layer == Some("diary") || kind == "diary" {
                    diary_documents += 1;
                }
                if matches!(status, "archived" | "tombstone") {
                    archived_documents += 1;
                }
                if let Some(rm) = cell_str(row, "room")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    distinct_rooms.insert(rm.to_string());
                }
                if let Some(sf) = cell_str(row, "source_file")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    distinct_files.insert(sf.to_string());
                }
                let updated = cell_ts_or_now(row, "updated_at");
                if last_updated_at.is_none_or(|prev| updated > prev) {
                    last_updated_at = Some(updated);
                }

                if let Some(doc_id) = cell_str(row, "id") {
                    chunks += r.count_by_eq("chunks", "document_id", doc_id)? as u64;
                    r.for_each_by_eq_projected(
                        "graph_nodes",
                        "document_id",
                        doc_id,
                        Some(&["id"]),
                        |n| {
                            graph_nodes += 1;
                            if let Some(nid) = cell_str(n, "id") {
                                source_node_ids.push(nid.to_string());
                            }
                            Ok(true)
                        },
                    )?;
                }
            }

            let mut graph_edges = 0u64;
            let mut resolved_cache: std::collections::HashMap<String, bool> =
                std::collections::HashMap::new();
            let mut unresolved_targets: HashSet<String> = HashSet::new();
            for nid in &source_node_ids {
                r.for_each_by_eq_projected(
                    "graph_edges",
                    "source_id",
                    nid.as_str(),
                    Some(&["target_id"]),
                    |edge_row| {
                        graph_edges += 1;
                        if let Some(tid) = cell_str(edge_row, "target_id") {
                            let is_resolved = if let Some(&cached) = resolved_cache.get(tid) {
                                cached
                            } else {
                                let res = r
                                    .get_projected_by_id("graph_nodes", tid, &["resolved"])?
                                    .is_none_or(|tr| cell_bool(&tr, "resolved", false));
                                resolved_cache.insert(tid.to_string(), res);
                                res
                            };
                            if !is_resolved {
                                unresolved_targets.insert(tid.to_string());
                            }
                        }
                        Ok(true)
                    },
                )?;
            }

            (
                documents,
                chunks,
                raw_documents,
                wiki_documents,
                diary_documents,
                archived_documents,
                distinct_rooms.len() as u64,
                distinct_files.len() as u64,
                graph_nodes,
                graph_edges,
                unresolved_targets.len() as u64,
                last_updated_at,
            )
        };

        let source_roots = self.list_source_roots(Some(project_id))?;
        Ok(ProjectHome {
            project_id: project_id.to_string(),
            documents,
            chunks,
            raw_documents,
            wiki_documents,
            diary_documents,
            archived_documents,
            rooms,
            source_files,
            graph_nodes,
            graph_edges,
            unresolved_links,
            source_roots,
            last_updated_at,
        })
    }
}

fn clean(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, Document, GraphEdge, GraphNode};

    /// The compile queue: the catalog's `compiled` filter must agree with the
    /// `uncompiled_raw_count` metric, page one shelf past the page cap, and rank shelves
    /// by real debt instead of a fixed sample.
    #[test]
    fn catalog_compiled_filter_pages_the_compile_debt() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("compile-queue.lin")).unwrap();
        for (id, room) in [("raw-a", "src"), ("raw-b", "src"), ("raw-c", "tests")] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///dodo/{id}.cs"),
                    title: id.into(),
                    content: "body".into(),
                    wing: Some("dodo".into()),
                    room: Some(room.into()),
                    layer: "raw".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        store
            .upsert_document(&Document {
                id: "page".into(),
                uri: "wiki://dodo".into(),
                title: "Dodo".into(),
                content: "compiled page".into(),
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Default::default()
            })
            .unwrap();
        for (id, document_id, uri) in [
            ("node-page", "page", "wiki://dodo"),
            ("node-raw-a", "raw-a", "file:///dodo/raw-a.cs"),
        ] {
            store
                .upsert_graph_node(&GraphNode {
                    id: id.into(),
                    kind: "document".into(),
                    label: document_id.into(),
                    document_id: Some(document_id.into()),
                    uri: Some(uri.into()),
                    resolved: true,
                    metadata_json: "{}".into(),
                })
                .unwrap();
        }
        store
            .insert_graph_edges(&[GraphEdge {
                id: "edge-page-raw-a".into(),
                source_id: "node-page".into(),
                target_id: "node-raw-a".into(),
                rel_type: "related".into(),
                weight: 1.0,
                context: None,
            }])
            .unwrap();

        let page = |room: Option<&str>, offset: usize| {
            store
                .list_document_catalog(&DocumentCatalogFilter {
                    layer: Some("raw".into()),
                    room: room.map(str::to_string),
                    compiled: Some(false),
                    limit: 1,
                    offset,
                    ..Default::default()
                })
                .unwrap()
        };
        let debt = page(None, 0);
        assert_eq!(debt.total, 2);
        assert_eq!(debt.items.len(), 1);
        assert_eq!(
            store.layer_health_counts().unwrap().uncompiled_raw_count,
            debt.total
        );
        // The second page names the other doc, so a 17 000-doc shelf is walkable.
        assert_ne!(page(None, 1).items[0].id, debt.items[0].id);
        assert_eq!(page(Some("tests"), 0).total, 1);
        assert_eq!(page(Some("missing-room"), 0).total, 0);

        let compiled = store
            .list_document_catalog(&DocumentCatalogFilter {
                layer: Some("raw".into()),
                compiled: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            compiled
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["raw-a"]
        );

        let queue = store.uncompiled_raw_shelves(8).unwrap();
        assert_eq!(queue.total_shelves, 2);
        assert_eq!(
            queue
                .shelves
                .iter()
                .map(|shelf| {
                    (
                        shelf.wing.clone().unwrap_or_default(),
                        shelf.room.clone().unwrap_or_default(),
                        shelf.docs,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("dodo".into(), "src".into(), 1),
                ("dodo".into(), "tests".into(), 1)
            ]
        );
    }

    #[test]
    fn catalog_is_lean_filtered_and_paginated_and_home_is_scoped() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("catalog.lin")).unwrap();
        for (id, project, layer, status) in [
            ("a", "alpha", "raw", "active"),
            ("b", "alpha", "wiki", "active"),
            ("c", "alpha", "raw", "archived"),
            ("d", "beta", "raw", "active"),
        ] {
            let document = Document {
                id: id.into(),
                uri: format!("file:///{id}.md"),
                title: format!("Document {id}"),
                content: format!("private body {id}"),
                wing: Some(project.into()),
                room: Some("root".into()),
                source_file: Some(format!("/{project}/{id}.md")),
                layer: layer.into(),
                status: status.into(),
                ..Default::default()
            };
            store.upsert_document(&document).unwrap();
            store
                .insert_chunks(&[Chunk {
                    id: format!("chunk-{id}"),
                    document_id: id.into(),
                    chunk_index: 0,
                    content: document.content,
                    embedding: vec![1.0, 0.0],
                    char_start: 0,
                    char_end: 14,
                    metadata_json: "{}".into(),
                }])
                .unwrap();
        }

        let first = store
            .list_document_catalog(&DocumentCatalogFilter {
                wing: Some("alpha".into()),
                limit: 1,
                ..DocumentCatalogFilter::default()
            })
            .unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.items.len(), 1);
        let second = store
            .list_document_catalog(&DocumentCatalogFilter {
                wing: Some("alpha".into()),
                q: Some("b.md".into()),
                limit: 50,
                ..DocumentCatalogFilter::default()
            })
            .unwrap();
        assert_eq!(second.total, 1);
        assert_eq!(second.items[0].id, "b");
        let home = store.project_home("alpha").unwrap();
        assert_eq!(home.documents, 3);
        assert_eq!(home.chunks, 3);
        assert_eq!(home.raw_documents, 2);
        assert_eq!(home.wiki_documents, 1);
        assert_eq!(home.archived_documents, 1);
        assert_eq!(home.source_files, 3);
    }
}
