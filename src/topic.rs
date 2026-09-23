use std::collections::HashSet;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "can", "could", "do", "for", "how", "i", "in", "is", "it", "me", "of",
    "on", "please", "the", "to", "we", "what", "with", "you",
];

fn terms(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| word.len() > 1 && !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

fn domain(text: &str) -> Option<&'static str> {
    let words = terms(text);
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

fn has_transition_phrase(text: &str) -> bool {
    let lower = text.to_lowercase();
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

fn is_continuation(previous: &str, current: &str, overlap: f32) -> bool {
    let words: Vec<String> = current
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect();
    let lower = current.to_lowercase();
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
    let pronoun_reference = words
        .iter()
        .any(|word| matches!(word.as_str(), "that" | "this" | "it" | "them" | "those"));
    !previous.trim().is_empty()
        && (overlap >= 0.25 || (words.len() <= 8 && (refinement || pronoun_reference)))
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
    let overlap = current_terms.intersection(&previous_terms).count() as f32
        / current_terms.len().max(1) as f32;

    if has_transition_phrase(current) && !is_continuation(previous, current, overlap) {
        return true;
    }

    if let (Some(previous_domain), Some(current_domain)) = (domain(previous), domain(current)) {
        if previous_domain != current_domain && !is_continuation(previous, current, overlap) {
            return true;
        }
    }

    current_terms.len() >= 4 && overlap < 0.12 && !is_continuation(previous, current, overlap)
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
