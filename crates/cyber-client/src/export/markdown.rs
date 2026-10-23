//! Markdown from the Session (prompts, responses) as inert HTML. Only the tags this
//! renderer writes reach the page: raw HTML is shown as escaped text, links and images
//! become text followed by their URL, and code blocks are `<pre><code>`. No attribute
//! carries Session content.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

use super::html::escape;

/// `text` rendered as HTML that cannot load, link or run anything.
pub fn to_html(text: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut writer = Writer::default();
    for event in Parser::new_ext(text, options) {
        writer.event(event);
    }
    writer.out
}

#[derive(Default)]
struct Writer {
    out: String,
    /// The URL to show after each open link or image; `None` for an autolink, whose
    /// text already is the URL.
    targets: Vec<Option<String>>,
    in_head: bool,
}

impl Writer {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text)
            | Event::Html(text)
            | Event::InlineHtml(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => self.out.push_str(&escape(&text)),
            Event::Code(code) => {
                self.out.push_str("<code>");
                self.out.push_str(&escape(&code));
                self.out.push_str("</code>");
            }
            Event::FootnoteReference(name) => {
                self.out.push_str(&format!("[^{}]", escape(&name)));
            }
            Event::SoftBreak => self.out.push('\n'),
            Event::HardBreak => self.out.push_str("<br>\n"),
            Event::Rule => self.out.push_str("<hr>\n"),
            Event::TaskListMarker(done) => self.out.push_str(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let open = match tag {
            Tag::Paragraph => "<p>".to_owned(),
            Tag::HtmlBlock => "<p class=\"raw\">".to_owned(),
            Tag::Heading { level, .. } => format!("<h{}>", heading(level)),
            Tag::BlockQuote(_) => "<blockquote>".to_owned(),
            Tag::CodeBlock(CodeBlockKind::Fenced(_) | CodeBlockKind::Indented) => {
                "<pre><code>".to_owned()
            }
            Tag::List(Some(first)) => format!("<ol start=\"{first}\">"),
            Tag::List(None) => "<ul>".to_owned(),
            Tag::Item => "<li>".to_owned(),
            Tag::FootnoteDefinition(name) => {
                format!("<div class=\"footnote\">[^{}]: ", escape(&name))
            }
            Tag::Table(_) => "<table>".to_owned(),
            Tag::TableHead => {
                self.in_head = true;
                "<thead><tr>".to_owned()
            }
            Tag::TableRow => "<tr>".to_owned(),
            Tag::TableCell => if self.in_head { "<th>" } else { "<td>" }.to_owned(),
            Tag::Emphasis => "<em>".to_owned(),
            Tag::Strong => "<strong>".to_owned(),
            Tag::Strikethrough => "<del>".to_owned(),
            Tag::Superscript => "<sup>".to_owned(),
            Tag::Subscript => "<sub>".to_owned(),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let shown = !matches!(link_type, LinkType::Autolink | LinkType::Email);
                self.targets.push(shown.then(|| dest_url.to_string()));
                "<span class=\"link\">".to_owned()
            }
            Tag::Image { dest_url, .. } => {
                self.targets.push(Some(dest_url.to_string()));
                "<span class=\"image\">[image: ".to_owned()
            }
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => String::new(),
        };
        self.out.push_str(&open);
    }

    fn end(&mut self, tag: TagEnd) {
        let close = match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => "</p>\n".to_owned(),
            TagEnd::Heading(level) => format!("</h{}>\n", heading(level)),
            TagEnd::BlockQuote(_) => "</blockquote>\n".to_owned(),
            TagEnd::CodeBlock => "</code></pre>\n".to_owned(),
            TagEnd::List(true) => "</ol>\n".to_owned(),
            TagEnd::List(false) => "</ul>\n".to_owned(),
            TagEnd::Item => "</li>\n".to_owned(),
            TagEnd::FootnoteDefinition => "</div>\n".to_owned(),
            TagEnd::Table => "</tbody></table>\n".to_owned(),
            TagEnd::TableHead => {
                self.in_head = false;
                "</tr></thead><tbody>".to_owned()
            }
            TagEnd::TableRow => "</tr>".to_owned(),
            TagEnd::TableCell => if self.in_head { "</th>" } else { "</td>" }.to_owned(),
            TagEnd::Emphasis => "</em>".to_owned(),
            TagEnd::Strong => "</strong>".to_owned(),
            TagEnd::Strikethrough => "</del>".to_owned(),
            TagEnd::Superscript => "</sup>".to_owned(),
            TagEnd::Subscript => "</sub>".to_owned(),
            TagEnd::Link => format!("</span>{}", self.target()),
            TagEnd::Image => format!("]</span>{}", self.target()),
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => String::new(),
        };
        self.out.push_str(&close);
    }

    /// The closing link's URL as text.
    fn target(&mut self) -> String {
        match self.targets.pop().flatten() {
            Some(url) if !url.is_empty() => {
                format!(" <span class=\"url\">({})</span>", escape(&url))
            }
            _ => String::new(),
        }
    }
}

fn heading(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_and_links_are_inert_text() {
        let html = to_html(
            "<img src=x onerror=alert(1)>\n\n[x](javascript:alert(1)) ![pic](https://e.com/p.png)\n\n<div>block</div>\n",
        );
        assert!(
            html.contains("&lt;img src=x onerror=alert(1)&gt;"),
            "{html}"
        );
        assert!(html.contains("&lt;div&gt;block&lt;/div&gt;"), "{html}");
        assert!(
            html.contains(
                "<span class=\"link\">x</span> <span class=\"url\">(javascript:alert(1))</span>"
            ),
            "{html}"
        );
        assert!(html.contains("[image: pic]"), "{html}");
        for forbidden in ["<img", "<a ", "href", "src=\"", "<div"] {
            assert!(!html.contains(forbidden), "{forbidden}: {html}");
        }
    }

    #[test]
    fn markdown_structure_renders() {
        let html = to_html(
            "# Title\n\n**b** *i* ~~s~~ `c`\n\n- [x] done\n\n1. one\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1 < 2;\n```\n\n<https://e.com>\n",
        );
        for part in [
            "<h1>Title</h1>",
            "<strong>b</strong>",
            "<em>i</em>",
            "<del>s</del>",
            "<code>c</code>",
            "<li>[x] done</li>",
            "<ol start=\"1\">",
            "<thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
            "<pre><code>let x = 1 &lt; 2;\n</code></pre>",
            "<span class=\"link\">https://e.com</span></p>",
        ] {
            assert!(html.contains(part), "{part}: {html}");
        }
    }
}
