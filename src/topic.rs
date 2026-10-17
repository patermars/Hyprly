use crate::text::normalize_word;
use std::collections::HashSet;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "can", "could", "do", "for", "how", "i", "in", "is", "it", "me", "of",
    "on", "please", "the", "to", "we", "what", "with", "you",
];

fn terms(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .map(normalize_word)
        .filter(|word| word.len() > 1 && !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

fn domain(words: &HashSet<String>) -> Option<&'static str> {
    if words.iter().any(|word| {
        matches!(
            word.as_str(),
            "sql" | "select" | "join" | "database" | "postgres" | "mysql"
        )
    }) {
        Some("sql")
    } else if words.iter().any(|word| {
        matches!(
            word.as_str(),
            "python" | "rust" | "javascript" | "typescript" | "java" | "code" | "function"
        )
    }) {
        Some("programming")
    } else if words.iter().any(|word| {
        matches!(
            word.as_str(),
            "algorithm" | "array" | "list" | "hashmap" | "complexity" | "twosum"
        )
    }) {
        Some("algorithms")
    } else {
        None
    }
}

fn has_transition_phrase(lower: &str) -> bool {
    [
        "now ",
        "next ",
        "instead ",
        "another question",
        "what about ",
        "switching topics",
        "also,",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

fn is_continuation(previous: &str, lower: &str, overlap: f32) -> bool {
    let word_count = lower.split_whitespace().count();
    let refinement = [
        "explain that",
        "explain this",
        "make it",
        "use ",
        "show me",
        "what is the time complexity",
        "can you clarify",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase));
    let pronoun_reference = lower
        .split_whitespace()
        .any(|word| matches!(word, "that" | "this" | "it" | "them" | "those"));
    !previous.trim().is_empty()
        && (overlap >= 0.25 || (word_count <= 8 && (refinement || pronoun_reference)))
}

/// Returns true when `current` should start a fresh answer context.
pub fn is_new_topic(previous: &str, current: &str) -> bool {
    if previous.trim().is_empty() {
        return false;
    }

    let previous_terms = terms(previous);
    let current_terms = terms(current);
    if current_terms.is_empty() {
        return false;
    }
    let overlap =
        current_terms.intersection(&previous_terms).count() as f32 / current_terms.len() as f32;
    let current_lower = current.to_lowercase();
    let domain_changed = matches!(
        (domain(&previous_terms), domain(&current_terms)),
        (Some(previous), Some(current)) if previous != current
    );
    let independent_question = current_terms.len() >= 4 && overlap < 0.12;
    let topic_change =
        has_transition_phrase(&current_lower) || domain_changed || independent_question;

    topic_change && !is_continuation(previous, &current_lower, overlap)
}

#[cfg(test)]
mod tests {
    use super::is_new_topic;

    #[test]
    fn detects_domain_switch() {
        assert!(is_new_topic(
            "Write Python code for the Two Sum algorithm",
            "Now write a SQL query to find duplicate emails"
        ));
    }

    #[test]
    fn keeps_short_followups_in_context() {
        assert!(!is_new_topic(
            "Write Python code for the Two Sum algorithm",
            "Can you make it faster?"
        ));
    }
}
