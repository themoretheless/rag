//! Bounded normalized-line metrics, separate from grouping and never body output.
use super::*;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct Options {
    /// Defaults true; this phase runs only for returned groups/members.
    pub enabled: Option<bool>,
    /// Non-primary member pairs (default 1000, clamped 1..100000).
    pub max_pairs: Option<usize>,
    /// Unique decoded input bytes charged (default 16 MiB, maximum 128 MiB).
    pub max_total_bytes: Option<usize>,
    /// Per-file decoded bytes (default 2 MiB, maximum 8 MiB).
    pub max_file_bytes: Option<usize>,
    /// Logical lines per file (default 50000, maximum 500000).
    pub max_lines: Option<usize>,
    /// Unique normalized lines per file (default 20000, maximum 200000).
    pub max_unique_lines: Option<usize>,
    /// Raw bytes per logical line (default 64 KiB, maximum 1 MiB).
    pub max_line_bytes: Option<usize>,
}
struct Limits {
    pairs: usize,
    total_bytes: usize,
    file_bytes: usize,
    lines: usize,
    unique: usize,
    line_bytes: usize,
}
impl Limits {
    fn new(options: &Options) -> Self {
        Self {
            pairs: options.max_pairs.unwrap_or(1000).clamp(1, 100000),
            total_bytes: options
                .max_total_bytes
                .unwrap_or(16 * 1024 * 1024)
                .clamp(1, 128 * 1024 * 1024),
            file_bytes: options
                .max_file_bytes
                .unwrap_or(2 * 1024 * 1024)
                .clamp(1, 8 * 1024 * 1024),
            lines: options.max_lines.unwrap_or(50000).clamp(1, 500000),
            unique: options.max_unique_lines.unwrap_or(20000).clamp(1, 200000),
            line_bytes: options
                .max_line_bytes
                .unwrap_or(64 * 1024)
                .clamp(1, 1024 * 1024),
        }
    }
}
struct Normalized<'a> {
    lines: BTreeSet<&'a str>,
    logical_lines: usize,
    ignored_empty: usize,
}
fn separator(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}
fn normalize<'a>(
    source: &'a str,
    limits: &Limits,
) -> std::result::Result<Normalized<'a>, &'static str> {
    let mut result = Normalized {
        lines: BTreeSet::new(),
        logical_lines: 0,
        ignored_empty: 0,
    };
    let mut line = |raw: &'a str| -> std::result::Result<(), &'static str> {
        result.logical_lines += 1;
        if result.logical_lines > limits.lines {
            return Err("line_limit");
        }
        if raw.len() > limits.line_bytes {
            return Err("line_byte_limit");
        }
        // Python strip additionally treats the four information separators as whitespace.
        let trimmed =
            raw.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
        if trimmed.is_empty() {
            result.ignored_empty += 1;
        } else {
            result.lines.insert(trimmed);
        }
        if result.lines.len() > limits.unique {
            return Err("unique_line_limit");
        }
        Ok(())
    };
    let mut chars = source.char_indices().peekable();
    let mut start = 0;
    while let Some((offset, c)) = chars.next() {
        if separator(c) {
            line(&source[start..offset])?;
            start = offset + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                let (offset, c) = chars.next().unwrap();
                start = offset + c.len_utf8();
            }
        }
    }
    if start < source.len() {
        line(&source[start..])?;
    }
    Ok(result)
}
fn rounded_similarity(value: f64) -> f64 {
    // Format the actual binary float at decimal precision; multiplying first changes ties.
    format!("{value:.3}")
        .parse()
        .expect("finite Jaccard fraction")
}
fn evidence(file: &File) -> serde_json::Value {
    serde_json::json!({"document_id":file.document_id,"path":file.path,"project":file.project,"revision":file.revision,"content_hash":file.content_hash})
}
fn unavailable(reason: &str, primary: &File, member: &File) -> serde_json::Value {
    serde_json::json!({"status":"not_checked","reason":reason,"primary":evidence(primary),"member":evidence(member),"complete":false,"body_text_included":false})
}
fn ensure<'a>(
    file: &'a File,
    cache: &mut HashMap<&'a str, std::result::Result<Normalized<'a>, &'static str>>,
    limits: &Limits,
    charged: &mut usize,
) {
    if cache.contains_key(file.document_id.as_str()) {
        return;
    }
    let result = if file.content.len() > limits.file_bytes {
        Err("file_byte_limit")
    } else if file.content.len() > limits.total_bytes.saturating_sub(*charged) {
        Err("total_byte_budget")
    } else {
        *charged += file.content.len();
        normalize(&file.content, limits)
    };
    cache.insert(&file.document_id, result);
}
pub(super) fn enrich(
    groups: &mut [serde_json::Value],
    files: &[File],
    options: Option<&Options>,
) -> serde_json::Value {
    let default = Options::default();
    let options = options.unwrap_or(&default);
    let enabled = options.enabled != Some(false);
    let limits = Limits::new(options);
    let by_id: HashMap<_, _> = files.iter().map(|f| (f.document_id.as_str(), f)).collect();
    let mut cache = HashMap::new();
    let mut charged = 0;
    let mut requested = 0;
    let mut attempted = 0;
    let mut checked = 0;
    let mut unchecked = 0;
    let mut self_members = 0;
    for group in groups {
        let Some(primary) = group["primary_candidate"]
            .as_str()
            .and_then(|id| by_id.get(id))
            .copied()
        else {
            continue;
        };
        for member in group["members"].as_array_mut().into_iter().flatten() {
            let Some(file) = member["document"]["document_id"]
                .as_str()
                .and_then(|id| by_id.get(id))
                .copied()
            else {
                continue;
            };
            let is_self = file.document_id == primary.document_id;
            if !is_self {
                requested += 1;
            } else {
                self_members += 1;
            }
            let comparison = if !enabled {
                unavailable("disabled", primary, file)
            } else if is_self {
                serde_json::json!({"status":"self","complete":true,"primary":evidence(primary),"member":evidence(file),"similarity":1.0,"lines_only_here":0,"lines_only_in_primary":0,"exact_stored_text_equal":true,"unique_line_counts_measured":false,"body_text_included":false})
            } else if attempted >= limits.pairs {
                unchecked += 1;
                unavailable("pair_budget", primary, file)
            } else {
                attempted += 1;
                ensure(primary, &mut cache, &limits, &mut charged);
                ensure(file, &mut cache, &limits, &mut charged);
                match (
                    &cache[primary.document_id.as_str()],
                    &cache[file.document_id.as_str()],
                ) {
                    (Ok(left), Ok(right)) => {
                        checked += 1;
                        let common = left.lines.intersection(&right.lines).count();
                        let only_primary = left.lines.len() - common;
                        let only_here = right.lines.len() - common;
                        let union = common + only_primary + only_here;
                        let similarity = if union == 0 {
                            1.0
                        } else {
                            common as f64 / union as f64
                        };
                        serde_json::json!({"status":"checked","complete":true,"primary":evidence(primary),"member":evidence(file),"similarity":rounded_similarity(similarity),"jaccard_unrounded":similarity,"lines_only_here":only_here,"lines_only_in_primary":only_primary,"shared_unique_lines":common,"union_unique_lines":union,"primary_unique_lines":left.lines.len(),"member_unique_lines":right.lines.len(),"primary_logical_lines":left.logical_lines,"member_logical_lines":right.logical_lines,"primary_ignored_empty_lines":left.ignored_empty,"member_ignored_empty_lines":right.ignored_empty,"exact_stored_text_equal":primary.content==file.content,"unique_line_counts_measured":true,"body_text_included":false})
                    }
                    (Err(reason), _) | (_, Err(reason)) => {
                        unchecked += 1;
                        unavailable(reason, primary, file)
                    }
                }
            };
            member["similarity_to_primary"] = comparison["similarity"].clone();
            member["lines_only_here"] = comparison["lines_only_here"].clone();
            member["lines_only_in_primary"] = comparison["lines_only_in_primary"].clone();
            member["primary_comparison"] = comparison;
        }
    }
    serde_json::json!({"contract_version":1,"status":if enabled{"evaluated"}else{"disabled"},"scope":"returned_groups_and_members","metric":"trimmed_nonempty_unique_line_jaccard","grouping_metric":false,"order_and_repetition_ignored":true,"whitespace_trimmed":true,"empty_lines_ignored":true,"line_breaks":"python_splitlines_compatible","similarity_rounding_decimal_places":3,"self_members":self_members,"pairs_requested":requested,"pairs_attempted":attempted,"pairs_checked":checked,"pairs_not_checked":if enabled{unchecked}else{requested},"complete":enabled && unchecked==0,"unique_files_admitted":cache.len(),"unique_files_normalized":cache.values().filter(|r|r.is_ok()).count(),"decoded_input_bytes_charged":charged,"body_text_included":false,"original_binary_equality_proven":false,"deletion_safe":false,"budgets":{"max_pairs":limits.pairs,"max_decoded_input_bytes":limits.total_bytes,"max_file_bytes":limits.file_bytes,"max_logical_lines":limits.lines,"max_unique_lines":limits.unique,"max_line_bytes":limits.line_bytes,"corpus_loading_grouping_and_role_work_included":false}})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(id: &str, body: &str) -> File {
        File {
            document_id: id.into(),
            path: format!("/p/{id}.txt"),
            project: Some("p".into()),
            revision: 7,
            content_hash: format!("hash-{id}"),
            source_encoding: None,
            source_encoding_basis: None,
            source_bytes_hash: None,
            source_byte_capture: source_bytes::Evidence::read(None, ""),
            content: body.into(),
        }
    }
    fn run(bodies: &[&str], options: Options) -> (serde_json::Value, serde_json::Value) {
        let files: Vec<_> = bodies
            .iter()
            .enumerate()
            .map(|(i, b)| file(&i.to_string(), b))
            .collect();
        let mut groups = vec![
            serde_json::json!({"primary_candidate":"0","members":files.iter().map(|f|serde_json::json!({"document":{"document_id":f.document_id}})).collect::<Vec<_>>()}),
        ];
        let summary = enrich(&mut groups, &files, Some(&options));
        (groups.remove(0), summary)
    }
    #[test]
    fn python_lines_unicode_crlf_and_trim() {
        let n=normalize("\u{1f}a\u{1f}\r\na\rb\u{b}c\u{c}d\u{1c}e\u{1d}f\u{1e}g\u{85}h\u{2028}i\u{2029}\u{2003}",&Limits::new(&Options::default())).unwrap();
        assert_eq!(
            n.lines,
            BTreeSet::from(["a", "b", "c", "d", "e", "f", "g", "h", "i"])
        );
        assert_eq!(n.logical_lines, 11);
        assert_eq!(n.ignored_empty, 1);
    }
    #[test]
    fn directional_deltas_rounding_and_body_free_provenance() {
        let (g, s) = run(&["a\nb\n", " b\nc\nc\n\n"], Options::default());
        let m = &g["members"][1];
        assert_eq!(m["similarity_to_primary"], 0.333);
        assert_eq!(m["lines_only_here"], 1);
        assert_eq!(m["lines_only_in_primary"], 1);
        assert_eq!(m["primary_comparison"]["member_logical_lines"], 4);
        assert_eq!(m["primary_comparison"]["member"]["revision"], 7);
        assert_eq!(s["pairs_checked"], 1);
        assert_eq!(s["complete"], true);
        assert!(!g.to_string().contains(" b\\nc"));
        let (g, _) = run(&["a\na\nb", " b \na\n"], Options::default());
        assert_eq!(g["members"][1]["similarity_to_primary"], 1.0);
        assert_eq!(
            g["members"][1]["primary_comparison"]["exact_stored_text_equal"],
            false
        );
        let (g, _) = run(&["a\nb\nc", "a"], Options::default());
        assert_eq!(g["members"][1]["lines_only_here"], 0);
        assert_eq!(g["members"][1]["lines_only_in_primary"], 2);
    }
    #[test]
    fn rounding_matches_binary_float_decimal_ties() {
        assert_eq!(rounded_similarity(1.0 / 16.0), 0.062);
        assert_eq!(rounded_similarity(1.0 / 80.0), 0.013);
        assert_eq!(rounded_similarity(9.0 / 16.0), 0.562);
    }
    #[test]
    fn empty_and_disjoint_sets() {
        for (a, b, want) in [("", " \n", 1.0), ("", "a", 0.0), ("a", "b", 0.0)] {
            let (g, _) = run(&[a, b], Options::default());
            assert_eq!(g["members"][1]["similarity_to_primary"], want);
        }
    }
    #[test]
    fn limits_publish_null_metrics_and_no_partial_counts() {
        for (options, reason) in [
            (
                Options {
                    max_file_bytes: Some(1),
                    ..Default::default()
                },
                "file_byte_limit",
            ),
            (
                Options {
                    max_total_bytes: Some(1),
                    ..Default::default()
                },
                "total_byte_budget",
            ),
            (
                Options {
                    max_lines: Some(1),
                    ..Default::default()
                },
                "line_limit",
            ),
            (
                Options {
                    max_unique_lines: Some(1),
                    ..Default::default()
                },
                "unique_line_limit",
            ),
            (
                Options {
                    max_line_bytes: Some(1),
                    ..Default::default()
                },
                "line_byte_limit",
            ),
            (
                Options {
                    enabled: Some(false),
                    ..Default::default()
                },
                "disabled",
            ),
        ] {
            let (g, s) = run(&["aa\nb", "aa\nc"], options);
            let m = &g["members"][1];
            assert!(m["similarity_to_primary"].is_null());
            assert!(m["lines_only_here"].is_null());
            assert_eq!(m["primary_comparison"]["reason"], reason);
            assert!(m["primary_comparison"]["shared_unique_lines"].is_null());
            assert_eq!(s["complete"], false);
        }
    }
    #[test]
    fn pair_budget_and_unique_file_cache() {
        let (g, s) = run(
            &["a", "b", "c"],
            Options {
                max_pairs: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(s["decoded_input_bytes_charged"], 2);
        assert_eq!(s["pairs_requested"], 2);
        assert_eq!(
            g["members"][2]["primary_comparison"]["reason"],
            "pair_budget"
        );
        let (_, s) = run(&["a", "b", "c"], Options::default());
        assert_eq!(s["decoded_input_bytes_charged"], 3);
        assert_eq!(s["unique_files_normalized"], 3);
        let (g, s) = run(
            &["arbitrarily long self"],
            Options {
                max_file_bytes: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(g["members"][0]["primary_comparison"]["status"], "self");
        assert_eq!(s["pairs_requested"], 0);
        assert_eq!(s["decoded_input_bytes_charged"], 0);
    }
}
