//! Explicit leading agent mentions; catalogue eligibility is checked by callers.

pub fn parse(text: &str) -> Option<(String, &str)> {
    let rest = text.trim_start().strip_prefix('@')?;
    if rest.starts_with('"') {
        let mut quoted = serde_json::Deserializer::from_str(rest).into_iter::<String>();
        let name = quoted.next()?.ok()?;
        let prompt = &rest[quoted.byte_offset()..];
        if !prompt.is_empty() && !prompt.starts_with(char::is_whitespace) {
            return None;
        }
        return Some((name, prompt.trim_start()));
    }
    let (name, prompt) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some((name.to_owned(), prompt.trim_start()))
}
