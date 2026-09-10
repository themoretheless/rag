//! Pairwise A/B comparison and regression list over per-query metrics.

use serde::{Deserialize, Serialize};

use super::labels::ErrorLabel;
use super::settings::SearchSettingsProfile;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuerySideMetrics {
    pub recall_at_k: f64,
    pub reciprocal_rank: f64,
    pub ndcg_at_k: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegressionEntry {
    pub query_id: String,
    pub metric: String,
    pub side_a: f64,
    pub side_b: f64,
    pub delta: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_label: Option<ErrorLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompareReport {
    pub dataset_content_hash: String,
    pub settings_a: SearchSettingsProfile,
    pub settings_b: SearchSettingsProfile,
    pub side_a_name: String,
    pub side_b_name: String,
    pub macro_a: QuerySideMetrics,
    pub macro_b: QuerySideMetrics,
    pub regressions: Vec<RegressionEntry>,
    pub improvements: Vec<RegressionEntry>,
    pub queries: Vec<ComparedQuery>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComparedQuery {
    pub query_id: String,
    pub side_a: QuerySideMetrics,
    pub side_b: QuerySideMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_label: Option<ErrorLabel>,
}

/// Build a compare report. `epsilon` ignores tiny floating noise when listing regressions.
pub fn compare_query_metrics(
    dataset_content_hash: String,
    settings_a: SearchSettingsProfile,
    settings_b: SearchSettingsProfile,
    pairs: Vec<(String, QuerySideMetrics, QuerySideMetrics, Option<ErrorLabel>)>,
    epsilon: f64,
) -> CompareReport {
    let side_a_name = settings_a.display_name("A");
    let side_b_name = settings_b.display_name("B");
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    let mut queries = Vec::with_capacity(pairs.len());
    for (query_id, a, b, error_label) in &pairs {
        for (metric, av, bv) in [
            ("recall_at_k", a.recall_at_k, b.recall_at_k),
            ("reciprocal_rank", a.reciprocal_rank, b.reciprocal_rank),
            ("ndcg_at_k", a.ndcg_at_k, b.ndcg_at_k),
        ] {
            let delta = bv - av;
            if delta < -epsilon {
                regressions.push(RegressionEntry {
                    query_id: query_id.clone(),
                    metric: metric.into(),
                    side_a: av,
                    side_b: bv,
                    delta,
                    error_label: error_label.clone(),
                });
            } else if delta > epsilon {
                improvements.push(RegressionEntry {
                    query_id: query_id.clone(),
                    metric: metric.into(),
                    side_a: av,
                    side_b: bv,
                    delta,
                    error_label: error_label.clone(),
                });
            }
        }
        queries.push(ComparedQuery {
            query_id: query_id.clone(),
            side_a: a.clone(),
            side_b: b.clone(),
            error_label: error_label.clone(),
        });
    }
    regressions.sort_by(|l, r| {
        l.delta
            .partial_cmp(&r.delta)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| l.query_id.cmp(&r.query_id))
    });
    improvements.sort_by(|l, r| {
        r.delta
            .partial_cmp(&l.delta)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| l.query_id.cmp(&r.query_id))
    });
    CompareReport {
        dataset_content_hash,
        side_a_name,
        side_b_name,
        macro_a: macro_of(pairs.iter().map(|(_, a, _, _)| a)),
        macro_b: macro_of(pairs.iter().map(|(_, _, b, _)| b)),
        settings_a,
        settings_b,
        regressions,
        improvements,
        queries,
    }
}

fn macro_of<'a>(values: impl Iterator<Item = &'a QuerySideMetrics>) -> QuerySideMetrics {
    let mut recall = 0.0;
    let mut mrr = 0.0;
    let mut ndcg = 0.0;
    let mut n = 0usize;
    for v in values {
        recall += v.recall_at_k;
        mrr += v.reciprocal_rank;
        ndcg += v.ndcg_at_k;
        n += 1;
    }
    let n = n.max(1) as f64;
    QuerySideMetrics {
        recall_at_k: recall / n,
        reciprocal_rank: mrr / n,
        ndcg_at_k: ndcg / n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_recall_regression() {
        let report = compare_query_metrics(
            "hash".into(),
            SearchSettingsProfile {
                name: Some("lex".into()),
                mode: Some("lex".into()),
                ..Default::default()
            },
            SearchSettingsProfile {
                name: Some("hybrid".into()),
                mode: Some("hybrid".into()),
                ..Default::default()
            },
            vec![(
                "q1".into(),
                QuerySideMetrics {
                    recall_at_k: 1.0,
                    reciprocal_rank: 1.0,
                    ndcg_at_k: 1.0,
                },
                QuerySideMetrics {
                    recall_at_k: 0.0,
                    reciprocal_rank: 0.0,
                    ndcg_at_k: 0.0,
                },
                None,
            )],
            1e-9,
        );
        assert_eq!(report.regressions.len(), 3);
        assert!(report.improvements.is_empty());
        assert_eq!(report.regressions[0].query_id, "q1");
    }
}
