---
title: front matter should be hidden
---

# mdv demo

A **fast** terminal *markdown* viewer with `inline code`, ~~strikethrough~~, and [links](https://commonmark.org "CommonMark"). Autolink: <https://example.com>, email <me@example.com>, relative [readme](../README.md#usage). This paragraph is long enough that it should wrap nicely at the configured width without breaking words apart.

## Lists

- first item
- second item with a `code` span
  - nested bullet
    - deeper
      1. ordered deep
- [ ] todo task
- [x] done task

8. eight
9. nine
10. ten

<!-- end list -->

1. loose item one

   second paragraph in item one

2. loose item two

## Quotes

> A block quote with **bold** text
> spanning lines.
>
> > Nested quote.

> [!WARNING]
> GitHub alert: be careful.

## Code

```rust
use std::collections::HashMap;

/// Doc comment
fn main() {
	let mut m: HashMap<&str, i32> = HashMap::new(); // tab-indented
    m.insert("answer", 42);
    println!("{:?}", m);
}
```

```python
def fib(n: int) -> int:
    return n if n < 2 else fib(n - 1) + fib(n - 2)
```

    indented code block

```
plain fence
```

## Table

| Left | Center | Right | Notes |
|:-----|:------:|------:|-------|
| a | b | 1.00 | short |
| longer cell | **bold** | 12345.67 | This note is quite long and should wrap inside its cell when the terminal is narrow, see [link](https://x.org). |
| `code` | ü 日本語 | 3 | |

## Misc

Text with footnote[^1] and hard  
break. Inline <kbd>Ctrl</kbd>+<kbd>C</kbd> and <b>html bold</b>.<br>After br.

![alt text](img/logo.png)

Term
: Definition of the term.

<!-- hidden comment -->
<div align="center">
  raw html block
</div>

---

[^1]: The footnote text.
