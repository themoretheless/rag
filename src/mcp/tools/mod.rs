//! Parameter structs for MCP tools (schemars + serde).
//!
//! Use rmcp's schemars re-export so JsonSchema matches the version rmcp expects (1.x).
//!
//! # ISP: clustered param surfaces
//!
//! Each domain's parameter types live in `tools/<domain>.rs`, next to the
//! router that uses them, and are re-exported here. The MCP facade maps each `*Params`
//! into domain code and JSON; business logic stays in `db` / `wiki` / `graph` /
//! `search_pack` / `maintain` / etc.
//!
//! Section banners (`// --- … ---`) mark the clusters below:
//!
//! | Cluster | Banner | Example types |
//! |---------|--------|---------------|
//! | **Ingest / raw** | `Ingest / raw layer` | [`IngestTextParams`], [`IngestFileParams`], [`IngestRawParams`], [`AddDrawerParams`], [`ListSourcesParams`], [`GetSourceParams`] |
//! | **Search / pack** | `Search / packing` | [`SearchParams`], [`PackHitParams`], [`PackContextParams`] |
//! | **Documents** | `Documents / integrity` | [`ListDocumentsParams`], [`GetDocumentParams`], [`DeleteDocumentParams`], [`UpdateDocumentMetaParams`], … |
//! | **Taxonomy** | `Taxonomy (wings / rooms)` | [`ListWingsParams`], [`ListRoomsParams`], [`GetTaxonomyParams`] |
//! | **Graph** | `Graph` | [`GetGraphParams`], [`GetNeighborsParams`], [`LinkNodesParams`], … |
//! | **Tunnels** | `Tunnels` | [`CreateTunnelParams`], [`FollowTunnelsParams`], [`GraphExpandSearchParams`], … |
//! | **Diary** | `Diary / checkpoint` | [`DiaryWriteParams`], [`WakeUpParams`], [`CheckpointParams`] |
//! | **Ops log** | `Ops log` | [`AppendLogParams`], [`ReadLogParams`], [`ReconnectParams`] |
//! | **Wiki** | `Karpathy wiki layer` | [`WriteWikiPageParams`], [`SearchWikiParams`], [`QueryWithIndexParams`], [`FileAnswerParams`], … |
//! | **Maintenance** | `Maintenance` | [`AnalyzeCorpusParams`], [`PlanMaintenanceParams`], [`ApplyMaintenancePlanParams`], … |
//! | **KG** | `Temporal knowledge graph` | [`KgAddParams`], [`KgQueryParams`], [`KgTimelineParams`], … |
//!
//! Prefer extending an existing cluster over a flat grab-bag. Shared cross-tool
//! fields (wing/room filters, rank options) should become flattenable JsonSchema
//! fragments later rather than copy-pasted onto every struct.
//!
//! Parameter types remain flat (or can later be re-exported from this module) so
//! `use crate::mcp::tools::…` and the rmcp schema surface stay stable; route
//! registration itself is split into the sibling domain modules.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

mod collections;
mod graph;
mod ingest;
mod kg;
mod maintain;
mod recovery;
mod search;
mod wiki;

pub use self::collections::*;
pub use self::graph::*;
pub use self::ingest::*;
pub use self::kg::*;
pub use self::maintain::*;
pub use self::recovery::*;
pub use self::search::*;
pub use self::wiki::*;

// --- Collections / outlines / dependency order ---

/// One ordered document entry supplied to collection create/update.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionEntryParams {
    /// Existing document primary key.
    pub document_id: String,
    /// Optional parent document in the same collection (outline nesting).
    #[serde(default)]
    pub parent_document_id: Option<String>,
    /// Optional prerequisite documents in the same collection.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

pub use crate::search_pack::PackHitParams;
