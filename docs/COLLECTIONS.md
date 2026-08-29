# Collections, outlines, and reading order

Collections are durable named sets of existing documents. They do not change document identity or content.

The full MCP surface (`RAG_TOOLS=full`) provides:

- `collection_create`: create metadata and an optional ordered `entries` array.
- `collection_list`: list collection metadata without loading entries.
- `collection_get`: return metadata and entries in reading order.
- `collection_update`: update metadata or replace the complete ordered entry set.

Each entry requires an existing `document_id`. Its array index is stored as `position`. Optional `parent_document_id` creates outline nesting, and optional `depends_on` lists prerequisite document IDs. Parents and prerequisites must be other members of the same collection. Outline cycles are rejected. Supplying `entries` to `collection_update` replaces membership, order, outline parents, and dependency links atomically; omitting it preserves all entries.

`collection_get` also derives `dependency_order` with reading order as the stable tie-breaker. When prerequisites cycle, `dependency_order` is empty and `dependency_cycle_members` identifies the affected entries.

Example entry list:

```json
[
  { "document_id": "intro" },
  { "document_id": "concepts", "parent_document_id": "intro" },
  { "document_id": "advanced", "depends_on": ["concepts"] }
]
```
