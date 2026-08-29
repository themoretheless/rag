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

/// Return Markdown heading metadata active at each chunk start offset.
///
/// The result follows the input chunk order. Each item is `(heading_path, section)`,
/// where `section` is the leaf heading and unheaded chunks have empty metadata.
pub fn markdown_sections(
    text: &str,
    chunks: &[(String, i32, i32)],
) -> Vec<(Vec<String>, Option<String>)> {
    let mut headings: Vec<(i32, usize, String)> = Vec::new();
    let mut char_offset = 0i32;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes)
            && trimmed.as_bytes().get(hashes).is_some_and(|b| b.is_ascii_whitespace())
        {
            let title = trimmed[hashes..].trim().trim_end_matches('#').trim();
            if !title.is_empty() {
                headings.push((char_offset, hashes, title.to_string()));
            }
        }
        char_offset += line.chars().count() as i32;
    }

    let mut path: Vec<String> = Vec::new();
    let mut next_heading = 0usize;
    chunks
        .iter()
        .map(|(_, start, _)| {
            while let Some((offset, level, title)) = headings.get(next_heading) {
                if offset > start {
                    break;
                }
                path.truncate(level.saturating_sub(1));
                while path.len() < level.saturating_sub(1) {
                    path.push(String::new());
                }
                path.push(title.clone());
                next_heading += 1;
            }
            let clean: Vec<String> = path.iter().filter(|s| !s.is_empty()).cloned().collect();
            (clean.clone(), clean.last().cloned())
        })
        .collect()
}

/// Encode Markdown section metadata using the existing JSON metadata convention.
pub fn markdown_section_metadata(text: &str, chunks: &[(String, i32, i32)]) -> Vec<String> {
    markdown_sections(text, chunks)
        .into_iter()
        .map(|(heading_path, section)| {
            if heading_path.is_empty() {
                "{}".into()
            } else {
                serde_json::json!({
                    "heading_path": heading_path,
                    "section": section,
                })
                .to_string()
            }
        })
        .collect()
}
