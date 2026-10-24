//! Structural source-code chunking without a parser (tree-sitter is not
//! available in the offline build environment).
//!
//! Supported languages:
//! - **Brace family** (`rust`, `javascript`, `typescript`): top-level items are
//!   detected at brace depth 0 after masking comments and string/char/template
//!   literals. An item runs from its signature line (plus contiguous
//!   attribute / doc-comment / decorator lines above it) to the line where the
//!   brace depth returns to 0, or to a top-level `;` for brace-less items.
//! - **Indent family** (`python`): top-level `def` / `async def` / `class`
//!   (with decorators) at indent 0; an item runs until the next item's first
//!   line at indent 0.
//!
//! Oversized items fall back to window splitting inside the item span (reusing
//! [`super::code_chunks`]); continuation parts carry `part > 0` so the indexer
//! can link them to the item's first part via `parent_chunk_id`.
//!
//! Merge policy: "hard" items (functions, classes, impls, structs, enums,
//! traits, interfaces, macros, modules) always stay isolated. Only soft spans
//! — unlabeled gaps and small `constant` / `type_alias` items — merge greedily
//! up to `chunk_size`; a merged chunk keeps symbol metadata only when it holds
//! exactly one labeled item.
//!
//! Limitations (documented, deterministic): no tree-sitter, so nested
//! functions are not items of their own, JS template literals are masked
//! naively (`${…}` contents count as string), and Rust raw strings with inner
//! quotes can desync the brace depth — in that case items degrade to
//! window-sized spans rather than failing.

/// One structural chunk with optional symbol metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralChunk {
    pub content: String,
    pub char_start: i32,
    pub char_end: i32,
    /// Top-level symbol name when the chunk maps to exactly one item.
    pub symbol: Option<String>,
    /// `function` | `class` | `impl` | `struct` | `enum` | `trait` | `module`
    /// | `constant` | `type_alias` | `macro` | `interface`.
    pub symbol_kind: Option<&'static str>,
    /// 0-based index of this window inside an oversized item (0 = whole item
    /// or first part).
    pub part: usize,
    /// Total window count for the item this chunk belongs to (1 = unsplit).
    pub parts: usize,
}

/// Languages handled by [`structural_code_chunks`]; anything else keeps the
/// legacy window splitter upstream.
pub fn is_structural_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "rust" | "python" | "javascript" | "typescript"
    )
}

/// Split `text` into top-level items for `language`
/// (`rust` | `python` | `javascript` | `typescript`).
pub fn structural_code_chunks(
    text: &str,
    language: &str,
    chunk_size: usize,
    overlap: usize,
) -> Vec<StructuralChunk> {
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let items = match language.trim().to_ascii_lowercase().as_str() {
        "python" => python_items(&chars),
        _ => brace_items(&chars, language),
    };
    assemble(&chars, items, chunk_size, overlap)
}

/// A detected top-level item: char span plus symbol identity.
#[derive(Debug, Clone)]
struct Item {
    start: usize,
    end: usize,
    symbol: Option<String>,
    kind: Option<&'static str>,
    /// Hard items (functions, classes, impls, …) never merge with neighbors.
    hard: bool,
}

fn is_hard_kind(kind: Option<&'static str>) -> bool {
    matches!(
        kind,
        Some("function")
            | Some("class")
            | Some("impl")
            | Some("struct")
            | Some("enum")
            | Some("trait")
            | Some("interface")
            | Some("macro")
            | Some("module")
    )
}

/// Turn item spans into final chunks: pass gaps through, merge adjacent soft
/// spans, and window-split oversized spans.
fn assemble(
    chars: &[char],
    items: Vec<Item>,
    chunk_size: usize,
    overlap: usize,
) -> Vec<StructuralChunk> {
    let chunk_size = chunk_size.max(1);

    // Interleave gaps (unlabeled module-level text) between items.
    let mut spans: Vec<Item> = Vec::new();
    let mut cursor = 0usize;
    for item in items {
        let start = item.start.min(chars.len());
        let end = item.end.min(chars.len()).max(start);
        if start > cursor {
            spans.push(Item {
                start: cursor,
                end: start,
                symbol: None,
                kind: None,
                hard: false,
            });
        }
        if end > start {
            cursor = end;
            spans.push(item);
        }
    }
    if cursor < chars.len() {
        spans.push(Item {
            start: cursor,
            end: chars.len(),
            symbol: None,
            kind: None,
            hard: false,
        });
    }

    // Greedy merge of adjacent soft spans; hard items flush and stand alone.
    let mut out = Vec::new();
    let mut buffer: Vec<Item> = Vec::new();
    for span in spans {
        let size = span.end - span.start;
        if size == 0 {
            continue;
        }
        if span.hard {
            flush_buffer(&mut out, chars, &mut buffer, chunk_size, overlap);
            emit_span(&mut out, chars, span, chunk_size, overlap);
            continue;
        }
        let buffered: usize = buffer.iter().map(|s| s.end - s.start).sum();
        if buffered + size > chunk_size {
            flush_buffer(&mut out, chars, &mut buffer, chunk_size, overlap);
        }
        buffer.push(span);
    }
    flush_buffer(&mut out, chars, &mut buffer, chunk_size, overlap);
    out
}

/// Emit the accumulated soft-span buffer as one merged chunk (or nothing).
fn flush_buffer(
    out: &mut Vec<StructuralChunk>,
    chars: &[char],
    buffer: &mut Vec<Item>,
    chunk_size: usize,
    overlap: usize,
) {
    if buffer.is_empty() {
        return;
    }
    let start = buffer.first().map(|s| s.start).unwrap_or(0);
    let end = buffer.last().map(|s| s.end).unwrap_or(start);
    let labeled: Vec<&Item> = buffer.iter().filter(|s| s.symbol.is_some()).collect();
    let (symbol, kind) = match labeled.as_slice() {
        [only] => (only.symbol.clone(), only.kind),
        _ => (None, None),
    };
    buffer.clear();
    emit_span(
        out,
        chars,
        Item {
            start,
            end,
            symbol,
            kind,
            hard: false,
        },
        chunk_size,
        overlap,
    );
}

/// Emit one span, window-splitting inside its own bounds when oversized.
fn emit_span(
    out: &mut Vec<StructuralChunk>,
    chars: &[char],
    span: Item,
    chunk_size: usize,
    overlap: usize,
) {
    let size = span.end - span.start;
    if size == 0 {
        return;
    }
    if size <= chunk_size.max(1) {
        let content: String = chars[span.start..span.end].iter().collect();
        out.push(StructuralChunk {
            content,
            char_start: span.start as i32,
            char_end: span.end as i32,
            symbol: span.symbol,
            symbol_kind: span.kind,
            part: 0,
            parts: 1,
        });
        return;
    }
    let item_text: String = chars[span.start..span.end].iter().collect();
    let windows = super::code_chunks(&item_text, chunk_size, overlap);
    let parts = windows.len();
    for (part, (content, rel_start, rel_end)) in windows.into_iter().enumerate() {
        out.push(StructuralChunk {
            content,
            char_start: span.start as i32 + rel_start,
            char_end: span.start as i32 + rel_end,
            symbol: span.symbol.clone(),
            symbol_kind: span.kind,
            part,
            parts,
        });
    }
}

/// `(start, end)` char spans of each line; `end` is just past the `\n`.
fn line_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            spans.push((start, i + 1));
            start = i + 1;
        }
    }
    if start < chars.len() {
        spans.push((start, chars.len()));
    }
    spans
}

// ---------------------------------------------------------------------------
// Brace family (rust, javascript, typescript)
// ---------------------------------------------------------------------------

/// Mask comments and string/char/template literals with spaces (newlines are
/// preserved), so brace depth reflects real structure.
fn mask_brace_family(chars: &[char], is_rust: bool) -> Vec<char> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        LineComment,
        BlockComment(usize),
        Str(char),
        Template,
    }
    let mut out = chars.to_vec();
    let mut state = State::Code;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match state {
            State::Code => match (c, next) {
                ('/', Some('/')) => {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    state = State::LineComment;
                    i += 1;
                }
                ('/', Some('*')) => {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    state = State::BlockComment(1);
                    i += 1;
                }
                ('"', _) => {
                    out[i] = ' ';
                    state = State::Str('"');
                }
                ('\'', _) if !is_rust => {
                    out[i] = ' ';
                    state = State::Str('\'');
                }
                ('\'', _) if is_rust => {
                    // Rust: distinguish a `'a'` char literal from a lifetime.
                    let closing = chars.get(i + 2).copied();
                    let after_escape = chars.get(i + 3).copied();
                    let is_char =
                        closing == Some('\'') || (closing == Some('\\') && after_escape.is_some());
                    if is_char {
                        out[i] = ' ';
                        state = State::Str('\'');
                    }
                }
                ('`', _) if !is_rust => {
                    out[i] = ' ';
                    state = State::Template;
                }
                _ => {}
            },
            State::LineComment => {
                if c == '\n' {
                    state = State::Code;
                } else {
                    out[i] = ' ';
                }
            }
            State::BlockComment(depth) => {
                if c == '/' && next == Some('*') && is_rust {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    state = State::BlockComment(depth + 1);
                    i += 1;
                } else if c == '*' && next == Some('/') {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    let depth = depth - 1;
                    state = if depth == 0 {
                        State::Code
                    } else {
                        State::BlockComment(depth)
                    };
                    i += 1;
                } else if c != '\n' {
                    out[i] = ' ';
                }
            }
            State::Str(q) => {
                if c == '\\' {
                    out[i] = ' ';
                    if let Some(n) = next {
                        if n != '\n' {
                            out[i + 1] = ' ';
                        }
                        i += 1;
                    }
                } else if c == q {
                    out[i] = ' ';
                    state = State::Code;
                } else if c == '\n' && !is_rust {
                    // Unterminated JS string: recover at the newline.
                    state = State::Code;
                } else if c != '\n' {
                    out[i] = ' ';
                }
            }
            State::Template => {
                if c == '\\' {
                    out[i] = ' ';
                    if next.is_some() {
                        out[i + 1] = ' ';
                        i += 1;
                    }
                } else if c == '`' {
                    out[i] = ' ';
                    state = State::Code;
                } else if c != '\n' {
                    out[i] = ' ';
                }
            }
        }
        i += 1;
    }
    out
}

/// Detect top-level items in a brace-family source file.
fn brace_items(chars: &[char], language: &str) -> Vec<Item> {
    let is_rust = language.trim().eq_ignore_ascii_case("rust");
    let masked = mask_brace_family(chars, is_rust);
    let lines = line_spans(chars);

    // Brace depth at the start of each line (from the masked text).
    let mut depth_at_line = Vec::with_capacity(lines.len());
    let mut depth = 0i64;
    for (start, end) in &lines {
        depth_at_line.push(depth);
        for c in &masked[*start..*end] {
            match c {
                '{' => depth += 1,
                '}' => depth = (depth - 1).max(0),
                _ => {}
            }
        }
    }

    let mut items = Vec::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        if depth_at_line[idx] != 0 {
            idx += 1;
            continue;
        }
        let header: String = masked[lines[idx].0..lines[idx].1]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        let Some((kind, name)) = brace_item_header(&header, is_rust) else {
            idx += 1;
            continue;
        };

        // Extend start upwards over contiguous attribute / doc / decorator
        // lines, checked on the ORIGINAL text (masking blanks comments out).
        let mut first_line = idx;
        while first_line > 0 {
            let prev: String = chars[lines[first_line - 1].0..lines[first_line - 1].1]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            let attaches = if is_rust {
                prev.starts_with("#[") || prev.starts_with("///") || prev.starts_with("//!")
            } else {
                prev.starts_with('@')
                    || prev.starts_with("/**")
                    || prev.starts_with('*')
                    || prev.ends_with("*/")
            };
            if attaches {
                first_line -= 1;
            } else {
                break;
            }
        }

        // Find the end: the first `{` at depth 0 opens the body; the item
        // ends where depth returns to 0. A top-level `;` before any `{`
        // closes a brace-less item (type alias, static, signature).
        let mut end_char = masked.len();
        let mut scan_depth = 0i64;
        let mut saw_open = false;
        let mut pos = lines[idx].0;
        while pos < masked.len() {
            match masked[pos] {
                '{' => {
                    scan_depth += 1;
                    saw_open = true;
                }
                '}' if saw_open => {
                    scan_depth -= 1;
                    if scan_depth == 0 {
                        while pos < masked.len() && masked[pos] != '\n' {
                            pos += 1;
                        }
                        end_char = pos;
                        break;
                    }
                }
                ';' if !saw_open && scan_depth == 0 => {
                    end_char = pos + 1;
                    break;
                }
                _ => {}
            }
            pos += 1;
        }

        items.push(Item {
            start: lines[first_line].0,
            end: end_char,
            symbol: name,
            kind: Some(kind),
            hard: is_hard_kind(Some(kind)),
        });
        while idx < lines.len() && lines[idx].0 < end_char {
            idx += 1;
        }
    }
    items
}

/// Parse an item header line at brace depth 0. Returns `(kind, name)`.
fn brace_item_header(line: &str, is_rust: bool) -> Option<(&'static str, Option<String>)> {
    let mut rest = line;
    // Strip visibility / modifier prefixes.
    loop {
        let trimmed = rest.trim_start();
        let stripped = if let Some(r) = trimmed.strip_prefix("pub(crate)") {
            r
        } else if let Some(r) = trimmed.strip_prefix("pub(super)") {
            r
        } else if let Some(r) = trimmed.strip_prefix("pub ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("export default ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("export ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("async ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("unsafe ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("default ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("declare ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("abstract ") {
            r
        } else if let Some(r) = trimmed.strip_prefix("extern ") {
            // extern "C" fn … — the ABI string is already masked to blanks.
            r
        } else {
            break;
        };
        rest = stripped;
    }
    let rest = rest.trim_start();

    if is_rust {
        for (prefix, kind) in [
            ("fn ", "function"),
            ("impl ", "impl"),
            ("struct ", "struct"),
            ("enum ", "enum"),
            ("trait ", "trait"),
            ("mod ", "module"),
            ("const ", "constant"),
            ("static ", "constant"),
            ("type ", "type_alias"),
        ] {
            if let Some(after) = rest.strip_prefix(prefix) {
                return Some((kind, extract_ident(after)));
            }
        }
        if let Some(after) = rest.strip_prefix("macro_rules!") {
            return Some(("macro", extract_ident(after)));
        }
        None
    } else {
        for (prefix, kind) in [
            ("function* ", "function"),
            ("function ", "function"),
            ("class ", "class"),
            ("interface ", "interface"),
            ("enum ", "enum"),
            ("type ", "type_alias"),
        ] {
            if let Some(after) = rest.strip_prefix(prefix) {
                return Some((kind, extract_ident(after)));
            }
        }
        for prefix in ["const ", "let ", "var "] {
            if let Some(after) = rest.strip_prefix(prefix) {
                return Some(("constant", extract_ident(after)));
            }
        }
        None
    }
}

/// Extract the first identifier token from `text` (skipping generics).
fn extract_ident(text: &str) -> Option<String> {
    let text = text.trim_start();
    // Skip a leading generic parameter list like `<T>` (rust impl<T>).
    let text = if text.starts_with('<') {
        let mut depth = 0i64;
        let mut end = 0usize;
        for (i, c) in text.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end == 0 {
            return None;
        }
        text[end..].trim_start()
    } else {
        text
    };
    let ident: String = text
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    if !ident.is_empty() {
        return Some(ident);
    }
    // `impl Trait for Type` — fall back to the last word before `{`/`;`.
    let fallback: String = text
        .chars()
        .take_while(|c| *c != '{' && *c != ';' && *c != '\n')
        .collect();
    let last = fallback.split_whitespace().last().unwrap_or("");
    let cleaned: String = last
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

// ---------------------------------------------------------------------------
// Python (indentation based)
// ---------------------------------------------------------------------------

fn python_items(chars: &[char]) -> Vec<Item> {
    let lines = line_spans(chars);
    let line_texts: Vec<String> = lines
        .iter()
        .map(|(s, e)| chars[*s..*e].iter().collect())
        .collect();

    // Item boundaries: non-blank, non-comment lines at indent 0, with
    // decorator lines pulled into the following def/class.
    let mut starts: Vec<usize> = Vec::new();
    for (i, text) in line_texts.iter().enumerate() {
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if text.len() == trimmed.len() {
            starts.push(i);
        }
    }
    let extended: Vec<usize> = starts
        .iter()
        .map(|&start| {
            let mut start = start;
            while start > 0 && line_texts[start - 1].trim_start().starts_with('@') {
                start -= 1;
            }
            start
        })
        .collect();

    let mut items = Vec::new();
    for (n, &start_line) in extended.iter().enumerate() {
        let end_char = extended
            .get(n + 1)
            .map(|&next| lines[next].0)
            .unwrap_or(chars.len());
        let header = line_texts[starts[n]].trim();
        let (kind, name, hard) = if let Some(after) = header.strip_prefix("async def ") {
            ("function", extract_ident(after), true)
        } else if let Some(after) = header.strip_prefix("def ") {
            ("function", extract_ident(after), true)
        } else if let Some(after) = header.strip_prefix("class ") {
            ("class", extract_ident(after), true)
        } else {
            // Other top-level statements (imports, constants) stay unlabeled.
            ("module", None, false)
        };
        items.push(Item {
            start: lines[start_line].0,
            end: end_char,
            symbol: name,
            kind: Some(kind),
            hard,
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_symbols(chunks: &[StructuralChunk]) -> Vec<(Option<String>, Option<&'static str>)> {
        chunks
            .iter()
            .map(|c| (c.symbol.clone(), c.symbol_kind))
            .collect()
    }

    #[test]
    fn rust_functions_are_isolated() {
        let src = r#"use std::io;

fn alpha(x: i32) -> i32 {
    x + 1
}

fn beta() {
    let s = "}";
    println!("{s}");
}
"#;
        let chunks = structural_code_chunks(src, "rust", 800, 120);
        let fns: Vec<_> = chunks
            .iter()
            .filter(|c| c.symbol_kind == Some("function"))
            .collect();
        assert_eq!(fns.len(), 2, "chunks: {chunks:?}");
        assert_eq!(fns[0].symbol.as_deref(), Some("alpha"));
        assert_eq!(fns[1].symbol.as_deref(), Some("beta"));
        assert!(
            fns[1].content.contains("\"}\""),
            "brace inside string must not close the fn"
        );
        // Contents slice the original text exactly.
        let chars: Vec<char> = src.chars().collect();
        for c in &chunks {
            let expected: String = chars[c.char_start as usize..c.char_end as usize]
                .iter()
                .collect();
            assert_eq!(c.content, expected);
        }
    }

    #[test]
    fn rust_impl_struct_and_attributes() {
        let src = r#"/// Documented.
#[derive(Debug)]
pub struct Point {
    x: f64,
}

impl Point {
    pub fn new(x: f64) -> Self {
        Self { x }
    }
}

pub(crate) const MAX: usize = 3;
"#;
        let chunks = structural_code_chunks(src, "rust", 800, 120);
        let kinds = chunk_symbols(&chunks);
        assert!(
            kinds.contains(&(Some("Point".into()), Some("struct"))),
            "struct item: {kinds:?}"
        );
        assert!(
            kinds.contains(&(Some("Point".into()), Some("impl"))),
            "impl item: {kinds:?}"
        );
        assert!(
            kinds.contains(&(Some("MAX".into()), Some("constant"))),
            "const item: {kinds:?}"
        );
        let struct_chunk = chunks
            .iter()
            .find(|c| c.symbol_kind == Some("struct"))
            .unwrap();
        assert!(struct_chunk.content.starts_with("/// Documented."));
    }

    #[test]
    fn oversized_item_falls_back_to_window_split() {
        let body = "    let x = 1;\n".repeat(200);
        let src = format!("fn big() {{\n{body}}}\nfn small() {{}}\n");
        let chunks = structural_code_chunks(&src, "rust", 200, 20);
        let big_parts: Vec<_> = chunks
            .iter()
            .filter(|c| c.symbol.as_deref() == Some("big"))
            .collect();
        assert!(
            big_parts.len() > 1,
            "oversized fn must split: {big_parts:?}"
        );
        assert!(big_parts.iter().all(|c| c.parts == big_parts.len()));
        assert!(big_parts.iter().enumerate().all(|(i, c)| c.part == i));
        let small: Vec<_> = chunks
            .iter()
            .filter(|c| c.symbol.as_deref() == Some("small"))
            .collect();
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].parts, 1);
    }

    #[test]
    fn tiny_adjacent_items_merge_up_to_chunk_size() {
        let src = "const A: u8 = 1;\nconst B: u8 = 2;\nconst C: u8 = 3;\n";
        let chunks = structural_code_chunks(src, "rust", 100, 10);
        assert_eq!(chunks.len(), 1, "tiny consts merge: {chunks:?}");
        assert_eq!(chunks[0].symbol, None, "merged chunk drops symbol identity");
        assert_eq!(chunks[0].content, src);
    }

    #[test]
    fn python_defs_and_classes_are_isolated() {
        let src = "import os\n\n\ndef alpha(x):\n    return x + 1\n\n\nclass Beta:\n    def method(self):\n        return 1\n\n\n@decorator\ndef gamma():\n    pass\n";
        let chunks = structural_code_chunks(src, "python", 800, 120);
        let alpha = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("alpha"))
            .expect("alpha");
        assert_eq!(alpha.symbol_kind, Some("function"));
        assert!(alpha.content.contains("return x + 1"));
        let beta = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("Beta"))
            .expect("Beta");
        assert_eq!(beta.symbol_kind, Some("class"));
        assert!(beta.content.contains("def method"));
        let gamma = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("gamma"))
            .expect("gamma");
        assert!(
            gamma.content.contains("@decorator"),
            "decorator attaches to def"
        );
    }

    #[test]
    fn javascript_items_cover_functions_arrows_and_classes() {
        let src = "import x from 'y';\n\nfunction alpha(a) {\n  return a * 2;\n}\n\nexport const beta = (b) => {\n  return `t${b}}`;\n};\n\nexport default class Gamma {\n  run() {\n    return 1;\n  }\n}\n";
        let chunks = structural_code_chunks(src, "javascript", 800, 120);
        let kinds = chunk_symbols(&chunks);
        assert!(
            kinds.contains(&(Some("alpha".into()), Some("function"))),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&(Some("beta".into()), Some("constant"))),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&(Some("Gamma".into()), Some("class"))),
            "{kinds:?}"
        );
        let beta = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("beta"))
            .unwrap();
        assert!(
            beta.content.contains('`'),
            "template literal stays inside the item"
        );
    }

    #[test]
    fn typescript_prefixes_are_stripped() {
        let src = "export async function load(): Promise<void> {\n  await go();\n}\n\nexport interface Shape {\n  name: string;\n}\n";
        let chunks = structural_code_chunks(src, "typescript", 800, 120);
        let kinds = chunk_symbols(&chunks);
        assert!(
            kinds.contains(&(Some("load".into()), Some("function"))),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&(Some("Shape".into()), Some("interface"))),
            "{kinds:?}"
        );
    }

    #[test]
    fn splitting_is_deterministic() {
        let src = "fn a() {}\n\nfn b() { { } }\n\nlet mut v = 1;\n";
        let first = structural_code_chunks(src, "rust", 800, 120);
        let second = structural_code_chunks(src, "rust", 800, 120);
        assert_eq!(first, second);
    }

    #[test]
    fn unsupported_language_gate() {
        assert!(is_structural_language("rust"));
        assert!(is_structural_language("Python"));
        assert!(!is_structural_language("go"));
    }
}
