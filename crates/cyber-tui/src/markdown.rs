//! Markdown rendering (`tui` → Markdown rendering, Syntax-highlighted code blocks):
//! CommonMark plus GitHub tables, task lists and strikethrough, parsed with
//! `pulldown-cmark` and laid out at the transcript width with hanging indents.
//!
//! Text is split into top-level blocks that are rendered and cached one by one, so a
//! streaming message re-parses only its last, unfinished block. The output is plain
//! ratatui lines: the text is untrusted model output, and ratatui drops control
//! characters from spans, so nothing in it reaches the terminal as an escape sequence.
//! Only [`Link`]s that pass [`osc8_safe`] are ever written as OSC 8 hyperlinks.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::highlight;
use crate::theme::Tokens;

/// The left margin of every Markdown row.
const MARGIN: &str = "  ";
/// The padding inside a code block, left of the code.
const CODE_PAD: &str = "  ";
const CONTINUED: &str = "↪ ";
/// Cached blocks or messages beyond this are dropped all at once.
const CACHE_LIMIT: usize = 4_096;

/// A link on a rendered row, at display column `col`.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub url: String,
}

/// A code block: the row of its language label and the code to copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Code {
    pub row: usize,
    /// The index of the label row's padding span, which a selection marker replaces.
    pub span: usize,
    pub text: String,
}

/// Rows that fit the width they were rendered for, with their links and code blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<Link>,
    pub code: Vec<Code>,
}

impl Rendered {
    /// Append `other` below these rows.
    pub fn append(&mut self, other: &Rendered) {
        let offset = self.lines.len();
        self.lines.extend(other.lines.iter().cloned());
        self.links.extend(other.links.iter().map(|l| Link {
            row: l.row + offset,
            ..l.clone()
        }));
        self.code.extend(other.code.iter().map(|c| Code {
            row: c.row + offset,
            ..c.clone()
        }));
    }
}

/// What a render depends on besides the text.
pub struct Ctx<'a> {
    pub t: &'a Tokens,
    pub width: usize,
    /// The terminal shows OSC 8 hyperlinks, so URLs are not appended to link text.
    pub hyperlinks: bool,
    theme_key: u64,
}

impl<'a> Ctx<'a> {
    pub fn new(t: &'a Tokens, width: usize, hyperlinks: bool) -> Self {
        Self {
            t,
            width,
            hyperlinks,
            theme_key: hash(&format!("{t:?}")),
        }
    }

    /// A cache key for everything but the text.
    pub fn key(&self) -> u64 {
        hash(&(
            self.width,
            self.theme_key,
            self.hyperlinks,
            highlight::ready(),
        ))
    }
}

fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// The byte ranges of `text`'s top-level blocks.
pub fn split(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    start = range.start;
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push(start..range.end);
                }
            }
            _ if depth == 0 => out.push(range),
            _ => {}
        }
    }
    out
}

/// The text of every code block in `text`, as `y` copies it.
pub fn code_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => current = Some(String::new()),
            Event::Text(t) => {
                if let Some(code) = &mut current {
                    code.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let code = current.take().unwrap_or_default();
                out.push(code.strip_suffix('\n').unwrap_or(&code).to_owned());
            }
            _ => {}
        }
    }
    out
}

/// Render a finished document without caching.
#[cfg(test)]
pub fn render(text: &str, ctx: &Ctx) -> Rendered {
    let mut out = Rendered::default();
    for range in split(text) {
        join(&mut out, &block(&text[range], ctx, false));
    }
    out
}

/// Blocks are separated by one blank row.
fn join(out: &mut Rendered, block: &Rendered) {
    if !out.lines.is_empty() && !block.lines.is_empty() {
        out.lines.push(Line::default());
    }
    out.append(block);
}

/// The block cache and the streaming split of each message.
#[derive(Default)]
pub struct Cache {
    blocks: Mutex<HashMap<u64, Arc<Rendered>>>,
    messages: Mutex<HashMap<u64, Arc<Rendered>>>,
    streams: Mutex<HashMap<String, Stream>>,
}

/// Where a streaming message's finished blocks end.
#[derive(Default)]
struct Stream {
    blocks: Vec<Range<usize>>,
    /// The start of the last, unfinished block.
    tail: usize,
    prefix: u64,
}

fn bounded<K, V>(map: &mut HashMap<K, V>) {
    if map.len() >= CACHE_LIMIT {
        map.clear();
    }
}

impl Cache {
    /// A finished message.
    pub fn message(&self, text: &str, ctx: &Ctx) -> Arc<Rendered> {
        let key = hash(&(text, ctx.key()));
        if let Some(hit) = lock(&self.messages).get(&key) {
            return hit.clone();
        }
        let mut out = Rendered::default();
        for range in split(text) {
            join(&mut out, &self.block(&text[range], ctx));
        }
        let out = Arc::new(out);
        let mut messages = lock(&self.messages);
        bounded(&mut messages);
        messages.insert(key, out.clone());
        out
    }

    /// A message still streaming as `id`: finished blocks come from the cache and only
    /// the text from the start of the last block is parsed again.
    pub fn streaming(&self, id: &str, text: &str, ctx: &Ctx) -> Rendered {
        let (blocks, tail) = self.advance(id, text);
        let mut out = Rendered::default();
        for range in blocks {
            join(&mut out, &self.block(&text[range], ctx));
        }
        if !text[tail..].trim().is_empty() {
            join(&mut out, &block(&text[tail..], ctx, true));
        }
        out
    }

    fn advance(&self, id: &str, text: &str) -> (Vec<Range<usize>>, usize) {
        let mut streams = lock(&self.streams);
        if !streams.contains_key(id) {
            bounded(&mut streams);
        }
        let stream = streams.entry(id.to_owned()).or_default();
        let kept = text
            .get(..stream.tail)
            .is_some_and(|prefix| hash(&prefix) == stream.prefix);
        if !kept {
            *stream = Stream::default();
            stream.prefix = hash(&"");
        }
        let ranges = split(&text[stream.tail..]);
        if let Some((last, done)) = ranges.split_last() {
            let base = stream.tail;
            stream
                .blocks
                .extend(done.iter().map(|r| r.start + base..r.end + base));
            stream.tail = base + last.start;
            stream.prefix = hash(&&text[..stream.tail]);
        }
        (stream.blocks.clone(), stream.tail)
    }

    fn block(&self, src: &str, ctx: &Ctx) -> Arc<Rendered> {
        self.memo(&(src, ctx.key()), || block(src, ctx, false))
    }

    /// `render()`'s rows, cached under `key`.
    pub fn memo(&self, key: &impl Hash, render: impl FnOnce() -> Rendered) -> Arc<Rendered> {
        let key = hash(key);
        if let Some(hit) = lock(&self.blocks).get(&key) {
            return hit.clone();
        }
        let rendered = Arc::new(render());
        let mut blocks = lock(&self.blocks);
        bounded(&mut blocks);
        blocks.insert(key, rendered.clone());
        rendered
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Render one top-level block. `open` marks the last block of a streaming message: its
/// code stays plain until the closing fence arrives.
fn block(src: &str, ctx: &Ctx, open: bool) -> Rendered {
    let src = if open { trim_partial_fence(src) } else { src };
    let mut r = Renderer::new(ctx, src, open);
    for (event, range) in Parser::new_ext(src, options()).into_offset_iter() {
        r.event(event, range);
    }
    r.flush();
    r.out
}

/// Drop a last, unterminated line that is the start of a closing fence (```` `` ````), so
/// a fence arriving a character at a time never shows as a code row.
fn trim_partial_fence(src: &str) -> &str {
    let Some(open) = fence_of(src) else {
        return src;
    };
    let cut = src.rfind('\n').map_or(0, |i| i + 1);
    let last = src[cut..].trim();
    let partial = cut > 0
        && !last.is_empty()
        && last.chars().all(|c| c == open.0)
        && last.chars().count() < open.1;
    if partial { &src[..cut] } else { src }
}

/// The fence character and length a block starts with.
fn fence_of(src: &str) -> Option<(char, usize)> {
    let first = src.trim_start().lines().next()?.trim_start();
    let c = first.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = first.chars().take_while(|&x| x == c).count();
    (n >= 3).then_some((c, n))
}

/// Whether a fenced block's source ends with its closing fence.
fn fence_closed(src: &str) -> bool {
    let Some((c, n)) = fence_of(src) else {
        return false;
    };
    let mut lines = src.trim_end().lines();
    lines.next();
    lines
        .last()
        .map(str::trim)
        .is_some_and(|l| l.chars().count() >= n && l.chars().all(|x| x == c))
}

/// A URL that may be written inside an OSC 8 sequence: a web or mail link of printable
/// ASCII without spaces, so it cannot end the sequence or smuggle another one.
pub fn osc8_safe(url: &str) -> bool {
    let scheme = ["https://", "http://", "mailto:"].iter().any(|s| {
        url.len() > s.len() && url.as_bytes()[..s.len()].eq_ignore_ascii_case(s.as_bytes())
    });
    scheme && url.len() <= 2048 && url.bytes().all(|b| (0x21..0x7f).contains(&b))
}

/// One character of inline content.
#[derive(Clone)]
struct Cell {
    ch: char,
    style: Style,
    link: Option<usize>,
}

enum Container {
    Quote,
    Item { marker: String, used: bool },
}

struct CodeState {
    info: String,
    text: String,
}

#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Cell>>>,
    row: Vec<Vec<Cell>>,
}

struct Renderer<'c, 's> {
    ctx: &'c Ctx<'c>,
    src: &'s str,
    open: bool,
    out: Rendered,
    containers: Vec<Container>,
    /// The next number of each open list, `None` for bullets.
    lists: Vec<Option<u64>>,
    inline: Vec<Cell>,
    styles: Vec<Style>,
    urls: Vec<String>,
    link: Option<(usize, String)>,
    heading: Option<HeadingLevel>,
    code: Option<CodeState>,
    table: Option<Table>,
    /// A blank row is due before the next block.
    blank: bool,
}

impl<'c, 's> Renderer<'c, 's> {
    fn new(ctx: &'c Ctx<'c>, src: &'s str, open: bool) -> Self {
        Self {
            ctx,
            src,
            open,
            out: Rendered::default(),
            containers: Vec::new(),
            lists: Vec::new(),
            inline: Vec::new(),
            styles: Vec::new(),
            urls: Vec::new(),
            link: None,
            heading: None,
            code: None,
            table: None,
            blank: false,
        }
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag, range),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.push_text(&code, self.style().fg(self.ctx.t.md_code)),
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            Event::InlineMath(m) | Event::DisplayMath(m) => self.text(&m),
            Event::FootnoteReference(name) => self.text(&format!("[^{name}]")),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.push_text("\n", Style::default()),
            Event::Rule => self.rule(),
            Event::TaskListMarker(done) => self.task(done),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.begin_block(),
            Tag::Heading { level, .. } => {
                self.begin_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.begin_block();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.begin_block();
                let info = match kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeState {
                    info,
                    text: String::new(),
                });
            }
            Tag::List(first) => {
                self.begin_block();
                self.lists.push(first);
            }
            Tag::Item => self.item(),
            Tag::Table(aligns) => {
                self.begin_block();
                self.table = Some(Table {
                    aligns,
                    ..Table::default()
                });
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self
                .styles
                .push(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self
                .styles
                .push(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self
                .styles
                .push(Style::default().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.styles.push(
                    Style::default()
                        .fg(self.ctx.t.md_link)
                        .add_modifier(Modifier::UNDERLINED),
                );
                self.link = Some((self.inline.len(), dest_url.to_string()));
            }
            Tag::Image { .. } => {
                self.styles
                    .push(Style::default().fg(self.ctx.t.text_subtle));
                self.push_text("[image: ", self.style());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.end_block(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
                self.blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.blank = true;
            }
            TagEnd::CodeBlock => self.end_code(range),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                self.blank |= !self.in_item();
            }
            TagEnd::Item => self.end_item(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(std::mem::take(&mut table.row));
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::Table => self.end_table(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => {
                self.push_text("]", self.style());
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn in_item(&self) -> bool {
        self.containers
            .iter()
            .any(|c| matches!(c, Container::Item { .. }))
    }

    /// Flush pending text and put the due blank row before a new block.
    fn begin_block(&mut self) {
        self.flush();
        if self.blank && !self.out.lines.is_empty() {
            let (_, rest) = self.prefixes();
            let gutters: Vec<Span<'static>> = rest
                .into_iter()
                .filter(|s| !s.content.trim().is_empty())
                .collect();
            let mut spans = vec![Span::raw(MARGIN)];
            spans.extend(gutters);
            self.out.lines.push(Line::from(spans));
        }
        self.blank = false;
    }

    fn end_block(&mut self) {
        self.flush();
        self.blank = true;
    }

    fn item(&mut self) {
        self.flush();
        let marker = match self.lists.last_mut() {
            Some(Some(n)) => {
                let marker = format!("{n}. ");
                *n += 1;
                marker
            }
            _ => "• ".to_owned(),
        };
        self.containers.push(Container::Item {
            marker,
            used: false,
        });
    }

    fn end_item(&mut self) {
        self.flush();
        if let Some(Container::Item { used: false, .. }) = self.containers.last() {
            let (first, _) = self.prefixes();
            self.out.lines.push(Line::from(first));
        }
        self.containers.pop();
    }

    fn task(&mut self, done: bool) {
        let mark = if done { "☑ " } else { "☐ " };
        if let Some(Container::Item { marker, .. }) = self.containers.last_mut()
            && marker == "• "
        {
            *marker = mark.to_owned();
            return;
        }
        self.push_text(mark, Style::default().fg(self.ctx.t.text));
    }

    fn end_link(&mut self) {
        self.styles.pop();
        let Some((start, url)) = self.link.take() else {
            return;
        };
        let text: String = self.inline[start..].iter().map(|c| c.ch).collect();
        if self.ctx.hyperlinks && self.table.is_none() && osc8_safe(&url) {
            let id = self.urls.len();
            self.urls.push(url);
            for cell in &mut self.inline[start..] {
                cell.link = Some(id);
            }
        } else if !url.is_empty() && text != url && !url.starts_with('#') {
            let style = Style::default().fg(self.ctx.t.text_subtle);
            self.push_text(&format!(" ({url})"), style);
        }
    }

    /// The style of inline text here: the block's base with every open modifier.
    fn style(&self) -> Style {
        let t = self.ctx.t;
        let base = if self.heading.is_some() {
            Style::default()
                .fg(t.md_heading)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text)
        };
        self.styles.iter().fold(base, |s, p| s.patch(*p))
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code {
            code.text.push_str(text);
            return;
        }
        self.push_text(text, self.style());
    }

    fn push_text(&mut self, text: &str, style: Style) {
        for ch in text.chars() {
            match ch {
                '\t' => self.inline.extend(std::iter::repeat_n(cell(' ', style), 4)),
                '\n' => self.inline.push(cell('\n', style)),
                c if c.is_control() => {}
                c => self.inline.push(cell(c, style)),
            }
        }
    }

    /// The first-row and continuation prefixes of the open containers.
    fn prefixes(&self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let t = self.ctx.t;
        let mut first = vec![Span::raw(MARGIN)];
        let mut rest = vec![Span::raw(MARGIN)];
        for container in &self.containers {
            match container {
                Container::Quote => {
                    let gutter = Span::styled("▎ ", Style::default().fg(t.md_quote));
                    first.push(gutter.clone());
                    rest.push(gutter);
                }
                Container::Item { marker, used } => {
                    let pad = " ".repeat(width_of(marker));
                    first.push(if *used {
                        Span::raw(pad.clone())
                    } else {
                        Span::styled(marker.clone(), Style::default().fg(t.text_muted))
                    });
                    rest.push(Span::raw(pad));
                }
            }
        }
        (first, rest)
    }

    fn mark_used(&mut self) {
        for container in &mut self.containers {
            if let Container::Item { used, .. } = container {
                *used = true;
            }
        }
    }

    /// Wrap the pending inline text under the containers' prefixes.
    fn flush(&mut self) {
        if self.inline.is_empty() || self.table.is_some() {
            return;
        }
        let cells = std::mem::take(&mut self.inline);
        let (first, rest) = self.prefixes();
        self.emit_wrapped(&cells, first, rest);
        self.mark_used();
    }

    fn emit_wrapped(
        &mut self,
        cells: &[Cell],
        first: Vec<Span<'static>>,
        rest: Vec<Span<'static>>,
    ) {
        let indent = spans_width(&rest);
        let avail = self.ctx.width.saturating_sub(indent).max(1);
        for (i, row) in wrap_words(cells, avail).into_iter().enumerate() {
            let prefix = if i == 0 { first.clone() } else { rest.clone() };
            self.emit_cells(prefix, &cells[row]);
        }
    }

    /// One row: `prefix`, then the cells merged into spans, recording links.
    fn emit_cells(&mut self, prefix: Vec<Span<'static>>, cells: &[Cell]) {
        let row = self.out.lines.len();
        let mut col = spans_width(&prefix);
        let mut spans = prefix;
        for run in runs(cells) {
            let text: String = run.iter().map(|c| c.ch).collect();
            let width = width_of(&text);
            if let Some(id) = run[0].link {
                self.out.links.push(Link {
                    row,
                    col,
                    text: text.clone(),
                    url: self.urls[id].clone(),
                });
            }
            spans.push(Span::styled(text, run[0].style));
            col += width;
        }
        self.out.lines.push(Line::from(spans));
    }

    fn rule(&mut self) {
        self.begin_block();
        let (first, _) = self.prefixes();
        let n = self.ctx.width.saturating_sub(spans_width(&first)).max(1);
        let mut spans = first;
        spans.push(Span::styled(
            "─".repeat(n),
            Style::default().fg(self.ctx.t.md_rule),
        ));
        self.out.lines.push(Line::from(spans));
        self.mark_used();
        self.blank = true;
    }

    fn end_code(&mut self, range: Range<usize>) {
        let Some(code) = self.code.take() else {
            return;
        };
        let closed = !self.open || fence_closed(&self.src[range]);
        let lang = highlight::fence_language(&code.info).unwrap_or_default();
        let lines = code_lines(&code, closed, self.ctx.t);
        self.code_block(
            lang,
            &lines,
            code.text.strip_suffix('\n').unwrap_or(&code.text),
        );
        self.blank = true;
    }

    /// A label row and the code rows, on `surface`, wrapped with `↪`.
    fn code_block(&mut self, lang: &str, lines: &[Vec<Cell>], text: &str) {
        let t = self.ctx.t;
        let bg = Style::default().bg(t.surface);
        let (first, rest) = self.prefixes();
        let indent = spans_width(&rest);
        let fill = self.ctx.width.saturating_sub(indent);
        let room = fill.saturating_sub(CODE_PAD.len());
        let lang: String = lang.chars().take(room).collect();
        let label = format!("{lang:<room$}");
        let span = first.len();
        let mut row = first;
        row.push(Span::styled(CODE_PAD, bg));
        row.push(Span::styled(label, bg.fg(t.text_subtle)));
        self.out.code.push(Code {
            row: self.out.lines.len(),
            span,
            text: text.to_owned(),
        });
        self.out.lines.push(Line::from(row));
        self.mark_used();
        let avail = fill.saturating_sub(CODE_PAD.len()).max(1);
        for line in lines {
            for (i, chunk) in wrap_chars(line, avail).into_iter().enumerate() {
                let mut spans = rest.clone();
                spans.push(if i == 0 {
                    Span::styled(CODE_PAD, bg)
                } else {
                    Span::styled(CONTINUED, bg.fg(t.text_subtle))
                });
                let used = cells_width(&line[chunk.clone()]);
                spans.extend(runs(&line[chunk]).map(|run| {
                    Span::styled(run.iter().map(|c| c.ch).collect::<String>(), run[0].style)
                }));
                spans.push(Span::styled(" ".repeat(avail.saturating_sub(used)), bg));
                self.out.lines.push(Line::from(spans));
            }
        }
    }

    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let (first, rest) = self.prefixes();
        let widths = column_widths(&table.rows);
        let needed = widths.iter().sum::<usize>() + 3 * widths.len() + 1;
        if spans_width(&rest) + needed > self.ctx.width {
            self.table_fallback(&table, first, rest);
        } else {
            self.table_grid(&table, &widths, first, rest);
        }
        self.mark_used();
        self.blank = true;
    }

    /// One `header: value` row per cell, a blank row between table rows.
    fn table_fallback(
        &mut self,
        table: &Table,
        first: Vec<Span<'static>>,
        rest: Vec<Span<'static>>,
    ) {
        let Some((head, body)) = table.rows.split_first() else {
            return;
        };
        let bold = Style::default()
            .fg(self.ctx.t.text)
            .add_modifier(Modifier::BOLD);
        let mut prefix = first;
        for (i, row) in body.iter().enumerate() {
            if i > 0 {
                self.out.lines.push(Line::from(vec![Span::raw(MARGIN)]));
            }
            for (c, value) in row.iter().enumerate() {
                let header: String = head
                    .get(c)
                    .map_or(String::new(), |h| h.iter().map(|x| x.ch).collect());
                let mut cells: Vec<Cell> = format!("{header}: ")
                    .chars()
                    .map(|ch| cell(ch, bold))
                    .collect();
                cells.extend(value.iter().cloned());
                self.emit_wrapped(&cells, prefix.clone(), rest.clone());
                prefix = rest.clone();
            }
        }
    }

    fn table_grid(
        &mut self,
        table: &Table,
        widths: &[usize],
        first: Vec<Span<'static>>,
        rest: Vec<Span<'static>>,
    ) {
        let border = Style::default().fg(self.ctx.t.border);
        let rule = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            Span::styled(format!("{l}{}{r}", inner.join(m)), border)
        };
        self.push_row(first, vec![rule("┌", "┬", "┐")]);
        for (i, row) in table.rows.iter().enumerate() {
            let mut spans = vec![Span::styled("│", border)];
            for (c, width) in widths.iter().enumerate() {
                let empty = Vec::new();
                let value = row.get(c).unwrap_or(&empty);
                let align = table.aligns.get(c).copied().unwrap_or(Alignment::None);
                let (left, right) = padding(align, width - cells_width(value));
                spans.push(Span::raw(" ".repeat(left + 1)));
                spans.extend(runs(value).map(|run| {
                    let style = if i == 0 {
                        run[0].style.add_modifier(Modifier::BOLD)
                    } else {
                        run[0].style
                    };
                    Span::styled(run.iter().map(|x| x.ch).collect::<String>(), style)
                }));
                spans.push(Span::raw(" ".repeat(right + 1)));
                spans.push(Span::styled("│", border));
            }
            self.push_row(rest.clone(), spans);
            if i == 0 && table.rows.len() > 1 {
                self.push_row(rest.clone(), vec![rule("├", "┼", "┤")]);
            }
        }
        self.push_row(rest, vec![rule("└", "┴", "┘")]);
    }

    fn push_row(&mut self, prefix: Vec<Span<'static>>, spans: Vec<Span<'static>>) {
        let mut row = prefix;
        row.extend(spans);
        self.out.lines.push(Line::from(row));
    }
}

fn cell(ch: char, style: Style) -> Cell {
    Cell {
        ch,
        style,
        link: None,
    }
}

/// The code's rows: highlighted when the block is finished and its language known,
/// otherwise plain `md_code`.
fn code_lines(code: &CodeState, closed: bool, t: &Tokens) -> Vec<Vec<Cell>> {
    let text = code.text.replace('\t', "    ");
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let bg = t.surface;
    let styled = closed
        .then(|| {
            let lang = highlight::fence_language(&code.info)?;
            let set = highlight::syntaxes()?;
            let syntax = highlight::for_fence(set, lang)?;
            highlight::highlight(set, syntax, text, t)
        })
        .flatten();
    match styled {
        Some(lines) => lines
            .into_iter()
            .map(|runs| {
                runs.into_iter()
                    .flat_map(|(style, s)| {
                        let style = style.bg(bg);
                        s.chars()
                            .filter(|c| !c.is_control())
                            .map(move |ch| cell(ch, style))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .collect(),
        None => {
            let style = Style::default().fg(t.md_code).bg(bg);
            text.split('\n')
                .map(|l| {
                    l.chars()
                        .filter(|c| !c.is_control())
                        .map(|ch| cell(ch, style))
                        .collect()
                })
                .collect()
        }
    }
}

fn padding(align: Alignment, extra: usize) -> (usize, usize) {
    match align {
        Alignment::Right => (extra, 0),
        Alignment::Center => (extra / 2, extra - extra / 2),
        Alignment::Left | Alignment::None => (0, extra),
    }
}

fn column_widths(rows: &[Vec<Vec<Cell>>]) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|cells| cells_width(cells))
                .max()
                .unwrap_or(0)
        })
        .collect()
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn width_of(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| char_width(c.ch)).sum()
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| width_of(&s.content)).sum()
}

/// Runs of cells sharing a style and link.
fn runs(cells: &[Cell]) -> impl Iterator<Item = &[Cell]> {
    cells.chunk_by(|a, b| a.style == b.style && a.link == b.link)
}

/// Rows of at most `avail` columns, broken after spaces where possible; `\n` forces a
/// break. Spaces at a break are dropped.
fn wrap_words(cells: &[Cell], avail: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let mut space = None;
    let mut i = 0;
    while i < cells.len() {
        let c = cells[i].ch;
        if c == '\n' {
            rows.push(start..i);
            (start, used, space) = (i + 1, 0, None);
        } else if used + char_width(c) > avail && i > start {
            let at = space.filter(|&s| s > start).unwrap_or(i);
            rows.push(start..at);
            start = if cells[at].ch == ' ' { at + 1 } else { at };
            space = None;
            i = i.max(start);
            used = cells_width(&cells[start..i]);
            continue;
        } else {
            if c == ' ' {
                space = Some(i);
            }
            used += char_width(c);
        }
        i += 1;
    }
    if start < cells.len() || rows.is_empty() {
        rows.push(start..cells.len());
    }
    rows
}

/// Rows of at most `avail` columns, broken at any character (code is never re-flowed).
fn wrap_chars(cells: &[Cell], avail: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (i, c) in cells.iter().enumerate() {
        let w = char_width(c.ch);
        if used + w > avail && i > start {
            rows.push(start..i);
            (start, used) = (i, 0);
        }
        used += w;
    }
    rows.push(start..cells.len());
    rows
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
