//! Label normalization (`GRAPH_DESIGN.md` §3).
//!
//! One key only: `label_key`. Display casing is preserved by [`display_label`].

use unicode_normalization::UnicodeNormalization;

/// Match / promote / stub key: trim → NFKC → Unicode lowercase → collapse
/// internal whitespace to single spaces.
///
/// Documented limits (GRAPH_DESIGN §3): `to_lowercase` is not a full Unicode
/// CaseFold (Turkish dotless I, final sigma), and homoglyphs stay distinct keys.
pub fn label_key(s: &str) -> String {
    display_label(s)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Display form only: trim + NFKC, casing preserved.
pub fn display_label(s: &str) -> String {
    s.trim().nfkc().collect()
}

/// Tag key: same normalization as any other label.
pub fn tag_key(s: &str) -> String {
    label_key(s.trim().trim_start_matches('#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_key_case_folds_and_trims() {
        assert_eq!(label_key("  Rag-MCP Overview "), "rag-mcp overview");
        assert_eq!(
            label_key("RAG-MCP overview"),
            label_key("rag-mcp   overview")
        );
    }

    #[test]
    fn label_key_applies_nfkc() {
        // Fullwidth Latin folds to ASCII, and the fi ligature decomposes.
        assert_eq!(label_key("ＦＵＬＬＷＩＤＴＨ"), "fullwidth");
        assert_eq!(label_key("ﬁle"), "file");
    }

    #[test]
    fn display_label_keeps_casing_but_nfkc_normalizes() {
        assert_eq!(display_label("  Foo Ｂar  "), "Foo Bar");
        assert_eq!(label_key(&display_label("Foo Ｂar")), "foo bar");
    }

    #[test]
    fn tag_key_drops_leading_hash() {
        assert_eq!(tag_key("#Multi/Level"), "multi/level");
        assert_eq!(tag_key("Multi/Level"), tag_key("# Multi/Level"));
    }
}
