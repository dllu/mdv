//! Built-in full-screen pager: scrolling (keys and mouse wheel), search, a status bar,
//! and re-rendering on terminal resize.

use std::io::{self, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::terminal;

const WHEEL_LINES: usize = 3;

pub struct Options<'a> {
    pub title: &'a str,
    /// Capture the mouse (reliable wheel scrolling, but the terminal then needs a
    /// modifier key for text selection and link clicks). Otherwise we rely on
    /// "alternate scroll mode", where the terminal turns the wheel into arrow keys.
    pub mouse: bool,
    /// Re-renders the document for a new terminal width; `None` if the width is fixed.
    pub rerender: Option<&'a dyn Fn(usize) -> Result<String, String>>,
}

struct Doc {
    lines: Vec<String>,
    /// Lines with escape sequences stripped, for searching.
    plain: Vec<String>,
}

impl Doc {
    fn new(text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let plain = lines.iter().map(|l| strip_ansi(l)).collect();
        Doc { lines, plain }
    }
}

enum Input {
    Normal,
    Search(String),
}

struct Pager<'a> {
    opts: Options<'a>,
    doc: Doc,
    top: usize,
    cols: usize,
    rows: usize,
    input: Input,
    query: Option<String>,
    message: Option<String>,
}

pub fn run(initial: &str, opts: Options) -> Result<(), String> {
    let (cols, rows) = terminal::size().map_err(|e| e.to_string())?;
    let mut p = Pager {
        opts,
        doc: Doc::new(initial),
        top: 0,
        cols: cols as usize,
        rows: rows as usize,
        input: Input::Normal,
        query: None,
        message: None,
    };
    setup(p.opts.mouse).map_err(|e| e.to_string())?;
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = teardown();
        prev_hook(info);
    }));
    let result = p.event_loop();
    let _ = teardown();
    result.map_err(|e| e.to_string())
}

fn setup(mouse: bool) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    // Alternate screen, hide cursor, no autowrap, alternate scroll mode.
    let mut s = String::from("\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?1007h");
    if mouse {
        s.push_str("\x1b[?1000h\x1b[?1006h");
    }
    let mut out = io::stdout().lock();
    out.write_all(s.as_bytes())?;
    out.flush()
}

fn teardown() -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[?1007l\x1b[?7h\x1b[?25h\x1b[?1049l")?;
    out.flush()?;
    terminal::disable_raw_mode()
}

impl Pager<'_> {
    fn page(&self) -> usize {
        self.rows.saturating_sub(1).max(1)
    }

    fn max_top(&self) -> usize {
        self.doc.lines.len().saturating_sub(self.page())
    }

    fn scroll_by(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta).min(self.max_top());
    }

    fn event_loop(&mut self) -> io::Result<()> {
        loop {
            self.draw()?;
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if !self.key(k) {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollDown => self.scroll_by(WHEEL_LINES as isize),
                    MouseEventKind::ScrollUp => self.scroll_by(-(WHEEL_LINES as isize)),
                    _ => {}
                },
                Event::Resize(c, r) => self.resize(c as usize, r as usize),
                _ => {}
            }
        }
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        let width_changed = cols != self.cols;
        self.cols = cols;
        self.rows = rows;
        if let (true, Some(render)) = (width_changed, self.opts.rerender) {
            if let Ok(text) = render(cols) {
                // Keep the same relative position in the document.
                let frac = self.top as f64 / self.doc.lines.len().max(1) as f64;
                self.doc = Doc::new(&text);
                self.top = (frac * self.doc.lines.len() as f64).round() as usize;
            }
        }
        self.top = self.top.min(self.max_top());
    }

    /// Handles a key press; returns false to quit.
    fn key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return false;
        }
        if let Input::Search(q) = &mut self.input {
            match k.code {
                KeyCode::Enter => {
                    let q = std::mem::take(q);
                    self.input = Input::Normal;
                    if !q.is_empty() {
                        self.query = Some(q);
                    }
                    self.find(true, true);
                }
                KeyCode::Esc => self.input = Input::Normal,
                KeyCode::Backspace => {
                    if q.pop().is_none() {
                        self.input = Input::Normal;
                    }
                }
                KeyCode::Char(c) => q.push(c),
                _ => {}
            }
            return true;
        }
        self.message = None;
        let page = self.page() as isize;
        match (k.code, ctrl) {
            (KeyCode::Char('q' | 'Q') | KeyCode::Esc, false) => return false,
            (KeyCode::Char('j') | KeyCode::Down | KeyCode::Enter, false)
            | (KeyCode::Char('n' | 'e'), true) => self.scroll_by(1),
            (KeyCode::Char('k') | KeyCode::Up, false) | (KeyCode::Char('p' | 'y'), true) => {
                self.scroll_by(-1)
            }
            (KeyCode::Char(' ' | 'f') | KeyCode::PageDown, false) | (KeyCode::Char('f'), true) => {
                self.scroll_by(page)
            }
            (KeyCode::Char('b') | KeyCode::PageUp, false) | (KeyCode::Char('b'), true) => {
                self.scroll_by(-page)
            }
            (KeyCode::Char('d'), _) => self.scroll_by(page / 2),
            (KeyCode::Char('u'), _) => self.scroll_by(-page / 2),
            (KeyCode::Char('g' | '<') | KeyCode::Home, false) => self.top = 0,
            (KeyCode::Char('G' | '>') | KeyCode::End, false) => self.top = self.max_top(),
            (KeyCode::Char('/'), false) => self.input = Input::Search(String::new()),
            (KeyCode::Char('n'), false) => self.find(true, false),
            (KeyCode::Char('N'), false) => self.find(false, false),
            _ => {}
        }
        true
    }

    /// Jumps to the next/previous line matching the query, wrapping around.
    fn find(&mut self, forward: bool, include_current: bool) {
        let Some(q) = &self.query else { return };
        let n = self.doc.plain.len();
        if n == 0 {
            return;
        }
        let start = if include_current { 0 } else { 1 };
        for i in start..=n {
            let idx = if forward { (self.top + i) % n } else { (self.top + n - i % n) % n };
            if find_in(&self.doc.plain[idx], q).is_some() {
                self.top = idx.min(self.max_top());
                return;
            }
        }
        self.message = Some(format!("Pattern not found: {q}"));
    }

    fn draw(&self) -> io::Result<()> {
        let mut s = String::with_capacity(self.cols * self.rows * 2);
        s.push_str("\x1b[?2026h");
        for i in 0..self.page() {
            s.push_str(&format!("\x1b[{};1H", i + 1));
            if let Some(line) = self.doc.lines.get(self.top + i) {
                match &self.query {
                    Some(q) => s.push_str(&highlight(line, &self.doc.plain[self.top + i], q)),
                    None => s.push_str(line),
                }
            }
            s.push_str("\x1b[0m\x1b[K");
        }
        s.push_str(&format!("\x1b[{};1H", self.rows));
        s.push_str(&self.status_bar());
        s.push_str("\x1b[?2026l");
        let mut out = io::stdout().lock();
        out.write_all(s.as_bytes())?;
        out.flush()
    }

    fn status_bar(&self) -> String {
        let total = self.doc.lines.len();
        let last = (self.top + self.page()).min(total);
        let pos = if total <= self.page() {
            "all".to_string()
        } else if self.top == 0 {
            "top".to_string()
        } else if self.top >= self.max_top() {
            "end".to_string()
        } else {
            format!("{}%", last * 100 / total)
        };
        let right = format!(" {}-{} of {} lines  {:>4} ", self.top + 1, last, total, pos);
        let left = match (&self.input, &self.message) {
            (Input::Search(q), _) => format!(" /{q}█"),
            (_, Some(m)) => format!(" {m}"),
            _ => format!(" {}", self.opts.title),
        };
        let right_w = crate::style::str_width(&right);
        let left = truncate(&left, self.cols.saturating_sub(right_w + 1));
        let pad = self.cols.saturating_sub(crate::style::str_width(&left) + right_w);
        let bar = format!("{left}{}{right}", " ".repeat(pad));
        format!("\x1b[0;7m{}\x1b[0m", truncate(&bar, self.cols))
    }
}

fn truncate(s: &str, width: usize) -> String {
    if crate::style::str_width(s) <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = crate::style::char_width(c);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Case-insensitive (ASCII) unless the query has uppercase letters ("smart case").
fn find_in(hay: &str, q: &str) -> Option<usize> {
    if q.chars().any(|c| c.is_uppercase()) {
        hay.find(q)
    } else {
        hay.to_ascii_lowercase().find(&q.to_ascii_lowercase())
    }
}

/// Wraps every match of `q` in reverse video. `plain` is `line` with escapes removed;
/// escape sequences inside a match are followed by a re-enable of reverse video.
fn highlight(line: &str, plain: &str, q: &str) -> String {
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(p) = find_in(&plain[from..], q) {
        let start = from + p;
        ranges.push((start, start + q.len()));
        from = start + q.len().max(1);
        if from >= plain.len() {
            break;
        }
    }
    if ranges.is_empty() {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + ranges.len() * 10);
    let mut pos = 0; // byte offset into `plain`
    let mut ri = 0;
    let mut inside = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        if let Some(len) = escape_len(&bytes[i..]) {
            out.push_str(&line[i..i + len]);
            if inside {
                out.push_str("\x1b[7m");
            }
            i += len;
            continue;
        }
        if ri < ranges.len() && pos == ranges[ri].0 && !inside {
            out.push_str("\x1b[7m");
            inside = true;
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
        pos += ch.len_utf8();
        if inside && pos >= ranges[ri].1 {
            out.push_str("\x1b[27m");
            inside = false;
            ri += 1;
        }
    }
    if inside {
        out.push_str("\x1b[27m");
    }
    out
}

/// Length of the CSI or OSC escape sequence at the start of `b`, if any.
fn escape_len(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&0x1b) {
        return None;
    }
    match b.get(1) {
        Some(b'[') => {
            let end = b[2..].iter().position(|c| (0x40..=0x7e).contains(c))?;
            Some(end + 3)
        }
        Some(b']') => {
            let mut j = 2;
            while j < b.len() {
                if b[j] == 0x07 {
                    return Some(j + 1);
                }
                if b[j] == 0x1b && b.get(j + 1) == Some(&b'\\') {
                    return Some(j + 2);
                }
                j += 1;
            }
            Some(b.len())
        }
        _ => Some(1),
    }
}

pub fn strip_ansi(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if let Some(len) = escape_len(&b[i..]) {
            i += len;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_and_highlights() {
        let line = "a \x1b]8;;http://x\x1b\\\x1b[1mBold\x1b[0m\x1b]8;;\x1b\\ word";
        let plain = strip_ansi(line);
        assert_eq!(plain, "a Bold word");
        let h = highlight(line, &plain, "old w");
        assert_eq!(strip_ansi(&h), plain);
        assert!(h.contains("B\x1b[7mold\x1b[0m\x1b[7m"));
        assert!(h.ends_with(" w\x1b[27mord"));
    }
}
