//! Text chunking: trait, factory, and fixed-size implementation.

pub mod fixed;

pub use fixed::FixedChunker;

/// Splits document text into content windows with character offsets.
pub trait Chunker {
    /// Split `text` into chunks.
    ///
    /// Each item is `(content, char_start, char_end)` where offsets are
    /// relative to the original `text` (half-open range semantics:
    /// `content` equals `text[char_start as usize..char_end as usize]`
    /// for the underlying char-index mapping used by the implementation).
    fn chunk(&self, text: &str) -> Vec<(String, i32, i32)>;
}

/// Build a fixed-size char chunker from configuration values.
pub fn from_config(chunk_size: usize, overlap: usize) -> FixedChunker {
    FixedChunker::new(chunk_size, overlap)
}
