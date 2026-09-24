mod highlight;
mod pager;
mod render;
mod style;
mod wrap;

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::{Parser, ValueEnum};

use style::ColorMode;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum When {
    Auto,
    Always,
    Never,
}

/// Render CommonMark/GFM markdown in the terminal.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Markdown files to render ("-" or none for stdin).
    files: Vec<PathBuf>,

    /// Render width in columns (default: terminal width, or 80 when not a terminal).
    #[arg(short, long)]
    width: Option<usize>,

    /// When to use colors and styles.
    #[arg(long, value_enum, default_value_t = When::Auto)]
    color: When,

    /// When to emit clickable OSC 8 hyperlinks (otherwise URLs are printed after link text).
    #[arg(long, value_enum, default_value_t = When::Auto)]
    hyperlinks: When,

    /// When to show output in the built-in pager (or $MDV_PAGER, if set).
    /// "auto" pages when the output is taller than the terminal.
    #[arg(long, value_enum, default_value_t = When::Auto)]
    paging: When,

    /// Capture the mouse in the pager for wheel scrolling on terminals that don't
    /// translate the wheel into arrow keys (selecting text/clicking links then needs Shift).
    #[arg(long)]
    mouse: bool,

    /// Shorthand for --paging=never.
    #[arg(short = 'P', long)]
    no_pager: bool,

    /// Syntax highlighting theme for code blocks.
    #[arg(short, long, env = "MDV_THEME")]
    theme: Option<String>,

    /// Use colors suited to a light terminal background.
    #[arg(long)]
    light: bool,

    /// List available code themes and exit.
    #[arg(long)]
    list_themes: bool,
}

fn detect_color_mode() -> ColorMode {
    match std::env::var("COLORTERM").as_deref() {
        Ok("truecolor" | "24bit") => ColorMode::TrueColor,
        _ => ColorMode::Ansi256,
    }
}

/// COLORFGBG is "fg;bg"; a bg of 7 or 15 means a light terminal.
fn terminal_is_light() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok()))
        .is_some_and(|bg| bg == 7 || bg == 15)
}

fn read_input(path: &Path) -> io::Result<String> {
    let bytes = if path.as_os_str() == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        std::fs::read(path)?
    };
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// Pipes output through a user-configured external pager.
fn external_page(pager: &str, output: &str) -> io::Result<()> {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", pager]);
    if std::env::var_os("LESS").is_none() {
        cmd.env("LESS", "FRX");
    }
    let mut child = cmd.stdin(Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // A broken pipe here just means the user quit the pager early.
        let _ = stdin.write_all(output.as_bytes());
    }
    child.wait()?;
    Ok(())
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    if args.list_themes {
        for name in highlight::theme_names() {
            println!("{name}");
        }
        return Ok(());
    }

    let stdout_tty = io::stdout().is_terminal();
    let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let mode = match args.color {
        When::Always => detect_color_mode(),
        When::Never => ColorMode::None,
        When::Auto if stdout_tty && !no_color_env => detect_color_mode(),
        When::Auto => ColorMode::None,
    };
    let hyperlinks = match args.hyperlinks {
        When::Always => true,
        When::Never => false,
        When::Auto => stdout_tty && std::env::var("TERM").map_or(true, |t| t != "dumb" && t != "linux"),
    };
    let term_size = terminal_size::terminal_size_of(io::stdout());
    let width = args
        .width
        .or_else(|| term_size.map(|(w, _)| w.0 as usize))
        .unwrap_or(80)
        .max(20);
    let light = args.light || terminal_is_light();
    let theme = args.theme.unwrap_or_else(|| {
        let t = if light { highlight::DEFAULT_LIGHT_THEME } else { highlight::DEFAULT_DARK_THEME };
        t.to_owned()
    });

    let files = if args.files.is_empty() {
        if io::stdin().is_terminal() {
            return Err("no input files (pass a path, or pipe markdown on stdin); see --help".into());
        }
        vec![PathBuf::from("-")]
    } else {
        args.files
    };

    let mut docs = Vec::new();
    for path in &files {
        let src = read_input(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let base_dir = if path.as_os_str() == "-" {
            std::env::current_dir().ok()
        } else {
            std::fs::canonicalize(path).ok().and_then(|p| p.parent().map(Path::to_path_buf))
        };
        docs.push((src, base_dir));
    }
    let render_all = |width: usize| -> Result<String, String> {
        let mut output = String::new();
        for (i, (src, base_dir)) in docs.iter().enumerate() {
            let cfg = render::Config {
                width: width.max(20),
                mode,
                hyperlinks,
                light,
                theme: theme.clone(),
                base_dir: base_dir.clone(),
            };
            if i > 0 {
                output.push('\n');
            }
            output.push_str(&render::render(src, cfg)?);
        }
        Ok(output)
    };
    let output = render_all(width)?;

    let paging = stdout_tty
        && match (args.paging, args.no_pager) {
            (_, true) | (When::Never, _) => false,
            (When::Always, _) => true,
            (When::Auto, _) => term_size.is_some_and(|(_, h)| output.lines().count() >= h.0 as usize),
        };
    if paging {
        if let Some(p) = std::env::var("MDV_PAGER").ok().filter(|p| !p.trim().is_empty()) {
            return external_page(&p, &output).map_err(|e| format!("{p}: {e}"));
        }
        let title = files
            .iter()
            .map(|p| if p.as_os_str() == "-" { "<stdin>".into() } else { p.display().to_string() })
            .collect::<Vec<_>>()
            .join(", ");
        let rerender: Option<&dyn Fn(usize) -> Result<String, String>> =
            if args.width.is_none() { Some(&render_all) } else { None };
        return pager::run(&output, pager::Options { title: &title, mouse: args.mouse, rerender });
    }
    let mut out = io::stdout().lock();
    match out.write_all(output.as_bytes()).and_then(|_| out.flush()) {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("mdv: {e}");
        std::process::exit(1);
    }
}
