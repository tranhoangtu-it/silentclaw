use std::collections::HashMap;

/// Reciprocal Rank Fusion (RRF) merge algorithm.
/// Combines ranked results from vector and FTS searches.
/// Score = Σ 1/(k + rank) where k=60 (standard RRF constant).
pub fn rrf_merge(
    vector_results: &[(String, f32)],
    fts_results: &[(String, f64)],
    k: u32,
    limit: usize,
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();

    for (rank, (id, _)) in vector_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k as f64 + rank as f64 + 1.0);
    }

    for (rank, (id, _)) in fts_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k as f64 + rank as f64 + 1.0);
    }

    let mut merged: Vec<(String, f64)> = scores.into_iter().collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(limit);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_merge_basic() {
        let vector = vec![
            ("doc_a".into(), 0.95f32),
            ("doc_b".into(), 0.80),
            ("doc_c".into(), 0.70),
        ];
        let fts = vec![
            ("doc_b".into(), -1.5f64), // BM25 scores are negative in SQLite
            ("doc_a".into(), -2.0),
            ("doc_d".into(), -3.0),
        ];

        let results = rrf_merge(&vector, &fts, 60, 10);

        // doc_a and doc_b appear in both lists → higher RRF scores
        assert!(results.len() == 4);
        let top_ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
        // Both doc_a and doc_b should be in top 2 (appear in both result sets)
        assert!(top_ids[..2].contains(&"doc_a"));
        assert!(top_ids[..2].contains(&"doc_b"));
    }

    #[test]
    fn test_rrf_merge_empty_inputs() {
        let results = rrf_merge(&[], &[], 60, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_merge_limit() {
        let vector = vec![("a".into(), 1.0f32), ("b".into(), 0.5)];
        let fts = vec![("c".into(), -1.0f64), ("d".into(), -2.0)];
        let results = rrf_merge(&vector, &fts, 60, 2);
        assert_eq!(results.len(), 2);
    }
}
