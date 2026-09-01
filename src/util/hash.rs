//! Content hashing and ingest path allowlist helpers.
//!
//! - [`content_hash`]: stable blake3 hex digest of text (for dedupe / embed cache).
//! - [`check_path_allowlist`]: gate filesystem paths against `RAG_INGEST_ROOTS`.

use std::env;
use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, Result};

/// Hex-encoded blake3 hash of `text` (UTF-8 bytes, no extra normalization).
///
/// Same input always yields the same 64-character lowercase hex string.
pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// Parse `RAG_INGEST_ROOTS` from the process environment.
///
/// Comma-separated paths (trimmed). Empty or unset env yields an empty list
/// (all file paths refused by the allowlist).
pub fn ingest_roots_from_env() -> Vec<PathBuf> {
    match env::var_os("RAG_INGEST_ROOTS") {
        Some(raw) if !raw.is_empty() => parse_ingest_roots(&raw),
        _ => Vec::new(),
    }
}

/// Parse a roots string the same way as `RAG_INGEST_ROOTS` (comma-separated).
pub fn parse_ingest_roots(raw: &std::ffi::OsStr) -> Vec<PathBuf> {
    raw.to_string_lossy()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Ensure `path` is under at least one allowlisted root.
///
/// Rules:
/// - Empty `roots` refuses every path (must configure `RAG_INGEST_ROOTS`).
/// - Paths are made absolute against the current directory, then normalized
///   (`.` / `..` components). When a path exists, `canonicalize` is preferred
///   so symlinks cannot escape the sandbox.
/// - Match is component-prefix based (`Path::starts_with`).
pub fn check_path_allowlist(path: &Path, roots: &[PathBuf]) -> Result<()> {
    if roots.is_empty() {
        return Err(AppError::config(
            "RAG_INGEST_ROOTS is empty or unset; refuse all file ingest paths",
        ));
    }

    let candidate = resolve_for_allowlist(path)?;

    for root in roots {
        let root_resolved = resolve_for_allowlist(root)?;
        if candidate.starts_with(&root_resolved) {
            return Ok(());
        }
    }

    Err(AppError::config(format!(
        "path '{}' is outside RAG_INGEST_ROOTS",
        path.display()
    )))
}

/// Absolute + normalized path for allowlist comparison.
fn resolve_for_allowlist(path: &Path) -> Result<PathBuf> {
    // Prefer real path when the entry exists (follows symlinks).
    if let Ok(canon) = path.canonicalize() {
        return Ok(canon);
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = env::current_dir().map_err(AppError::from)?;
        cwd.join(path)
    };

    Ok(normalize_lexically(&absolute))
}

/// Lexical normalize: drop `.`, apply `..`, keep root/prefix. Does not touch the FS.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Do not pop past root / prefix.
                if let Some(Component::Normal(_)) = out.components().next_back() {
                    out.pop();
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn content_hash_is_stable_and_hex() {
        let a = content_hash("hello world");
        let b = content_hash("hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(content_hash("hello world"), content_hash("hello world!"));
    }

    #[test]
    fn empty_roots_refuse_all() {
        let err = check_path_allowlist(Path::new("/tmp/x"), &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("RAG_INGEST_ROOTS"), "{msg}");
    }

    #[test]
    fn allowlist_accepts_path_under_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let file = root.join("note.md");
        fs::write(&file, "x").unwrap();

        check_path_allowlist(&file, &[root]).expect("allowed");
    }

    #[test]
    fn allowlist_rejects_outside_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("allowed");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("secret.txt");
        fs::write(&outside, "nope").unwrap();

        let err = check_path_allowlist(&outside, &[root]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("outside RAG_INGEST_ROOTS"), "{msg}");
    }

    #[test]
    fn allowlist_blocks_parent_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("vault");
        fs::create_dir_all(&root).unwrap();
        let escape = root.join("..").join("escape.txt");
        fs::write(dir.path().join("escape.txt"), "x").unwrap();

        let err = check_path_allowlist(&escape, &[root]).unwrap_err();
        assert!(err.to_string().contains("outside RAG_INGEST_ROOTS"));
    }

    #[test]
    fn parse_ingest_roots_empty() {
        assert!(parse_ingest_roots(std::ffi::OsStr::new("")).is_empty());
    }

    #[test]
    fn parse_ingest_roots_comma_separated() {
        let roots = parse_ingest_roots(std::ffi::OsStr::new("/a, /b ,/c"));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ]
        );
    }
}
