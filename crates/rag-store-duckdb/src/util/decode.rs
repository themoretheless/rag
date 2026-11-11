//! Text decoding of source bytes: BOM, strict UTF-8, then a single-byte
//! fallback (Windows-1251 on Cyrillic evidence, else Windows-1252). Shared by
//! file ingest and the code-intelligence readers (drift, Git impact).

use std::path::Path;

use thiserror::Error;

/// Why bytes could not be decoded as text. The messages match the
/// corresponding `FileIngestError` variants of the ingest layer.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("cannot decode {path}: {message}")]
    Encoding { path: String, message: String },
    #[error("refusing to ingest binary-looking file {0}")]
    Binary(String),
}

/// Decode `bytes` read from `path` into text plus the encoding name and the
/// basis it was chosen on (`bom`, `strict_utf8`, `heuristic_*`).
pub fn decode_text(
    bytes: &[u8],
    path: &Path,
) -> Result<(String, &'static str, &'static str), DecodeError> {
    use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1251, WINDOWS_1252};
    let fail = || DecodeError::Encoding {
        path: path.display().to_string(),
        message: "malformed declared encoding; replacement characters are not inserted".into(),
    };
    let (text, encoding, basis) = if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (
            UTF_8
                .decode_without_bom_handling_and_without_replacement(bytes)
                .ok_or_else(fail)?
                .into_owned(),
            "utf-8-sig",
            "bom",
        )
    } else if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Err(DecodeError::Encoding {
            path: path.display().to_string(),
            message: "UTF-32 is unsupported".into(),
        });
    } else if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little = bytes.starts_with(&[0xff, 0xfe]);
        let decoder = if little { UTF_16LE } else { UTF_16BE };
        (
            decoder
                .decode_without_bom_handling_and_without_replacement(&bytes[2..])
                .ok_or_else(fail)?
                .into_owned(),
            if little { "utf-16le" } else { "utf-16be" },
            "bom",
        )
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        (text.into(), "utf-8", "strict_utf8")
    } else {
        // Single-byte Cyrillic evidence, matching the reference's deterministic
        // threshold. This is not proof of the author's intended encoding.
        let cyr = bytes
            .iter()
            .filter(|b| matches!(**b, 0xc0..=0xff | 0xa8 | 0xb8))
            .count();
        let letters = cyr + bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
        let russian = cyr >= 4 && cyr.saturating_mul(20) >= letters;
        let decoder = if russian { WINDOWS_1251 } else { WINDOWS_1252 };
        let text = decoder
            .decode_without_bom_handling_and_without_replacement(bytes)
            .ok_or_else(fail)?
            .into_owned();
        (
            text,
            if russian {
                "windows-1251"
            } else {
                "windows-1252"
            },
            if russian {
                "heuristic_cyrillic_share"
            } else {
                "heuristic_western_fallback"
            },
        )
    };
    if text.contains('\0')
        || (basis.starts_with("heuristic")
            && text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}')))
    {
        return Err(DecodeError::Binary(path.display().to_string()));
    }
    Ok((text, encoding, basis))
}
