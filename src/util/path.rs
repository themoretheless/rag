//! Shared safety policy for allowlisted output files and live-database targets.

use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

use super::check_path_allowlist;

/// Resolve a not-yet-created output file under an allowlisted root.
///
/// The parent must already exist so it can be canonicalized before the caller
/// writes, preventing a symlink or `..` escape between separate tool surfaces.
pub fn resolve_allowlisted_output_file(raw: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AppError::config("path must be non-empty"));
    }
    let path = PathBuf::from(raw);
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let parent = parent.canonicalize()?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::config("path must name a file"))?;
    let resolved = parent.join(file_name);
    check_path_allowlist(&resolved, roots)?;
    Ok(resolved)
}

/// Refuse an output path that resolves to the active DuckDB file.
pub fn refuse_live_database_target(path: &Path, database_path: &Path) -> Result<()> {
    let database = database_path
        .canonicalize()
        .unwrap_or_else(|_| database_path.to_path_buf());
    let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let same_path = candidate == database;
    #[cfg(unix)]
    let same_inode = {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(path), std::fs::metadata(database_path)) {
            (Ok(candidate), Ok(database)) => {
                candidate.dev() == database.dev() && candidate.ino() == database.ino()
            }
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let same_inode = false;
    if same_path || same_inode {
        return Err(AppError::forbidden(
            "recovery output path must not be the live DuckDB file",
        ));
    }
    Ok(())
}

/// Paths written by a DuckDB backup: the database copy and its integrity sidecars.
pub fn backup_artifact_paths(path: &Path) -> [PathBuf; 3] {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}.sha256", path.display())),
        PathBuf::from(format!("{}.metadata.json", path.display())),
    ]
}

/// Apply the output allowlist and live-database alias guard to every backup artifact.
pub fn validate_backup_output_paths(
    path: &Path,
    roots: &[PathBuf],
    database_path: &Path,
) -> Result<()> {
    for artifact in backup_artifact_paths(path) {
        check_path_allowlist(&artifact, roots)?;
        refuse_live_database_target(&artifact, database_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_file_is_canonicalized_and_live_database_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("rag.duckdb");
        std::fs::write(&database, "db").unwrap();
        let backup = resolve_allowlisted_output_file(
            root.path().join("backup.duckdb").to_str().unwrap(),
            &[root.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(
            backup,
            root.path().canonicalize().unwrap().join("backup.duckdb")
        );
        assert!(refuse_live_database_target(&backup, &database).is_ok());

        let live = resolve_allowlisted_output_file(
            database.to_str().unwrap(),
            &[root.path().to_path_buf()],
        )
        .unwrap();
        assert!(matches!(
            refuse_live_database_target(&live, &database),
            Err(AppError::Forbidden(_))
        ));

        let alias = root.path().join("live-alias.duckdb");
        std::fs::hard_link(&database, &alias).unwrap();
        assert!(matches!(
            refuse_live_database_target(&alias, &database),
            Err(AppError::Forbidden(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn backup_sidecars_cannot_escape_the_allowlist_through_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let database = root.path().join("rag.duckdb");
        std::fs::write(&database, "db").unwrap();
        let backup = root.path().join("backup.duckdb");
        let outside_target = outside.path().join("stolen.sha256");
        std::fs::write(&outside_target, "keep").unwrap();
        symlink(
            &outside_target,
            PathBuf::from(format!("{}.sha256", backup.display())),
        )
        .unwrap();

        assert!(
            validate_backup_output_paths(&backup, &[root.path().to_path_buf()], &database,)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(outside_target).unwrap(), "keep");
    }
}
