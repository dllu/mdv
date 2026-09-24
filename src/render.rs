//! Markdown event stream -> styled terminal lines.

use std::collections::HashMap;
use std::path::PathBuf;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag,
    TagEnd,
};

use crate::highlight::Highlighter;
use crate::style::{Color, ColorMode, Span, Style, Writer, spans_width, str_width};
use crate::wrap::{Line, hard_wrap, wrap};

pub struct Config {
    pub width: usize,
    pub mode: ColorMode,
    pub hyperlinks: bool,
    pub light: bool,
    pub theme: String,
    /// Directory used to resolve relative link targets into file:// URLs.
    pub base_dir: Option<PathBuf>,
}

struct Palette {
    h: [Style; 6],
    rule: Style,
    link: Style,
    code: Style,
    quote: Style,
    marker: Style,
    dim: Style,
    border: Style,
    footnote: Style,
    checked: Style,
}

impl Palette {
    fn new(light: bool) -> Self {
        let a = Color::Ansi;
        let (code_fg, code_bg) = if light {
            (Color::Rgb(0xb3, 0x1d, 0x28), Color::Rgb(0xee, 0xee, 0xee))
        } else {
            (Color::Rgb(0xff, 0xa6, 0x57), Color::Rgb(0x30, 0x30, 0x30))
        };
        Palette {
            h: [
                Style::fg(a(13)).bold(),
                Style::fg(a(14)).bold(),
                Style::fg(a(12)).bold(),
                Style::fg(a(10)).bold(),
                Style::fg(a(11)).bold(),
                Style::default().bold().dim(),
            ],
            rule: Style::fg(a(8)),
            link: Style { fg: Some(a(if light { 4 } else { 12 })), underline: true, ..Default::default() },
            code: Style::fg(code_fg).on(code_bg),
            quote: Style::fg(a(8)),
            marker: Style::fg(a(if light { 5 } else { 13 })),
            dim: Style::default().dim(),
            border: Style::fg(a(8)),
            footnote: Style::fg(a(6)),
            checked: Style::fg(a(10)),
        }
    }
}

enum Container {
    Quote(Style),
    /// List item, footnote or definition body: a marker on the first line, indent afterwards.
    Item { marker: Vec<Span>, width: usize, used: bool },
}

struct List {
    next: Option<u64>,
    tight: bool,
    width: usize,
}

struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span>>>,
    head_rows: usize,
}

struct LinkInfo {
    id: u32,
    url: String,
    start: usize,
    show_url: bool,
}

pub struct Renderer<'e> {
    cfg: Config,
    pal: Palette,
    events: Vec<Event<'e>>,
    w: Writer,
    out: String,
    containers: Vec<Container>,
    lists: Vec<List>,
    inline: Vec<Span>,
    styles: Vec<Style>,
    pending_blank: bool,
    wrote_any: bool,
    code: Option<(String, String)>,
    html: Option<String>,
    table: Option<Table>,
    heading: Option<HeadingLevel>,
    links: Vec<LinkInfo>,
    html_tags: Vec<String>,
    footnotes: HashMap<String, usize>,
    highlighter: Option<Highlighter>,
    skip_depth: usize,
}

pub fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
}

pub fn render(src: &str, cfg: Config) -> Result<String, String> {
    let events: Vec<Event> = Parser::new_ext(src, parser_options()).collect();
    let has_code = cfg.mode != ColorMode::None
        && events.iter().any(|e| matches!(e, Event::Start(Tag::CodeBlock(_))));
    let highlighter = if has_code { Some(Highlighter::new(&cfg.theme)?) } else { None };
    let mut r = Renderer {
        pal: Palette::new(cfg.light),
        w: Writer { mode: cfg.mode, hyperlinks: cfg.hyperlinks, links: Vec::new() },
        cfg,
        events,
        out: String::with_capacity(src.len() * 2),
        containers: Vec::new(),
        lists: Vec::new(),
        inline: Vec::new(),
        styles: vec![Style::default()],
        pending_blank: false,
        wrote_any: false,
        code: None,
        html: None,
        table: None,
        heading: None,
        links: Vec::new(),
        html_tags: Vec::new(),
        footnotes: HashMap::new(),
        highlighter,
        skip_depth: 0,
    };
    let events = std::mem::take(&mut r.events);
    for (i, ev) in events.iter().enumerate() {
        r.event(&events, i, ev);
    }
    r.flush_inline();
    Ok(r.out)
}

fn heading_idx(l: HeadingLevel) -> usize {
    l as usize - 1
}

impl<'e> Renderer<'e> {
    fn color(&self) -> bool {
        self.cfg.mode != ColorMode::None
    }

    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn push_style(&mut self, s: Style) {
        let top = self.style().patch(s);
        self.styles.push(top);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn text(&mut self, t: &str) {
        let style = self.style();
        match self.inline.last_mut() {
            Some(last) if last.style == style => last.text.push_str(t),
            _ => self.inline.push(Span::new(t, style)),
        }
    }

    fn prefix_width(&self) -> usize {
        self.containers
            .iter()
            .map(|c| match c {
                Container::Quote(_) => 2,
                Container::Item { width, .. } => *width,
            })
            .sum()
    }

    fn avail(&self) -> usize {
        self.cfg.width.saturating_sub(self.prefix_width()).max(10)
    }

    fn prefix(&mut self, blank: bool) -> Vec<Span> {
        let mut p = Vec::new();
        for c in &mut self.containers {
            match c {
                Container::Quote(st) => p.push(Span::new("│ ", *st)),
                Container::Item { marker, width, used } => {
                    if !blank && !*used {
                        p.extend(marker.iter().cloned());
                        *used = true;
                    } else {
                        p.push(Span::plain(" ".repeat(*width)));
                    }
                }
            }
        }
        if blank {
            while p.last().is_some_and(|s| s.text.trim().is_empty()) {
                p.pop();
            }
            if let Some(last) = p.last_mut() {
                last.text.truncate(last.text.trim_end().len());
            }
        }
        p
    }

    fn emit(&mut self, content: Line) {
        let mut line = self.prefix(false);
        line.extend(content);
        self.w.write_line(&mut self.out, &line);
        self.wrote_any = true;
    }

    fn emit_blank(&mut self) {
        let line = self.prefix(true);
        self.w.write_line(&mut self.out, &line);
    }

    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        if spans.iter().all(|s| s.text.trim().is_empty()) {
            return;
        }
        for line in wrap(&spans, self.avail()) {
            self.emit(line);
        }
    }

    fn start_block(&mut self) {
        self.flush_inline();
        if self.pending_blank && self.wrote_any {
            self.emit_blank();
        }
        self.pending_blank = false;
    }

    fn end_block(&mut self) {
        self.flush_inline();
        self.pending_blank = true;
    }

    fn event(&mut self, events: &[Event<'e>], i: usize, ev: &Event<'e>) {
        if self.skip_depth > 0 {
            match ev {
                Event::Start(_) => self.skip_depth += 1,
                Event::End(_) => self.skip_depth -= 1,
                _ => {}
            }
            return;
        }
        if let Some((_, buf)) = &mut self.code {
            match ev {
                Event::Text(t) => buf.push_str(t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, buf) = self.code.take().unwrap();
                    self.render_code(&lang, &buf);
                    self.end_block();
                }
                _ => {}
            }
            return;
        }
        if let Some(buf) = &mut self.html {
            match ev {
                Event::Html(t) | Event::Text(t) => buf.push_str(t),
                Event::End(TagEnd::HtmlBlock) => {
                    let buf = self.html.take().unwrap();
                    self.render_html_block(&buf);
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => self.start(events, i, tag),
            Event::End(tag) => self.end(*tag),
            Event::Text(t) => self.text(t),
            Event::Code(t) => {
                let st = self.style().patch(self.pal.code);
                if self.color() {
                    self.inline.push(Span::new(t.as_ref(), st));
                } else {
                    self.inline.push(Span::new(format!("`{t}`"), st));
                }
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let st = self.style().patch(self.pal.code);
                self.inline.push(Span::new(t.as_ref(), st));
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.start_block();
                let n = self.avail();
                let rule = Span::new("─".repeat(n), self.pal.rule);
                self.emit(vec![rule]);
                self.end_block();
            }
            Event::Html(t) => {
                // Html outside an HtmlBlock does not normally happen; treat as a block.
                self.start_block();
                self.render_html_block(t);
            }
            Event::InlineHtml(t) => self.inline_html(t),
            Event::FootnoteReference(label) => {
                let n = self.footnote_number(label);
                let st = self.style().patch(self.pal.footnote);
                self.inline.push(Span::new(format!("[{n}]"), st));
            }
            Event::TaskListMarker(checked) => {
                let (text, st) = if *checked {
                    ("☑ ", self.pal.checked)
                } else {
                    ("☐ ", self.pal.marker)
                };
                match self.containers.last_mut() {
                    Some(Container::Item { marker, width, used: false }) if self.inline.is_empty() => {
                        let pad = width.saturating_sub(2);
                        *marker = vec![Span::plain(" ".repeat(pad)), Span::new(text, st)];
                    }
                    _ => self.inline.push(Span::new(text, st)),
                }
            }
        }
    }

    fn footnote_number(&mut self, label: &str) -> usize {
        let next = self.footnotes.len() + 1;
        *self.footnotes.entry(label.to_owned()).or_insert(next)
    }

    fn start(&mut self, events: &[Event<'e>], i: usize, tag: &Tag<'e>) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.heading = Some(*level);
                let idx = heading_idx(*level);
                self.push_style(self.pal.h[idx]);
                if idx >= 2 || !self.color() {
                    let hashes = "#".repeat(idx + 1) + " ";
                    self.inline.push(Span::new(hashes, self.pal.h[idx].dim()));
                }
            }
            Tag::BlockQuote(kind) => {
                self.start_block();
                let (st, title) = match kind {
                    None => (self.pal.quote, None),
                    Some(k) => {
                        let (c, t) = match k {
                            BlockQuoteKind::Note => (12, "ⓘ Note"),
                            BlockQuoteKind::Tip => (10, "✱ Tip"),
                            BlockQuoteKind::Important => (13, "❢ Important"),
                            BlockQuoteKind::Warning => (11, "⚠ Warning"),
                            BlockQuoteKind::Caution => (9, "⊘ Caution"),
                        };
                        (Style::fg(Color::Ansi(c)), Some(t))
                    }
                };
                self.containers.push(Container::Quote(st));
                if let Some(t) = title {
                    self.emit(vec![Span::new(t, st.bold())]);
                }
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.start_block();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.start_block();
                let (tight, count) = list_info(events, i);
                let width = match start {
                    Some(s) => (s + count.saturating_sub(1) as u64).to_string().len() + 2,
                    None => 2,
                };
                self.lists.push(List { next: *start, tight, width });
            }
            Tag::Item => {
                self.flush_inline();
                let depth = self.lists.len().saturating_sub(1);
                let list = self.lists.last_mut().expect("item outside list");
                if list.tight {
                    self.pending_blank = false;
                }
                let width = list.width;
                let marker = match &mut list.next {
                    Some(n) => {
                        let m = format!("{:>w$} ", format!("{n}."), w = width - 1);
                        *n += 1;
                        m
                    }
                    None => format!("{} ", ["•", "◦", "▪", "▫"][depth % 4]),
                };
                let marker = vec![Span::new(marker, self.pal.marker)];
                self.containers.push(Container::Item { marker, width, used: false });
            }
            Tag::FootnoteDefinition(label) => {
                self.start_block();
                let n = self.footnote_number(label);
                let m = format!("[{n}] ");
                let width = str_width(&m);
                self.containers.push(Container::Item {
                    marker: vec![Span::new(m, self.pal.footnote)],
                    width,
                    used: false,
                });
            }
            Tag::DefinitionList => self.start_block(),
            Tag::DefinitionListTitle => {
                self.start_block();
                self.push_style(Style::default().bold());
            }
            Tag::DefinitionListDefinition => {
                self.flush_inline();
                self.pending_blank = false;
                self.containers.push(Container::Item {
                    marker: vec![Span::new("  ⮡ ", self.pal.marker)],
                    width: 4,
                    used: false,
                });
            }
            Tag::Table(aligns) => {
                self.start_block();
                self.table = Some(Table { aligns: aligns.clone(), rows: Vec::new(), head_rows: 0 });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
                if matches!(tag, Tag::TableHead) {
                    self.push_style(Style::default().bold());
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.push_style(Style::default().italic()),
            Tag::Strong => self.push_style(Style::default().bold()),
            Tag::Strikethrough => {
                self.push_style(Style { strike: true, ..Default::default() })
            }
            Tag::Superscript | Tag::Subscript => self.push_style(Style::default()),
            Tag::Link { link_type, dest_url, .. } => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => self.resolve_url(dest_url),
                };
                let show_url = !self.cfg.hyperlinks
                    && !matches!(link_type, LinkType::Autolink | LinkType::Email)
                    && !dest_url.starts_with('#');
                self.start_link(url, show_url, self.pal.link);
            }
            Tag::Image { dest_url, .. } => {
                let url = self.resolve_url(dest_url);
                let show_url = !self.cfg.hyperlinks;
                let st = self.pal.link;
                self.start_link(url, show_url, Style { italic: true, ..st });
                let label_st = self.style().patch(self.pal.dim);
                self.inline.push(Span::new("[image: ", label_st));
            }
            Tag::MetadataBlock(_) => self.skip_depth = 1,
        }
    }

    fn start_link(&mut self, url: String, show_url: bool, st: Style) {
        let id = self.w.add_link(url.clone());
        self.links.push(LinkInfo { id, url, start: self.inline.len(), show_url });
        self.push_style(Style { link: id, ..st });
    }

    fn end_link(&mut self, image: bool) {
        self.pop_style();
        let Some(info) = self.links.pop() else { return };
        if image {
            let link = Style { link: info.id, ..Default::default() };
            let st = self.style().patch(self.pal.dim).patch(link);
            self.inline.push(Span::new("]", st));
        }
        if info.show_url {
            let text: String = self.inline[info.start..].iter().map(|s| s.text.as_str()).collect();
            if text != info.url {
                let st = self.style().patch(self.pal.dim);
                self.inline.push(Span::new(format!(" <{}>", info.url), st));
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_block(),
            TagEnd::Heading(level) => {
                self.flush_inline();
                self.pop_style();
                self.heading = None;
                let idx = heading_idx(level);
                if idx < 2 && self.color() {
                    let ch = if idx == 0 { "━" } else { "─" };
                    let st = if idx == 0 { self.pal.h[0] } else { self.pal.rule };
                    let n = self.avail();
                    self.emit(vec![Span::new(ch.repeat(n), Style { bold: false, ..st })]);
                }
                self.pending_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
                self.pending_blank = true;
            }
            TagEnd::CodeBlock | TagEnd::HtmlBlock => {}
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                self.pending_blank = true;
            }
            TagEnd::Item | TagEnd::FootnoteDefinition | TagEnd::DefinitionListDefinition => {
                self.flush_inline();
                if let Some(Container::Item { used: false, .. }) = self.containers.last() {
                    self.emit(Vec::new());
                }
                self.containers.pop();
                if tag != TagEnd::Item {
                    self.pending_blank = true;
                }
            }
            TagEnd::DefinitionList => self.pending_blank = true,
            TagEnd::DefinitionListTitle => {
                self.flush_inline();
                self.pop_style();
            }
            TagEnd::Table => {
                let t = self.table.take().unwrap();
                self.render_table(t);
                self.pending_blank = true;
            }
            TagEnd::TableHead => {
                self.pop_style();
                if let Some(t) = &mut self.table {
                    t.head_rows = t.rows.len();
                }
            }
            TagEnd::TableRow => {}
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript => self.pop_style(),
            TagEnd::Link => self.end_link(false),
            TagEnd::Image => self.end_link(true),
            TagEnd::MetadataBlock(_) => {}
        }
    }

    fn resolve_url(&self, url: &str) -> String {
        let has_scheme = url
            .split_once(':')
            .is_some_and(|(s, _)| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && s.len() > 1);
        if has_scheme || url.starts_with('#') || url.starts_with("//") {
            return url.to_owned();
        }
        let Some(base) = &self.cfg.base_dir else { return url.to_owned() };
        let (path, frag) = match url.find(['#', '?']) {
            Some(p) => url.split_at(p),
            None => (url, ""),
        };
        let path = if path.starts_with('/') { PathBuf::from(path) } else { base.join(path) };
        format!("file://{}{}", percent_encode_path(&path.to_string_lossy()), frag)
    }

    fn inline_html(&mut self, html: &str) {
        let t = html.trim();
        if t.starts_with("<!--") {
            return;
        }
        let closing = t.starts_with("</");
        let name: String = t
            .trim_start_matches(['<', '/'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let style = match name.as_str() {
            "br" => {
                self.text("\n");
                return;
            }
            "b" | "strong" => Style::default().bold(),
            "i" | "em" | "cite" | "var" => Style::default().italic(),
            "s" | "del" | "strike" => Style { strike: true, ..Default::default() },
            "u" | "ins" => Style { underline: true, ..Default::default() },
            "code" | "kbd" | "tt" | "samp" => self.pal.code,
            "mark" => Style::fg(Color::Ansi(0)).on(Color::Ansi(11)),
            "sup" | "sub" | "span" | "small" | "abbr" | "font" => Style::default(),
            "a" if !closing => match attr(t, "href") {
                Some(href) => {
                    let url = self.resolve_url(&href);
                    let show = !self.cfg.hyperlinks && !href.starts_with('#');
                    self.start_link(url, show, self.pal.link);
                    self.html_tags.push(name);
                    return;
                }
                None => Style::default(),
            },
            "a" => {
                if self.html_tags.last().is_some_and(|n| n == "a") {
                    self.html_tags.pop();
                    self.end_link(false);
                }
                return;
            }
            "img" => {
                let url = attr(t, "src").map(|s| self.resolve_url(&s)).unwrap_or_default();
                let alt = attr(t, "alt").unwrap_or_default();
                self.start_link(url, !self.cfg.hyperlinks, Style { italic: true, ..self.pal.link });
                let st = self.style().patch(self.pal.dim);
                self.inline.push(Span::new("[image: ", st));
                self.text(&alt);
                self.end_link(true);
                return;
            }
            _ => {
                let st = self.style().patch(self.pal.dim);
                self.inline.push(Span::new(html, st));
                return;
            }
        };
        if closing {
            if self.html_tags.last() == Some(&name) {
                self.html_tags.pop();
                self.pop_style();
            }
        } else if !t.ends_with("/>") {
            self.html_tags.push(name);
            self.push_style(style);
        }
    }

    fn render_html_block(&mut self, html: &str) {
        let mut in_comment = false;
        let st = self.pal.dim;
        let mut lines = Vec::new();
        for line in html.lines() {
            let mut visible = String::new();
            let mut rest = line;
            loop {
                if in_comment {
                    match rest.find("-->") {
                        Some(p) => {
                            rest = &rest[p + 3..];
                            in_comment = false;
                        }
                        None => break,
                    }
                } else {
                    match rest.find("<!--") {
                        Some(p) => {
                            visible.push_str(&rest[..p]);
                            rest = &rest[p + 4..];
                            in_comment = true;
                        }
                        None => {
                            visible.push_str(rest);
                            break;
                        }
                    }
                }
            }
            if !visible.trim().is_empty() {
                lines.push(visible);
            }
        }
        if lines.is_empty() {
            return;
        }
        let avail = self.avail();
        for l in lines {
            for (row, _) in hard_wrap(&[Span::new(l, st)], avail) {
                self.emit(row);
            }
        }
        self.pending_blank = true;
    }

    fn render_code(&mut self, lang: &str, code: &str) {
        let code = code.strip_suffix('\n').unwrap_or(code);
        let avail = self.avail();
        let Some(hl) = &self.highlighter else {
            let indent = 4.min(avail / 4);
            for line in code.lines() {
                for (row, _) in hard_wrap(&[Span::plain(line)], avail - indent) {
                    let mut l = vec![Span::plain(" ".repeat(indent))];
                    l.extend(row);
                    self.emit(l);
                }
            }
            return;
        };
        let bg = hl.background();
        let fill = Style { bg, ..Default::default() };
        let lines = hl.highlight(lang, code);
        let inner = avail.saturating_sub(2).max(1);

        // Header row: blank band with the language name at the right.
        let label = lang.split([',', ' ']).next().unwrap_or("").trim_matches(['{', '}', '.']);
        let label_w = str_width(label);
        let mut header = vec![Span::new(" ".repeat(avail.saturating_sub(label_w + 1)), fill)];
        if label_w > 0 && label_w + 1 < avail {
            let st = Style { fg: hl.foreground(), dim: true, italic: true, ..fill };
            header.push(Span::new(label, st));
            header.push(Span::new(" ", fill));
        } else {
            header = vec![Span::new(" ".repeat(avail), fill)];
        }
        let mut rows = vec![header];
        for spans in lines {
            let spans: Vec<Span> = spans
                .into_iter()
                .map(|s| Span { style: Style { bg, ..s.style }, ..s })
                .collect();
            for (row, w) in hard_wrap(&spans, inner) {
                let mut l = vec![Span::new(" ", fill)];
                l.extend(row);
                l.push(Span::new(" ".repeat(inner - w.min(inner) + 1), fill));
                rows.push(l);
            }
        }
        rows.push(vec![Span::new(" ".repeat(avail), fill)]);
        for r in rows {
            self.emit(r);
        }
    }

    fn render_table(&mut self, t: Table) {
        let ncols = t.rows.iter().map(Vec::len).max().unwrap_or(0).max(t.aligns.len());
        if ncols == 0 {
            return;
        }
        let avail = self.avail();
        // Per cell: hard-broken segments, each a list of word widths.
        let measured: Vec<Vec<CellWords>> = t
            .rows
            .iter()
            .map(|row| (0..ncols).map(|c| measure_cell(row.get(c).map_or(&[][..], |v| v))).collect())
            .collect();
        let mut natural = vec![1usize; ncols];
        let mut minimum = vec![1usize; ncols];
        for row in &measured {
            for (c, cell) in row.iter().enumerate() {
                for seg in cell {
                    let w = seg.iter().sum::<usize>() + seg.len().saturating_sub(1);
                    natural[c] = natural[c].max(w);
                    let longest = seg.iter().copied().max().unwrap_or(0);
                    minimum[c] = minimum[c].max(longest.min(24));
                }
            }
        }
        // Cell content widths must fit alongside "│ " + " │ " * (n-1) + " │".
        let space = avail.saturating_sub(3 * ncols + 1);
        let words_fit = minimum.iter().map(|m| (*m).min(12)).sum::<usize>() <= space;
        if natural.iter().sum::<usize>() > space && (!words_fit || space < 4 * ncols) {
            return self.render_table_stacked(t, ncols);
        }
        let widths = fit_columns(&measured, &natural, &minimum, space);

        let bst = self.pal.border;
        let border = |l: &str, m: &str, r: &str| -> Line {
            let mut s = String::from(l);
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(w + 2));
                s.push_str(if i + 1 == ncols { r } else { m });
            }
            vec![Span::new(s, bst)]
        };
        let mut out = vec![border("┌", "┬", "┐")];
        let empty = Vec::new();
        for (ri, row) in t.rows.iter().enumerate() {
            if ri == t.head_rows && ri > 0 {
                out.push(border("╞", "╪", "╡").into_iter().map(|s| Span { text: s.text.replace('─', "═"), ..s }).collect());
            } else if ri > 0 {
                out.push(border("├", "┼", "┤"));
            }
            let wrapped: Vec<Vec<Line>> = (0..ncols)
                .map(|c| wrap(row.get(c).unwrap_or(&empty), widths[c]))
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            for li in 0..height {
                let mut line = vec![Span::new("│", bst)];
                for c in 0..ncols {
                    let content = wrapped[c].get(li).cloned().unwrap_or_default();
                    let cw = spans_width(&content);
                    let pad = widths[c].saturating_sub(cw);
                    let (lp, rp) = match t.aligns.get(c) {
                        Some(Alignment::Right) => (pad, 0),
                        Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                        _ => (0, pad),
                    };
                    line.push(Span::plain(" ".repeat(lp + 1)));
                    line.extend(content);
                    line.push(Span::plain(" ".repeat(rp + 1)));
                    line.push(Span::new("│", bst));
                }
                out.push(line);
            }
        }
        out.push(border("└", "┴", "┘"));
        for l in out {
            self.emit(l);
        }
    }
}

impl Renderer<'_> {
    /// Fallback for tables too wide for the terminal: one "Header  value" record per row.
    fn render_table_stacked(&mut self, t: Table, ncols: usize) {
        let empty = Vec::new();
        let headers: Vec<&Vec<Span>> = match t.head_rows {
            0 => vec![&empty; ncols],
            _ => (0..ncols).map(|c| t.rows[0].get(c).unwrap_or(&empty)).collect(),
        };
        let key_w = headers.iter().map(|h| spans_width(h)).max().unwrap_or(0).min(self.avail() / 3);
        let avail = self.avail();
        let bst = self.pal.border;
        for (ri, row) in t.rows.iter().enumerate().skip(t.head_rows) {
            if ri > t.head_rows {
                self.emit(vec![Span::new("─".repeat(avail), bst)]);
            }
            for c in 0..ncols {
                let key = wrap(headers[c], key_w.max(1));
                let val = wrap(row.get(c).unwrap_or(&empty), avail.saturating_sub(key_w + 3).max(1));
                for i in 0..key.len().max(val.len()) {
                    let k = key.get(i).cloned().unwrap_or_default();
                    let kw = spans_width(&k);
                    let mut line: Line = k.into_iter().map(|s| Span { style: s.style.bold(), ..s }).collect();
                    line.push(Span::plain(" ".repeat(key_w.saturating_sub(kw) + 1)));
                    line.push(Span::new("│ ", bst));
                    line.extend(val.get(i).cloned().unwrap_or_default());
                    self.emit(line);
                }
            }
        }
    }
}

type CellWords = Vec<Vec<usize>>;

fn measure_cell(cell: &[Span]) -> CellWords {
    let text: String = cell.iter().map(|s| s.text.as_str()).collect();
    text.split('\n')
        .map(|seg| seg.split([' ', '\t']).filter(|w| !w.is_empty()).map(str_width).collect())
        .collect()
}

/// Number of lines `wrap` produces for a cell at width `w` (mirrors its greedy algorithm).
fn count_lines(cell: &CellWords, w: usize) -> u32 {
    let mut n = 0u32;
    for seg in cell {
        n += 1;
        let mut cur = 0;
        for &ww in seg {
            let sp = (cur > 0) as usize;
            if cur + sp + ww <= w {
                cur += sp + ww;
                continue;
            }
            if cur > 0 {
                n += 1;
            }
            if ww <= w {
                cur = ww;
            } else {
                let extra = (ww - 1) / w;
                n += extra as u32;
                cur = ww - extra * w;
            }
        }
    }
    n.max(1)
}

/// Chooses column widths summing to at most `space`, minimizing the table's rendered
/// height (a row is as tall as its tallest cell). Natural widths are used if they fit.
/// Otherwise several candidate layouts are built and each refined by local search;
/// the shortest table wins:
/// - water-fill against a percentile of each column's cell widths, so a few long
///   outlier cells don't make an otherwise narrow column wide;
/// - greedy: starting from the longest words, give width to whichever column cuts
///   the most lines per column spent (with a smooth max to get past plateaus where
///   several columns must widen together).
fn fit_columns(cells: &[Vec<CellWords>], natural: &[usize], minimum: &[usize], space: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= space {
        return natural.to_vec();
    }
    let min: Vec<usize> = minimum.iter().zip(natural).map(|(m, n)| (*m).min(*n)).collect();
    let min_total: usize = min.iter().sum();
    if min_total >= space {
        return min.iter().map(|m| (m * space / min_total).max(1)).collect();
    }
    let fit = Fitter::new(cells, natural, min, space);
    let mut candidates = vec![fit.greedy()];
    for q in [50, 75, 90, 100] {
        candidates.push(fit.water_fill(&fit.percentile_widths(q)));
    }
    candidates
        .into_iter()
        .map(|w| fit.local_search(w))
        .min_by_key(|w| fit.height(w))
        .map(|w| fit.fill_leftover(w))
        .unwrap()
}

struct Fitter<'a> {
    cells: &'a [Vec<CellWords>],
    natural: &'a [usize],
    min: Vec<usize>,
    max_w: Vec<usize>,
    space: usize,
    /// lines[c][w - min[c]][row]
    lines: Vec<Vec<Vec<u32>>>,
}

impl<'a> Fitter<'a> {
    fn new(cells: &'a [Vec<CellWords>], natural: &'a [usize], min: Vec<usize>, space: usize) -> Self {
        let spare = space - min.iter().sum::<usize>();
        let max_w: Vec<usize> = (0..natural.len()).map(|c| natural[c].min(min[c] + spare)).collect();
        let lines = (0..natural.len())
            .map(|c| {
                (min[c]..=max_w[c])
                    .map(|w| cells.iter().map(|row| count_lines(&row[c], w)).collect())
                    .collect()
            })
            .collect();
        Fitter { cells, natural, min, max_w, space, lines }
    }

    fn ncols(&self) -> usize {
        self.natural.len()
    }

    fn col(&self, c: usize, w: usize) -> &[u32] {
        &self.lines[c][w.min(self.max_w[c]) - self.min[c]]
    }

    fn height(&self, widths: &[usize]) -> u32 {
        (0..self.cells.len())
            .map(|r| (0..self.ncols()).map(|c| self.col(c, widths[c])[r]).max().unwrap())
            .sum()
    }

    fn percentile_widths(&self, q: usize) -> Vec<usize> {
        (0..self.ncols())
            .map(|c| {
                let mut ws: Vec<usize> = self
                    .cells
                    .iter()
                    .map(|row| row[c].iter().map(|s| s.iter().sum::<usize>() + s.len().saturating_sub(1)).max().unwrap_or(0))
                    .collect();
                ws.sort_unstable();
                ws.get((ws.len().saturating_sub(1)) * q / 100).copied().unwrap_or(0)
            })
            .collect()
    }

    /// Widths min(target, max(level, min)) for the highest level that fits.
    fn water_fill(&self, target: &[usize]) -> Vec<usize> {
        let at = |level: usize| -> Vec<usize> {
            (0..self.ncols()).map(|c| target[c].max(self.min[c]).min(level.max(self.min[c])).min(self.max_w[c])).collect()
        };
        let (mut lo, mut hi) = (0, self.space);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if at(mid).iter().sum::<usize>() <= self.space {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        at(lo)
    }

    fn greedy(&self) -> Vec<usize> {
        const P: i32 = 4;
        let pow = |l: u32| (l as f64).powi(P);
        let root = |x: f64| x.powf(1.0 / P as f64);
        let (ncols, nrows) = (self.ncols(), self.cells.len());
        let mut widths = self.min.clone();
        let mut left = self.space - self.min.iter().sum::<usize>();
        let mut row_sum: Vec<f64> =
            (0..nrows).map(|r| (0..ncols).map(|c| pow(self.col(c, widths[c])[r])).sum()).collect();
        let mut other_max = vec![0u32; nrows];
        while left > 0 {
            let cur_h = self.height(&widths);
            let cur_soft: f64 = row_sum.iter().map(|&s| root(s)).sum();
            // (gain per column spent, column, added width)
            let mut best_exact: Option<(f64, usize, usize)> = None;
            let mut best_soft: Option<(f64, usize, usize)> = None;
            for c in 0..ncols {
                for (r, m) in other_max.iter_mut().enumerate() {
                    *m = (0..ncols).filter(|&o| o != c).map(|o| self.col(o, widths[o])[r]).max().unwrap_or(0);
                }
                let old = self.col(c, widths[c]);
                let mut prev = old;
                for w in widths[c] + 1..=self.max_w[c].min(widths[c] + left) {
                    let new = self.col(c, w);
                    if new == prev {
                        continue;
                    }
                    prev = new;
                    let k = (w - widths[c]) as f64;
                    let h: u32 = (0..nrows).map(|r| new[r].max(other_max[r])).sum();
                    if h < cur_h {
                        let g = (cur_h - h) as f64 / k;
                        if best_exact.is_none_or(|(b, _, _)| g > b) {
                            best_exact = Some((g, c, w - widths[c]));
                        }
                    }
                    if best_exact.is_none() {
                        let soft: f64 = (0..nrows).map(|r| root(row_sum[r] - pow(old[r]) + pow(new[r]))).sum();
                        let g = (cur_soft - soft) / k;
                        if g > 1e-9 && best_soft.is_none_or(|(b, _, _)| g > b) {
                            best_soft = Some((g, c, w - widths[c]));
                        }
                    }
                }
            }
            let Some((_, c, k)) = best_exact.or(best_soft) else { break };
            let (old, new) = (self.col(c, widths[c]), self.col(c, widths[c] + k));
            for r in 0..nrows {
                row_sum[r] += pow(new[r]) - pow(old[r]);
            }
            widths[c] += k;
            left -= k;
        }
        widths
    }

    /// Spends unused width, then moves width from column `d` to column `c` while the
    /// table gets shorter.
    fn local_search(&self, mut widths: Vec<usize>) -> Vec<usize> {
        let n = self.ncols();
        let mut cur_h = self.height(&widths);
        for _ in 0..64 {
            let left = self.space - widths.iter().sum::<usize>();
            let mut best: Option<(u32, usize, Option<usize>, usize)> = None;
            for c in 0..n {
                let donors = (0..n).filter(|&d| d != c).map(Some).chain([None]);
                for d in donors {
                    let room = d.map_or(left, |d| widths[d] - self.min[d]).min(self.max_w[c] - widths[c]);
                    for k in 1..=room {
                        let mut trial = widths.clone();
                        trial[c] += k;
                        if let Some(d) = d {
                            trial[d] -= k;
                        }
                        let h = self.height(&trial);
                        if h < best.map_or(cur_h, |b| b.0) {
                            best = Some((h, c, d, k));
                        }
                    }
                }
            }
            let Some((h, c, d, k)) = best else { break };
            widths[c] += k;
            if let Some(d) = d {
                widths[d] -= k;
            }
            cur_h = h;
        }
        widths
    }

    /// Gives leftover width to the columns furthest from their natural width.
    fn fill_leftover(&self, mut widths: Vec<usize>) -> Vec<usize> {
        let mut left = self.space - widths.iter().sum::<usize>();
        while left > 0 {
            let Some(c) = (0..self.ncols())
                .filter(|&c| widths[c] < self.natural[c])
                .max_by_key(|&c| self.natural[c] - widths[c])
            else {
                break;
            };
            widths[c] += 1;
            left -= 1;
        }
        widths
    }
}

/// Looks ahead from a `Start(List)` to find whether it is tight and how many items it has.
fn list_info(events: &[Event], i: usize) -> (bool, usize) {
    let mut depth = 0usize;
    let mut count = 0;
    let mut tight = true;
    for j in i + 1..events.len() {
        match &events[j] {
            Event::Start(Tag::List(_)) => depth += 1,
            Event::End(TagEnd::List(_)) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Event::Start(Tag::Item) if depth == 0 => {
                count += 1;
                if matches!(events.get(j + 1), Some(Event::Start(Tag::Paragraph))) {
                    tight = false;
                }
            }
            _ => {}
        }
    }
    (tight, count)
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name) {
        let at = from + p;
        from = at + name.len();
        let before_ok = at > 0 && lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let val = match rest.chars().next()? {
            q @ ('"' | '\'') => rest[1..].split(q).next()?,
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?,
        };
        return Some(val.to_owned());
    }
    None
}

fn percent_encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(md: &str, width: usize) -> String {
        let cfg = Config {
            width,
            mode: ColorMode::None,
            hyperlinks: false,
            light: false,
            theme: String::new(),
            base_dir: Some(PathBuf::from("/docs")),
        };
        render(md, cfg).unwrap()
    }

    #[test]
    fn count_lines_matches_wrap() {
        let texts = ["aaa bbb ccc dddddddddd", "x", "", "a\nbb cc", "averyveryverylongword and more"];
        for t in texts {
            for w in 1..30 {
                let spans = [Span::plain(t)];
                assert_eq!(count_lines(&measure_cell(&spans), w) as usize, wrap(&spans, w).len(), "{t:?} @ {w}");
            }
        }
    }

    #[test]
    fn fit_columns_favors_text_heavy_columns() {
        let cell = |t: &str| measure_cell(&[Span::plain(t)]);
        let long = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod";
        let cells: Vec<Vec<CellWords>> = (0..5)
            .map(|i| vec![cell("No evidence"), cell(if i == 0 { "Live evidence evidence evidence" } else { "No" }), cell(long)])
            .collect();
        let natural = [11, 31, long.len()];
        let widths = fit_columns(&cells, &natural, &[8, 8, 11], 50);
        assert_eq!(widths.iter().sum::<usize>(), 50);
        assert!(widths[2] > widths[1], "{widths:?}");
    }

    #[test]
    fn tight_and_ordered_lists() {
        assert_eq!(plain("- a\n- b\n  - c\n", 40), "• a\n• b\n  ◦ c\n");
        assert_eq!(plain("9. a\n10. b\n", 40), " 9. a\n10. b\n");
    }

    #[test]
    fn links_resolve_relative_paths() {
        assert_eq!(plain("[x](<a b.md#h>)", 80), "x <file:///docs/a%20b.md#h>\n");
        assert_eq!(plain("<https://e.org>", 80), "https://e.org\n");
    }

    #[test]
    fn table_layout() {
        let out = plain("| a | b |\n|---|--:|\n| x | 12 |\n| y | 3 |\n", 40);
        let want = "┌───┬────┐\n│ a │  b │\n╞═══╪════╡\n│ x │ 12 │\n├───┼────┤\n│ y │  3 │\n└───┴────┘\n";
        assert_eq!(out, want);
    }
}
