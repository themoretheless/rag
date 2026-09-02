//! Shared helpers (hashing, path allowlist, wiki URI slug).

pub mod hash;
pub mod path;
pub mod slug;
pub mod time;

pub use hash::{check_path_allowlist, content_hash, ingest_roots_from_env, parse_ingest_roots};
pub use path::{
    backup_artifact_paths, refuse_live_database_target, resolve_allowlisted_output_file,
    validate_backup_output_paths,
};
pub use slug::{slugify, SlugPolicy};
pub use time::{format_db_timestamp, parse_db_timestamp, parse_flexible_timestamp};

/// Extract the slug segment from a `wiki://` URI.
///
/// Strips the `wiki://` scheme, then trims whitespace and surrounding `/`.
/// Returns `None` when the URI is not `wiki://` or the remainder is empty.
pub fn wiki_slug_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("wiki://")?;
    let s = rest.trim().trim_matches('/');
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
