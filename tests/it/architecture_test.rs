//! Layering guard for the root crate (docs/refactor/ARCHITECTURE.md).
//!
//! Parses every `crate::<module>` path in production code under `src/`
//! (`use crate::…` statements, grouped `use crate::{…}` items and inline
//! paths), maps it to an edge between top-level modules, and fails on an edge
//! that points up the layering or sideways between the two edges:
//!
//! ```text
//! domain(0) → store(1) → intel(2) → app(3) → edge(4) → composition(5)
//! ```
//!
//! A module may depend on its own layer or a lower one. Test code is skipped:
//! `tests.rs` files, `tests/` directories, `#[cfg(test)]` items and everything
//! after the first `#[cfg(test)] mod …` in a file. Crate-root re-exports
//! (`crate::{Store, AppError}`) are not resolved and so not checked; the
//! binaries (`src/bin`, `main.rs`) and `lib.rs` are the composition root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Layer rank of a top-level module; unlisted modules are `app`.
fn layer(module: &str) -> u8 {
    match module {
        "models" | "error" | "config" | "util" | "chunking" | "graph" | "search_pack"
        | "tool_spec" => 0,
        "db" => 1,
        "code_index" | "external_db" | "gate" | "calibration" | "analysis_tasks" => 2,
        "mcp" | "http_api" => 4,
        "bootstrap" => 5,
        _ => 3,
    }
}

/// Edges that break the rule today and are scheduled for a later stage.
const ALLOWED: &[(&str, &str)] = &[];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "tests.rs")
        {
            out.push(path);
        }
    }
}

/// Production part of a file with comments and `#[cfg(test)]` items removed.
fn production_code(source: &str) -> String {
    let mut out = String::new();
    let mut skip_next = false;
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or("");
        let trimmed = code.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            skip_next = true;
            continue;
        }
        if skip_next {
            if trimmed.is_empty() || trimmed.starts_with("#[") {
                continue;
            }
            skip_next = false;
            if trimmed.starts_with("mod ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub(crate) mod ")
            {
                // A test module: the rest of the file is test code.
                break;
            }
            // A single test-only item such as a `use`.
            continue;
        }
        out.push_str(code);
        out.push('\n');
    }
    out
}

fn ident_at(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Top-level modules named by `crate::…` paths in `code`.
fn referenced_modules(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = code;
    while let Some(pos) = rest.find("crate::") {
        let before = rest[..pos].chars().last();
        rest = &rest[pos + "crate::".len()..];
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue; // e.g. `extern_crate::`
        }
        if let Some(group) = rest.strip_prefix('{') {
            let mut depth = 1usize;
            let mut item_start = true;
            for (i, c) in group.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    ',' if depth == 1 => item_start = true,
                    c if item_start && depth == 1 && !c.is_whitespace() => {
                        item_start = false;
                        found.push(ident_at(&group[i..]).to_string());
                    }
                    _ => {}
                }
            }
        } else {
            found.push(ident_at(rest).to_string());
        }
    }
    found
        .into_iter()
        .filter(|m| m.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
        .collect()
}

#[test]
fn module_edges_follow_the_layering() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let allowed: BTreeSet<(&str, &str)> = ALLOWED.iter().copied().collect();
    let mut seen_allowed = BTreeSet::new();
    let mut violations: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for file in &files {
        let rel = file.strip_prefix(&src).unwrap();
        let first = rel
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy();
        if first == "bin" || first == "main.rs" || first == "lib.rs" {
            continue;
        }
        let from = first.trim_end_matches(".rs").to_string();
        let source = std::fs::read_to_string(file).unwrap();
        for (n, line) in production_code(&source).lines().enumerate() {
            for to in referenced_modules(line) {
                if to == from || to == "self" || to == "super" {
                    continue;
                }
                let upward = layer(&to) > layer(&from);
                let sideways = layer(&from) == 4 && layer(&to) == 4;
                if !(upward || sideways) {
                    continue;
                }
                if allowed.contains(&(from.as_str(), to.as_str())) {
                    seen_allowed.insert((from.clone(), to.clone()));
                    continue;
                }
                violations
                    .entry((from.clone(), to.clone()))
                    .or_default()
                    .push(format!("src/{}:{}", rel.display(), n + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "module edges against the layering (see docs/refactor/ARCHITECTURE.md):\n{violations:#?}"
    );
    // Keep the allow-list honest: an entry whose edge is gone must be removed.
    let stale: Vec<_> = ALLOWED
        .iter()
        .filter(|(a, b)| !seen_allowed.contains(&(a.to_string(), b.to_string())))
        .collect();
    assert!(
        stale.is_empty(),
        "allow-listed edges no longer present: {stale:?}"
    );
}

#[test]
fn edge_parser_reads_grouped_and_inline_paths() {
    let code = "use crate::{db::Store, error::{AppError, Result}, Store};\nlet x = crate::gate::DDL; extern_crate::no";
    let mut found = referenced_modules(code);
    found.sort();
    assert_eq!(found, vec!["db", "error", "gate"]);
    let prod = production_code("use crate::a;\n#[cfg(test)]\nuse crate::b;\nfn f() {} // crate::c\n#[cfg(test)]\nmod tests {\nuse crate::d;\n}\n");
    assert_eq!(referenced_modules(&prod), vec!["a"]);
}

#[test]
fn store_is_extended_only_inside_db() {
    // Code outside `src/db` reaches the database through repository types and
    // `Store::with_conn` / `with_tx`, never by adding methods to `Store`.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let offenders: Vec<String> = files
        .iter()
        .filter(|f| !f.strip_prefix(&src).unwrap().starts_with("db"))
        .filter(|f| {
            std::fs::read_to_string(f).unwrap().lines().any(|l| {
                let l = l.trim_start();
                l.starts_with("impl Store {")
                    || l.starts_with("impl crate::Store {")
                    || l.starts_with("impl crate::db::Store {")
            })
        })
        .map(|f| f.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "impl Store outside src/db: {offenders:?}"
    );
}
