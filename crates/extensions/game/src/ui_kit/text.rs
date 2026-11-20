//! Shared formatting for player-facing prose.

/// Joins non-empty sentences with one space. Fold whitespace within
/// each sentence to spaces so the guest widget can wrap the prose.
pub(crate) fn join_sentences(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut text = String::new();
    for part in parts {
        let part = part.as_ref().trim();
        if part.is_empty() {
            continue;
        }
        for word in part.split_whitespace() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(word);
        }
        if !part
            .trim_end_matches(['"', '”', '’', ')', ']'])
            .ends_with(['.', '!', '?', '…'])
        {
            text.push('.');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_use_one_space_and_keep_existing_punctuation() {
        assert_eq!(
            join_sentences([
                "  ",
                "First\r\nline.",
                "Second  line ",
                "Ready?",
                "Go!",
                "Wait…"
            ]),
            "First line. Second line. Ready? Go! Wait…"
        );
        assert_eq!(join_sentences(["", " "]), "");
    }
}
