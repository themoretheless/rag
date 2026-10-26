//! Extract binary document formats from exactly the bytes whose digest is recorded.
use super::*;
use std::io::{Read, Write};

pub(super) struct Snapshot {
    file: tempfile::NamedTempFile,
    pub hash: String,
    pub bytes: u64,
}
impl Snapshot {
    pub fn read(path: &Path) -> Result<Self, FileIngestError> {
        let fail = |source| FileIngestError::Read {
            path: path.display().to_string(),
            source,
        };
        let mut input = std::fs::File::open(path).map_err(fail)?;
        let mut file = tempfile::Builder::new()
            .prefix("rag-source-capture-")
            .tempfile()
            .map_err(fail)?;
        let mut hash = blake3::Hasher::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buffer).map_err(fail)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).map_err(fail)?;
            hash.update(&buffer[..n]);
            bytes += n as u64;
        }
        file.flush().map_err(fail)?;
        Ok(Self {
            file,
            hash: hash.finalize().to_hex().to_string(),
            bytes,
        })
    }
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}
/// Errors should identify the requested source, not the implementation's temporary file.
pub(super) fn source_error(error: FileIngestError, path: &Path) -> FileIngestError {
    let name = path.display().to_string();
    match error {
        FileIngestError::Read { source, .. } => FileIngestError::Read { path: name, source },
        FileIngestError::PdfToolMissing { .. } => FileIngestError::PdfToolMissing { path: name },
        FileIngestError::PdfExtract { message, .. } => FileIngestError::PdfExtract {
            path: name,
            message,
        },
        FileIngestError::Office {
            format, message, ..
        } => FileIngestError::Office {
            path: name,
            format,
            message,
        },
        other => other,
    }
}
