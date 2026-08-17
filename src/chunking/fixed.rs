//! Fixed-size character window chunker with overlap.

/// One text span produced by [`FixedChunker`].
///
/// `char_start` / `char_end` are Unicode scalar indices into the original
/// content (`char_end` exclusive), matching the domain `Chunk` offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedChunk {
    pub char_start: i32,
    pub char_end: i32,
    pub content: String,
}

/// Fixed-size character chunker with configurable overlap.
///
/// Windows of `size` characters advance by roughly `size - overlap`. When the
/// window is not at the end of the input, a break is preferred at the last
/// whitespace (space / newline / other Unicode whitespace) inside the final
/// 20% of the window so words are less often split mid-token.
#[derive(Debug, Clone)]
pub struct FixedChunker {
    pub size: usize,
    pub overlap: usize,
}

impl FixedChunker {
    /// Create a chunker. `size` must be at least 1; `overlap` is clamped to
    /// `size.saturating_sub(1)` so each step always makes progress.
    pub fn new(size: usize, overlap: usize) -> Self {
        let size = size.max(1);
        let overlap = overlap.min(size.saturating_sub(1));
        Self { size, overlap }
    }

    /// Split `text` into overlapping character windows.
    ///
    /// Empty input yields no chunks. Offsets are relative to the original
    /// character sequence of `text`.
    pub fn chunk(&self, text: &str) -> Vec<FixedChunk> {
        if text.is_empty() {
            return Vec::new();
        }

        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut out = Vec::new();
        let mut start: usize = 0;

        while start < n {
            let ideal_end = (start + self.size).min(n);
            let mut end = ideal_end;

            // Soft break: prefer last whitespace/newline in the final 20% of
            // the window when more text remains after the ideal end.
            if ideal_end < n && ideal_end > start {
                let window_len = ideal_end - start;
                let soft_len = (window_len * 20) / 100;
                if soft_len > 0 {
                    let search_from = ideal_end - soft_len;
                    if let Some(rel) = chars[search_from..ideal_end]
                        .iter()
                        .rposition(|c| c.is_whitespace())
                    {
                        let break_at = search_from + rel + 1; // include the break char
                        if break_at > start {
                            end = break_at;
                        }
                    }
                }
            }

            // Guaranteed progress even if size/overlap are pathological.
            if end <= start {
                end = (start + 1).min(n).max(ideal_end.min(n));
                if end <= start {
                    end = n;
                }
            }

            let content: String = chars[start..end].iter().collect();
            out.push(FixedChunk {
                char_start: start as i32,
                char_end: end as i32,
                content,
            });

            if end >= n {
                break;
            }

            let mut next = end.saturating_sub(self.overlap);
            if next <= start {
                next = end;
            }
            start = next;
        }

        out
    }
}

impl super::Chunker for FixedChunker {
    fn chunk(&self, text: &str) -> Vec<(String, i32, i32)> {
        FixedChunker::chunk(self, text)
            .into_iter()
            .map(|c| (c.content, c.char_start, c.char_end))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_chunks() {
        let c = FixedChunker::new(10, 2);
        assert!(c.chunk("").is_empty());
    }

    #[test]
    fn short_text_is_single_chunk() {
        let c = FixedChunker::new(100, 10);
        let chunks = c.chunk("hello world");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "hello world");
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks[0].char_end, 11);
    }

    #[test]
    fn windows_respect_size_and_overlap() {
        // No soft break opportunity (no whitespace): hard windows of size 4,
        // step = size - overlap = 2.
        let c = FixedChunker::new(4, 2);
        let text = "abcdefghij"; // 10 chars
        let chunks = c.chunk(text);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks[0].content, "abcd");
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks[0].char_end, 4);
        // Next starts at 4 - 2 = 2
        assert_eq!(chunks[1].char_start, 2);
        assert_eq!(chunks[1].content, "cdef");
    }

    #[test]
    fn prefers_break_at_space_in_last_20_percent() {
        // size=10 → last 20% is 2 chars. Text has a space near the end of the
        // first window so we should break there instead of mid-word.
        let c = FixedChunker::new(10, 0);
        //          0123456789  (indices)
        let text = "abcdefgh ijk"; // space at index 8 within last 20% [8,10)
        let chunks = c.chunk(text);
        assert!(!chunks.is_empty());
        // Soft break includes the space: end = 9
        assert_eq!(chunks[0].char_end, 9);
        assert_eq!(chunks[0].content, "abcdefgh ");
        assert_eq!(chunks[1].content, "ijk");
    }

    #[test]
    fn prefers_break_at_newline() {
        let c = FixedChunker::new(10, 0);
        // newline at index 8 (last 20% of first 10-char window)
        let text = "abcdefgh\nijk";
        let chunks = c.chunk(text);
        assert_eq!(chunks[0].char_end, 9);
        assert_eq!(chunks[0].content, "abcdefgh\n");
        assert_eq!(chunks[1].content, "ijk");
    }

    #[test]
    fn no_soft_break_when_whitespace_outside_last_20_percent() {
        // size=10, last 20% = indices 8..10 of the window. Only space is early.
        let c = FixedChunker::new(10, 0);
        let text = "ab cdefghijXYZ"; // space at index 2, not in last 20%
        let chunks = c.chunk(text);
        assert_eq!(chunks[0].char_end, 10);
        assert_eq!(chunks[0].content, "ab cdefghi");
    }

    #[test]
    fn offsets_cover_full_text_without_gaps_when_overlap_zero() {
        let c = FixedChunker::new(5, 0);
        let text = "0123456789abc"; // 13 chars
        let chunks = c.chunk(text);
        assert_eq!(chunks[0].char_start, 0);
        for w in chunks.windows(2) {
            assert_eq!(w[0].char_end, w[1].char_start);
        }
        assert_eq!(chunks.last().unwrap().char_end as usize, text.chars().count());
    }

    #[test]
    fn unicode_chars_counted_not_bytes() {
        let c = FixedChunker::new(3, 1);
        let text = "привет"; // 6 Cyrillic chars
        let chunks = c.chunk(text);
        assert_eq!(chunks[0].content.chars().count(), 3);
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks[0].char_end, 3);
        // step = 3 - 1 = 2
        assert_eq!(chunks[1].char_start, 2);
    }

    #[test]
    fn overlap_clamped_below_size() {
        let c = FixedChunker::new(5, 100);
        assert_eq!(c.overlap, 4);
        let chunks = c.chunk("0123456789");
        assert!(!chunks.is_empty());
        // Must terminate
        assert!(chunks.len() < 100);
    }

    #[test]
    fn content_matches_slice_of_original() {
        let c = FixedChunker::new(8, 3);
        let text = "The quick brown fox jumps";
        let chars: Vec<char> = text.chars().collect();
        for ch in c.chunk(text) {
            let s = ch.char_start as usize;
            let e = ch.char_end as usize;
            let expected: String = chars[s..e].iter().collect();
            assert_eq!(ch.content, expected);
        }
    }
}
