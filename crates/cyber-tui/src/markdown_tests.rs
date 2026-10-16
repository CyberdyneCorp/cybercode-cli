use ratatui::style::{Color, Modifier, Style};

use super::*;

fn tokens() -> Tokens {
    crate::theme::Catalog::default()
        .get("cyber")
        .resolve(false, crate::theme::Depth::TrueColor)
}

fn rows(r: &Rendered) -> Vec<String> {
    r.lines
        .iter()
        .map(|l| l.to_string().trim_end().to_owned())
        .collect()
}

fn show(text: &str, width: usize) -> (Vec<String>, Rendered) {
    let t = tokens();
    let r = render(text, &Ctx::new(&t, width, false));
    (rows(&r), r)
}

/// The style of the span containing `needle`.
fn style_of(r: &Rendered, needle: &str) -> Style {
    r.lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not rendered: {:?}", rows(r)))
        .style
}

#[test]
fn headings_are_bold_md_heading_without_hashes() {
    let t = tokens();
    let (rows, r) = show("# Title\n\nBody\n\n### Small\n", 40);
    assert_eq!(rows, ["  Title", "", "  Body", "", "  Small"]);
    let style = style_of(&r, "Title");
    assert_eq!(style.fg, Some(t.md_heading));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn emphasis_strikethrough_and_inline_code() {
    let t = tokens();
    let (rows, r) = show("a **bold** *it* ~~gone~~ `code` b", 60);
    assert_eq!(rows, ["  a bold it gone code b"]);
    assert!(style_of(&r, "bold").add_modifier.contains(Modifier::BOLD));
    assert!(style_of(&r, "it").add_modifier.contains(Modifier::ITALIC));
    assert!(
        style_of(&r, "gone")
            .add_modifier
            .contains(Modifier::CROSSED_OUT)
    );
    assert_eq!(style_of(&r, "code").fg, Some(t.md_code));
}

#[test]
fn numbered_list_with_bold_terms() {
    let t = tokens();
    let (rows, r) = show("1. **Memoization:** Results are cached\n2. Two\n", 60);
    assert_eq!(rows, ["  1. Memoization: Results are cached", "  2. Two"]);
    let bold = style_of(&r, "Memoization:");
    assert!(bold.add_modifier.contains(Modifier::BOLD));
    assert_eq!(style_of(&r, " Results are cached").fg, Some(t.text));
    assert!(!rows.concat().contains('*'));
}

#[test]
fn ordered_lists_keep_their_numbers_and_nested_lists_indent() {
    let (rows, _) = show("3. three\n4. four\n   - inner\n     - deeper\n", 60);
    assert_eq!(
        rows,
        ["  3. three", "  4. four", "     • inner", "       • deeper"]
    );
}

#[test]
fn task_items_use_boxes() {
    let (rows, _) = show("- [ ] todo\n- [x] done\n", 40);
    assert_eq!(rows, ["  ☐ todo", "  ☑ done"]);
}

#[test]
fn wrapped_items_and_quotes_keep_their_hanging_indent() {
    let (rows, _) = show("- one two three four five six\n", 16);
    assert_eq!(rows, ["  • one two", "    three four", "    five six"]);
    let (rows, r) = show("> quoted words that wrap around\n", 18);
    assert_eq!(rows, ["  ▎ quoted words", "  ▎ that wrap", "  ▎ around"]);
    assert_eq!(style_of(&r, "▎").fg, Some(tokens().md_quote));
}

#[test]
fn a_word_longer_than_the_row_is_broken() {
    let (rows, _) = show("abcdefghijkl", 8);
    assert_eq!(rows, ["  abcdef", "  ghijkl"]);
}

#[test]
fn rules_span_the_width() {
    let t = tokens();
    let (rows, r) = show("a\n\n---\n\nb", 12);
    assert_eq!(rows[2], format!("  {}", "─".repeat(10)));
    assert_eq!(style_of(&r, "─").fg, Some(t.md_rule));
}

#[test]
fn tables_draw_borders_and_fall_back_when_too_wide() {
    let text = "| Name | Qty |\n|:-----|----:|\n| fib | 3 |\n";
    let (rows, r) = show(text, 40);
    assert_eq!(
        rows,
        [
            "  ┌──────┬─────┐",
            "  │ Name │ Qty │",
            "  ├──────┼─────┤",
            "  │ fib  │   3 │",
            "  └──────┴─────┘",
        ]
    );
    assert!(style_of(&r, "Name").add_modifier.contains(Modifier::BOLD));
    let header: Vec<String> = (1..=6).map(|i| format!("Column number {i}")).collect();
    let row: Vec<String> = (1..=6).map(|i| format!("value {i:>10}")).collect();
    let wide = format!(
        "| {} |\n|{}|\n| {} |\n",
        header.join(" | "),
        ["---"; 6].join("|"),
        row.join(" | ")
    );
    let (rows, _) = show(&wide, 80);
    assert_eq!(rows.len(), 6, "{rows:?}");
    assert_eq!(rows[0], "  Column number 1: value          1");
    assert_eq!(rows[5], "  Column number 6: value          6");
}

#[test]
fn links_append_their_url_without_hyperlinks_and_record_it_with() {
    let t = tokens();
    let text = "see [the docs](https://example.com/d) and <https://example.com/x>";
    let (rows, r) = show(text, 80);
    assert_eq!(
        rows,
        ["  see the docs (https://example.com/d) and https://example.com/x"]
    );
    assert_eq!(
        style_of(&r, "(https://example.com/d)").fg,
        Some(t.text_subtle)
    );
    let docs = style_of(&r, "the docs");
    assert_eq!(docs.fg, Some(t.md_link));
    assert!(docs.add_modifier.contains(Modifier::UNDERLINED));
    assert!(r.links.is_empty());

    let r = render(text, &Ctx::new(&t, 80, true));
    assert_eq!(rows_of(&r), ["  see the docs and https://example.com/x"]);
    assert_eq!(
        r.links[0],
        Link {
            row: 0,
            col: 6,
            text: "the docs".into(),
            url: "https://example.com/d".into(),
        }
    );
    let r = render("[x](javascript:alert(1))", &Ctx::new(&t, 80, true));
    assert!(r.links.is_empty());
    assert_eq!(rows_of(&r), ["  x (javascript:alert(1))"]);
}

fn rows_of(r: &Rendered) -> Vec<String> {
    rows(r)
}

#[test]
fn a_link_wrapped_over_two_rows_is_recorded_on_both() {
    let t = tokens();
    let r = render("[one two](https://e.com)", &Ctx::new(&t, 7, true));
    assert_eq!(rows(&r), ["  one", "  two"]);
    let cols: Vec<(usize, usize, &str)> = r
        .links
        .iter()
        .map(|l| (l.row, l.col, l.text.as_str()))
        .collect();
    assert_eq!(cols, [(0, 2, "one"), (1, 2, "two")]);
}

#[test]
fn raw_html_and_control_characters_are_literal_text() {
    let (rows, _) = show("<b>hi</b> \x1b]52;c;eA==\x07 ok", 60);
    assert_eq!(rows, ["  <b>hi</b> ]52;c;eA== ok"]);
}

#[test]
fn code_blocks_have_a_label_padding_and_surface() {
    let t = tokens();
    let (rows, r) = show("```text\nhello\n```\n", 20);
    assert_eq!(rows, ["    text", "    hello"]);
    assert_eq!(r.lines[0].to_string().chars().count(), 20);
    let hello = style_of(&r, "hello");
    assert_eq!(hello.bg, Some(t.surface));
    assert_eq!(style_of(&r, "text").fg, Some(t.text_subtle));
    assert_eq!(
        r.code,
        [Code {
            row: 0,
            span: 1,
            text: "hello".into()
        }]
    );
}

#[test]
fn long_code_lines_wrap_with_a_continuation_marker() {
    let t = tokens();
    let (rows, r) = show("```\nabcdefghij\n```\n", 10);
    assert_eq!(rows, ["", "    abcdef", "  ↪ ghij"]);
    assert_eq!(style_of(&r, "↪").fg, Some(t.text_subtle));
}

#[test]
fn unknown_languages_render_in_md_code() {
    let t = tokens();
    crate::highlight::preload();
    let (rows, r) = show("```klingon\nqapla'\n```\n", 30);
    assert_eq!(rows, ["    klingon", "    qapla'"]);
    assert_eq!(style_of(&r, "qapla").fg, Some(t.md_code));
}

#[test]
fn python_blocks_are_highlighted_by_scope() {
    let t = tokens();
    crate::highlight::preload();
    let (_, r) = show(
        "```python\n# comment\nfor i in range(10):\n    pass\n```\n",
        40,
    );
    let comment = style_of(&r, "# comment");
    assert_eq!(comment.fg, Some(t.syntax_comment));
    assert!(comment.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(comment.bg, Some(t.surface));
    assert_eq!(style_of(&r, "for").fg, Some(t.syntax_keyword));
}

#[test]
fn a_streaming_block_stays_plain_until_its_fence_closes() {
    let t = tokens();
    crate::highlight::preload();
    let cache = Cache::default();
    let ctx = Ctx::new(&t, 40, false);
    let mut text = String::from("Intro\n\n```python\n# comment\nfor i in range(3):\n");
    let open = cache.streaming("m1", &text, &ctx);
    assert_eq!(style_of(&open, "# comment").fg, Some(t.md_code));
    for partial in ["`", "``"] {
        let r = cache.streaming("m1", &format!("{text}{partial}"), &ctx);
        assert_eq!(
            r.lines.len(),
            open.lines.len(),
            "a partial fence adds no row"
        );
    }
    text.push_str("```");
    let closed = cache.streaming("m1", &text, &ctx);
    assert_eq!(closed.lines.len(), open.lines.len());
    assert_eq!(rows(&closed), rows(&open));
    assert_eq!(style_of(&closed, "# comment").fg, Some(t.syntax_comment));
    let done = cache.message(&format!("{text}\n"), &ctx);
    assert_eq!(rows(&done), rows(&closed));
}

#[test]
fn streaming_reparses_only_from_the_last_block() {
    let t = tokens();
    let cache = Cache::default();
    let ctx = Ctx::new(&t, 40, false);
    let first = "# One\n\nfirst paragraph\n\n- a\n";
    cache.streaming("m", first, &ctx);
    let (blocks, tail) = cache.advance("m", first);
    assert_eq!(blocks.len(), 2);
    assert_eq!(&first[tail..], "- a\n");
    let more = format!("{first}- b\n\nlast");
    let r = cache.streaming("m", &more, &ctx);
    let (blocks, tail) = cache.advance("m", &more);
    assert_eq!(blocks.len(), 3);
    assert_eq!(&more[tail..], "last");
    assert_eq!(rows(&r), rows(&render(&more, &ctx)));
    // A different text under the same ID starts over.
    let other = "fresh";
    assert_eq!(rows(&cache.streaming("m", other, &ctx)), ["  fresh"]);
}

#[test]
fn split_finds_top_level_blocks() {
    let text = "# H\n\npara\n\n- a\n- b\n\n```\ncode\n```\n\n---\n";
    let blocks: Vec<&str> = split(text)
        .into_iter()
        .map(|r| text[r].trim_end())
        .collect();
    assert_eq!(blocks, ["# H", "para", "- a\n- b", "```\ncode\n```", "---"]);
}

#[test]
fn code_blocks_lists_the_code_to_copy() {
    let text = "a\n\n```rust\nfn a() {}\n```\n\n- item\n\n  ```\n  nested\n  ```\n";
    assert_eq!(code_blocks(text), ["fn a() {}", "nested"]);
}

#[test]
fn osc8_accepts_only_printable_web_and_mail_urls() {
    assert!(osc8_safe("https://example.com/a?b=c#d"));
    assert!(osc8_safe("mailto:a@b.c"));
    assert!(!osc8_safe("https://"));
    assert!(!osc8_safe("file:///etc/passwd"));
    assert!(!osc8_safe("https://e.com/\x07"));
    assert!(!osc8_safe("https://e.com/é"));
    assert!(!osc8_safe(&format!("https://e.com/{}", "a".repeat(2048))));
}

/// Regression: a multibyte character across the scheme's byte length panicked the
/// scheme check, so model text such as `[x](mailt€abc)` crashed the renderer.
#[test]
fn osc8_check_never_panics_on_a_multibyte_scheme() {
    assert!(!osc8_safe("mailt€x"));
    assert!(!osc8_safe("http:/€x"));
    assert!(!osc8_safe("https:/€x"));
    let t = tokens();
    let r = render("[x](mailt€abc) [y](http:/€x)", &Ctx::new(&t, 80, true));
    assert!(r.links.is_empty());
}

/// Regression: the code a block copies is cleaned to what the screen draws, so an ESC
/// or a bracketed-paste end marker hidden in model output never reaches the clipboard.
#[test]
fn copied_code_blocks_drop_controls_the_screen_never_drew() {
    let text = "```sh\necho ok\x1b[201~\r\nrm -rf ~\n```\n";
    let blocks = code_blocks(text);
    assert_eq!(
        crate::osc::clipboard_safe(&blocks[0]),
        "echo ok[201~\nrm -rf ~"
    );
}

#[test]
fn monochrome_tokens_leave_cells_uncolored() {
    let t = crate::theme::Catalog::default()
        .get("cyber")
        .resolve(false, crate::theme::Depth::None);
    let r = render(
        "# H\n\n```python\nx = 1\n```\n\n> q",
        &Ctx::new(&t, 30, false),
    );
    for span in r.lines.iter().flat_map(|l| l.spans.iter()) {
        assert!(
            matches!(span.style.fg, None | Some(Color::Reset))
                && matches!(span.style.bg, None | Some(Color::Reset)),
            "{span:?}"
        );
    }
}
