//! Hidden characters (`untrusted-content` → Hidden character exposure): which characters
//! count as hidden, and their visible `[U+XXXX]` / `[ESC]` markers.

/// Values shown in an attribute or a record are cut to this many characters, the last
/// one being `…`.
pub const VALUE_LIMIT: usize = 512;

/// `text` with each hidden character replaced by `[U+XXXX]` (`[ESC]` for ESC). Nothing
/// is deleted.
pub fn expose_hidden(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for index in 0..chars.len() {
        push_exposed(&mut out, &chars, index);
    }
    out
}

/// Push `chars[index]`, or its marker when it is hidden.
pub(super) fn push_exposed(out: &mut String, chars: &[char], index: usize) {
    match (chars[index], is_hidden(chars, index)) {
        (c, false) => out.push(c),
        ('\u{1b}', true) => out.push_str("[ESC]"),
        (c, true) => out.push_str(&format!("[U+{:04X}]", c as u32)),
    }
}

/// Whether `chars[index]` is hidden: zero-width characters (a BOM only after offset 0,
/// a joiner only outside emoji sequences), other invisible default-ignorable characters
/// (soft hyphen, fillers, invisible operators), variation selectors outside an emoji or
/// ideographic variation sequence, bidi controls, Unicode tag characters, and C0
/// controls other than tab, line feed and carriage return, plus DEL.
pub fn is_hidden(chars: &[char], index: usize) -> bool {
    hidden_class(chars, index).is_some()
}

/// How a hidden character counts toward the `hidden_text` signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HiddenClass {
    /// A zero-width character: it takes three to raise the signal.
    ZeroWidth,
    /// A bidi control, tag character, ESC or other control: one raises the signal.
    Control,
}

pub(super) fn hidden_class(chars: &[char], index: usize) -> Option<HiddenClass> {
    match chars[index] {
        '\t' | '\n' | '\r' => None,
        '\u{0}'..='\u{1f}' | '\u{7f}' => Some(HiddenClass::Control),
        '\u{200b}' | '\u{200c}' | '\u{2060}' => Some(HiddenClass::ZeroWidth),
        '\u{feff}' => (index > 0).then_some(HiddenClass::ZeroWidth),
        '\u{200d}' => (!joins_emoji(chars, index)).then_some(HiddenClass::ZeroWidth),
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => Some(HiddenClass::Control),
        '\u{e0000}'..='\u{e007f}' => Some(HiddenClass::Control),
        '\u{fe00}'..='\u{fe0f}' => {
            (!selects_emoji_variant(chars, index)).then_some(HiddenClass::ZeroWidth)
        }
        // Ideographic variation selectors follow a CJK ideograph; elsewhere (after an
        // emoji, say) they are the byte-smuggling encoding.
        '\u{e0100}'..='\u{e01ef}' => (!index
            .checked_sub(1)
            .is_some_and(|before| is_ideograph(chars[before])))
        .then_some(HiddenClass::Control),
        c if invisible(c) => Some(HiddenClass::ZeroWidth),
        _ => None,
    }
}

/// Default-ignorable characters that render as nothing: soft hyphen, the combining
/// grapheme joiner, Hangul and Mongolian fillers and selectors, invisible operators,
/// deprecated format controls and musical formatting characters.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{34f}' | '\u{115f}' | '\u{1160}' | '\u{17b4}' | '\u{17b5}'
        | '\u{180b}'..='\u{180f}' | '\u{2061}'..='\u{2065}' | '\u{206a}'..='\u{206f}'
        | '\u{3164}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}' | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}')
}

/// A variation selector right after an emoji (`❤️`), or after a keycap base before the
/// keycap mark (`1️⃣`); a run of selectors is never one.
fn selects_emoji_variant(chars: &[char], index: usize) -> bool {
    let Some(&before) = index.checked_sub(1).and_then(|at| chars.get(at)) else {
        return false;
    };
    is_emoji(before)
        || (matches!(before, '0'..='9' | '#' | '*') && chars.get(index + 1) == Some(&'\u{20e3}'))
}

fn is_ideograph(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
        | '\u{20000}'..='\u{3134f}')
}

/// A zero-width joiner between two emoji, past the first one's presentation selector
/// or skin-tone modifier.
fn joins_emoji(chars: &[char], index: usize) -> bool {
    let before = chars[..index]
        .iter()
        .rev()
        .find(|&&c| !matches!(c, '\u{fe0f}' | '\u{1f3fb}'..='\u{1f3ff}'));
    before.is_some_and(|&c| is_emoji(c)) && chars.get(index + 1).is_some_and(|&c| is_emoji(c))
}

/// Extended pictographic characters, approximated by the blocks emoji are drawn from.
fn is_emoji(c: char) -> bool {
    matches!(c,
        '\u{a9}' | '\u{ae}' | '\u{203c}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
        | '\u{2194}'..='\u{2199}' | '\u{21a9}'..='\u{21aa}' | '\u{231a}'..='\u{23ff}'
        | '\u{24c2}' | '\u{25aa}'..='\u{25fe}' | '\u{2600}'..='\u{27bf}'
        | '\u{2934}'..='\u{2935}' | '\u{2b05}'..='\u{2b55}' | '\u{3030}' | '\u{303d}'
        | '\u{3297}' | '\u{3299}' | '\u{1f000}'..='\u{1faff}')
}

/// A value shown to people or the model as provenance (an `origin`): C0 controls
/// dropped, other hidden characters as markers, cut to 512 characters ending in `…`.
pub fn visible_value(value: &str) -> String {
    let visible = expose_hidden(&value.replace(|c: char| c <= '\u{1f}', ""));
    if visible.chars().count() <= VALUE_LIMIT {
        return visible;
    }
    visible.chars().take(VALUE_LIMIT - 1).chain(['…']).collect()
}

/// A URL (or a value that embeds one, such as `mcp:<server>:<uri>`) without the
/// userinfo of its first authority, which may carry credentials.
pub fn without_userinfo(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.into();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_is_removed() {
        assert_eq!(
            without_userinfo("https://user:pass@host.test/p?q=1"),
            "https://host.test/p?q=1"
        );
        assert_eq!(
            without_userinfo("mcp:docs:https://u:p@x.test/a"),
            "mcp:docs:https://x.test/a"
        );
        assert_eq!(
            without_userinfo("https://host.test/a@b"),
            "https://host.test/a@b"
        );
        assert_eq!(without_userinfo("search:brave"), "search:brave");
    }

    #[test]
    fn values_drop_controls_expose_hidden_and_are_cut() {
        assert_eq!(visible_value("a\nb\u{0}c\u{202e}"), "abc[U+202E]");
        let cut = visible_value(&"a".repeat(600));
        assert_eq!(cut.chars().count(), VALUE_LIMIT);
        assert!(cut.ends_with('…'));
    }

    /// Scenario "Smuggled tag characters".
    #[test]
    fn tag_characters_become_markers() {
        let hidden: String = "ignore"
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        let out = expose_hidden(&format!("hello{hidden}"));
        assert!(out.starts_with("hello[U+E0069][U+E0067][U+E006E]"), "{out}");
    }

    #[test]
    fn controls_bidi_and_zero_width_become_markers() {
        assert_eq!(
            expose_hidden("\u{feff}a\u{1b}[31mb\u{7f}\u{0}\t\r\n\u{200b}\u{200c}\u{2060}\u{feff}"),
            "\u{feff}a[ESC][31mb[U+007F][U+0000]\t\r\n[U+200B][U+200C][U+2060][U+FEFF]"
        );
        assert_eq!(
            expose_hidden("\u{202a}\u{202e}\u{2066}\u{2069}"),
            "[U+202A][U+202E][U+2066][U+2069]"
        );
        assert_eq!(expose_hidden("ig\u{200b}nore"), "ig[U+200B]nore");
        assert_eq!(expose_hidden("a\u{200d}b"), "a[U+200D]b");
    }

    /// Scenario "Emoji sequence kept".
    #[test]
    fn emoji_sequences_keep_their_joiners() {
        for emoji in [
            "👨\u{200d}👩\u{200d}👧\u{200d}👦",
            "🏳\u{fe0f}\u{200d}🌈",
            "👩🏽\u{200d}💻",
            "❤\u{fe0f}\u{200d}🔥",
        ] {
            assert_eq!(expose_hidden(emoji), emoji);
        }
        assert_eq!(expose_hidden("👨\u{200d}"), "👨[U+200D]");
        assert_eq!(expose_hidden("\u{200d}👨"), "[U+200D]👨");
        // Presentation selectors after an emoji or a keycap base are kept, as is an
        // ideographic variation sequence.
        for kept in ["❤\u{fe0f}", "1\u{fe0f}\u{20e3}", "葛\u{e0100}"] {
            assert_eq!(expose_hidden(kept), kept);
        }
    }

    /// Regression: invisible characters outside the first list (soft hyphen, fillers,
    /// invisible operators) and variation-selector smuggling passed unseen.
    #[test]
    fn other_invisible_characters_and_smuggled_selectors_become_markers() {
        assert_eq!(expose_hidden("ig\u{ad}nore"), "ig[U+00AD]nore");
        assert_eq!(
            expose_hidden("a\u{34f}\u{2062}\u{3164}\u{115f}\u{180e}"),
            "a[U+034F][U+2062][U+3164][U+115F][U+180E]"
        );
        assert_eq!(expose_hidden("x\u{fe01}"), "x[U+FE01]");
        assert_eq!(expose_hidden("❤\u{fe0f}\u{fe0f}"), "❤\u{fe0f}[U+FE0F]");
        assert_eq!(
            expose_hidden("😀\u{e0100}\u{e0101}"),
            "😀[U+E0100][U+E0101]"
        );
    }

    /// Property: exposure never loses a character. Decoding every marker back to the
    /// character it names gives the input exactly, and the markers count the hidden
    /// characters. The alphabet has no `[`, so a marker cannot be confused with text.
    #[test]
    fn exposure_never_loses_a_character() {
        let alphabet = [
            'a',
            ' ',
            '\n',
            '\t',
            '\r',
            '<',
            'U',
            '+',
            '\u{0}',
            '\u{1b}',
            '\u{7f}',
            '\u{200b}',
            '\u{200c}',
            '\u{200d}',
            '\u{2060}',
            '\u{feff}',
            '\u{202e}',
            '\u{2067}',
            '\u{e0041}',
            '\u{e007f}',
            '👨',
            '\u{fe0f}',
            '\u{1f3fd}',
            '\u{ad}',
            '\u{fe01}',
            '\u{e0100}',
            '\u{3164}',
            'é',
            '中',
        ];
        let mut seed: u64 = 0x5eed;
        for _ in 0..2000 {
            let len = (seed % 24) as usize;
            let input: String = (0..len)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    alphabet[(seed >> 33) as usize % alphabet.len()]
                })
                .collect();
            let out = expose_hidden(&input);
            let (decoded, markers) = decode(&out);
            assert_eq!(decoded, input, "{out:?}");
            let chars: Vec<char> = input.chars().collect();
            let hidden = (0..chars.len()).filter(|&i| is_hidden(&chars, i)).count();
            assert_eq!(markers, hidden, "{input:?}");
        }
    }

    fn decode(out: &str) -> (String, usize) {
        let mut decoded = String::new();
        let mut markers = 0;
        let mut rest = out;
        while let Some(at) = rest.find('[') {
            decoded.push_str(&rest[..at]);
            let end = rest[at..].find(']').unwrap() + at;
            let marker = &rest[at + 1..end];
            let c = match marker.strip_prefix("U+") {
                Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap(),
                None => {
                    assert_eq!(marker, "ESC");
                    '\u{1b}'
                }
            };
            decoded.push(c);
            markers += 1;
            rest = &rest[end + 1..];
        }
        decoded.push_str(rest);
        (decoded, markers)
    }
}
