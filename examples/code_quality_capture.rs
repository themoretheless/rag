//! Native export for owned, stopped MCP qualification stores only.
use rag_mcp::db::Store;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root =
        PathBuf::from(args.next().ok_or("expected owned fixture directory")?).canonicalize()?;
    let output = PathBuf::from(args.next().ok_or("expected output JSON path")?);
    if args.next().is_some()
        || !root
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("rag-code-quality-"))
        || std::fs::read_to_string(root.join(".qualification-owned"))?
            != "rag code quality temporary store\n"
    {
        return Err("refusing a directory not owned by the code quality runner".into());
    }
    // The runner closes its MCP process before this opens the disposable DB.
    let store = Store::open(&root.join("test.duckdb"))?;
    let snapshot = store.code_snapshot()?;
    if !snapshot.stale_documents.is_empty() || !snapshot.unindexed_documents.is_empty() {
        return Err("native and MCP captures do not share a fresh extraction format".into());
    }
    let mut destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    serde_json::to_writer(&mut destination, &snapshot)?;
    Ok(())
}
