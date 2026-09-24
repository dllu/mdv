//! Word wrapping of styled spans.

use crate::style::{Span, Style, char_width};

pub type Line = Vec<Span>;

fn push_char(line: &mut Line, c: char, style: Style) {
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push(c),
        _ => line.push(Span::new(c.to_string(), style)),
    }
}

fn push_str(line: &mut Line, s: &str, style: Style) {
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push_str(s),
        _ => line.push(Span::new(s, style)),
    }
}

struct Wrapper {
    width: usize,
    lines: Vec<Line>,
    cur: Line,
    cur_w: usize,
    space: Option<Style>,
    word: Vec<(char, Style, usize)>,
    word_w: usize,
}

impl Wrapper {
    fn break_line(&mut self) {
        self.lines.push(std::mem::take(&mut self.cur));
        self.cur_w = 0;
    }

    fn flush_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let space = self.space.filter(|_| self.cur_w > 0);
        let sp = space.is_some() as usize;
        if self.cur_w + sp + self.word_w <= self.width {
            if let Some(st) = space {
                push_char(&mut self.cur, ' ', st);
                self.cur_w += 1;
            }
        } else if self.cur_w > 0 {
            self.break_line();
        }
        let word = std::mem::take(&mut self.word);
        if self.cur_w + self.word_w <= self.width {
            for (c, st, _) in &word {
                push_char(&mut self.cur, *c, *st);
            }
            self.cur_w += self.word_w;
        } else {
            // Word longer than the line: break it anywhere.
            for (c, st, w) in word {
                if self.cur_w + w > self.width && self.cur_w > 0 {
                    self.break_line();
                }
                push_char(&mut self.cur, c, st);
                self.cur_w += w;
            }
        }
        self.word_w = 0;
        self.space = None;
    }
}

/// Greedy word wrap. Only ASCII space/tab are break opportunities, so
/// non-breaking spaces stay intact; `\n` forces a line break.
pub fn wrap(spans: &[Span], width: usize) -> Vec<Line> {
    let mut w = Wrapper {
        width: width.max(1),
        lines: Vec::new(),
        cur: Vec::new(),
        cur_w: 0,
        space: None,
        word: Vec::new(),
        word_w: 0,
    };
    for span in spans {
        for c in span.text.chars() {
            match c {
                '\n' => {
                    w.flush_word();
                    w.break_line();
                    w.space = None;
                }
                ' ' | '\t' => {
                    w.flush_word();
                    if w.space.is_none() {
                        w.space = Some(span.style);
                    }
                }
                // Emoji presentation selector: the preceding symbol renders 2 columns wide.
                '\u{FE0F}' => {
                    if let Some(last) = w.word.last_mut().filter(|l| l.2 == 1) {
                        last.2 = 2;
                        w.word_w += 1;
                    }
                    w.word.push((c, span.style, 0));
                }
                _ => {
                    let cw = char_width(c);
                    w.word.push((c, span.style, cw));
                    w.word_w += cw;
                }
            }
        }
    }
    w.flush_word();
    if !w.cur.is_empty() || w.lines.is_empty() {
        w.lines.push(w.cur);
    }
    w.lines
}

/// Character wrap (for code): breaks exactly at `width` columns, keeping all whitespace.
pub fn hard_wrap(spans: &[Span], width: usize) -> Vec<(Line, usize)> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut cur = Vec::new();
    let mut cur_w = 0;
    for span in spans {
        if cur_w + crate::style::str_width(&span.text) <= width {
            push_str(&mut cur, &span.text, span.style);
            cur_w += crate::style::str_width(&span.text);
            continue;
        }
        for c in span.text.chars() {
            let cw = char_width(c);
            if cur_w + cw > width && cur_w > 0 {
                lines.push((std::mem::take(&mut cur), cur_w));
                cur_w = 0;
            }
            push_char(&mut cur, c, span.style);
            cur_w += cw;
        }
    }
    lines.push((cur, cur_w));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect()).collect()
    }

    #[test]
    fn wraps_on_words_and_breaks_long_ones() {
        let spans = [Span::plain("aaa bbb ccc"), Span::plain(" dddddddddd")];
        assert_eq!(text(&wrap(&spans, 7)), ["aaa bbb", "ccc", "ddddddd", "ddd"]);
    }

    #[test]
    fn words_span_styles_and_hard_breaks() {
        let bold = Style::default().bold();
        let spans = [Span::plain("ab"), Span::new("cd", bold), Span::plain(" e\nf")];
        let lines = wrap(&spans, 4);
        assert_eq!(text(&lines), ["abcd", "e", "f"]);
        assert_eq!(lines[0].len(), 2);
    }

    #[test]
    fn wide_chars_and_emoji_selector() {
        assert_eq!(text(&wrap(&[Span::plain("日本語 x")], 6)), ["日本語", "x"]);
        assert_eq!(text(&wrap(&[Span::plain("⚠\u{FE0F} ab")], 4)), ["⚠\u{FE0F}", "ab"]);
    }
}
