//! Captured source-byte digests are independent from decoded-text similarity.
use super::*;
#[derive(Debug, Clone, Serialize)]
pub(super) struct Capture {
    version: u32,
    algorithm: &'static str,
    hash: String,
    bytes: u64,
    source_text_hash: String,
}
#[derive(Debug, Clone, Serialize)]
pub(super) struct Evidence {
    status: &'static str,
    capture: Option<Capture>,
}
impl Evidence {
    pub(super) fn read(meta: Option<&serde_json::Value>, text_hash: &str) -> Self {
        let unavailable = |status| Self {
            status,
            capture: None,
        };
        let Some(m) = meta else {
            return unavailable("metadata_unavailable");
        };
        if m["source_capture_version"].is_null() {
            return unavailable("legacy_or_missing_capture");
        }
        if m["source_capture_version"] != 1 {
            return unavailable("unsupported_capture_version");
        }
        if m["source_text_hash"].as_str() != Some(text_hash) {
            return unavailable("source_text_mismatch");
        }
        if m["source_bytes_hash_algorithm"] != "blake3" {
            return unavailable("unsupported_hash_algorithm");
        }
        let Some(hash) = m["source_bytes_hash"].as_str().filter(|h| {
            h.len() == 64
                && h.bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        }) else {
            return unavailable("invalid_digest");
        };
        let Some(bytes) = m["source_bytes_count"].as_u64() else {
            return unavailable("invalid_byte_count");
        };
        Self {
            status: "qualified_capture",
            capture: Some(Capture {
                version: 1,
                algorithm: "blake3",
                hash: hash.into(),
                bytes,
                source_text_hash: text_hash.into(),
            }),
        }
    }
}
pub(super) fn candidates(files: &[File]) -> Vec<family_groups::Candidate> {
    let mut groups: BTreeMap<(Option<&str>, &str, u64), Vec<usize>> = BTreeMap::new();
    for (i, file) in files.iter().enumerate() {
        if let Some(capture) = &file.source_byte_capture.capture {
            groups
                .entry((file.project.as_deref(), &capture.hash, capture.bytes))
                .or_default()
                .push(i);
        }
    }
    groups
        .into_values()
        .filter(|v| v.len() > 1)
        .map(|indices| family_groups::Candidate {
            indices,
            kind: "exact_captured_source_bytes",
            family: None,
        })
        .collect()
}
fn compare(primary: &File, member: &File) -> serde_json::Value {
    let (status, equal) = match (
        &primary.source_byte_capture.capture,
        &member.source_byte_capture.capture,
    ) {
        (Some(a), Some(b)) if a.hash == b.hash && a.bytes == b.bytes => {
            ("same_capture_digest", Some(true))
        }
        (Some(a), Some(b)) if a.hash == b.hash => ("inconsistent_digest_byte_count", None),
        (Some(_), Some(_)) => ("different_capture_digest", Some(false)),
        _ => ("unavailable", None),
    };
    serde_json::json!({"status":status,"captured_source_digest_equal":equal,"primary":primary.source_byte_capture,"member":member.source_byte_capture,"same_decoded_text":primary.content_hash==member.content_hash,"basis":"captured_digest_and_byte_count","hash_algorithm":"blake3","current_filesystem_read":false,"source_bytes_directly_compared":false,"deletion_safe":false})
}
pub(super) fn enrich(groups: &mut [serde_json::Value], files: &[File]) -> serde_json::Value {
    let by_id: HashMap<_, _> = files.iter().map(|f| (f.document_id.as_str(), f)).collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for file in files {
        *counts.entry(file.source_byte_capture.status).or_default() += 1;
    }
    let mut pairs = 0;
    let mut comparable = 0;
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
            let comparison = compare(primary, file);
            if primary.document_id != file.document_id {
                pairs += 1;
                comparable += usize::from(comparison["captured_source_digest_equal"].is_boolean());
            }
            member["source_bytes_to_primary"] = comparison;
        }
    }
    serde_json::json!({"contract_version":1,"basis":"qualified_captured_blake3_digest_and_byte_count","documents_checked":files.len(),"capture_status_counts":counts,"returned_nonprimary_pairs":pairs,"returned_comparable_pairs":comparable,"current_filesystem_read":false,"source_bytes_directly_compared":false,"metadata_grouping_included_in_primary_comparison_budget":false,"original_bytes_retained":false,"deletion_safe":false})
}
#[cfg(test)]
mod tests {
    use super::*;
    fn metadata() -> serde_json::Value {
        serde_json::json!({"source_capture_version":1,"source_bytes_hash_algorithm":"blake3","source_bytes_count":3,"source_bytes_hash":"a".repeat(64),"source_text_hash":"text"})
    }
    #[test]
    fn capture_requires_current_fingerprint_version_algorithm_digest_and_length() {
        assert_eq!(
            Evidence::read(Some(&metadata()), "text").status,
            "qualified_capture"
        );
        for (key, value, want) in [
            (
                "source_capture_version",
                serde_json::json!(2),
                "unsupported_capture_version",
            ),
            (
                "source_text_hash",
                serde_json::json!("old"),
                "source_text_mismatch",
            ),
            (
                "source_bytes_hash_algorithm",
                serde_json::json!("sha256"),
                "unsupported_hash_algorithm",
            ),
            (
                "source_bytes_hash",
                serde_json::json!("raw"),
                "invalid_digest",
            ),
            (
                "source_bytes_count",
                serde_json::json!(-1),
                "invalid_byte_count",
            ),
        ] {
            let mut m = metadata();
            m[key] = value;
            let result = Evidence::read(Some(&m), "text");
            assert_eq!(result.status, want);
            assert!(result.capture.is_none());
        }
        assert_eq!(Evidence::read(None, "text").status, "metadata_unavailable");
        assert_eq!(
            Evidence::read(Some(&serde_json::json!({})), "text").status,
            "legacy_or_missing_capture"
        );
    }
}

#[cfg(test)]
mod pair_tests {
    use super::*;
    fn file(id: &str, body: &str, hash: &str, bytes: u64, project: &str) -> File {
        let text_hash = content_hash(body);
        let meta = serde_json::json!({"source_capture_version":1,"source_bytes_hash_algorithm":"blake3","source_bytes_count":bytes,"source_bytes_hash":hash,"source_text_hash":text_hash});
        File {
            document_id: id.into(),
            path: format!("/{id}.txt"),
            project: Some(project.into()),
            revision: 1,
            content_hash: text_hash.clone(),
            source_encoding: None,
            source_encoding_basis: None,
            source_bytes_hash: Some(hash.into()),
            source_byte_capture: Evidence::read(Some(&meta), &text_hash),
            content: body.into(),
        }
    }
    #[test]
    fn byte_identity_is_independent_of_text_and_guards_inconsistent_lengths_and_projects() {
        let a = file("a", "same", &"a".repeat(64), 4, "p");
        let b = file("b", "same", &"b".repeat(64), 7, "p");
        let c = file("c", "same", &"a".repeat(64), 4, "p");
        let d = file("d", "same", &"a".repeat(64), 4, "q");
        let bad = file("bad", "same", &"a".repeat(64), 9, "p");
        assert_eq!(compare(&a, &b)["captured_source_digest_equal"], false);
        assert_eq!(compare(&a, &b)["same_decoded_text"], true);
        assert_eq!(compare(&a, &c)["captured_source_digest_equal"], true);
        assert!(compare(&a, &bad)["captured_source_digest_equal"].is_null());
        let groups = candidates(&[a, b, c, d, bad]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].indices, vec![0, 2]);
    }
}
