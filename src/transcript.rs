#[derive(Clone, Debug)]
pub struct CleanConfig {
    pub remove_fillers: bool,
    pub collapse_repeats: bool,
    pub remove_false_starts: bool,
    pub remove_incomplete_fragments: bool,
    pub custom_fillers: Vec<String>,
}

impl Default for CleanConfig {
    fn default() -> Self {
        Self {
            remove_fillers: true,
            collapse_repeats: true,
            remove_false_starts: true,
            remove_incomplete_fragments: true,
            custom_fillers: Vec::new(),
        }
    }
}

fn word_key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn is_meaningful_filler(words: &[&str], index: usize) -> bool {
    let word = word_key(words[index]);
    let next = words.get(index + 1).map(|word| word_key(word));
    matches!(
        (word.as_str(), next.as_deref()),
        ("ah", Some("yes" | "no" | "okay" | "right")) | ("hmm", Some("let" | "well" | "okay"))
    )
}

fn should_keep_like(words: &[&str], index: usize) -> bool {
    let previous = index
        .checked_sub(1)
        .and_then(|i| words.get(i))
        .map(|word| word_key(word));
    let next = words.get(index + 1).map(|word| word_key(word));
    matches!(
        (previous.as_deref(), next.as_deref()),
        (Some("would" | "looks" | "feel" | "seem"), _)
            | (_, Some("a" | "an" | "the" | "that" | "this"))
    )
}

fn remove_fillers(text: &str, config: &CleanConfig) -> String {
    let mut fillers = vec!["uh", "um", "uhh", "umm", "er", "ah", "hmm"];
    let custom: Vec<String> = config
        .custom_fillers
        .iter()
        .map(|filler| filler.to_lowercase())
        .collect();
    fillers.extend(custom.iter().map(String::as_str));

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut kept: Vec<String> = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let key = word_key(word);
        if fillers.contains(&key.as_str()) && !is_meaningful_filler(&words, index) {
            if let Some(previous) = kept.last_mut() {
                if previous.ends_with(',') {
                    previous.pop();
                }
            }
            continue;
        }
        if key == "like" && !should_keep_like(&words, index) {
            continue;
        }
        kept.push((*word).to_string());
    }
    kept.join(" ")
}

fn collapse_repeats(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        if kept
            .last()
            .map(|last| word_key(last) == word_key(word) && !word_key(word).is_empty())
            .unwrap_or(false)
        {
            continue;
        }
        kept.push(word);
    }
    kept.join(" ")
}

fn remove_false_starts(text: &str) -> String {
    // Only remove cues when they are a complete comma-delimited segment.
    let cues = [
        "no wait",
        "i mean",
        "sorry",
        "actually",
        "let me rephrase",
        "rather",
    ];
    text.split(',')
        .map(str::trim)
        .filter(|segment| !cues.iter().any(|cue| segment.eq_ignore_ascii_case(cue)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn remove_incomplete_fragment(text: &str) -> String {
    const DANGLING: [&str; 9] = [
        "and", "but", "or", "so", "because", "to", "of", "with", "that",
    ];
    let trimmed = text.trim();
    let Some(last) = trimmed.split_whitespace().last() else {
        return String::new();
    };
    let last_key = word_key(last);
    if !DANGLING.contains(&last_key.as_str()) {
        return trimmed.to_string();
    }
    let without_last = trimmed[..trimmed.len() - last.len()].trim_end();
    if without_last.ends_with(['.', '?', '!']) {
        without_last.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn clean(raw: &str, config: &CleanConfig) -> String {
    let mut text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if config.remove_fillers {
        text = remove_fillers(&text, config);
    }
    if config.collapse_repeats {
        text = collapse_repeats(&text);
    }
    if config.remove_false_starts {
        text = remove_false_starts(&text);
    }
    if config.remove_incomplete_fragments {
        text = remove_incomplete_fragment(&text);
    }

    let mut chars = text.chars();
    if let Some(first) = chars.next() {
        if first.is_lowercase() {
            text = first.to_uppercase().collect::<String>() + chars.as_str();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{clean, CleanConfig};

    #[test]
    fn removes_fillers_but_preserves_meaning() {
        let config = CleanConfig::default();
        assert_eq!(
            clean(
                "Um, I think, uh, we should review the numbers again.",
                &config
            ),
            "I think we should review the numbers again."
        );
        assert_eq!(
            clean("Ah, yes, that's right.", &config),
            "Ah, yes, that's right."
        );
        assert_eq!(
            clean("I would like a review.", &config),
            "I would like a review."
        );
    }

    #[test]
    fn keeps_intentful_false_start_phrases() {
        let config = CleanConfig::default();
        assert_eq!(
            clean("I mean this sincerely.", &config),
            "I mean this sincerely."
        );
        assert_eq!(
            clean("No wait, the migration is first.", &config),
            "The migration is first."
        );
    }

    #[test]
    fn removes_only_obvious_trailing_fragments() {
        let config = CleanConfig::default();
        assert_eq!(
            clean("We should finish the migration. And", &config),
            "We should finish the migration."
        );
        assert_eq!(clean("Sounds good", &config), "Sounds good");
    }
}
