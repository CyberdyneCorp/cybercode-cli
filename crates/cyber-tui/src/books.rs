//! The Stats tab's book comparison line (`tui` → Settings Stats tab): a period's input
//! plus output tokens next to a well-known book. Shared with `cyber stats`.

/// Well-known books and their approximate token counts (about 1.3 tokens per word),
/// smallest first.
pub const BOOKS: &[(&str, u64)] = &[
    ("The Little Prince", 22_000),
    ("The Great Gatsby", 62_000),
    ("Brave New World", 77_700),
    ("Frankenstein", 98_000),
    ("Pride and Prejudice", 160_000),
    ("Moby-Dick", 275_000),
    ("The Lord of the Rings", 640_000),
    ("War and Peace", 780_000),
];

/// Tokens on one paperback page, the scale below the smallest book.
const PAGE_TOKENS: u64 = 400;

/// `Your input and output are ~7718x the tokens in Brave New World` for the largest book
/// `tokens` exceeds, a page count below the smallest book, and `None` without usage.
pub fn comparison(tokens: u64) -> Option<String> {
    if tokens == 0 {
        return None;
    }
    let lead = "Your input and output";
    let Some((title, size)) = BOOKS.iter().rev().find(|(_, size)| tokens > *size) else {
        let pages = tokens / PAGE_TOKENS;
        return Some(match pages {
            0 => format!("{lead} would fit on one page of a paperback"),
            1 => format!("{lead} are ~1 page of a paperback"),
            n => format!("{lead} are ~{n} pages of a paperback"),
        });
    };
    let ratio = tokens as f64 / *size as f64;
    let times = if ratio < 10.0 {
        format!("{ratio:.1}")
    } else {
        format!("{}", ratio.round() as u64)
    };
    Some(format!("{lead} are ~{times}x the tokens in {title}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_largest_book_the_total_exceeds() {
        assert!(BOOKS.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(
            comparison(599_700_000).as_deref(),
            Some("Your input and output are ~769x the tokens in War and Peace")
        );
        // Up to Frankenstein's size (not exceeded), Brave New World is the book.
        assert_eq!(
            comparison(98_000).as_deref(),
            Some("Your input and output are ~1.3x the tokens in Brave New World")
        );
        assert_eq!(
            comparison(70_000).as_deref(),
            Some("Your input and output are ~1.1x the tokens in The Great Gatsby")
        );
        assert_eq!(
            comparison(22_001).as_deref(),
            Some("Your input and output are ~1.0x the tokens in The Little Prince")
        );
    }

    #[test]
    fn small_totals_count_pages_and_no_usage_has_no_line() {
        assert_eq!(comparison(0), None);
        assert_eq!(
            comparison(120).as_deref(),
            Some("Your input and output would fit on one page of a paperback")
        );
        assert_eq!(
            comparison(450).as_deref(),
            Some("Your input and output are ~1 page of a paperback")
        );
        assert_eq!(
            comparison(22_000).as_deref(),
            Some("Your input and output are ~55 pages of a paperback")
        );
    }
}
