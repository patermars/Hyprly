pub(crate) fn normalize_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::normalize_word;

    #[test]
    fn trims_punctuation_and_normalizes_case() {
        assert_eq!(normalize_word("Hello,"), "hello");
        assert_eq!(normalize_word("..."), "");
    }
}
