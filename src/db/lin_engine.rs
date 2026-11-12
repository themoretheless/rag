//! Embedded `lin` engine adapter, `BridgeEmbedder`, typed `Row`/`Cell` helpers,
//! concurrent `RwLock` read/write guards, and ACID `Transaction` wrapper.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use chrono::{DateTime, Utc};
use lin::{
    disk_v2::{
        is_disk_v2_dir, DiskColKind, DiskColSchema, DiskEngineV2, DiskTableSchema, DiskWalOp,
        HotCacheConfig, HotCacheStats,
    },
    Cell, Db, Embedder, OpenOpts, Pred, Query, Quotas, ReadDb, Row, Source, Step, Stmt, SyncMode,
    Value,
};

use crate::error::{AppError, Result};
pub use crate::util::format_db_timestamp;

/// Maximum estimated byte payload per single `Stmt::Insert` WAL frame (`lin` `MAX_RECORD = 16 MiB`).
const MAX_INSERT_BATCH_BYTES: usize = 3 * 1024 * 1024;
const MAX_INSERT_BATCH_ROWS: usize = 512;

/// Production quotas raised for large RAG corpora (~1M+ chunks, ~100k+ docs, ~500k+ edges).
pub fn rag_lin_quotas() -> Quotas {
    Quotas {
        max_rows: 100_000_000,
        max_edges: 100_000_000,
        max_log_bytes: 2 * 1024 * 1024 * 1024,
        max_query_bytes: 64 * 1024 * 1024,
        max_ast_depth: 512,
        max_ast_bytes: 256 * 1024 * 1024,
        max_output_bytes: 2 * 1024 * 1024 * 1024,
        max_execution_work: 500_000_000,
        max_aggregate_bytes: 512 * 1024 * 1024,
        max_journal_bytes: 1024 * 1024 * 1024,
        max_tx_mutations: 5_000_000,
        spill_enabled: true,
        ..Quotas::default()
    }
}

/// Normalize a filesystem path so `lin::persist::validate_data_dir` and `write_backup`
/// never reject lexical `..` (`Component::ParentDir`) segments.
pub fn normalize_lin_path(path: &Path) -> PathBuf {
    if path.as_os_str() == ":memory:" || path.as_os_str().is_empty() {
        return path.to_path_buf();
    }
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(seg) => out.push(seg),
        }
    }
    if let (Some(parent), Some(name)) = (out.parent(), out.file_name()) {
        if let Ok(canon_parent) = parent.canonicalize() {
            return canon_parent.join(name);
        }
    }
    out
}

/// Alias for `normalize_lin_path` used by CLI maintenance tools.
pub fn clean_db_path(path: &Path) -> PathBuf {
    normalize_lin_path(path)
}

/// Restore a portable `LIN\x04` backup file into a durable `DiskEngineV2` directory.
pub fn restore_portable_backup_to_dir(backup: &Path, data_dir: &Path) -> Result<()> {
    let clean_backup = normalize_lin_path(backup);
    let clean_dir = normalize_lin_path(data_dir);
    let extra_idx = rag_default_indexed_fields();
    let _eng = DiskEngineV2::migrate_from_legacy(&clean_backup, &clean_dir, 65_536, &extra_idx)?;
    Ok(())
}

/// Default indexed columns per table for `DiskEngineV2` segments.
pub fn rag_default_indexed_fields() -> std::collections::BTreeMap<String, Vec<String>> {
    let mut m = std::collections::BTreeMap::new();
    m.insert(
        "documents".into(),
        vec![
            "uri".into(),
            "wing".into(),
            "room".into(),
            "layer".into(),
            "status".into(),
            "source_file".into(),
            "kind".into(),
            "content_hash".into(),
        ],
    );
    m.insert("document_revisions".into(), vec!["document_id".into()]);
    m.insert(
        "chunks".into(),
        vec!["document_id".into(), "parent_chunk_id".into()],
    );
    m.insert(
        "chunk_change_log".into(),
        vec!["generation".into(), "document_id".into()],
    );
    m.insert(
        "graph_nodes".into(),
        vec![
            "kind".into(),
            "label_key".into(),
            "document_id".into(),
            "uri".into(),
        ],
    );
    m.insert(
        "graph_edges".into(),
        vec![
            "source_id".into(),
            "target_id".into(),
            "rel_type".into(),
            "chunk_id".into(),
        ],
    );
    m.insert(
        "ops_log".into(),
        vec!["seq".into(), "op".into(), "entity_id".into()],
    );
    m.insert(
        "wiki_index".into(),
        vec![
            "slug".into(),
            "kind".into(),
            "document_id".into(),
            "page_id".into(),
        ],
    );
    m.insert(
        "source_manifest".into(),
        vec![
            "canonical_path".into(),
            "canonical_root".into(),
            "document_id".into(),
        ],
    );
    m.insert(
        "reembed_progress".into(),
        vec!["target_fingerprint".into(), "document_id".into()],
    );
    m.insert(
        "kg_facts".into(),
        vec![
            "subject".into(),
            "predicate".into(),
            "object".into(),
            "status".into(),
            "source_document_id".into(),
        ],
    );
    m.insert("meta".into(), vec!["key".into()]);
    m.insert(
        "collection_entries".into(),
        vec![
            "collection_id".into(),
            "document_id".into(),
            "parent_document_id".into(),
        ],
    );
    m.insert(
        "collection_dependencies".into(),
        vec!["document_id".into(), "depends_on_document_id".into()],
    );
    m
}

/// Predicate matching every row in any collection (`id != null`), since `lin::Pred` has no `True` variant.
pub fn pred_all() -> Pred {
    lin::query::pred::is_not_null("id")
}

/// Check whether `path` starts with a `lin` portable backup header (`[0x01..=0x04] + LIN\x04` or `LIN\x04`).
pub fn is_lin_portable_backup_file(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 5];
    use std::io::Read;
    let Ok(n) = file.read(&mut head) else {
        return false;
    };
    if n >= 5 && matches!(head[0], 0x01..=0x04) && head[1..5] == *b"LIN\x04" {
        return true;
    }
    if n >= 4 && head[0..4] == *b"LIN\x04" {
        return true;
    }
    false
}

/// Check whether `path` is a durable `lin` directory (`DiskEngineV2` `manifest.v2.json` or legacy `head`/`snapshot`/`log`).
pub fn is_lin_data_dir(path: &Path) -> bool {
    is_disk_v2_dir(path)
        || (path.is_dir()
            && (path.join("head").is_file()
                || path.join("snapshot").is_file()
                || path.join("log").is_file()))
}

pub fn is_legacy_lin_v1_dir(path: &Path) -> bool {
    !is_disk_v2_dir(path)
        && path.is_dir()
        && (path.join("head").is_file()
            || path.join("snapshot").is_file()
            || path.join("log").is_file())
}

/// Compute total disk bytes occupied by a `lin` directory or portable backup file.
pub fn lin_disk_bytes(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return Some(meta.len());
    }
    if !meta.is_dir() {
        return None;
    }
    fn dir_bytes(dir: &Path) -> std::io::Result<u64> {
        let mut sum = 0u64;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_file() {
                sum = sum.saturating_add(entry.metadata()?.len());
            } else if ft.is_dir() {
                sum = sum.saturating_add(dir_bytes(&entry.path())?);
            }
        }
        Ok(sum)
    }
    dir_bytes(path).ok()
}

/// Bridge embedder wired into `lin::Db::with_embedder` so `Stmt::Insert` on `chunks`
/// stores exact pre-computed neural `Arc<[f32]>` vectors into `Cell::Vec` without lossy JSON.
#[derive(Debug)]
pub struct BridgeEmbedder {
    embed_id: String,
    staged: StdMutex<HashMap<String, Arc<[f32]>>>,
}

impl BridgeEmbedder {
    pub fn new(embed_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            embed_id: embed_id.into(),
            staged: StdMutex::new(HashMap::new()),
        })
    }

    pub fn stage(&self, id: String, vec: Arc<[f32]>) {
        let mut guard = self.staged.lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(id, vec);
    }

    pub fn stage_batch<I>(&self, items: I)
    where
        I: IntoIterator<Item = (String, Arc<[f32]>)>,
    {
        let mut guard = self.staged.lock().unwrap_or_else(|p| p.into_inner());
        for (id, vec) in items {
            guard.insert(id, vec);
        }
    }

    pub fn take_staged(&self, id: &str) -> Option<Arc<[f32]>> {
        let mut guard = self.staged.lock().unwrap_or_else(|p| p.into_inner());
        guard.remove(id)
    }

    pub fn clear_staged(&self) {
        let mut guard = self.staged.lock().unwrap_or_else(|p| p.into_inner());
        guard.clear();
    }
}

impl Embedder for BridgeEmbedder {
    fn id(&self) -> &str {
        &self.embed_id
    }

    fn dim(&self) -> usize {
        0
    }

    fn embed(&self, _text: &str) -> Arc<[f32]> {
        static EMPTY: std::sync::LazyLock<Arc<[f32]>> =
            std::sync::LazyLock::new(|| Arc::from(Vec::<f32>::new()));
        Arc::clone(&EMPTY)
    }

    fn embed_rows(&self, rows: &[&Row]) -> Vec<Arc<[f32]>> {
        static EMPTY: std::sync::LazyLock<Arc<[f32]>> =
            std::sync::LazyLock::new(|| Arc::from(Vec::<f32>::new()));
        let mut guard = self.staged.lock().unwrap_or_else(|p| p.into_inner());
        rows.iter()
            .map(|row| {
                row.get("id")
                    .and_then(Cell::text)
                    .and_then(|id| guard.remove(id))
                    .unwrap_or_else(|| Arc::clone(&EMPTY))
            })
            .collect()
    }
}

/// Underlying `lin` database instance held by `Store`.
#[allow(clippy::large_enum_variant)]
pub enum LinBackend {
    /// Out-of-core columnar `DiskEngineV2` (`mmap` segments + ACID WAL + L1 RAM `HotTierCache`).
    /// Scales up to 500+ GiB on disk while serving hot working sets directly from RAM!
    DiskV2 {
        engine: DiskEngineV2,
        bridge: Arc<BridgeEmbedder>,
        read_only_flag: bool,
    },
    /// Pure in-memory (`:memory:` / imported portable `.lin` file) or legacy v1 directory.
    Primary {
        db: Db,
        bridge: Arc<BridgeEmbedder>,
        read_only_flag: bool,
    },
    /// Concurrent OS-shared read-only snapshot (`Db::open_read`) over a legacy v1 `lin` directory.
    ReadOnlyDir {
        rdb: ReadDb,
    },
}

impl std::fmt::Debug for LinBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DiskV2 { read_only_flag, .. } => f
                .debug_struct("LinBackend::DiskV2")
                .field("read_only", read_only_flag)
                .finish(),
            Self::Primary { read_only_flag, .. } => f
                .debug_struct("LinBackend::Primary")
                .field("read_only", read_only_flag)
                .finish(),
            Self::ReadOnlyDir { .. } => f.debug_struct("LinBackend::ReadOnlyDir").finish(),
        }
    }
}

impl LinBackend {
    pub fn open_memory() -> Self {
        let bridge = BridgeEmbedder::new("nomic-embed-text/768");
        let db = Db::empty()
            .with_quotas(rag_lin_quotas())
            .with_embedder(bridge.clone());
        Self::Primary {
            db,
            bridge,
            read_only_flag: false,
        }
    }

    pub fn open_durable(path: &Path, sync: SyncMode) -> Result<Self> {
        let clean = normalize_lin_path(path);
        if clean.is_file() {
            if is_lin_portable_backup_file(&clean) {
                let bridge = BridgeEmbedder::new("nomic-embed-text/768");
                let db = Db::import_backup(&clean)?
                    .with_quotas(rag_lin_quotas())
                    .with_embedder(bridge.clone());
                return Ok(Self::Primary {
                    db,
                    bridge,
                    read_only_flag: false,
                });
            }
            return Err(AppError::db(format!(
                "path '{}' is a regular file (legacy DuckDB or non-lin file); expected a lin database directory or LIN\\x04 portable backup (run `migrate_to_lin` first)",
                path.display()
            )));
        }
        if is_legacy_lin_v1_dir(&clean) {
            let opts = OpenOpts {
                sync,
                cold: false,
            };
            let bridge = BridgeEmbedder::new("nomic-embed-text/768");
            let db = Db::open_with(&clean, opts)?
                .with_quotas(rag_lin_quotas())
                .with_embedder(bridge.clone());
            return Ok(Self::Primary {
                db,
                bridge,
                read_only_flag: false,
            });
        }
        let bridge = BridgeEmbedder::new("nomic-embed-text/768");
        let engine = DiskEngineV2::open_with_hot_cache(
            &clean,
            sync == SyncMode::Full,
            HotCacheConfig::default(),
        )?;
        // Pin tiny hot metadata tables into L1 RAM cache if already populated
        let _ = engine.warm_table_into_hot_cache("meta", Some(256));
        let _ = engine.warm_table_into_hot_cache("schema_version", Some(64));
        let _ = engine.warm_table_into_hot_cache("embedding_manifest", Some(16));
        Ok(Self::DiskV2 {
            engine,
            bridge,
            read_only_flag: false,
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self> {
        let clean = normalize_lin_path(path);
        if !clean.exists() {
            return Err(AppError::not_found(format!(
                "database '{}'",
                path.display()
            )));
        }
        if clean.is_file() {
            if is_lin_portable_backup_file(&clean) {
                let bridge = BridgeEmbedder::new("nomic-embed-text/768");
                let db = Db::import_backup(&clean)?
                    .with_quotas(rag_lin_quotas())
                    .with_embedder(bridge.clone());
                return Ok(Self::Primary {
                    db,
                    bridge,
                    read_only_flag: true,
                });
            }
            return Err(AppError::db(format!(
                "cannot open '{}' in read-only lin mode: expected a lin directory or LIN\\x04 backup file",
                path.display()
            )));
        }
        if is_disk_v2_dir(&clean) {
            let bridge = BridgeEmbedder::new("nomic-embed-text/768");
            let engine =
                DiskEngineV2::open_read_only_with_hot_cache(&clean, HotCacheConfig::default())?;
            let _ = engine.warm_table_into_hot_cache("meta", Some(256));
            let _ = engine.warm_table_into_hot_cache("schema_version", Some(64));
            let _ = engine.warm_table_into_hot_cache("embedding_manifest", Some(16));
            return Ok(Self::DiskV2 {
                engine,
                bridge,
                read_only_flag: true,
            });
        }
        let rdb = Db::open_read(&clean)?;
        Ok(Self::ReadOnlyDir { rdb })
    }

    pub fn hot_cache_stats(&self) -> Option<HotCacheStats> {
        match self {
            Self::DiskV2 { engine, .. } => Some(engine.hot_cache_stats()),
            _ => None,
        }
    }

    pub fn clear_hot_cache(&self) {
        if let Self::DiskV2 { engine, .. } = self {
            engine.clear_hot_cache();
        }
    }
}

/// Value conversion trait supporting `&String`, `&str`, `String`, `i64`, `i32`, `usize`, `f64`, `bool`, and `lin::Value`.
pub trait IntoLinValue {
    fn into_lin_value(self) -> Value;
}

impl IntoLinValue for Value {
    fn into_lin_value(self) -> Value {
        self
    }
}

impl IntoLinValue for &str {
    fn into_lin_value(self) -> Value {
        Value::String(Arc::from(self))
    }
}

impl IntoLinValue for String {
    fn into_lin_value(self) -> Value {
        Value::String(Arc::from(self))
    }
}

impl IntoLinValue for &String {
    fn into_lin_value(self) -> Value {
        Value::String(Arc::from(self.as_str()))
    }
}

impl IntoLinValue for i64 {
    fn into_lin_value(self) -> Value {
        Value::Int(self)
    }
}

impl IntoLinValue for i32 {
    fn into_lin_value(self) -> Value {
        Value::Int(i64::from(self))
    }
}

impl IntoLinValue for usize {
    fn into_lin_value(self) -> Value {
        Value::Int(self as i64)
    }
}

impl IntoLinValue for f64 {
    fn into_lin_value(self) -> Value {
        val_f64(self)
    }
}

impl IntoLinValue for f32 {
    fn into_lin_value(self) -> Value {
        val_f64(f64::from(self))
    }
}

impl IntoLinValue for bool {
    fn into_lin_value(self) -> Value {
        Value::Bool(self)
    }
}

/// Optional string argument trait so `RecBuilder::opt_string` accepts both `&Option<String>` and owned `Option<String>`.
pub trait OptStrArg {
    fn as_opt_str(&self) -> Option<&str>;
}

impl OptStrArg for Option<String> {
    fn as_opt_str(&self) -> Option<&str> {
        self.as_deref()
    }
}

impl OptStrArg for &Option<String> {
    fn as_opt_str(&self) -> Option<&str> {
        self.as_deref()
    }
}

impl OptStrArg for Option<&str> {
    fn as_opt_str(&self) -> Option<&str> {
        *self
    }
}

impl OptStrArg for &Option<&str> {
    fn as_opt_str(&self) -> Option<&str> {
        **self
    }
}

impl OptStrArg for Option<&String> {
    fn as_opt_str(&self) -> Option<&str> {
        self.map(String::as_str)
    }
}

/// Shared read guard over `LinBackend` (`Multiple concurrent readers, zero Store cloning`).
pub struct LinReadGuard<'a> {
    guard: RwLockReadGuard<'a, LinBackend>,
}

/// Exclusive write guard over `LinBackend` (`Single writer, ACID rollback via LinTx`).
pub struct LinWriteGuard<'a> {
    guard: RwLockWriteGuard<'a, LinBackend>,
}

#[allow(clippy::large_enum_variant)]
enum LinTxInner<'a> {
    DiskV2 {
        engine: &'a mut DiskEngineV2,
        committed_or_rolled_back: bool,
    },
    Legacy(lin::Transaction<'a>),
    Finished,
}

/// ACID transaction wrapper over `&mut DiskEngineV2` or `&mut lin::Db` with automatic rollback on drop.
pub struct LinTx<'a> {
    inner: LinTxInner<'a>,
    bridge: Arc<BridgeEmbedder>,
}

impl Drop for LinTx<'_> {
    fn drop(&mut self) {
        if let LinTxInner::DiskV2 {
            engine,
            committed_or_rolled_back,
        } = &mut self.inner
            && !*committed_or_rolled_back
        {
            self.bridge.clear_staged();
            engine.rollback_savepoint();
            *committed_or_rolled_back = true;
        }
    }
}

impl Deref for LinReadGuard<'_> {
    type Target = LinBackend;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl Deref for LinWriteGuard<'_> {
    type Target = LinBackend;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for LinWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn value_to_disk_cell(v: &Value, now_ms: i64) -> Cell {
    if is_null_param(v) {
        return Cell::Null;
    }
    match v {
        Value::String(s) => Cell::Text(Arc::clone(s)),
        Value::Int(n) => Cell::Int(*n),
        Value::Float(f) => Cell::Float(*f),
        Value::Bool(b) => Cell::Bool(*b),
        Value::Timestamp(ms) => Cell::Time(*ms),
        Value::Now => Cell::Time(now_ms),
        Value::NowMinus(d) => Cell::Time(now_ms - d.as_millis()),
        Value::Duration(d) => Cell::Int(d.as_millis()),
        Value::Name(n) => Cell::from_text(n.as_str()),
        Value::Param(_) => Cell::Null,
    }
}

fn fields_to_disk_row(
    collection: &str,
    fields: Vec<(String, Value)>,
    bridge: &BridgeEmbedder,
    now_ms: i64,
) -> Row {
    let mut row = Row::new();
    let mut row_id: Option<String> = None;
    for (k, v) in fields {
        if is_null_param(&v) {
            continue;
        }
        let cell = value_to_disk_cell(&v, now_ms);
        if k == "id"
            && let Some(s) = cell.text()
        {
            row_id = Some(s.to_string());
        }
        if !matches!(cell, Cell::Null) {
            row.insert(k, cell);
        }
    }
    if collection == "chunks"
        && let Some(ref id) = row_id
        && let Some(vec) = bridge.take_staged(id)
        && !vec.is_empty()
    {
        row.insert("embedding".to_string(), Cell::Vec(vec));
    }
    row
}

/// Unified read operations implemented by `LinBackend`, `LinReadGuard`, `LinWriteGuard`, and `LinTx`
/// so all `*_locked` helpers work seamlessly across `&r`, `&*r`, `&w`, `&*w`, and `&tx`!
pub trait LinReader {
    /// Direct access to out-of-core `DiskEngineV2` + L1 `HotTierCache` when backed by `DiskV2`.
    fn disk_v2(&self) -> Option<&DiskEngineV2> {
        None
    }

    /// Fast direct access to `&lin::Db` when backed by `Primary` (in-memory `:memory:` or imported backup).
    fn primary_db(&self) -> Option<&Db>;

    /// Check whether a collection is declared in the catalog.
    fn has_collection(&self, collection: &str) -> bool {
        if let Some(eng) = self.disk_v2() {
            return eng.has_table(collection);
        }
        if let Some(db) = self.primary_db() {
            db.catalog.collection(collection).is_some()
        } else {
            self.run_query_rows(Query {
                source: Source::Collection(collection.to_string()),
                steps: vec![Step::Take { n: Some(0) }],
                explain: None,
                ignore_filter: true,
            })
            .is_ok()
        }
    }

    /// Execute a `lin::Query` and materialize matching rows.
    fn run_query_rows(&self, query: Query) -> Result<Vec<Row>>;

    /// Fetch a single row by primary key `id` in O(1) (served from L0 MemTable -> L1 HotTierCache -> L2 mmap!).
    fn get_by_id(&self, collection: &str, id: &str) -> Result<Option<Row>> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.get_by_id(collection, id)?);
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(db.store.get_by_id(collection, id).cloned());
        }
        let rows = self.query_filter(collection, Some(lin::query::pred::eq("id", id)), None)?;
        Ok(rows.into_iter().next())
    }

    /// Zero-clone point lookup by primary key `id`: passes `&Row` directly from `Primary` RAM or `DiskV2` `Arc<Row>`!
    fn with_by_id<T, F>(&self, collection: &str, id: &str, f: F) -> Result<Option<T>>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return match eng.get_arc_by_id(collection, id)? {
                Some(arc_row) => Ok(Some(f(&arc_row)?)),
                None => Ok(None),
            };
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return match db.store.get_by_id(collection, id) {
                Some(row) => Ok(Some(f(row)?)),
                None => Ok(None),
            };
        }
        match self.get_by_id(collection, id)? {
            Some(row) => Ok(Some(f(&row)?)),
            None => Ok(None),
        }
    }

    /// Fetch only projected fields for primary key `id` in O(1) without decompressing large columns!
    fn get_projected_by_id(
        &self,
        collection: &str,
        id: &str,
        fields: &[&str],
    ) -> Result<Option<Row>> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.get_projected_by_id(collection, id, fields)?);
        }
        self.get_by_id(collection, id)
    }

    /// Check whether a row with primary key `id` exists in O(1) without cloning the `Row` or decompressing text!
    fn exists_by_id(&self, collection: &str, id: &str) -> Result<bool> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.exists_by_id(collection, id));
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(db.store.row_index(collection, id).is_some());
        }
        Ok(self.get_by_id(collection, id)?.is_some())
    }

    /// Count all rows in a collection in O(1).
    fn count_all(&self, collection: &str) -> Result<usize> {
        self.count_rows(collection, None)
    }

    /// Count rows in a collection (`O(#segments)` nanoseconds when unfiltered on `DiskV2`).
    fn count_rows(&self, collection: &str, pred: Option<Pred>) -> Result<usize> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            if let Some(ref p) = pred {
                let now_ms = Utc::now().timestamp_millis();
                return Ok(eng
                    .query_filter(collection, Some(p), None, Some(&["id"]), now_ms)?
                    .len());
            }
            return Ok(eng.count_table(collection));
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            if pred.is_none() {
                return Ok(db.store.collection(collection).len());
            }
        }
        let mut steps = Vec::with_capacity(2);
        if let Some(p) = pred {
            steps.push(Step::Filter(p));
        }
        steps.push(Step::Count { by: None });
        let rows = self.run_query_rows(Query {
            source: Source::Collection(collection.to_string()),
            steps,
            explain: None,
            ignore_filter: true,
        })?;
        let n = rows
            .first()
            .and_then(|r| {
                cell_i64_opt(r, "hits")
                    .or_else(|| cell_i64_opt(r, "n"))
                    .or_else(|| cell_i64_opt(r, "count"))
            })
            .unwrap_or(0);
        Ok(n.max(0) as usize)
    }

    /// Scan all rows in `collection`, streaming out-of-core on `DiskV2` without accumulating the table in RAM!
    fn scan_each<F>(&self, collection: &str, mut visit: F) -> Result<()>
    where
        F: FnMut(&Row) -> Result<bool>,
    {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let mut app_err: Option<AppError> = None;
            eng.scan_projected(collection, None, |row| match visit(row) {
                Ok(cont) => Ok(cont),
                Err(e) => {
                    app_err = Some(e);
                    Ok(false)
                }
            })?;
            if let Some(e) = app_err {
                return Err(e);
            }
            return Ok(());
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            for row in db.store.collection(collection) {
                if !visit(row)? {
                    break;
                }
            }
            return Ok(());
        }
        let rows = self.query_filter(collection, None, None)?;
        for row in &rows {
            if !visit(row)? {
                break;
            }
        }
        Ok(())
    }

    /// Stream only projected columns (`fields`) of `collection` out-of-core without decompressing unused columns!
    fn scan_each_projected<F>(&self, collection: &str, fields: &[&str], mut visit: F) -> Result<()>
    where
        F: FnMut(&Row) -> Result<bool>,
    {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let mut app_err: Option<AppError> = None;
            eng.scan_projected(collection, Some(fields), |row| match visit(row) {
                Ok(cont) => Ok(cont),
                Err(e) => {
                    app_err = Some(e);
                    Ok(false)
                }
            })?;
            if let Some(e) = app_err {
                return Err(e);
            }
            return Ok(());
        }
        self.scan_each(collection, visit)
    }

    /// Collect mapped rows from `collection` matching a Rust closure filter.
    fn filter_map_rows<T, F>(&self, collection: &str, mut f: F) -> Result<Vec<T>>
    where
        F: FnMut(&Row) -> Result<Option<T>>,
    {
        let mut out = Vec::new();
        self.scan_each(collection, |row| {
            if let Some(item) = f(row)? {
                out.push(item);
            }
            Ok(true)
        })?;
        Ok(out)
    }

    /// Query rows using `lin`'s indexed predicate engine (`Pred`), optional `limit`.
    fn query_filter(
        &self,
        collection: &str,
        pred: Option<Pred>,
        limit: Option<usize>,
    ) -> Result<Vec<Row>> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let now_ms = Utc::now().timestamp_millis();
            return Ok(eng.query_filter(collection, pred.as_ref(), limit, None, now_ms)?);
        }
        let mut steps = Vec::with_capacity(3);
        if let Some(p) = pred {
            let needs_uri_point_bypass = collection != "docs" && pred_has_top_uri_eq(&p);
            steps.push(Step::Filter(p));
            if needs_uri_point_bypass {
                steps.push(Step::Filter(Pred::IsNull {
                    field: lin::Field::path("id"),
                    negated: true,
                }));
            }
        }
        steps.push(Step::Take {
            n: limit.map(|n| n as i64),
        });
        self.run_query_rows(Query {
            source: Source::Collection(collection.to_string()),
            steps,
            explain: None,
            ignore_filter: true,
        })
    }

    /// Seek rows by exact equality on an indexed single-column `var`/`id`/`i64` field.
    fn find_by_eq(
        &self,
        collection: &str,
        field: &str,
        val: impl IntoLinValue,
    ) -> Result<Vec<Row>> {
        let v = val.into_lin_value();
        if field == "id" {
            if let Value::String(id) = &v {
                return Ok(self.get_by_id(collection, id)?.into_iter().collect());
            }
        }
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let key_cell = value_to_disk_cell(&v, Utc::now().timestamp_millis());
            let mut out = Vec::new();
            eng.for_each_by_eq_projected(collection, field, &key_cell, None, |row| {
                out.push(row.clone());
                Ok(true)
            })?;
            return Ok(out);
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            if let Value::String(s) = &v {
                let mut label = String::with_capacity(collection.len() + field.len() + 2);
                label.push_str(collection);
                label.push('[');
                label.push_str(field);
                label.push(']');
                if let Some(live_idx) = db.store.indexes.get(&label)
                    && live_idx.def.pred.is_none()
                    && live_idx.def.fields.len() == 1
                {
                    let key = with_probe_row(field, Cell::from_text(s.as_ref()), |probe| {
                        live_idx.key_of(probe)
                    });
                    let mut out = Vec::new();
                    if let Some(posting) = live_idx.forward.get(&key) {
                        let rows = db.store.collection(collection);
                        posting.for_each_ordered(false, |idx| {
                            if let Some(row) = rows.get(idx) {
                                out.push(row.clone());
                            }
                            true
                        });
                    }
                    return Ok(out);
                }
            }
        }
        self.query_filter(
            collection,
            Some(Pred::Cmp {
                field: lin::Field::path(field),
                op: lin::CmpOp::Eq,
                value: v,
            }),
            None,
        )
    }

    /// Count rows matching exact equality on an indexed field (`0` row allocations on `DiskEngineV2`!).
    fn count_by_eq(&self, collection: &str, field: &str, val: impl IntoLinValue) -> Result<usize> {
        let v = val.into_lin_value();
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let key_cell = value_to_disk_cell(&v, 0);
            return Ok(eng.count_by_eq(collection, field, &key_cell)?);
        }
        let mut n = 0usize;
        self.for_each_by_eq_projected(collection, field, v, Some(&["id"]), |_| {
            n += 1;
            Ok(true)
        })?;
        Ok(n)
    }

    /// Visitor over rows matching exact equality on a single-column indexed field, projecting only `fields`.
    #[allow(clippy::too_many_lines)]
    fn for_each_by_eq_projected<F>(
        &self,
        collection: &str,
        field: &str,
        val: impl IntoLinValue,
        fields: Option<&[&str]>,
        mut visit: F,
    ) -> Result<()>
    where
        F: FnMut(&Row) -> Result<bool>,
    {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let v = val.into_lin_value();
            let key_cell = value_to_disk_cell(&v, 0);
            let mut app_err: Option<AppError> = None;
            eng.for_each_by_eq_projected(collection, field, &key_cell, fields, |row| {
                match visit(row) {
                    Ok(cont) => Ok(cont),
                    Err(e) => {
                        app_err = Some(e);
                        Ok(false)
                    }
                }
            })?;
            if let Some(e) = app_err {
                return Err(e);
            }
            return Ok(());
        }
        self.for_each_by_eq(collection, field, val, visit)
    }

    /// Visitor over rows matching exact equality on a single-column indexed field.
    fn for_each_by_eq<F>(
        &self,
        collection: &str,
        field: &str,
        val: impl IntoLinValue,
        mut visit: F,
    ) -> Result<()>
    where
        F: FnMut(&Row) -> Result<bool>,
    {
        if self.disk_v2().is_some() {
            return self.for_each_by_eq_projected(collection, field, val, None, visit);
        }
        if let Some(db) = self.primary_db() {
            if db.catalog.collection(collection).is_none() {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let v = val.into_lin_value();
            if field == "id" {
                if let Value::String(id) = &v {
                    if let Some(row) = db.store.get_by_id(collection, id) {
                        let _ = visit(row)?;
                    }
                    return Ok(());
                }
            }
            if let Value::String(s) = &v {
                let mut label = String::with_capacity(collection.len() + field.len() + 2);
                label.push_str(collection);
                label.push('[');
                label.push_str(field);
                label.push(']');
                if let Some(live_idx) = db.store.indexes.get(&label)
                    && live_idx.def.pred.is_none()
                    && live_idx.def.fields.len() == 1
                {
                    let key = with_probe_row(field, Cell::from_text(s.as_ref()), |probe| {
                        live_idx.key_of(probe)
                    });
                    if let Some(posting) = live_idx.forward.get(&key) {
                        let rows = db.store.collection(collection);
                        let mut idxs = Vec::with_capacity(posting.len());
                        posting.extend_into(&mut idxs);
                        for idx in idxs {
                            if let Some(row) = rows.get(idx) {
                                if !visit(row)? {
                                    break;
                                }
                            }
                        }
                    }
                    return Ok(());
                }
            }
            for row in self.find_by_eq(collection, field, v)? {
                if !visit(&row)? {
                    break;
                }
            }
            return Ok(());
        }
        for row in self.find_by_eq(collection, field, val)? {
            if !visit(&row)? {
                break;
            }
        }
        Ok(())
    }

    /// Zero-`Row` secondary-index visitor yielding up to 3 borrowed `&str` columns directly!
    #[allow(clippy::too_many_arguments)]
    fn for_each_by_eq_str3<F>(
        &self,
        collection: &str,
        index_field: &str,
        key_str: &str,
        col1: &str,
        col2: &str,
        col3: Option<&str>,
        mut visit: F,
    ) -> Result<()>
    where
        F: FnMut(Option<&str>, Option<&str>, Option<&str>) -> Result<bool>,
    {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let mut app_err: Option<AppError> = None;
            eng.for_each_by_eq_str3(
                collection,
                index_field,
                key_str,
                col1,
                col2,
                col3,
                |s1, s2, s3| match visit(s1, s2, s3) {
                    Ok(cont) => Ok(cont),
                    Err(e) => {
                        app_err = Some(e);
                        Ok(false)
                    }
                },
            )?;
            if let Some(e) = app_err {
                return Err(e);
            }
            return Ok(());
        }
        self.for_each_by_eq(collection, index_field, key_str, |row| {
            let s1 = cell_str(row, col1);
            let s2 = cell_str(row, col2);
            let s3 = col3.and_then(|c| cell_str(row, c));
            visit(s1, s2, s3)
        })
    }

    /// Columnar 1-column `GROUP BY` (`col` -> count) using `TextDict` ordinals on `DiskEngineV2`!
    #[allow(clippy::type_complexity)]
    fn group_count_str1(
        &self,
        collection: &str,
        col: &str,
    ) -> Result<(u64, Vec<(Option<String>, u64)>)> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.group_count_str1(collection, col)?);
        }
        let mut total = 0u64;
        let mut map: std::collections::HashMap<Option<String>, u64> =
            std::collections::HashMap::new();
        self.scan_each(collection, |row| {
            total += 1;
            let k = cell_str(row, col).map(str::to_string);
            *map.entry(k).or_insert(0) += 1;
            Ok(true)
        })?;
        Ok((total, map.into_iter().collect()))
    }

    /// Columnar 2-column `GROUP BY` (`(col1, col2)` -> count) using `TextDict` ordinals on `DiskEngineV2`!
    #[allow(clippy::type_complexity)]
    fn group_count_str2(
        &self,
        collection: &str,
        col1: &str,
        col2: &str,
    ) -> Result<(u64, Vec<((Option<String>, Option<String>), u64)>)> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.group_count_str2(collection, col1, col2)?);
        }
        let mut total = 0u64;
        let mut map: std::collections::HashMap<(Option<String>, Option<String>), u64> =
            std::collections::HashMap::new();
        self.scan_each(collection, |row| {
            total += 1;
            let k1 = cell_str(row, col1).map(str::to_string);
            let k2 = cell_str(row, col2).map(str::to_string);
            *map.entry((k1, k2)).or_insert(0) += 1;
            Ok(true)
        })?;
        Ok((total, map.into_iter().collect()))
    }

    /// Columnar 3-column `GROUP BY` (`(col1, col2, col3)` -> count) using `TextDict` ordinals on `DiskEngineV2`!
    #[allow(clippy::type_complexity)]
    fn group_count_str3(
        &self,
        collection: &str,
        col1: &str,
        col2: &str,
        col3: &str,
    ) -> Result<(u64, Vec<((Option<String>, Option<String>, Option<String>), u64)>)> {
        if let Some(eng) = self.disk_v2() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            return Ok(eng.group_count_str3(collection, col1, col2, col3)?);
        }
        let mut total = 0u64;
        let mut map: std::collections::HashMap<
            (Option<String>, Option<String>, Option<String>),
            u64,
        > = std::collections::HashMap::new();
        self.scan_each(collection, |row| {
            total += 1;
            let k1 = cell_str(row, col1).map(str::to_string);
            let k2 = cell_str(row, col2).map(str::to_string);
            let k3 = cell_str(row, col3).map(str::to_string);
            *map.entry((k1, k2, k3)).or_insert(0) += 1;
            Ok(true)
        })?;
        Ok((total, map.into_iter().collect()))
    }
}

fn with_probe_row<R>(field: &str, cell: Cell, f: impl FnOnce(&Row) -> R) -> R {
    use std::cell::RefCell;
    thread_local! {
        static PROBE_ROW: RefCell<Row> = const { RefCell::new(Row::new()) };
    }
    PROBE_ROW.with(|slot| {
        let mut row = slot.borrow_mut();
        row.clear();
        row.insert(field.to_string(), cell);
        let out = f(&row);
        row.clear();
        out
    })
}

fn pred_has_top_uri_eq(pred: &Pred) -> bool {
    match pred {
        Pred::Cmp {
            field,
            op: lin::CmpOp::Eq,
            value: Value::String(_),
        } => field.parts.len() == 1 && field.parts[0] == "uri",
        Pred::And(a, b) => pred_has_top_uri_eq(a) || pred_has_top_uri_eq(b),
        _ => false,
    }
}

/// Unified write operations implemented by `LinBackend`, `LinWriteGuard`, and `LinTx`
/// so all `*_locked` write helpers work identically standalone or inside `with_tx`!
pub trait LinWriter: LinReader {
    fn disk_v2_mut(&mut self) -> Option<&mut DiskEngineV2> {
        None
    }
    fn bridge_embedder(&self) -> &Arc<BridgeEmbedder>;
    fn exec_stmt_mut(&mut self, stmt: Stmt) -> Result<usize>;
    fn exec_stmt_params_mut(&mut self, stmt: Stmt, params: &[(&str, Cell)]) -> Result<usize>;

    /// Declare a collection if it does not already exist in the catalog.
    fn ensure_col(&mut self, name: &str, append: bool, fields: &[(&str, &str)]) -> Result<()> {
        if self.has_collection(name) {
            return Ok(());
        }
        if let Some(eng) = self.disk_v2_mut() {
            let mut columns = Vec::with_capacity(fields.len());
            for &(fname, ftype) in fields {
                if name == "chunks" && fname == "vec" {
                    continue;
                }
                let kind = if (name == "chunks" && fname == "embedding") || ftype == "vec" {
                    DiskColKind::VecF32Sq8 { dim: 0 }
                } else {
                    match ftype {
                        "i64" | "int" => DiskColKind::Int64,
                        "f64" | "float" => DiskColKind::Float64,
                        "bool" => DiskColKind::Bool,
                        "time" => DiskColKind::Time64,
                        _ => DiskColKind::TextZstd,
                    }
                };
                columns.push(DiskColSchema {
                    name: fname.to_string(),
                    kind,
                });
            }
            let indexed_fields = rag_default_indexed_fields()
                .remove(name)
                .unwrap_or_default();
            eng.commit_ops(vec![DiskWalOp::EnsureTable {
                schema: DiskTableSchema {
                    name: name.to_string(),
                    columns,
                    indexed_fields,
                },
            }])?;
            return Ok(());
        }
        let mut src = String::new();
        src.push_str("col ");
        src.push_str(name);
        if append {
            src.push_str(" append");
        }
        src.push_str(" { ");
        for (i, (fname, ftype)) in fields.iter().enumerate() {
            if i > 0 {
                src.push_str(", ");
            }
            src.push_str(fname);
            if !ftype.is_empty() && fname != ftype {
                src.push_str(": ");
                src.push_str(ftype);
            }
        }
        src.push_str(" }");
        let stmt = lin::parse(&src)?;
        self.exec_stmt_mut(stmt)?;
        Ok(())
    }

    /// Declare a secondary index if it does not already exist in the catalog.
    fn ensure_index(&mut self, collection: &str, unique: bool, fields: &[&str]) -> Result<()> {
        if let Some(eng) = self.disk_v2_mut() {
            if fields.len() == 1 && fields[0] != "id" {
                let f = fields[0];
                let already = eng
                    .manifest
                    .tables
                    .get(collection)
                    .is_some_and(|t| t.indexed_fields.iter().any(|x| x == f));
                if !already {
                    eng.commit_ops(vec![DiskWalOp::EnsureIndex {
                        table: collection.to_string(),
                        field: f.to_string(),
                    }])?;
                }
            }
            return Ok(());
        }
        let index_exists = self.primary_db().is_some_and(|db| {
            db.catalog.indexes.values().any(|idx| {
                idx.collection == collection
                    && idx.unique == unique
                    && idx.pred.is_none()
                    && idx.fields.len() == fields.len()
                    && idx.fields.iter().zip(fields.iter()).all(|(a, b)| a == b)
            })
        });
        if index_exists {
            return Ok(());
        }
        let mut src = String::from("index ");
        src.push_str(collection);
        if unique {
            src.push_str(" unique");
        }
        src.push_str(" [");
        src.push_str(&fields.join(", "));
        src.push(']');
        let stmt = lin::parse(&src)?;
        match self.exec_stmt_mut(stmt) {
            Ok(_) => Ok(()),
            Err(AppError::Db(msg)) if msg.contains("already exists") || msg.contains("duplicate") => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Insert records in safe WAL-bounded batches (`<= 3 MiB` and `<= 512` rows per frame).
    fn insert_records(
        &mut self,
        collection: &str,
        records: Vec<Vec<(String, Value)>>,
    ) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        if self.disk_v2().is_some() {
            let bridge = Arc::clone(self.bridge_embedder());
            let now_ms = Utc::now().timestamp_millis();
            let mut rows = Vec::with_capacity(records.len());
            for rec in records {
                let row = fields_to_disk_row(collection, rec, &bridge, now_ms);
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            let count = rows.len();
            if count > 0
                && let Some(eng) = self.disk_v2_mut()
            {
                eng.commit_ops(vec![DiskWalOp::UpsertBatch {
                    table: collection.to_string(),
                    rows,
                }])?;
            }
            return Ok(count);
        }
        let mut total = 0usize;
        let mut batch: Vec<Vec<(String, Value)>> = Vec::new();
        let mut batch_bytes = 0usize;

        for rec in records {
            let est = estimate_record_bytes(&rec);
            if !batch.is_empty()
                && (batch.len() >= MAX_INSERT_BATCH_ROWS
                    || batch_bytes.saturating_add(est) > MAX_INSERT_BATCH_BYTES)
            {
                total += self.flush_insert_batch(collection, std::mem::take(&mut batch))?;
                batch_bytes = 0;
            }
            batch_bytes = batch_bytes.saturating_add(est);
            batch.push(rec);
        }
        if !batch.is_empty() {
            total += self.flush_insert_batch(collection, batch)?;
        }
        Ok(total)
    }

    fn flush_insert_batch(
        &mut self,
        collection: &str,
        batch: Vec<Vec<(String, Value)>>,
    ) -> Result<usize> {
        if batch.is_empty() {
            return Ok(0);
        }
        if self.disk_v2().is_some() {
            let bridge = Arc::clone(self.bridge_embedder());
            let now_ms = Utc::now().timestamp_millis();
            let mut rows = Vec::with_capacity(batch.len());
            for rec in batch {
                let row = fields_to_disk_row(collection, rec, &bridge, now_ms);
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            let count = rows.len();
            if count > 0
                && let Some(eng) = self.disk_v2_mut()
            {
                eng.commit_ops(vec![DiskWalOp::UpsertBatch {
                    table: collection.to_string(),
                    rows,
                }])?;
            }
            return Ok(count);
        }
        let mut filtered: Vec<Vec<(String, Value)>> = Vec::with_capacity(batch.len());
        for rec in batch {
            let clean: Vec<(String, Value)> = rec
                .into_iter()
                .filter(|(_, v)| !is_null_param(v))
                .collect();
            if !clean.is_empty() {
                filtered.push(clean);
            }
        }
        if filtered.is_empty() {
            return Ok(0);
        }
        let count = filtered.len();
        let src = format_insert_src(collection, &filtered);
        let stmt = lin::parse(&src)?;
        self.exec_stmt_mut(stmt)?;
        Ok(count)
    }

    /// Insert a single record.
    fn insert_one(&mut self, collection: &str, fields: Vec<(String, Value)>) -> Result<()> {
        self.insert_records(collection, vec![fields])?;
        Ok(())
    }

    /// Upsert a single record by `"id"` (updates if `id` exists, inserts otherwise).
    fn upsert_by_id(
        &mut self,
        collection: &str,
        id: &str,
        fields: Vec<(String, Value)>,
    ) -> Result<()> {
        if self.disk_v2().is_some() {
            let bridge = Arc::clone(self.bridge_embedder());
            let now_ms = Utc::now().timestamp_millis();
            let mut row = self
                .get_by_id(collection, id)?
                .unwrap_or_else(|| {
                    let mut r = Row::new();
                    r.insert("id".to_string(), Cell::from_text(id));
                    r
                });
            row.insert("id".to_string(), Cell::from_text(id));
            for (k, v) in fields {
                if k == "id" {
                    continue;
                }
                if is_null_param(&v) {
                    row.remove(&k);
                } else {
                    let c = value_to_disk_cell(&v, now_ms);
                    if matches!(c, Cell::Null) {
                        row.remove(&k);
                    } else {
                        row.insert(k, c);
                    }
                }
            }
            if collection == "chunks"
                && let Some(vec) = bridge.take_staged(id)
                && !vec.is_empty()
            {
                row.insert("embedding".to_string(), Cell::Vec(vec));
            }
            if let Some(eng) = self.disk_v2_mut() {
                eng.commit_ops(vec![DiskWalOp::UpsertBatch {
                    table: collection.to_string(),
                    rows: vec![row],
                }])?;
            }
            return Ok(());
        }
        if self.get_by_id(collection, id)?.is_some() {
            let update_fields: Vec<(String, Value)> = fields
                .into_iter()
                .filter(|(k, _)| k != "id")
                .collect();
            if !update_fields.is_empty() {
                self.update_where(
                    collection,
                    lin::query::pred::eq("id", id),
                    update_fields,
                )?;
            }
            Ok(())
        } else {
            let mut all_fields = Vec::with_capacity(fields.len() + 1);
            if !fields.iter().any(|(k, _)| k == "id") {
                all_fields.push(("id".to_string(), Value::String(Arc::from(id))));
            }
            all_fields.extend(fields);
            self.insert_one(collection, all_fields)
        }
    }

    /// Upsert a single record using its `"id"` field (`Value::String`).
    fn upsert(&mut self, collection: &str, fields: Vec<(String, Value)>) -> Result<()> {
        let id = fields
            .iter()
            .find_map(|(k, v)| match (k.as_str(), v) {
                ("id", Value::String(s)) => Some(s.to_string()),
                _ => None,
            })
            .ok_or_else(|| AppError::db(format!("upsert into {collection} missing string 'id' field")))?;
        self.upsert_by_id(collection, &id, fields)
    }

    /// Update matching rows (`cas each`), returning `Ok(0)` instead of error when 0 rows match.
    /// Null fields (`Value::Param("__null")`) evaluate natively to `Cell::Null` in `lin`.
    fn update_where(
        &mut self,
        collection: &str,
        pred: Pred,
        fields: Vec<(String, Value)>,
    ) -> Result<usize> {
        if fields.is_empty() {
            return Ok(0);
        }
        if self.disk_v2().is_some() {
            let bridge = Arc::clone(self.bridge_embedder());
            let now_ms = Utc::now().timestamp_millis();
            if let Some(eng) = self.disk_v2_mut() {
                if !eng.has_table(collection) {
                    return Err(AppError::db(format!("missing collection: {collection}")));
                }
                let patch: Vec<(String, Cell)> = fields
                    .into_iter()
                    .map(|(k, v)| {
                        if is_null_param(&v) {
                            (k, Cell::Null)
                        } else {
                            (k, value_to_disk_cell(&v, now_ms))
                        }
                    })
                    .collect();
                let mut matched = eng.query_filter(collection, Some(&pred), None, None, now_ms)?;
                if matched.is_empty() {
                    return Ok(0);
                }
                for row in &mut matched {
                    for (k, v) in &patch {
                        if matches!(v, Cell::Null) {
                            row.remove(k);
                        } else {
                            row.insert(k.clone(), v.clone());
                        }
                    }
                    if collection == "chunks"
                        && let Some(id) = row.get("id").and_then(Cell::text)
                        && let Some(vec) = bridge.take_staged(id)
                    {
                        if vec.is_empty() {
                            row.remove("embedding");
                        } else {
                            row.insert("embedding".to_string(), Cell::Vec(vec));
                        }
                    }
                }
                let n = matched.len();
                eng.commit_ops(vec![DiskWalOp::UpsertBatch {
                    table: collection.to_string(),
                    rows: matched,
                }])?;
                return Ok(n);
            }
            return Err(AppError::db("cannot mutate read-only DiskV2 lin store"));
        }
        let has_null = fields.iter().any(|(_, v)| is_null_param(v));
        let src = format_update_src(collection, &pred, &fields);
        let stmt = lin::parse(&src)?;
        let res = if has_null {
            let params = [("__null", Cell::Null)];
            self.exec_stmt_params_mut(stmt, &params)
        } else {
            self.exec_stmt_mut(stmt)
        };
        match res {
            Ok(n) => Ok(n),
            Err(AppError::Db(msg)) if msg.contains("update: no matching row") => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Delete matching rows (`cas each`), returning `Ok(0)` instead of error when 0 rows match.
    fn delete_where(&mut self, collection: &str, pred: Pred) -> Result<usize> {
        if let Some(eng) = self.disk_v2_mut() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let now_ms = Utc::now().timestamp_millis();
            return Ok(eng.delete_where(collection, &pred, now_ms)?);
        }
        let stmt = Stmt::Delete {
            collection: collection.to_string(),
            pred,
            cas: None,
            cas_each: true,
        };
        match self.exec_stmt_mut(stmt) {
            Ok(n) => Ok(n),
            Err(AppError::Db(msg)) if msg.contains("delete: no matching row") => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Delete a single row by `"id"`, returning `true` if deleted.
    fn delete_by_id(&mut self, collection: &str, id: &str) -> Result<bool> {
        if let Some(eng) = self.disk_v2_mut() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            if !eng.exists_by_id(collection, id) {
                return Ok(false);
            }
            eng.commit_ops(vec![DiskWalOp::DeleteIds {
                table: collection.to_string(),
                ids: vec![id.to_string()],
            }])?;
            return Ok(true);
        }
        Ok(self.delete_where(collection, lin::query::pred::eq("id", id))? > 0)
    }

    /// Delete multiple rows by `"id"` in chunks.
    fn delete_by_ids(&mut self, collection: &str, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        if let Some(eng) = self.disk_v2_mut() {
            if !eng.has_table(collection) {
                return Err(AppError::db(format!("missing collection: {collection}")));
            }
            let mut live_ids = Vec::with_capacity(ids.len());
            for id in ids {
                if eng.exists_by_id(collection, id) {
                    live_ids.push(id.clone());
                }
            }
            let n = live_ids.len();
            if n > 0 {
                eng.commit_ops(vec![DiskWalOp::DeleteIds {
                    table: collection.to_string(),
                    ids: live_ids,
                }])?;
            }
            return Ok(n);
        }
        let mut total = 0usize;
        for chunk in ids.chunks(500) {
            let pred = if chunk.len() == 1 {
                lin::query::pred::eq("id", chunk[0].as_str())
            } else {
                lin::query::pred::in_list("id", chunk.iter().map(String::as_str))
            };
            total += self.delete_where(collection, pred)?;
        }
        Ok(total)
    }
}

fn run_disk_v2_query_rows(eng: &DiskEngineV2, query: Query) -> Result<Vec<Row>> {
    let Source::Collection(collection) = query.source else {
        return Err(AppError::db(
            "DiskEngineV2 query source only supports direct collections",
        ));
    };
    if !eng.has_table(&collection) {
        return Err(AppError::db(format!("missing collection: {collection}")));
    }
    let mut combined_pred: Option<Pred> = None;
    let mut limit: Option<usize> = None;
    let mut want_count = false;
    for step in query.steps {
        match step {
            Step::Filter(p) => {
                combined_pred = Some(match combined_pred {
                    Some(prev) => Pred::And(Box::new(prev), Box::new(p)),
                    None => p,
                });
            }
            Step::Take { n: Some(n) } => {
                let take = n.max(0) as usize;
                limit = Some(limit.map_or(take, |cur| cur.min(take)));
            }
            Step::Take { n: None } => {}
            Step::Count { by: None } => {
                want_count = true;
            }
            other => {
                return Err(AppError::db(format!(
                    "unsupported query step on DiskEngineV2: {other:?}"
                )));
            }
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    if want_count {
        let count = if let Some(ref p) = combined_pred {
            eng.query_filter(&collection, Some(p), limit, Some(&["id"]), now_ms)?
                .len()
        } else {
            let total = eng.count_table(&collection);
            limit.map_or(total, |l| total.min(l))
        };
        let mut r = Row::new();
        r.insert("count".to_string(), Cell::Int(count as i64));
        r.insert("hits".to_string(), Cell::Int(count as i64));
        return Ok(vec![r]);
    }
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    Ok(eng.query_filter(&collection, combined_pred.as_ref(), limit, None, now_ms)?)
}

impl LinReader for LinBackend {
    fn disk_v2(&self) -> Option<&DiskEngineV2> {
        match self {
            LinBackend::DiskV2 { engine, .. } => Some(engine),
            _ => None,
        }
    }

    fn primary_db(&self) -> Option<&Db> {
        match self {
            LinBackend::Primary { db, .. } => Some(db),
            _ => None,
        }
    }

    fn run_query_rows(&self, query: Query) -> Result<Vec<Row>> {
        match self {
            LinBackend::DiskV2 { engine, .. } => run_disk_v2_query_rows(engine, query),
            LinBackend::Primary { db, .. } => {
                let prep = db.prepare_stmt(Stmt::Query(query))?;
                Ok(db.run_prepared_readonly(&prep)?.into_rows())
            }
            LinBackend::ReadOnlyDir { rdb } => Ok(rdb.run_stmt(Stmt::Query(query))?.into_rows()),
        }
    }
}

impl LinWriter for LinBackend {
    fn disk_v2_mut(&mut self) -> Option<&mut DiskEngineV2> {
        match self {
            LinBackend::DiskV2 {
                engine,
                read_only_flag: false,
                ..
            } => Some(engine),
            _ => None,
        }
    }

    fn bridge_embedder(&self) -> &Arc<BridgeEmbedder> {
        match self {
            LinBackend::DiskV2 { bridge, .. } | LinBackend::Primary { bridge, .. } => bridge,
            LinBackend::ReadOnlyDir { .. } => unreachable!("ReadOnlyDir cannot be mutated"),
        }
    }

    fn exec_stmt_mut(&mut self, stmt: Stmt) -> Result<usize> {
        match self {
            LinBackend::Primary {
                db,
                read_only_flag: false,
                ..
            } => {
                let handle = db.run_stmt(stmt)?;
                Ok(handle.done.n)
            }
            _ => Err(AppError::db("cannot mutate read-only or DiskV2 lin store via raw Stmt")),
        }
    }

    fn exec_stmt_params_mut(&mut self, stmt: Stmt, params: &[(&str, Cell)]) -> Result<usize> {
        match self {
            LinBackend::Primary {
                db,
                read_only_flag: false,
                ..
            } => {
                let prep = db.prepare_stmt(stmt)?;
                let done = prep.execute_with(db, params)?;
                Ok(done.n)
            }
            _ => Err(AppError::db("cannot mutate read-only or DiskV2 lin store via raw Stmt")),
        }
    }
}

impl<'a> LinReadGuard<'a> {
    pub(crate) fn new(guard: RwLockReadGuard<'a, LinBackend>) -> Self {
        Self { guard }
    }
}

impl LinReader for LinReadGuard<'_> {
    fn disk_v2(&self) -> Option<&DiskEngineV2> {
        self.guard.disk_v2()
    }

    fn primary_db(&self) -> Option<&Db> {
        self.guard.primary_db()
    }

    fn run_query_rows(&self, query: Query) -> Result<Vec<Row>> {
        self.guard.run_query_rows(query)
    }
}

impl<'a> LinWriteGuard<'a> {
    pub(crate) fn new(guard: RwLockWriteGuard<'a, LinBackend>) -> Result<Self> {
        match &*guard {
            LinBackend::DiskV2 {
                read_only_flag: false,
                ..
            }
            | LinBackend::Primary {
                read_only_flag: false,
                ..
            } => Ok(Self { guard }),
            _ => Err(AppError::db("cannot mutate read-only lin store")),
        }
    }

    pub fn db(&self) -> &Db {
        match &*self.guard {
            LinBackend::Primary { db, .. } => db,
            _ => panic!("LinWriteGuard::db() called on DiskV2 backend; use LinReader/LinWriter traits"),
        }
    }

    pub fn db_mut(&mut self) -> &mut Db {
        match &mut *self.guard {
            LinBackend::Primary { db, .. } => db,
            _ => panic!("LinWriteGuard::db_mut() called on DiskV2 backend; use LinWriter/export_backup"),
        }
    }

    pub fn checkpoint(&mut self) -> Result<()> {
        match &mut *self.guard {
            LinBackend::DiskV2 { engine, .. } => {
                engine.checkpoint()?;
                Ok(())
            }
            LinBackend::Primary { db, .. } => {
                db.checkpoint()?;
                Ok(())
            }
            LinBackend::ReadOnlyDir { .. } => unreachable!("verified writable in new()"),
        }
    }

    /// Run full out-of-core segment compaction (`DiskEngineV2::vacuum`) or `checkpoint()` on `:memory:`/v1.
    pub fn vacuum(&mut self) -> Result<lin::disk_v2::CompactionReport> {
        match &mut *self.guard {
            LinBackend::DiskV2 { engine, .. } => Ok(engine.vacuum()?),
            LinBackend::Primary { db, .. } => {
                db.checkpoint()?;
                Ok(lin::disk_v2::CompactionReport::default())
            }
            LinBackend::ReadOnlyDir { .. } => unreachable!("verified writable in new()"),
        }
    }

    /// Export a portable single-file `LIN\x04` backup directly to `dst` (streaming out-of-core on `DiskV2`!).
    pub fn export_backup(&mut self, dst: &Path) -> Result<()> {
        let clean = normalize_lin_path(dst);
        match &mut *self.guard {
            LinBackend::DiskV2 { engine, .. } => {
                engine.export_portable_backup_file(&clean)?;
                Ok(())
            }
            LinBackend::Primary { db, .. } => {
                let _ = db.checkpoint();
                db.export_backup(&clean)?;
                Ok(())
            }
            LinBackend::ReadOnlyDir { .. } => unreachable!("verified writable in new()"),
        }
    }

    /// Capture an MVCC-consistent snapshot of the current database memory image
    /// so backup serialization can proceed after releasing the `Store` lock.
    pub fn capture_portable_snapshot(&mut self) -> Result<lin::persist::Snapshot> {
        match &mut *self.guard {
            LinBackend::DiskV2 { engine, .. } => {
                let _ = engine.checkpoint();
                Ok(engine.to_legacy_snapshot()?)
            }
            LinBackend::Primary { db, .. } => {
                let _ = db.checkpoint();
                let hash = db.catalog.hash_str().to_string();
                Ok(db.store.capture_snapshot_at(hash, 0))
            }
            LinBackend::ReadOnlyDir { .. } => unreachable!("verified writable in new()"),
        }
    }

    /// Materialize the current in-memory (or durable) `lin` image directly into a
    /// clean durable `DiskEngineV2` `.lin` directory!
    pub fn materialize_durable_dir(&mut self, dir: &Path) -> Result<()> {
        let clean_dir = normalize_lin_path(dir);
        let snap = self.capture_portable_snapshot()?;
        let _ = DiskEngineV2::from_legacy_snapshot(
            snap,
            &clean_dir,
            65_536,
            &rag_default_indexed_fields(),
        )?;
        Ok(())
    }

    /// Prime a newly opened `lin::Transaction` with `Undo::Full` (`MemBackup`) so mixed
    /// `Delete` + `Insert` operations on the same collection (e.g. atomic chunk/edge
    /// replacement or re-embedding) roll back bit-for-bit cleanly without hitting
    /// `Undo::Mixed` index/length interactions.
    fn open_full_undo_tx(db: &mut Db) -> Result<lin::Transaction<'_>> {
        const SENTINEL_SRC: &str = concat!(
            "update meta [id == \"__lin_tx_full_undo_sentinel__\"] cas each { value: \"\" }\n",
            "insert meta { id: \"__lin_tx_full_undo_sentinel__\", key: \"\", value: \"\" }"
        );
        let prep = db.prepare(SENTINEL_SRC)?;
        let mut tx = db.transaction()?;
        let _ = tx.execute_prepared(&prep);
        Ok(tx)
    }

    /// Begin an explicit ACID transaction (auto-rolls back on `Drop` unless `commit()` is called).
    pub fn begin_tx(&mut self) -> Result<LinTx<'_>> {
        match &mut *self.guard {
            LinBackend::DiskV2 {
                engine,
                bridge,
                read_only_flag: false,
            } => {
                engine.begin_savepoint()?;
                Ok(LinTx {
                    inner: LinTxInner::DiskV2 {
                        engine,
                        committed_or_rolled_back: false,
                    },
                    bridge: bridge.clone(),
                })
            }
            LinBackend::Primary {
                db,
                bridge,
                read_only_flag: false,
            } => {
                let tx = Self::open_full_undo_tx(db)?;
                Ok(LinTx {
                    inner: LinTxInner::Legacy(tx),
                    bridge: bridge.clone(),
                })
            }
            _ => Err(AppError::db("cannot open transaction on read-only lin store")),
        }
    }

    pub fn with_tx<T>(&mut self, work: impl FnOnce(&mut LinTx<'_>) -> Result<T>) -> Result<T> {
        let mut lin_tx = self.begin_tx()?;
        let bridge = Arc::clone(&lin_tx.bridge);
        match work(&mut lin_tx) {
            Ok(val) => {
                lin_tx.commit()?;
                Ok(val)
            }
            Err(err) => {
                bridge.clear_staged();
                let _ = lin_tx.rollback();
                Err(err)
            }
        }
    }
}

impl LinReader for LinWriteGuard<'_> {
    fn disk_v2(&self) -> Option<&DiskEngineV2> {
        self.guard.disk_v2()
    }

    fn primary_db(&self) -> Option<&Db> {
        self.guard.primary_db()
    }

    fn run_query_rows(&self, query: Query) -> Result<Vec<Row>> {
        self.guard.run_query_rows(query)
    }
}

impl LinWriter for LinWriteGuard<'_> {
    fn disk_v2_mut(&mut self) -> Option<&mut DiskEngineV2> {
        self.guard.disk_v2_mut()
    }

    fn bridge_embedder(&self) -> &Arc<BridgeEmbedder> {
        self.guard.bridge_embedder()
    }

    fn exec_stmt_mut(&mut self, stmt: Stmt) -> Result<usize> {
        self.guard.exec_stmt_mut(stmt)
    }

    fn exec_stmt_params_mut(&mut self, stmt: Stmt, params: &[(&str, Cell)]) -> Result<usize> {
        self.guard.exec_stmt_params_mut(stmt, params)
    }
}

impl LinTx<'_> {
    pub fn commit(mut self) -> Result<()> {
        match std::mem::replace(&mut self.inner, LinTxInner::Finished) {
            LinTxInner::DiskV2 { engine, .. } => {
                engine.commit_savepoint()?;
                Ok(())
            }
            LinTxInner::Legacy(tx) => {
                tx.commit()?;
                Ok(())
            }
            LinTxInner::Finished => Ok(()),
        }
    }

    pub fn rollback(mut self) -> Result<()> {
        self.bridge.clear_staged();
        match std::mem::replace(&mut self.inner, LinTxInner::Finished) {
            LinTxInner::DiskV2 { engine, .. } => {
                engine.rollback_savepoint();
                Ok(())
            }
            LinTxInner::Legacy(tx) => {
                tx.rollback()?;
                Ok(())
            }
            LinTxInner::Finished => Ok(()),
        }
    }
}

impl LinReader for LinTx<'_> {
    fn disk_v2(&self) -> Option<&DiskEngineV2> {
        match &self.inner {
            LinTxInner::DiskV2 { engine, .. } => Some(&**engine),
            LinTxInner::Legacy(_) | LinTxInner::Finished => None,
        }
    }

    fn primary_db(&self) -> Option<&Db> {
        match &self.inner {
            LinTxInner::DiskV2 { .. } | LinTxInner::Finished => None,
            LinTxInner::Legacy(tx) => Some(tx.db()),
        }
    }

    fn run_query_rows(&self, query: Query) -> Result<Vec<Row>> {
        match &self.inner {
            LinTxInner::DiskV2 { engine, .. } => run_disk_v2_query_rows(engine, query),
            LinTxInner::Legacy(tx) => {
                let prep = tx.db().prepare_stmt(Stmt::Query(query))?;
                Ok(tx.db().run_prepared_readonly(&prep)?.into_rows())
            }
            LinTxInner::Finished => Err(AppError::db("transaction already finished")),
        }
    }
}

impl LinWriter for LinTx<'_> {
    fn disk_v2_mut(&mut self) -> Option<&mut DiskEngineV2> {
        match &mut self.inner {
            LinTxInner::DiskV2 { engine, .. } => Some(&mut **engine),
            LinTxInner::Legacy(_) | LinTxInner::Finished => None,
        }
    }

    fn bridge_embedder(&self) -> &Arc<BridgeEmbedder> {
        &self.bridge
    }

    fn exec_stmt_mut(&mut self, stmt: Stmt) -> Result<usize> {
        match &mut self.inner {
            LinTxInner::Legacy(tx) => {
                let prep = tx.db().prepare_stmt(stmt)?;
                let done = tx.execute_prepared(&prep)?;
                Ok(done.n)
            }
            LinTxInner::DiskV2 { .. } | LinTxInner::Finished => Err(AppError::db(
                "cannot execute raw Stmt on DiskEngineV2 transaction",
            )),
        }
    }

    fn exec_stmt_params_mut(&mut self, stmt: Stmt, params: &[(&str, Cell)]) -> Result<usize> {
        match &mut self.inner {
            LinTxInner::Legacy(tx) => {
                let prep = tx.db().prepare_stmt(stmt)?;
                let done = tx.execute_with(&prep, params)?;
                Ok(done.n)
            }
            LinTxInner::DiskV2 { .. } | LinTxInner::Finished => Err(AppError::db(
                "cannot execute raw Stmt on DiskEngineV2 transaction",
            )),
        }
    }
}

/// Acquire a shared read lock on `Arc<RwLock<LinBackend>>`.
pub fn read_lock(backend: &RwLock<LinBackend>) -> Result<LinReadGuard<'_>> {
    let guard = backend
        .read()
        .map_err(|e| AppError::db(format!("lin rwlock read poisoned: {e}")))?;
    Ok(LinReadGuard::new(guard))
}

/// Acquire an exclusive write lock on `Arc<RwLock<LinBackend>>`.
pub fn write_lock(backend: &RwLock<LinBackend>) -> Result<LinWriteGuard<'_>> {
    let guard = backend
        .write()
        .map_err(|e| AppError::db(format!("lin rwlock write poisoned: {e}")))?;
    LinWriteGuard::new(guard)
}

// ============================================================================
// Typed `Value` / `Cell` / `Row` Conversion & Formatting Helpers
// ============================================================================

fn null_param_val() -> Value {
    static NULL_ARC: std::sync::LazyLock<Arc<str>> =
        std::sync::LazyLock::new(|| Arc::from("__null"));
    Value::Param(Arc::clone(&NULL_ARC))
}

#[inline]
fn is_null_param(v: &Value) -> bool {
    matches!(v, Value::Param(p) if p.as_ref() == "__null")
}

pub fn val_str(s: impl AsRef<str>) -> Value {
    Value::String(Arc::from(s.as_ref()))
}

pub fn val_opt_str(s: Option<&str>) -> Value {
    match s {
        Some(v) => Value::String(Arc::from(v)),
        None => null_param_val(),
    }
}

pub fn val_opt_string(s: &Option<String>) -> Value {
    val_opt_str(s.as_deref())
}

pub fn val_i64(n: i64) -> Value {
    Value::Int(n)
}

pub fn val_opt_i64(n: Option<i64>) -> Value {
    match n {
        Some(v) => Value::Int(v),
        None => null_param_val(),
    }
}

pub fn val_f64(f: f64) -> Value {
    if f.is_finite() {
        Value::Float(f)
    } else {
        Value::Float(0.0)
    }
}

pub fn val_opt_f64(f: Option<f64>) -> Value {
    match f {
        Some(v) if v.is_finite() => Value::Float(v),
        Some(_) => Value::Float(0.0),
        None => null_param_val(),
    }
}

pub fn val_bool(b: bool) -> Value {
    Value::Bool(b)
}

pub fn val_opt_bool(b: Option<bool>) -> Value {
    match b {
        Some(v) => Value::Bool(v),
        None => null_param_val(),
    }
}

pub fn val_ts(dt: &DateTime<Utc>) -> Value {
    Value::String(Arc::from(dt.to_rfc3339()))
}

pub fn val_opt_ts(dt: &Option<DateTime<Utc>>) -> Value {
    match dt {
        Some(d) => Value::String(Arc::from(d.to_rfc3339())),
        None => null_param_val(),
    }
}

pub fn cell_str<'a>(row: &'a Row, field: &str) -> Option<&'a str> {
    match row.get(field)? {
        Cell::Text(s) => Some(s.as_ref()),
        Cell::SmallText(s) => Some(s.as_str()),
        _ => None,
    }
}

pub fn req_str<'a>(row: &'a Row, field: &str) -> Result<&'a str> {
    cell_str(row, field).ok_or_else(|| AppError::db(format!("missing required text field '{field}'")))
}

pub fn opt_str<'a>(row: &'a Row, field: &str) -> Option<&'a str> {
    cell_str(row, field)
}

pub fn cell_string(row: &Row, field: &str) -> String {
    cell_str(row, field).unwrap_or("").to_string()
}

pub fn cell_opt_string(row: &Row, field: &str) -> Option<String> {
    cell_str(row, field).map(str::to_string)
}

pub fn cell_nonempty_string(row: &Row, field: &str) -> Option<String> {
    cell_str(row, field)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn cell_i64_opt(row: &Row, field: &str) -> Option<i64> {
    match row.get(field)? {
        Cell::Int(n) => Some(*n),
        Cell::Time(t) => Some(*t),
        Cell::Float(f) => Some(*f as i64),
        Cell::Text(s) => s.parse().ok(),
        Cell::SmallText(s) => s.as_str().parse().ok(),
        _ => None,
    }
}

pub fn opt_i64(row: &Row, field: &str) -> Option<i64> {
    cell_i64_opt(row, field)
}

pub fn cell_i64(row: &Row, field: &str, default: i64) -> i64 {
    cell_i64_opt(row, field).unwrap_or(default)
}

pub fn cell_i32(row: &Row, field: &str, default: i32) -> i32 {
    cell_i64_opt(row, field)
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(default)
}

pub fn cell_f64_opt(row: &Row, field: &str) -> Option<f64> {
    match row.get(field)? {
        Cell::Float(f) => Some(*f),
        Cell::Int(n) => Some(*n as f64),
        Cell::Text(s) => s.parse().ok(),
        Cell::SmallText(s) => s.as_str().parse().ok(),
        _ => None,
    }
}

pub fn cell_f64(row: &Row, field: &str, default: f64) -> f64 {
    cell_f64_opt(row, field).unwrap_or(default)
}

pub fn cell_bool_opt(row: &Row, field: &str) -> Option<bool> {
    match row.get(field)? {
        Cell::Bool(b) => Some(*b),
        Cell::Int(n) => Some(*n != 0),
        _ => None,
    }
}

pub fn cell_bool(row: &Row, field: &str, default: bool) -> bool {
    cell_bool_opt(row, field).unwrap_or(default)
}

pub fn cell_ts(row: &Row, field: &str) -> Result<DateTime<Utc>> {
    match row.get(field) {
        Some(Cell::Text(s)) => crate::util::parse_db_timestamp(s)
            .ok_or_else(|| AppError::db(format!("invalid timestamp '{s}' in '{field}'"))),
        Some(Cell::SmallText(s)) => crate::util::parse_db_timestamp(s.as_str())
            .ok_or_else(|| AppError::db(format!("invalid timestamp '{}' in '{field}'", s.as_str()))),
        Some(Cell::Time(ms)) | Some(Cell::Int(ms)) => DateTime::from_timestamp_millis(*ms)
            .ok_or_else(|| AppError::db(format!("invalid epoch millis {ms} in {field}"))),
        _ => Err(AppError::db(format!("missing timestamp field '{field}'"))),
    }
}

pub fn cell_ts_or_now(row: &Row, field: &str) -> DateTime<Utc> {
    cell_ts(row, field).unwrap_or_else(|_| Utc::now())
}

pub fn cell_opt_ts(row: &Row, field: &str) -> Result<Option<DateTime<Utc>>> {
    match row.get(field) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::Text(s)) if s.trim().is_empty() => Ok(None),
        Some(Cell::SmallText(s)) if s.as_str().trim().is_empty() => Ok(None),
        _ => cell_ts(row, field).map(Some),
    }
}

pub fn cell_vec_f32(row: &Row, field: &str) -> Vec<f32> {
    if let Some(Cell::Vec(v)) = row.get(field) {
        return v.as_ref().to_vec();
    }
    if let Some(json_str) = cell_str(row, "embedding_json") {
        if !json_str.is_empty() {
            if let Ok(parsed) = serde_json::from_str::<Vec<f32>>(json_str) {
                return parsed;
            }
        }
    }
    Vec::new()
}

pub fn cell_arc_vec(row: &Row, field: &str) -> Option<Arc<[f32]>> {
    if let Some(Cell::Vec(v)) = row.get(field) {
        return Some(Arc::clone(v));
    }
    if let Some(json_str) = cell_str(row, "embedding_json") {
        if !json_str.is_empty() {
            if let Ok(parsed) = serde_json::from_str::<Vec<f32>>(json_str) {
                return Some(Arc::from(parsed));
            }
        }
    }
    None
}

/// Build a composite primary key from two parts using unit-separator `\x1f`.
pub fn composite_id_2(a: &str, b: &str) -> String {
    format!("{a}\x1f{b}")
}

/// Build a composite primary key from three parts using unit-separator `\x1f`.
pub fn composite_id_3(a: &str, b: &str, c: &str) -> String {
    format!("{a}\x1f{b}\x1f{c}")
}

fn estimate_record_bytes(fields: &[(String, Value)]) -> usize {
    let mut bytes = 64usize;
    for (k, v) in fields {
        bytes += k.len() + 16;
        if let Value::String(s) = v {
            bytes += s.len();
        }
    }
    bytes
}

fn write_lin_string_literal(out: &mut String, s: &str) {
    // Note: `lin`'s lexer (`lin/src/parse.rs:355-359`) only decodes `\n`, `\t`, `\\`, `\"`,
    // and passes `\r` through as literal `'r'`! Write raw `\r` bytes directly inside `"..."`
    // so CRLF source files preserve exact byte-for-byte line endings.
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_lin_value(out: &mut String, val: &Value) {
    match val {
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => {
            if !f.is_finite() {
                out.push_str("0.0");
            } else if f.fract() == 0.0 {
                out.push_str(&format!("{f:.1}"));
            } else {
                let s = f.to_string();
                if s.contains('.') {
                    out.push_str(&s);
                } else {
                    out.push_str(&format!("{f:.1}"));
                }
            }
        }
        Value::String(s) => write_lin_string_literal(out, s),
        Value::Param(p) => {
            out.push('$');
            out.push_str(p);
        }
        other => out.push_str(&other.to_string()),
    }
}

fn write_lin_record(out: &mut String, fields: &[(String, Value)]) {
    out.push_str("{ ");
    for (i, (k, v)) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(k);
        out.push_str(": ");
        write_lin_value(out, v);
    }
    out.push_str(" }");
}

fn write_lin_pred(out: &mut String, pred: &Pred) {
    match pred {
        Pred::And(a, b) => {
            out.push('(');
            write_lin_pred(out, a);
            out.push_str(" and ");
            write_lin_pred(out, b);
            out.push(')');
        }
        Pred::Or(a, b) => {
            out.push('(');
            write_lin_pred(out, a);
            out.push_str(" or ");
            write_lin_pred(out, b);
            out.push(')');
        }
        Pred::Not(inner) => {
            out.push_str("not (");
            write_lin_pred(out, inner);
            out.push(')');
        }
        Pred::Cmp { field, op, value } => {
            out.push_str(&field.to_string());
            out.push(' ');
            out.push_str(op.as_str());
            out.push(' ');
            write_lin_value(out, value);
        }
        Pred::IsNull { field, negated } => {
            out.push_str(&field.to_string());
            if *negated {
                out.push_str(" is not null");
            } else {
                out.push_str(" is null");
            }
        }
        Pred::In { field, values } => {
            out.push_str(&field.to_string());
            out.push_str(" in [");
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_lin_value(out, v);
            }
            out.push(']');
        }
        Pred::Has { field, ci, needle } => {
            out.push_str(&field.to_string());
            if *ci {
                out.push_str(" has ci ");
            } else {
                out.push_str(" has ");
            }
            write_lin_string_literal(out, needle);
        }
        Pred::Contains { field, needle } => {
            out.push_str(&field.to_string());
            out.push_str(" ~ ");
            write_lin_string_literal(out, needle);
        }
        Pred::Regex {
            field,
            pattern,
            flags,
        } => {
            out.push_str(&field.to_string());
            out.push_str(" ~ /");
            out.push_str(&pattern.replace('/', "\\/"));
            out.push('/');
            out.push_str(flags);
        }
    }
}

fn format_insert_src(collection: &str, records: &[Vec<(String, Value)>]) -> String {
    let approx: usize = records.iter().map(|r| estimate_record_bytes(r)).sum();
    let mut out = String::with_capacity(approx + 64);
    out.push_str("insert ");
    out.push_str(collection);
    if records.len() == 1 {
        out.push(' ');
        write_lin_record(&mut out, &records[0]);
    } else {
        out.push_str(" [");
        for (i, rec) in records.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_lin_record(&mut out, rec);
        }
        out.push(']');
    }
    out
}

fn format_update_src(collection: &str, pred: &Pred, fields: &[(String, Value)]) -> String {
    let mut out = String::with_capacity(estimate_record_bytes(fields) + 128);
    out.push_str("update ");
    out.push_str(collection);
    out.push_str(" [");
    write_lin_pred(&mut out, pred);
    out.push_str("] cas each ");
    write_lin_record(&mut out, fields);
    out
}

/// Convenient row-record builder for `LinWriter::insert_one` / `upsert_by_id` / `update_where`.
#[derive(Default)]
pub struct RecBuilder {
    fields: Vec<(String, Value)>,
}

impl RecBuilder {
    pub fn new() -> Self {
        Self {
            fields: Vec::with_capacity(16),
        }
    }

    pub fn id(self, v: impl AsRef<str>) -> Self {
        self.str("id", v)
    }

    pub fn str(mut self, k: &str, v: impl AsRef<str>) -> Self {
        self.fields
            .push((k.to_string(), Value::String(Arc::from(v.as_ref()))));
        self
    }

    pub fn opt_str(mut self, k: &str, v: Option<&str>) -> Self {
        self.fields.push((k.to_string(), val_opt_str(v)));
        self
    }

    pub fn opt_string(self, k: &str, v: impl OptStrArg) -> Self {
        self.opt_str(k, v.as_opt_str())
    }

    pub fn i64(mut self, k: &str, v: i64) -> Self {
        self.fields.push((k.to_string(), Value::Int(v)));
        self
    }

    pub fn opt_i64(mut self, k: &str, v: Option<i64>) -> Self {
        self.fields.push((k.to_string(), val_opt_i64(v)));
        self
    }

    pub fn f64(mut self, k: &str, v: f64) -> Self {
        self.fields.push((k.to_string(), val_f64(v)));
        self
    }

    pub fn opt_f64(mut self, k: &str, v: Option<f64>) -> Self {
        self.fields.push((k.to_string(), val_opt_f64(v)));
        self
    }

    pub fn bool(mut self, k: &str, v: bool) -> Self {
        self.fields.push((k.to_string(), Value::Bool(v)));
        self
    }

    pub fn opt_bool(mut self, k: &str, v: Option<bool>) -> Self {
        self.fields.push((k.to_string(), val_opt_bool(v)));
        self
    }

    pub fn ts(mut self, k: &str, dt: &DateTime<Utc>) -> Self {
        self.fields.push((k.to_string(), val_ts(dt)));
        self
    }

    pub fn opt_ts(mut self, k: &str, dt: &Option<DateTime<Utc>>) -> Self {
        self.fields.push((k.to_string(), val_opt_ts(dt)));
        self
    }

    pub fn build(self) -> Vec<(String, Value)> {
        self.fields
    }
}
