//! Styled text spans and ANSI/OSC 8 serialization.

use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    /// One of the 256 palette colors (0-15 follow the user's terminal theme).
    Ansi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    None,
    Ansi256,
    TrueColor,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// 1-based index into the link table; 0 means "not a link".
    pub link: u32,
}

impl Style {
    pub fn fg(c: Color) -> Self {
        Style { fg: Some(c), ..Default::default() }
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub fn on(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }
    /// Layers `top` over `self`: set attributes/colors in `top` win.
    pub fn patch(mut self, top: Style) -> Self {
        self.fg = top.fg.or(self.fg);
        self.bg = top.bg.or(self.bg);
        self.bold |= top.bold;
        self.dim |= top.dim;
        self.italic |= top.italic;
        self.underline |= top.underline;
        self.strike |= top.strike;
        if top.link != 0 {
            self.link = top.link;
        }
        self
    }
    fn without_link(mut self) -> Self {
        self.link = 0;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Span { text: text.into(), style }
    }
    pub fn plain(text: impl Into<String>) -> Self {
        Span { text: text.into(), style: Style::default() }
    }
}

pub fn str_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

pub fn char_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| str_width(&s.text)).sum()
}

/// Serializes lines of spans into a terminal byte stream.
pub struct Writer {
    pub mode: ColorMode,
    pub hyperlinks: bool,
    pub links: Vec<String>,
}

impl Writer {
    pub fn add_link(&mut self, url: String) -> u32 {
        self.links.push(url);
        self.links.len() as u32
    }

    pub fn write_line(&self, out: &mut String, spans: &[Span]) {
        let mut cur = Style::default();
        let mut cur_link = 0u32;
        for span in spans {
            if span.text.is_empty() {
                continue;
            }
            let link = if self.hyperlinks { span.style.link } else { 0 };
            if link != cur_link {
                if cur_link != 0 {
                    out.push_str("\x1b]8;;\x1b\\");
                }
                if link != 0 {
                    let url = &self.links[link as usize - 1];
                    let _ = write!(out, "\x1b]8;;{url}\x1b\\");
                }
                cur_link = link;
            }
            if self.mode != ColorMode::None {
                let st = span.style.without_link();
                if st != cur {
                    if cur != Style::default() {
                        out.push_str("\x1b[0m");
                    }
                    self.sgr(out, st);
                    cur = st;
                }
            }
            out.push_str(&span.text);
        }
        if cur != Style::default() {
            out.push_str("\x1b[0m");
        }
        if cur_link != 0 {
            out.push_str("\x1b]8;;\x1b\\");
        }
        out.push('\n');
    }

    fn sgr(&self, out: &mut String, st: Style) {
        if st == Style::default() {
            return;
        }
        out.push_str("\x1b[");
        let mut first = true;
        let mut code = |out: &mut String, s: &str| {
            if !first {
                out.push(';');
            }
            first = false;
            out.push_str(s);
        };
        if st.bold {
            code(out, "1");
        }
        if st.dim {
            code(out, "2");
        }
        if st.italic {
            code(out, "3");
        }
        if st.underline {
            code(out, "4");
        }
        if st.strike {
            code(out, "9");
        }
        if let Some(c) = st.fg {
            code(out, &self.color_code(c, false));
        }
        if let Some(c) = st.bg {
            code(out, &self.color_code(c, true));
        }
        out.push('m');
    }

    fn color_code(&self, c: Color, bg: bool) -> String {
        let base = if bg { 40 } else { 30 };
        match c {
            Color::Ansi(n) if n < 8 => (base + n as u32).to_string(),
            Color::Ansi(n) if n < 16 => (base + 60 + n as u32 - 8).to_string(),
            Color::Ansi(n) => format!("{};5;{n}", base + 8),
            Color::Rgb(r, g, b) => match self.mode {
                ColorMode::TrueColor => format!("{};2;{r};{g};{b}", base + 8),
                _ => format!("{};5;{}", base + 8, rgb_to_256(r, g, b)),
            },
        }
    }
}

fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        return match r {
            0..8 => 16,
            249.. => 231,
            _ => 232 + ((r as u16 - 8) * 24 / 241) as u8,
        };
    }
    let q = |v: u8| -> u8 {
        match v {
            0..48 => 0,
            48..115 => 1,
            _ => (v - 35) / 40,
        }
    };
    16 + 36 * q(r) + 6 * q(g) + q(b)
}
