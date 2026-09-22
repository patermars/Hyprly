#[derive(Clone, Debug)]
pub struct CleanConfig {
    pub remove_fillers: bool,
    pub collapse_repeats: bool,
    pub remove_false_starts: bool,
    pub custom_fillers: Vec<String>,
}

impl Default for CleanConfig {
    fn default() -> Self {
        Self {
            remove_fillers: true,
            collapse_repeats: true,
            remove_false_starts: true,
            custom_fillers: Vec::new(),
        }
    }
}

pub fn clean(raw: &str, config: &CleanConfig) -> String {
    let mut text = raw.to_string();

    if config.remove_fillers {
        let mut fillers = vec!["uh", "um", "uhh", "umm", "er", "ah", "hmm"];
        let custom: Vec<&str> = config.custom_fillers.iter().map(AsRef::as_ref).collect();
        fillers.extend(custom);

        let words: Vec<&str> = text.split_whitespace().collect();
        let mut new_words = Vec::new();
        let len = words.len();

        for i in 0..len {
            let w_lower = words[i].to_lowercase();
            let w_clean = w_lower.trim_matches(|c: char| !c.is_alphabetic());

            if fillers.contains(&w_clean) {
                continue;
            }

            if w_clean == "like" {
                if i > 0 && i < len - 1 {
                    let prev = words[i - 1].to_lowercase();
                    let prev_clean = prev.trim_matches(|c: char| !c.is_alphabetic());
                    let next = words[i + 1].to_lowercase();
                    let next_clean = next.trim_matches(|c: char| !c.is_alphabetic());

                    let retain = matches!(
                        (prev_clean, next_clean),
                        ("would", _)
                            | ("looks", _)
                            | ("feel", _)
                            | (_, "a")
                            | (_, "the")
                            | (_, "that")
                            | (_, "this")
                    );

                    if !retain {
                        continue;
                    }
                } else if i > 0 {
                    let prev = words[i - 1].to_lowercase();
                    let prev_clean = prev.trim_matches(|c: char| !c.is_alphabetic());
                    let retain = matches!(prev_clean, "would" | "looks" | "feel");
                    if !retain {
                        continue;
                    }
                } else if i < len - 1 {
                    let next = words[i + 1].to_lowercase();
                    let next_clean = next.trim_matches(|c: char| !c.is_alphabetic());
                    let retain = matches!(next_clean, "a" | "the" | "that" | "this");
                    if !retain {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            new_words.push(words[i]);
        }
        text = new_words.join(" ");
    }

    if config.collapse_repeats {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut new_words: Vec<&str> = Vec::new();
        for w in words {
            if let Some(last) = new_words.last() {
                let w_lower = w.to_lowercase();
                let last_lower = last.to_lowercase();
                let w_clean = w_lower.trim_matches(|c: char| !c.is_alphabetic());
                let last_clean = last_lower.trim_matches(|c: char| !c.is_alphabetic());
                if w_clean != last_clean || w_clean.is_empty() {
                    new_words.push(w);
                }
            } else {
                new_words.push(w);
            }
        }
        text = new_words.join(" ");
    }

    if config.remove_false_starts {
        let cues = [
            "no wait",
            "i mean",
            "sorry",
            "actually",
            "let me rephrase",
            "rather",
        ];
        let segments: Vec<&str> = text.split(',').collect();
        let mut cleaned_segments = Vec::new();

        for seg in segments {
            let mut seg_text = seg.to_string();
            let mut changed = true;
            while changed {
                changed = false;
                let lower = seg_text.to_lowercase();
                let mut first_match = None;
                for cue in cues {
                    if let Some(idx) = lower.find(cue) {
                        if first_match.map_or(true, |(first_idx, _)| idx < first_idx) {
                            first_match = Some((idx, cue.len()));
                        }
                    }
                }
                if let Some((idx, len)) = first_match {
                    seg_text = seg_text[idx + len..].trim().to_string();
                    changed = true;
                }
            }
            if !seg_text.trim().is_empty() {
                cleaned_segments.push(seg_text.trim().to_string());
            }
        }
        text = cleaned_segments.join(", ");
    }

    text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if let Some(c) = text.chars().next() {
        if c.is_lowercase() {
            let mut chars = text.chars();
            text = match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            };
        }
    }

    text
}
