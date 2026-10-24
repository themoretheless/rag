//! Pure-Rust text extraction for OOXML formats (.docx, .xlsx).
//!
//! Both formats are zip archives of XML parts; we read only the parts needed
//! for text (plus style/shared-string tables) and emit Markdown-ish text:
//! headings become `#`-prefixed lines so the heading-aware chunker keeps
//! section metadata, and table rows become `cell | cell` lines.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use crate::file_ingest::FileIngestError;

fn office_error(path: &Path, format: &'static str, message: impl Into<String>) -> FileIngestError {
    FileIngestError::Office {
        path: path.display().to_string(),
        format,
        message: message.into(),
    }
}

fn read_part(
    archive: &mut ZipArchive<std::io::Cursor<Vec<u8>>>,
    path: &Path,
    format: &'static str,
    name: &str,
) -> Result<String, FileIngestError> {
    let mut part = archive
        .by_name(name)
        .map_err(|error| office_error(path, format, format!("missing {name}: {error}")))?;
    let mut bytes = Vec::new();
    part.read_to_end(&mut bytes)
        .map_err(|error| office_error(path, format, format!("cannot read {name}: {error}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn open_archive(
    path: &Path,
    format: &'static str,
) -> Result<ZipArchive<std::io::Cursor<Vec<u8>>>, FileIngestError> {
    let bytes = std::fs::read(path).map_err(|source| FileIngestError::Read {
        path: path.display().to_string(),
        source,
    })?;
    ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| {
        office_error(
            path,
            format,
            format!("not a valid {format} archive: {error}"),
        )
    })
}

fn local_name(raw: &str) -> &str {
    match raw.as_bytes().iter().rposition(|byte| *byte == b':') {
        Some(index) => &raw[index + 1..],
        None => raw,
    }
}

/// Decode the standard XML entities plus numeric character references.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find(';') {
            Some(end) if end <= 10 => {
                let entity = &after[1..end];
                let decoded = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                match decoded {
                    Some(character) => output.push(character),
                    None => output.push_str(&after[..=end]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                output.push('&');
                rest = &after[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

/// Push a `GeneralRef` event (`&entity;` / `&#NN;`) decoded into `target`.
fn push_entity_ref(target: &mut String, reference: &quick_xml::events::BytesRef<'_>) {
    target.push_str(&decode_entities(&format!("&{};", &**reference)));
}

fn attr_value(event: &quick_xml::events::BytesStart<'_>, wanted: &str) -> Option<String> {
    for attribute in event.attributes().flatten() {
        if local_name(attribute.key.as_ref()) == wanted {
            return Some(decode_entities(&attribute.value));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------

/// Map paragraph style ids to Markdown heading levels using `word/styles.xml`.
/// A style is a heading when its name is `heading N` or it carries an outline
/// level; both are how Word and common generators mark headings.
fn docx_heading_styles(path: &Path, xml: &str) -> HashMap<String, usize> {
    let mut styles = HashMap::new();
    let mut reader = Reader::from_str(xml);
    let mut current: Option<(String, Option<usize>)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match event.local_name().into_inner() {
                    "style" => {
                        if let Some(id) = attr_value(&event, "styleId") {
                            current = Some((id, None));
                        }
                    }
                    "name" => {
                        if let Some((_, level)) = current.as_mut() {
                            if let Some(name) = attr_value(&event, "val") {
                                let lower = name.to_ascii_lowercase();
                                if let Some(digits) = lower.strip_prefix("heading ") {
                                    if let Ok(parsed) = digits.trim().parse::<usize>() {
                                        *level = Some(parsed.clamp(1, 6));
                                    }
                                }
                            }
                        }
                    }
                    "outlineLvl" => {
                        if let Some((_, level)) = current.as_mut() {
                            if let Some(value) = attr_value(&event, "val") {
                                if let Ok(parsed) = value.parse::<usize>() {
                                    *level = Some((parsed + 1).clamp(1, 6));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) => {
                if event.local_name().into_inner() == "style" {
                    if let Some((id, Some(level))) = current.take() {
                        styles.insert(id, level);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                tracing::warn!(path = %path.display(), "docx styles.xml parse stopped early: {error}");
                break;
            }
            _ => {}
        }
    }
    styles
}

fn style_fallback_level(style_id: &str) -> Option<usize> {
    let lower = style_id.to_ascii_lowercase();
    let digits: String = lower.chars().filter(|c| c.is_ascii_digit()).collect();
    if lower.starts_with("heading") && !digits.is_empty() {
        digits.parse::<usize>().ok().map(|level| level.clamp(1, 6))
    } else {
        None
    }
}

/// Extract text from a .docx file: headings as `#` lines, table rows as
/// `cell | cell`, everything else as plain paragraphs.
pub fn extract_docx(path: &Path) -> Result<(String, usize), FileIngestError> {
    let mut archive = open_archive(path, "docx")?;
    let styles = match read_part(&mut archive, path, "docx", "word/styles.xml") {
        Ok(xml) => docx_heading_styles(path, &xml),
        Err(_) => HashMap::new(), // styles are optional; degrade to plain text
    };
    let document = read_part(&mut archive, path, "docx", "word/document.xml")?;

    let mut lines: Vec<String> = Vec::new();
    let mut heading_count = 0usize;
    let mut paragraph: Option<(Option<String>, String)> = None;
    let mut in_text = false;
    let mut cell: Option<String> = None;
    let mut row: Option<Vec<String>> = None;

    let mut reader = Reader::from_str(&document);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().into_inner() {
                "p" => paragraph = Some((None, String::new())),
                "pStyle" => {
                    if let Some((style, _)) = paragraph.as_mut() {
                        *style = attr_value(&event, "val");
                    }
                }
                "t" => in_text = true,
                "tab" => {
                    if let Some((_, text)) = paragraph.as_mut() {
                        text.push('\t');
                    }
                }
                "br" | "cr" => {
                    if let Some((_, text)) = paragraph.as_mut() {
                        text.push('\n');
                    }
                }
                "tr" => row = Some(Vec::new()),
                "tc" => cell = Some(String::new()),
                _ => {}
            },
            Ok(Event::Empty(event)) => match event.local_name().into_inner() {
                "pStyle" => {
                    if let Some((style, _)) = paragraph.as_mut() {
                        *style = attr_value(&event, "val");
                    }
                }
                "tab" => {
                    if let Some((_, text)) = paragraph.as_mut() {
                        text.push('\t');
                    }
                }
                "br" | "cr" => {
                    if let Some((_, text)) = paragraph.as_mut() {
                        text.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Text(event)) => {
                if in_text {
                    if let Some((_, text)) = paragraph.as_mut() {
                        text.push_str(&event.xml10_content());
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if in_text {
                    if let Some((_, text)) = paragraph.as_mut() {
                        push_entity_ref(text, &reference);
                    }
                }
            }
            Ok(Event::End(event)) => match event.local_name().into_inner() {
                "t" => in_text = false,
                "p" => {
                    if let Some((style, text)) = paragraph.take() {
                        let trimmed = text.trim();
                        if let Some(current) = cell.as_mut() {
                            if !trimmed.is_empty() {
                                if !current.is_empty() {
                                    current.push(' ');
                                }
                                current.push_str(trimmed);
                            }
                        } else if !trimmed.is_empty() {
                            let level = style.as_deref().and_then(|id| {
                                styles.get(id).copied().or_else(|| style_fallback_level(id))
                            });
                            match level {
                                Some(level) => {
                                    heading_count += 1;
                                    lines.push(format!("{} {}", "#".repeat(level), trimmed));
                                }
                                None => lines.push(trimmed.to_string()),
                            }
                        }
                    }
                }
                "tc" => {
                    if let (Some(current_row), Some(finished)) = (row.as_mut(), cell.take()) {
                        current_row.push(finished.trim().to_string());
                    }
                }
                "tr" => {
                    if let Some(finished) = row.take() {
                        if finished.iter().any(|c| !c.is_empty()) {
                            lines.push(finished.join(" | "));
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(office_error(
                    path,
                    "docx",
                    format!("document.xml parse error: {error}"),
                ));
            }
            _ => {}
        }
    }
    Ok((lines.join("\n"), heading_count))
}

// ---------------------------------------------------------------------------
// XLSX
// ---------------------------------------------------------------------------

fn xlsx_shared_strings(path: &Path, xml: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().into_inner() {
                "si" => current = Some(String::new()),
                "t" => in_text = true,
                _ => {}
            },
            Ok(Event::Text(event)) => {
                if in_text {
                    if let Some(buffer) = current.as_mut() {
                        buffer.push_str(&event.xml10_content());
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if in_text {
                    if let Some(buffer) = current.as_mut() {
                        push_entity_ref(buffer, &reference);
                    }
                }
            }
            Ok(Event::End(event)) => match event.local_name().into_inner() {
                "t" => in_text = false,
                "si" => {
                    if let Some(finished) = current.take() {
                        strings.push(finished);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                tracing::warn!(path = %path.display(), "xlsx sharedStrings.xml parse stopped early: {error}");
                break;
            }
            _ => {}
        }
    }
    strings
}

/// Sheet name → worksheet part path, resolved through the workbook rels.
fn xlsx_sheets(path: &Path, workbook_xml: &str, rels_xml: &str) -> Vec<(String, String)> {
    let mut rels: HashMap<String, String> = HashMap::new();
    let mut reader = Reader::from_str(rels_xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().into_inner() == "Relationship" {
                    if let (Some(id), Some(target)) =
                        (attr_value(&event, "Id"), attr_value(&event, "Target"))
                    {
                        let normalized = if target.starts_with('/') {
                            target.trim_start_matches('/').to_string()
                        } else if target.starts_with("xl/") {
                            target
                        } else {
                            format!("xl/{target}")
                        };
                        rels.insert(id, normalized);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    let mut sheets = Vec::new();
    let mut reader = Reader::from_str(workbook_xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().into_inner() == "sheet" {
                    let name = attr_value(&event, "name");
                    let rel_id = event
                        .attributes()
                        .flatten()
                        .find(|attribute| {
                            let key = attribute.key.as_ref();
                            key == "id" || key.ends_with(":id")
                        })
                        .map(|attribute| decode_entities(&attribute.value));
                    if let (Some(name), Some(rel_id)) = (name, rel_id) {
                        if let Some(target) = rels.get(&rel_id) {
                            sheets.push((name, target.clone()));
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                tracing::warn!(path = %path.display(), "xlsx workbook.xml parse stopped early: {error}");
                break;
            }
            _ => {}
        }
    }
    sheets
}

fn xlsx_sheet_rows(xml: &str, shared: &[String]) -> Vec<String> {
    #[derive(Default)]
    struct Cell {
        kind: String,
        value: String,
        inline: String,
    }

    let mut rows: Vec<String> = Vec::new();
    let mut row: Option<Vec<String>> = None;
    let mut cell: Option<Cell> = None;
    let mut in_value = false;
    let mut in_text = false;

    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().into_inner() {
                "row" => row = Some(Vec::new()),
                "c" => {
                    let mut current = Cell::default();
                    if let Some(kind) = attr_value(&event, "t") {
                        current.kind = kind;
                    }
                    cell = Some(current);
                }
                "v" => in_value = true,
                "t" => in_text = true,
                _ => {}
            },
            Ok(Event::Text(event)) => {
                let decoded = event.xml10_content();
                if let Some(current) = cell.as_mut() {
                    if in_value {
                        current.value.push_str(&decoded);
                    } else if in_text {
                        current.inline.push_str(&decoded);
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(current) = cell.as_mut() {
                    if in_value {
                        push_entity_ref(&mut current.value, &reference);
                    } else if in_text {
                        push_entity_ref(&mut current.inline, &reference);
                    }
                }
            }
            Ok(Event::End(event)) => match event.local_name().into_inner() {
                "v" => in_value = false,
                "t" => in_text = false,
                "c" => {
                    if let (Some(current_row), Some(finished)) = (row.as_mut(), cell.take()) {
                        let text = match finished.kind.as_str() {
                            "s" => finished
                                .value
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|index| shared.get(index))
                                .cloned()
                                .unwrap_or_default(),
                            "inlineStr" => finished.inline,
                            _ => finished.value,
                        };
                        current_row.push(text.trim().to_string());
                    }
                }
                "row" => {
                    if let Some(finished) = row.take() {
                        if finished.iter().any(|c| !c.is_empty()) {
                            rows.push(finished.join(" | "));
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    rows
}

/// Extract text from a .xlsx file: one `## Sheet: name` section per sheet,
/// rows as `cell | cell` lines.
pub fn extract_xlsx(path: &Path) -> Result<String, FileIngestError> {
    let mut archive = open_archive(path, "xlsx")?;
    let shared = match read_part(&mut archive, path, "xlsx", "xl/sharedStrings.xml") {
        Ok(xml) => xlsx_shared_strings(path, &xml),
        Err(_) => Vec::new(), // shared strings are optional (inline/numbers only)
    };
    let workbook = read_part(&mut archive, path, "xlsx", "xl/workbook.xml")?;
    let rels = read_part(&mut archive, path, "xlsx", "xl/_rels/workbook.xml.rels")?;
    let sheets = xlsx_sheets(path, &workbook, &rels);
    if sheets.is_empty() {
        return Err(office_error(
            path,
            "xlsx",
            "no worksheets found in workbook",
        ));
    }

    let mut lines: Vec<String> = Vec::new();
    for (name, part) in sheets {
        let xml = match read_part(&mut archive, path, "xlsx", &part) {
            Ok(xml) => xml,
            Err(error) => {
                tracing::warn!(path = %path.display(), sheet = %part, "skipping unreadable sheet: {error}");
                continue;
            }
        };
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("## Sheet: {name}"));
        lines.extend(xlsx_sheet_rows(&xml, &shared));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_entities_handles_named_and_numeric() {
        assert_eq!(
            decode_entities("a &amp; b &lt; c &#65; &#x42;"),
            "a & b < c A B"
        );
        assert_eq!(decode_entities("plain"), "plain");
        assert_eq!(decode_entities("&unknown; kept"), "&unknown; kept");
    }

    #[test]
    fn local_name_strips_prefix() {
        assert_eq!(local_name("w:p"), "p");
        assert_eq!(local_name("p"), "p");
    }

    fn write_zip(parts: &[(&str, &str)]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let file = tempfile::NamedTempFile::new().expect("temp zip file");
        let mut writer =
            zip::ZipWriter::new(std::fs::File::create(file.path()).expect("create zip"));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in parts {
            writer.start_file(*name, options).expect("start zip entry");
            writer
                .write_all(content.as_bytes())
                .expect("write zip entry");
        }
        writer.finish().expect("finish zip");
        file
    }

    const DOCX_STYLES: &str = r#"<?xml version="1.0"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/><w:outlineLvl w:val="0"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading2">
    <w:name w:val="heading 2"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
</w:styles>"#;

    const DOCX_DOCUMENT: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
  <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>
  <w:p><w:r><w:t xml:space="preserve">Revenue grew &amp; margins held.</w:t></w:r></w:p>
  <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Details</w:t></w:r></w:p>
  <w:tbl>
    <w:tr>
      <w:tc><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc>
      <w:tc><w:p><w:r><w:t>Sales</w:t></w:r></w:p></w:tc>
    </w:tr>
    <w:tr>
      <w:tc><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc>
      <w:tc><w:p><w:r><w:t>42</w:t></w:r></w:p></w:tc>
    </w:tr>
  </w:tbl>
  <w:p><w:pPr><w:pStyle w:val="Heading3"/></w:pPr><w:r><w:t>Custom Style Heading</w:t></w:r></w:p>
</w:body>
</w:document>"#;

    #[test]
    fn extract_docx_preserves_headings_tables_and_entities() {
        let file = write_zip(&[
            ("word/styles.xml", DOCX_STYLES),
            ("word/document.xml", DOCX_DOCUMENT),
        ]);
        let (text, heading_count) = extract_docx(file.path()).expect("extract docx");
        assert!(
            text.contains("# Quarterly Report"),
            "h1 heading line: {text}"
        );
        assert!(text.contains("## Details"), "h2 heading line: {text}");
        assert!(
            text.contains("Revenue grew & margins held."),
            "entity decoding: {text}"
        );
        assert!(text.contains("Region | Sales"), "table header row: {text}");
        assert!(text.contains("North | 42"), "table body row: {text}");
        assert_eq!(heading_count, 3);
    }

    #[test]
    fn extract_docx_falls_back_to_style_id_digits() {
        let file = write_zip(&[("word/document.xml", DOCX_DOCUMENT)]);
        let (text, _) = extract_docx(file.path()).expect("extract docx without styles");
        assert!(
            text.contains("### Custom Style Heading"),
            "styleId fallback: {text}"
        );
        assert!(
            text.contains("Quarterly Report"),
            "body text still extracted: {text}"
        );
    }

    #[test]
    fn extract_docx_rejects_non_zip() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        use std::io::Write;
        file.write_all(b"not a zip").expect("write");
        assert!(extract_docx(file.path()).is_err());
    }

    const XLSX_WORKBOOK: &str = r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;

    const XLSX_RELS: &str = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#;

    const XLSX_SHARED: &str = r#"<?xml version="1.0"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <si><t>Item</t></si>
  <si><t>Cost</t></si>
  <si><t>Widget &amp; Gadget</t></si>
</sst>"#;

    const XLSX_SHEET: &str = r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
    <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>99.5</v></c></row>
  </sheetData>
</worksheet>"#;

    #[test]
    fn extract_xlsx_reads_sheets_rows_and_shared_strings() {
        let file = write_zip(&[
            ("xl/workbook.xml", XLSX_WORKBOOK),
            ("xl/_rels/workbook.xml.rels", XLSX_RELS),
            ("xl/sharedStrings.xml", XLSX_SHARED),
            ("xl/worksheets/sheet1.xml", XLSX_SHEET),
        ]);
        let text = extract_xlsx(file.path()).expect("extract xlsx");
        assert!(text.contains("## Sheet: Budget"), "sheet heading: {text}");
        assert!(text.contains("Item | Cost"), "header row: {text}");
        assert!(text.contains("Widget & Gadget | 99.5"), "data row: {text}");
    }
}
