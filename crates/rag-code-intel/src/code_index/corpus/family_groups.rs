//! Independent exact-text partitions and directory-sensitive numbered-name families.
use super::*;

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    SimilarityComponents,
    ExactAndNameFamilies,
    SourceByteCopies,
}
pub(super) struct Candidate {
    pub indices: Vec<usize>,
    pub kind: &'static str,
    pub family: Option<String>,
}
fn family_key(path: &str, suffix: &regex::Regex) -> String {
    let path = path.replace('\\', "/");
    let mut path = path.trim();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path = path.trim_start_matches('/');
    let (head, tail) = path.rsplit_once('/').map_or(("", path), |(h, t)| (h, t));
    let (stem, ext) = tail.rsplit_once('.').map_or((tail, ""), |(s, e)| (s, e));
    let stem = suffix.replace(stem, "");
    let tail = if ext.is_empty() {
        stem.into_owned()
    } else {
        format!("{stem}.{ext}")
    };
    if head.is_empty() {
        tail
    } else {
        format!("{head}/{tail}")
    }
}
pub(super) fn candidates(files: &[File]) -> Result<Vec<Candidate>> {
    let suffix = regex::Regex::new(r"[_\-]?\d+$").map_err(|e| AppError::config(e.to_string()))?;
    let mut exact: BTreeMap<(Option<&str>, &str), Vec<usize>> = BTreeMap::new();
    let mut families: BTreeMap<(Option<&str>, String), Vec<usize>> = BTreeMap::new();
    for (i, file) in files.iter().enumerate() {
        exact
            .entry((file.project.as_deref(), &file.content_hash))
            .or_default()
            .push(i);
        families
            .entry((file.project.as_deref(), family_key(&file.path, &suffix)))
            .or_default()
            .push(i);
    }
    let mut output = Vec::new();
    let mut exact_sets = BTreeSet::new();
    for indices in exact.into_values().filter(|v| v.len() > 1) {
        exact_sets.insert(indices.clone());
        output.push(Candidate {
            indices,
            kind: "exact_stored_text",
            family: None,
        });
    }
    for ((_, family), indices) in families.into_iter().filter(|(_, v)| v.len() > 1) {
        // Keep proper exact subgroups; suppress only an identical member set.
        if !exact_sets.contains(&indices) {
            output.push(Candidate {
                indices,
                kind: "name_family",
                family: Some(family),
            });
        }
    }
    Ok(output)
}
pub(super) fn classify(groups: &mut [serde_json::Value], threshold: f64) {
    for group in groups {
        if group["kind"] != "name_family" {
            continue;
        }
        let members = group["members"].as_array().expect("generated member array");
        let checked: Vec<_> = members
            .iter()
            .filter_map(|m| {
                let c = &m["primary_comparison"];
                (c["status"] == "checked")
                    .then(|| c["jaccard_unrounded"].as_f64())
                    .flatten()
            })
            .collect();
        let best = checked.iter().copied().reduce(f64::max);
        let complete = group["members_truncated"] == false && checked.len() + 1 == members.len();
        let related = best.is_some_and(|b| b >= threshold);
        let classification = if related {
            "family"
        } else if complete {
            "same_name"
        } else {
            "not_checked"
        };
        group["kind"] = serde_json::json!(classification);
        group["family_classification"] = serde_json::json!({"status":classification,"complete":complete,"metric":"trimmed_nonempty_unique_line_jaccard","threshold":threshold,"threshold_uses_unrounded_fraction":true,"best_checked_similarity":best,"checked_nonprimary_members":checked.len(),"scope":"selected_primary_against_returned_members","same_name_requires_all_members_checked":true,"similarity_is_semantic_equivalence":false,"deletion_safe":false});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_numbered_path_keys_preserve_directory_case_and_nonnumber_suffixes() {
        let suffix = regex::Regex::new(r"[_\-]?\d+$").unwrap();
        for (path, want) in [
            ("php/Wizard/Files7.php", "php/Wizard/Files.php"),
            ("./cgi-bin/import_12.pl", "cgi-bin/import.pl"),
            ("/cgi-bin/import-٢.pl", "cgi-bin/import.pl"),
            ("cgi-bin/import_stdin.pl", "cgi-bin/import_stdin.pl"),
            ("a/Files_old.php", "a/Files_old.php"),
            ("a/Files2.PHP", "a/Files.PHP"),
            ("a/Files.php", "a/Files.php"),
            ("b/Files.php", "b/Files.php"),
        ] {
            assert_eq!(family_key(path, &suffix), want);
        }
    }
    #[test]
    fn incomplete_never_claims_dissimilar_family() {
        let mut groups = vec![
            serde_json::json!({"kind":"name_family","members_truncated":true,"members":[{"primary_comparison":{"status":"self"}},{"primary_comparison":{"status":"checked","jaccard_unrounded":0.1}}]}),
        ];
        classify(&mut groups, 0.5);
        assert_eq!(groups[0]["kind"], "not_checked");
        assert_eq!(groups[0]["family_classification"]["complete"], false);
        groups[0]["kind"] = serde_json::json!("name_family");
        groups[0]["members_truncated"] = serde_json::json!(false);
        classify(&mut groups, 0.5);
        assert_eq!(groups[0]["kind"], "same_name");
        groups[0]["kind"] = serde_json::json!("name_family");
        groups[0]["members"][1]["primary_comparison"]["jaccard_unrounded"] =
            serde_json::json!(0.4999);
        classify(&mut groups, 0.5);
        assert_eq!(groups[0]["kind"], "same_name");
    }
}
