//! Bounded identity of imported standard SIMD structs; no external definitions.
use super::*;

fn expand_import(doc: &IndexedDocument, at: usize, path: &str) -> String {
    let (head, tail) = path.split_once("::").unwrap_or((path, ""));
    if let Some(import) = resolve::import_target(doc, head, at) {
        let import = import.strip_prefix("\0extern_crate::").unwrap_or(import);
        if tail.is_empty() {
            import.into()
        } else {
            format!("{import}::{tail}")
        }
    } else {
        path.into()
    }
}

pub(super) fn identity(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    at: usize,
    name: &str,
    canonical: &str,
) -> Option<String> {
    let arch = rust_cargo::captured_target_arch(docs, doc)?;
    let candidate = if canonical == name
        && !name.contains("::")
        && resolve::import_target(doc, name, at).is_none()
    {
        let glob = resolve::unique_rust_glob_target(doc, at)?.strip_suffix("::*")?;
        format!("{}::{name}", expand_import(doc, at, glob))
    } else {
        expand_import(doc, at, canonical)
    };
    let parts: Vec<_> = candidate.split("::").collect();
    let [root, "arch", module, item] = parts.as_slice() else {
        return None;
    };
    if !matches!(*root, "core" | "std")
        || !rust_cargo::standard_crate_unshadowed(docs, doc, at, root)
        || resolve::unknown_rust_namespace(doc, root, at)
        || !(arch == *module || (arch == "arm64ec" && *module == "aarch64"))
        || !super::stdarch_catalog::known(module, item)
    {
        return None;
    }
    Some(format!("core::arch::{module}::{item}"))
}
