//! Reproducible confidence-threshold reports. Never silently applies learned thresholds.
use crate::{
    db::Store,
    error::{AppError, Result},
    util::content_hash,
};
use duckdb::params;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
pub(crate) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS analysis_calibrations (
 seq BIGINT PRIMARY KEY, id VARCHAR UNIQUE NOT NULL, created_ms BIGINT NOT NULL,
 kind VARCHAR NOT NULL, sample_size BIGINT NOT NULL, index_rev BIGINT, report_json VARCHAR NOT NULL
);
"#;
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: String,
    pub score: f64,
    pub relevant: bool,
}
#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Samples {
    #[serde(default)]
    pub training: Vec<Sample>,
    #[serde(default)]
    pub validation: Vec<Sample>,
    pub dataset_name: Option<String>,
    /// Assertion only when no labels are supplied; never treated as measured evidence.
    pub sample_size: Option<usize>,
    pub params: Option<Value>,
    /// Reference flat parameter maps remain unvalidated assertions.
    #[serde(flatten)]
    pub parameter_assertions: BTreeMap<String, Value>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CalibrationQuery {
    pub kind: Option<String>,
    pub samples: Option<Samples>,
    /// Training precision floor (0..1); default 0.9. Selection maximizes F1, then threshold.
    pub min_precision: Option<f64>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ReportsQuery {
    pub kind: Option<String>,
    pub before_seq: Option<i64>,
    pub limit: Option<usize>,
}
#[derive(Debug, Serialize)]
struct Metrics {
    tp: usize,
    fp: usize,
    tn: usize,
    #[serde(rename = "fn")]
    fn_: usize,
    predicted_positive: usize,
    precision: Option<f64>,
    recall: Option<f64>,
    f1: f64,
    precision_wilson_95_lower: Option<f64>,
}
fn metrics(rows: &[Sample], threshold: Option<f64>) -> Metrics {
    let (mut tp, mut fp, mut tn, mut fn_) = (0, 0, 0, 0);
    for r in rows {
        match (threshold.is_some_and(|t| r.score >= t), r.relevant) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fn_ += 1,
            (false, false) => tn += 1,
        }
    }
    let predicted = tp + fp;
    let precision = (predicted > 0).then(|| tp as f64 / predicted as f64);
    let recall = (tp + fn_ > 0).then(|| tp as f64 / (tp + fn_) as f64);
    let f1 = if 2 * tp + fp + fn_ > 0 {
        2.0 * tp as f64 / (2 * tp + fp + fn_) as f64
    } else {
        0.0
    };
    let lower = precision.map(|p| {
        let n = predicted as f64;
        let z = 1.959963984540054;
        let a = z * z;
        ((p + a / (2.0 * n) - z * ((p * (1.0 - p) + a / (4.0 * n)) / n).sqrt()) / (1.0 + a / n))
            .clamp(0.0, 1.0)
    });
    Metrics {
        tp,
        fp,
        tn,
        fn_,
        predicted_positive: predicted,
        precision,
        recall,
        f1,
        precision_wilson_95_lower: lower,
    }
}
fn validate(q: &CalibrationQuery) -> Result<(String, f64, Samples)> {
    let kind = q.kind.as_deref().unwrap_or("confidence").trim().to_string();
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(AppError::config(
            "kind must be 1..64 ASCII letters/digits/_/-",
        ));
    }
    let min = q.min_precision.unwrap_or(0.9);
    if !min.is_finite() || !(0.0..=1.0).contains(&min) {
        return Err(AppError::config("min_precision must be finite and 0..1"));
    }
    // A typed clone keeps canonical hashing independent from incoming object-key order.
    let mut samples: Samples = q
        .samples
        .as_ref()
        .map(|s| serde_json::from_value(serde_json::to_value(s)?))
        .transpose()?
        .unwrap_or_default();
    if samples.training.len() + samples.validation.len() > 2048 {
        return Err(AppError::config("at most 2048 labelled samples"));
    }
    if samples
        .dataset_name
        .as_ref()
        .is_some_and(|s| s.chars().count() > 200 || s.contains('\0'))
    {
        return Err(AppError::config("dataset_name exceeds limit"));
    }
    if !samples.parameter_assertions.is_empty() {
        if !samples.training.is_empty() || !samples.validation.is_empty() {
            return Err(AppError::config(
                "unknown keys in labelled calibration samples",
            ));
        }
        let mut params = samples.params.take().unwrap_or_else(|| json!({}));
        let object = params
            .as_object_mut()
            .ok_or_else(|| AppError::config("params must be an object"))?;
        for (key, value) in std::mem::take(&mut samples.parameter_assertions) {
            if object.insert(key, value).is_some() {
                return Err(AppError::config(
                    "duplicate flat/nested calibration parameter",
                ));
            }
        }
        samples.params = Some(params);
    }
    if samples.params.as_ref().is_some_and(|p| !p.is_object()) {
        return Err(AppError::config("params must be an object"));
    }
    if serde_json::to_vec(&samples.params)?.len() > 65536 {
        return Err(AppError::config("calibration params exceed 64 KiB"));
    }
    if !samples.training.is_empty() && kind != "confidence" {
        return Err(AppError::config(
            "labelled threshold fitting currently requires kind=confidence",
        ));
    }
    if !samples.validation.is_empty() && samples.training.is_empty() {
        return Err(AppError::config("validation requires training samples"));
    }
    if !samples.training.is_empty()
        && samples
            .sample_size
            .is_some_and(|n| n != samples.training.len() + samples.validation.len())
    {
        return Err(AppError::config(
            "declared sample_size does not match labelled rows",
        ));
    }
    let mut seen = HashSet::new();
    for r in samples.training.iter().chain(&samples.validation) {
        if r.id.trim().is_empty()
            || r.id.chars().count() > 200
            || r.id.contains('\0')
            || !seen.insert(r.id.trim().to_string())
        {
            return Err(AppError::config(
                "sample IDs must be nonempty, unique across both splits and at most 200 characters",
            ));
        }
        if !r.score.is_finite() || !(0.0..=1.0).contains(&r.score) {
            return Err(AppError::config(
                "confidence scores must be finite and 0..1",
            ));
        }
    }
    Ok((kind, min, samples))
}
fn fit(kind: &str, min: f64, samples: &Samples) -> Value {
    let fingerprint = content_hash(&serde_json::to_string(samples).unwrap());
    if samples.training.is_empty() {
        return json!({"status":"recorded_unvalidated","kind":kind,"params":samples.params,"declared_sample_size":samples.sample_size,"measured_sample_size":0,"dataset_name":samples.dataset_name,"dataset_fingerprint":fingerprint,"selected_threshold":null,"training_metrics":null,"validation_metrics":null,"applied":false,"production_qualified":false,"notice":"No labels supplied. Parameters and sample-size assertions are not empirical calibration."});
    }
    let pos = samples.training.iter().filter(|r| r.relevant).count();
    let both = pos > 0 && pos < samples.training.len();
    let mut thresholds: Vec<_> = samples.training.iter().map(|r| r.score).collect();
    thresholds.sort_by(f64::total_cmp);
    thresholds.dedup();
    let mut best: Option<(f64, Metrics)> = None;
    if both {
        for t in thresholds {
            let m = metrics(&samples.training, Some(t));
            if m.precision.is_some_and(|p| p + 1e-12 >= min)
                && best.as_ref().is_none_or(|(old, prior)| {
                    m.f1 > prior.f1 + 1e-12 || ((m.f1 - prior.f1).abs() <= 1e-12 && t > *old)
                })
            {
                best = Some((t, m));
            }
        }
    }
    let threshold = best.as_ref().map(|b| b.0);
    let train = best
        .map(|b| b.1)
        .unwrap_or_else(|| metrics(&samples.training, None));
    let validation =
        (!samples.validation.is_empty()).then(|| metrics(&samples.validation, threshold));
    let target_met = validation
        .as_ref()
        .and_then(|m| m.precision)
        .is_some_and(|p| p + 1e-12 >= min);
    let lower_met = validation
        .as_ref()
        .and_then(|m| m.precision_wilson_95_lower)
        .is_some_and(|p| p >= min);
    let validation_pos = samples.validation.iter().filter(|r| r.relevant).count();
    let validation_both = validation_pos > 0 && validation_pos < samples.validation.len();
    let status = if !both {
        "training_requires_both_classes"
    } else if threshold.is_none() {
        "no_threshold_meets_target"
    } else if validation.is_none() {
        "training_only_unvalidated"
    } else if !validation_both {
        "validation_requires_both_classes"
    } else if !target_met {
        "validation_target_not_met"
    } else if samples.training.len() < 20 || samples.validation.len() < 20 {
        "exploratory_small_sample"
    } else {
        "evaluated_on_declared_holdout"
    };
    json!({"status":status,"kind":kind,"dataset_name":samples.dataset_name,"dataset_fingerprint":fingerprint,"score_domain":"caller_supplied_0_to_1_confidence","acceptance_rule":"score_greater_than_or_equal_to_threshold","selection_objective":"maximize_training_F1_subject_to_precision_floor_then_larger_threshold","min_precision":min,"selected_threshold":threshold,"training_size":samples.training.len(),"validation_size":samples.validation.len(),"class_counts":{"training_positive":pos,"training_negative":samples.training.len()-pos,"validation_positive":validation_pos,"validation_negative":samples.validation.len()-validation_pos},"measured_sample_size":samples.training.len()+samples.validation.len(),"training_metrics":train,"validation_metrics":validation,"validation_point_target_met":target_met,"validation_precision_lower_bound_meets_target":lower_met,"params":samples.params,"applied":false,"production_qualified":false,"not_checked":["label_accuracy","representativeness","score_semantics","hidden_cross_split_duplicates","future_distribution_shift"],"notice":"A held-out report is not a deployment decision. No runtime thresholds were changed."})
}
impl Store {
    pub fn calibrate_confidence(&self, q: &CalibrationQuery) -> Result<Value> {
        let (kind, min, samples) = validate(q)?;
        let mut report = fit(&kind, min, &samples);
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM analysis_calibrations",
            [],
            |r| r.get(0),
        )?;
        let rev: Option<i64> =
            tx.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?;
        let id = uuid::Uuid::new_v4().to_string();
        let at = chrono::Utc::now().timestamp_millis();
        let o = report.as_object_mut().unwrap();
        o.insert("id".into(), json!(id));
        o.insert("seq".into(), json!(seq));
        o.insert("created_ms".into(), json!(at));
        o.insert("index_rev".into(), json!(rev));
        tx.execute(
            "INSERT INTO analysis_calibrations VALUES (?,?,?,?,?,?,?)",
            params![
                seq,
                id,
                at,
                kind,
                (samples.training.len() + samples.validation.len()) as i64,
                rev,
                serde_json::to_string(&report)?
            ],
        )?;
        tx.commit()?;
        Ok(report)
    }
    pub fn calibration_reports(&self, q: &ReportsQuery) -> Result<Value> {
        let limit = q.limit.unwrap_or(20).clamp(1, 200);
        let conn = self.lock()?;
        let mut stmt=conn.prepare("SELECT report_json FROM analysis_calibrations WHERE (? IS NULL OR kind=?) AND (? IS NULL OR seq<?) ORDER BY seq DESC LIMIT ?")?;
        let rows = stmt.query_map(
            params![q.kind, q.kind, q.before_seq, q.before_seq, limit as i64 + 1],
            |r| r.get::<_, String>(0),
        )?;
        let mut items = vec![];
        let mut bytes = 0usize;
        let mut more = false;
        for row in rows {
            let row = row?;
            if items.len() >= limit || bytes + row.len() > 256 * 1024 {
                more = true;
                break;
            }
            bytes += row.len();
            items.push(serde_json::from_str::<Value>(&row)?);
        }
        let next = if more {
            items.last().and_then(|v| v["seq"].as_i64())
        } else {
            None
        };
        Ok(
            json!({"items":items,"next_before_seq":next,"truncated":more,"applied":false,"ordering":"seq_desc","source":"calibration_report","report_bytes":bytes,"response_report_budget_bytes":256*1024}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample(id: &str, score: f64, relevant: bool) -> Sample {
        Sample {
            id: id.into(),
            score,
            relevant,
        }
    }
    #[test]
    fn fitting_groups_ties_and_holdout_failure_is_explicit() {
        let s = Samples {
            training: vec![
                sample("a", 0.9, true),
                sample("b", 0.9, false),
                sample("c", 0.8, true),
                sample("d", 0.1, false),
            ],
            validation: vec![sample("v1", 0.95, false), sample("v2", 0.8, true)],
            ..Default::default()
        };
        let r = fit("confidence", 0.6, &s);
        assert_eq!(r["selected_threshold"], 0.8, "{r}");
        assert_eq!(r["training_metrics"]["tp"], 2);
        assert_eq!(r["training_metrics"]["fp"], 1);
        assert_eq!(r["validation_metrics"]["tp"], 1);
        assert_eq!(r["validation_metrics"]["fp"], 1);
        assert_eq!(r["status"], "validation_target_not_met");
        assert_eq!(r["applied"], false);
        let r = fit("confidence", 0.9, &s);
        assert!(r["selected_threshold"].is_null());
        assert_eq!(r["status"], "no_threshold_meets_target");
        assert!(r["training_metrics"]["precision"].is_null());
    }
    #[test]
    fn small_perfect_samples_do_not_claim_production_quality_or_replace_holdout() {
        let s = Samples {
            training: vec![sample("a", 0.9, true), sample("b", 0.1, false)],
            validation: vec![sample("c", 0.95, true), sample("d", 0.05, false)],
            ..Default::default()
        };
        let edge = metrics(&[sample("zero", 1.0, false)], Some(0.5));
        assert_eq!(edge.precision_wilson_95_lower, Some(0.0));
        let r = fit("confidence", 0.9, &s);
        assert_eq!(r["status"], "exploratory_small_sample");
        assert_eq!(r["validation_point_target_met"], true);
        assert_eq!(r["validation_precision_lower_bound_meets_target"], false);
        assert_eq!(r["production_qualified"], false);
        let s = Samples {
            training: s.training,
            ..Default::default()
        };
        assert_eq!(
            fit("confidence", 0.9, &s)["status"],
            "training_only_unvalidated"
        );
    }
    #[test]
    fn assertions_are_not_labels_and_reports_survive_restart_with_stable_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.duckdb");
        let store = Store::open(&path).unwrap();
        let q = CalibrationQuery {
            samples: Some(Samples {
                sample_size: Some(1000),
                params: Some(json!({"threshold":0.5})),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = store.calibrate_confidence(&q).unwrap();
        assert_eq!(r["measured_sample_size"], 0);
        assert_eq!(r["status"], "recorded_unvalidated");
        let legacy: CalibrationQuery =
            serde_json::from_value(json!({"samples":{"sample_size":25,"threshold":0.6}})).unwrap();
        let (kind, min, normalized) = validate(&legacy).unwrap();
        let legacy = fit(&kind, min, &normalized);
        assert_eq!(legacy["params"]["threshold"], 0.6);
        assert_eq!(legacy["measured_sample_size"], 0);

        store.calibrate_confidence(&q).unwrap();
        let page = store
            .calibration_reports(&ReportsQuery {
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        let cursor = page["next_before_seq"].as_i64().unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let older = store
            .calibration_reports(&ReportsQuery {
                before_seq: Some(cursor),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(older["items"][0]["id"], r["id"]);
        assert!(store.list_documents().unwrap().is_empty());
    }
    #[test]
    fn invalid_scores_duplicate_ids_and_cross_split_leakage_are_rejected() {
        let q = CalibrationQuery {
            samples: Some(Samples {
                training: vec![sample("a", 0.9, true)],
                validation: vec![sample("a", 0.1, false)],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(validate(&q).is_err());
        let q = CalibrationQuery {
            samples: Some(Samples {
                training: vec![sample("a", f64::NAN, true)],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(validate(&q).is_err());
        let q = CalibrationQuery {
            min_precision: Some(2.0),
            ..Default::default()
        };
        assert!(validate(&q).is_err());
    }
}
