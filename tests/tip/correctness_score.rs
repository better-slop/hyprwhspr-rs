#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CorrectnessScore {
    pub exact_match: bool,
    pub expected_word_count: usize,
    pub actual_word_count: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub replacements: usize,
    pub word_error_rate: f64,
    pub word_accuracy: f64,
    pub character_similarity: f64,
}

impl CorrectnessScore {
    pub(crate) fn calculate(expected: &str, actual: &str) -> Self {
        let expected_words = words(expected);
        let actual_words = words(actual);
        let (insertions, deletions, replacements) =
            word_edit_counts(&expected_words, &actual_words);
        let word_errors = insertions + deletions + replacements;
        let expected_word_count = expected_words.len();
        let actual_word_count = actual_words.len();
        let word_error_rate = if expected_word_count == 0 {
            if word_errors == 0 { 0.0 } else { 1.0 }
        } else {
            word_errors as f64 / expected_word_count as f64
        };
        let word_accuracy = (1.0 - word_error_rate).max(0.0);
        let character_similarity = character_similarity(expected, actual);

        Self {
            exact_match: expected == actual,
            expected_word_count,
            actual_word_count,
            insertions,
            deletions,
            replacements,
            word_error_rate,
            word_accuracy,
            character_similarity,
        }
    }

    pub(crate) fn render(&self) -> String {
        format!(
            "CorrectnessScore exact_match={} word_accuracy={:.4} word_error_rate={:.4} character_similarity={:.4} expected_words={} actual_words={} insertions={} deletions={} replacements={}",
            self.exact_match,
            self.word_accuracy,
            self.word_error_rate,
            self.character_similarity,
            self.expected_word_count,
            self.actual_word_count,
            self.insertions,
            self.deletions,
            self.replacements,
        )
    }
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

fn word_edit_counts(expected: &[String], actual: &[String]) -> (usize, usize, usize) {
    let rows = expected.len() + 1;
    let cols = actual.len() + 1;
    let mut dp = vec![vec![0usize; cols]; rows];

    for (idx, row) in dp.iter_mut().enumerate() {
        row[0] = idx;
    }
    for idx in 0..cols {
        dp[0][idx] = idx;
    }

    for i in 1..rows {
        for j in 1..cols {
            let cost = usize::from(expected[i - 1] != actual[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    let mut i = expected.len();
    let mut j = actual.len();
    let mut insertions = 0;
    let mut deletions = 0;
    let mut replacements = 0;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && expected[i - 1] == actual[j - 1] && dp[i][j] == dp[i - 1][j - 1] {
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0 && dp[i][j] == dp[i - 1][j - 1] + 1 {
            replacements += 1;
            i -= 1;
            j -= 1;
        } else if j > 0 && dp[i][j] == dp[i][j - 1] + 1 {
            insertions += 1;
            j -= 1;
        } else {
            deletions += 1;
            i -= 1;
        }
    }

    (insertions, deletions, replacements)
}

fn character_similarity(expected: &str, actual: &str) -> f64 {
    let expected_chars = expected.chars().collect::<Vec<_>>();
    let actual_chars = actual.chars().collect::<Vec<_>>();
    let max_len = expected_chars.len().max(actual_chars.len());
    if max_len == 0 {
        return 1.0;
    }

    let distance = levenshtein_chars(&expected_chars, &actual_chars);
    (1.0 - (distance as f64 / max_len as f64)).max(0.0)
}

fn levenshtein_chars(expected: &[char], actual: &[char]) -> usize {
    let mut previous = (0..=actual.len()).collect::<Vec<_>>();
    let mut current = vec![0; actual.len() + 1];

    for (i, expected_char) in expected.iter().enumerate() {
        current[0] = i + 1;
        for (j, actual_char) in actual.iter().enumerate() {
            let cost = usize::from(expected_char != actual_char);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[actual.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_scores_perfectly() {
        let score = CorrectnessScore::calculate("hello world", "hello world");
        assert!(score.exact_match);
        assert_eq!(score.expected_word_count, 2);
        assert_eq!(score.actual_word_count, 2);
        assert_eq!(score.insertions + score.deletions + score.replacements, 0);
        assert_eq!(score.word_error_rate, 0.0);
        assert_eq!(score.word_accuracy, 1.0);
        assert_eq!(score.character_similarity, 1.0);
    }

    #[test]
    fn mismatch_counts_word_operations() {
        let score = CorrectnessScore::calculate("hello brave world", "hello new world now");
        assert!(!score.exact_match);
        assert_eq!(score.expected_word_count, 3);
        assert_eq!(score.actual_word_count, 4);
        assert_eq!(score.insertions, 1);
        assert_eq!(score.replacements, 1);
        assert_eq!(score.deletions, 0);
    }
}
