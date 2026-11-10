//! Typed parameters per maintenance action.
//!
//! The wire shape of a plan step stays `{action, reason?, target_id?, params}`
//! with `params` a free JSON object: MCP/HTTP clients and stored plans send it
//! that way, and the apply layer has always read it leniently (numbers as
//! strings, comma-separated lists, `"yes"`/`"1"` booleans, unknown keys
//! ignored). Each action reads its own typed struct from that object here,
//! once, with exactly those rules; executors never look at raw JSON.

use super::*;

// --- lenient readers (the historic param rules) ---

pub(super) fn param_str(params: &serde_json::Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(super) fn param_f64(params: &serde_json::Value, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

/// Non-negative integer param (e.g. `max_docs`). Absent → `Ok(None)`;
/// negative, NaN or infinite values are rejected; fractions truncate.
pub(super) fn param_usize(params: &serde_json::Value, key: &str) -> Result<Option<usize>> {
    match param_f64(params, key) {
        None => Ok(None),
        Some(n) if n.is_finite() && n >= 0.0 => Ok(Some(n.min(usize::MAX as f64) as usize)),
        Some(n) => Err(AppError::invalid(format!(
            "maintenance param '{key}' must be a non-negative finite number, got {n}"
        ))),
    }
}

pub(super) fn param_bool(params: &serde_json::Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|v| {
        v.as_bool().or_else(|| match v.as_str() {
            Some("true") | Some("1") | Some("yes") => Some(true),
            Some("false") | Some("0") | Some("no") => Some(false),
            _ => None,
        })
    })
}

pub(super) fn param_str_list(params: &serde_json::Value, key: &str) -> Vec<String> {
    let Some(v) = params.get(key) else {
        return Vec::new();
    };
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .filter_map(|x| x.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .map(|s| s.to_string())
            .collect();
    }
    if let Some(s) = v.as_str() {
        return s
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect();
    }
    Vec::new()
}

/// Like [`param_str`], but an explicit empty string means "clear".
fn param_str_or_clear(params: &serde_json::Value, key: &str) -> Option<String> {
    param_str(params, key).or_else(|| {
        if params.get(key).and_then(|v| v.as_str()) == Some("") {
            Some(String::new())
        } else {
            None
        }
    })
}

/// Caps an optional per-step `max_docs` by the configured maintenance cap.
pub(super) fn capped_max_docs(requested: Option<usize>, config: &Config) -> usize {
    requested
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1))
}

// --- typed params, one struct per action that reads params ---

/// `refile`: metadata fields to change (`""` clears wing / room).
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct RefileParams {
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub source_file: Option<String>,
    pub title: Option<String>,
}

impl RefileParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        Self {
            wing: param_str_or_clear(params, "wing"),
            room: param_str_or_clear(params, "room"),
            layer: param_str(params, "layer"),
            kind: param_str(params, "kind"),
            status: param_str(params, "status"),
            source_file: param_str(params, "source_file"),
            title: param_str(params, "title"),
        }
    }

    pub fn to_update(&self) -> DocumentMetaUpdate {
        DocumentMetaUpdate {
            wing: self.wing.clone(),
            room: self.room.clone(),
            layer: self.layer.clone(),
            kind: self.kind.clone(),
            status: self.status.clone(),
            source_file: self.source_file.clone(),
            title: self.title.clone(),
            ..Default::default()
        }
    }
}

/// `set_boost`: the new boost (required; checked by the executor).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SetBoostParams {
    pub boost: Option<f64>,
}

impl SetBoostParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        Self {
            boost: param_f64(params, "boost"),
        }
    }
}

/// `set_tags`: `tags` (array or comma/space list) or `tag`; `mode` add|replace.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SetTagsParams {
    pub tags: Vec<String>,
    pub mode: String,
}

impl SetTagsParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        let mut tags = param_str_list(params, "tags");
        if tags.is_empty() {
            tags = param_str_list(params, "tag");
        }
        if tags.is_empty() {
            if let Some(t) = param_str(params, "tag") {
                tags.push(t);
            }
        }
        Self {
            tags,
            mode: param_str(params, "mode").unwrap_or_else(|| "replace".into()),
        }
    }
}

/// `rebuild_graph`: dirty documents only (default true) and a batch cap.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RebuildGraphParams {
    pub dirty_only: bool,
    pub max_docs: Option<usize>,
}

impl RebuildGraphParams {
    pub fn parse(params: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            dirty_only: param_bool(params, "dirty_only").unwrap_or(true),
            max_docs: param_usize(params, "max_docs")?,
        })
    }
}

/// `reembed_all`: batch cap.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReembedAllParams {
    pub max_docs: Option<usize>,
}

impl ReembedAllParams {
    pub fn parse(params: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            max_docs: param_usize(params, "max_docs")?,
        })
    }
}

/// `compile_source`: source when `target_id` is absent.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CompileSourceParams {
    pub source_id_or_uri: Option<String>,
}

impl CompileSourceParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        Self {
            source_id_or_uri: param_str(params, "source_id_or_uri")
                .or_else(|| param_str(params, "source_id")),
        }
    }
}

/// `consolidate`: a pre-synthesized page (`content`, alias `body`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ConsolidateParams {
    pub title: Option<String>,
    pub content: Option<String>,
    pub slug: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
    pub summary: Option<String>,
}

impl ConsolidateParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        Self {
            title: param_str(params, "title"),
            content: param_str(params, "content").or_else(|| param_str(params, "body")),
            slug: param_str(params, "slug"),
            kind: param_str(params, "kind"),
            category: param_str(params, "category"),
            summary: param_str(params, "summary"),
        }
    }
}

/// `refresh_stale_wiki`: page cap and nested preview flag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RefreshStaleParams {
    pub max_docs: Option<usize>,
    pub dry_run: bool,
}

impl RefreshStaleParams {
    pub fn parse(params: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            max_docs: param_usize(params, "max_docs")?,
            dry_run: param_bool(params, "dry_run").unwrap_or(false),
        })
    }
}

/// `file_answer`: answer page (`body`, alias `content`) and citations.
#[derive(Debug, Clone)]
pub(super) struct FileAnswerParams {
    pub title: Option<String>,
    pub body: Option<String>,
    pub slug: Option<String>,
    pub citations: Option<Vec<FileAnswerCitation>>,
}

impl FileAnswerParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        Self {
            title: param_str(params, "title"),
            body: param_str(params, "body").or_else(|| param_str(params, "content")),
            slug: param_str(params, "slug"),
            citations: params
                .get("citations")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
        }
    }
}

/// `merge_exact_dup`: canonical id fallback, declared hash, requested sources.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MergeExactDupParams {
    pub keep_id: Option<String>,
    pub content_hash: Option<String>,
    /// `source_ids` (else `document_ids`) plus `document_id_b` / `drop_id` /
    /// `source_id`, in that order, before de-duplication against the keep id.
    pub source_ids: Vec<String>,
    pub allow_raw_delete: bool,
    pub delete: bool,
}

impl MergeExactDupParams {
    pub fn parse(params: &serde_json::Value) -> Self {
        let mut source_ids = param_str_list(params, "source_ids");
        if source_ids.is_empty() {
            source_ids = param_str_list(params, "document_ids");
        }
        if let Some(b) = param_str(params, "document_id_b")
            .or_else(|| param_str(params, "drop_id"))
            .or_else(|| param_str(params, "source_id"))
        {
            if !source_ids.contains(&b) {
                source_ids.push(b);
            }
        }
        Self {
            keep_id: param_str(params, "keep_id"),
            content_hash: param_str(params, "content_hash"),
            source_ids,
            allow_raw_delete: param_bool(params, "allow_raw_delete").unwrap_or(false),
            delete: param_bool(params, "delete").unwrap_or(false),
        }
    }
}

/// `drop_tombstones`: raw-layer opt-in and batch cap.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DropTombstonesParams {
    pub allow_raw_delete: bool,
    pub max_docs: Option<usize>,
}

impl DropTombstonesParams {
    pub fn parse(params: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            allow_raw_delete: param_bool(params, "allow_raw_delete").unwrap_or(false),
            max_docs: param_usize(params, "max_docs")?,
        })
    }
}
