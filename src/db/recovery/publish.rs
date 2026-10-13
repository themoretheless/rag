//! Filesystem publication: staging, fsync, no-clobber publish, verification and inventory.

use super::*;

/// Atomically publish one recovery artifact after fully staging and syncing it.
///
/// With `overwrite=false`, a destination that appears after preflight is left
/// untouched and reported as a conflict.
pub fn publish_recovery_artifact(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<bool> {
    publish_recovery_artifact_with_hook(destination, bytes, overwrite, |_| Ok(()))
}

pub(super) fn publish_recovery_artifact_with_hook<F>(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
    before_publish: F,
) -> Result<bool>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "recovery artifact parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let existed = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if existed && !overwrite {
        return Err(AppError::conflict(format!(
            "recovery artifact destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }

    let temporary = stage_bytes(destination, bytes)?;
    let result = (|| {
        before_publish(destination)?;
        publish_temporary_artifact(&temporary, destination, overwrite)?;
        if !overwrite {
            fs::remove_file(&temporary)?;
        }
        sync_parent_directory(destination)?;
        Ok(existed)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn verify_backup(path: &Path) -> Result<BackupVerification> {
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "backup file '{}'",
            path.display()
        )));
    }
    let bytes = fs::metadata(path)?.len();
    let sha256 = sha256_file(path)?;
    let verification = (|| -> Result<BackupVerification> {
        // `Store::open` runs migrations. Perform those checks on a disposable
        // byte-for-byte copy so verification can never upgrade or otherwise
        // mutate the artifact it was asked to inspect.
        let (temporary_path, copied_bytes) = stage_file_copy(path, path)?;
        let temporary = TemporaryArtifact::new(temporary_path);
        if copied_bytes != bytes || sha256_file(temporary.path())? != sha256 {
            return Err(AppError::db(format!(
                "backup '{}' changed while its verification copy was being made",
                path.display()
            )));
        }
        let store = Store::open(temporary.path())?;
        let schema_version = store.schema_version()?.unwrap_or(0);
        let (documents, chunks, nodes, edges) = store.stats()?;
        let (_, orphan_chunks, orphan_document_nodes, orphan_edges, _) =
            store.integrity_counts()?;
        let embedding_manifest = store.get_embedding_manifest()?;
        let embedding_contract_ok =
            verify_embedding_contract(&store, chunks, embedding_manifest.as_ref())?;
        drop(store);
        let ok = orphan_chunks == 0
            && orphan_document_nodes == 0
            && orphan_edges == 0
            && embedding_contract_ok;
        Ok(BackupVerification {
            ok,
            path: path.display().to_string(),
            bytes,
            sha256: sha256.clone(),
            schema_version,
            documents,
            chunks,
            nodes,
            edges,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            embedding_contract_ok,
            embedding_manifest,
        })
    })();

    let bytes_after = fs::metadata(path)?.len();
    let sha256_after = sha256_file(path)?;
    if bytes_after != bytes || sha256_after != sha256 {
        return Err(AppError::conflict(format!(
            "backup '{}' changed during verification",
            path.display()
        )));
    }
    verification
}

pub(super) fn verify_embedding_contract(
    store: &Store,
    chunk_count: u64,
    manifest: Option<&EmbeddingManifest>,
) -> Result<bool> {
    let Some(manifest) = manifest else {
        return Ok(chunk_count == 0);
    };
    if validate_embedding_manifest(manifest, "backup").is_err() {
        return Ok(false);
    }
    if chunk_count == 0 {
        return Ok(true);
    }
    let expected_dims = match usize::try_from(manifest.dims) {
        Ok(dims) if dims > 0 => dims,
        _ => return Ok(false),
    };
    let conn = store.lock()?;
    let mut statement = conn.prepare("SELECT embedding_json FROM chunks ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut consistent = true;
    crate::db::rows::try_visit_rows(&mut rows, |row| {
        let raw: String = row.get(0)?;
        consistent = serde_json::from_str::<Vec<f32>>(&raw)
            .is_ok_and(|embedding| embedding.len() == expected_dims);
        Ok(if consistent {
            std::ops::ControlFlow::Continue(())
        } else {
            std::ops::ControlFlow::Break(())
        })
    })?;
    Ok(consistent)
}

pub fn backup_inventory(dir: &Path) -> Result<Vec<BackupInventoryItem>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("duckdb") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let modified_at: DateTime<Utc> = modified.into();
        let protected = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.contains("final"));
        rows.push(BackupInventoryItem {
            path: path.display().to_string(),
            bytes: metadata.len(),
            modified_at,
            protected,
            newest: false,
        });
    }
    rows.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    if let Some(first) = rows.first_mut() {
        first.newest = true;
        first.protected = true;
    }
    Ok(rows)
}

pub fn retention_preview(dir: &Path, keep: usize) -> Result<Vec<BackupInventoryItem>> {
    Ok(backup_inventory(dir)?
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| (index >= keep.max(1) && !item.protected).then_some(item))
        .collect())
}

pub(super) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub(super) fn backup_publication_lock(path: &Path) -> Arc<StdMutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Weak<StdMutex<()>>>>> = OnceLock::new();

    let key = normalized_destination_path(path);
    let mut locks = LOCKS
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(StdMutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub(super) fn normalized_destination_path(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match (parent.canonicalize(), path.file_name()) {
        (Ok(parent), Some(file_name)) => parent.join(file_name),
        _ => path.to_path_buf(),
    }
}

pub(super) fn backup_verifications_match(
    left: &BackupVerification,
    right: &BackupVerification,
) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}

pub(super) fn verify_published_backup_group(
    path: &Path,
    source: &Path,
) -> Result<BackupVerification> {
    let verification = verify_backup(path)?;
    let artifacts = backup_artifact_paths(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("backup.duckdb");
    let expected_checksum = format!("{}  {}\n", verification.sha256, file_name);
    let checksum = fs::read(&artifacts[SHA256_ARTIFACT])?;
    if checksum != expected_checksum.as_bytes() {
        return Err(AppError::db(format!(
            "published checksum sidecar '{}' does not describe database generation '{}'",
            artifacts[SHA256_ARTIFACT].display(),
            path.display()
        )));
    }

    let metadata: BackupSidecar = serde_json::from_slice(&fs::read(&artifacts[METADATA_ARTIFACT])?)
        .map_err(|error| {
            AppError::db(format!(
                "invalid backup metadata sidecar '{}': {error}",
                artifacts[METADATA_ARTIFACT].display()
            ))
        })?;
    if metadata.format != "rag-duckdb-backup"
        || metadata.source != source.display().to_string()
        || metadata.required_free_bytes != verification.bytes.saturating_mul(2)
        || !backup_verifications_match(&metadata.verification, &verification)?
    {
        return Err(AppError::db(format!(
            "published metadata sidecar '{}' does not describe database generation '{}'",
            artifacts[METADATA_ARTIFACT].display(),
            path.display()
        )));
    }
    Ok(verification)
}

pub(super) struct BackupSidecarBytes {
    pub(super) sha256: Vec<u8>,
    pub(super) metadata: Vec<u8>,
}

pub(super) fn backup_sidecar_bytes(
    path: &Path,
    source: &Path,
    verification: &BackupVerification,
) -> Result<BackupSidecarBytes> {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("backup.duckdb");
    let sidecar = BackupSidecar {
        format: "rag-duckdb-backup".into(),
        created_at: Utc::now(),
        source: source.display().to_string(),
        required_free_bytes: verification.bytes.saturating_mul(2),
        verification: verification.clone(),
    };
    Ok(BackupSidecarBytes {
        sha256: format!("{}  {}\n", verification.sha256, name).into_bytes(),
        metadata: serde_json::to_vec_pretty(&sidecar)?,
    })
}

/// Owns the staged and published members of one three-file backup group.
///
/// A normal Rust error unwinds through `Drop`, which rolls back artifacts
/// published by this attempt and restores overwrite targets from hard-link
/// snapshots (subject, unavoidably, to the filesystem accepting cleanup I/O).
/// Filesystems do not provide a transaction spanning three names, though: a
/// process/power loss between renames can still leave sidecars without a new
/// database, hidden staging/rollback links, or (when overwriting) a mixed
/// generation. Publishing the database last makes a *new* database name the
/// commit marker; callers needing the strongest crash semantics should use a
/// fresh destination (`overwrite=false`) and ignore orphan sidecars.
pub(super) struct BackupGroupPublication {
    pub(super) artifacts: [PathBuf; 3],
    pub(super) staged: [Option<PathBuf>; 3],
    pub(super) overwritten: [Option<PathBuf>; 3],
    pub(super) published: [bool; 3],
    pub(super) overwrite: bool,
    pub(super) committed: bool,
}

impl BackupGroupPublication {
    pub(super) fn new(artifacts: [PathBuf; 3], overwrite: bool) -> Self {
        Self {
            artifacts,
            staged: std::array::from_fn(|_| None),
            overwritten: std::array::from_fn(|_| None),
            published: [false; 3],
            overwrite,
            committed: false,
        }
    }

    pub(super) fn set_staged(&mut self, index: usize, path: PathBuf) {
        self.staged[index] = Some(path);
    }

    pub(super) fn staged_path(&self, index: usize) -> Result<&Path> {
        self.staged[index]
            .as_deref()
            .ok_or_else(|| AppError::db("backup artifact was not staged"))
    }

    pub(super) fn preserve_overwritten_artifacts(&mut self) -> Result<()> {
        if !self.overwrite {
            return Ok(());
        }
        for index in 0..self.artifacts.len() {
            match fs::symlink_metadata(&self.artifacts[index]) {
                Ok(_) => {
                    self.overwritten[index] = Some(create_rollback_link(&self.artifacts[index])?);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(super) fn publish(&mut self, index: usize) -> Result<()> {
        let staged = self.staged_path(index)?.to_path_buf();
        publish_temporary_artifact(&staged, &self.artifacts[index], self.overwrite)?;
        self.published[index] = true;
        if !self.overwrite {
            // `hard_link` publishes the final name without consuming staging.
            // Mark ownership first so an unlink error rolls back both names.
            fs::remove_file(&staged)?;
        }
        self.staged[index] = None;
        Ok(())
    }

    pub(super) fn commit(mut self) {
        self.committed = true;
        for path in self.overwritten.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(path);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }

    pub(super) fn rollback(&mut self) {
        for index in (0..self.artifacts.len()).rev() {
            if self.published[index] {
                let _ = fs::remove_file(&self.artifacts[index]);
                self.published[index] = false;
                if let Some(original) = self.overwritten[index].take() {
                    if fs::rename(&original, &self.artifacts[index]).is_err() {
                        // Keep the hard-link snapshot for manual recovery rather
                        // than deleting the last known copy of the old artifact.
                        self.overwritten[index] = Some(original);
                    }
                }
            } else if let Some(original) = self.overwritten[index].take() {
                let _ = fs::remove_file(original);
            }
        }
        for staged in self.staged.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(staged);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }
}

impl Drop for BackupGroupPublication {
    fn drop(&mut self) {
        if !self.committed {
            self.rollback();
        }
    }
}

pub(super) struct TemporaryArtifact {
    pub(super) path: PathBuf,
}

impl TemporaryArtifact {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let mut wal_name: OsString = self.path.as_os_str().to_owned();
        wal_name.push(".wal");
        let _ = fs::remove_file(PathBuf::from(wal_name));
    }
}

pub(super) fn preflight_backup_target(
    source: &Path,
    destination: &Path,
    overwrite: bool,
    label: &str,
) -> Result<bool> {
    refuse_same_file(source, destination)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "{label} parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::forbidden(format!(
            "{label} destination '{}' must not be a symbolic link",
            destination.display()
        )));
    }
    let exists = metadata.is_some();
    if exists && !overwrite {
        return Err(AppError::conflict(format!(
            "{label} destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }
    Ok(exists)
}

pub(super) fn stage_file_copy(source: &Path, destination: &Path) -> Result<(PathBuf, u64)> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<(PathBuf, u64)> {
        let mut input = fs::File::open(source)?;
        let bytes = std::io::copy(&mut input, &mut temporary)?;
        if let Ok(metadata) = fs::metadata(source) {
            fs::set_permissions(&temporary_path, metadata.permissions())?;
        }
        temporary.sync_all()?;
        drop(temporary);
        Ok((temporary_path.clone(), bytes))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(super) fn quote_duckdb_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

pub(super) fn quote_duckdb_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Create a transactionally consistent DuckDB snapshot without retaining the
/// process-wide Store mutex during the potentially long copy.
pub(super) fn stage_database_snapshot<F>(
    snapshot_conn: duckdb::Connection,
    source: &Path,
    destination: &Path,
    mut copy_hook: F,
) -> Result<(PathBuf, u64)>
where
    F: FnMut() -> Result<()>,
{
    static NEXT_CATALOG_ID: AtomicU64 = AtomicU64::new(0);

    let temporary_path = create_temporary_database_artifact(destination)?;
    let result = (|| -> Result<(PathBuf, u64)> {
        let temporary_utf8 = temporary_path.to_str().ok_or_else(|| {
            AppError::config(format!(
                "DuckDB snapshot staging path '{}' must be valid UTF-8",
                temporary_path.display()
            ))
        })?;
        let source_catalog: String =
            snapshot_conn.query_row("SELECT current_database()", [], |row| row.get(0))?;
        let catalog = format!(
            "rag_backup_snapshot_{}_{}",
            std::process::id(),
            NEXT_CATALOG_ID.fetch_add(1, Ordering::Relaxed)
        );
        let attach = format!(
            "ATTACH {} AS {}",
            quote_duckdb_string(temporary_utf8),
            quote_duckdb_identifier(&catalog)
        );
        snapshot_conn.execute_batch(&attach)?;
        snapshot_conn.execute_batch("BEGIN TRANSACTION")?;
        let source_catalog = quote_duckdb_identifier(&source_catalog);
        let catalog = quote_duckdb_identifier(&catalog);
        let copy = (|| -> Result<()> {
            // Pin the source snapshot before releasing the hook. COPY SCHEMA
            // and COPY DATA then observe one MVCC-consistent generation.
            snapshot_conn.query_row(
                &format!("SELECT COUNT(*) FROM {source_catalog}.main.schema_version"),
                [],
                |_row| Ok(()),
            )?;
            copy_hook()?;
            snapshot_conn
                .execute_batch(&format!("COPY FROM DATABASE {source_catalog} TO {catalog}"))?;
            Ok(())
        })();
        let transaction_end =
            snapshot_conn.execute_batch(if copy.is_ok() { "COMMIT" } else { "ROLLBACK" });
        let detach = snapshot_conn.execute_batch(&format!("DETACH {catalog}"));
        copy?;
        transaction_end?;
        detach?;
        sync_file(&temporary_path)?;
        if let Ok(metadata) = fs::metadata(source) {
            fs::set_permissions(&temporary_path, metadata.permissions())?;
        }
        let bytes = fs::metadata(&temporary_path)?.len();
        Ok((temporary_path.clone(), bytes))
    })();
    drop(snapshot_conn);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        let mut wal = temporary_path.as_os_str().to_os_string();
        wal.push(".wal");
        let _ = fs::remove_file(PathBuf::from(wal));
    }
    result
}

pub(super) fn stage_bytes(destination: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<PathBuf> {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        Ok(temporary_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(super) fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

pub(super) fn create_rollback_link(destination: &Path) -> Result<PathBuf> {
    static NEXT_ROLLBACK_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_ROLLBACK_ID.fetch_add(1, Ordering::Relaxed);
        let mut rollback_name = file_name.to_os_string();
        rollback_name.push(format!(".rag-backup-rollback-{}-{id}", std::process::id()));
        let rollback_path = parent.join(rollback_name);
        match fs::hard_link(destination, &rollback_path) {
            Ok(()) => return Ok(rollback_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not preserve overwritten backup artifact '{}'",
        destination.display()
    )))
}

#[cfg(unix)]
pub(super) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

pub(super) fn create_temporary_artifact(destination: &Path) -> Result<(PathBuf, fs::File)> {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".rag-backup-tmp-{}-{id}", std::process::id()));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not allocate a temporary backup artifact beside '{}'",
        destination.display()
    )))
}

pub(super) fn create_temporary_database_artifact(destination: &Path) -> Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(
            ".rag-backup-tmp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let temporary_path = parent.join(temporary_name);
        let reservation = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        drop(reservation);
        fs::remove_file(&temporary_path)?;
        match duckdb::Connection::open(&temporary_path) {
            Ok(conn) => {
                let initialized = conn.execute_batch("CHECKPOINT");
                drop(conn);
                match initialized {
                    Ok(()) => return Ok(temporary_path),
                    Err(error) => {
                        let _ = fs::remove_file(&temporary_path);
                        return Err(error.into());
                    }
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Err(error.into());
            }
        }
    }
    Err(AppError::conflict(format!(
        "could not allocate a temporary DuckDB backup artifact beside '{}'",
        destination.display()
    )))
}

pub(super) fn publish_temporary_artifact(
    temporary: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<()> {
    if overwrite {
        fs::rename(temporary, destination)?;
        return Ok(());
    }
    match fs::hard_link(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(AppError::conflict(format!(
                "backup artifact '{}' appeared while the backup was running",
                destination.display()
            )))
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn refuse_same_file(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        return Err(AppError::forbidden(
            "backup destination is the live database",
        ));
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (fs::metadata(source), fs::metadata(destination)) {
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Err(AppError::forbidden(
                "backup destination resolves to the live database inode",
            ));
        }
    }
    Ok(())
}
