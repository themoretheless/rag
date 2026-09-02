//! Shared source-tree discovery policy for every ingest entry point.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::file_ingest::is_supported_source;

pub const SKIP_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".agents",
    ".codex",
    ".claude",
    ".grok",
    ".idea",
    ".vscode",
    ".zed",
    ".cache",
    "target",
    "node_modules",
    "bin",
    "obj",
    "dist",
    "build",
    "out",
    "coverage",
    "vendor",
    "backups",
    ".yarn",
    ".turbo",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "storybook-static",
    "TestResults",
    "worktrees",
];

#[derive(Debug, Clone, Default)]
pub struct SourceScanPolicy {
    max_bytes: Option<u64>,
    extensions: Option<BTreeSet<String>>,
}

impl SourceScanPolicy {
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    pub fn with_extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.extensions = Some(
            extensions
                .into_iter()
                .map(|extension| {
                    extension
                        .as_ref()
                        .trim()
                        .trim_start_matches('.')
                        .to_ascii_lowercase()
                })
                .filter(|extension| !extension.is_empty())
                .collect(),
        );
        self
    }

    /// Maximum accepted file size, when configured.
    pub fn max_bytes(&self) -> Option<u64> {
        self.max_bytes
    }

    fn accepts(&self, path: &Path, size: u64) -> bool {
        if self.max_bytes.is_some_and(|limit| size > limit) {
            return false;
        }
        match &self.extensions {
            Some(extensions) => path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|extension| extensions.contains(&extension)),
            None => is_supported_source(path),
        }
    }
}

/// Return whether traversal rooted at `root` intentionally excludes `source`.
///
/// This deliberately covers only directory exclusions, not file extensions or
/// size limits. Callers may therefore prune state for sources excluded by a
/// policy update without treating an unreadable or oversized existing file as
/// deleted.
pub fn is_explicitly_excluded_source_path(root: &Path, source: &Path) -> bool {
    let Ok(relative) = source.strip_prefix(root) else {
        return false;
    };
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }

    let mut directory = root.to_path_buf();
    if has_regular_pyvenv_sentinel(&directory) {
        return true;
    }
    let Some(parent) = relative.parent() else {
        return false;
    };
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return false;
        };
        directory.push(name);
        if is_skipped_directory_name(name) || has_regular_pyvenv_sentinel(&directory) {
            return true;
        }
    }
    false
}

pub fn collect_source_files(
    root: &Path,
    policy: &SourceScanPolicy,
) -> std::io::Result<Vec<PathBuf>> {
    collect_source_files_while(root, policy, || true)
}

/// Collect source files while `keep_going` remains true.
///
/// Returning false stops traversal promptly and returns the files found so far;
/// the caller owns the cancellation outcome.
pub fn collect_source_files_while(
    root: &Path,
    policy: &SourceScanPolicy,
    mut keep_going: impl FnMut() -> bool,
) -> std::io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        if !keep_going() {
            break;
        }
        if has_regular_pyvenv_sentinel(&directory) {
            continue;
        }
        for entry in std::fs::read_dir(directory)? {
            if !keep_going() {
                files.sort();
                return Ok(files);
            }
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                if !path.file_name().is_some_and(is_skipped_directory_name) {
                    pending.push(path);
                }
            } else if file_type.is_file() {
                let size = entry.metadata()?.len();
                if policy.accepts(&path, size) {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn is_skipped_directory_name(name: &OsStr) -> bool {
    SKIP_DIRECTORY_NAMES
        .iter()
        .any(|skipped| name == OsStr::new(skipped))
}

fn has_regular_pyvenv_sentinel(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join("pyvenv.cfg"))
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_skips_generated_trees_and_unsupported_files() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("keep.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("skip.lock"), "lock").unwrap();
        for directory in [
            "bin",
            "obj",
            ".yarn",
            ".turbo",
            "storybook-static",
            "TestResults",
            "worktrees",
        ] {
            let generated = root.path().join(directory);
            std::fs::create_dir_all(&generated).unwrap();
            std::fs::write(generated.join("duplicate.rs"), "fn duplicate() {}").unwrap();
        }

        let files = collect_source_files(root.path(), &SourceScanPolicy::default()).unwrap();
        assert_eq!(files, vec![root.path().join("keep.rs")]);
    }

    #[test]
    fn default_policy_skips_caches_and_python_virtual_environments_without_overmatching() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested_cache = root.path().join("project/.cache/bulk-deps");
        std::fs::create_dir_all(&nested_cache).unwrap();
        std::fs::write(nested_cache.join("dependency.py"), "cached = True").unwrap();

        let virtual_environment = root.path().join("project/.venv-pattern");
        std::fs::create_dir_all(virtual_environment.join("lib/site-packages")).unwrap();
        std::fs::write(virtual_environment.join("pyvenv.cfg"), "home = /python").unwrap();
        std::fs::write(
            virtual_environment.join("lib/site-packages/dependency.py"),
            "installed = True",
        )
        .unwrap();

        let mut expected = Vec::new();
        for (parent, directory) in [
            ("lowercase", "cache"),
            ("uppercase", "Cache"),
            ("environment", "env"),
            ("generated-source", "generated"),
        ] {
            let retained = root.path().join(parent).join(directory).join("source.rs");
            std::fs::create_dir_all(retained.parent().unwrap()).unwrap();
            std::fs::write(&retained, "pub fn retained() {}").unwrap();
            expected.push(retained);
        }
        let non_regular_sentinel = root.path().join("not-an-environment/pyvenv.cfg");
        std::fs::create_dir_all(&non_regular_sentinel).unwrap();
        let retained_below_non_regular = root.path().join("not-an-environment/lib/source.py");
        std::fs::create_dir_all(retained_below_non_regular.parent().unwrap()).unwrap();
        std::fs::write(&retained_below_non_regular, "retained = True").unwrap();
        expected.push(retained_below_non_regular);
        expected.sort();

        let files = collect_source_files(root.path(), &SourceScanPolicy::default()).unwrap();
        assert_eq!(files, expected);
    }

    #[test]
    fn explicit_exclusion_matches_only_skipped_ancestors_and_regular_venv_sentinels() {
        let root = tempfile::tempdir().expect("tempdir");
        let cached = root.path().join("project/.cache/dependency.py");
        let similarly_named = root.path().join("project/Cache/dependency.py");
        let virtualized = root.path().join("project/environment/lib/dependency.py");
        let outside = root.path().parent().unwrap().join("outside/.cache/file.py");
        for source in [&cached, &similarly_named, &virtualized] {
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(source, "body").unwrap();
        }
        std::fs::write(
            root.path().join("project/environment/pyvenv.cfg"),
            "home = /python",
        )
        .unwrap();

        assert!(is_explicitly_excluded_source_path(root.path(), &cached));
        assert!(is_explicitly_excluded_source_path(
            root.path(),
            &virtualized
        ));
        assert!(!is_explicitly_excluded_source_path(
            root.path(),
            &similarly_named
        ));
        assert!(!is_explicitly_excluded_source_path(root.path(), &outside));
    }

    #[test]
    fn controlled_scan_stops_when_requested() {
        let root = tempfile::tempdir().expect("tempdir");
        for index in 0..10 {
            std::fs::write(root.path().join(format!("{index}.md")), "body").unwrap();
        }
        let checks = std::cell::Cell::new(0usize);

        let files = collect_source_files_while(root.path(), &SourceScanPolicy::default(), || {
            let next = checks.get() + 1;
            checks.set(next);
            next <= 4
        })
        .unwrap();

        assert!(checks.get() >= 5);
        assert!(files.len() < 10);
    }

    #[test]
    fn custom_extensions_and_size_limit_are_composed() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("keep.custom"), "ok").unwrap();
        std::fs::write(root.path().join("large.custom"), "too large").unwrap();
        std::fs::write(root.path().join("skip.rs"), "rs").unwrap();
        let policy = SourceScanPolicy::default()
            .with_extensions([".custom"])
            .with_max_bytes(3);

        let files = collect_source_files(root.path(), &policy).unwrap();
        assert_eq!(files, vec![root.path().join("keep.custom")]);
    }
}
