pub(crate) fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut shortened = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    while shortened.ends_with(char::is_whitespace) {
        shortened.pop();
    }
    shortened.push_str("...");
    shortened
}
