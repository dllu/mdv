//! Syntax highlighting for fenced code blocks (syntect + bat's grammar/theme collection).
//! The syntax set is only deserialized if a document actually has a tagged code block.

use std::cell::OnceCell;

use syntect::easy::HighlightLines;
use syntect::highlighting::{self, FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::LazyThemeSet;

use crate::style::{Color, Span, Style};

pub const DEFAULT_DARK_THEME: &str = "Monokai Extended";
pub const DEFAULT_LIGHT_THEME: &str = "Monokai Extended Light";
const TAB_WIDTH: usize = 4;

pub struct Highlighter {
    syntaxes: OnceCell<SyntaxSet>,
    theme: Theme,
}

pub fn theme_names() -> Vec<String> {
    let set = LazyThemeSet::from(two_face::theme::extra());
    set.theme_names().map(str::to_owned).collect()
}

fn convert(c: highlighting::Color) -> Option<Color> {
    // bat's "ansi"/"base16" themes encode palette indices with alpha 0,
    // and "terminal default" with alpha 1.
    match c.a {
        0 => Some(Color::Ansi(c.r)),
        1 => None,
        _ => Some(Color::Rgb(c.r, c.g, c.b)),
    }
}

fn normalize_lang(info: &str) -> &str {
    let tok = info
        .split(|c: char| c == ',' || c == '{' || c == '}' || c.is_whitespace())
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .trim_start_matches('.');
    match tok.to_ascii_lowercase().as_str() {
        "shell" | "console" | "shell-session" | "zsh" | "sh" => "bash",
        "c++" => "cpp",
        "golang" => "go",
        "jsonc" | "json5" => "json",
        "text" | "txt" | "plain" | "plaintext" | "output" => "",
        "py3" | "python3" => "py",
        "ps1" | "pwsh" => "powershell",
        _ => tok,
    }
}

impl Highlighter {
    pub fn new(theme_name: &str) -> Result<Self, String> {
        let set = LazyThemeSet::from(two_face::theme::extra());
        let theme = set
            .get(theme_name)
            .ok_or_else(|| format!("unknown theme '{theme_name}' (see --list-themes)"))?
            .clone();
        Ok(Highlighter { syntaxes: OnceCell::new(), theme })
    }

    pub fn background(&self) -> Option<Color> {
        self.theme.settings.background.and_then(convert)
    }

    pub fn foreground(&self) -> Option<Color> {
        self.theme.settings.foreground.and_then(convert)
    }

    fn find_syntax(&self, lang: &str) -> Option<(&SyntaxSet, &SyntaxReference)> {
        let lang = normalize_lang(lang);
        if lang.is_empty() {
            return None;
        }
        let ss = self.syntaxes.get_or_init(two_face::syntax::extra_newlines);
        let syn = ss
            .find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(&lang.to_ascii_lowercase()))?;
        Some((ss, syn))
    }

    /// Returns one span list per source line (tabs expanded, newlines stripped).
    pub fn highlight(&self, lang: &str, code: &str) -> Vec<Vec<Span>> {
        let base = Style { fg: self.foreground(), ..Default::default() };
        let Some((ss, syntax)) = self.find_syntax(lang) else {
            return code.lines().map(|l| vec![Span::new(expand_tabs(l, 0).0, base)]).collect();
        };
        let mut h = HighlightLines::new(syntax, &self.theme);
        let mut out = Vec::new();
        for line in LinesWithEndings::from(code) {
            let mut spans = Vec::new();
            let mut col = 0;
            match h.highlight_line(line, ss) {
                Ok(ranges) => {
                    for (st, text) in ranges {
                        let text = text.trim_end_matches(['\n', '\r']);
                        if text.is_empty() {
                            continue;
                        }
                        let (t, c) = expand_tabs(text, col);
                        col = c;
                        spans.push(Span::new(t, to_style(st)));
                    }
                }
                Err(_) => {
                    let text = line.trim_end_matches(['\n', '\r']);
                    spans.push(Span::new(expand_tabs(text, 0).0, base));
                }
            }
            out.push(spans);
        }
        out
    }
}

fn to_style(st: highlighting::Style) -> Style {
    Style {
        fg: convert(st.foreground),
        bold: st.font_style.contains(FontStyle::BOLD),
        italic: st.font_style.contains(FontStyle::ITALIC),
        underline: st.font_style.contains(FontStyle::UNDERLINE),
        ..Default::default()
    }
}

/// Expands tabs given the starting column; returns the text and the new column.
fn expand_tabs(s: &str, mut col: usize) -> (String, usize) {
    if !s.contains('\t') {
        return (s.to_owned(), col + crate::style::str_width(s));
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += crate::style::char_width(c);
        }
    }
    (out, col)
}
