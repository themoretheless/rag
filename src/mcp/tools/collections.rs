//! Collections tool parameters.

use super::*;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionCreateParams {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// Reading order; array order becomes the durable position.
    #[serde(default)]
    pub entries: Vec<CollectionEntryParams>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CollectionListParams {}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionGetParams {
    pub collection_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionUpdateParams {
    pub collection_id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Empty string clears the description.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// When present, replaces membership, reading order, outline parents, and dependencies.
    #[serde(default)]
    pub entries: Option<Vec<CollectionEntryParams>>,
}
