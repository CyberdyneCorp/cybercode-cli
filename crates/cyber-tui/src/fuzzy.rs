//! Subsequence fuzzy matching for pickers and autocomplete.

/// A score for `query` in `text` (higher is better), or `None` when not every query
/// character appears in order. Consecutive and word-start matches score higher.
pub fn score(query: &str, text: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for q in query.to_lowercase().chars() {
        let found = (pos..text.len()).find(|&i| text[i] == q)?;
        score += 10;
        if last == Some(found.wrapping_sub(1)) {
            score += 15;
        }
        if found == 0 || matches!(text[found - 1], '/' | '-' | '_' | ' ' | '.') {
            score += 10;
        }
        score -= (found - pos) as i64;
        last = Some(found);
        pos = found + 1;
    }
    Some(score - text.len() as i64 / 10)
}

/// Indices of `items` matching `query`, best first.
pub fn rank<T>(query: &str, items: &[T], text: impl Fn(&T) -> String) -> Vec<usize> {
    let mut scored: Vec<(i64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(query, &text(it)).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequences_match_and_word_starts_win() {
        assert!(score("smr", "src/main.rs").is_some());
        assert!(score("xyz", "src/main.rs").is_none());
        let items = ["src/lib.rs", "src/main.rs", "README.md"];
        assert_eq!(rank("main", &items, |s| s.to_string())[0], 1);
        assert_eq!(rank("", &items, |s| s.to_string()).len(), 3);
    }
}
