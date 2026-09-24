# mdv

A fast terminal viewer for CommonMark / GitHub-flavored Markdown.

```sh
cargo install --path .
mdv README.md            # page through a file
curl -s …/README.md | mdv  # or read stdin
```

## Features

- CommonMark via `pulldown-cmark`, plus GFM tables, task lists, strikethrough,
  footnotes, alerts (`> [!NOTE]`), definition lists; front matter is hidden
- Syntax-highlighted code blocks (bat's grammars and themes), loaded lazily
- Tables with alignment, row dividers, and wrapped cells; column widths are chosen to
  minimize the table's height, with a stacked layout on very narrow terminals
- Clickable OSC 8 hyperlinks; relative links resolve to `file://` URLs
- Unicode-width-aware word wrapping (CJK, emoji)
- Built-in pager when output exceeds the screen: mouse-wheel scrolling, search,
  status bar, re-layout on resize (set `$MDV_PAGER` to use an external pager)

## Options

| Flag | Description |
|------|-------------|
| `-w, --width N` | Render width (default: terminal width) |
| `-t, --theme NAME` | Code theme (`--list-themes`; env `MDV_THEME`) |
| `--light` | Palette for light backgrounds (auto-detected from `COLORFGBG`) |
| `--color`, `--hyperlinks`, `--paging` | `auto` \| `always` \| `never` |
| `-P, --no-pager` | Print directly |
| `--mouse` | Capture the mouse in the pager (see below) |

`NO_COLOR` is honored. Truecolor is used when `COLORTERM=truecolor`, 256 colors otherwise.

## Pager keys

| Keys | Action |
|------|--------|
| `j` `k` `↓` `↑` `Enter`, mouse wheel | Scroll a line (wheel: 3 lines) |
| `Space` `f` `PgDn` / `b` `PgUp` | Page down / up |
| `d` / `u` | Half page down / up |
| `g` `Home` / `G` `End` | Top / bottom |
| `/` then `n` / `N` | Search (smart case), next / previous match |
| `q` `Esc` `Ctrl-C` | Quit |

By default the mouse wheel works through the terminal's *alternate scroll mode*
(wheel → arrow keys), which keeps text selection and link clicking normal.
If your terminal doesn't support that, `--mouse` captures the mouse directly.
