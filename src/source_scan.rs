//! Shared source-tree discovery policy for every ingest entry point.

use std::collections::BTreeSet;
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

pub fn collect_source_files(
    root: &Path,
    policy: &SourceScanPolicy,
) -> std::io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if !SKIP_DIRECTORY_NAMES.contains(&name) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_skips_generated_trees_and_unsupported_files() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("keep.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("skip.lock"), "lock").unwrap();
        for directory in ["bin", "obj", ".yarn", ".turbo", "TestResults", "worktrees"] {
            let generated = root.path().join(directory);
            std::fs::create_dir_all(&generated).unwrap();
            std::fs::write(generated.join("duplicate.rs"), "fn duplicate() {}").unwrap();
        }

        let files = collect_source_files(root.path(), &SourceScanPolicy::default()).unwrap();
        assert_eq!(files, vec![root.path().join("keep.rs")]);
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
