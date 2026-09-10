//! Pure retrieval scoring for title-labelled eval queries.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedTitle {
    pub rank: usize,
    pub document_title: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoredQuery {
    pub id: String,
    pub recall_at_k: f64,
    pub reciprocal_rank: f64,
    pub ndcg_at_k: f64,
    pub results: Vec<ScoredResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoredResult {
    pub rank: usize,
    pub document_title: String,
    pub relevance: u32,
    pub score: f32,
}

/// Score ranked hits against graded `document_title → relevance` labels.
pub fn score_ranked_titles(
    query_id: &str,
    labels: &[(String, u32)],
    hits: &[RankedTitle],
    top_k: usize,
) -> ScoredQuery {
    let label_map: HashMap<&str, u32> = labels
        .iter()
        .map(|(title, relevance)| (title.as_str(), *relevance))
        .collect();
    let mut seen = HashSet::new();
    let results: Vec<_> = hits
        .iter()
        .take(top_k)
        .map(|hit| {
            let relevance = if seen.insert(hit.document_title.clone()) {
                label_map
                    .get(hit.document_title.as_str())
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            ScoredResult {
                rank: hit.rank,
                document_title: hit.document_title.clone(),
                relevance,
                score: hit.score,
            }
        })
        .collect();
    let reciprocal_rank = results
        .iter()
        .find(|h| h.relevance > 0)
        .map(|h| 1.0 / h.rank as f64)
        .unwrap_or(0.0);
    let dcg = gain(results.iter().map(|h| h.relevance));
    let mut ideal: Vec<_> = labels.iter().map(|(_, r)| *r).collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    ideal.truncate(top_k);
    let idcg = gain(ideal.into_iter());
    let labeled = labels.len().max(1) as f64;
    ScoredQuery {
        id: query_id.to_string(),
        recall_at_k: results.iter().filter(|h| h.relevance > 0).count() as f64 / labeled,
        reciprocal_rank,
        ndcg_at_k: if idcg == 0.0 { 0.0 } else { dcg / idcg },
        results,
    }
}

fn gain(values: impl Iterator<Item = u32>) -> f64 {
    values
        .enumerate()
        .map(|(i, r)| (2_f64.powi(r as i32) - 1.0) / ((i + 2) as f64).log2())
        .sum()
}

pub fn macro_average(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for value in values {
        sum += value;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_hit_scores_one() {
        let scored = score_ranked_titles(
            "q1",
            &[("a.md".into(), 3)],
            &[RankedTitle {
                rank: 1,
                document_title: "a.md".into(),
                score: 1.0,
            }],
            5,
        );
        assert_eq!(scored.recall_at_k, 1.0);
        assert_eq!(scored.reciprocal_rank, 1.0);
        assert!(scored.ndcg_at_k > 0.99);
    }
}
