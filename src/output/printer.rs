use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::output::style::{IconSet, OutputStyle};

/// Kind of AI agent activity to display during streaming operations.
#[derive(Debug, Clone, Copy)]
pub enum ActivityKind {
    Reading,
    Editing,
    Searching,
    Running,
}

struct GlyphSet {
    action: &'static str,
    success: &'static str,
    warning: &'static str,
    error: &'static str,
    removal: &'static str,
    dry_run: &'static str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LineLayout {
    TopLevel,
    Detail,
    SubDetail,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum AnsiStyle {
    Action,
    Success,
    Warning,
    Error,
    Activity,
}

pub struct Printer {
    style: OutputStyle,
}

pub(crate) struct LoadingIndicator {
    text: Option<Arc<Mutex<String>>>,
    stop: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl Printer {
    pub const fn new(style: OutputStyle) -> Self {
        Self { style }
    }

    #[must_use]
    pub const fn style(&self) -> OutputStyle {
        self.style
    }

    pub fn action(&self, text: &str) {
        println!("{}", self.action_line(text));
    }

    pub fn success(&self, text: &str) {
        println!("{}", self.success_line(text));
    }

    pub fn warn(&self, text: &str) {
        println!("{}", self.warn_line(text));
    }

    pub fn error(&self, text: &str) {
        eprintln!("{}", self.error_line(text));
    }

    pub fn removal(&self, text: &str) {
        println!("{}", self.removal_line(text));
    }

    pub fn heading(text: &str) {
        println!("\n{}{text}", LineLayout::Detail.indent());
    }

    pub fn body(text: &str) {
        println!("{}{text}", LineLayout::Detail.indent());
    }

    pub fn sub_detail(text: &str) {
        println!("{}{text}", LineLayout::SubDetail.indent());
    }

    pub fn dry_run_banner(&self) {
        println!("{}", self.dry_run_line());
    }

    pub fn activity(&self, kind: ActivityKind, text: &str) {
        let glyph = match kind {
            ActivityKind::Reading => "%",
            ActivityKind::Searching => "@",
            ActivityKind::Editing => "~",
            ActivityKind::Running => ">",
        };
        println!(
            "{}",
            self.paint(
                layout_line(LineLayout::Detail, glyph, text),
                AnsiStyle::Activity
            )
        );
    }

    #[must_use]
    pub(crate) fn loading(&self, text: &str) -> LoadingIndicator {
        if !loading_enabled(self.style, io::stderr().is_terminal()) {
            return LoadingIndicator::disabled();
        }

        let (stop, stopped) = mpsc::channel();
        let text = Arc::new(Mutex::new(text.to_string()));
        let thread_text = Arc::clone(&text);
        let frames = loading_frames(self.style.icon_set);
        let color = self.style.color;
        let handle = thread::spawn(move || {
            let mut index = 0usize;
            loop {
                let frame = frames[index % frames.len()];
                let text = thread_text
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default();
                eprint!(
                    "\r\x1b[2K{}{}{} {text}{}",
                    ansi_prefix(color, AnsiStyle::Activity),
                    LineLayout::TopLevel.indent(),
                    frame,
                    ansi_reset(color),
                );
                let _ = io::stderr().flush();
                index += 1;
                match stopped.recv_timeout(Duration::from_millis(120)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
        });

        LoadingIndicator {
            text: Some(text),
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    pub(crate) fn with_loading<T>(
        &self,
        text: &str,
        run: impl FnOnce(&LoadingIndicator) -> T,
    ) -> T {
        let loading = self.loading(text);
        let result = run(&loading);
        loading.finish();
        result
    }

    pub fn detail(text: &str) {
        println!("{}{text}", LineLayout::Detail.indent());
    }

    pub fn stream_line(text: &str, indent: &str, width: usize) {
        for segment in wrapped_segments(text, width.saturating_sub(indent.len()).max(20)) {
            println!("{indent}{segment}");
        }
    }

    pub fn confirm(prompt: &str, default_yes: bool) -> bool {
        let suffix = if default_yes { " [Y/n]: " } else { " [y/N]: " };
        print!("{}{prompt}{suffix}", LineLayout::Detail.indent());
        let _ = io::stdout().flush();
        let mut line = String::new();
        let read_result = io::stdin().lock().read_line(&mut line);
        if !io::stdin().is_terminal() {
            println!();
        }
        match read_result {
            Ok(0) | Err(_) => default_yes,
            Ok(_) => parse_confirm_response(&line, default_yes),
        }
    }

    const fn glyphs(&self) -> GlyphSet {
        match self.style.icon_set {
            IconSet::Unicode => GlyphSet {
                action: "➜",
                success: "✔",
                warning: "!",
                error: "✘",
                removal: "-",
                dry_run: "~",
            },
            IconSet::Minimal => GlyphSet {
                action: ">",
                success: "+",
                warning: "!",
                error: "x",
                removal: "-",
                dry_run: "~",
            },
        }
    }

    fn action_line(&self, text: &str) -> String {
        self.paint(
            layout_line_with_prefix("\n", LineLayout::TopLevel, self.glyphs().action, text),
            AnsiStyle::Action,
        )
    }

    fn success_line(&self, text: &str) -> String {
        self.paint(
            layout_line(LineLayout::TopLevel, self.glyphs().success, text),
            AnsiStyle::Success,
        )
    }

    fn warn_line(&self, text: &str) -> String {
        self.paint(
            layout_line(LineLayout::TopLevel, self.glyphs().warning, text),
            AnsiStyle::Warning,
        )
    }

    fn error_line(&self, text: &str) -> String {
        self.paint(
            layout_line(LineLayout::TopLevel, self.glyphs().error, text),
            AnsiStyle::Error,
        )
    }

    fn removal_line(&self, text: &str) -> String {
        self.paint(
            layout_line(LineLayout::TopLevel, self.glyphs().removal, text),
            AnsiStyle::Error,
        )
    }

    fn dry_run_line(&self) -> String {
        self.paint(
            layout_line_with_prefix(
                "\n",
                LineLayout::TopLevel,
                self.glyphs().dry_run,
                "Dry Run (no changes will be made)",
            ),
            AnsiStyle::Warning,
        )
    }

    fn paint(&self, text: String, style: AnsiStyle) -> String {
        if self.style.color {
            format!("{}{text}{}", ansi_prefix(true, style), ansi_reset(true))
        } else {
            text
        }
    }
}

#[cfg(test)]
fn loading_line(color: bool, frame: &str, text: &str) -> String {
    format!(
        "{}{}{} {text}{}",
        ansi_prefix(color, AnsiStyle::Activity),
        LineLayout::TopLevel.indent(),
        frame,
        ansi_reset(color),
    )
}

fn layout_line(layout: LineLayout, glyph: &str, text: &str) -> String {
    layout_line_with_prefix("", layout, glyph, text)
}

fn layout_line_with_prefix(prefix: &str, layout: LineLayout, glyph: &str, text: &str) -> String {
    format!("{prefix}{}{} {text}", layout.indent(), glyph)
}

impl LineLayout {
    const fn indent(self) -> &'static str {
        match self {
            Self::TopLevel => "",
            Self::Detail => "  ",
            Self::SubDetail => "    ",
        }
    }
}

const fn ansi_prefix(color: bool, style: AnsiStyle) -> &'static str {
    if !color {
        return "";
    }
    match style {
        AnsiStyle::Action => "\x1b[36m",
        AnsiStyle::Success => "\x1b[32m",
        AnsiStyle::Warning => "\x1b[33m",
        AnsiStyle::Error => "\x1b[1;31m",
        AnsiStyle::Activity => "\x1b[35m",
    }
}

const fn ansi_reset(color: bool) -> &'static str {
    if color { "\x1b[0m" } else { "" }
}

impl LoadingIndicator {
    fn disabled() -> Self {
        Self {
            text: None,
            stop: None,
            handle: None,
        }
    }

    pub(crate) fn set_text(&self, text: &str) {
        if let Some(shared) = &self.text
            && let Ok(mut current) = shared.lock()
        {
            current.clear();
            current.push_str(text);
        }
    }

    pub(crate) fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LoadingIndicator {
    fn drop(&mut self) {
        self.stop();
    }
}

const fn loading_frames(icon_set: IconSet) -> &'static [&'static str] {
    match icon_set {
        IconSet::Unicode => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        IconSet::Minimal => &["-", "\\", "|", "/"],
    }
}

const fn loading_enabled(style: OutputStyle, stderr_is_terminal: bool) -> bool {
    !style.plain && stderr_is_terminal
}

fn parse_confirm_response(response: &str, default_yes: bool) -> bool {
    let trimmed = response.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return default_yes;
    }
    trimmed == "y" || trimmed == "yes"
}

fn wrapped_segments(line: &str, max_content: usize) -> Vec<&str> {
    if line.chars().count() <= max_content {
        return vec![line];
    }

    let mut out = Vec::new();
    let mut remaining = line;
    while remaining.chars().count() > max_content {
        let candidate = nth_char_boundary(remaining, max_content);
        let split = match remaining[..candidate].rfind(' ') {
            // Avoid producing tiny leading fragments like "File" when the first
            // meaningful split point is near the hard width boundary.
            Some(idx) if idx >= (candidate / 2) => idx,
            _ => candidate,
        }
        .max(1);
        out.push(&remaining[..split]);
        remaining = remaining[split..].trim_start();
        if remaining.is_empty() {
            return out;
        }
    }

    out.push(remaining);
    out
}

fn nth_char_boundary(input: &str, n: usize) -> usize {
    if input.chars().count() <= n {
        return input.len();
    }
    input
        .char_indices()
        .nth(n)
        .map_or(input.len(), |(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use super::{
        LineLayout, Printer, layout_line, loading_enabled, loading_frames, loading_line,
        parse_confirm_response, wrapped_segments,
    };
    use crate::output::style::{IconSet, OutputStyle};

    #[test]
    fn wrapped_segments_preserves_long_word_chunks() {
        let segments = wrapped_segments("alpha beta gamma delta", 8);
        assert_eq!(segments, vec!["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn printer_uses_unicode_glyphs_when_requested() {
        let printer = Printer::new(OutputStyle {
            plain: false,
            icon_set: IconSet::Unicode,
            color: false,
        });

        let glyphs = printer.glyphs();
        assert_eq!(glyphs.action, "➜");
        assert_eq!(glyphs.success, "✔");
        assert_eq!(glyphs.error, "✘");
    }

    #[test]
    fn printer_uses_minimal_glyphs_for_plain_mode() {
        let printer = Printer::new(OutputStyle {
            plain: true,
            icon_set: IconSet::Minimal,
            color: false,
        });

        let glyphs = printer.glyphs();
        assert_eq!(glyphs.action, ">");
        assert_eq!(glyphs.success, "+");
        assert_eq!(glyphs.error, "x");
    }

    #[test]
    fn warning_glyph_is_bang_for_both_icon_sets() {
        for icon_set in [IconSet::Unicode, IconSet::Minimal] {
            let printer = Printer::new(OutputStyle {
                plain: false,
                icon_set,
                color: false,
            });
            assert_eq!(printer.glyphs().warning, "!");
        }
    }

    #[test]
    fn success_line_uses_ansi_when_color_enabled() {
        let printer = Printer::new(OutputStyle {
            plain: false,
            icon_set: IconSet::Unicode,
            color: true,
        });
        let line = printer.success_line("ok");
        assert!(line.contains("\x1b[32m"));
        assert!(line.contains("\x1b[0m"));
    }

    #[test]
    fn success_line_has_no_ansi_when_color_disabled() {
        let printer = Printer::new(OutputStyle {
            plain: false,
            icon_set: IconSet::Unicode,
            color: false,
        });
        let line = printer.success_line("ok");
        assert!(!line.contains("\x1b["));
    }

    #[test]
    fn loading_policy_requires_non_plain_terminal_stderr() {
        let style = OutputStyle {
            plain: false,
            icon_set: IconSet::Minimal,
            color: false,
        };
        assert!(loading_enabled(style, true));
        assert!(!loading_enabled(style, false));
        assert!(!loading_enabled(
            OutputStyle {
                plain: true,
                ..style
            },
            true
        ));
    }

    #[test]
    fn loading_frames_follow_icon_set() {
        assert_eq!(loading_frames(IconSet::Minimal), &["-", "\\", "|", "/"]);
        assert!(loading_frames(IconSet::Unicode).contains(&"⠋"));
    }

    #[test]
    fn top_level_status_lines_start_at_column_zero() {
        let printer = Printer::new(OutputStyle {
            plain: false,
            icon_set: IconSet::Minimal,
            color: false,
        });

        assert!(printer.action_line("work").starts_with("\n> "));
        assert!(printer.success_line("ok").starts_with("+ "));
        assert!(printer.warn_line("warn").starts_with("! "));
        assert!(printer.error_line("fail").starts_with("x "));
        assert!(printer.removal_line("remove").starts_with("- "));
        assert!(printer.dry_run_line().starts_with("\n~ "));
        assert_eq!(loading_line(false, "|", "Sizing caches"), "| Sizing caches");
    }

    #[test]
    fn loading_line_keeps_spinner_unindented_with_color() {
        assert_eq!(
            loading_line(true, "⠋", "Sizing caches"),
            "\x1b[35m⠋ Sizing caches\x1b[0m"
        );
    }

    #[test]
    fn detail_layout_is_the_only_status_indent() {
        assert_eq!(layout_line(LineLayout::Detail, ">", "nested"), "  > nested");
        assert_eq!(
            layout_line(LineLayout::SubDetail, "-", "deeper"),
            "    - deeper"
        );
    }

    #[test]
    fn with_loading_returns_closure_value() {
        let printer = Printer::new(OutputStyle {
            plain: true,
            icon_set: IconSet::Minimal,
            color: false,
        });

        let value = printer.with_loading("Working", |loading| {
            loading.set_text("Still working");
            42
        });

        assert_eq!(value, 42);
    }

    #[test]
    fn confirm_response_accepts_y_and_yes() {
        assert!(parse_confirm_response("y\n", false));
        assert!(parse_confirm_response("Y\n", false));
        assert!(parse_confirm_response("yes\n", false));
        assert!(parse_confirm_response("YES\n", false));
    }

    #[test]
    fn confirm_response_rejects_n_and_no() {
        assert!(!parse_confirm_response("n\n", true));
        assert!(!parse_confirm_response("N\n", true));
        assert!(!parse_confirm_response("no\n", true));
    }

    #[test]
    fn confirm_response_empty_uses_default() {
        assert!(parse_confirm_response("\n", true));
        assert!(!parse_confirm_response("\n", false));
        assert!(parse_confirm_response("  \n", true));
        assert!(!parse_confirm_response("  \n", false));
    }
}
